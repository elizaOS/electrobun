//! The [`Core`] facade: a safe, typed wrapper over libElectrobunCore.
//!
//! Ports the Zig `Core` struct. [`Core::load`] opens the library and resolves
//! the full [`Symbols`] table; every method forwards to a stored fn pointer.
//!
//! ## Error model
//! Mutating calls mirror the Zig `ensureLastCallSucceeded`: after the call we
//! poll `electrobun_core_last_error`, and a non-empty message becomes
//! [`ElectrobunError::Core`]. Calls that return an id use `0` as the failure
//! sentinel (then read `last_error`); calls returning an optional C string use
//! null as the sentinel.
//!
//! ## C-string ownership
//! Verified against `package/src/core/main.zig`: the string-returning exports
//! (`getPrimaryDisplay`, `getAllDisplays`, `getCursorScreenPoint`,
//! `clipboardReadText`, `clipboardAvailableFormats`, `sessionGetCookies`,
//! `openFileDialog`, `getTrayBounds`) all forward a pointer owned by core or
//! the native platform layer (static buffers / autoreleased strings / the
//! `empty_rect_json` sentinel). The Zig SDK only `std.mem.span`s and copies
//! them — it never frees them. `freeCoreString` exists solely for
//! `popNextQueuedHostMessage`, which the SDK does not call. We therefore copy
//! every returned string into an owned Rust value and never free the original.

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};

use libloading::Library;

use crate::error::{ElectrobunError, Result};
use crate::handlers::{
    AppReopenHandler, GlobalShortcutHandler, QuitRequestedHandler, StatusItemHandler,
    URLOpenHandler,
};
use crate::paths::BundlePaths;
use crate::symbols::Symbols;
use crate::types::{
    AppInfo, Cookie, Display, MessageBoxOptions, NotificationOptions, OpenFileDialogOptions, Point,
    Rect, TrayOptions, WGPUViewOptions, WebviewOptions, WindowOptions,
};

/// The platform-specific libElectrobunCore filename.
fn core_lib_name() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "ElectrobunCore.dll"
    }
    #[cfg(target_os = "macos")]
    {
        "libElectrobunCore.dylib"
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        "libElectrobunCore.so"
    }
}

/// Safe, typed handle to libElectrobunCore.
///
/// Owns the [`Library`] and its resolved [`Symbols`]; both drop together so the
/// stored fn pointers never outlive the loaded library.
pub struct Core {
    // Field order matters: `symbols` (holding fn pointers into `library`) must
    // drop before `library`. Rust drops fields in declaration order.
    symbols: Symbols,
    #[allow(dead_code)]
    library: Library,
}

impl Core {
    /// Open libElectrobunCore from the bundle's executable directory and
    /// resolve every symbol. Fails if the library or any symbol is missing.
    pub fn load() -> Result<Core> {
        let bundle_paths = crate::paths::resolve_bundle_paths()?;
        Self::load_from(&bundle_paths)
    }

    /// Open libElectrobunCore from a specific bundle layout.
    pub fn load_from(bundle_paths: &BundlePaths) -> Result<Core> {
        let lib_path = bundle_paths.exe_dir.join(core_lib_name());
        // SAFETY: opening a shared library executes its initializers; the path
        // is the app's own bundled core, trusted by construction.
        let library =
            unsafe { Library::new(&lib_path) }.map_err(|source| ElectrobunError::LibraryOpen {
                path: lib_path.display().to_string(),
                source,
            })?;
        // SAFETY: `library` is libElectrobunCore; signatures are verified
        // against package/src/core/main.zig.
        let symbols = unsafe { Symbols::load(&library)? };
        Ok(Core { symbols, library })
    }

    /// Read `electrobun_core_last_error` as a borrowed string.
    fn last_error(&self) -> String {
        // SAFETY: core guarantees a NUL-terminated static string (possibly "").
        let ptr = (self.symbols.last_error)();
        unsafe { cstr_to_string(ptr) }
    }

    /// Mirror of Zig `ensureLastCallSucceeded`.
    fn ensure_last_call_succeeded(&self) -> Result<()> {
        let message = self.last_error();
        if message.is_empty() {
            Ok(())
        } else {
            Err(ElectrobunError::Core(message))
        }
    }

    /// Turn the last-error message into an [`ElectrobunError`], with a generic
    /// fallback when core left it empty (mirrors Zig `errorFromLastError`).
    fn error_from_last(&self) -> ElectrobunError {
        let message = self.last_error();
        if message.is_empty() {
            ElectrobunError::Core("electrobun core failure".to_string())
        } else {
            ElectrobunError::Core(message)
        }
    }

    // -- Runtime configuration --------------------------------------------

    /// Configure the webview runtime from preload scripts in the bundle's
    /// `Resources` directory. Mirrors `configureWebviewRuntimeFromExecutableDir`.
    pub fn configure_webview_runtime_from_executable_dir(
        &self,
        bundle_paths: &BundlePaths,
        rpc_port: u32,
    ) -> Result<()> {
        let full_path = bundle_paths.resources_dir.join("preload-full.js");
        let sandboxed_path = bundle_paths.resources_dir.join("preload-sandboxed.js");

        let full_preload = read_file_cstring(&full_path)?;
        let sandboxed_preload = read_file_cstring(&sandboxed_path)?;

        let ok = (self.symbols.configure_webview_runtime)(
            rpc_port,
            full_preload.as_ptr(),
            sandboxed_preload.as_ptr(),
        );
        if ok {
            Ok(())
        } else {
            Err(self.error_from_last())
        }
    }

    /// Compute the default packed window style mask. Mirrors `defaultWindowStyle`.
    pub fn default_window_style(&self) -> u32 {
        (self.symbols.get_window_style)(
            false, true, true, true, true, false, false, false, false, false, false, false,
        )
    }

