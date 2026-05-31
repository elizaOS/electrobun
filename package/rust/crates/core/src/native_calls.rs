//! Typed wrappers around dlopen'd native-wrapper symbols.
//!
//! Each function here resolves a native symbol by name (via
//! [`crate::native_wrapper::lookup_native_symbol`]), transmutes the resolved
//! address to the concrete `extern "C"` fn-pointer type, and invokes it. This
//! is the single place that performs the `transmute` + call, keeping every
//! unsafe native invocation in one auditable module. Names and signatures match
//! the Zig `lookupNativeSymbol(<FnType>, "<symbol>")` call sites exactly.
//!
//! Booleans passed to these direct API functions are C `bool` (Rust `bool`,
//! one byte) — matching the Zig, which declared `bool` parameters here (NOT the
//! `u32` callback-boolean convention from callbacks.h).

use std::ffi::c_void;
use std::os::raw::{c_char, c_int};

use crate::ffi_types::{
    AppReopenHandler, DecideNavigationHandler, GlobalShortcutHandler, QuitRequestedHandler,
    StatusItemHandler, TrayPtr, UrlOpenHandler, WebviewEventHandler, WebviewPostMessageHandler,
    WebviewPtr, WgpuViewPtr, WindowBlurHandler, WindowCloseHandler, WindowFocusHandler,
    WindowKeyHandler, WindowMoveHandler, WindowPtr, WindowResizeHandler,
};
use crate::native_wrapper::lookup_native_symbol;

/// Resolve `name`, transmute to `$ty`, and bind it to `$name`, returning
/// `$ret` from the enclosing function if the symbol is missing (last-error is
/// set by the resolver).
macro_rules! native_fn {
    ($name:ident : $ty:ty = $sym:literal else return $ret:expr) => {
        let Some(addr) = lookup_native_symbol($sym) else {
            return $ret;
        };
        // SAFETY: the resolver returned the address of an exported symbol whose
        // ABI matches `$ty` (verified against nativeWrapper.{mm,cpp}); the
        // library outlives the call.
        let $name: $ty = unsafe { std::mem::transmute::<*const c_void, $ty>(addr) };
    };
    ($name:ident : $ty:ty = $sym:literal else return) => {
        let Some(addr) = lookup_native_symbol($sym) else {
            return;
        };
        // SAFETY: see above.
        let $name: $ty = unsafe { std::mem::transmute::<*const c_void, $ty>(addr) };
    };
}

// ---- window ----

#[allow(clippy::too_many_arguments)]
pub fn create_window(
    window_id: u32,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    style_mask: u32,
    title_bar_style: *const c_char,
    transparent: bool,
    traffic_light_offset_x: f64,
    traffic_light_offset_y: f64,
    close_handler: Option<WindowCloseHandler>,
    move_handler: Option<WindowMoveHandler>,
    resize_handler: Option<WindowResizeHandler>,
    focus_handler: Option<WindowFocusHandler>,
    blur_handler: Option<WindowBlurHandler>,
    key_handler: Option<WindowKeyHandler>,
) -> WindowPtr {
    type Fn = unsafe extern "C" fn(
        u32,
        f64,
        f64,
        f64,
        f64,
        u32,
        *const c_char,
        bool,
        f64,
        f64,
        Option<WindowCloseHandler>,
        Option<WindowMoveHandler>,
        Option<WindowResizeHandler>,
        Option<WindowFocusHandler>,
        Option<WindowBlurHandler>,
        Option<WindowKeyHandler>,
    ) -> WindowPtr;
    native_fn!(f: Fn = "createWindowWithFrameAndStyleFromWorker" else return std::ptr::null_mut());
    // SAFETY: argument types match the native signature.
    unsafe {
        f(
            window_id,
            x,
            y,
            width,
            height,
            style_mask,
            title_bar_style,
            transparent,
            traffic_light_offset_x,
            traffic_light_offset_y,
            close_handler,
            move_handler,
            resize_handler,
            focus_handler,
            blur_handler,
            key_handler,
        )
    }
}

pub fn set_window_title(window: WindowPtr, title: *const c_char) {
    type Fn = unsafe extern "C" fn(WindowPtr, *const c_char);
    native_fn!(f: Fn = "setWindowTitle" else return);
    // SAFETY: signature matches.
    unsafe { f(window, title) }
}

pub fn show_window(window: WindowPtr, activate: bool) {
    type Fn = unsafe extern "C" fn(WindowPtr, bool);
    native_fn!(f: Fn = "showWindow" else return);
    // SAFETY: signature matches.
    unsafe { f(window, activate) }
}

