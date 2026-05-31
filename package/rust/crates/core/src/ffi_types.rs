//! Shared C-ABI type aliases mirroring the callback fn-pointer signatures in
//! `package/src/native/shared/callbacks.h` and the function-pointer types in
//! the former `core/main.zig`.
//!
//! Booleans that cross the FFI boundary as *callback* arguments are `u32`
//! (see callbacks.h: Bun's `FFIType.bool` does not interop cleanly with
//! Objective-C `YES`/`NO`, so JS callbacks use `uint32_t`). Booleans passed to
//! the direct native-wrapper API functions are C `bool` (one byte), matching
//! the `bool` parameters the Zig used when calling into libNativeWrapper.

use std::os::raw::{c_char, c_int};

/// Opaque native handles (NSWindow*, WKWebView*, tray item, wgpu view, ...).
pub type WindowPtr = *mut std::ffi::c_void;
pub type WebviewPtr = *mut std::ffi::c_void;
pub type WgpuViewPtr = *mut std::ffi::c_void;
pub type TrayPtr = *mut std::ffi::c_void;

// Eagerly-resolved native-wrapper entrypoints.
pub type StartEventLoopFn = unsafe extern "C" fn(*const c_char, *const c_char, *const c_char);
pub type ForceExitFn = unsafe extern "C" fn(c_int);

// Window event callbacks (fired from the C++ main thread).
pub type WindowCloseHandler = unsafe extern "C" fn(u32);
pub type WindowMoveHandler = unsafe extern "C" fn(u32, f64, f64);
pub type WindowResizeHandler = unsafe extern "C" fn(u32, f64, f64, f64, f64);
pub type WindowFocusHandler = unsafe extern "C" fn(u32);
pub type WindowBlurHandler = unsafe extern "C" fn(u32);
pub type WindowKeyHandler = unsafe extern "C" fn(u32, u32, u32, u32, u32);

// Webview callbacks. `DecideNavigationHandler` returns the navigation decision
// as a `u32` boolean (callbacks.h `DecideNavigationCallback`).
pub type DecideNavigationHandler = unsafe extern "C" fn(u32, *const c_char) -> u32;
pub type WebviewEventHandler = unsafe extern "C" fn(u32, *const c_char, *const c_char);
pub type WebviewPostMessageHandler = unsafe extern "C" fn(u32, *const c_char);

// Tray / menu / lifecycle callbacks.
pub type StatusItemHandler = unsafe extern "C" fn(u32, *const c_char);
pub type GlobalShortcutHandler = unsafe extern "C" fn(*const c_char);
pub type QuitRequestedHandler = unsafe extern "C" fn();
pub type UrlOpenHandler = unsafe extern "C" fn(*const c_char);
pub type AppReopenHandler = unsafe extern "C" fn();

/// Renderer kind for a webview. Mirrors the Zig `WebviewRendererKind` enum.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WebviewRendererKind {
    Native,
    Cef,
}

impl WebviewRendererKind {
    /// Parse the renderer string exactly as the Zig did: only the literal
    /// `"cef"` selects CEF; everything else (including invalid UTF-8) is native.
    pub fn parse(renderer: &str) -> Self {
        if renderer == "cef" {
            WebviewRendererKind::Cef
        } else {
            WebviewRendererKind::Native
        }
    }
}