    // -- Window lifecycle --------------------------------------------------

    /// Create a native window. Returns the new window id.
    pub fn create_window(&self, options: &WindowOptions<'_>) -> Result<u32> {
        let title = CString::new(options.title).map_err(nul_err)?;
        let title_bar_style = CString::new(options.title_bar_style).map_err(nul_err)?;

        let style = &options.style;
        let style_mask = (self.symbols.get_window_style)(
            style.borderless,
            style.titled,
            style.closable,
            style.miniaturizable,
            style.resizable,
            style.unified_title_and_toolbar,
            style.full_screen,
            style.full_size_content_view,
            style.utility_window,
            style.doc_modal_window,
            style.nonactivating_panel,
            style.hud_window,
        );

        let window_id = (self.symbols.create_window)(
            options.frame.x,
            options.frame.y,
            options.frame.width,
            options.frame.height,
            style_mask,
            title_bar_style.as_ptr(),
            options.transparent,
            title.as_ptr(),
            options.hidden,
            options.activate,
            options.traffic_light_offset.x,
            options.traffic_light_offset.y,
            options.callbacks.close,
            options.callbacks.move_,
            options.callbacks.resize,
            options.callbacks.focus,
            options.callbacks.blur,
            options.callbacks.key,
        );

        if window_id == 0 {
            Err(self.error_from_last())
        } else {
            Ok(window_id)
        }
    }

    /// Set a window's title.
    pub fn set_window_title(&self, window_id: u32, title: &str) -> Result<()> {
        let title = CString::new(title).map_err(nul_err)?;
        (self.symbols.set_window_title)(window_id, title.as_ptr());
        self.ensure_last_call_succeeded()
    }

    /// Minimize a window.
    pub fn minimize_window(&self, window_id: u32) -> Result<()> {
        (self.symbols.minimize_window)(window_id);
        self.ensure_last_call_succeeded()
    }

    /// Restore a minimized window.
    pub fn restore_window(&self, window_id: u32) -> Result<()> {
        (self.symbols.restore_window)(window_id);
        self.ensure_last_call_succeeded()
    }

    /// Whether a window is minimized.
    pub fn is_window_minimized(&self, window_id: u32) -> bool {
        (self.symbols.is_window_minimized)(window_id)
    }

    /// Maximize a window.
    pub fn maximize_window(&self, window_id: u32) -> Result<()> {
        (self.symbols.maximize_window)(window_id);
        self.ensure_last_call_succeeded()
    }

    /// Unmaximize a window.
    pub fn unmaximize_window(&self, window_id: u32) -> Result<()> {
        (self.symbols.unmaximize_window)(window_id);
        self.ensure_last_call_succeeded()
    }

    /// Whether a window is maximized.
    pub fn is_window_maximized(&self, window_id: u32) -> bool {
        (self.symbols.is_window_maximized)(window_id)
    }

    /// Set a window's full-screen state.
    pub fn set_window_full_screen(&self, window_id: u32, full_screen: bool) -> Result<()> {
        (self.symbols.set_window_full_screen)(window_id, full_screen);
        self.ensure_last_call_succeeded()
    }

    /// Whether a window is full-screen.
    pub fn is_window_full_screen(&self, window_id: u32) -> bool {
        (self.symbols.is_window_full_screen)(window_id)
    }

    /// Set a window's always-on-top state.
    pub fn set_window_always_on_top(&self, window_id: u32, always_on_top: bool) -> Result<()> {
        (self.symbols.set_window_always_on_top)(window_id, always_on_top);
        self.ensure_last_call_succeeded()
    }

    /// Whether a window is always on top.
    pub fn is_window_always_on_top(&self, window_id: u32) -> bool {
        (self.symbols.is_window_always_on_top)(window_id)
    }

    /// Set whether a window is visible on all workspaces (macOS).
    pub fn set_window_visible_on_all_workspaces(
        &self,
        window_id: u32,
        visible: bool,
    ) -> Result<()> {
        (self.symbols.set_window_visible_on_all_workspaces)(window_id, visible);
        self.ensure_last_call_succeeded()
    }

    /// Whether a window is visible on all workspaces (macOS).
    pub fn is_window_visible_on_all_workspaces(&self, window_id: u32) -> bool {
        (self.symbols.is_window_visible_on_all_workspaces)(window_id)
    }

    /// Show a window, optionally activating it.
    pub fn show_window(&self, window_id: u32, activate: bool) -> Result<()> {
        (self.symbols.show_window)(window_id, activate);
        self.ensure_last_call_succeeded()
    }

    /// Activate (focus) a window.
    pub fn activate_window(&self, window_id: u32) -> Result<()> {
        (self.symbols.activate_window)(window_id);
        self.ensure_last_call_succeeded()
    }

    /// Hide a window.
    pub fn hide_window(&self, window_id: u32) -> Result<()> {
        (self.symbols.hide_window)(window_id);
        self.ensure_last_call_succeeded()
    }

    /// Set the traffic-light button position (macOS).
    pub fn set_window_button_position(&self, window_id: u32, x: f64, y: f64) -> Result<()> {
        (self.symbols.set_window_button_position)(window_id, x, y);
        self.ensure_last_call_succeeded()
    }

    /// Move a window.
    pub fn set_window_position(&self, window_id: u32, x: f64, y: f64) -> Result<()> {
        (self.symbols.set_window_position)(window_id, x, y);
        self.ensure_last_call_succeeded()
    }

    /// Resize a window.
    pub fn set_window_size(&self, window_id: u32, width: f64, height: f64) -> Result<()> {
        (self.symbols.set_window_size)(window_id, width, height);
        self.ensure_last_call_succeeded()
    }

