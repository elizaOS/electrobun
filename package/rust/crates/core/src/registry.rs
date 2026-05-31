//! ID → native-pointer registries for windows, webviews, wgpu views, and
//! trays, plus the per-kind id counters.
//!
//! Every registry is `Mutex`-guarded because callback trampolines fire on the
//! C++ main thread while the FFI exports run on the Bun worker thread. The
//! stored native pointers (`*mut c_void`) are not themselves `Send`, so the
//! whole map is wrapped in a newtype with explicit `unsafe impl Send`/`Sync`:
//! the pointers are only ever dereferenced by the native code, never by Rust,
//! and access is serialized by the mutex.

use std::collections::HashMap;
use std::sync::Mutex;

use once_cell::sync::Lazy;

use crate::ffi_types::{
    StatusItemHandler, TrayPtr, WebviewPostMessageHandler, WebviewPtr, WebviewRendererKind,
    WgpuViewPtr, WindowBlurHandler, WindowCloseHandler, WindowFocusHandler, WindowKeyHandler,
    WindowMoveHandler, WindowPtr, WindowResizeHandler,
};

/// AES-256-GCM key length.
pub const SECRET_KEY_LEN: usize = 32;
pub type WebviewSecretKey = [u8; SECRET_KEY_LEN];

#[derive(Clone)]
pub struct WindowState {
    pub ptr: WindowPtr,
    pub transparent: bool,
    pub close_handler: Option<WindowCloseHandler>,
    pub move_handler: Option<WindowMoveHandler>,
    pub resize_handler: Option<WindowResizeHandler>,
    pub focus_handler: Option<WindowFocusHandler>,
    pub blur_handler: Option<WindowBlurHandler>,
    pub key_handler: Option<WindowKeyHandler>,
}

#[derive(Clone)]
pub struct WebviewState {
    pub ptr: WebviewPtr,
    pub window_id: u32,
    pub host_webview_id: Option<u32>,
    pub renderer: WebviewRendererKind,
    pub secret_key: WebviewSecretKey,
    /// Raw socket fd of the live websocket connection for this webview, if any.
    pub socket_handle: Option<i32>,
    pub transport_ready: bool,
}

#[derive(Clone)]
pub struct WgpuViewState {
    pub ptr: WgpuViewPtr,
    pub window_id: u32,
}

pub struct TrayState {
    pub title: std::ffi::CString,
    pub image: std::ffi::CString,
    pub menu_config: Option<std::ffi::CString>,
    pub is_template: bool,
    pub width: u32,
    pub height: u32,
    pub handler: Option<StatusItemHandler>,
    pub ptr: TrayPtr,
    pub visible: bool,
}

/// Snapshot of a webview's transport fields (mirrors the Zig
/// `WebviewTransportContext`), copied out under the lock to avoid holding it
/// across socket I/O.
#[derive(Clone, Copy)]
pub struct WebviewTransportContext {
    pub secret_key: WebviewSecretKey,
    pub socket_handle: Option<i32>,
    pub transport_ready: bool,
}

/// Default webview callbacks remembered from the first `createWebview` call, so
/// internally-created webview tags reuse the same handler set. Mirrors the Zig
/// `DefaultWebviewCallbacks`.
#[derive(Clone, Copy, Default)]
pub struct DefaultWebviewCallbacks {
    pub navigation_callback: Option<crate::ffi_types::DecideNavigationHandler>,
    pub webview_event_handler: Option<crate::ffi_types::WebviewEventHandler>,
    pub event_bridge_handler: Option<WebviewPostMessageHandler>,
}

/// Newtype that asserts thread-safety for a map holding raw native pointers.
/// Access is always under the outer `Mutex`, and Rust never dereferences the
/// pointers (only the native wrapper does).
struct PtrMap<V>(HashMap<u32, V>);

// SAFETY: see module docs — pointers are opaque to Rust and the map is only
// touched while holding the surrounding mutex.
unsafe impl<V> Send for PtrMap<V> {}

struct Counter(u32);

pub struct Registries {
    windows: Mutex<PtrMap<WindowState>>,
    webviews: Mutex<PtrMap<WebviewState>>,
    wgpu_views: Mutex<PtrMap<WgpuViewState>>,
    trays: Mutex<PtrMap<TrayState>>,
    next_window_id: Mutex<Counter>,
    next_webview_id: Mutex<Counter>,
    next_wgpu_view_id: Mutex<Counter>,
    next_tray_id: Mutex<Counter>,
    default_webview_callbacks: Mutex<DefaultWebviewCallbacks>,
}