/// Window-pointer + C-bool setters that share the `(WindowPtr, bool)` shape.
macro_rules! window_bool_setter {
    ($fn_name:ident, $sym:literal) => {
        pub fn $fn_name(window: WindowPtr, value: bool) {
            type Fn = unsafe extern "C" fn(WindowPtr, bool);
            native_fn!(f: Fn = $sym else return);
            // SAFETY: signature matches.
            unsafe { f(window, value) }
        }
    };
}

window_bool_setter!(set_window_full_screen, "setWindowFullScreen");
window_bool_setter!(set_window_always_on_top, "setWindowAlwaysOnTop");
window_bool_setter!(
    set_window_visible_on_all_workspaces,
    "setWindowVisibleOnAllWorkspaces"
);

/// Window-pointer getters returning C bool.
macro_rules! window_bool_getter {
    ($fn_name:ident, $sym:literal) => {
        pub fn $fn_name(window: WindowPtr) -> bool {
            type Fn = unsafe extern "C" fn(WindowPtr) -> bool;
            native_fn!(f: Fn = $sym else return false);
            // SAFETY: signature matches.
            unsafe { f(window) }
        }
    };
}

window_bool_getter!(is_window_minimized, "isWindowMinimized");
window_bool_getter!(is_window_maximized, "isWindowMaximized");
window_bool_getter!(is_window_full_screen, "isWindowFullScreen");
window_bool_getter!(is_window_always_on_top, "isWindowAlwaysOnTop");
window_bool_getter!(
    is_window_visible_on_all_workspaces,
    "isWindowVisibleOnAllWorkspaces"
);

/// Window-pointer void actions with no extra args.
macro_rules! window_action {
    ($fn_name:ident, $sym:literal) => {
        pub fn $fn_name(window: WindowPtr) {
            type Fn = unsafe extern "C" fn(WindowPtr);
            native_fn!(f: Fn = $sym else return);
            // SAFETY: signature matches.
            unsafe { f(window) }
        }
    };
}

window_action!(minimize_window, "minimizeWindow");
window_action!(restore_window, "restoreWindow");
window_action!(maximize_window, "maximizeWindow");
window_action!(unmaximize_window, "unmaximizeWindow");
window_action!(activate_window, "activateWindow");
window_action!(hide_window, "hideWindow");
window_action!(close_window, "closeWindow");
window_action!(start_window_move, "startWindowMove");

pub fn set_window_button_position(window: WindowPtr, x: f64, y: f64) {
    type Fn = unsafe extern "C" fn(WindowPtr, f64, f64);
    native_fn!(f: Fn = "setWindowButtonPosition" else return);
    // SAFETY: signature matches.
    unsafe { f(window, x, y) }
}

pub fn set_window_position(window: WindowPtr, x: f64, y: f64) {
    type Fn = unsafe extern "C" fn(WindowPtr, f64, f64);
    native_fn!(f: Fn = "setWindowPosition" else return);
    // SAFETY: signature matches.
    unsafe { f(window, x, y) }
}

pub fn set_window_size(window: WindowPtr, width: f64, height: f64) {
    type Fn = unsafe extern "C" fn(WindowPtr, f64, f64);
    native_fn!(f: Fn = "setWindowSize" else return);
    // SAFETY: signature matches.
    unsafe { f(window, width, height) }
}

pub fn set_window_frame(window: WindowPtr, x: f64, y: f64, width: f64, height: f64) {
    type Fn = unsafe extern "C" fn(WindowPtr, f64, f64, f64, f64);
    native_fn!(f: Fn = "setWindowFrame" else return);
    // SAFETY: signature matches.
    unsafe { f(window, x, y, width, height) }
}

pub fn get_window_frame(
    window: WindowPtr,
    x: *mut f64,
    y: *mut f64,
    width: *mut f64,
    height: *mut f64,
) {
    type Fn = unsafe extern "C" fn(WindowPtr, *mut f64, *mut f64, *mut f64, *mut f64);
    native_fn!(f: Fn = "getWindowFrame" else return);
    // SAFETY: caller (export) provides four valid out-pointers.
    unsafe { f(window, x, y, width, height) }
}

pub fn stop_window_move() {
    type Fn = unsafe extern "C" fn();
    native_fn!(f: Fn = "stopWindowMove" else return);
    // SAFETY: signature matches.
    unsafe { f() }
}

