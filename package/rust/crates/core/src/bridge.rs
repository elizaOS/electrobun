//! Internal webview→core JSON bridge.
//!
//! The webview's internal bridge posts JSON packets (single objects or an array
//! of stringified objects) that core dispatches synchronously. `request`
//! packets get a `{type:"response",id,success,payload}` reply pushed back via
//! `window.__electrobun.receiveInternalMessageFromHost(...)`; `message` packets
//! are fire-and-forget native actions (resize, navigate, devtools, ...).
//!
//! This is a faithful port of the Zig `processInternalBridgeBatch` /
//! `handleInternalBridgePacket` / `handleInternalRequest` /
//! `handleInternalMessage` plus the `json*` coercion helpers, which accept the
//! same loose shapes (`number_string`, integral floats) the Zig did.

use std::ffi::CString;

use serde_json::Value;

use crate::ops;
use crate::registry::REGISTRIES;

// ---- JSON coercion helpers (mirror jsonString/jsonU32/jsonF64/jsonBool) ----

fn json_string(value: &Value) -> Option<&str> {
    match value {
        Value::String(s) => Some(s.as_str()),
        // serde_json has no `number_string`; numbers are handled by json_u32 /
        // json_f64. A bare number is not a string here (matches the Zig, which
        // only treated `.string`/`.number_string` as strings).
        _ => None,
    }
}

fn json_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(b) => Some(*b),
        _ => None,
    }
}

fn json_u32(value: &Value) -> Option<u32> {
    match value {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                if (0..=u32::MAX as i64).contains(&i) {
                    return Some(i as u32);
                }
                return None;
            }
            if let Some(f) = n.as_f64() {
                if !f.is_finite() || f.floor() != f || f < 0.0 || f > u32::MAX as f64 {
                    return None;
                }
                return Some(f as u32);
            }
            None
        }
        Value::String(s) => s.parse::<u32>().ok(),
        _ => None,
    }
}

fn json_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse::<f64>().ok(),
        _ => None,
    }
}

struct InternalRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

fn parse_internal_rect(value: &Value) -> Option<InternalRect> {
    let object = value.as_object()?;
    Some(InternalRect {
        x: json_f64(object.get("x")?)?,
        y: json_f64(object.get("y")?)?,
        width: json_f64(object.get("width")?)?,
        height: json_f64(object.get("height")?)?,
    })
}

/// Send a `{type:"response",...}` packet back to the requesting host webview.
/// `payload` is any serde_json value. Mirrors `sendInternalBridgeResponse`.
fn send_internal_bridge_response(
    host_webview_id: u32,
    request_id: &str,
    success: bool,
    payload: Value,
) {
    let encoded_id = serde_json::Value::String(request_id.to_string());
    let response = serde_json::json!({
        "type": "response",
        "id": encoded_id,
        "success": success,
        "payload": payload,
    });
    ops::send_internal_message_to_webview(host_webview_id, &response.to_string());
}

// ---- request handling ----

