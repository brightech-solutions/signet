//! Captures the file path macOS hands an app via Finder's "Open With" or a
//! double-click.
//!
//! Unlike a direct CLI launch (`cargo run -- path`, or this app's own
//! File → Open dialog), a Finder-initiated open never appears in
//! `std::env::args()`: macOS launches the app with no arguments at all and
//! instead sends it an Apple Event, which AppKit surfaces as
//! `application:openURLs:` on the app's `NSApplicationDelegate`. Winit
//! deliberately never installs a delegate of its own (see its
//! `platform::macos` module docs), so nothing in the dependency stack
//! otherwise listens for this event — this module installs a minimal
//! delegate purely to catch it.

use std::path::PathBuf;
use std::sync::Mutex;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{ClassType, DeclaredClass, declare_class, msg_send_id, mutability};
use objc2_app_kit::{NSApplication, NSApplicationDelegate};
use objc2_foundation::{MainThreadMarker, NSArray, NSObject, NSObjectProtocol, NSURL};

static PENDING_OPENS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static REPAINT_CONTEXT: Mutex<Option<egui::Context>> = Mutex::new(None);

declare_class!(
    struct OpenUrlsDelegate;

    unsafe impl ClassType for OpenUrlsDelegate {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "SignetOpenUrlsDelegate";
    }

    impl DeclaredClass for OpenUrlsDelegate {}

    unsafe impl NSObjectProtocol for OpenUrlsDelegate {}

    unsafe impl NSApplicationDelegate for OpenUrlsDelegate {
        #[method(application:openURLs:)]
        fn application_open_urls(&self, _application: &NSApplication, urls: &NSArray<NSURL>) {
            let paths: Vec<PathBuf> = urls
                .iter()
                .filter_map(|url| unsafe { url.path() })
                .map(|path| PathBuf::from(path.to_string()))
                .collect();
            if paths.is_empty() {
                return;
            }
            PENDING_OPENS.lock().unwrap().extend(paths);
            // Wake the app if it's sitting idle: egui only repaints on
            // demand, and without this an already-running app wouldn't
            // notice the new path until some unrelated input arrived.
            if let Some(ctx) = &*REPAINT_CONTEXT.lock().unwrap() {
                ctx.request_repaint();
            }
        }
    }
);

impl OpenUrlsDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        unsafe { msg_send_id![super(mtm.alloc().set_ivars(())), init] }
    }
}

/// Installs the delegate on the shared `NSApplication`.
///
/// Must run after the winit event loop is built but before it starts
/// running: AppKit requires `NSApplication::sharedApplication` to be called
/// after `EventLoop::new`/`build` (see winit's `platform::macos` docs), and
/// the delegate must already be in place before the run loop starts
/// dispatching launch-time Apple Events — which is exactly when a Finder
/// double-click / "Open With" launch delivers its file path.
pub fn install() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let delegate = OpenUrlsDelegate::new(mtm);
    let app = NSApplication::sharedApplication(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    // `NSApplication.delegate` holds a weak reference, so something must
    // own this delegate for the rest of the process's life — which is
    // exactly how long we want it listening.
    std::mem::forget(delegate);
}

/// Lets the running app wake itself up as soon as a path arrives via
/// [`install`]'s delegate, even while otherwise idle.
pub fn set_repaint_context(ctx: egui::Context) {
    *REPAINT_CONTEXT.lock().unwrap() = Some(ctx);
}

/// Drains any file paths Finder has asked this app to open since the last
/// call, oldest first.
pub fn take_pending_opens() -> Vec<PathBuf> {
    std::mem::take(&mut *PENDING_OPENS.lock().unwrap())
}