#[allow(clippy::too_many_arguments)]
pub fn get_window_style(
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
    type Fn = unsafe extern "C" fn(
        bool,
        bool,
        bool,
        bool,
        bool,
        bool,
        bool,
        bool,
        bool,
        bool,
        bool,
        bool,
    ) -> u32;
    native_fn!(f: Fn = "getWindowStyle" else return 0);
    // SAFETY: signature matches.
    unsafe {
        f(
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
    }
}

// ---- webview ----

pub fn set_next_webview_flags(start_transparent: bool, start_passthrough: bool) {
    type Fn = unsafe extern "C" fn(bool, bool);
    native_fn!(f: Fn = "setNextWebviewFlags" else return);
    // SAFETY: signature matches.
    unsafe { f(start_transparent, start_passthrough) }
}

#[allow(clippy::too_many_arguments)]
pub fn init_webview(
    webview_id: u32,
    window: WindowPtr,
    renderer: *const c_char,
    url: *const c_char,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    auto_resize: bool,
    partition: *const c_char,
    navigation_callback: Option<DecideNavigationHandler>,
    webview_event_handler: Option<WebviewEventHandler>,
    event_bridge_handler: Option<WebviewPostMessageHandler>,
    host_bridge_handler: Option<WebviewPostMessageHandler>,
    internal_bridge_handler: Option<WebviewPostMessageHandler>,
    electrobun_preload_script: *const c_char,
    custom_preload_script: *const c_char,
    views_root: *const c_char,
    transparent: bool,
    sandbox: bool,
) -> WebviewPtr {
    type Fn = unsafe extern "C" fn(
        u32,
        WindowPtr,
        *const c_char,
        *const c_char,
        f64,
        f64,
        f64,
        f64,
        bool,
        *const c_char,
        Option<DecideNavigationHandler>,
        Option<WebviewEventHandler>,
        Option<WebviewPostMessageHandler>,
        Option<WebviewPostMessageHandler>,
        Option<WebviewPostMessageHandler>,
        *const c_char,
        *const c_char,
        *const c_char,
        bool,
        bool,
    ) -> WebviewPtr;
    native_fn!(f: Fn = "initWebview" else return std::ptr::null_mut());
    // SAFETY: signature matches the native initWebview; pointers come from
    // owned CStrings held alive by the caller for the duration of the call.
    unsafe {
        f(
            webview_id,
            window,
            renderer,
            url,
            x,
            y,
            width,
            height,
            auto_resize,
            partition,
            navigation_callback,
            webview_event_handler,
            event_bridge_handler,
            host_bridge_handler,
            internal_bridge_handler,
            electrobun_preload_script,
            custom_preload_script,
            views_root,
            transparent,
            sandbox,
        )
    }
}

pub fn resize_webview(
    webview: WebviewPtr,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    masks_json: *const c_char,
) {
    type Fn = unsafe extern "C" fn(WebviewPtr, f64, f64, f64, f64, *const c_char);
    native_fn!(f: Fn = "resizeWebview" else return);
    // SAFETY: signature matches.
    unsafe { f(webview, x, y, width, height, masks_json) }
}

pub fn load_url_in_webview(webview: WebviewPtr, url: *const c_char) {
    type Fn = unsafe extern "C" fn(WebviewPtr, *const c_char);
    native_fn!(f: Fn = "loadURLInWebView" else return);
    // SAFETY: signature matches.
    unsafe { f(webview, url) }
}

pub fn load_html_in_webview(webview: WebviewPtr, html: *const c_char) {
    type Fn = unsafe extern "C" fn(WebviewPtr, *const c_char);
    native_fn!(f: Fn = "loadHTMLInWebView" else return);
    // SAFETY: signature matches.
    unsafe { f(webview, html) }
}

pub fn update_preload_script_to_webview(
    webview: WebviewPtr,
    script_identifier: *const c_char,
    script: *const c_char,
    all_frames: bool,
) {
    type Fn = unsafe extern "C" fn(WebviewPtr, *const c_char, *const c_char, bool);
    native_fn!(f: Fn = "updatePreloadScriptToWebView" else return);
    // SAFETY: signature matches.
    unsafe { f(webview, script_identifier, script, all_frames) }
}

/// Webview-pointer getters returning C bool.
macro_rules! webview_bool_getter {
    ($fn_name:ident, $sym:literal) => {
        pub fn $fn_name(webview: WebviewPtr) -> bool {
            type Fn = unsafe extern "C" fn(WebviewPtr) -> bool;
            native_fn!(f: Fn = $sym else return false);
            // SAFETY: signature matches.
            unsafe { f(webview) }
        }
    };
}

webview_bool_getter!(webview_can_go_back, "webviewCanGoBack");
webview_bool_getter!(webview_can_go_forward, "webviewCanGoForward");

/// Webview-pointer void actions.
macro_rules! webview_action {
    ($fn_name:ident, $sym:literal) => {
        pub fn $fn_name(webview: WebviewPtr) {
            type Fn = unsafe extern "C" fn(WebviewPtr);
            native_fn!(f: Fn = $sym else return);
            // SAFETY: signature matches.
            unsafe { f(webview) }
        }
    };
}

webview_action!(webview_go_back, "webviewGoBack");
webview_action!(webview_go_forward, "webviewGoForward");
webview_action!(webview_reload, "webviewReload");
webview_action!(webview_remove, "webviewRemove");
webview_action!(webview_open_devtools, "webviewOpenDevTools");
webview_action!(webview_close_devtools, "webviewCloseDevTools");
webview_action!(webview_toggle_devtools, "webviewToggleDevTools");
webview_action!(webview_stop_find, "webviewStopFind");

/// Webview-pointer + C-bool setters.
macro_rules! webview_bool_setter {
    ($fn_name:ident, $sym:literal) => {
        pub fn $fn_name(webview: WebviewPtr, value: bool) {
            type Fn = unsafe extern "C" fn(WebviewPtr, bool);
            native_fn!(f: Fn = $sym else return);
            // SAFETY: signature matches.
            unsafe { f(webview, value) }
        }
    };
}

webview_bool_setter!(webview_set_transparent, "webviewSetTransparent");
webview_bool_setter!(webview_set_passthrough, "webviewSetPassthrough");
webview_bool_setter!(webview_set_hidden, "webviewSetHidden");

pub fn set_webview_html_content(webview_id: u32, html: *const c_char) {
    type Fn = unsafe extern "C" fn(u32, *const c_char);
    native_fn!(f: Fn = "setWebviewHTMLContent" else return);
    // SAFETY: signature matches (takes the id, not the pointer).
    unsafe { f(webview_id, html) }
}

pub fn set_webview_navigation_rules(webview: WebviewPtr, rules_json: *const c_char) {
    type Fn = unsafe extern "C" fn(WebviewPtr, *const c_char);
    native_fn!(f: Fn = "setWebviewNavigationRules" else return);
    // SAFETY: signature matches.
    unsafe { f(webview, rules_json) }
}

pub fn webview_find_in_page(
    webview: WebviewPtr,
    search_text: *const c_char,
    forward: bool,
    match_case: bool,
) {
    type Fn = unsafe extern "C" fn(WebviewPtr, *const c_char, bool, bool);
    native_fn!(f: Fn = "webviewFindInPage" else return);
    // SAFETY: signature matches.
    unsafe { f(webview, search_text, forward, match_case) }
}

pub fn evaluate_javascript_with_no_completion(webview: WebviewPtr, js: *const c_char) {
    type Fn = unsafe extern "C" fn(WebviewPtr, *const c_char);
    native_fn!(f: Fn = "evaluateJavaScriptWithNoCompletion" else return);
    // SAFETY: signature matches.
    unsafe { f(webview, js) }
}

pub fn webview_set_page_zoom(webview: WebviewPtr, zoom_level: f64) {
    type Fn = unsafe extern "C" fn(WebviewPtr, f64);
    native_fn!(f: Fn = "webviewSetPageZoom" else return);
    // SAFETY: signature matches.
    unsafe { f(webview, zoom_level) }
}

pub fn webview_get_page_zoom(webview: WebviewPtr) -> f64 {
    type Fn = unsafe extern "C" fn(WebviewPtr) -> f64;
    native_fn!(f: Fn = "webviewGetPageZoom" else return 1.0);
    // SAFETY: signature matches.
    unsafe { f(webview) }
}

// ---- wgpu views ----

#[allow(clippy::too_many_arguments)]
pub fn init_wgpu_view(
    wgpu_view_id: u32,
    window: WindowPtr,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    auto_resize: bool,
    start_transparent: bool,
    start_passthrough: bool,
) -> WgpuViewPtr {
    type Fn =
        unsafe extern "C" fn(u32, WindowPtr, f64, f64, f64, f64, bool, bool, bool) -> WgpuViewPtr;
    native_fn!(f: Fn = "initWGPUView" else return std::ptr::null_mut());
    // SAFETY: signature matches.
    unsafe {
        f(
            wgpu_view_id,
            window,
            x,
            y,
            width,
            height,
            auto_resize,
            start_transparent,
            start_passthrough,
        )
    }
}

pub fn wgpu_view_set_frame(wgpu_view: WgpuViewPtr, x: f64, y: f64, width: f64, height: f64) {
    type Fn = unsafe extern "C" fn(WgpuViewPtr, f64, f64, f64, f64);
    native_fn!(f: Fn = "wgpuViewSetFrame" else return);
    // SAFETY: signature matches.
    unsafe { f(wgpu_view, x, y, width, height) }
}

/// The Zig maps `resizeWGPUView` to the native `resizeWebview` symbol (shared
/// AbstractView resize entrypoint).
pub fn resize_wgpu_view(
    wgpu_view: WgpuViewPtr,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    masks_json: *const c_char,
) {
    type Fn = unsafe extern "C" fn(WgpuViewPtr, f64, f64, f64, f64, *const c_char);
    native_fn!(f: Fn = "resizeWebview" else return);
    // SAFETY: signature matches.
    unsafe { f(wgpu_view, x, y, width, height, masks_json) }
}

macro_rules! wgpu_bool_setter {
    ($fn_name:ident, $sym:literal) => {
        pub fn $fn_name(wgpu_view: WgpuViewPtr, value: bool) {
            type Fn = unsafe extern "C" fn(WgpuViewPtr, bool);
            native_fn!(f: Fn = $sym else return);
            // SAFETY: signature matches.
            unsafe { f(wgpu_view, value) }
        }
    };
}

wgpu_bool_setter!(wgpu_view_set_transparent, "wgpuViewSetTransparent");
wgpu_bool_setter!(wgpu_view_set_passthrough, "wgpuViewSetPassthrough");
wgpu_bool_setter!(wgpu_view_set_hidden, "wgpuViewSetHidden");

macro_rules! wgpu_action {
    ($fn_name:ident, $sym:literal) => {
        pub fn $fn_name(wgpu_view: WgpuViewPtr) {
            type Fn = unsafe extern "C" fn(WgpuViewPtr);
            native_fn!(f: Fn = $sym else return);
            // SAFETY: signature matches.
            unsafe { f(wgpu_view) }
        }
    };
}

wgpu_action!(wgpu_view_remove, "wgpuViewRemove");
wgpu_action!(wgpu_run_gpu_test, "wgpuRunGPUTest");
wgpu_action!(wgpu_toggle_gpu_test_shader, "wgpuToggleGPUTestShader");

pub fn wgpu_view_get_native_handle(wgpu_view: WgpuViewPtr) -> WgpuViewPtr {
    type Fn = unsafe extern "C" fn(WgpuViewPtr) -> WgpuViewPtr;
    native_fn!(f: Fn = "wgpuViewGetNativeHandle" else return std::ptr::null_mut());
    // SAFETY: signature matches.
    unsafe { f(wgpu_view) }
}

// ---- wgpu main-thread surface API ----

pub fn wgpu_create_surface_for_view(instance: *mut c_void, view_ptr: *mut c_void) -> *mut c_void {
    type Fn = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
    native_fn!(f: Fn = "wgpuCreateSurfaceForView" else return std::ptr::null_mut());
    // SAFETY: signature matches.
    unsafe { f(instance, view_ptr) }
}

pub fn wgpu_create_adapter_device_main_thread(
    instance_ptr: *mut c_void,
    surface_ptr: *mut c_void,
    out_adapter_device: *mut c_void,
) {
    type Fn = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void);
    native_fn!(f: Fn = "wgpuCreateAdapterDeviceMainThread" else return);
    // SAFETY: signature matches.
    unsafe { f(instance_ptr, surface_ptr, out_adapter_device) }
}