fn create_managed_webview_from_request(params: &Value) -> Option<u32> {
    let params_object = params.as_object()?;

    let callbacks = REGISTRIES.default_webview_callbacks();
    callbacks.webview_event_handler?;
    callbacks.event_bridge_handler?;

    let host_webview_id = json_u32(params_object.get("hostWebviewId")?)?;
    let window_id = json_u32(params_object.get("windowId")?)?;
    let renderer = json_string(params_object.get("renderer")?)?;
    let partition = params_object
        .get("partition")
        .and_then(json_string)
        .unwrap_or("persist:default");
    let frame = parse_internal_rect(params_object.get("frame")?)?;
    let sandbox = params_object
        .get("sandbox")
        .and_then(json_bool)
        .unwrap_or(false);
    let transparent = params_object
        .get("transparent")
        .and_then(json_bool)
        .unwrap_or(false);
    let passthrough = params_object
        .get("passthrough")
        .and_then(json_bool)
        .unwrap_or(false);

    let html_value = params_object.get("html");
    let html_present = matches!(html_value, Some(v) if !v.is_null());

    let url = params_object.get("url").and_then(json_string).unwrap_or("");
    let preload = params_object
        .get("preload")
        .and_then(json_string)
        .unwrap_or("");

    // Generate a fresh per-webview key and format it as the comma-separated
    // decimal list createWebview expects. Mirrors the Zig random key bytes.
    let mut secret_key = [0u8; crate::registry::SECRET_KEY_LEN];
    fill_random(&mut secret_key)?;
    let secret_key_csv = secret_key
        .iter()
        .map(|b| b.to_string())
        .collect::<Vec<_>>()
        .join(",");

    // When html is provided, the URL passed to createWebview is empty (HTML is
    // loaded afterward). Matches the Zig.
    let create_url = if html_present { "" } else { url };

    let renderer_c = CString::new(renderer).ok()?;
    let url_c = CString::new(create_url).ok()?;
    let partition_c = CString::new(partition).ok()?;
    let preload_c = CString::new(preload).ok()?;
    let views_root_c = CString::new("").ok()?;
    let secret_key_for_call = secret_key_csv.clone();

    let webview_id = ops::create_webview(
        window_id,
        host_webview_id,
        renderer,
        &renderer_c,
        &url_c,
        frame.x,
        frame.y,
        frame.width,
        frame.height,
        false,
        &partition_c,
        callbacks.navigation_callback,
        callbacks.webview_event_handler,
        callbacks.event_bridge_handler,
        &secret_key_for_call,
        &preload_c,
        &views_root_c,
        sandbox,
        transparent,
        passthrough,
    );
    if webview_id == 0 {
        return None;
    }

    if html_present {
        if let Some(html) = html_value.and_then(json_string) {
            if !html.is_empty() {
                if let Ok(html_c) = CString::new(html) {
                    ops::load_managed_html(webview_id, &html_c);
                }
            }
        }
    }

    if let Some(rules_value) = params_object.get("navigationRules") {
        if !rules_value.is_null() {
            let rules_json = rules_value.to_string();
            if let Ok(rules_c) = CString::new(rules_json) {
                let webview = REGISTRIES.webview_ptr(webview_id);
                if !webview.is_null() {
                    crate::native_calls::set_webview_navigation_rules(webview, rules_c.as_ptr());
                }
            }
        }
    }

    Some(webview_id)
}

fn create_managed_wgpu_view_from_request(params: &Value) -> Option<u32> {
    let params_object = params.as_object()?;
    let window_id = json_u32(params_object.get("windowId")?)?;
    let frame = parse_internal_rect(params_object.get("frame")?)?;
    let transparent = params_object
        .get("transparent")
        .and_then(json_bool)
        .unwrap_or(false);
    let passthrough = params_object
        .get("passthrough")
        .and_then(json_bool)
        .unwrap_or(false);

    let wgpu_view_id = ops::create_wgpu_view(
        window_id,
        frame.x,
        frame.y,
        frame.width,
        frame.height,
        false,
        transparent,
        passthrough,
    );
    if wgpu_view_id == 0 {
        None
    } else {
        Some(wgpu_view_id)
    }
}

fn handle_internal_request(request_object: &serde_json::Map<String, Value>) {
    let Some(method) = request_object.get("method").and_then(json_string) else {
        return;
    };
    let Some(request_id) = request_object.get("id").and_then(json_string) else {
        return;
    };
    let request_id = request_id.to_string();
    let Some(host_webview_id) = request_object.get("hostWebviewId").and_then(json_u32) else {
        return;
    };
    let params = request_object.get("params").cloned().unwrap_or(Value::Null);

    match method {
        "webviewTagInit" => match create_managed_webview_from_request(&params) {
            Some(webview_id) => send_internal_bridge_response(
                host_webview_id,
                &request_id,
                true,
                Value::from(webview_id),
            ),
            None => send_internal_bridge_response(
                host_webview_id,
                &request_id,
                false,
                Value::from("Failed to create webview tag"),
            ),
        },
        "wgpuTagInit" => match create_managed_wgpu_view_from_request(&params) {
            Some(wgpu_view_id) => send_internal_bridge_response(
                host_webview_id,
                &request_id,
                true,
                Value::from(wgpu_view_id),
            ),
            None => send_internal_bridge_response(
                host_webview_id,
                &request_id,
                false,
                Value::from("Failed to create WGPU view"),
            ),
        },
        "webviewTagCanGoBack" => {
            let Some(webview_id) = params
                .as_object()
                .and_then(|o| o.get("id"))
                .and_then(json_u32)
            else {
                return;
            };
            let webview = REGISTRIES.webview_ptr(webview_id);
            let can = !webview.is_null() && crate::native_calls::webview_can_go_back(webview);
            send_internal_bridge_response(host_webview_id, &request_id, true, Value::from(can));
        }
        "webviewTagCanGoForward" => {
            let Some(webview_id) = params
                .as_object()
                .and_then(|o| o.get("id"))
                .and_then(json_u32)
            else {
                return;
            };
            let webview = REGISTRIES.webview_ptr(webview_id);
            let can = !webview.is_null() && crate::native_calls::webview_can_go_forward(webview);
            send_internal_bridge_response(host_webview_id, &request_id, true, Value::from(can));
        }
        _ => send_internal_bridge_response(
            host_webview_id,
            &request_id,
            false,
            Value::from("Unknown internal request"),
        ),
    }
}