    /// Set a window's full frame.
    pub fn set_window_frame(&self, window_id: u32, frame: Rect) -> Result<()> {
        (self.symbols.set_window_frame)(window_id, frame.x, frame.y, frame.width, frame.height);
        self.ensure_last_call_succeeded()
    }

    /// Read a window's current frame via out-params.
    pub fn get_window_frame(&self, window_id: u32) -> Result<Rect> {
        let mut x: f64 = 0.0;
        let mut y: f64 = 0.0;
        let mut width: f64 = 0.0;
        let mut height: f64 = 0.0;
        (self.symbols.get_window_frame)(window_id, &mut x, &mut y, &mut width, &mut height);
        self.ensure_last_call_succeeded()?;
        Ok(Rect {
            x,
            y,
            width,
            height,
        })
    }

    /// Close a window.
    pub fn close_window(&self, window_id: u32) -> Result<()> {
        (self.symbols.close_window)(window_id);
        self.ensure_last_call_succeeded()
    }

    // -- Webview lifecycle -------------------------------------------------

    /// Create a webview. Returns the new webview id.
    pub fn create_webview(&self, options: &WebviewOptions<'_>) -> Result<u32> {
        let renderer = CString::new(options.renderer.as_tag()).map_err(nul_err)?;
        let url = CString::new(options.url).map_err(nul_err)?;
        let partition = CString::new(options.partition).map_err(nul_err)?;
        let secret_key = CString::new(options.secret_key).map_err(nul_err)?;
        let preload = CString::new(options.preload).map_err(nul_err)?;
        let views_root = CString::new(options.views_root).map_err(nul_err)?;

        // Mirror Zig: host_bridge takes precedence over bun_bridge.
        let host_or_bun = options
            .callbacks
            .host_bridge
            .or(options.callbacks.bun_bridge);

        let webview_id = (self.symbols.create_webview)(
            options.window_id,
            options.host_webview_id,
            renderer.as_ptr(),
            url.as_ptr(),
            options.frame.x,
            options.frame.y,
            options.frame.width,
            options.frame.height,
            options.auto_resize,
            partition.as_ptr(),
            options.callbacks.decide_navigation,
            options.callbacks.event,
            options.callbacks.event_bridge,
            host_or_bun,
            options.callbacks.internal_bridge,
            secret_key.as_ptr(),
            preload.as_ptr(),
            views_root.as_ptr(),
            options.sandbox,
            options.start_transparent,
            options.start_passthrough,
        );

        if webview_id == 0 {
            Err(self.error_from_last())
        } else {
            Ok(webview_id)
        }
    }

    /// Resize a webview, masking out the given (JSON-encoded) rects.
    pub fn resize_webview(&self, webview_id: u32, frame: Rect, masks_json: &str) -> Result<()> {
        let masks = CString::new(masks_json).map_err(nul_err)?;
        (self.symbols.resize_webview)(
            webview_id,
            frame.x,
            frame.y,
            frame.width,
            frame.height,
            masks.as_ptr(),
        );
        self.ensure_last_call_succeeded()
    }

    /// Load a URL into a webview.
    pub fn load_url_in_webview(&self, webview_id: u32, url: &str) -> Result<()> {
        let url = CString::new(url).map_err(nul_err)?;
        (self.symbols.load_url_in_webview)(webview_id, url.as_ptr());
        self.ensure_last_call_succeeded()
    }

    /// Load inline HTML into a webview.
    pub fn load_html_in_webview(&self, webview_id: u32, html: &str) -> Result<()> {
        let html = CString::new(html).map_err(nul_err)?;
        (self.symbols.load_html_in_webview)(webview_id, html.as_ptr());
        self.ensure_last_call_succeeded()
    }

    /// Install/replace a named preload script on a webview.
    pub fn update_preload_script_to_webview(
        &self,
        webview_id: u32,
        script_identifier: &str,
        script: &str,
        all_frames: bool,
    ) -> Result<()> {
        let identifier = CString::new(script_identifier).map_err(nul_err)?;
        let script = CString::new(script).map_err(nul_err)?;
        (self.symbols.update_preload_script_to_webview)(
            webview_id,
            identifier.as_ptr(),
            script.as_ptr(),
            all_frames,
        );
        self.ensure_last_call_succeeded()
    }

    /// Whether the webview can navigate back.
    pub fn can_webview_go_back(&self, webview_id: u32) -> bool {
        (self.symbols.webview_can_go_back)(webview_id)
    }

    /// Whether the webview can navigate forward.
    pub fn can_webview_go_forward(&self, webview_id: u32) -> bool {
        (self.symbols.webview_can_go_forward)(webview_id)
    }

    /// Navigate the webview back.
    pub fn webview_go_back(&self, webview_id: u32) -> Result<()> {
        (self.symbols.webview_go_back)(webview_id);
        self.ensure_last_call_succeeded()
    }

    /// Navigate the webview forward.
    pub fn webview_go_forward(&self, webview_id: u32) -> Result<()> {
        (self.symbols.webview_go_forward)(webview_id);
        self.ensure_last_call_succeeded()
    }

    /// Reload the webview.
    pub fn reload_webview(&self, webview_id: u32) -> Result<()> {
        (self.symbols.webview_reload)(webview_id);
        self.ensure_last_call_succeeded()
    }

    /// Remove (destroy) the webview.
    pub fn remove_webview(&self, webview_id: u32) -> Result<()> {
        (self.symbols.webview_remove)(webview_id);
        self.ensure_last_call_succeeded()
    }

    /// Replace the webview's HTML content.
    pub fn set_webview_html_content(&self, webview_id: u32, html: &str) -> Result<()> {
        let html = CString::new(html).map_err(nul_err)?;
        (self.symbols.set_webview_html_content)(webview_id, html.as_ptr());
        self.ensure_last_call_succeeded()
    }

