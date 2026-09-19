//! Native file previews through the system's own preview panel.
//!
//! This is the panel Finder opens on Space: it pages through documents,
//! scrolls them, plays video, and covers every format the system knows,
//! without this app rendering any of it. It opens as its own window, which is
//! where macOS users expect a Quick Look preview to appear.

use std::path::Path;

/// Whether this platform has a system preview panel.
pub fn available() -> bool {
    cfg!(target_os = "macos")
}

/// Opens the system preview on these files, starting at `at`.
///
/// The panel arrows through the whole set, so a chat's attachments are handed
/// over together rather than one at a time.
pub fn show(paths: &[&Path], at: usize) -> bool {
    imp::show(paths, at)
}

#[cfg(target_os = "macos")]
mod imp {
    use std::cell::RefCell;
    use std::path::Path;

    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2::{AnyThread, DefinedClass, MainThreadMarker, define_class, msg_send};
    use objc2_app_kit::NSWindow;
    use objc2_foundation::{NSInteger, NSObject, NSObjectProtocol, NSString, NSURL};
    use objc2_quick_look_ui::{QLPreviewItem, QLPreviewPanel, QLPreviewPanelDataSource};

    /// The files the shared panel is currently showing.
    struct Items {
        urls: Vec<Retained<NSURL>>,
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[name = "MerlinPreviewSource"]
        #[ivars = Items]
        struct Source;

        unsafe impl NSObjectProtocol for Source {}

        unsafe impl QLPreviewPanelDataSource for Source {
            #[unsafe(method(numberOfPreviewItemsInPreviewPanel:))]
            fn count(&self, _panel: Option<&QLPreviewPanel>) -> NSInteger {
                self.ivars().urls.len() as NSInteger
            }

            #[unsafe(method(previewPanel:previewItemAtIndex:))]
            fn item(
                &self,
                _panel: Option<&QLPreviewPanel>,
                index: NSInteger,
            ) -> *mut ProtocolObject<dyn QLPreviewItem> {
                // The panel expects an autoreleased item, not one handed over.
                let Some(url) = usize::try_from(index)
                    .ok()
                    .and_then(|index| self.ivars().urls.get(index))
                else {
                    return std::ptr::null_mut();
                };
                Retained::autorelease_return(ProtocolObject::from_retained(url.clone()))
            }
        }
    );

    impl Source {
        fn new(urls: Vec<Retained<NSURL>>) -> Retained<Self> {
            let this = Self::alloc().set_ivars(Items { urls });
            unsafe { msg_send![super(this), init] }
        }
    }

    thread_local! {
        /// The panel does not retain its data source, so this keeps it alive.
        static SOURCE: RefCell<Option<Retained<Source>>> = const { RefCell::new(None) };
    }

    /// Opens the system preview panel on these files.
    pub(super) fn show(paths: &[&Path], at: usize) -> bool {
        let Some(mtm) = MainThreadMarker::new() else {
            return false;
        };
        let urls: Vec<Retained<NSURL>> = paths
            .iter()
            .filter_map(|path| path.to_str())
            .map(|path| NSURL::fileURLWithPath(&NSString::from_str(path)))
            .collect();
        let count = urls.len();
        if count == 0 {
            return false;
        }
        let source = Source::new(urls);
        let Some(panel) = (unsafe { QLPreviewPanel::sharedPreviewPanel(mtm) }) else {
            return false;
        };
        unsafe {
            panel.setDataSource(Some(ProtocolObject::from_ref(&*source)));
            panel.reloadData();
            // Opens on the attachment that was clicked; its arrow keys walk
            // the rest of the chat's media from there.
            panel.setCurrentPreviewItemIndex(at.min(count - 1) as NSInteger);
        }
        SOURCE.with(|held| held.replace(Some(source)));
        let window: &NSWindow = &panel;
        window.makeKeyAndOrderFront(None);
        true
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use std::path::Path;

    pub(super) fn show(_paths: &[&Path], _at: usize) -> bool {
        false
    }
}
