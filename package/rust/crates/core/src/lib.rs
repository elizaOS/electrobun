//! `libElectrobunCore` — Rust port of the Electrobun `core` (formerly
//! `package/src/core/main.zig`).
//!
//! This cdylib is loaded by Bun via `bun:ffi` on both the main thread (which
//! calls only [`electrobun_core_run_main_thread`]) and a worker thread (which
//! drives the rest of the API). It has no `main()`. Responsibilities:
//!
//! 1. ID→pointer registries for windows / webviews / wgpu views / trays,
//!    mutex-guarded because native callbacks fire on the main thread while
//!    exports run on the worker ([`registry`]).
//! 2. An encrypted WebSocket server — the high-throughput host↔webview
//!    transport ([`transport`]).
//! 3. A self-pipe + FIFO that hands inbound webview messages to Bun
//!    ([`host_queue`]).
//! 4. `dlopen` of `libNativeWrapper` and symbol dispatch into it
//!    ([`native_wrapper`] + [`native_calls`]). The native event loop lives in
//!    the C++ wrapper; [`electrobun_core_run_main_thread`] blocks in it.
//!
//! Every `#[no_mangle] extern "C"` body is wrapped in [`util::guard`]
//! (`catch_unwind`) so a panic can never unwind across the FFI boundary into
//! Bun. The `freeCoreString` ownership contract is documented in [`error`].

// The cdylib is named `ElectrobunCore` (fixed by the loader symbol contract in
// `launcher/main.ts` + `bun/proc/native.ts`), and every FFI export keeps its
// camelCase C name to match the Zig `export fn` ABI Bun dlopens. Both trip the
// snake-case lint, which is intentional here.
#![allow(non_snake_case)]

mod bridge;
mod error;
mod ffi_types;
mod host_queue;
mod lifecycle;
mod native_calls;
mod native_wrapper;
mod ops;
mod registry;
mod runtime;
mod trampolines;
mod transport;
mod util;

use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_void};

use ffi_types::{
    AppReopenHandler, DecideNavigationHandler, GlobalShortcutHandler, QuitRequestedHandler,
    StatusItemHandler, UrlOpenHandler, WebviewEventHandler, WebviewPostMessageHandler, WebviewPtr,
    WgpuViewPtr, WindowBlurHandler, WindowCloseHandler, WindowFocusHandler, WindowKeyHandler,
    WindowMoveHandler, WindowPtr, WindowResizeHandler,
};
use registry::REGISTRIES;
use util::{cstr_to_bytes, cstr_to_str, guard};

/// Borrow a C string as `&str` or substitute `""` (matches the Zig treating a
/// pointer to an empty C string the same as the literal it would `std.mem.span`).
fn str_or_empty<'a>(ptr: *const c_char) -> &'a str {
    cstr_to_str(ptr).unwrap_or("")
}

/// Owned `CString` from a C pointer, falling back to an empty string on null /
/// interior-NUL. Used to re-own arguments before passing them to native calls
/// (keeps lifetimes explicit; the pointers live at least until the call ends).
fn owned_cstring(ptr: *const c_char) -> CString {
    match cstr_to_bytes(ptr) {
        Some(bytes) => CString::new(bytes).unwrap_or_default(),
        None => CString::default(),
    }
}

// ============================================================================
// error channel + host queue
// ============================================================================

/// Pointer to the calling thread's last-error message (or `""`). Not freed by
/// `freeCoreString`.
#[no_mangle]
pub extern "C" fn electrobun_core_last_error() -> *const c_char {
    guard(c"".as_ptr(), error::last_error_ptr)
}

/// Pop the next queued host-bound message. Writes the originating webview id to
/// `out_webview_id`; returns a heap C string the caller frees via
/// `freeCoreString`, or null when the queue is empty.
// FFI entry point: `out_webview_id` is an external raw pointer Bun owns; the
// `unsafe` write happens inside. The Zig export is likewise a plain `export fn`.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub extern "C" fn popNextQueuedHostMessage(out_webview_id: *mut u32) -> *mut c_char {
    guard(std::ptr::null_mut(), || {
        error::clear_last_error();
        // SAFETY: Bun passes a valid `*mut u32` (a 1-element Uint32Array ptr).
        unsafe { host_queue::pop_next(out_webview_id) }
    })
}

/// Free a string previously returned by core (e.g. from
/// `popNextQueuedHostMessage`). Null is a no-op.
// FFI entry point: `value` is an external raw pointer; ownership contract is
// upheld in `free_owned_c_string`.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub extern "C" fn freeCoreString(value: *mut c_char) {
    guard((), || {
        // SAFETY: contract — `value` is null or a pointer from
        // `host_queue` / `error::into_owned_c_string` not yet freed.
        unsafe { error::free_owned_c_string(value) }
    });
}

/// Read fd of the wakeup self-pipe, or `-1` (Windows / failure → Bun polls).
#[no_mangle]
pub extern "C" fn getHostMessageWakeupReadFD() -> c_int {
    guard(-1, || {
        error::clear_last_error();
        host_queue::wakeup_read_fd()
    })
}

// ============================================================================
// webview runtime configuration
// ============================================================================

/// Store the compiled preload scripts and start the transport server. Returns
/// false (last-error set) on failure. Mirrors `configureWebviewRuntime`.
#[no_mangle]
pub extern "C" fn configureWebviewRuntime(
    rpc_port: u32,
    preload_script: *const c_char,
    preload_script_sandboxed: *const c_char,
) -> bool {
    guard(false, || {
        error::clear_last_error();
        let preload = str_or_empty(preload_script).to_string();
        let preload_sandboxed = str_or_empty(preload_script_sandboxed).to_string();
        runtime::configure(rpc_port, preload, preload_sandboxed);

        if !transport::start_host_transport_server(rpc_port) {
            return false;
        }
        runtime::mark_configured();
        true
    })
}

// ============================================================================
// main-thread entrypoint
// ============================================================================

