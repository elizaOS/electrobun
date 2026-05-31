//! Plain value types: structs and enums passed across the SDK surface.
//!
//! These map 1:1 to the value declarations in `package/src/zig-sdk/electrobun.zig`.
//! Serde derives are present only where the Zig SDK serialized the type to JSON
//! (cookies, displays, cursor point) or parsed it from JSON (cookie filter,
//! storage types, bundle `version.json`).

use serde::{Deserialize, Serialize};

/// Web renderer backend. Serializes to its lowercase tag (`"native"` / `"cef"`),
/// matching the Zig `@tagName` used when passing the renderer to core.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Renderer {
    /// Platform-native webview (WKWebView / WebView2 / WebKitGTK).
    #[default]
    Native,
    /// Chromium Embedded Framework.
    Cef,
}

impl Renderer {
    /// The lowercase tag passed across the C ABI (`"native"` / `"cef"`).
    pub fn as_tag(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Cef => "cef",
        }
    }
}

/// Borrowed application identity passed to core at launch.
#[derive(Debug, Clone, Copy)]
pub struct AppInfo<'a> {
    /// Reverse-DNS bundle identifier (e.g. `com.example.app`).
    pub identifier: &'a str,
    /// Human-readable application name.
    pub name: &'a str,
    /// Release channel (e.g. `stable`, `canary`).
    pub channel: &'a str,
}

/// Owned application identity, typically parsed from the bundle's `version.json`.
///
/// Replaces the Zig `OwnedAppInfo` with `deinit`; ownership is handled by `Drop`.
#[derive(Debug, Clone, Deserialize)]
pub struct OwnedAppInfo {
    /// Reverse-DNS bundle identifier.
    pub identifier: String,
    /// Human-readable application name.
    pub name: String,
    /// Release channel.
    pub channel: String,
}

impl OwnedAppInfo {
    /// Borrow this owned identity as an [`AppInfo`].
    pub fn borrowed(&self) -> AppInfo<'_> {
        AppInfo {
            identifier: &self.identifier,
            name: &self.name,
            channel: &self.channel,
        }
    }
}

/// A rectangle in screen/window coordinates. Matches the Zig defaults
/// (`width = 800`, `height = 600`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    /// Left edge.
    #[serde(default)]
    pub x: f64,
    /// Top edge.
    #[serde(default)]
    pub y: f64,
    /// Width.
    #[serde(default = "default_width")]
    pub width: f64,
    /// Height.
    #[serde(default = "default_height")]
    pub height: f64,
}

fn default_width() -> f64 {
    800.0
}
fn default_height() -> f64 {
    600.0
}

impl Default for Rect {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        }
    }
}

/// Offset of the macOS traffic-light buttons from their default position.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TrafficLightOffset {
    /// Horizontal offset.
    pub x: f64,
    /// Vertical offset.
    pub y: f64,
}

/// Window style flags, expanded into a packed style mask by `getWindowStyle`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowStyle {
    /// No border / title bar chrome.
    pub borderless: bool,
    /// Show a title bar.
    pub titled: bool,
    /// Allow the close button.
    pub closable: bool,
    /// Allow minimization.
    pub miniaturizable: bool,
    /// Allow resizing.
    pub resizable: bool,
    /// Unify the title bar and toolbar (macOS).
    pub unified_title_and_toolbar: bool,
    /// Start full-screen.
    pub full_screen: bool,
    /// Extend content under the title bar (macOS).
    pub full_size_content_view: bool,
    /// Utility-window panel (macOS).
    pub utility_window: bool,
    /// Document-modal panel (macOS).
    pub doc_modal_window: bool,
    /// Non-activating panel (macOS).
    pub nonactivating_panel: bool,
    /// HUD-style panel (macOS).
    pub hud_window: bool,
}

