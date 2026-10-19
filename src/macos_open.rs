//! Captures the file path macOS hands an app via Finder's "Open With" or a
//! double-click.
//!
//! Unlike a direct CLI launch (`cargo run -- path`, or this app's own
//! File → Open dialog), a Finder-initiated open never appears in
//! `std::env::args()`: macOS launches the app with no arguments at all and
//! instead sends it an Apple Event, which AppKit surfaces as
//! `application:openURLs:` on the app's `NSApplicationDelegate`.
//!
//! The obvious way to catch that is `NSApplication::setDelegate` with our
//! own delegate object. That's wrong for this app: winit 0.30.13's
//! `EventLoop::new` already installs its *own* delegate (see winit's
//! `platform_impl::macos::event_loop::EventLoop::new`), and several of
//! winit's internals — its `sendEvent:` override among them — fetch that
//! exact object back via `delegate.is_kind_of::<ApplicationDelegate>()`,
//! panicking (and aborting the process) the moment that check fails. Our
//! own delegate would fail it instantly, so this module never calls
//! `setDelegate`. Instead it leaves winit's delegate object in place and
//! uses the Objective-C runtime to graft one extra method —
//! `application:openURLs:` — directly onto its class. This is the same
//! method-injection technique winit itself uses to override `sendEvent:`.

use std::path::PathBuf;
use std::sync::Mutex;

use objc2::ffi::{class_addMethod, object_getClass};
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::sel;
use objc2_app_kit::{NSApplication, NSApplicationDelegate};
use objc2_foundation::{MainThreadMarker, NSArray, NSURL};

static PENDING_OPENS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static REPAINT_CONTEXT: Mutex<Option<egui::Context>> = Mutex::new(None);

/// The `application:openURLs:` implementation, injected onto winit's
/// delegate class by [`install`]. `_this` and `_cmd` are the implicit
/// receiver/selector AppKit passes to every Objective-C method.
extern "C" fn application_open_urls(
    _this: &AnyObject,
    _cmd: Sel,
    _application: &NSApplication,
    urls: &NSArray<NSURL>,
) {
    let paths: Vec<PathBuf> = urls
        .iter()
        .filter_map(|url| unsafe { url.path() })
        .map(|path| PathBuf::from(path.to_string()))
        .collect();
    if paths.is_empty() {
        return;
    }
    PENDING_OPENS.lock().unwrap().extend(paths);
    // Wake the app if it's sitting idle: egui only repaints on demand, and
    // without this an already-running app wouldn't notice the new path
    // until some unrelated input arrived.
    if let Some(ctx) = &*REPAINT_CONTEXT.lock().unwrap() {
        ctx.request_repaint();
    }
}

/// Grafts `application:openURLs:` onto winit's own `NSApplicationDelegate`.
///
/// Must run after the winit event loop is built (so winit's delegate
/// already exists to graft onto) but before it starts running (so the
/// method is in place before the run loop dispatches the launch-time Apple
/// Event that a Finder double-click / "Open With" launch delivers).
pub fn install() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    let Some(delegate) = (unsafe { app.delegate() }) else {
        return;
    };

    // SAFETY: `delegate` is a valid, live Objective-C object (AppKit itself
    // handed it to us). `object_getClass` is a read-only runtime query.
    let delegate_ptr = (&*delegate as *const ProtocolObject<dyn NSApplicationDelegate>)
        .cast::<objc2::ffi::objc_object>();
    let class_ptr = unsafe { object_getClass(delegate_ptr) }.cast_mut();

    let selector = sel!(application:openURLs:);
    // SAFETY: `application_open_urls` has the signature AppKit expects for
    // `application:openURLs:` (self, _cmd, then the method's two object
    // arguments), matching the `v@:@@` type encoding below. `class_ptr` is
    // non-null (it came from a valid live object). Adding a method that
    // doesn't already exist on the class is exactly what `class_addMethod`
    // is for.
    unsafe {
        class_addMethod(
            class_ptr,
            selector.as_ptr(),
            Some(std::mem::transmute::<
                extern "C" fn(&AnyObject, Sel, &NSApplication, &NSArray<NSURL>),
                unsafe extern "C" fn(),
            >(application_open_urls)),
            c"v@:@@".as_ptr(),
        );
    }
}

/// Lets the running app wake itself up as soon as a path arrives via
/// [`install`]'s injected method, even while otherwise idle.
pub fn set_repaint_context(ctx: egui::Context) {
    *REPAINT_CONTEXT.lock().unwrap() = Some(ctx);
}

/// Drains any file paths Finder has asked this app to open since the last
/// call, oldest first.
pub fn take_pending_opens() -> Vec<PathBuf> {
    std::mem::take(&mut *PENDING_OPENS.lock().unwrap())
}