/// Blocks the calling (main) thread in the native event loop, then force-exits.
/// Returns 1 (last-error set) if the native wrapper could not be loaded.
/// Mirrors `electrobun_core_run_main_thread`.
#[no_mangle]
pub extern "C" fn electrobun_core_run_main_thread(
    identifier: *const c_char,
    name: *const c_char,
    channel: *const c_char,
    exit_code: c_int,
) -> c_int {
    guard(1, || {
        error::clear_last_error();
        let identifier = owned_cstring(identifier);
        let name = owned_cstring(name);
        let channel = owned_cstring(channel);

        if !native_wrapper::run_start_event_loop(&identifier, &name, &channel) {
            return 1;
        }
        native_wrapper::run_force_exit(exit_code);
        0
    })
}

// ============================================================================
// window API
// ============================================================================

#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn getWindowStyle(
    borderless: bool,
    titled: bool,
    closable: bool,
    miniaturizable: bool,
    resizable: bool,
    unified_title_and_toolbar: bool,
    full_screen: bool,
    full_size_content_view: bool,
    utility_window: bool,
    doc_modal_window: bool,
    nonactivating_panel: bool,
    hud_window: bool,
) -> u32 {
    guard(0, || {
        native_calls::get_window_style(
            borderless,
            titled,
            closable,
            miniaturizable,
            resizable,
            unified_title_and_toolbar,
            full_screen,
            full_size_content_view,
            utility_window,
            doc_modal_window,
            nonactivating_panel,
            hud_window,
        )
    })
}

#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn createWindow(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    style_mask: u32,
    title_bar_style: *const c_char,
    transparent: bool,
    title: *const c_char,
    hidden: bool,
    activate: bool,
    traffic_light_offset_x: f64,
    traffic_light_offset_y: f64,
    close_handler: Option<WindowCloseHandler>,
    move_handler: Option<WindowMoveHandler>,
    resize_handler: Option<WindowResizeHandler>,
    focus_handler: Option<WindowFocusHandler>,
    blur_handler: Option<WindowBlurHandler>,
    key_handler: Option<WindowKeyHandler>,
) -> u32 {
    guard(0, || {
        error::clear_last_error();
        let title_bar_style_c = owned_cstring(title_bar_style);
        let title_c = owned_cstring(title);
        ops::create_window(
            x,
            y,
            width,
            height,
            style_mask,
            &title_bar_style_c,
            transparent,
            &title_c,
            hidden,
            activate,
            traffic_light_offset_x,
            traffic_light_offset_y,
            close_handler,
            move_handler,
            resize_handler,
            focus_handler,
            blur_handler,
            key_handler,
        )
    })
}

#[no_mangle]
pub extern "C" fn getWindowPointer(window_id: u32) -> WindowPtr {
    guard(std::ptr::null_mut(), || {
        error::clear_last_error();
        REGISTRIES.window_ptr(window_id)
    })
}

/// Require a window pointer or set the standard "not found" last-error.
fn require_window_ptr(window_id: u32) -> WindowPtr {
    let ptr = REGISTRIES.window_ptr(window_id);
    if ptr.is_null() {
        error::set_last_error(format!("Window {window_id} not found"));
    }
    ptr
}

#[no_mangle]
pub extern "C" fn setWindowTitle(window_id: u32, title: *const c_char) {
    guard((), || {
        let window = require_window_ptr(window_id);
        if window.is_null() {
            return;
        }
        let title_c = owned_cstring(title);
        native_calls::set_window_title(window, title_c.as_ptr());
    });
}

/// Window action exports that resolve the pointer (with "not found" error) and
/// call a `native_calls` fn taking just the pointer.
macro_rules! window_action_export {
    ($name:ident, $call:path) => {
        #[no_mangle]
        pub extern "C" fn $name(window_id: u32) {
            guard((), || {
                let window = require_window_ptr(window_id);
                if window.is_null() {
                    return;
                }
                $call(window);
            });
        }
    };
}

window_action_export!(minimizeWindow, native_calls::minimize_window);
window_action_export!(restoreWindow, native_calls::restore_window);
window_action_export!(maximizeWindow, native_calls::maximize_window);
window_action_export!(unmaximizeWindow, native_calls::unmaximize_window);
window_action_export!(activateWindow, native_calls::activate_window);
window_action_export!(hideWindow, native_calls::hide_window);
window_action_export!(closeWindow, native_calls::close_window);

/// Window boolean getters that do NOT set last-error on a missing window
/// (return false), matching the Zig `lookupWindowPtr orelse return false`.
macro_rules! window_bool_getter_export {
    ($name:ident, $call:path) => {
        #[no_mangle]
        pub extern "C" fn $name(window_id: u32) -> bool {
            guard(false, || {
                let window = REGISTRIES.window_ptr(window_id);
                if window.is_null() {
                    return false;
                }
                $call(window)
            })
        }
    };
}

window_bool_getter_export!(isWindowMinimized, native_calls::is_window_minimized);
window_bool_getter_export!(isWindowMaximized, native_calls::is_window_maximized);
window_bool_getter_export!(isWindowFullScreen, native_calls::is_window_full_screen);
window_bool_getter_export!(isWindowAlwaysOnTop, native_calls::is_window_always_on_top);
window_bool_getter_export!(
    isWindowVisibleOnAllWorkspaces,
    native_calls::is_window_visible_on_all_workspaces
);

/// Window boolean setters that require the pointer (set "not found" error).
macro_rules! window_bool_setter_export {
    ($name:ident, $call:path) => {
        #[no_mangle]
        pub extern "C" fn $name(window_id: u32, value: bool) {
            guard((), || {
                let window = require_window_ptr(window_id);
                if window.is_null() {
                    return;
                }
                $call(window, value);
            });
        }
    };
}

