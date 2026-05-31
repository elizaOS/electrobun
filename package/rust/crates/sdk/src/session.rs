//! Session / cookie partition helpers.
//!
//! Ports the Zig `SessionPartition` / `Session`. These borrow a [`Core`] and a
//! partition name and JSON-encode their typed arguments (filters, cookies,
//! storage-type lists) before forwarding to the corresponding `Core` method.

use crate::core::Core;
use crate::error::{ElectrobunError, Result};
use crate::types::{Cookie, CookieFilter, StorageType};

/// The default persistent partition used by [`Session::default_session`].
pub const DEFAULT_PARTITION: &str = "persist:default";

/// A cookie/storage session scoped to a single partition.
#[derive(Clone, Copy)]
pub struct SessionPartition<'core, 'name> {
    core: &'core Core,
    partition: &'name str,
}

impl<'core, 'name> SessionPartition<'core, 'name> {
    /// Read cookies matching `filter` (or all cookies when `filter` is `None`).
    pub fn get_cookies(&self, filter: Option<&CookieFilter>) -> Result<Vec<Cookie>> {
        let default_filter;
        let filter = match filter {
            Some(filter) => filter,
            None => {
                default_filter = CookieFilter::default();
                &default_filter
            }
        };
        let filter_json = serde_json::to_string(filter).map_err(json_err("CookieFilter"))?;
        self.core.session_get_cookies(self.partition, &filter_json)
    }

    /// Set a cookie on this partition. Returns success.
    pub fn set_cookie(&self, cookie: &Cookie) -> Result<bool> {
        let cookie_json = serde_json::to_string(cookie).map_err(json_err("Cookie"))?;
        self.core.session_set_cookie(self.partition, &cookie_json)
    }

    /// Remove a cookie by URL + name. Returns success.
    pub fn remove_cookie(&self, url: &str, name: &str) -> Result<bool> {
        self.core.session_remove_cookie(self.partition, url, name)
    }

    /// Clear all cookies on this partition.
    pub fn clear_cookies(&self) -> Result<()> {
        self.core.session_clear_cookies(self.partition)
    }

    /// Clear storage data for the given types. An empty slice clears `["all"]`,
    /// matching the Zig behaviour.
    pub fn clear_storage_data(&self, storage_types: &[StorageType]) -> Result<()> {
        if storage_types.is_empty() {
            return self
                .core
                .session_clear_storage_data(self.partition, "[\"all\"]");
        }
        let names: Vec<&str> = storage_types.iter().map(|kind| kind.as_tag()).collect();
        let storage_types_json =
            serde_json::to_string(&names).map_err(json_err("StorageType[]"))?;
        self.core
            .session_clear_storage_data(self.partition, &storage_types_json)
    }
}

/// Factory for [`SessionPartition`] values. Mirrors the Zig `Session` namespace.
pub struct Session;

impl Session {
    /// Wrap a specific partition.
    pub fn from_partition<'core, 'name>(
        core: &'core Core,
        partition: &'name str,
    ) -> SessionPartition<'core, 'name> {
        SessionPartition { core, partition }
    }

    /// Wrap the default persistent partition (`persist:default`).
    pub fn default_session(core: &Core) -> SessionPartition<'_, 'static> {
        SessionPartition {
            core,
            partition: DEFAULT_PARTITION,
        }
    }
}

/// Build a closure mapping a serde serialization error to [`ElectrobunError`].
fn json_err(context: &'static str) -> impl Fn(serde_json::Error) -> ElectrobunError {
    move |source| ElectrobunError::Json {
        context: context.to_string(),
        source,
    }
}
