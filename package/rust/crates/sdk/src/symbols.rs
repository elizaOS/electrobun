//! The resolved libElectrobunCore symbol table.
//!
//! Mirrors the Zig `Core.Symbols` struct + its `lib.lookup(...) orelse
//! error.MissingCoreSymbol` block. We declare every export from
//! `package/src/core/main.zig` that the Zig SDK consumed as a typed
//! `extern "C" fn` pointer and resolve all of them at load time; a single
//! missing symbol fails the whole load (matching the Zig behaviour).
//!
//! ## How the 110-symbol table is structured
//!
//! A single declarative macro [`core_symbols!`] is the source of truth. Each
//! row is `field_name: c_symbol_name = fn(args...) -> ret`. The macro expands
//! to three things kept in lock-step so they can never drift:
//!   1. the [`Symbols`] struct field declarations (raw fn-pointer types),
//!   2. the resolution block inside [`Symbols::load`] (one `lookup` per row),
//!   3. nothing else — call sites name fields directly.
//!
//! Each pointer is read out of its `libloading::Symbol` (`*sym`) and stored as
//! a plain fn pointer. That is sound only while the backing [`Library`] stays
//! loaded, so [`Symbols`] is always held next to its `Library` in `Core` and
//! both drop together.

use std::os::raw::{c_char, c_int, c_void};

use libloading::Library;

use crate::error::{ElectrobunError, Result};
use crate::handlers::{
    AppReopenHandler, DecideNavigationHandler, GlobalShortcutHandler, QuitRequestedHandler,
    StatusItemHandler, URLOpenHandler, WebviewEventHandler, WebviewPostMessageHandler,
    WindowBlurHandler, WindowCloseHandler, WindowFocusHandler, WindowKeyHandler, WindowMoveHandler,
    WindowResizeHandler,
};

/// Resolve one symbol or fail with [`ElectrobunError::MissingCoreSymbol`].
///
/// # Safety
/// The caller asserts that `name` in `lib` has the C-ABI signature `Fn`.
unsafe fn lookup<Fn: Copy>(lib: &Library, name: &'static [u8]) -> Result<Fn> {
    // SAFETY: forwarded to the caller's assertion on the signature `Fn`.
    let symbol =
        unsafe { lib.get::<Fn>(name) }.map_err(|_| ElectrobunError::MissingCoreSymbol {
            // Strip the trailing NUL for the error message.
            symbol: std::str::from_utf8(&name[..name.len() - 1]).unwrap_or("<invalid-utf8>"),
        })?;
    Ok(*symbol)
}

macro_rules! core_symbols {
    ( $( $field:ident : $cname:literal = $ty:ty );+ $(;)? ) => {
        /// All libElectrobunCore C-ABI entry points the SDK calls.
        #[allow(non_snake_case)]
        pub(crate) struct Symbols {
            $( pub(crate) $field: $ty ),+
        }

        impl Symbols {
            /// Resolve every symbol from an already-opened library.
            ///
            /// # Safety
            /// `lib` must be libElectrobunCore (its exports must match the
            /// declared signatures). The returned fn pointers are only valid
            /// while `lib` stays loaded.
            pub(crate) unsafe fn load(lib: &Library) -> Result<Self> {
                Ok(Self {
                    // SAFETY: each signature is verified against the matching
                    // `export fn` in package/src/core/main.zig.
                    $( $field: unsafe { lookup::<$ty>(lib, concat!($cname, "\0").as_bytes())? } ),+
                })
            }
        }
    };
}