window_bool_setter_export!(setWindowFullScreen, native_calls::set_window_full_screen);
window_bool_setter_export!(setWindowAlwaysOnTop, native_calls::set_window_always_on_top);
window_bool_setter_export!(
    setWindowVisibleOnAllWorkspaces,
    native_calls::set_window_visible_on_all_workspaces
);

#[no_mangle]
pub extern "C" fn setWindowButtonPosition(window_id: u32, x: f64, y: f64) {
    guard((), || {
        let window = require_window_ptr(window_id);
        if window.is_null() {
            return;
        }
        native_calls::set_window_button_position(window, x, y);
    });
}

#[no_mangle]
pub extern "C" fn showWindow(window_id: u32, activate: bool) {
    guard((), || {
        let window = require_window_ptr(window_id);
        if window.is_null() {
            return;
        }
        native_calls::show_window(window, activate);
    });
}

#[no_mangle]
pub extern "C" fn setWindowPosition(window_id: u32, x: f64, y: f64) {
    guard((), || {
        let window = require_window_ptr(window_id);
        if window.is_null() {
            return;
        }
        native_calls::set_window_position(window, x, y);
    });
}

#[no_mangle]
pub extern "C" fn setWindowSize(window_id: u32, width: f64, height: f64) {
    guard((), || {
        let window = require_window_ptr(window_id);
        if window.is_null() {
            return;
        }
        native_calls::set_window_size(window, width, height);
    });
}

#[no_mangle]
pub extern "C" fn setWindowFrame(window_id: u32, x: f64, y: f64, width: f64, height: f64) {
    guard((), || {
        let window = require_window_ptr(window_id);
        if window.is_null() {
            return;
        }
        native_calls::set_window_frame(window, x, y, width, height);
    });
}

#[no_mangle]
pub extern "C" fn getWindowFrame(
    window_id: u32,
    x: *mut f64,
    y: *mut f64,
    width: *mut f64,
    height: *mut f64,
) {
    guard((), || {
        let window = require_window_ptr(window_id);
        if window.is_null() {
            return;
        }
        native_calls::get_window_frame(window, x, y, width, height);
    });
}

#[no_mangle]
pub extern "C" fn beginWindowMove(window_id: u32) {
    guard((), || {
        error::clear_last_error();
        let window = require_window_ptr(window_id);
        if window.is_null() {
            return;
        }
        native_calls::start_window_move(window);
    });
}

#[no_mangle]
pub extern "C" fn endWindowMove() {
    guard((), || {
        error::clear_last_error();
        native_calls::stop_window_move();
    });
}

// ============================================================================
// webview API
// ============================================================================

#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn createWebview(
    window_id: u32,
    host_webview_id: u32,
    renderer: *const c_char,
    url: *const c_char,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    auto_resize: bool,
    partition_identifier: *const c_char,
    navigation_callback: Option<DecideNavigationHandler>,
    webview_event_handler: Option<WebviewEventHandler>,
    event_bridge_handler: Option<WebviewPostMessageHandler>,
    _host_bridge_handler: Option<WebviewPostMessageHandler>,
    _internal_bridge_handler: Option<WebviewPostMessageHandler>,
    secret_key: *const c_char,
    custom_preload_script: *const c_char,
    views_root: *const c_char,
    sandbox: bool,
    start_transparent: bool,
    start_passthrough: bool,
) -> u32 {
    guard(0, || {
        error::clear_last_error();
        let renderer_str = str_or_empty(renderer);
        let renderer_c = owned_cstring(renderer);
        let url_c = owned_cstring(url);
        let partition_c = owned_cstring(partition_identifier);
        let secret_key_csv = str_or_empty(secret_key).to_string();
        let custom_preload_c = owned_cstring(custom_preload_script);
        let views_root_c = owned_cstring(views_root);

        ops::create_webview(
            window_id,
            host_webview_id,
            renderer_str,
            &renderer_c,
            &url_c,
            x,
            y,
            width,
            height,
            auto_resize,
            &partition_c,
            navigation_callback,
            webview_event_handler,
            event_bridge_handler,
            &secret_key_csv,
            &custom_preload_c,
            &views_root_c,
            sandbox,
            start_transparent,
            start_passthrough,
        )
    })
}

#[no_mangle]
pub extern "C" fn getWebviewPointer(webview_id: u32) -> WebviewPtr {
    guard(std::ptr::null_mut(), || {
        error::clear_last_error();
        REGISTRIES.webview_ptr(webview_id)
    })
}

/// Require a webview pointer or set the standard "not found" last-error.
fn require_webview_ptr(webview_id: u32) -> WebviewPtr {
    let ptr = REGISTRIES.webview_ptr(webview_id);
    if ptr.is_null() {
        error::set_last_error(format!("Webview {webview_id} not found"));
    }
    ptr
}

#[no_mangle]
pub extern "C" fn resizeWebview(
    webview_id: u32,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    masks_json: *const c_char,
) {
    guard((), || {
        error::clear_last_error();
        let webview = require_webview_ptr(webview_id);
        if webview.is_null() {
            return;
        }
        let masks_c = owned_cstring(masks_json);
        native_calls::resize_webview(webview, x, y, width, height, masks_c.as_ptr());
    });
}

#[no_mangle]
pub extern "C" fn loadURLInWebView(webview_id: u32, url: *const c_char) {
    guard((), || {
        error::clear_last_error();
        let webview = require_webview_ptr(webview_id);
        if webview.is_null() {
            return;
        }
        let url_c = owned_cstring(url);
        native_calls::load_url_in_webview(webview, url_c.as_ptr());
    });
}

#[no_mangle]
pub extern "C" fn loadHTMLInWebView(webview_id: u32, html: *const c_char) {
    guard((), || {
        error::clear_last_error();
        let webview = require_webview_ptr(webview_id);
        if webview.is_null() {
            return;
        }
        let html_c = owned_cstring(html);
        native_calls::load_html_in_webview(webview, html_c.as_ptr());
    });
}

