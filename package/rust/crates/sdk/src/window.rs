//! Window registry and window handle.
//!
//! Ports the Zig `WindowRegistry` / `BrowserWindowRef`. The registry tracks the
//! ids it created so [`BrowserWindowRef::close`] can forget them and
//! [`WindowRegistry::get_by_id`] can refuse unknown ids. The Zig original
//! mutates an `AutoHashMap` through a shared `*Core`; we use an interior
//! [`RefCell`] over the id set so handles can mutate it while borrowing the
//! registry immutably.

use std::cell::RefCell;
use std::collections::HashSet;

use crate::core::Core;
use crate::error::Result;
use crate::types::{Rect, WindowOptions};

/// Tracks windows created through it. Borrows a [`Core`] for its lifetime.
pub struct WindowRegistry<'core> {
    core: &'core Core,
    ids: RefCell<HashSet<u32>>,
}

impl<'core> WindowRegistry<'core> {
    /// Create an empty registry over `core`.
    pub fn new(core: &'core Core) -> WindowRegistry<'core> {
        WindowRegistry {
            core,
            ids: RefCell::new(HashSet::new()),
        }
    }

    /// The [`Core`] this registry wraps.
    pub fn core(&self) -> &'core Core {
        self.core
    }

    /// Create a window and track its id.
    pub fn create_browser_window(
        &self,
        options: &WindowOptions<'_>,
    ) -> Result<BrowserWindowRef<'_, 'core>> {
        let id = self.core.create_window(options)?;
        self.ids.borrow_mut().insert(id);
        Ok(BrowserWindowRef { registry: self, id })
    }

    /// Return a handle for a tracked id, or `None` if it is not tracked.
    pub fn get_by_id(&self, id: u32) -> Option<BrowserWindowRef<'_, 'core>> {
        if self.ids.borrow().contains(&id) {
            Some(BrowserWindowRef { registry: self, id })
        } else {
            None
        }
    }
}

/// A handle to a tracked window. Mirrors the Zig `BrowserWindowRef`.
#[derive(Clone, Copy)]
pub struct BrowserWindowRef<'reg, 'core> {
    registry: &'reg WindowRegistry<'core>,
    id: u32,
}

impl BrowserWindowRef<'_, '_> {
    /// The window's id.
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Close the window and forget it from the registry.
    pub fn close(&self) -> Result<()> {
        self.registry.core.close_window(self.id)?;
        self.registry.ids.borrow_mut().remove(&self.id);
        Ok(())
    }

    /// Read the window's frame.
    pub fn get_frame(&self) -> Result<Rect> {
        self.registry.core.get_window_frame(self.id)
    }

    /// Set the traffic-light button position (macOS).
    pub fn set_window_button_position(&self, x: f64, y: f64) -> Result<()> {
        self.registry.core.set_window_button_position(self.id, x, y)
    }
}
