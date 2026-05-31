//! Operation layer: registry + native-wrapper orchestration shared by the
//! `#[no_mangle]` exports ([`crate::lib`]) and the internal JSON bridge
//! ([`crate::bridge`]).
//!
//! Functions here take already-borrowed Rust values (ids, `&str`, parsed
//! handles) and mirror the bodies of the corresponding Zig `export fn`s:
//! reserve-then-fill registry flows for creation, id→pointer lookup for
//! mutation, and the JS-string construction the Zig performed inline.

use std::ffi::CString;

use crate::error::set_last_error;
use crate::ffi_types::{
    DecideNavigationHandler, StatusItemHandler, WebviewEventHandler, WebviewPostMessageHandler,
    WebviewRendererKind,
};
use crate::native_calls as nc;
use crate::registry::{
    WebviewSecretKey, WebviewState, WgpuViewState, WindowState, REGISTRIES, SECRET_KEY_LEN,
};
use crate::util::json_quote;

/// Empty-rect JSON returned by `getTrayBounds` fallbacks (matches the Zig
/// `empty_rect_json` literal). Static — never freed via `freeCoreString`.
pub const EMPTY_RECT_JSON: &str = "{\"x\":0,\"y\":0,\"width\":0,\"height\":0}";

/// Parse a comma-separated decimal byte list into a 32-byte AES key, exactly
/// like the Zig `parseWebviewSecretKey`: trims whitespace, skips empty parts,
/// requires exactly 32 bytes, records the matching last-error on failure.
pub fn parse_webview_secret_key(secret_key: &str) -> Option<WebviewSecretKey> {
    let input = secret_key.trim_matches([' ', '\t', '\r', '\n']);
    let mut parsed = [0u8; SECRET_KEY_LEN];
    let mut index = 0usize;

    for part in input.split(',') {
        let trimmed = part.trim_matches([' ', '\t', '\r', '\n']);
        if trimmed.is_empty() {
            continue;
        }
        if index >= parsed.len() {
            set_last_error(format!(
                "Webview secret key must contain exactly {SECRET_KEY_LEN} bytes"
            ));
            return None;
        }
        match trimmed.parse::<u8>() {
            Ok(byte) => parsed[index] = byte,
            Err(err) => {
                set_last_error(format!("Failed to parse webview secret key byte: {err}"));
                return None;
            }
        }
        index += 1;
    }

    if index != parsed.len() {
        set_last_error(format!(
            "Webview secret key must contain exactly {SECRET_KEY_LEN} bytes"
        ));
        return None;
    }
    Some(parsed)
}

/// Build the per-window create flow: reserve id, native-create with
/// trampolines, fill the pointer, then title + show. Returns the new window id
/// or 0 (last-error set). Mirrors `createWindow`.
#[allow(clippy::too_many_arguments)]
pub fn create_window(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    style_mask: u32,
    title_bar_style: &CString,
    transparent: bool,
    title: &CString,
    hidden: bool,
    activate: bool,
    traffic_light_offset_x: f64,
    traffic_light_offset_y: f64,
    close_handler: Option<crate::ffi_types::WindowCloseHandler>,
    move_handler: Option<crate::ffi_types::WindowMoveHandler>,
    resize_handler: Option<crate::ffi_types::WindowResizeHandler>,
    focus_handler: Option<crate::ffi_types::WindowFocusHandler>,
    blur_handler: Option<crate::ffi_types::WindowBlurHandler>,
    key_handler: Option<crate::ffi_types::WindowKeyHandler>,
) -> u32 {
    // Resolve the three required native symbols up front (the Zig bails to 0 if
    // any is missing).
    if crate::native_wrapper::lookup_native_symbol("createWindowWithFrameAndStyleFromWorker")
        .is_none()
        || crate::native_wrapper::lookup_native_symbol("setWindowTitle").is_none()
        || crate::native_wrapper::lookup_native_symbol("showWindow").is_none()
    {
        return 0;
    }

    let placeholder = WindowState {
        ptr: std::ptr::null_mut(),
        transparent,
        close_handler,
        move_handler,
        resize_handler,
        focus_handler,
        blur_handler,
        key_handler,
    };
    let Some(window_id) = REGISTRIES.insert_window_placeholder(placeholder) else {
        set_last_error("Failed to allocate window id");
        return 0;
    };

    let window_ptr = nc::create_window(
        window_id,
        x,
        y,
        width,
        height,
        style_mask,
        title_bar_style.as_ptr(),
        transparent,
        traffic_light_offset_x,
        traffic_light_offset_y,
        Some(crate::trampolines::window_close_trampoline),
        Some(crate::trampolines::window_move_trampoline),
        Some(crate::trampolines::window_resize_trampoline),
        Some(crate::trampolines::window_focus_trampoline),
        Some(crate::trampolines::window_blur_trampoline),
        Some(crate::trampolines::window_key_trampoline),
    );

    if window_ptr.is_null() {
        REGISTRIES.remove_window(window_id);
        set_last_error("Failed to create window");
        return 0;
    }

    if !REGISTRIES.set_window_ptr(window_id, window_ptr) {
        set_last_error(format!("Window {window_id} disappeared during creation"));
        return 0;
    }

    nc::set_window_title(window_ptr, title.as_ptr());
    if !hidden {
        nc::show_window(window_ptr, activate);
    }
    window_id
}

