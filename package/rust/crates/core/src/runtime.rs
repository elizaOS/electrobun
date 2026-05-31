//! Webview runtime configuration + preload-script construction.
//!
//! `configureWebviewRuntime` stores the two compiled preload scripts (normal +
//! sandboxed) and starts the transport server. `buildElectrobunPreload`
//! prepends the per-webview `window.__electrobun*` globals — including the
//! AES key bytes and the rpc/host socket ports — to the stored script. The
//! injected JS is byte-for-byte identical to the Zig `allocPrintZ` templates so
//! the shipped webview client finds the globals it expects.

use std::sync::Mutex;

use once_cell::sync::Lazy;

use crate::error::set_last_error;

struct WebviewRuntimeState {
    rpc_port: u32,
    preload_script: Option<String>,
    preload_script_sandboxed: Option<String>,
    configured: bool,
}

static RUNTIME: Lazy<Mutex<WebviewRuntimeState>> = Lazy::new(|| {
    Mutex::new(WebviewRuntimeState {
        rpc_port: 0,
        preload_script: None,
        preload_script_sandboxed: None,
        configured: false,
    })
});

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poison| poison.into_inner())
}

pub fn set_rpc_port(port: u32) {
    lock(&RUNTIME).rpc_port = port;
}

/// Store the preload scripts and set the initial rpc_port. The transport server
/// is started by the caller (`configureWebviewRuntime` export). Returns nothing;
/// allocation cannot fail for owned `String`s.
pub fn configure(rpc_port: u32, preload_script: String, preload_script_sandboxed: String) {
    let mut state = lock(&RUNTIME);
    state.rpc_port = rpc_port;
    state.preload_script = Some(preload_script);
    state.preload_script_sandboxed = Some(preload_script_sandboxed);
}

pub fn mark_configured() {
    lock(&RUNTIME).configured = true;
}

/// Validate the runtime is configured with both preload scripts (mirrors
/// `ensureWebviewRuntimeConfigured`), recording the matching last-error on
/// failure.
fn ensure_configured(state: &WebviewRuntimeState) -> bool {
    if !state.configured {
        set_last_error("Webview runtime is not configured");
        return false;
    }
    if state.preload_script.is_none() || state.preload_script_sandboxed.is_none() {
        set_last_error("Webview runtime preload scripts are not configured");
        return false;
    }
    true
}

/// Build the per-webview preload script, prepending the `window.__electrobun*`
/// globals to the configured (sandboxed or normal) script. `secret_key_csv` is
/// the comma-separated decimal byte list exactly as Bun passed it to
/// `createWebview` (it becomes the JS array literal `[...]`). Returns `None`
/// (last-error set) if the runtime is not configured.
pub fn build_preload(
    webview_id: u32,
    window_id: u32,
    secret_key_csv: &str,
    sandbox: bool,
) -> Option<String> {
    let state = lock(&RUNTIME);
    if !ensure_configured(&state) {
        return None;
    }

    if sandbox {
        let sandboxed = state.preload_script_sandboxed.as_ref().unwrap();
        return Some(format!(
            "window.__electrobunWebviewId = {webview_id};\n\
window.__electrobunWindowId = {window_id};\n\
window.__electrobunEventBridge = window.__electrobunEventBridge || window.webkit?.messageHandlers?.eventBridge || window.eventBridge || window.chrome?.webview?.hostObjects?.eventBridge;\n\
window.__electrobunInternalBridge = window.__electrobunInternalBridge || window.webkit?.messageHandlers?.internalBridge || window.internalBridge || window.chrome?.webview?.hostObjects?.internalBridge;\n\
{sandboxed}"
        ));
    }

    let preload = state.preload_script.as_ref().unwrap();
    let rpc_port = state.rpc_port;
    Some(format!(
        "window.__electrobunWebviewId = {webview_id};\n\
window.__electrobunWindowId = {window_id};\n\
window.__electrobunHostSocketPort = {rpc_port};\n\
window.__electrobunRpcSocketPort = {rpc_port};\n\
window.__electrobunSecretKeyBytes = [{secret_key_csv}];\n\
window.__electrobunEventBridge = window.__electrobunEventBridge || window.webkit?.messageHandlers?.eventBridge || window.eventBridge || window.chrome?.webview?.hostObjects?.eventBridge;\n\
window.__electrobunInternalBridge = window.__electrobunInternalBridge || window.webkit?.messageHandlers?.internalBridge || window.internalBridge || window.chrome?.webview?.hostObjects?.internalBridge;\n\
window.__electrobunHostBridge = window.__electrobunHostBridge || window.__electrobunBunBridge || window.webkit?.messageHandlers?.hostBridge || window.webkit?.messageHandlers?.bunBridge || window.hostBridge || window.bunBridge || window.chrome?.webview?.hostObjects?.hostBridge || window.chrome?.webview?.hostObjects?.bunBridge;\n\
window.__electrobunBunBridge = window.__electrobunBunBridge || window.webkit?.messageHandlers?.bunBridge || window.bunBridge || window.chrome?.webview?.hostObjects?.bunBridge;\n\
{preload}"
    ))
}
