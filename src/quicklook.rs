//! Document previews from the operating system's own thumbnailer.
//!
//! macOS renders the first page of anything QuickLook understands, so PDFs,
//! Pages and Word files preview in the app without bundling a PDF engine. The
//! system framework costs nothing in the binary. Other platforms have no
//! equivalent worth its weight, so documents there keep opening in the
//! desktop's handler.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

/// Width, height, and straight-alpha RGBA.
type Pixels = (usize, usize, Vec<u8>);

/// Forces previews on for tests and the demo, which run where the system
/// thumbnailer does not exist.
#[cfg(any(test, feature = "demo"))]
static FORCED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Pretends the platform can preview documents, so the routing can be tested
/// away from macOS. The preview itself still reports nothing to show.
#[cfg(any(test, feature = "demo"))]
pub fn force_available(on: bool) {
    FORCED.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Whether this platform can preview documents in the app.
pub fn available() -> bool {
    #[cfg(any(test, feature = "demo"))]
    if FORCED.load(std::sync::atomic::Ordering::Relaxed) {
        return true;
    }
    cfg!(target_os = "macos")
}

/// What the viewer should draw for a document.
pub enum Preview {
    Ready(egui::TextureHandle),
    /// Being generated; show a spinner.
    Pending,
    /// No preview on this platform, or the thumbnailer declined the file.
    Unavailable,
}

enum Entry {
    Loading(Receiver<Option<Pixels>>),
    Ready(egui::TextureHandle),
    Missing,
}

#[derive(Clone, Default)]
struct Cache(Arc<Mutex<HashMap<PathBuf, Entry>>>);

fn cache(ctx: &egui::Context) -> Cache {
    ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<Cache>(egui::Id::new("quicklook"))
            .clone()
    })
}

/// Preview for a document, starting generation on first ask.
///
/// The thumbnailer answers on its own queue, so the work happens off the
/// interface thread and the frame that starts it draws a spinner.
pub fn preview(ctx: &egui::Context, path: &Path, side: f32) -> Preview {
    if !available() {
        return Preview::Unavailable;
    }
    let cache = cache(ctx);
    let mut entries = cache
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match entries.get(path) {
        Some(Entry::Ready(texture)) => return Preview::Ready(texture.clone()),
        Some(Entry::Missing) => return Preview::Unavailable,
        Some(Entry::Loading(receiver)) => {
            let Ok(answer) = receiver.try_recv() else {
                return Preview::Pending;
            };
            let entry = match answer {
                Some((width, height, rgba)) => {
                    let image = egui::ColorImage::from_rgba_premultiplied([width, height], &rgba);
                    Entry::Ready(ctx.load_texture(
                        format!("quicklook-{}", path.display()),
                        image,
                        egui::TextureOptions::LINEAR,
                    ))
                }
                None => Entry::Missing,
            };
            let drawn = match &entry {
                Entry::Ready(texture) => Preview::Ready(texture.clone()),
                _ => Preview::Unavailable,
            };
            entries.insert(path.to_path_buf(), entry);
            return drawn;
        }
        None => {}
    }
    let (sender, receiver) = channel();
    let owned = path.to_path_buf();
    let waker = ctx.clone();
    std::thread::Builder::new()
        .name("quicklook".to_owned())
        .spawn(move || {
            let _ = sender.send(imp::thumbnail(&owned, side));
            waker.request_repaint();
        })
        .ok();
    entries.insert(path.to_path_buf(), Entry::Loading(receiver));
    Preview::Pending
}

#[cfg(target_os = "macos")]
mod imp {
    use super::Pixels;
    use std::path::Path;
    use std::sync::mpsc;
    use std::time::Duration;

    use block2::RcBlock;
    use objc2::AnyThread;
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use objc2_core_graphics::{
        CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo,
        CGImageByteOrderInfo,
    };
    use objc2_foundation::{NSError, NSString, NSURL};
    use objc2_quick_look_thumbnailing::{
        QLThumbnailGenerationRequest, QLThumbnailGenerationRequestRepresentationTypes,
        QLThumbnailGenerator, QLThumbnailRepresentation,
    };

    /// Draws the thumbnail into a known RGBA layout, whatever the source format.
    fn pixels(representation: &QLThumbnailRepresentation) -> Option<Pixels> {
        let image = unsafe { representation.CGImage() };
        let width = CGImage::width(Some(&image));
        let height = CGImage::height(Some(&image));
        if width == 0 || height == 0 {
            return None;
        }
        let mut buffer = vec![0u8; width * height * 4];
        let space = CGColorSpace::new_device_rgb()?;
        // The buffer outlives the context, which is dropped before the vector
        // is returned, so the pointer stays valid for every draw into it.
        let context = unsafe {
            CGBitmapContextCreate(
                buffer.as_mut_ptr().cast(),
                width,
                height,
                8,
                width * 4,
                Some(&space),
                CGImageByteOrderInfo::OrderDefault.0 | CGImageAlphaInfo::PremultipliedLast.0,
            )
        }?;
        CGContext::draw_image(
            Some(&context),
            CGRect {
                origin: CGPoint { x: 0.0, y: 0.0 },
                size: CGSize {
                    width: width as f64,
                    height: height as f64,
                },
            },
            Some(&image),
        );
        drop(context);
        Some((width, height, buffer))
    }

    pub fn thumbnail(path: &Path, side: f32) -> Option<Pixels> {
        let text = NSString::from_str(path.to_str()?);
        let url = NSURL::fileURLWithPath(&text);
        let request = unsafe {
            QLThumbnailGenerationRequest::initWithFileAtURL_size_scale_representationTypes(
                QLThumbnailGenerationRequest::alloc(),
                &url,
                CGSize {
                    width: f64::from(side),
                    height: f64::from(side),
                },
                2.0,
                QLThumbnailGenerationRequestRepresentationTypes::Thumbnail,
            )
        };
        // The handler answers on a private queue, so waiting here blocks only
        // this thread.
        let (sender, receiver) = mpsc::channel::<Option<Pixels>>();
        let handler = RcBlock::new(
            move |representation: *mut QLThumbnailRepresentation, _error: *mut NSError| {
                let _ = sender.send(unsafe { representation.as_ref() }.and_then(pixels));
            },
        );
        unsafe {
            QLThumbnailGenerator::sharedGenerator()
                .generateBestRepresentationForRequest_completionHandler(&request, &handler);
        }
        receiver
            .recv_timeout(Duration::from_secs(10))
            .ok()
            .flatten()
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::Pixels;
    use std::path::Path;

    pub fn thumbnail(_path: &Path, _side: f32) -> Option<Pixels> {
        None
    }
}