    /// Set the webview's transparency.
    pub fn set_webview_transparent(&self, webview_id: u32, transparent: bool) -> Result<()> {
        (self.symbols.webview_set_transparent)(webview_id, transparent);
        self.ensure_last_call_succeeded()
    }

    /// Set whether the webview passes through pointer events.
    pub fn set_webview_passthrough(&self, webview_id: u32, passthrough: bool) -> Result<()> {
        (self.symbols.webview_set_passthrough)(webview_id, passthrough);
        self.ensure_last_call_succeeded()
    }

    /// Set the webview's hidden state.
    pub fn set_webview_hidden(&self, webview_id: u32, hidden: bool) -> Result<()> {
        (self.symbols.webview_set_hidden)(webview_id, hidden);
        self.ensure_last_call_succeeded()
    }

    /// Set the webview's navigation rules (JSON-encoded).
    pub fn set_webview_navigation_rules(&self, webview_id: u32, rules_json: &str) -> Result<()> {
        let rules = CString::new(rules_json).map_err(nul_err)?;
        (self.symbols.set_webview_navigation_rules)(webview_id, rules.as_ptr());
        self.ensure_last_call_succeeded()
    }

    /// Find text in the webview's page.
    pub fn webview_find_in_page(
        &self,
        webview_id: u32,
        search_text: &str,
        forward: bool,
        match_case: bool,
    ) -> Result<()> {
        let search_text = CString::new(search_text).map_err(nul_err)?;
        (self.symbols.webview_find_in_page)(webview_id, search_text.as_ptr(), forward, match_case);
        self.ensure_last_call_succeeded()
    }

    /// Stop an in-progress find.
    pub fn webview_stop_find(&self, webview_id: u32) -> Result<()> {
        (self.symbols.webview_stop_find)(webview_id);
        self.ensure_last_call_succeeded()
    }

    /// Open the webview devtools.
    pub fn open_webview_dev_tools(&self, webview_id: u32) -> Result<()> {
        (self.symbols.webview_open_devtools)(webview_id);
        self.ensure_last_call_succeeded()
    }

    /// Close the webview devtools.
    pub fn close_webview_dev_tools(&self, webview_id: u32) -> Result<()> {
        (self.symbols.webview_close_devtools)(webview_id);
        self.ensure_last_call_succeeded()
    }

    /// Toggle the webview devtools.
    pub fn toggle_webview_dev_tools(&self, webview_id: u32) -> Result<()> {
        (self.symbols.webview_toggle_devtools)(webview_id);
        self.ensure_last_call_succeeded()
    }

    /// Set the webview's page zoom factor.
    pub fn set_webview_page_zoom(&self, webview_id: u32, zoom_level: f64) -> Result<()> {
        (self.symbols.webview_set_page_zoom)(webview_id, zoom_level);
        self.ensure_last_call_succeeded()
    }

    /// Read the webview's page zoom factor.
    pub fn get_webview_page_zoom(&self, webview_id: u32) -> f64 {
        (self.symbols.webview_get_page_zoom)(webview_id)
    }

    /// Evaluate JavaScript in the webview without awaiting a result.
    pub fn evaluate_javascript_with_no_completion(&self, webview_id: u32, js: &str) -> Result<()> {
        let js = CString::new(js).map_err(nul_err)?;
        (self.symbols.evaluate_javascript_with_no_completion)(webview_id, js.as_ptr());
        self.ensure_last_call_succeeded()
    }

    /// Send a host message (already JSON-encoded) to the webview, falling back
    /// to a direct `receiveMessageFromHost` JS call if the transport is not
    /// ready. Mirrors Zig `sendHostMessageToWebview`.
    pub fn send_host_message_to_webview(&self, webview_id: u32, message_json: &str) -> Result<()> {
        let message = CString::new(message_json).map_err(nul_err)?;
        if (self.symbols.send_host_message_to_webview_via_transport)(webview_id, message.as_ptr()) {
            return Ok(());
        }
        let js = format!("window.__electrobun.receiveMessageFromHost({message_json});");
        self.evaluate_javascript_with_no_completion(webview_id, &js)
    }

    /// Alias for [`Core::send_host_message_to_webview`]. Mirrors Zig
    /// `sendMessageToWebview`.
    pub fn send_message_to_webview(&self, webview_id: u32, message_json: &str) -> Result<()> {
        self.send_host_message_to_webview(webview_id, message_json)
    }

    /// Send an internal message (already JSON-encoded) to the webview.
    pub fn send_internal_message_to_webview(
        &self,
        webview_id: u32,
        message_json: &str,
    ) -> Result<()> {
        let message = CString::new(message_json).map_err(nul_err)?;
        if (self.symbols.send_internal_message_to_webview)(webview_id, message.as_ptr()) {
            Ok(())
        } else {
            Err(self.error_from_last())
        }
    }

    // -- WGPU views --------------------------------------------------------

    /// Create a WGPU view. Returns the new view id.
    pub fn create_wgpu_view(&self, options: &WGPUViewOptions) -> Result<u32> {
        let wgpu_view_id = (self.symbols.create_wgpu_view)(
            options.window_id,
            options.frame.x,
            options.frame.y,
            options.frame.width,
            options.frame.height,
            options.auto_resize,
            options.start_transparent,
            options.start_passthrough,
        );
        if wgpu_view_id == 0 {
            Err(self.error_from_last())
        } else {
            Ok(wgpu_view_id)
        }
    }

    /// Set a WGPU view's frame.
    pub fn set_wgpu_view_frame(&self, wgpu_view_id: u32, frame: Rect) -> Result<()> {
        (self.symbols.set_wgpu_view_frame)(
            wgpu_view_id,
            frame.x,
            frame.y,
            frame.width,
            frame.height,
        );
        self.ensure_last_call_succeeded()
    }