pub fn wgpu_surface_configure_main_thread(surface_ptr: *mut c_void, config_ptr: *mut c_void) {
    type Fn = unsafe extern "C" fn(*mut c_void, *mut c_void);
    native_fn!(f: Fn = "wgpuSurfaceConfigureMainThread" else return);
    // SAFETY: signature matches.
    unsafe { f(surface_ptr, config_ptr) }
}

pub fn wgpu_surface_get_current_texture_main_thread(
    surface_ptr: *mut c_void,
    surface_texture_ptr: *mut c_void,
) {
    type Fn = unsafe extern "C" fn(*mut c_void, *mut c_void);
    native_fn!(f: Fn = "wgpuSurfaceGetCurrentTextureMainThread" else return);
    // SAFETY: signature matches.
    unsafe { f(surface_ptr, surface_texture_ptr) }
}

pub fn wgpu_surface_present_main_thread(surface_ptr: *mut c_void) -> i32 {
    type Fn = unsafe extern "C" fn(*mut c_void) -> i32;
    native_fn!(f: Fn = "wgpuSurfacePresentMainThread" else return -1);
    // SAFETY: signature matches.
    unsafe { f(surface_ptr) }
}

// ---- tray / menu ----

pub fn create_tray(
    tray_id: u32,
    title: *const c_char,
    image: *const c_char,
    is_template: bool,
    width: u32,
    height: u32,
    handler: Option<StatusItemHandler>,
) -> TrayPtr {
    type Fn = unsafe extern "C" fn(
        u32,
        *const c_char,
        *const c_char,
        bool,
        u32,
        u32,
        Option<StatusItemHandler>,
    ) -> TrayPtr;
    native_fn!(f: Fn = "createTray" else return std::ptr::null_mut());
    // SAFETY: signature matches.
    unsafe { f(tray_id, title, image, is_template, width, height, handler) }
}

