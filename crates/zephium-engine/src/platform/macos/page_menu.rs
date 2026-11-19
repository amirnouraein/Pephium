//! Browser items in a page's own context menu. WebKit builds that menu and
//! shows it; Zephium only adds to it as it opens.

use objc2::{
    define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, NSObject},
    sel, MainThreadOnly,
};
use objc2_app_kit::{NSMenu, NSMenuItem, NSUserInterfaceItemIdentification};
use objc2_foundation::{MainThreadMarker, NSObjectProtocol, NSString};
use objc2_web_kit::WKWebView;
use wry::WryWebView;

use crate::host::SCROLL_TOP_JS;

/// WebKit's identifier for its Reload item, which Scroll to Top follows.
const RELOAD_IDENTIFIER: &str = "WKMenuItemIdentifierReload";

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ZephiumPageMenuTarget"]
    struct Target;

    impl Target {
        #[unsafe(method(scrollToTop:))]
        fn scroll_to_top(&self, sender: &NSMenuItem) {
            let Some(webview) = sender
                .representedObject()
                .and_then(|object| object.downcast::<WKWebView>().ok())
            else {
                return;
            };
            unsafe {
                webview.evaluateJavaScript_completionHandler(&NSString::from_str(SCROLL_TOP_JS), None)
            };
        }
    }

    unsafe impl NSObjectProtocol for Target {}
);

thread_local! {
    // A menu item holds its target weakly, so the one target lives here.
    static TARGET: Retained<Target> = {
        let mtm = MainThreadMarker::new().expect("page menus open on the main thread");
        unsafe { msg_send![Target::alloc(mtm), init] }
    };
}

/// Adds Scroll to Top after WebKit's Reload, or at the end of a menu that has
/// none (one opened on a link, an image or selected text).
pub(crate) fn add_scroll_to_top(webview: &WryWebView, menu: &NSMenu) {
    let mtm = webview.mtm();
    let title = NSString::from_str(&crate::scroll_to_top_label());
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &title,
            Some(sel!(scrollToTop:)),
            &NSString::new(),
        )
    };
    TARGET.with(|target| unsafe { item.setTarget(Some(target)) });
    let webview: &AnyObject = webview.as_ref();
    unsafe { item.setRepresentedObject(Some(webview)) };
    let reload = NSString::from_str(RELOAD_IDENTIFIER);
    let after_reload = menu.itemArray().iter().position(|existing| {
        existing
            .identifier()
            .is_some_and(|identifier| identifier.isEqualToString(&reload))
    });
    match after_reload {
        Some(index) => menu.insertItem_atIndex(&item, index as isize + 1),
        None => {
            if menu.numberOfItems() > 0 {
                menu.addItem(&NSMenuItem::separatorItem(mtm));
            }
            menu.addItem(&item);
        }
    }
}