core_symbols! {
    last_error: "electrobun_core_last_error" = extern "C" fn() -> *const c_char;
    run_main_thread: "electrobun_core_run_main_thread" =
        extern "C" fn(*const c_char, *const c_char, *const c_char, c_int) -> c_int;
    configure_webview_runtime: "configureWebviewRuntime" =
        extern "C" fn(u32, *const c_char, *const c_char) -> bool;
    get_window_style: "getWindowStyle" = extern "C" fn(u32) -> u32;
    create_window: "createWindow" = extern "C" fn(
        f64, f64, f64, f64, u32, *const c_char, bool, *const c_char, bool, bool, f64, f64,
        Option<WindowCloseHandler>, Option<WindowMoveHandler>, Option<WindowResizeHandler>,
        Option<WindowFocusHandler>, Option<WindowBlurHandler>, Option<WindowKeyHandler>,
    ) -> u32;
    create_webview: "createWebview" = extern "C" fn(
        u32, u32, *const c_char, *const c_char, f64, f64, f64, f64, bool, *const c_char,
        Option<DecideNavigationHandler>, Option<WebviewEventHandler>,
        Option<WebviewPostMessageHandler>, Option<WebviewPostMessageHandler>,
        Option<WebviewPostMessageHandler>, *const c_char, *const c_char, *const c_char,
        bool, bool, bool,
    ) -> u32;
    create_wgpu_view: "createWGPUView" =
        extern "C" fn(u32, f64, f64, f64, f64, bool, bool, bool) -> u32;
    set_window_title: "setWindowTitle" = extern "C" fn(u32, *const c_char);
    minimize_window: "minimizeWindow" = extern "C" fn(u32);
    restore_window: "restoreWindow" = extern "C" fn(u32);
    is_window_minimized: "isWindowMinimized" = extern "C" fn(u32) -> bool;
    maximize_window: "maximizeWindow" = extern "C" fn(u32);
    unmaximize_window: "unmaximizeWindow" = extern "C" fn(u32);
    is_window_maximized: "isWindowMaximized" = extern "C" fn(u32) -> bool;
    set_window_full_screen: "setWindowFullScreen" = extern "C" fn(u32, bool);
    is_window_full_screen: "isWindowFullScreen" = extern "C" fn(u32) -> bool;
    set_window_always_on_top: "setWindowAlwaysOnTop" = extern "C" fn(u32, bool);
    is_window_always_on_top: "isWindowAlwaysOnTop" = extern "C" fn(u32) -> bool;
    set_window_visible_on_all_workspaces: "setWindowVisibleOnAllWorkspaces" =
        extern "C" fn(u32, bool);
    is_window_visible_on_all_workspaces: "isWindowVisibleOnAllWorkspaces" =
        extern "C" fn(u32) -> bool;
    show_window: "showWindow" = extern "C" fn(u32, bool);
    activate_window: "activateWindow" = extern "C" fn(u32);
    hide_window: "hideWindow" = extern "C" fn(u32);
    set_window_button_position: "setWindowButtonPosition" = extern "C" fn(u32, f64, f64);
    set_window_position: "setWindowPosition" = extern "C" fn(u32, f64, f64);
    set_window_size: "setWindowSize" = extern "C" fn(u32, f64, f64);
    set_window_frame: "setWindowFrame" = extern "C" fn(u32, f64, f64, f64, f64);
    get_window_frame: "getWindowFrame" =
        extern "C" fn(u32, *mut f64, *mut f64, *mut f64, *mut f64);
    close_window: "closeWindow" = extern "C" fn(u32);
    resize_webview: "resizeWebview" = extern "C" fn(u32, f64, f64, f64, f64, *const c_char);
    load_url_in_webview: "loadURLInWebView" = extern "C" fn(u32, *const c_char);
    load_html_in_webview: "loadHTMLInWebView" = extern "C" fn(u32, *const c_char);
    update_preload_script_to_webview: "updatePreloadScriptToWebView" =
        extern "C" fn(u32, *const c_char, *const c_char, bool);
    webview_can_go_back: "webviewCanGoBack" = extern "C" fn(u32) -> bool;
    webview_can_go_forward: "webviewCanGoForward" = extern "C" fn(u32) -> bool;
    webview_go_back: "webviewGoBack" = extern "C" fn(u32);
    webview_go_forward: "webviewGoForward" = extern "C" fn(u32);
    webview_reload: "webviewReload" = extern "C" fn(u32);
    webview_remove: "webviewRemove" = extern "C" fn(u32);
    set_webview_html_content: "setWebviewHTMLContent" = extern "C" fn(u32, *const c_char);
    webview_set_transparent: "webviewSetTransparent" = extern "C" fn(u32, bool);
    webview_set_passthrough: "webviewSetPassthrough" = extern "C" fn(u32, bool);
    webview_set_hidden: "webviewSetHidden" = extern "C" fn(u32, bool);
    set_webview_navigation_rules: "setWebviewNavigationRules" = extern "C" fn(u32, *const c_char);
    webview_find_in_page: "webviewFindInPage" = extern "C" fn(u32, *const c_char, bool, bool);
    webview_stop_find: "webviewStopFind" = extern "C" fn(u32);
    send_internal_message_to_webview: "sendInternalMessageToWebview" =
        extern "C" fn(u32, *const c_char) -> bool;
    send_host_message_to_webview_via_transport: "sendHostMessageToWebviewViaTransport" =
        extern "C" fn(u32, *const c_char) -> bool;
    webview_open_devtools: "webviewOpenDevTools" = extern "C" fn(u32);
    webview_close_devtools: "webviewCloseDevTools" = extern "C" fn(u32);
    webview_toggle_devtools: "webviewToggleDevTools" = extern "C" fn(u32);
    webview_set_page_zoom: "webviewSetPageZoom" = extern "C" fn(u32, f64);
    webview_get_page_zoom: "webviewGetPageZoom" = extern "C" fn(u32) -> f64;
    set_wgpu_view_frame: "setWGPUViewFrame" = extern "C" fn(u32, f64, f64, f64, f64);
    resize_wgpu_view: "resizeWGPUView" = extern "C" fn(u32, f64, f64, f64, f64, *const c_char);
    set_wgpu_view_transparent: "setWGPUViewTransparent" = extern "C" fn(u32, bool);
    set_wgpu_view_passthrough: "setWGPUViewPassthrough" = extern "C" fn(u32, bool);
    set_wgpu_view_hidden: "setWGPUViewHidden" = extern "C" fn(u32, bool);
    remove_wgpu_view: "removeWGPUView" = extern "C" fn(u32);
    get_wgpu_view_pointer: "getWGPUViewPointer" = extern "C" fn(u32) -> *mut c_void;
    get_wgpu_view_native_handle: "getWGPUViewNativeHandle" = extern "C" fn(u32) -> *mut c_void;
    run_wgpu_view_test: "runWGPUViewTest" = extern "C" fn(u32);
    toggle_wgpu_view_test_shader: "toggleWGPUViewTestShader" = extern "C" fn(u32);
    evaluate_javascript_with_no_completion: "evaluateJavaScriptWithNoCompletion" =
        extern "C" fn(u32, *const c_char);
    create_tray: "createTray" = extern "C" fn(
        *const c_char, *const c_char, bool, u32, u32, Option<StatusItemHandler>,
    ) -> u32;
    show_tray: "showTray" = extern "C" fn(u32) -> bool;
    hide_tray: "hideTray" = extern "C" fn(u32);
    set_tray_title: "setTrayTitle" = extern "C" fn(u32, *const c_char);
    remove_tray: "removeTray" = extern "C" fn(u32);
    get_tray_bounds: "getTrayBounds" = extern "C" fn(u32) -> *const c_char;
    set_dock_icon_visible: "setDockIconVisible" = extern "C" fn(bool);
    is_dock_icon_visible: "isDockIconVisible" = extern "C" fn() -> bool;
    get_primary_display: "getPrimaryDisplay" = extern "C" fn() -> *const c_char;
    get_all_displays: "getAllDisplays" = extern "C" fn() -> *const c_char;
    get_cursor_screen_point: "getCursorScreenPoint" = extern "C" fn() -> *const c_char;
    move_to_trash: "moveToTrash" = extern "C" fn(*const c_char) -> bool;
    show_item_in_folder: "showItemInFolder" = extern "C" fn(*const c_char);
    open_external: "openExternal" = extern "C" fn(*const c_char) -> bool;
    open_path: "openPath" = extern "C" fn(*const c_char) -> bool;
    show_notification: "showNotification" =
        extern "C" fn(*const c_char, *const c_char, *const c_char, bool);
    clipboard_read_text: "clipboardReadText" = extern "C" fn() -> *const c_char;
    clipboard_write_text: "clipboardWriteText" = extern "C" fn(*const c_char);
    clipboard_clear: "clipboardClear" = extern "C" fn();
    clipboard_available_formats: "clipboardAvailableFormats" = extern "C" fn() -> *const c_char;
    set_application_menu: "setApplicationMenu" =
        extern "C" fn(*const c_char, Option<StatusItemHandler>);
    show_context_menu: "showContextMenu" =
        extern "C" fn(*const c_char, Option<StatusItemHandler>);
    open_file_dialog: "openFileDialog" = extern "C" fn(
        *const c_char, *const c_char, c_int, c_int, c_int,
    ) -> *const c_char;
    show_message_box: "showMessageBox" = extern "C" fn(
        *const c_char, *const c_char, *const c_char, *const c_char, *const c_char, c_int, c_int,
    ) -> c_int;
    set_global_shortcut_callback: "setGlobalShortcutCallback" =
        extern "C" fn(Option<GlobalShortcutHandler>);
    register_global_shortcut: "registerGlobalShortcut" = extern "C" fn(*const c_char) -> bool;
    unregister_global_shortcut: "unregisterGlobalShortcut" = extern "C" fn(*const c_char) -> bool;
    unregister_all_global_shortcuts: "unregisterAllGlobalShortcuts" = extern "C" fn();
    is_global_shortcut_registered: "isGlobalShortcutRegistered" =
        extern "C" fn(*const c_char) -> bool;
    session_get_cookies: "sessionGetCookies" =
        extern "C" fn(*const c_char, *const c_char) -> *const c_char;
    session_set_cookie: "sessionSetCookie" =
        extern "C" fn(*const c_char, *const c_char) -> bool;
    session_remove_cookie: "sessionRemoveCookie" =
        extern "C" fn(*const c_char, *const c_char, *const c_char) -> bool;
    session_clear_cookies: "sessionClearCookies" = extern "C" fn(*const c_char);
    session_clear_storage_data: "sessionClearStorageData" =
        extern "C" fn(*const c_char, *const c_char);
    set_url_open_handler: "setURLOpenHandler" = extern "C" fn(Option<URLOpenHandler>);
    set_app_reopen_handler: "setAppReopenHandler" = extern "C" fn(Option<AppReopenHandler>);
    set_quit_requested_handler: "setQuitRequestedHandler" =
        extern "C" fn(Option<QuitRequestedHandler>);
    stop_event_loop: "stopEventLoop" = extern "C" fn();
    wait_for_shutdown_complete: "waitForShutdownComplete" = extern "C" fn(c_int);
    force_exit: "forceExit" = extern "C" fn(c_int);
    wgpu_create_surface_for_view: "wgpuCreateSurfaceForView" =
        extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
    wgpu_create_adapter_device_main_thread: "wgpuCreateAdapterDeviceMainThread" =
        extern "C" fn(*mut c_void, *mut c_void, *mut c_void);
    wgpu_surface_configure_main_thread: "wgpuSurfaceConfigureMainThread" =
        extern "C" fn(*mut c_void, *mut c_void);
    wgpu_surface_get_current_texture_main_thread: "wgpuSurfaceGetCurrentTextureMainThread" =
        extern "C" fn(*mut c_void, *mut c_void);
    wgpu_surface_present_main_thread: "wgpuSurfacePresentMainThread" =
        extern "C" fn(*mut c_void) -> c_int;
}