pub fn set_tray_menu(tray_ptr: TrayPtr, menu_config: *const c_char) {
    type Fn = unsafe extern "C" fn(TrayPtr, *const c_char);
    native_fn!(f: Fn = "setTrayMenu" else return);
    // SAFETY: signature matches.
    unsafe { f(tray_ptr, menu_config) }
}

pub fn remove_tray(tray_ptr: TrayPtr) {
    type Fn = unsafe extern "C" fn(TrayPtr);
    native_fn!(f: Fn = "removeTray" else return);
    // SAFETY: signature matches.
    unsafe { f(tray_ptr) }
}

pub fn set_tray_title(tray_ptr: TrayPtr, title: *const c_char) {
    type Fn = unsafe extern "C" fn(TrayPtr, *const c_char);
    native_fn!(f: Fn = "setTrayTitle" else return);
    // SAFETY: signature matches.
    unsafe { f(tray_ptr, title) }
}

pub fn set_tray_image(tray_ptr: TrayPtr, image: *const c_char) {
    type Fn = unsafe extern "C" fn(TrayPtr, *const c_char);
    native_fn!(f: Fn = "setTrayImage" else return);
    // SAFETY: signature matches.
    unsafe { f(tray_ptr, image) }
}

/// Returns the native tray-bounds JSON pointer, or null if missing. The pointer
/// is owned by the native side (not freed via `freeCoreString`), matching the
/// Zig which returned the native `[*:0]const u8` directly.
pub fn get_tray_bounds(tray_ptr: TrayPtr) -> *const c_char {
    type Fn = unsafe extern "C" fn(TrayPtr) -> *const c_char;
    native_fn!(f: Fn = "getTrayBounds" else return std::ptr::null());
    // SAFETY: signature matches.
    unsafe { f(tray_ptr) }
}

