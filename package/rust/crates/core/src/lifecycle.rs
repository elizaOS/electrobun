//! App-lifecycle state: the managed quit-requested handler and the
//! `exit_on_last_window_closed` flag.
//!
//! Mirrors the Zig globals `managed_quit_requested_handler` and
//! `exit_on_last_window_closed`. The handler is the JS `JSCallback` Bun set via
//! `setQuitRequestedHandler`; the window-close trampoline fires it when the
//! last window closes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use once_cell::sync::Lazy;

use crate::ffi_types::QuitRequestedHandler;

static EXIT_ON_LAST_WINDOW_CLOSED: AtomicBool = AtomicBool::new(true);

/// Newtype so the fn-pointer can live behind a `Mutex` in a `Sync` static.
struct QuitHandlerSlot(Option<QuitRequestedHandler>);

// SAFETY: a fn-pointer is a plain code address, safe to share across threads.
unsafe impl Send for QuitHandlerSlot {}

static QUIT_HANDLER: Lazy<Mutex<QuitHandlerSlot>> = Lazy::new(|| Mutex::new(QuitHandlerSlot(None)));

pub fn set_exit_on_last_window_closed(enabled: bool) {
    EXIT_ON_LAST_WINDOW_CLOSED.store(enabled, Ordering::SeqCst);
}

pub fn exit_on_last_window_closed() -> bool {
    EXIT_ON_LAST_WINDOW_CLOSED.load(Ordering::SeqCst)
}

pub fn set_managed_quit_requested_handler(handler: Option<QuitRequestedHandler>) {
    let mut slot = QUIT_HANDLER
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    slot.0 = handler;
}

/// Invoke the managed quit handler if one is set (mirrors the Zig
/// `managed_quit_requested_handler.?()` call sites).
pub fn invoke_managed_quit_requested() {
    let handler = {
        let slot = QUIT_HANDLER
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        slot.0
    };
    if let Some(handler) = handler {
        // SAFETY: JSCallback fn-pointer registered by Bun and kept alive.
        unsafe { handler() };
    }
}
