//! Safe, typed Rust SDK for native-mainProcess Electrobun apps.
//!
//! This crate is a *consumer* of `libElectrobunCore`: at runtime it `dlopen`s
//! the bundled core library (and, for WebGPU, `libwebgpu_dawn`) via
//! [`libloading`], resolves the C-ABI symbol table, and exposes ergonomic
//! methods over it. It declares no `extern`/`export` symbols of its own.
//!
//! It is a 1:1 port of `package/src/zig-sdk/electrobun.zig`; every public
//! method maps to a Zig SDK function and every C-ABI signature is verified
//! against the matching `export fn` in `package/src/core/main.zig`.
//!
//! # Entry points
//! - [`Core::load`] opens core and resolves all symbols.
//! - [`WindowRegistry`] / [`BrowserWindowRef`] manage windows.
//! - [`Session`] / [`SessionPartition`] manage cookies and storage.
//! - [`WgpuNative`] / [`WgpuContext`] drive WebGPU.
//! - [`Paths`] / [`resolve_bundle_paths`] / [`resolve_app_info_from_bundle`]
//!   handle bundle layout and OS/app-scoped directories.
//!
//! # Callbacks
//! Native callbacks must be `'static` `extern "C" fn` pointers (never
//! closures), since core stores some of them for the lifetime of the process.
//! See [`handlers`] for the type aliases and the no-op defaults.

#![warn(missing_docs)]

mod core;
mod error;
pub mod handlers;
mod paths;
mod session;
mod symbols;
mod types;
mod wgpu;
mod window;

pub use crate::core::Core;
pub use crate::error::{ElectrobunError, Result};
pub use crate::handlers::{
    allow_all_navigation, noop_webview_event, noop_webview_post_message, AppReopenHandler,
    DecideNavigationHandler, GlobalShortcutHandler, QuitRequestedHandler, StatusItemHandler,
    URLOpenHandler, WebviewEventHandler, WebviewPostMessageHandler, WindowBlurHandler,
    WindowCloseHandler, WindowFocusHandler, WindowKeyHandler, WindowMoveHandler,
    WindowResizeHandler,
};
pub use crate::paths::{
    quit, resolve_app_info_from_bundle, resolve_bundle_paths, BundlePaths, Paths,
};
pub use crate::session::{Session, SessionPartition, DEFAULT_PARTITION};
pub use crate::types::{
    AppInfo, Cookie, CookieFilter, Display, MessageBoxOptions, NotificationOptions,
    OpenFileDialogOptions, OwnedAppInfo, Point, Rect, Renderer, StorageType, TrafficLightOffset,
    TrayOptions, WGPUViewOptions, WebviewCallbacks, WebviewOptions, WgpuAdapterDevice,
    WindowCallbacks, WindowOptions, WindowStyle,
};
pub use crate::wgpu::{WgpuContext, WgpuNative};
pub use crate::window::{BrowserWindowRef, WindowRegistry};