pub fn set_application_menu(menu_config: *const c_char, handler: Option<StatusItemHandler>) {
    type Fn = unsafe extern "C" fn(*const c_char, Option<StatusItemHandler>);
    native_fn!(f: Fn = "setApplicationMenu" else return);
    // SAFETY: signature matches.
    unsafe { f(menu_config, handler) }
}

pub fn show_context_menu(menu_config: *const c_char, handler: Option<StatusItemHandler>) {
    type Fn = unsafe extern "C" fn(*const c_char, Option<StatusItemHandler>);
    native_fn!(f: Fn = "showContextMenu" else return);
    // SAFETY: signature matches.
    unsafe { f(menu_config, handler) }
}

// ---- os utils ----

/// `(*const c_char) -> bool` os-utility functions (trash/openExternal/...).
macro_rules! path_bool_fn {
    ($fn_name:ident, $sym:literal) => {
        pub fn $fn_name(arg: *const c_char) -> bool {
            type Fn = unsafe extern "C" fn(*const c_char) -> bool;
            native_fn!(f: Fn = $sym else return false);
            // SAFETY: signature matches.
            unsafe { f(arg) }
        }
    };
}

path_bool_fn!(move_to_trash, "moveToTrash");
path_bool_fn!(open_external, "openExternal");
path_bool_fn!(open_path, "openPath");

pub fn show_item_in_folder(path: *const c_char) {
    type Fn = unsafe extern "C" fn(*const c_char);
    native_fn!(f: Fn = "showItemInFolder" else return);
    // SAFETY: signature matches.
    unsafe { f(path) }
}