/// Full `createWebview` flow. `secret_key_csv` is the raw comma-separated key
/// string Bun passed (reused verbatim when building the preload). Returns the
/// new webview id or 0.
#[allow(clippy::too_many_arguments)]
pub fn create_webview(
    window_id: u32,
    host_webview_id: u32,
    renderer: &str,
    renderer_c: &CString,
    url_c: &CString,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    auto_resize: bool,
    partition_c: &CString,
    navigation_callback: Option<DecideNavigationHandler>,
    webview_event_handler: Option<WebviewEventHandler>,
    event_bridge_handler: Option<WebviewPostMessageHandler>,
    secret_key_csv: &str,
    custom_preload_c: &CString,
    views_root_c: &CString,
    sandbox: bool,
    start_transparent: bool,
    start_passthrough: bool,
) -> u32 {
    REGISTRIES.remember_default_webview_callbacks(
        navigation_callback,
        webview_event_handler,
        event_bridge_handler,
    );

    let Some(window_state) = REGISTRIES.window_state(window_id) else {
        set_last_error(format!("Window {window_id} not found"));
        return 0;
    };
    let window = window_state.ptr;
    if window.is_null() {
        set_last_error(format!("Window {window_id} not found"));
        return 0;
    }

    if crate::native_wrapper::lookup_native_symbol("setNextWebviewFlags").is_none()
        || crate::native_wrapper::lookup_native_symbol("initWebview").is_none()
    {
        return 0;
    }

    let Some(parsed_secret_key) = parse_webview_secret_key(secret_key_csv) else {
        return 0;
    };

    let placeholder = WebviewState {
        ptr: std::ptr::null_mut(),
        window_id,
        host_webview_id: if host_webview_id == 0 {
            None
        } else {
            Some(host_webview_id)
        },
        renderer: WebviewRendererKind::parse(renderer),
        secret_key: parsed_secret_key,
        socket_handle: None,
        transport_ready: false,
    };
    let Some(webview_id) = REGISTRIES.insert_webview_placeholder(placeholder) else {
        set_last_error("Failed to allocate webview id");
        return 0;
    };

    let Some(preload) =
        crate::runtime::build_preload(webview_id, window_id, secret_key_csv, sandbox)
    else {
        REGISTRIES.remove_webview(webview_id);
        return 0;
    };
    let Ok(preload_c) = CString::new(preload) else {
        REGISTRIES.remove_webview(webview_id);
        set_last_error("Failed to build preload script");
        return 0;
    };

    nc::set_next_webview_flags(start_transparent, start_passthrough);

    let webview_ptr = nc::init_webview(
        webview_id,
        window,
        renderer_c.as_ptr(),
        url_c.as_ptr(),
        x,
        y,
        width,
        height,
        auto_resize,
        partition_c.as_ptr(),
        navigation_callback,
        webview_event_handler,
        event_bridge_handler,
        Some(crate::trampolines::host_bridge_queue_trampoline),
        Some(crate::trampolines::internal_bridge_core_trampoline),
        preload_c.as_ptr(),
        custom_preload_c.as_ptr(),
        views_root_c.as_ptr(),
        window_state.transparent,
        sandbox,
    );

    if webview_ptr.is_null() {
        REGISTRIES.remove_webview(webview_id);
        set_last_error("Failed to create webview");
        return 0;
    }

    if !REGISTRIES.set_webview_ptr(webview_id, webview_ptr) {
        set_last_error(format!("Webview {webview_id} disappeared during creation"));
        return 0;
    }
    webview_id
}