// ---- message handling ----

fn dispatch_stored_webview_event(payload: &Value) {
    let Some(payload_object) = payload.as_object() else {
        return;
    };
    let callbacks = REGISTRIES.default_webview_callbacks();
    let Some(handler) = callbacks.webview_event_handler else {
        return;
    };
    let Some(webview_id) = payload_object.get("id").and_then(json_u32) else {
        return;
    };
    let Some(event_name) = payload_object.get("eventName").and_then(json_string) else {
        return;
    };
    let Some(detail) = payload_object.get("detail").and_then(json_string) else {
        return;
    };
    let (Ok(event_name_c), Ok(detail_c)) = (CString::new(event_name), CString::new(detail)) else {
        return;
    };
    // SAFETY: handler is the JSCallback fn-pointer Bun registered; the CStrings
    // outlive the call.
    unsafe { handler(webview_id, event_name_c.as_ptr(), detail_c.as_ptr()) };
}

/// Resolve an `id` field to a webview pointer, or null. Helper for the
/// id-keyed message handlers below.
fn webview_id_field(object: &serde_json::Map<String, Value>) -> Option<u32> {
    object.get("id").and_then(json_u32)
}

fn handle_internal_message(message_id: &str, payload: &Value) {
    if message_id == "webviewEvent" {
        dispatch_stored_webview_event(payload);
        return;
    }

    let Some(object) = payload.as_object() else {
        return;
    };

    match message_id {
        "webviewTagResize" => {
            let Some(webview_id) = webview_id_field(object) else {
                return;
            };
            let Some(frame) = object.get("frame").and_then(parse_internal_rect) else {
                return;
            };
            let masks = object.get("masks").and_then(json_string).unwrap_or("[]");
            if let Ok(masks_c) = CString::new(masks) {
                let webview = REGISTRIES.webview_ptr(webview_id);
                if !webview.is_null() {
                    crate::native_calls::resize_webview(
                        webview,
                        frame.x,
                        frame.y,
                        frame.width,
                        frame.height,
                        masks_c.as_ptr(),
                    );
                }
            }
        }
        "wgpuTagResize" => {
            let Some(wgpu_view_id) = webview_id_field(object) else {
                return;
            };
            let Some(frame) = object.get("frame").and_then(parse_internal_rect) else {
                return;
            };
            let masks = object.get("masks").and_then(json_string).unwrap_or("[]");
            if let Ok(masks_c) = CString::new(masks) {
                let wgpu_view = REGISTRIES.wgpu_view_ptr(wgpu_view_id);
                if !wgpu_view.is_null() {
                    crate::native_calls::resize_wgpu_view(
                        wgpu_view,
                        frame.x,
                        frame.y,
                        frame.width,
                        frame.height,
                        masks_c.as_ptr(),
                    );
                }
            }
        }
        "webviewTagUpdateSrc" => {
            let Some(webview_id) = webview_id_field(object) else {
                return;
            };
            let Some(url) = object.get("url").and_then(json_string) else {
                return;
            };
            if let Ok(url_c) = CString::new(url) {
                let webview = REGISTRIES.webview_ptr(webview_id);
                if !webview.is_null() {
                    crate::native_calls::load_url_in_webview(webview, url_c.as_ptr());
                }
            }
        }
        "webviewTagUpdateHtml" => {
            let Some(webview_id) = webview_id_field(object) else {
                return;
            };
            let Some(html) = object.get("html").and_then(json_string) else {
                return;
            };
            if let Ok(html_c) = CString::new(html) {
                ops::load_managed_html(webview_id, &html_c);
            }
        }
        "webviewTagUpdatePreload" => {
            let Some(webview_id) = webview_id_field(object) else {
                return;
            };
            let Some(preload) = object.get("preload").and_then(json_string) else {
                return;
            };
            let (Ok(id_c), Ok(preload_c)) = (
                CString::new("electrobun_custom_preload_script"),
                CString::new(preload),
            ) else {
                return;
            };
            let webview = REGISTRIES.webview_ptr(webview_id);
            if !webview.is_null() {
                crate::native_calls::update_preload_script_to_webview(
                    webview,
                    id_c.as_ptr(),
                    preload_c.as_ptr(),
                    true,
                );
            }
        }
        "webviewTagGoBack" => {
            if let Some(webview_id) = webview_id_field(object) {
                let webview = REGISTRIES.webview_ptr(webview_id);
                if !webview.is_null() {
                    crate::native_calls::webview_go_back(webview);
                }
            }
        }
        "webviewTagGoForward" => {
            if let Some(webview_id) = webview_id_field(object) {
                let webview = REGISTRIES.webview_ptr(webview_id);
                if !webview.is_null() {
                    crate::native_calls::webview_go_forward(webview);
                }
            }
        }
        "webviewTagReload" => {
            if let Some(webview_id) = webview_id_field(object) {
                let webview = REGISTRIES.webview_ptr(webview_id);
                if !webview.is_null() {
                    crate::native_calls::webview_reload(webview);
                }
            }
        }
        "webviewTagRemove" => {
            if let Some(webview_id) = webview_id_field(object) {
                ops::webview_remove(webview_id);
            }
        }
        "startWindowMove" => {
            if let Some(window_id) = webview_id_field(object) {
                let window = REGISTRIES.window_ptr(window_id);
                if !window.is_null() {
                    crate::native_calls::start_window_move(window);
                }
            }
        }
        "stopWindowMove" => {
            crate::native_calls::stop_window_move();
        }
        "webviewTagSetTransparent" => {
            let Some(webview_id) = webview_id_field(object) else {
                return;
            };
            let Some(transparent) = object.get("transparent").and_then(json_bool) else {
                return;
            };
            let webview = REGISTRIES.webview_ptr(webview_id);
            if !webview.is_null() {
                crate::native_calls::webview_set_transparent(webview, transparent);
            }
        }
        "wgpuTagSetTransparent" => {
            let Some(wgpu_view_id) = webview_id_field(object) else {
                return;
            };
            let Some(transparent) = object.get("transparent").and_then(json_bool) else {
                return;
            };
            let wgpu_view = REGISTRIES.wgpu_view_ptr(wgpu_view_id);
            if !wgpu_view.is_null() {
                crate::native_calls::wgpu_view_set_transparent(wgpu_view, transparent);
            }
        }
        "webviewTagSetPassthrough" => {
            let Some(webview_id) = webview_id_field(object) else {
                return;
            };
            let Some(passthrough) = object.get("enablePassthrough").and_then(json_bool) else {
                return;
            };
            let webview = REGISTRIES.webview_ptr(webview_id);
            if !webview.is_null() {
                crate::native_calls::webview_set_passthrough(webview, passthrough);
            }
        }
        "wgpuTagSetPassthrough" => {
            let Some(wgpu_view_id) = webview_id_field(object) else {
                return;
            };
            let Some(passthrough) = object.get("passthrough").and_then(json_bool) else {
                return;
            };
            let wgpu_view = REGISTRIES.wgpu_view_ptr(wgpu_view_id);
            if !wgpu_view.is_null() {
                crate::native_calls::wgpu_view_set_passthrough(wgpu_view, passthrough);
            }
        }
        "webviewTagSetHidden" => {
            let Some(webview_id) = webview_id_field(object) else {
                return;
            };
            let Some(hidden) = object.get("hidden").and_then(json_bool) else {
                return;
            };
            let webview = REGISTRIES.webview_ptr(webview_id);
            if !webview.is_null() {
                crate::native_calls::webview_set_hidden(webview, hidden);
            }
        }
        "wgpuTagSetHidden" => {
            let Some(wgpu_view_id) = webview_id_field(object) else {
                return;
            };
            let Some(hidden) = object.get("hidden").and_then(json_bool) else {
                return;
            };
            let wgpu_view = REGISTRIES.wgpu_view_ptr(wgpu_view_id);
            if !wgpu_view.is_null() {
                crate::native_calls::wgpu_view_set_hidden(wgpu_view, hidden);
            }
        }
        "wgpuTagRemove" => {
            if let Some(wgpu_view_id) = webview_id_field(object) {
                ops::remove_wgpu_view(wgpu_view_id);
            }
        }
        "wgpuTagRunTest" => {
            if let Some(wgpu_view_id) = webview_id_field(object) {
                let wgpu_view = REGISTRIES.wgpu_view_ptr(wgpu_view_id);
                if !wgpu_view.is_null() {
                    crate::native_calls::wgpu_run_gpu_test(wgpu_view);
                }
            }
        }
        "webviewTagSetNavigationRules" => {
            let Some(webview_id) = webview_id_field(object) else {
                return;
            };
            let Some(rules_value) = object.get("rules") else {
                return;
            };
            let rules_json = rules_value.to_string();
            if let Ok(rules_c) = CString::new(rules_json) {
                let webview = REGISTRIES.webview_ptr(webview_id);
                if !webview.is_null() {
                    crate::native_calls::set_webview_navigation_rules(webview, rules_c.as_ptr());
                }
            }
        }
        "webviewTagFindInPage" => {
            let Some(webview_id) = webview_id_field(object) else {
                return;
            };
            let Some(search_text) = object.get("searchText").and_then(json_string) else {
                return;
            };
            let Some(forward) = object.get("forward").and_then(json_bool) else {
                return;
            };
            let Some(match_case) = object.get("matchCase").and_then(json_bool) else {
                return;
            };
            if let Ok(search_c) = CString::new(search_text) {
                let webview = REGISTRIES.webview_ptr(webview_id);
                if !webview.is_null() {
                    crate::native_calls::webview_find_in_page(
                        webview,
                        search_c.as_ptr(),
                        forward,
                        match_case,
                    );
                }
            }
        }
        "webviewTagStopFind" => {
            if let Some(webview_id) = webview_id_field(object) {
                let webview = REGISTRIES.webview_ptr(webview_id);
                if !webview.is_null() {
                    crate::native_calls::webview_stop_find(webview);
                }
            }
        }
        "webviewTagOpenDevTools" => {
            if let Some(webview_id) = webview_id_field(object) {
                let webview = REGISTRIES.webview_ptr(webview_id);
                if !webview.is_null() {
                    crate::native_calls::webview_open_devtools(webview);
                }
            }
        }
        "webviewTagCloseDevTools" => {
            if let Some(webview_id) = webview_id_field(object) {
                let webview = REGISTRIES.webview_ptr(webview_id);
                if !webview.is_null() {
                    crate::native_calls::webview_close_devtools(webview);
                }
            }
        }
        "webviewTagToggleDevTools" => {
            if let Some(webview_id) = webview_id_field(object) {
                let webview = REGISTRIES.webview_ptr(webview_id);
                if !webview.is_null() {
                    crate::native_calls::webview_toggle_devtools(webview);
                }
            }
        }
        "webviewTagExecuteJavascript" => {
            let Some(webview_id) = webview_id_field(object) else {
                return;
            };
            let Some(js) = object.get("js").and_then(json_string) else {
                return;
            };
            if let Ok(js_c) = CString::new(js) {
                ops::evaluate_javascript(webview_id, &js_c);
            }
        }
        _ => {}
    }
}