pub fn show_notification(
    title: *const c_char,
    body: *const c_char,
    subtitle: *const c_char,
    silent: bool,
) {
    type Fn = unsafe extern "C" fn(*const c_char, *const c_char, *const c_char, bool);
    native_fn!(f: Fn = "showNotification" else return);
    // SAFETY: signature matches.
    unsafe { f(title, body, subtitle, silent) }
}

pub fn set_dock_icon_visible(visible: bool) {
    type Fn = unsafe extern "C" fn(bool);
    native_fn!(f: Fn = "setDockIconVisible" else return);
    // SAFETY: signature matches.
    unsafe { f(visible) }
}

pub fn is_dock_icon_visible() -> bool {
    type Fn = unsafe extern "C" fn() -> bool;
    native_fn!(f: Fn = "isDockIconVisible" else return false);
    // SAFETY: signature matches.
    unsafe { f() }
}

pub fn open_file_dialog(
    starting_folder: *const c_char,
    allowed_file_types: *const c_char,
    can_choose_files: c_int,
    can_choose_directories: c_int,
    allows_multiple_selection: c_int,
) -> *const c_char {
    type Fn =
        unsafe extern "C" fn(*const c_char, *const c_char, c_int, c_int, c_int) -> *const c_char;
    native_fn!(f: Fn = "openFileDialog" else return std::ptr::null());
    // SAFETY: signature matches.
    unsafe {
        f(
            starting_folder,
            allowed_file_types,
            can_choose_files,
            can_choose_directories,
            allows_multiple_selection,
        )
    }
}

#[allow(clippy::too_many_arguments)]
pub fn show_message_box(
    box_type: *const c_char,
    title: *const c_char,
    message: *const c_char,
    detail: *const c_char,
    buttons: *const c_char,
    default_id: c_int,
    cancel_id: c_int,
) -> c_int {
    type Fn = unsafe extern "C" fn(
        *const c_char,
        *const c_char,
        *const c_char,
        *const c_char,
        *const c_char,
        c_int,
        c_int,
    ) -> c_int;
    native_fn!(f: Fn = "showMessageBox" else return -1);
    // SAFETY: signature matches.
    unsafe {
        f(
            box_type, title, message, detail, buttons, default_id, cancel_id,
        )
    }
}

// ---- clipboard ----

/// Native-owned C string getters (not freed via `freeCoreString`).
macro_rules! cstr_getter {
    ($fn_name:ident, $sym:literal) => {
        pub fn $fn_name() -> *const c_char {
            type Fn = unsafe extern "C" fn() -> *const c_char;
            native_fn!(f: Fn = $sym else return std::ptr::null());
            // SAFETY: signature matches.
            unsafe { f() }
        }
    };
}

cstr_getter!(clipboard_read_text, "clipboardReadText");
cstr_getter!(clipboard_available_formats, "clipboardAvailableFormats");
cstr_getter!(get_primary_display, "getPrimaryDisplay");
cstr_getter!(get_all_displays, "getAllDisplays");
cstr_getter!(get_cursor_screen_point, "getCursorScreenPoint");

pub fn clipboard_write_text(text: *const c_char) {
    type Fn = unsafe extern "C" fn(*const c_char);
    native_fn!(f: Fn = "clipboardWriteText" else return);
    // SAFETY: signature matches.
    unsafe { f(text) }
}

pub fn clipboard_read_image(out_size: *mut u64) -> *const c_void {
    type Fn = unsafe extern "C" fn(*mut u64) -> *const c_void;
    native_fn!(f: Fn = "clipboardReadImage" else return std::ptr::null());
    // SAFETY: caller supplies a valid out_size pointer.
    unsafe { f(out_size) }
}

pub fn clipboard_write_image(data: *const c_void, size: u64) {
    type Fn = unsafe extern "C" fn(*const c_void, u64);
    native_fn!(f: Fn = "clipboardWriteImage" else return);
    // SAFETY: signature matches.
    unsafe { f(data, size) }
}

pub fn clipboard_clear() {
    type Fn = unsafe extern "C" fn();
    native_fn!(f: Fn = "clipboardClear" else return);
    // SAFETY: signature matches.
    unsafe { f() }
}

pub fn get_mouse_buttons() -> u64 {
    type Fn = unsafe extern "C" fn() -> u64;
    native_fn!(f: Fn = "getMouseButtons" else return 0);
    // SAFETY: signature matches.
    unsafe { f() }
}

// ---- global shortcuts ----

pub fn set_global_shortcut_callback(callback: Option<GlobalShortcutHandler>) {
    type Fn = unsafe extern "C" fn(Option<GlobalShortcutHandler>);
    native_fn!(f: Fn = "setGlobalShortcutCallback" else return);
    // SAFETY: signature matches.
    unsafe { f(callback) }
}

