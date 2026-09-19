//! Pictures decoded at the size they are drawn.
//!
//! egui's image loader ignores the size it is asked for, so pointing it at a
//! photo decodes it at camera resolution and keeps it, once as file bytes,
//! once as pixels and once as a texture. A 12 megapixel photo costs about
//! 150 MB that way, and nothing evicts it. Decoding here instead costs about
//! one megabyte for the same picture on screen, and the cache has a ceiling.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Pixels the cache may hold before it evicts. Roughly 100 MB of RGBA, which
/// is a few dozen pictures at the size they are drawn.
const MAX_RESIDENT_PIXELS: usize = 25_000_000;

/// Decoders allowed at once.
///
/// Each one briefly holds a whole decoded picture, because the decoder has no
/// way to read a JPEG at reduced size, so this bounds the spike as much as the
/// thread count. Two keeps a chat scrolling without a third full-size buffer.
const MAX_DECODERS: usize = 2;

/// Requested sizes are rounded up to this, so dragging a window edge does not
/// decode the same picture again at every intermediate width.
const BUCKET: u32 = 128;

static DECODING: AtomicUsize = AtomicUsize::new(0);

struct DecodeSlot;

impl Drop for DecodeSlot {
    fn drop(&mut self) {
        DECODING.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A decoded picture at a bucketed size.
type Pixels = (usize, usize, Vec<u8>);

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    path: PathBuf,
    bucket: u32,
}

enum Entry {
    Loading(Receiver<Option<Pixels>>),
    Ready {
        texture: egui::TextureHandle,
        pixels: usize,
        last_drawn: Instant,
    },
    Failed,
}

#[derive(Clone, Default)]
struct Cache(Arc<Mutex<HashMap<Key, Entry>>>);

fn cache(ctx: &egui::Context) -> Cache {
    ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<Cache>(egui::Id::new("scaled-pictures"))
            .clone()
    })
}

/// What to draw for a picture.
pub enum Thumb {
    Ready(egui::TextureHandle),
    /// Being decoded; show a placeholder of the expected size.
    Pending,
    /// Not a picture this build can decode.
    Failed,
}

/// A picture decoded to fit `max`, starting the decode on first ask.
pub fn scaled(ctx: &egui::Context, path: &Path, max: egui::Vec2) -> Thumb {
    let bucket = ((max.x.max(max.y).max(1.0) as u32).div_ceil(BUCKET) * BUCKET).max(BUCKET);
    let key = Key {
        path: path.to_path_buf(),
        bucket,
    };
    let cache = cache(ctx);
    let mut entries = cache
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    match entries.get_mut(&key) {
        Some(Entry::Ready {
            texture,
            last_drawn,
            ..
        }) => {
            *last_drawn = Instant::now();
            return Thumb::Ready(texture.clone());
        }
        Some(Entry::Failed) => return Thumb::Failed,
        Some(Entry::Loading(receiver)) => {
            let Ok(answer) = receiver.try_recv() else {
                return Thumb::Pending;
            };
            let entry = match answer {
                Some((width, height, rgba)) => {
                    let image = egui::ColorImage::from_rgba_unmultiplied([width, height], &rgba);
                    Entry::Ready {
                        texture: ctx.load_texture(
                            format!("picture-{}@{bucket}", key.path.display()),
                            image,
                            egui::TextureOptions::LINEAR,
                        ),
                        pixels: width * height,
                        last_drawn: Instant::now(),
                    }
                }
                None => Entry::Failed,
            };
            let drawn = match &entry {
                Entry::Ready { texture, .. } => Thumb::Ready(texture.clone()),
                _ => Thumb::Failed,
            };
            entries.insert(key, entry);
            evict(&mut entries);
            return drawn;
        }
        None => {}
    }

    if DECODING.load(Ordering::Acquire) >= MAX_DECODERS {
        ctx.request_repaint_after(std::time::Duration::from_millis(120));
        return Thumb::Pending;
    }
    DECODING.fetch_add(1, Ordering::AcqRel);
    let slot = DecodeSlot;
    let (sender, receiver) = channel();
    let owned = key.path.clone();
    let side = bucket;
    let waker = ctx.clone();
    let spawned = std::thread::Builder::new()
        .name("picture-decode".to_owned())
        .spawn(move || {
            let _slot = slot;
            let decoded =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| decode(&owned, side)))
                    .unwrap_or(None);
            let _ = sender.send(decoded);
            waker.request_repaint();
        });
    if spawned.is_err() {
        entries.insert(key, Entry::Failed);
        return Thumb::Failed;
    }
    entries.insert(key, Entry::Loading(receiver));
    Thumb::Pending
}

/// Decodes a picture and scales it to fit a square of `side`.
///
/// The full-size decode is the high-water mark of the whole cache, so it is
/// released before the small copy is taken rather than at the end of the call.
fn decode(path: &Path, side: u32) -> Option<Pixels> {
    let full = image::open(path).ok()?;
    let small = full.thumbnail(side, side);
    drop(full);
    let scaled = small.to_rgba8();
    Some((
        scaled.width() as usize,
        scaled.height() as usize,
        scaled.into_raw(),
    ))
}

/// Drops the least recently drawn pictures until the cache is under budget.
fn evict(entries: &mut HashMap<Key, Entry>) {
    let mut resident: usize = entries
        .values()
        .map(|entry| match entry {
            Entry::Ready { pixels, .. } => *pixels,
            _ => 0,
        })
        .sum();
    while resident > MAX_RESIDENT_PIXELS {
        let victim = entries
            .iter()
            .filter_map(|(key, entry)| match entry {
                Entry::Ready {
                    pixels, last_drawn, ..
                } => Some((key.clone(), *last_drawn, *pixels)),
                _ => None,
            })
            .min_by_key(|(_, last_drawn, _)| *last_drawn);
        let Some((victim, _, pixels)) = victim else {
            break;
        };
        entries.remove(&victim);
        resident -= pixels;
    }
}