    /// Resize a WGPU view, masking out the given (JSON-encoded) rects.
    pub fn resize_wgpu_view(&self, wgpu_view_id: u32, frame: Rect, masks_json: &str) -> Result<()> {
        let masks = CString::new(masks_json).map_err(nul_err)?;
        (self.symbols.resize_wgpu_view)(
            wgpu_view_id,
            frame.x,
            frame.y,
            frame.width,
            frame.height,
            masks.as_ptr(),
        );
        self.ensure_last_call_succeeded()
    }

    /// Set a WGPU view's transparency.
    pub fn set_wgpu_view_transparent(&self, wgpu_view_id: u32, transparent: bool) -> Result<()> {
        (self.symbols.set_wgpu_view_transparent)(wgpu_view_id, transparent);
        self.ensure_last_call_succeeded()
    }

    /// Set whether a WGPU view passes through pointer events.
    pub fn set_wgpu_view_passthrough(&self, wgpu_view_id: u32, passthrough: bool) -> Result<()> {
        (self.symbols.set_wgpu_view_passthrough)(wgpu_view_id, passthrough);
        self.ensure_last_call_succeeded()
    }

    /// Set a WGPU view's hidden state.
    pub fn set_wgpu_view_hidden(&self, wgpu_view_id: u32, hidden: bool) -> Result<()> {
        (self.symbols.set_wgpu_view_hidden)(wgpu_view_id, hidden);
        self.ensure_last_call_succeeded()
    }

    /// Remove (destroy) a WGPU view.
    pub fn remove_wgpu_view(&self, wgpu_view_id: u32) -> Result<()> {
        (self.symbols.remove_wgpu_view)(wgpu_view_id);
        self.ensure_last_call_succeeded()
    }

    /// Get the opaque native view pointer backing a WGPU view.
    pub fn get_wgpu_view_pointer(&self, wgpu_view_id: u32) -> Result<*mut c_void> {
        let handle = (self.symbols.get_wgpu_view_pointer)(wgpu_view_id);
        self.ensure_last_call_succeeded()?;
        Ok(handle)
    }

    /// Get the opaque native handle for a WGPU view.
    pub fn get_wgpu_view_native_handle(&self, wgpu_view_id: u32) -> Result<*mut c_void> {
        let handle = (self.symbols.get_wgpu_view_native_handle)(wgpu_view_id);
        self.ensure_last_call_succeeded()?;
        Ok(handle)
    }

    /// Run the built-in WGPU view smoke test.
    pub fn run_wgpu_view_test(&self, wgpu_view_id: u32) -> Result<()> {
        (self.symbols.run_wgpu_view_test)(wgpu_view_id);
        self.ensure_last_call_succeeded()
    }

    /// Toggle the WGPU view's test shader.
    pub fn toggle_wgpu_view_test_shader(&self, wgpu_view_id: u32) -> Result<()> {
        (self.symbols.toggle_wgpu_view_test_shader)(wgpu_view_id);
        self.ensure_last_call_succeeded()
    }

    // -- Tray --------------------------------------------------------------

    /// Create a tray (status-bar) item. Returns the new tray id.
    pub fn create_tray(&self, options: &TrayOptions<'_>) -> Result<u32> {
        let title = CString::new(options.title).map_err(nul_err)?;
        let image = CString::new(options.image).map_err(nul_err)?;
        let tray_id = (self.symbols.create_tray)(
            title.as_ptr(),
            image.as_ptr(),
            options.is_template,
            options.width,
            options.height,
            None,
        );
        if tray_id == 0 {
            Err(self.error_from_last())
        } else {
            Ok(tray_id)
        }
    }

    /// Show a tray item.
    pub fn show_tray(&self, tray_id: u32) -> Result<()> {
        if (self.symbols.show_tray)(tray_id) {
            Ok(())
        } else {
            Err(self.error_from_last())
        }
    }

    /// Hide a tray item.
    pub fn hide_tray(&self, tray_id: u32) -> Result<()> {
        (self.symbols.hide_tray)(tray_id);
        self.ensure_last_call_succeeded()
    }

    /// Set a tray item's title.
    pub fn set_tray_title(&self, tray_id: u32, title: &str) -> Result<()> {
        let title = CString::new(title).map_err(nul_err)?;
        (self.symbols.set_tray_title)(tray_id, title.as_ptr());
        self.ensure_last_call_succeeded()
    }

    /// Read a tray item's bounds. Core returns JSON (or the empty-rect
    /// sentinel); we copy and parse it.
    pub fn get_tray_bounds(&self, tray_id: u32) -> Result<Rect> {
        let ptr = (self.symbols.get_tray_bounds)(tray_id);
        // SAFETY: core returns a NUL-terminated JSON string it owns.
        let json = unsafe { cstr_to_string(ptr) };
        parse_rect_json(&json)
    }

    /// Remove (destroy) a tray item.
    pub fn remove_tray(&self, tray_id: u32) -> Result<()> {
        (self.symbols.remove_tray)(tray_id);
        self.ensure_last_call_succeeded()
    }

    // -- Menus -------------------------------------------------------------

    /// Set the application menu from a JSON config.
    pub fn set_application_menu_json(
        &self,
        menu_json: &str,
        handler: Option<StatusItemHandler>,
    ) -> Result<()> {
        let menu = CString::new(menu_json).map_err(nul_err)?;
        (self.symbols.set_application_menu)(menu.as_ptr(), handler);
        self.ensure_last_call_succeeded()
    }

    /// Show a context menu from a JSON config.
    pub fn show_context_menu_json(
        &self,
        menu_json: &str,
        handler: Option<StatusItemHandler>,
    ) -> Result<()> {
        let menu = CString::new(menu_json).map_err(nul_err)?;
        (self.symbols.show_context_menu)(menu.as_ptr(), handler);
        self.ensure_last_call_succeeded()
    }