macro_rules! accelerator_bool_fn {
    ($fn_name:ident, $sym:literal) => {
        pub fn $fn_name(accelerator: *const c_char) -> bool {
            type Fn = unsafe extern "C" fn(*const c_char) -> bool;
            native_fn!(f: Fn = $sym else return false);
            // SAFETY: signature matches.
            unsafe { f(accelerator) }
        }
    };
}

accelerator_bool_fn!(register_global_shortcut, "registerGlobalShortcut");
accelerator_bool_fn!(unregister_global_shortcut, "unregisterGlobalShortcut");
accelerator_bool_fn!(is_global_shortcut_registered, "isGlobalShortcutRegistered");

pub fn unregister_all_global_shortcuts() {
    type Fn = unsafe extern "C" fn();
    native_fn!(f: Fn = "unregisterAllGlobalShortcuts" else return);
    // SAFETY: signature matches.
    unsafe { f() }
}

// ---- sessions / cookies ----

pub fn session_get_cookies(
    partition_identifier: *const c_char,
    filter_json: *const c_char,
) -> *const c_char {
    type Fn = unsafe extern "C" fn(*const c_char, *const c_char) -> *const c_char;
    native_fn!(f: Fn = "sessionGetCookies" else return std::ptr::null());
    // SAFETY: signature matches.
    unsafe { f(partition_identifier, filter_json) }
}

pub fn session_set_cookie(partition_identifier: *const c_char, cookie_json: *const c_char) -> bool {
    type Fn = unsafe extern "C" fn(*const c_char, *const c_char) -> bool;
    native_fn!(f: Fn = "sessionSetCookie" else return false);
    // SAFETY: signature matches.
    unsafe { f(partition_identifier, cookie_json) }
}

pub fn session_remove_cookie(
    partition_identifier: *const c_char,
    url: *const c_char,
    cookie_name: *const c_char,
) -> bool {
    type Fn = unsafe extern "C" fn(*const c_char, *const c_char, *const c_char) -> bool;
    native_fn!(f: Fn = "sessionRemoveCookie" else return false);
    // SAFETY: signature matches.
    unsafe { f(partition_identifier, url, cookie_name) }
}

pub fn session_clear_cookies(partition_identifier: *const c_char) {
    type Fn = unsafe extern "C" fn(*const c_char);
    native_fn!(f: Fn = "sessionClearCookies" else return);
    // SAFETY: signature matches.
    unsafe { f(partition_identifier) }
}

pub fn session_clear_storage_data(
    partition_identifier: *const c_char,
    storage_types_json: *const c_char,
) {
    type Fn = unsafe extern "C" fn(*const c_char, *const c_char);
    native_fn!(f: Fn = "sessionClearStorageData" else return);
    // SAFETY: signature matches.
    unsafe { f(partition_identifier, storage_types_json) }
}

// ---- lifecycle ----

pub fn set_url_open_handler(handler: Option<UrlOpenHandler>) {
    type Fn = unsafe extern "C" fn(Option<UrlOpenHandler>);
    native_fn!(f: Fn = "setURLOpenHandler" else return);
    // SAFETY: signature matches.
    unsafe { f(handler) }
}

pub fn set_app_reopen_handler(handler: Option<AppReopenHandler>) {
    type Fn = unsafe extern "C" fn(Option<AppReopenHandler>);
    native_fn!(f: Fn = "setAppReopenHandler" else return);
    // SAFETY: signature matches.
    unsafe { f(handler) }
}

pub fn set_quit_requested_handler(handler: Option<QuitRequestedHandler>) {
    type Fn = unsafe extern "C" fn(Option<QuitRequestedHandler>);
    native_fn!(f: Fn = "setQuitRequestedHandler" else return);
    // SAFETY: signature matches.
    unsafe { f(handler) }
}

/// `stopEventLoop` resolved via lookup (used by both the export and
/// `quitGracefully`). Returns whether the symbol was found+called.
pub fn stop_event_loop() -> bool {
    type Fn = unsafe extern "C" fn();
    native_fn!(f: Fn = "stopEventLoop" else return false);
    // SAFETY: signature matches.
    unsafe { f() }
    true
}

pub fn wait_for_shutdown_complete(timeout_ms: c_int) -> bool {
    type Fn = unsafe extern "C" fn(c_int);
    native_fn!(f: Fn = "waitForShutdownComplete" else return false);
    // SAFETY: signature matches.
    unsafe { f(timeout_ms) }
    true
}