fn handle_internal_bridge_packet(packet: &Value) {
    let Some(object) = packet.as_object() else {
        return;
    };
    let Some(packet_type) = object.get("type").and_then(json_string) else {
        return;
    };
    match packet_type {
        "request" => handle_internal_request(object),
        "message" => {
            let Some(message_id) = object.get("id").and_then(json_string) else {
                return;
            };
            let message_id = message_id.to_string();
            let payload = object.get("payload").cloned().unwrap_or(Value::Null);
            handle_internal_message(&message_id, &payload);
        }
        _ => {}
    }
}

/// Entry point from the internal-bridge trampoline. A batch is a JSON array of
/// stringified packets; a single packet is dispatched directly. Mirrors
/// `processInternalBridgeBatch`.
pub fn process_internal_bridge_batch(message_json: &str) {
    let Ok(parsed) = serde_json::from_str::<Value>(message_json) else {
        return;
    };
    if let Value::Array(items) = &parsed {
        for item in items {
            let Some(item_json) = json_string(item) else {
                continue;
            };
            let Ok(nested) = serde_json::from_str::<Value>(item_json) else {
                continue;
            };
            handle_internal_bridge_packet(&nested);
        }
        return;
    }
    handle_internal_bridge_packet(&parsed);
}

fn fill_random(buf: &mut [u8]) -> Option<()> {
    use aes_gcm::aead::rand_core::RngCore;
    aes_gcm::aead::OsRng.try_fill_bytes(buf).ok()
}