    // -- Dock / displays / cursor -----------------------------------------

    /// Set the dock icon's visibility (macOS).
    pub fn set_dock_icon_visible(&self, visible: bool) -> Result<()> {
        (self.symbols.set_dock_icon_visible)(visible);
        self.ensure_last_call_succeeded()
    }

    /// Whether the dock icon is visible (macOS).
    pub fn is_dock_icon_visible(&self) -> bool {
        (self.symbols.is_dock_icon_visible)()
    }

    /// Read the primary display.
    pub fn get_primary_display(&self) -> Result<Display> {
        let ptr = (self.symbols.get_primary_display)();
        let json = require_cstring(ptr, &self.error_from_last())?;
        parse_json(&json, "getPrimaryDisplay")
    }

    /// Read all connected displays.
    pub fn get_all_displays(&self) -> Result<Vec<Display>> {
        let ptr = (self.symbols.get_all_displays)();
        let json = require_cstring(ptr, &self.error_from_last())?;
        parse_json(&json, "getAllDisplays")
    }

    /// Read the current cursor position in screen coordinates.
    pub fn get_cursor_screen_point(&self) -> Result<Point> {
        let ptr = (self.symbols.get_cursor_screen_point)();
        let json = require_cstring(ptr, &self.error_from_last())?;
        parse_json(&json, "getCursorScreenPoint")
    }

    // -- Shell / files -----------------------------------------------------

    /// Move a path to the trash. Returns whether it succeeded.
    pub fn move_to_trash(&self, path: &str) -> Result<bool> {
        let path = CString::new(path).map_err(nul_err)?;
        Ok((self.symbols.move_to_trash)(path.as_ptr()))
    }

    /// Reveal a path in the system file manager.
    pub fn show_item_in_folder(&self, path: &str) -> Result<()> {
        let path = CString::new(path).map_err(nul_err)?;
        (self.symbols.show_item_in_folder)(path.as_ptr());
        self.ensure_last_call_succeeded()
    }

    /// Open a URL with the default external handler. Returns whether it
    /// succeeded.
    pub fn open_external(&self, url: &str) -> Result<bool> {
        let url = CString::new(url).map_err(nul_err)?;
        Ok((self.symbols.open_external)(url.as_ptr()))
    }

    /// Open a path with its default application. Returns whether it succeeded.
    pub fn open_path(&self, path: &str) -> Result<bool> {
        let path = CString::new(path).map_err(nul_err)?;
        Ok((self.symbols.open_path)(path.as_ptr()))
    }

    /// Show an open-file dialog. Returns the (possibly empty) selection string
    /// exactly as core formats it. Mirrors Zig `openFileDialog`.
    pub fn open_file_dialog(&self, options: &OpenFileDialogOptions<'_>) -> Result<String> {
        let starting_folder = CString::new(options.starting_folder).map_err(nul_err)?;
        let allowed_file_types = CString::new(options.allowed_file_types).map_err(nul_err)?;

        let ptr = (self.symbols.open_file_dialog)(
            starting_folder.as_ptr(),
            allowed_file_types.as_ptr(),
            c_int::from(options.can_choose_files),
            c_int::from(options.can_choose_directory),
            c_int::from(options.allows_multiple_selection),
        );
        if ptr.is_null() {
            return Ok(String::new());
        }
        // SAFETY: non-null pointer to a NUL-terminated string core owns.
        Ok(unsafe { cstr_to_string(ptr) })
    }

    /// Show a native message box. Returns the index of the clicked button.
    pub fn show_message_box(&self, options: &MessageBoxOptions<'_>) -> Result<i32> {
        let box_type = CString::new(options.box_type).map_err(nul_err)?;
        let title = CString::new(options.title).map_err(nul_err)?;
        let message = CString::new(options.message).map_err(nul_err)?;
        let detail = CString::new(options.detail).map_err(nul_err)?;
        let buttons_joined = options.buttons.join(",");
        let buttons = CString::new(buttons_joined).map_err(nul_err)?;

        let response = (self.symbols.show_message_box)(
            box_type.as_ptr(),
            title.as_ptr(),
            message.as_ptr(),
            detail.as_ptr(),
            buttons.as_ptr(),
            options.default_id,
            options.cancel_id,
        );
        self.ensure_last_call_succeeded()?;
        Ok(response)
    }

    /// Show a desktop notification.
    pub fn show_notification(&self, options: &NotificationOptions<'_>) -> Result<()> {
        let title = CString::new(options.title).map_err(nul_err)?;
        let body = CString::new(options.body).map_err(nul_err)?;
        let subtitle = CString::new(options.subtitle).map_err(nul_err)?;
        (self.symbols.show_notification)(
            title.as_ptr(),
            body.as_ptr(),
            subtitle.as_ptr(),
            options.silent,
        );
        self.ensure_last_call_succeeded()
    }

    // -- Clipboard ---------------------------------------------------------

    /// Read clipboard text. Returns `None` when the clipboard has no text.
    pub fn clipboard_read_text(&self) -> Option<String> {
        let ptr = (self.symbols.clipboard_read_text)();
        if ptr.is_null() {
            return None;
        }
        // SAFETY: non-null pointer to a NUL-terminated string core owns.
        Some(unsafe { cstr_to_string(ptr) })
    }

    /// Write text to the clipboard.
    pub fn clipboard_write_text(&self, text: &str) -> Result<()> {
        let text = CString::new(text).map_err(nul_err)?;
        (self.symbols.clipboard_write_text)(text.as_ptr());
        self.ensure_last_call_succeeded()
    }

    /// Clear the clipboard.
    pub fn clipboard_clear(&self) -> Result<()> {
        (self.symbols.clipboard_clear)();
        self.ensure_last_call_succeeded()
    }