#[no_mangle]
pub extern "C" fn updatePreloadScriptToWebView(
    webview_id: u32,
    script_identifier: *const c_char,
    script: *const c_char,
    all_frames: bool,
) {
    guard((), || {
        error::clear_last_error();
        let webview = require_webview_ptr(webview_id);
        if webview.is_null() {
            return;
        }
        let id_c = owned_cstring(script_identifier);
        let script_c = owned_cstring(script);
        native_calls::update_preload_script_to_webview(
            webview,
            id_c.as_ptr(),
            script_c.as_ptr(),
            all_frames,
        );
    });
}

/// Webview boolean getters that do NOT set last-error on miss (return false).
macro_rules! webview_bool_getter_export {
    ($name:ident, $call:path) => {
        #[no_mangle]
        pub extern "C" fn $name(webview_id: u32) -> bool {
            guard(false, || {
                error::clear_last_error();
                let webview = REGISTRIES.webview_ptr(webview_id);
                if webview.is_null() {
                    return false;
                }
                $call(webview)
            })
        }
    };
}

webview_bool_getter_export!(webviewCanGoBack, native_calls::webview_can_go_back);
webview_bool_getter_export!(webviewCanGoForward, native_calls::webview_can_go_forward);

/// Webview void actions requiring the pointer (set "not found" error).
macro_rules! webview_action_export {
    ($name:ident, $call:path) => {
        #[no_mangle]
        pub extern "C" fn $name(webview_id: u32) {
            guard((), || {
                error::clear_last_error();
                let webview = require_webview_ptr(webview_id);
                if webview.is_null() {
                    return;
                }
                $call(webview);
            });
        }
    };
}

webview_action_export!(webviewGoBack, native_calls::webview_go_back);
webview_action_export!(webviewGoForward, native_calls::webview_go_forward);
webview_action_export!(webviewReload, native_calls::webview_reload);
webview_action_export!(webviewOpenDevTools, native_calls::webview_open_devtools);
webview_action_export!(webviewCloseDevTools, native_calls::webview_close_devtools);
webview_action_export!(webviewToggleDevTools, native_calls::webview_toggle_devtools);
webview_action_export!(webviewStopFind, native_calls::webview_stop_find);

#[no_mangle]
pub extern "C" fn webviewRemove(webview_id: u32) {
    guard((), || {
        error::clear_last_error();
        ops::webview_remove(webview_id);
    });
}

#[no_mangle]
pub extern "C" fn setWebviewHTMLContent(webview_id: u32, html: *const c_char) {
    guard((), || {
        error::clear_last_error();
        let html_c = owned_cstring(html);
        native_calls::set_webview_html_content(webview_id, html_c.as_ptr());
    });
}

/// Webview boolean setters requiring the pointer (set "not found" error).
macro_rules! webview_bool_setter_export {
    ($name:ident, $call:path) => {
        #[no_mangle]
        pub extern "C" fn $name(webview_id: u32, value: bool) {
            guard((), || {
                error::clear_last_error();
                let webview = require_webview_ptr(webview_id);
                if webview.is_null() {
                    return;
                }
                $call(webview, value);
            });
        }
    };
}

webview_bool_setter_export!(webviewSetTransparent, native_calls::webview_set_transparent);
webview_bool_setter_export!(webviewSetPassthrough, native_calls::webview_set_passthrough);
webview_bool_setter_export!(webviewSetHidden, native_calls::webview_set_hidden);

#[no_mangle]
pub extern "C" fn setWebviewNavigationRules(webview_id: u32, rules_json: *const c_char) {
    guard((), || {
        error::clear_last_error();
        let webview = require_webview_ptr(webview_id);
        if webview.is_null() {
            return;
        }
        let rules_c = owned_cstring(rules_json);
        native_calls::set_webview_navigation_rules(webview, rules_c.as_ptr());
    });
}

#[no_mangle]
pub extern "C" fn webviewFindInPage(
    webview_id: u32,
    search_text: *const c_char,
    forward: bool,
    match_case: bool,
) {
    guard((), || {
        error::clear_last_error();
        let webview = require_webview_ptr(webview_id);
        if webview.is_null() {
            return;
        }
        let search_c = owned_cstring(search_text);
        native_calls::webview_find_in_page(webview, search_c.as_ptr(), forward, match_case);
    });
}

#[no_mangle]
pub extern "C" fn evaluateJavaScriptWithNoCompletion(webview_id: u32, js: *const c_char) {
    guard((), || {
        error::clear_last_error();
        let js_c = owned_cstring(js);
        ops::evaluate_javascript(webview_id, &js_c);
    });
}

#[no_mangle]
pub extern "C" fn dispatchHostWebviewEvent(
    webview_id: u32,
    event_name: *const c_char,
    detail: *const c_char,
) -> bool {
    guard(false, || {
        error::clear_last_error();
        let event_name_str = str_or_empty(event_name);
        let detail_str = str_or_empty(detail);
        ops::dispatch_host_webview_event(webview_id, event_name_str, detail_str)
    })
}

#[no_mangle]
pub extern "C" fn clearWebviewHostTransport(webview_id: u32) {
    guard((), || {
        error::clear_last_error();
        transport::close_and_clear_webview_socket(webview_id);
    });
}

#[no_mangle]
pub extern "C" fn sendHostMessageToWebviewViaTransport(
    webview_id: u32,
    message_json: *const c_char,
) -> bool {
    guard(false, || {
        error::clear_last_error();
        let message = str_or_empty(message_json);
        transport::send_via_transport(webview_id, message)
    })
}

#[no_mangle]
pub extern "C" fn sendInternalMessageToWebview(
    webview_id: u32,
    message_json: *const c_char,
) -> bool {
    guard(false, || {
        error::clear_last_error();
        let message = str_or_empty(message_json);
        ops::send_internal_message_to_webview(webview_id, message)
    })
}

