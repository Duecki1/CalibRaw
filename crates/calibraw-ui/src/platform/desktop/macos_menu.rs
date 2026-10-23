//! The macOS menu bar: the app menu and the Window menu.
//!
//! winit installs a default app menu titled after the executable
//! (`calibraw`). This one replaces it with the app's name, adds Settings…
//! (Cmd+,) and a Window menu, and leaves every other command to the in-app
//! toolbar. Standard items send AppKit actions through the responder chain;
//! Settings… sets a flag the app takes through
//! [`Platform::take_settings_request`](crate::platform::Platform::take_settings_request).

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, Sel};
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem, NSWindow};
use objc2_foundation::{ns_string, NSString};
use std::cell::OnceCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

const APP_NAME: &str = "CalibRaw";

static SETTINGS_REQUESTED: AtomicBool = AtomicBool::new(false);
type Wake = Box<dyn Fn() + Send + Sync>;
static WAKE_UI: Mutex<Option<Wake>> = Mutex::new(None);

thread_local! {
    /// A menu item holds its target weakly, so the target lives as long as
    /// the menu bar: until the process ends.
    static TARGET: OnceCell<Retained<MenuTarget>> = const { OnceCell::new() };
}

define_class!(
    // SAFETY:
    // - The superclass NSObject does not have any subclassing requirements.
    // - `MenuTarget` does not implement `Drop`.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "CalibRawMenuTarget"]
    struct MenuTarget;

    impl MenuTarget {
        // SAFETY: The signature matches an AppKit action: one sender, no result.
        #[unsafe(method(openSettings:))]
        fn open_settings(&self, _sender: Option<&AnyObject>) {
            SETTINGS_REQUESTED.store(true, Ordering::Release);
            if let Ok(wake) = WAKE_UI.lock() {
                if let Some(wake) = wake.as_ref() {
                    wake();
                }
            }
        }
    }
);

impl MenuTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: The signature of `NSObject`'s `init` method is correct.
        unsafe { msg_send![super(this), init] }
    }
}

/// Replaces winit's default menu bar. Call on the main thread after the
/// event loop has launched, which is when winit installs its own; elsewhere
/// it does nothing.
pub(super) fn install() {
    let Some(mtm) = MainThreadMarker::new() else {
        log::warn!("The macOS menu bar was not installed off the main thread");
        return;
    };
    let target = TARGET.with(|cell| cell.get_or_init(|| MenuTarget::new(mtm)).clone());
    let app = NSApplication::sharedApplication(mtm);
    // CalibRaw has one window; window tabs would only add empty Window menu
    // items.
    NSWindow::setAllowsAutomaticWindowTabbing(false, mtm);

    let menu_bar = NSMenu::new(mtm);

    let app_menu = NSMenu::new(mtm);
    let services_menu = NSMenu::new(mtm);
    let services_item = menu_item(mtm, ns_string!("Services"), None, "", None);
    services_item.setSubmenu(Some(&services_menu));
    let settings_item = menu_item(
        mtm,
        ns_string!("Settings…"),
        Some(sel!(openSettings:)),
        ",",
        None,
    );
    // SAFETY: `target` stays alive in `TARGET` for the rest of the process,
    // and it implements `openSettings:`.
    unsafe { settings_item.setTarget(Some(&target)) };
    for item in [
        menu_item(
            mtm,
            &NSString::from_str(&format!("About {APP_NAME}")),
            Some(sel!(orderFrontStandardAboutPanel:)),
            "",
            None,
        ),
        NSMenuItem::separatorItem(mtm),
        settings_item,
        NSMenuItem::separatorItem(mtm),
        services_item,
        NSMenuItem::separatorItem(mtm),
        menu_item(
            mtm,
            &NSString::from_str(&format!("Hide {APP_NAME}")),
            Some(sel!(hide:)),
            "h",
            None,
        ),
        menu_item(
            mtm,
            ns_string!("Hide Others"),
            Some(sel!(hideOtherApplications:)),
            "h",
            Some(NSEventModifierFlags::Option | NSEventModifierFlags::Command),
        ),
        menu_item(
            mtm,
            ns_string!("Show All"),
            Some(sel!(unhideAllApplications:)),
            "",
            None,
        ),
        NSMenuItem::separatorItem(mtm),
        menu_item(
            mtm,
            &NSString::from_str(&format!("Quit {APP_NAME}")),
            Some(sel!(terminate:)),
            "q",
            None,
        ),
    ] {
        app_menu.addItem(&item);
    }
    let app_menu_item = NSMenuItem::new(mtm);
    app_menu_item.setSubmenu(Some(&app_menu));
    menu_bar.addItem(&app_menu_item);

    // AppKit appends the window list and, on newer macOS, its tiling items.
    let window_menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!("Window"));
    for item in [
        menu_item(
            mtm,
            ns_string!("Minimize"),
            Some(sel!(performMiniaturize:)),
            "m",
            None,
        ),
        menu_item(mtm, ns_string!("Zoom"), Some(sel!(performZoom:)), "", None),
        NSMenuItem::separatorItem(mtm),
        // NSWindow retitles this item to "Exit Full Screen" while in full screen.
        menu_item(
            mtm,
            ns_string!("Enter Full Screen"),
            Some(sel!(toggleFullScreen:)),
            "f",
            Some(NSEventModifierFlags::Control | NSEventModifierFlags::Command),
        ),
        NSMenuItem::separatorItem(mtm),
        menu_item(
            mtm,
            ns_string!("Bring All to Front"),
            Some(sel!(arrangeInFront:)),
            "",
            None,
        ),
    ] {
        window_menu.addItem(&item);
    }
    let window_menu_item = NSMenuItem::new(mtm);
    window_menu_item.setSubmenu(Some(&window_menu));
    menu_bar.addItem(&window_menu_item);

    app.setServicesMenu(Some(&services_menu));
    app.setWindowsMenu(Some(&window_menu));
    app.setMainMenu(Some(&menu_bar));
}

/// An item with `key` as its key equivalent (`""` for none), held with
/// Command unless `modifiers` says otherwise. A `None` action disables it,
/// except for an item with a submenu.
fn menu_item(
    mtm: MainThreadMarker,
    title: &NSString,
    action: Option<Sel>,
    key: &str,
    modifiers: Option<NSEventModifierFlags>,
) -> Retained<NSMenuItem> {
    // SAFETY: Every selector passed here is an AppKit action taking one
    // sender, or `openSettings:`, which `MenuTarget` implements.
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            title,
            action,
            &NSString::from_str(key),
        )
    };
    if let Some(modifiers) = modifiers {
        item.setKeyEquivalentModifierMask(modifiers);
    }
    item
}

/// Whether Settings… was chosen since the last call.
pub(super) fn take_settings_request() -> bool {
    SETTINGS_REQUESTED.swap(false, Ordering::AcqRel)
}

/// Sets what a menu command calls to wake the UI, or `None` to wake nothing.
pub(super) fn set_wake_ui(wake: Option<Wake>) {
    if let Ok(mut slot) = WAKE_UI.lock() {
        *slot = wake;
    }
}