impl Default for WindowStyle {
    fn default() -> Self {
        Self {
            borderless: false,
            titled: true,
            closable: true,
            miniaturizable: true,
            resizable: true,
            unified_title_and_toolbar: false,
            full_screen: false,
            full_size_content_view: false,
            utility_window: false,
            doc_modal_window: false,
            nonactivating_panel: false,
            hud_window: false,
        }
    }
}

/// Per-window native event callbacks. `None` slots are not installed.
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowCallbacks {
    /// Close callback.
    pub close: Option<crate::handlers::WindowCloseHandler>,
    /// Move callback.
    pub move_: Option<crate::handlers::WindowMoveHandler>,
    /// Resize callback.
    pub resize: Option<crate::handlers::WindowResizeHandler>,
    /// Focus callback.
    pub focus: Option<crate::handlers::WindowFocusHandler>,
    /// Blur callback.
    pub blur: Option<crate::handlers::WindowBlurHandler>,
    /// Key callback.
    pub key: Option<crate::handlers::WindowKeyHandler>,
}

/// Options for [`crate::Core::create_window`].
#[derive(Debug, Clone)]
pub struct WindowOptions<'a> {
    /// Window title.
    pub title: &'a str,
    /// Initial frame.
    pub frame: Rect,
    /// Style flags.
    pub style: WindowStyle,
    /// Title-bar style hint (e.g. `"default"`, `"hiddenInset"`).
    pub title_bar_style: &'a str,
    /// Transparent background.
    pub transparent: bool,
    /// Create hidden.
    pub hidden: bool,
    /// Activate on show.
    pub activate: bool,
    /// Traffic-light offset (macOS).
    pub traffic_light_offset: TrafficLightOffset,
    /// Native event callbacks.
    pub callbacks: WindowCallbacks,
}

impl<'a> WindowOptions<'a> {
    /// Construct options with the Zig defaults for everything but `title`.
    pub fn new(title: &'a str) -> Self {
        Self {
            title,
            frame: Rect::default(),
            style: WindowStyle::default(),
            title_bar_style: "default",
            transparent: false,
            hidden: false,
            activate: true,
            traffic_light_offset: TrafficLightOffset::default(),
            callbacks: WindowCallbacks::default(),
        }
    }
}

/// Per-webview bridge/navigation callbacks.
#[derive(Debug, Clone, Copy, Default)]
pub struct WebviewCallbacks {
    /// Navigation gate callback.
    pub decide_navigation: Option<crate::handlers::DecideNavigationHandler>,
    /// Generic event callback.
    pub event: Option<crate::handlers::WebviewEventHandler>,
    /// Event-transport bridge callback.
    pub event_bridge: Option<crate::handlers::WebviewPostMessageHandler>,
    /// Host bridge callback (takes precedence over `bun_bridge`).
    pub host_bridge: Option<crate::handlers::WebviewPostMessageHandler>,
    /// Bun bridge callback (used when `host_bridge` is `None`).
    pub bun_bridge: Option<crate::handlers::WebviewPostMessageHandler>,
    /// Internal bridge callback.
    pub internal_bridge: Option<crate::handlers::WebviewPostMessageHandler>,
}

/// Options for [`crate::Core::create_webview`].
#[derive(Debug, Clone)]
pub struct WebviewOptions<'a> {
    /// Owning window id.
    pub window_id: u32,
    /// Host webview id for nested `<electrobun-webview>` tags (`0` = none).
    pub host_webview_id: u32,
    /// Renderer backend.
    pub renderer: Renderer,
    /// Initial URL.
    pub url: &'a str,
    /// Initial frame.
    pub frame: Rect,
    /// Auto-resize with the window.
    pub auto_resize: bool,
    /// Storage partition (e.g. `persist:default`).
    pub partition: &'a str,
    /// Bridge/navigation callbacks.
    pub callbacks: WebviewCallbacks,
    /// Transport secret key (comma-separated bytes).
    pub secret_key: &'a str,
    /// Custom preload script.
    pub preload: &'a str,
    /// Root for `views://` resolution.
    pub views_root: &'a str,
    /// Run in a sandboxed content world.
    pub sandbox: bool,
    /// Start transparent.
    pub start_transparent: bool,
    /// Start click-through (passthrough).
    pub start_passthrough: bool,
}