#[no_mangle]
pub extern "C" fn webviewSetPageZoom(webview_id: u32, zoom_level: f64) {
    guard((), || {
        error::clear_last_error();
        let webview = require_webview_ptr(webview_id);
        if webview.is_null() {
            return;
        }
        native_calls::webview_set_page_zoom(webview, zoom_level);
    });
}

#[no_mangle]
pub extern "C" fn webviewGetPageZoom(webview_id: u32) -> f64 {
    guard(1.0, || {
        error::clear_last_error();
        let webview = REGISTRIES.webview_ptr(webview_id);
        if webview.is_null() {
            return 1.0;
        }
        native_calls::webview_get_page_zoom(webview)
    })
}

// ============================================================================
// wgpu view API
// ============================================================================

#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn createWGPUView(
    window_id: u32,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    auto_resize: bool,
    start_transparent: bool,
    start_passthrough: bool,
) -> u32 {
    guard(0, || {
        error::clear_last_error();
        ops::create_wgpu_view(
            window_id,
            x,
            y,
            width,
            height,
            auto_resize,
            start_transparent,
            start_passthrough,
        )
    })
}

#[no_mangle]
pub extern "C" fn getWGPUViewPointer(wgpu_view_id: u32) -> WgpuViewPtr {
    guard(std::ptr::null_mut(), || {
        error::clear_last_error();
        REGISTRIES.wgpu_view_ptr(wgpu_view_id)
    })
}

/// Require a wgpu-view pointer or set the standard "not found" last-error.
fn require_wgpu_view_ptr(wgpu_view_id: u32) -> WgpuViewPtr {
    let ptr = REGISTRIES.wgpu_view_ptr(wgpu_view_id);
    if ptr.is_null() {
        error::set_last_error(format!("WGPUView {wgpu_view_id} not found"));
    }
    ptr
}

#[no_mangle]
pub extern "C" fn setWGPUViewFrame(wgpu_view_id: u32, x: f64, y: f64, width: f64, height: f64) {
    guard((), || {
        error::clear_last_error();
        let wgpu_view = require_wgpu_view_ptr(wgpu_view_id);
        if wgpu_view.is_null() {
            return;
        }
        native_calls::wgpu_view_set_frame(wgpu_view, x, y, width, height);
    });
}

#[no_mangle]
pub extern "C" fn resizeWGPUView(
    wgpu_view_id: u32,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    masks_json: *const c_char,
) {
    guard((), || {
        error::clear_last_error();
        let wgpu_view = require_wgpu_view_ptr(wgpu_view_id);
        if wgpu_view.is_null() {
            return;
        }
        let masks_c = owned_cstring(masks_json);
        native_calls::resize_wgpu_view(wgpu_view, x, y, width, height, masks_c.as_ptr());
    });
}

/// wgpu-view boolean setters requiring the pointer (set "not found" error).
macro_rules! wgpu_bool_setter_export {
    ($name:ident, $call:path) => {
        #[no_mangle]
        pub extern "C" fn $name(wgpu_view_id: u32, value: bool) {
            guard((), || {
                error::clear_last_error();
                let wgpu_view = require_wgpu_view_ptr(wgpu_view_id);
                if wgpu_view.is_null() {
                    return;
                }
                $call(wgpu_view, value);
            });
        }
    };
}

wgpu_bool_setter_export!(
    setWGPUViewTransparent,
    native_calls::wgpu_view_set_transparent
);
wgpu_bool_setter_export!(
    setWGPUViewPassthrough,
    native_calls::wgpu_view_set_passthrough
);
wgpu_bool_setter_export!(setWGPUViewHidden, native_calls::wgpu_view_set_hidden);

#[no_mangle]
pub extern "C" fn removeWGPUView(wgpu_view_id: u32) {
    guard((), || {
        error::clear_last_error();
        ops::remove_wgpu_view(wgpu_view_id);
    });
}

#[no_mangle]
pub extern "C" fn getWGPUViewNativeHandle(wgpu_view_id: u32) -> WgpuViewPtr {
    guard(std::ptr::null_mut(), || {
        error::clear_last_error();
        let wgpu_view = require_wgpu_view_ptr(wgpu_view_id);
        if wgpu_view.is_null() {
            return std::ptr::null_mut();
        }
        native_calls::wgpu_view_get_native_handle(wgpu_view)
    })
}

#[no_mangle]
pub extern "C" fn runWGPUViewTest(wgpu_view_id: u32) {
    guard((), || {
        error::clear_last_error();
        let wgpu_view = require_wgpu_view_ptr(wgpu_view_id);
        if wgpu_view.is_null() {
            return;
        }
        native_calls::wgpu_run_gpu_test(wgpu_view);
    });
}

#[no_mangle]
pub extern "C" fn toggleWGPUViewTestShader(wgpu_view_id: u32) {
    guard((), || {
        error::clear_last_error();
        let wgpu_view = require_wgpu_view_ptr(wgpu_view_id);
        if wgpu_view.is_null() {
            return;
        }
        native_calls::wgpu_toggle_gpu_test_shader(wgpu_view);
    });
}

// ============================================================================
// tray / menu API
// ============================================================================

#[no_mangle]
pub extern "C" fn createTray(
    title: *const c_char,
    image: *const c_char,
    is_template: bool,
    width: u32,
    height: u32,
    tray_item_handler: Option<StatusItemHandler>,
) -> u32 {
    guard(0, || {
        error::clear_last_error();
        let title_c = owned_cstring(title);
        let image_c = owned_cstring(image);

        let tray_id = REGISTRIES.insert_tray(registry::TrayState {
            title: title_c.clone(),
            image: image_c.clone(),
            menu_config: None,
            is_template,
            width,
            height,
            handler: tray_item_handler,
            ptr: std::ptr::null_mut(),
            visible: false,
        });

        match ops::create_native_tray_for_state(
            tray_id,
            &title_c,
            &image_c,
            is_template,
            width,
            height,
            tray_item_handler,
            None,
        ) {
            Some(ptr) => {
                REGISTRIES.with_tray_mut(tray_id, |state| {
                    state.ptr = ptr;
                    state.visible = true;
                });
                tray_id
            }
            None => {
                REGISTRIES.remove_tray(tray_id);
                0
            }
        }
    })
}