    /// Read available clipboard formats as a CSV string (empty when none).
    pub fn clipboard_available_formats_csv(&self) -> String {
        let ptr = (self.symbols.clipboard_available_formats)();
        if ptr.is_null() {
            return String::new();
        }
        // SAFETY: non-null pointer to a NUL-terminated string core owns.
        unsafe { cstr_to_string(ptr) }
    }

    // -- Sessions / cookies ------------------------------------------------

    /// Read cookies for a partition matching a JSON filter. Mirrors Zig
    /// `sessionGetCookies`.
    pub fn session_get_cookies(&self, partition: &str, filter_json: &str) -> Result<Vec<Cookie>> {
        let partition = CString::new(partition).map_err(nul_err)?;
        let filter = CString::new(filter_json).map_err(nul_err)?;
        let ptr = (self.symbols.session_get_cookies)(partition.as_ptr(), filter.as_ptr());
        if ptr.is_null() {
            return Ok(Vec::new());
        }
        // SAFETY: non-null pointer to a NUL-terminated JSON string core owns.
        let json = unsafe { cstr_to_string(ptr) };
        parse_json(&json, "sessionGetCookies")
    }

    /// Set a cookie (already JSON-encoded) on a partition. Returns success.
    pub fn session_set_cookie(&self, partition: &str, cookie_json: &str) -> Result<bool> {
        let partition = CString::new(partition).map_err(nul_err)?;
        let cookie = CString::new(cookie_json).map_err(nul_err)?;
        Ok((self.symbols.session_set_cookie)(
            partition.as_ptr(),
            cookie.as_ptr(),
        ))
    }

    /// Remove a cookie by URL + name from a partition. Returns success.
    pub fn session_remove_cookie(&self, partition: &str, url: &str, name: &str) -> Result<bool> {
        let partition = CString::new(partition).map_err(nul_err)?;
        let url = CString::new(url).map_err(nul_err)?;
        let name = CString::new(name).map_err(nul_err)?;
        Ok((self.symbols.session_remove_cookie)(
            partition.as_ptr(),
            url.as_ptr(),
            name.as_ptr(),
        ))
    }

    /// Clear all cookies for a partition.
    pub fn session_clear_cookies(&self, partition: &str) -> Result<()> {
        let partition = CString::new(partition).map_err(nul_err)?;
        (self.symbols.session_clear_cookies)(partition.as_ptr());
        self.ensure_last_call_succeeded()
    }

    /// Clear storage data for a partition (storage types JSON-encoded).
    pub fn session_clear_storage_data(
        &self,
        partition: &str,
        storage_types_json: &str,
    ) -> Result<()> {
        let partition = CString::new(partition).map_err(nul_err)?;
        let storage_types = CString::new(storage_types_json).map_err(nul_err)?;
        (self.symbols.session_clear_storage_data)(partition.as_ptr(), storage_types.as_ptr());
        self.ensure_last_call_succeeded()
    }

    // -- Global shortcuts --------------------------------------------------

    /// Install the global-shortcut activation callback.
    pub fn set_global_shortcut_callback(
        &self,
        callback: Option<GlobalShortcutHandler>,
    ) -> Result<()> {
        (self.symbols.set_global_shortcut_callback)(callback);
        self.ensure_last_call_succeeded()
    }

    /// Register a global shortcut accelerator. Returns success.
    pub fn register_global_shortcut(&self, accelerator: &str) -> Result<bool> {
        let accelerator = CString::new(accelerator).map_err(nul_err)?;
        Ok((self.symbols.register_global_shortcut)(
            accelerator.as_ptr(),
        ))
    }

    /// Unregister a global shortcut accelerator. Returns success.
    pub fn unregister_global_shortcut(&self, accelerator: &str) -> Result<bool> {
        let accelerator = CString::new(accelerator).map_err(nul_err)?;
        Ok((self.symbols.unregister_global_shortcut)(
            accelerator.as_ptr(),
        ))
    }

    /// Unregister every global shortcut.
    pub fn unregister_all_global_shortcuts(&self) -> Result<()> {
        (self.symbols.unregister_all_global_shortcuts)();
        self.ensure_last_call_succeeded()
    }

    /// Whether a global shortcut accelerator is registered.
    pub fn is_global_shortcut_registered(&self, accelerator: &str) -> Result<bool> {
        let accelerator = CString::new(accelerator).map_err(nul_err)?;
        Ok((self.symbols.is_global_shortcut_registered)(
            accelerator.as_ptr(),
        ))
    }

    // -- App lifecycle handlers -------------------------------------------

    /// Install the URL-open handler.
    pub fn set_url_open_handler(&self, handler: Option<URLOpenHandler>) -> Result<()> {
        (self.symbols.set_url_open_handler)(handler);
        self.ensure_last_call_succeeded()
    }

    /// Install the app-reopen handler.
    pub fn set_app_reopen_handler(&self, handler: Option<AppReopenHandler>) -> Result<()> {
        (self.symbols.set_app_reopen_handler)(handler);
        self.ensure_last_call_succeeded()
    }

    /// Install the quit-requested handler.
    pub fn set_quit_requested_handler(&self, handler: Option<QuitRequestedHandler>) -> Result<()> {
        (self.symbols.set_quit_requested_handler)(handler);
        self.ensure_last_call_succeeded()
    }

    /// Ask the event loop to stop.
    pub fn stop_event_loop(&self) -> Result<()> {
        (self.symbols.stop_event_loop)();
        self.ensure_last_call_succeeded()
    }

    /// Block until shutdown completes or `timeout_ms` elapses.
    pub fn wait_for_shutdown_complete(&self, timeout_ms: c_int) -> Result<()> {
        (self.symbols.wait_for_shutdown_complete)(timeout_ms);
        self.ensure_last_call_succeeded()
    }