pub static REGISTRIES: Lazy<Registries> = Lazy::new(|| Registries {
    windows: Mutex::new(PtrMap(HashMap::new())),
    webviews: Mutex::new(PtrMap(HashMap::new())),
    wgpu_views: Mutex::new(PtrMap(HashMap::new())),
    trays: Mutex::new(PtrMap(HashMap::new())),
    next_window_id: Mutex::new(Counter(1)),
    next_webview_id: Mutex::new(Counter(1)),
    next_wgpu_view_id: Mutex::new(Counter(1)),
    next_tray_id: Mutex::new(Counter(1)),
    default_webview_callbacks: Mutex::new(DefaultWebviewCallbacks::default()),
});

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Allocate the next free id using the Zig's wrapping scheme: never 0, skip
/// ids already present in `map`, and wrap `u32` back to 1. Returns `None` if
/// the id space is exhausted.
fn allocate_id<V>(counter: &mut Counter, map: &HashMap<u32, V>) -> Option<u32> {
    let start = counter.0;
    let mut id = counter.0;
    while id == 0 || map.contains_key(&id) {
        id = id.wrapping_add(1);
        if id == 0 {
            id = 1;
        }
        if id == start {
            return None;
        }
    }
    let mut next = id.wrapping_add(1);
    if next == 0 {
        next = 1;
    }
    counter.0 = next;
    Some(id)
}

impl Registries {
    // ---- windows ----

    /// Reserve an id and insert placeholder window state (ptr null), mirroring
    /// the Zig's "store first, fill ptr after native create" flow.
    pub fn insert_window_placeholder(&self, state: WindowState) -> Option<u32> {
        let mut counter = lock(&self.next_window_id);
        let mut map = lock(&self.windows);
        let id = allocate_id(&mut counter, &map.0)?;
        map.0.insert(id, state);
        Some(id)
    }

    pub fn set_window_ptr(&self, window_id: u32, ptr: WindowPtr) -> bool {
        let mut map = lock(&self.windows);
        match map.0.get_mut(&window_id) {
            Some(state) => {
                state.ptr = ptr;
                true
            }
            None => false,
        }
    }

    pub fn remove_window(&self, window_id: u32) -> Option<WindowState> {
        lock(&self.windows).0.remove(&window_id)
    }

    pub fn window_state(&self, window_id: u32) -> Option<WindowState> {
        lock(&self.windows).0.get(&window_id).cloned()
    }

    pub fn window_ptr(&self, window_id: u32) -> WindowPtr {
        lock(&self.windows)
            .0
            .get(&window_id)
            .map(|s| s.ptr)
            .unwrap_or(std::ptr::null_mut())
    }

    pub fn has_open_windows(&self) -> bool {
        !lock(&self.windows).0.is_empty()
    }

    // ---- webviews ----

    pub fn insert_webview_placeholder(&self, state: WebviewState) -> Option<u32> {
        let mut counter = lock(&self.next_webview_id);
        let mut map = lock(&self.webviews);
        let id = allocate_id(&mut counter, &map.0)?;
        map.0.insert(id, state);
        Some(id)
    }

    pub fn set_webview_ptr(&self, webview_id: u32, ptr: WebviewPtr) -> bool {
        let mut map = lock(&self.webviews);
        match map.0.get_mut(&webview_id) {
            Some(state) => {
                state.ptr = ptr;
                true
            }
            None => false,
        }
    }

    pub fn remove_webview(&self, webview_id: u32) -> Option<WebviewState> {
        lock(&self.webviews).0.remove(&webview_id)
    }

    pub fn webview_state(&self, webview_id: u32) -> Option<WebviewState> {
        lock(&self.webviews).0.get(&webview_id).cloned()
    }

    pub fn webview_ptr(&self, webview_id: u32) -> WebviewPtr {
        lock(&self.webviews)
            .0
            .get(&webview_id)
            .map(|s| s.ptr)
            .unwrap_or(std::ptr::null_mut())
    }

    pub fn webview_host_id(&self, webview_id: u32) -> Option<u32> {
        lock(&self.webviews)
            .0
            .get(&webview_id)
            .and_then(|s| s.host_webview_id)
    }

    pub fn webview_transport_context(&self, webview_id: u32) -> Option<WebviewTransportContext> {
        lock(&self.webviews)
            .0
            .get(&webview_id)
            .map(|s| WebviewTransportContext {
                secret_key: s.secret_key,
                socket_handle: s.socket_handle,
                transport_ready: s.transport_ready,
            })
    }

    pub fn webview_ids_for_window(&self, window_id: u32) -> Vec<u32> {
        lock(&self.webviews)
            .0
            .iter()
            .filter(|(_, s)| s.window_id == window_id)
            .map(|(id, _)| *id)
            .collect()
    }