#[no_mangle]
pub extern "C" fn showTray(tray_id: u32) -> bool {
    guard(false, || {
        error::clear_last_error();
        // Snapshot what we need under the lock.
        let snapshot = REGISTRIES.with_tray_mut(tray_id, |state| {
            (
                state.visible && !state.ptr.is_null(),
                state.title.clone(),
                state.image.clone(),
                state.is_template,
                state.width,
                state.height,
                state.handler,
                state.menu_config.clone(),
            )
        });
        let Some((already_visible, title, image, is_template, width, height, handler, menu)) =
            snapshot
        else {
            error::set_last_error(format!("Tray {tray_id} not found"));
            return false;
        };
        if already_visible {
            return true;
        }
        match ops::create_native_tray_for_state(
            tray_id,
            &title,
            &image,
            is_template,
            width,
            height,
            handler,
            menu.as_ref(),
        ) {
            Some(ptr) => {
                REGISTRIES.with_tray_mut(tray_id, |state| {
                    state.ptr = ptr;
                    state.visible = true;
                });
                true
            }
            None => false,
        }
    })
}

/// Hide a tray: remove its native item (if any) and clear ptr/visible. Mirrors
/// `hideNativeTray`.
fn hide_native_tray(tray_id: u32) {
    let ptr = REGISTRIES.with_tray_mut(tray_id, |state| {
        let ptr = state.ptr;
        state.ptr = std::ptr::null_mut();
        state.visible = false;
        ptr
    });
    if let Some(ptr) = ptr {
        if !ptr.is_null() {
            native_calls::remove_tray(ptr);
        }
    }
}

#[no_mangle]
pub extern "C" fn hideTray(tray_id: u32) {
    guard((), || {
        error::clear_last_error();
        hide_native_tray(tray_id);
    });
}

#[no_mangle]
pub extern "C" fn setTrayTitle(tray_id: u32, title: *const c_char) {
    guard((), || {
        error::clear_last_error();
        let title_c = owned_cstring(title);
        let ptr = REGISTRIES.with_tray_mut(tray_id, |state| {
            state.title = title_c.clone();
            state.ptr
        });
        if let Some(ptr) = ptr {
            if !ptr.is_null() {
                native_calls::set_tray_title(ptr, title_c.as_ptr());
            }
        }
    });
}

#[no_mangle]
pub extern "C" fn setTrayImage(tray_id: u32, image: *const c_char) {
    guard((), || {
        error::clear_last_error();
        let image_c = owned_cstring(image);
        let ptr = REGISTRIES.with_tray_mut(tray_id, |state| {
            state.image = image_c.clone();
            state.ptr
        });
        if let Some(ptr) = ptr {
            if !ptr.is_null() {
                native_calls::set_tray_image(ptr, image_c.as_ptr());
            }
        }
    });
}

#[no_mangle]
pub extern "C" fn setTrayMenu(tray_id: u32, menu_config: *const c_char) {
    guard((), || {
        error::clear_last_error();
        let menu_c = owned_cstring(menu_config);
        let ptr = REGISTRIES.with_tray_mut(tray_id, |state| {
            state.menu_config = Some(menu_c.clone());
            state.ptr
        });
        if let Some(ptr) = ptr {
            if !ptr.is_null() {
                native_calls::set_tray_menu(ptr, menu_c.as_ptr());
            }
        }
    });
}

#[no_mangle]
pub extern "C" fn removeTray(tray_id: u32) {
    guard((), || {
        error::clear_last_error();
        hide_native_tray(tray_id);
        REGISTRIES.remove_tray(tray_id);
    });
}

#[no_mangle]
pub extern "C" fn getTrayBounds(tray_id: u32) -> *const c_char {
    guard(ops::EMPTY_RECT_JSON.as_ptr() as *const c_char, || {
        error::clear_last_error();
        let ptr = REGISTRIES.with_tray_mut(tray_id, |state| state.ptr);
        let tray_ptr = match ptr {
            Some(ptr) if !ptr.is_null() => ptr,
            _ => return ops::EMPTY_RECT_JSON.as_ptr() as *const c_char,
        };
        let bounds = native_calls::get_tray_bounds(tray_ptr);
        if bounds.is_null() {
            ops::EMPTY_RECT_JSON.as_ptr() as *const c_char
        } else {
            bounds
        }
    })
}

#[no_mangle]
pub extern "C" fn setApplicationMenu(
    menu_config: *const c_char,
    application_menu_handler: Option<StatusItemHandler>,
) {
    guard((), || {
        let menu_c = owned_cstring(menu_config);
        native_calls::set_application_menu(menu_c.as_ptr(), application_menu_handler);
    });
}

#[no_mangle]
pub extern "C" fn showContextMenu(
    menu_config: *const c_char,
    context_menu_handler: Option<StatusItemHandler>,
) {
    guard((), || {
        let menu_c = owned_cstring(menu_config);
        native_calls::show_context_menu(menu_c.as_ptr(), context_menu_handler);
    });
}

// ============================================================================
// os utils
// ============================================================================

#[no_mangle]
pub extern "C" fn moveToTrash(path: *const c_char) -> bool {
    guard(false, || {
        let path_c = owned_cstring(path);
        native_calls::move_to_trash(path_c.as_ptr())
    })
}

#[no_mangle]
pub extern "C" fn showItemInFolder(path: *const c_char) {
    guard((), || {
        let path_c = owned_cstring(path);
        native_calls::show_item_in_folder(path_c.as_ptr());
    });
}

#[no_mangle]
pub extern "C" fn openExternal(url: *const c_char) -> bool {
    guard(false, || {
        let url_c = owned_cstring(url);
        native_calls::open_external(url_c.as_ptr())
    })
}