/// `createWGPUView` flow. Returns the new id or 0.
#[allow(clippy::too_many_arguments)]
pub fn create_wgpu_view(
    window_id: u32,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    auto_resize: bool,
    start_transparent: bool,
    start_passthrough: bool,
) -> u32 {
    let window = REGISTRIES.window_ptr(window_id);
    if window.is_null() {
        set_last_error(format!("Window {window_id} not found"));
        return 0;
    }
    if crate::native_wrapper::lookup_native_symbol("initWGPUView").is_none() {
        return 0;
    }

    let Some(wgpu_view_id) = REGISTRIES.insert_wgpu_view_placeholder(WgpuViewState {
        ptr: std::ptr::null_mut(),
        window_id,
    }) else {
        set_last_error("Failed to allocate WGPUView id");
        return 0;
    };

    let wgpu_view_ptr = nc::init_wgpu_view(
        wgpu_view_id,
        window,
        x,
        y,
        width,
        height,
        auto_resize,
        start_transparent,
        start_passthrough,
    );

    if wgpu_view_ptr.is_null() {
        REGISTRIES.remove_wgpu_view(wgpu_view_id);
        set_last_error("Failed to create WGPUView");
        return 0;
    }

    if !REGISTRIES.set_wgpu_view_ptr(wgpu_view_id, wgpu_view_ptr) {
        set_last_error(format!(
            "WGPUView {wgpu_view_id} disappeared during creation"
        ));
        return 0;
    }
    wgpu_view_id
}

/// Remove a webview by id: drop registry state, close its socket, and call the
/// native `webviewRemove`. Mirrors the Zig `webviewRemove` export body (also
/// used by the window-close trampoline and the internal bridge).
pub fn webview_remove(webview_id: u32) {
    let removed = REGISTRIES.remove_webview(webview_id);
    let (webview, socket_handle) = match removed {
        Some(state) => (state.ptr, state.socket_handle),
        None => (std::ptr::null_mut(), None),
    };
    if let Some(handle) = socket_handle {
        crate::transport::close_socket_handle(handle);
    }
    if webview.is_null() {
        return;
    }
    nc::webview_remove(webview);
}

/// Remove a wgpu view by id. Mirrors `removeWGPUView`.
pub fn remove_wgpu_view(wgpu_view_id: u32) {
    let removed = REGISTRIES.remove_wgpu_view(wgpu_view_id);
    let wgpu_view = match removed {
        Some(state) => state.ptr,
        None => std::ptr::null_mut(),
    };
    if wgpu_view.is_null() {
        return;
    }
    nc::wgpu_view_remove(wgpu_view);
}

/// Evaluate JS in a webview by id (resolves pointer, sets last-error if
/// missing). Mirrors the Zig `evaluateJavaScriptWithNoCompletion` export.
pub fn evaluate_javascript(webview_id: u32, js: &CString) {
    let webview = REGISTRIES.webview_ptr(webview_id);
    if webview.is_null() {
        set_last_error(format!("Webview {webview_id} not found"));
        return;
    }
    nc::evaluate_javascript_with_no_completion(webview, js.as_ptr());
}