impl<'a> WebviewOptions<'a> {
    /// Construct options with the Zig defaults for the given window.
    pub fn new(window_id: u32) -> Self {
        Self {
            window_id,
            host_webview_id: 0,
            renderer: Renderer::Native,
            url: "",
            frame: Rect::default(),
            auto_resize: true,
            partition: "persist:default",
            callbacks: WebviewCallbacks::default(),
            secret_key: "",
            preload: "",
            views_root: "",
            sandbox: true,
            start_transparent: false,
            start_passthrough: false,
        }
    }
}

/// Options for [`crate::Core::create_wgpu_view`].
#[derive(Debug, Clone, Copy)]
pub struct WGPUViewOptions {
    /// Owning window id.
    pub window_id: u32,
    /// Initial frame.
    pub frame: Rect,
    /// Auto-resize with the window.
    pub auto_resize: bool,
    /// Start transparent.
    pub start_transparent: bool,
    /// Start click-through (passthrough).
    pub start_passthrough: bool,
}

impl WGPUViewOptions {
    /// Construct options with the Zig defaults for the given window.
    pub fn new(window_id: u32) -> Self {
        Self {
            window_id,
            frame: Rect::default(),
            auto_resize: true,
            start_transparent: false,
            start_passthrough: false,
        }
    }
}

/// Options for [`crate::Core::create_tray`].
#[derive(Debug, Clone)]
pub struct TrayOptions<'a> {
    /// Tray title text.
    pub title: &'a str,
    /// Image path / `views://` URL.
    pub image: &'a str,
    /// Treat the image as a macOS template image.
    pub is_template: bool,
    /// Icon width.
    pub width: u32,
    /// Icon height.
    pub height: u32,
}

impl<'a> TrayOptions<'a> {
    /// Construct options with the Zig defaults (`18x18`) for the given image.
    pub fn new(image: &'a str) -> Self {
        Self {
            title: "",
            image,
            is_template: false,
            width: 18,
            height: 18,
        }
    }
}

/// A connected display, deserialized from `getPrimaryDisplay` / `getAllDisplays`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Display {
    /// Platform display id.
    pub id: i64,
    /// Full display bounds.
    pub bounds: Rect,
    /// Usable work area (excludes menu bar / taskbar).
    #[serde(rename = "workArea")]
    pub work_area: Rect,
    /// Backing scale factor.
    #[serde(rename = "scaleFactor")]
    pub scale_factor: f64,
    /// Whether this is the primary display.
    #[serde(rename = "isPrimary")]
    pub is_primary: bool,
}

/// A 2D point, deserialized from `getCursorScreenPoint`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    /// X coordinate.
    pub x: f64,
    /// Y coordinate.
    pub y: f64,
}

/// Options for [`crate::Core::show_notification`].
#[derive(Debug, Clone)]
pub struct NotificationOptions<'a> {
    /// Notification title.
    pub title: &'a str,
    /// Body text.
    pub body: &'a str,
    /// Subtitle (macOS).
    pub subtitle: &'a str,
    /// Suppress sound.
    pub silent: bool,
}

impl<'a> NotificationOptions<'a> {
    /// Construct options with empty body/subtitle and sound enabled.
    pub fn new(title: &'a str) -> Self {
        Self {
            title,
            body: "",
            subtitle: "",
            silent: false,
        }
    }
}

/// A cookie. Serialized to JSON for `sessionSetCookie`; the same shape is
/// returned by `sessionGetCookies`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cookie {
    /// Cookie name.
    pub name: String,
    /// Cookie value.
    pub value: String,
    /// Domain scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    /// Path scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Secure flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secure: Option<bool>,
    /// HttpOnly flag.
    #[serde(default, rename = "httpOnly", skip_serializing_if = "Option::is_none")]
    pub http_only: Option<bool>,
    /// SameSite policy.
    #[serde(default, rename = "sameSite", skip_serializing_if = "Option::is_none")]
    pub same_site: Option<String>,
    /// Expiration as a Unix timestamp (seconds).
    #[serde(
        default,
        rename = "expirationDate",
        skip_serializing_if = "Option::is_none"
    )]
    pub expiration_date: Option<f64>,
}

