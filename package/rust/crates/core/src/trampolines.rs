//! `extern "C"` callback trampolines passed into the native wrapper.
//!
//! These are fired by the C++ **main thread** while the FFI exports run on the
//! Bun **worker thread**. They only touch `Mutex`-guarded registries and the
//! self-pipe queue, never thread-affine state. Each body is wrapped in
//! `catch_unwind` so a panic can never unwind across the C ABI boundary.
//!
//! The window-event trampolines look up the per-window user handler (the JS
//! `JSCallback` Bun registered) and forward to it, mirroring the Zig
//! `window*Trampoline`. `hostBridgeQueueTrampoline` enqueues inbound host
//! messages; `internalBridgeCoreTrampoline` feeds the internal JSON dispatch.

use std::os::raw::c_char;

use crate::ffi_types::{
    WindowBlurHandler, WindowCloseHandler, WindowFocusHandler, WindowKeyHandler, WindowMoveHandler,
    WindowResizeHandler,
};
use crate::registry::REGISTRIES;
use crate::util::guard;

/// Enqueue an inbound host-bridge message for the Bun worker. Bound as the
/// `hostBridgePostmessageHandler` fn-pointer in `initWebview`.
pub extern "C" fn host_bridge_queue_trampoline(webview_id: u32, message: *const c_char) {
    guard((), || {
        let Some(message) = crate::util::cstr_to_str(message) else {
            return;
        };
        crate::host_queue::enqueue(webview_id, message);
    });
}

/// Feed an internal-bridge packet (or batch) into the JSON dispatch. Bound as
/// the `internalBridgeHandler` fn-pointer in `initWebview`. The webview id is
/// unused (the packet carries `hostWebviewId`), matching the Zig.
pub extern "C" fn internal_bridge_core_trampoline(_webview_id: u32, message: *const c_char) {
    guard((), || {
        let Some(message) = crate::util::cstr_to_str(message) else {
            return;
        };
        crate::bridge::process_internal_bridge_batch(message);
    });
}

/// Window-close: remove the window, tear down its child webviews/wgpu views,
/// invoke the user close handler, and fire the managed quit handler when the
/// last window closes (if `exit_on_last_window_closed`). Mirrors
/// `windowCloseTrampoline`.
pub extern "C" fn window_close_trampoline(window_id: u32) {
    guard((), || {
        let removed = REGISTRIES.remove_window(window_id);
        let close_handler = removed.and_then(|state| state.close_handler);

        for webview_id in REGISTRIES.webview_ids_for_window(window_id) {
            crate::ops::webview_remove(webview_id);
        }
        for wgpu_view_id in REGISTRIES.wgpu_view_ids_for_window(window_id) {
            crate::ops::remove_wgpu_view(wgpu_view_id);
        }

        if let Some(handler) = close_handler {
            // SAFETY: handler is a JSCallback fn-pointer Bun registered; the
            // worker keeps it alive for the window's lifetime.
            unsafe { call_window_close(handler, window_id) };
        }

        if crate::lifecycle::exit_on_last_window_closed() && !REGISTRIES.has_open_windows() {
            crate::lifecycle::invoke_managed_quit_requested();
        }
    });
}

pub extern "C" fn window_move_trampoline(window_id: u32, x: f64, y: f64) {
    guard((), || {
        if let Some(handler) = REGISTRIES
            .window_state(window_id)
            .and_then(|s| s.move_handler)
        {
            // SAFETY: registered JSCallback fn-pointer.
            unsafe { call_window_move(handler, window_id, x, y) };
        }
    });
}

pub extern "C" fn window_resize_trampoline(
    window_id: u32,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) {
    guard((), || {
        if let Some(handler) = REGISTRIES
            .window_state(window_id)
            .and_then(|s| s.resize_handler)
        {
            // SAFETY: registered JSCallback fn-pointer.
            unsafe { call_window_resize(handler, window_id, x, y, width, height) };
        }
    });
}

pub extern "C" fn window_focus_trampoline(window_id: u32) {
    guard((), || {
        if let Some(handler) = REGISTRIES
            .window_state(window_id)
            .and_then(|s| s.focus_handler)
        {
            // SAFETY: registered JSCallback fn-pointer.
            unsafe { call_window_focus(handler, window_id) };
        }
    });
}

pub extern "C" fn window_blur_trampoline(window_id: u32) {
    guard((), || {
        if let Some(handler) = REGISTRIES
            .window_state(window_id)
            .and_then(|s| s.blur_handler)
        {
            // SAFETY: registered JSCallback fn-pointer.
            unsafe { call_window_blur(handler, window_id) };
        }
    });
}

pub extern "C" fn window_key_trampoline(
    window_id: u32,
    key_code: u32,
    modifiers: u32,
    is_down: u32,
    is_repeat: u32,
) {
    guard((), || {
        if let Some(handler) = REGISTRIES
            .window_state(window_id)
            .and_then(|s| s.key_handler)
        {
            // SAFETY: registered JSCallback fn-pointer.
            unsafe { call_window_key(handler, window_id, key_code, modifiers, is_down, is_repeat) };
        }
    });
}

// Thin call helpers keep every fn-pointer invocation in an explicit `unsafe`
// block so `unsafe_op_in_unsafe_fn` is satisfied at each call site above.
unsafe fn call_window_close(handler: WindowCloseHandler, id: u32) {
    unsafe { handler(id) }
}
unsafe fn call_window_move(handler: WindowMoveHandler, id: u32, x: f64, y: f64) {
    unsafe { handler(id, x, y) }
}
unsafe fn call_window_resize(
    handler: WindowResizeHandler,
    id: u32,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) {
    unsafe { handler(id, x, y, w, h) }
}
unsafe fn call_window_focus(handler: WindowFocusHandler, id: u32) {
    unsafe { handler(id) }
}
unsafe fn call_window_blur(handler: WindowBlurHandler, id: u32) {
    unsafe { handler(id) }
}
unsafe fn call_window_key(
    handler: WindowKeyHandler,
    id: u32,
    key_code: u32,
    modifiers: u32,
    is_down: u32,
    is_repeat: u32,
) {
    unsafe { handler(id, key_code, modifiers, is_down, is_repeat) }
}

/// The managed quit-requested trampoline registered with the native wrapper
/// when the app sets a quit handler. Mirrors `managedQuitRequestedTrampoline`.
pub extern "C" fn managed_quit_requested_trampoline() {
    guard((), || {
        crate::lifecycle::invoke_managed_quit_requested();
    });
}