#[no_mangle]
pub extern "C" fn openPath(path: *const c_char) -> bool {
    guard(false, || {
        let path_c = owned_cstring(path);
        native_calls::open_path(path_c.as_ptr())
    })
}

#[no_mangle]
pub extern "C" fn showNotification(
    title: *const c_char,
    body: *const c_char,
    subtitle: *const c_char,
    silent: bool,
) {
    guard((), || {
        let title_c = owned_cstring(title);
        let body_c = owned_cstring(body);
        let subtitle_c = owned_cstring(subtitle);
        native_calls::show_notification(
            title_c.as_ptr(),
            body_c.as_ptr(),
            subtitle_c.as_ptr(),
            silent,
        );
    });
}

#[no_mangle]
pub extern "C" fn setDockIconVisible(visible: bool) {
    guard((), || native_calls::set_dock_icon_visible(visible));
}

#[no_mangle]
pub extern "C" fn isDockIconVisible() -> bool {
    guard(false, native_calls::is_dock_icon_visible)
}

#[no_mangle]
pub extern "C" fn openFileDialog(
    starting_folder: *const c_char,
    allowed_file_types: *const c_char,
    can_choose_files: c_int,
    can_choose_directories: c_int,
    allows_multiple_selection: c_int,
) -> *const c_char {
    guard(std::ptr::null(), || {
        let starting_c = owned_cstring(starting_folder);
        let types_c = owned_cstring(allowed_file_types);
        native_calls::open_file_dialog(
            starting_c.as_ptr(),
            types_c.as_ptr(),
            can_choose_files,
            can_choose_directories,
            allows_multiple_selection,
        )
    })
}

#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn showMessageBox(
    box_type: *const c_char,
    title: *const c_char,
    message: *const c_char,
    detail: *const c_char,
    buttons: *const c_char,
    default_id: c_int,
    cancel_id: c_int,
) -> c_int {
    guard(-1, || {
        let box_type_c = owned_cstring(box_type);
        let title_c = owned_cstring(title);
        let message_c = owned_cstring(message);
        let detail_c = owned_cstring(detail);
        let buttons_c = owned_cstring(buttons);
        native_calls::show_message_box(
            box_type_c.as_ptr(),
            title_c.as_ptr(),
            message_c.as_ptr(),
            detail_c.as_ptr(),
            buttons_c.as_ptr(),
            default_id,
            cancel_id,
        )
    })
}

// ---- clipboard ----

#[no_mangle]
pub extern "C" fn clipboardReadText() -> *const c_char {
    guard(std::ptr::null(), native_calls::clipboard_read_text)
}

#[no_mangle]
pub extern "C" fn clipboardWriteText(text: *const c_char) {
    guard((), || {
        let text_c = owned_cstring(text);
        native_calls::clipboard_write_text(text_c.as_ptr());
    });
}

#[no_mangle]
pub extern "C" fn clipboardReadImage(out_size: *mut u64) -> *const c_void {
    guard(std::ptr::null(), || {
        native_calls::clipboard_read_image(out_size)
    })
}

#[no_mangle]
pub extern "C" fn clipboardWriteImage(data: *const c_void, size: u64) {
    guard((), || native_calls::clipboard_write_image(data, size));
}

#[no_mangle]
pub extern "C" fn clipboardClear() {
    guard((), native_calls::clipboard_clear);
}

#[no_mangle]
pub extern "C" fn clipboardAvailableFormats() -> *const c_char {
    guard(std::ptr::null(), native_calls::clipboard_available_formats)
}

// ---- displays / cursor ----

#[no_mangle]
pub extern "C" fn getPrimaryDisplay() -> *const c_char {
    guard(std::ptr::null(), native_calls::get_primary_display)
}

#[no_mangle]
pub extern "C" fn getAllDisplays() -> *const c_char {
    guard(std::ptr::null(), native_calls::get_all_displays)
}

#[no_mangle]
pub extern "C" fn getCursorScreenPoint() -> *const c_char {
    guard(std::ptr::null(), native_calls::get_cursor_screen_point)
}

#[no_mangle]
pub extern "C" fn getMouseButtons() -> u64 {
    guard(0, native_calls::get_mouse_buttons)
}

// ---- global shortcuts ----

#[no_mangle]
pub extern "C" fn setGlobalShortcutCallback(callback: Option<GlobalShortcutHandler>) {
    guard((), || {
        error::clear_last_error();
        native_calls::set_global_shortcut_callback(callback);
    });
}

/// Global-shortcut accelerator → bool exports.
macro_rules! accelerator_bool_export {
    ($name:ident, $call:path) => {
        #[no_mangle]
        pub extern "C" fn $name(accelerator: *const c_char) -> bool {
            guard(false, || {
                error::clear_last_error();
                let accelerator_c = owned_cstring(accelerator);
                $call(accelerator_c.as_ptr())
            })
        }
    };
}

accelerator_bool_export!(
    registerGlobalShortcut,
    native_calls::register_global_shortcut
);
accelerator_bool_export!(
    unregisterGlobalShortcut,
    native_calls::unregister_global_shortcut
);
accelerator_bool_export!(
    isGlobalShortcutRegistered,
    native_calls::is_global_shortcut_registered
);

#[no_mangle]
pub extern "C" fn unregisterAllGlobalShortcuts() {
    guard((), || {
        error::clear_last_error();
        native_calls::unregister_all_global_shortcuts();
    });
}

// ---- sessions / cookies ----

#[no_mangle]
pub extern "C" fn sessionGetCookies(
    partition_identifier: *const c_char,
    filter_json: *const c_char,
) -> *const c_char {
    guard(std::ptr::null(), || {
        error::clear_last_error();
        let partition_c = owned_cstring(partition_identifier);
        let filter_c = owned_cstring(filter_json);
        native_calls::session_get_cookies(partition_c.as_ptr(), filter_c.as_ptr())
    })
}