/// Filter for [`crate::SessionPartition::get_cookies`]. Serialized to JSON.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CookieFilter {
    /// Match by URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Match by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Match by domain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    /// Match by path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Match by secure flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secure: Option<bool>,
    /// Match by session flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<bool>,
}

/// Storage categories for [`crate::SessionPartition::clear_storage_data`].
/// Serializes to its lowercase-ish tag matching the Zig `@tagName`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StorageType {
    /// HTTP cookies.
    #[serde(rename = "cookies")]
    Cookies,
    /// `localStorage`.
    #[serde(rename = "localStorage")]
    LocalStorage,
    /// `sessionStorage`.
    #[serde(rename = "sessionStorage")]
    SessionStorage,
    /// IndexedDB.
    #[serde(rename = "indexedDB")]
    IndexedDb,
    /// WebSQL.
    #[serde(rename = "webSQL")]
    WebSql,
    /// HTTP cache.
    #[serde(rename = "cache")]
    Cache,
    /// All storage.
    #[serde(rename = "all")]
    All,
}

impl StorageType {
    /// The JSON tag, matching the Zig enum field name.
    pub fn as_tag(self) -> &'static str {
        match self {
            Self::Cookies => "cookies",
            Self::LocalStorage => "localStorage",
            Self::SessionStorage => "sessionStorage",
            Self::IndexedDb => "indexedDB",
            Self::WebSql => "webSQL",
            Self::Cache => "cache",
            Self::All => "all",
        }
    }
}

/// Options for [`crate::Core::open_file_dialog`].
#[derive(Debug, Clone)]
pub struct OpenFileDialogOptions<'a> {
    /// Initial directory.
    pub starting_folder: &'a str,
    /// Allowed file types (comma-separated extensions or `*`).
    pub allowed_file_types: &'a str,
    /// Allow choosing files.
    pub can_choose_files: bool,
    /// Allow choosing directories.
    pub can_choose_directory: bool,
    /// Allow multi-selection.
    pub allows_multiple_selection: bool,
}

impl Default for OpenFileDialogOptions<'_> {
    fn default() -> Self {
        Self {
            starting_folder: "~/",
            allowed_file_types: "*",
            can_choose_files: true,
            can_choose_directory: true,
            allows_multiple_selection: true,
        }
    }
}

/// Options for [`crate::Core::show_message_box`].
#[derive(Debug, Clone)]
pub struct MessageBoxOptions<'a> {
    /// Box type (`"info"`, `"warning"`, `"error"`, `"question"`).
    pub box_type: &'a str,
    /// Dialog title.
    pub title: &'a str,
    /// Primary message.
    pub message: &'a str,
    /// Secondary detail text.
    pub detail: &'a str,
    /// Button labels (joined with `,` across the ABI).
    pub buttons: &'a [&'a str],
    /// Index of the default button.
    pub default_id: i32,
    /// Index of the cancel button (`-1` for none).
    pub cancel_id: i32,
}

impl Default for MessageBoxOptions<'_> {
    fn default() -> Self {
        Self {
            box_type: "info",
            title: "",
            message: "",
            detail: "",
            buttons: &["OK"],
            default_id: 0,
            cancel_id: -1,
        }
    }
}

/// An adapter/device pair returned from WGPU context creation.
#[derive(Debug, Clone, Copy)]
pub struct WgpuAdapterDevice {
    /// Opaque `WGPUAdapter` handle.
    pub adapter: *mut std::os::raw::c_void,
    /// Opaque `WGPUDevice` handle.
    pub device: *mut std::os::raw::c_void,
}