    /// Attach a socket handle to a webview, returning the previous handle (if
    /// different) so the caller can close it outside the lock. Returns
    /// `Err(())` if the webview is unknown. Mirrors `attachWebviewSocketHandle`.
    #[allow(clippy::result_unit_err)]
    pub fn attach_webview_socket(&self, webview_id: u32, handle: i32) -> Result<Option<i32>, ()> {
        let mut map = lock(&self.webviews);
        let Some(state) = map.0.get_mut(&webview_id) else {
            return Err(());
        };
        let previous = match state.socket_handle {
            Some(prev) if prev != handle => Some(prev),
            _ => None,
        };
        state.socket_handle = Some(handle);
        state.transport_ready = false;
        Ok(previous)
    }

    /// Clear the socket handle only if it still matches `handle`; mirrors
    /// `clearWebviewSocketHandleIfCurrent`.
    pub fn clear_webview_socket_if_current(&self, webview_id: u32, handle: i32) {
        let mut map = lock(&self.webviews);
        if let Some(state) = map.0.get_mut(&webview_id) {
            if state.socket_handle == Some(handle) {
                state.socket_handle = None;
                state.transport_ready = false;
            }
        }
    }

    /// Detach and return the current socket handle; mirrors
    /// `closeAndClearWebviewSocketHandle` (caller closes the returned fd).
    pub fn take_webview_socket(&self, webview_id: u32) -> Option<i32> {
        let mut map = lock(&self.webviews);
        let state = map.0.get_mut(&webview_id)?;
        let handle = state.socket_handle.take();
        state.transport_ready = false;
        handle
    }

    pub fn mark_webview_transport_ready(&self, webview_id: u32, handle: i32) {
        let mut map = lock(&self.webviews);
        if let Some(state) = map.0.get_mut(&webview_id) {
            if state.socket_handle == Some(handle) {
                state.transport_ready = true;
            }
        }
    }

    // ---- wgpu views ----

    pub fn insert_wgpu_view_placeholder(&self, state: WgpuViewState) -> Option<u32> {
        let mut counter = lock(&self.next_wgpu_view_id);
        let mut map = lock(&self.wgpu_views);
        let id = allocate_id(&mut counter, &map.0)?;
        map.0.insert(id, state);
        Some(id)
    }

    pub fn set_wgpu_view_ptr(&self, wgpu_view_id: u32, ptr: WgpuViewPtr) -> bool {
        let mut map = lock(&self.wgpu_views);
        match map.0.get_mut(&wgpu_view_id) {
            Some(state) => {
                state.ptr = ptr;
                true
            }
            None => false,
        }
    }

    pub fn remove_wgpu_view(&self, wgpu_view_id: u32) -> Option<WgpuViewState> {
        lock(&self.wgpu_views).0.remove(&wgpu_view_id)
    }

    pub fn wgpu_view_ptr(&self, wgpu_view_id: u32) -> WgpuViewPtr {
        lock(&self.wgpu_views)
            .0
            .get(&wgpu_view_id)
            .map(|s| s.ptr)
            .unwrap_or(std::ptr::null_mut())
    }

    pub fn wgpu_view_ids_for_window(&self, window_id: u32) -> Vec<u32> {
        lock(&self.wgpu_views)
            .0
            .iter()
            .filter(|(_, s)| s.window_id == window_id)
            .map(|(id, _)| *id)
            .collect()
    }

    // ---- trays ----

    /// Insert tray state, allocating an id with the Zig's simple monotonic
    /// `next_tray_id += 1` scheme (no wrap/collision handling there).
    pub fn insert_tray(&self, state: TrayState) -> u32 {
        let mut counter = lock(&self.next_tray_id);
        let id = counter.0;
        counter.0 += 1;
        lock(&self.trays).0.insert(id, state);
        id
    }

    pub fn remove_tray(&self, tray_id: u32) -> Option<TrayState> {
        lock(&self.trays).0.remove(&tray_id)
    }

    /// Run `f` against the mutable tray state under the lock, returning its
    /// result, or `None` if the tray id is unknown.
    pub fn with_tray_mut<R>(&self, tray_id: u32, f: impl FnOnce(&mut TrayState) -> R) -> Option<R> {
        let mut map = lock(&self.trays);
        map.0.get_mut(&tray_id).map(f)
    }

    // ---- default webview callbacks ----

    pub fn remember_default_webview_callbacks(
        &self,
        navigation_callback: Option<crate::ffi_types::DecideNavigationHandler>,
        webview_event_handler: Option<crate::ffi_types::WebviewEventHandler>,
        event_bridge_handler: Option<WebviewPostMessageHandler>,
    ) {
        let mut slot = lock(&self.default_webview_callbacks);
        if navigation_callback.is_some() {
            slot.navigation_callback = navigation_callback;
        }
        if webview_event_handler.is_some() {
            slot.webview_event_handler = webview_event_handler;
        }
        if event_bridge_handler.is_some() {
            slot.event_bridge_handler = event_bridge_handler;
        }
    }

    pub fn default_webview_callbacks(&self) -> DefaultWebviewCallbacks {
        *lock(&self.default_webview_callbacks)
    }
}