/// Load managed HTML, branching on renderer kind exactly like the Zig
/// `loadManagedHTMLForWebview`: CEF webviews go through `setWebviewHTMLContent`
/// + a load of the internal index URL; native webviews load the HTML directly.
pub fn load_managed_html(webview_id: u32, html: &CString) {
    let Some(state) = REGISTRIES.webview_state(webview_id) else {
        return;
    };
    if state.renderer == WebviewRendererKind::Cef {
        nc::set_webview_html_content(webview_id, html.as_ptr());
        if let Ok(internal_url) = CString::new("views://internal/index.html") {
            let webview = REGISTRIES.webview_ptr(webview_id);
            if !webview.is_null() {
                nc::load_url_in_webview(webview, internal_url.as_ptr());
            }
        }
        return;
    }
    let webview = REGISTRIES.webview_ptr(webview_id);
    if webview.is_null() {
        set_last_error(format!("Webview {webview_id} not found"));
        return;
    }
    nc::load_html_in_webview(webview, html.as_ptr());
}

/// Build the host-webview-event JS and run it in the host webview. Returns
/// false if the webview has no host id or the JS could not be built. Mirrors
/// `dispatchHostWebviewEvent` + `buildHostWebviewEventJavascript`.
pub fn dispatch_host_webview_event(webview_id: u32, event_name: &str, detail: &str) -> bool {
    let Some(host_webview_id) = REGISTRIES.webview_host_id(webview_id) else {
        return false;
    };

    let encoded_event_name = json_quote(event_name);
    // For these two events the detail is already a JSON value and is emitted
    // raw; otherwise the detail is JSON-quoted as a string. Matches the Zig.
    let js = if event_name == "new-window-open" || event_name == "host-message" {
        format!(
            "document.querySelector('#electrobun-webview-{webview_id}').emit({encoded_event_name}, {detail});"
        )
    } else {
        let encoded_detail = json_quote(detail);
        format!(
            "document.querySelector('#electrobun-webview-{webview_id}').emit({encoded_event_name}, {encoded_detail});"
        )
    };

    let Ok(js_c) = CString::new(js) else {
        set_last_error("Failed to allocate javascript string");
        return false;
    };
    evaluate_javascript(host_webview_id, &js_c);
    true
}

/// Build the internal-message JS (`receiveInternalMessageFromHost(...)`) and
/// run it in the target webview. Mirrors `sendInternalMessageToWebview` +
/// `buildInternalMessageJavascript`.
pub fn send_internal_message_to_webview(webview_id: u32, message_json: &str) -> bool {
    let js = format!("window.__electrobun.receiveInternalMessageFromHost({message_json});");
    let Ok(js_c) = CString::new(js) else {
        set_last_error("Failed to allocate javascript string");
        return false;
    };
    evaluate_javascript(webview_id, &js_c);
    true
}

/// Create the native tray for an already-stored tray state (sets ptr/visible
/// and applies the menu). Mirrors `createNativeTrayForState`. Returns the new
/// native tray pointer, or `None` (last-error set) on failure.
#[allow(clippy::too_many_arguments)]
pub fn create_native_tray_for_state(
    tray_id: u32,
    title: &CString,
    image: &CString,
    is_template: bool,
    width: u32,
    height: u32,
    handler: Option<StatusItemHandler>,
    menu_config: Option<&CString>,
) -> Option<crate::ffi_types::TrayPtr> {
    crate::native_wrapper::lookup_native_symbol("createTray")?;
    let tray_ptr = nc::create_tray(
        tray_id,
        title.as_ptr(),
        image.as_ptr(),
        is_template,
        width,
        height,
        handler,
    );
    if tray_ptr.is_null() {
        set_last_error("Failed to create tray");
        return None;
    }
    if let Some(menu_config) = menu_config {
        crate::native_wrapper::lookup_native_symbol("setTrayMenu")?;
        nc::set_tray_menu(tray_ptr, menu_config.as_ptr());
    }
    Some(tray_ptr)
}