#[no_mangle]
pub extern "C" fn sessionSetCookie(
    partition_identifier: *const c_char,
    cookie_json: *const c_char,
) -> bool {
    guard(false, || {
        error::clear_last_error();
        let partition_c = owned_cstring(partition_identifier);
        let cookie_c = owned_cstring(cookie_json);
        native_calls::session_set_cookie(partition_c.as_ptr(), cookie_c.as_ptr())
    })
}

#[no_mangle]
pub extern "C" fn sessionRemoveCookie(
    partition_identifier: *const c_char,
    url: *const c_char,
    cookie_name: *const c_char,
) -> bool {
    guard(false, || {
        error::clear_last_error();
        let partition_c = owned_cstring(partition_identifier);
        let url_c = owned_cstring(url);
        let cookie_name_c = owned_cstring(cookie_name);
        native_calls::session_remove_cookie(
            partition_c.as_ptr(),
            url_c.as_ptr(),
            cookie_name_c.as_ptr(),
        )
    })
}

#[no_mangle]
pub extern "C" fn sessionClearCookies(partition_identifier: *const c_char) {
    guard((), || {
        error::clear_last_error();
        let partition_c = owned_cstring(partition_identifier);
        native_calls::session_clear_cookies(partition_c.as_ptr());
    });
}

#[no_mangle]
pub extern "C" fn sessionClearStorageData(
    partition_identifier: *const c_char,
    storage_types_json: *const c_char,
) {
    guard((), || {
        error::clear_last_error();
        let partition_c = owned_cstring(partition_identifier);
        let storage_c = owned_cstring(storage_types_json);
        native_calls::session_clear_storage_data(partition_c.as_ptr(), storage_c.as_ptr());
    });
}

// ============================================================================
// lifecycle
// ============================================================================

#[no_mangle]
pub extern "C" fn setURLOpenHandler(handler: Option<UrlOpenHandler>) {
    guard((), || {
        error::clear_last_error();
        native_calls::set_url_open_handler(handler);
    });
}

#[no_mangle]
pub extern "C" fn setAppReopenHandler(handler: Option<AppReopenHandler>) {
    guard((), || {
        error::clear_last_error();
        native_calls::set_app_reopen_handler(handler);
    });
}

#[no_mangle]
pub extern "C" fn setQuitRequestedHandler(handler: Option<QuitRequestedHandler>) {
    guard((), || {
        error::clear_last_error();
        lifecycle::set_managed_quit_requested_handler(handler);
        // Register our trampoline with native only when a handler is set,
        // matching the Zig (`if handler != null ... else null`).
        let native_handler = handler.map(|_| {
            trampolines::managed_quit_requested_trampoline as ffi_types::QuitRequestedHandler
        });
        native_calls::set_quit_requested_handler(native_handler);
    });
}

#[no_mangle]
pub extern "C" fn setExitOnLastWindowClosed(enabled: bool) {
    guard((), || {
        error::clear_last_error();
        lifecycle::set_exit_on_last_window_closed(enabled);
    });
}

#[no_mangle]
pub extern "C" fn quitGracefully(code: c_int, timeout_ms: c_int) {
    guard((), || {
        error::clear_last_error();
        // Best-effort: stop the loop, wait, then force-exit. Each native call
        // is a no-op if its symbol is missing, mirroring the Zig.
        native_calls::stop_event_loop();
        native_calls::wait_for_shutdown_complete(timeout_ms);
        native_wrapper::run_force_exit(code);
    });
}

#[no_mangle]
pub extern "C" fn stopEventLoop() {
    guard((), || {
        error::clear_last_error();
        native_calls::stop_event_loop();
    });
}

#[no_mangle]
pub extern "C" fn waitForShutdownComplete(timeout_ms: c_int) {
    guard((), || {
        error::clear_last_error();
        native_calls::wait_for_shutdown_complete(timeout_ms);
    });
}

#[no_mangle]
pub extern "C" fn forceExit(code: c_int) {
    guard((), || {
        error::clear_last_error();
        native_wrapper::run_force_exit(code);
    });
}

// ============================================================================
// wgpu main-thread surface API (pass-through)
// ============================================================================

#[no_mangle]
pub extern "C" fn wgpuCreateSurfaceForView(
    instance: *mut c_void,
    view_ptr: *mut c_void,
) -> *mut c_void {
    guard(std::ptr::null_mut(), || {
        error::clear_last_error();
        native_calls::wgpu_create_surface_for_view(instance, view_ptr)
    })
}

#[no_mangle]
pub extern "C" fn wgpuCreateAdapterDeviceMainThread(
    instance_ptr: *mut c_void,
    surface_ptr: *mut c_void,
    out_adapter_device: *mut c_void,
) {
    guard((), || {
        error::clear_last_error();
        native_calls::wgpu_create_adapter_device_main_thread(
            instance_ptr,
            surface_ptr,
            out_adapter_device,
        );
    });
}

#[no_mangle]
pub extern "C" fn wgpuSurfaceConfigureMainThread(
    surface_ptr: *mut c_void,
    config_ptr: *mut c_void,
) {
    guard((), || {
        error::clear_last_error();
        native_calls::wgpu_surface_configure_main_thread(surface_ptr, config_ptr);
    });
}

#[no_mangle]
pub extern "C" fn wgpuSurfaceGetCurrentTextureMainThread(
    surface_ptr: *mut c_void,
    surface_texture_ptr: *mut c_void,
) {
    guard((), || {
        error::clear_last_error();
        native_calls::wgpu_surface_get_current_texture_main_thread(
            surface_ptr,
            surface_texture_ptr,
        );
    });
}

#[no_mangle]
pub extern "C" fn wgpuSurfacePresentMainThread(surface_ptr: *mut c_void) -> i32 {
    guard(-1, || {
        error::clear_last_error();
        native_calls::wgpu_surface_present_main_thread(surface_ptr)
    })
}