    /// Force the process to exit. Calls core then [`std::process::exit`].
    pub fn force_exit(&self, code: c_int) -> ! {
        (self.symbols.force_exit)(code);
        std::process::exit(code)
    }

    /// Stop the loop, wait up to 5s, then force-exit. Mirrors Zig
    /// `quitGracefully`.
    pub fn quit_gracefully(&self, code: c_int) -> ! {
        let _ = self.stop_event_loop();
        let _ = self.wait_for_shutdown_complete(5000);
        self.force_exit(code)
    }

    // -- WGPU surface (main-thread) ---------------------------------------

    /// Create a WGPU surface for a native view pointer.
    pub fn wgpu_create_surface_for_view(
        &self,
        instance_ptr: *mut c_void,
        view_ptr: *mut c_void,
    ) -> Result<*mut c_void> {
        let surface = (self.symbols.wgpu_create_surface_for_view)(instance_ptr, view_ptr);
        self.ensure_last_call_succeeded()?;
        Ok(surface)
    }

    /// Create an adapter/device pair on the main thread, writing both into
    /// `out_adapter_device` (a `*mut [usize; 2]`).
    pub fn wgpu_create_adapter_device_main_thread(
        &self,
        instance_ptr: *mut c_void,
        surface_ptr: *mut c_void,
        out_adapter_device: *mut c_void,
    ) -> Result<()> {
        (self.symbols.wgpu_create_adapter_device_main_thread)(
            instance_ptr,
            surface_ptr,
            out_adapter_device,
        );
        self.ensure_last_call_succeeded()
    }

    /// Configure a WGPU surface on the main thread.
    pub fn wgpu_surface_configure_main_thread(
        &self,
        surface_ptr: *mut c_void,
        config_ptr: *mut c_void,
    ) -> Result<()> {
        (self.symbols.wgpu_surface_configure_main_thread)(surface_ptr, config_ptr);
        self.ensure_last_call_succeeded()
    }

    /// Acquire a surface's current texture on the main thread.
    pub fn wgpu_surface_get_current_texture_main_thread(
        &self,
        surface_ptr: *mut c_void,
        surface_texture_ptr: *mut c_void,
    ) -> Result<()> {
        (self.symbols.wgpu_surface_get_current_texture_main_thread)(
            surface_ptr,
            surface_texture_ptr,
        );
        self.ensure_last_call_succeeded()
    }

    /// Present a WGPU surface on the main thread.
    pub fn wgpu_surface_present_main_thread(&self, surface_ptr: *mut c_void) -> Result<i32> {
        let result = (self.symbols.wgpu_surface_present_main_thread)(surface_ptr);
        self.ensure_last_call_succeeded()?;
        Ok(result)
    }

    // -- Main loop ---------------------------------------------------------

    /// Run the native main thread / event loop. Mirrors Zig `runMainThread`.
    pub fn run_main_thread(&self, app_info: AppInfo<'_>) -> Result<()> {
        let identifier = CString::new(app_info.identifier).map_err(nul_err)?;
        let name = CString::new(app_info.name).map_err(nul_err)?;
        let channel = CString::new(app_info.channel).map_err(nul_err)?;

        let status =
            (self.symbols.run_main_thread)(identifier.as_ptr(), name.as_ptr(), channel.as_ptr(), 0);
        if status != 0 {
            Err(self.error_from_last())
        } else {
            Ok(())
        }
    }
}

/// Map a `CString::new` interior-NUL error to [`ElectrobunError::Core`].
fn nul_err(_: std::ffi::NulError) -> ElectrobunError {
    ElectrobunError::Core("string argument contained an interior NUL byte".to_string())
}

/// Copy a NUL-terminated C string into an owned [`String`] (lossy on invalid
/// UTF-8). Does NOT free the source — see the module C-string ownership note.
///
/// # Safety
/// `ptr` must be a valid, NUL-terminated C string for the duration of the call.
unsafe fn cstr_to_string(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: caller guarantees a valid NUL-terminated string.
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}

/// Require a non-null C string, copying it; otherwise clone the supplied error.
fn require_cstring(ptr: *const c_char, on_null: &ElectrobunError) -> Result<String> {
    if ptr.is_null() {
        return Err(match on_null {
            ElectrobunError::Core(msg) => ElectrobunError::Core(msg.clone()),
            _ => ElectrobunError::Core("electrobun core failure".to_string()),
        });
    }
    // SAFETY: non-null pointer to a NUL-terminated string core owns.
    Ok(unsafe { cstr_to_string(ptr) })
}

/// Parse a JSON document returned by core into `T`.
fn parse_json<T: serde::de::DeserializeOwned>(json: &str, context: &'static str) -> Result<T> {
    serde_json::from_str(json).map_err(|source| ElectrobunError::Json {
        context: context.to_string(),
        source,
    })
}

/// Parse a `Rect` from core's tray-bounds JSON. Mirrors Zig `parseRectJson`,
/// which requires all four numeric fields to be present.
fn parse_rect_json(json: &str) -> Result<Rect> {
    #[derive(serde::Deserialize)]
    struct RawRect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    }
    let raw: RawRect = serde_json::from_str(json).map_err(|source| ElectrobunError::Json {
        context: "getTrayBounds".to_string(),
        source,
    })?;
    Ok(Rect {
        x: raw.x,
        y: raw.y,
        width: raw.width,
        height: raw.height,
    })
}

/// Read a file into a [`CString`] (rejecting interior NULs). Mirrors Zig
/// `readFileZ`.
fn read_file_cstring(path: &std::path::Path) -> Result<CString> {
    let bytes = std::fs::read(path).map_err(|source| ElectrobunError::Io {
        context: format!("read {}", path.display()),
        source,
    })?;
    CString::new(bytes).map_err(|_| {
        ElectrobunError::Core(format!("{} contained an interior NUL byte", path.display()))
    })
}
