//! C-ABI callback function-pointer type aliases and the no-op default callbacks.
//!
//! These mirror the `*const fn (...) callconv(.C) ...` aliases in both
//! `package/src/zig-sdk/electrobun.zig` and `package/src/core/main.zig`. Core
//! stores some of these pointers for the lifetime of the process, so any handler
//! an app installs must be a `'static` `extern "C" fn` (never a closure).

use std::os::raw::c_char;

/// `fn (window_id) -> void` — fired when a window closes.
pub type WindowCloseHandler = extern "C" fn(u32);
/// `fn (window_id, x, y) -> void` — fired when a window moves.
pub type WindowMoveHandler = extern "C" fn(u32, f64, f64);
/// `fn (window_id, x, y, width, height) -> void` — fired when a window resizes.
pub type WindowResizeHandler = extern "C" fn(u32, f64, f64, f64, f64);
/// `fn (window_id) -> void` — fired when a window gains focus.
pub type WindowFocusHandler = extern "C" fn(u32);
/// `fn (window_id) -> void` — fired when a window loses focus.
pub type WindowBlurHandler = extern "C" fn(u32);
/// `fn (window_id, keycode, modifiers, key, characters) -> void`.
pub type WindowKeyHandler = extern "C" fn(u32, u32, u32, u32, u32);

/// `fn (webview_id, url) -> u32` — return non-zero to allow navigation.
pub type DecideNavigationHandler = extern "C" fn(u32, *const c_char) -> u32;
/// `fn (webview_id, event_name, detail) -> void`.
pub type WebviewEventHandler = extern "C" fn(u32, *const c_char, *const c_char);
/// `fn (webview_id, message) -> void` — bridge post-message handler.
pub type WebviewPostMessageHandler = extern "C" fn(u32, *const c_char);

/// `fn (status_item_id, action) -> void` — menu / status-item activation.
pub type StatusItemHandler = extern "C" fn(u32, *const c_char);
/// `fn (accelerator) -> void` — global shortcut activation.
pub type GlobalShortcutHandler = extern "C" fn(*const c_char);
/// `fn () -> void` — invoked when the OS requests a quit.
pub type QuitRequestedHandler = extern "C" fn();
/// `fn (url) -> void` — invoked when the app is asked to open a URL.
pub type URLOpenHandler = extern "C" fn(*const c_char);
/// `fn () -> void` — invoked when the app is reopened (e.g. dock click).
pub type AppReopenHandler = extern "C" fn();

/// Default navigation handler that allows every navigation (returns `1`).
///
/// Mirrors `allowAllNavigation` in the Zig SDK.
pub extern "C" fn allow_all_navigation(_webview_id: u32, _url: *const c_char) -> u32 {
    1
}

/// Default webview event handler that ignores all events.
///
/// Mirrors `noopWebviewEvent` in the Zig SDK.
pub extern "C" fn noop_webview_event(
    _webview_id: u32,
    _event_name: *const c_char,
    _detail: *const c_char,
) {
}

/// Default post-message handler that ignores all messages (returns `0`).
///
/// Mirrors `noopWebviewPostMessage` in the Zig SDK. The Zig original returns
/// `u32`; core only stores it through the `WebviewPostMessageHandler` (void)
/// slot, so the value is discarded. We expose a `void` variant matching the
/// stored pointer type so it slots into [`WebviewPostMessageHandler`] directly.
pub extern "C" fn noop_webview_post_message(_webview_id: u32, _message: *const c_char) {}
