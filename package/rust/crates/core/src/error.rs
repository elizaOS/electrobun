//! Error channel + the `freeCoreString` allocator contract.
//!
//! The Zig kept a single process-global `last_error`. Bun always reads
//! `electrobun_core_last_error()` immediately after a failing call **on the
//! same thread** (see `getCoreLastError()` in `bun/proc/native.ts`). Core is
//! dlopened on two threads (main + worker) which each call distinct export
//! groups, so a thread-local error slot is both faithful to that usage and
//! free of cross-thread pointer aliasing. The returned pointer stays valid
//! until the next call that mutates the slot on that same thread.
//!
//! ## `freeCoreString` ownership contract
//!
//! Every string this library returns to Bun that Bun frees via
//! `freeCoreString` is produced with [`into_owned_c_string`], which leaks a
//! `CString` via `CString::into_raw`. `freeCoreString` reclaims it with the
//! matching `CString::from_raw`. The two MUST stay paired so the global
//! allocator frees what it allocated. Static string literals (e.g. the empty
//! string returned for "no error", or `EMPTY_RECT_JSON`) are never handed to
//! `freeCoreString`, matching the Zig (which returned non-owned `[*:0]const u8`
//! for those and only freed heap strings).

use std::cell::RefCell;
use std::ffi::{c_char, CString};

thread_local! {
    /// Owns the current thread's last-error string so the pointer returned by
    /// [`last_error_ptr`] remains valid until the slot is next written.
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

/// Empty C string returned when there is no pending error (matches the Zig
/// `return "";`). Pointer is into static storage and must not be freed.
const EMPTY: &[u8] = b"\0";

/// Clear the calling thread's pending error (mirrors `clearLastError`).
pub fn clear_last_error() {
    LAST_ERROR.with(|slot| {
        *slot.borrow_mut() = None;
    });
}

/// Record an error message for the calling thread (mirrors `setLastError`).
/// A message containing an interior NUL is truncated at the NUL, which cannot
/// happen for any of our `format!`-built messages.
pub fn set_last_error(message: impl Into<Vec<u8>>) {
    let owned = CString::new(message).unwrap_or_else(|err| {
        let valid_up_to = err.nul_position();
        let mut bytes = err.into_vec();
        bytes.truncate(valid_up_to);
        // SAFETY: bytes was truncated at the first interior NUL, so it now
        // contains no NUL bytes and `from_vec_unchecked` is sound.
        unsafe { CString::from_vec_unchecked(bytes) }
    });
    LAST_ERROR.with(|slot| {
        *slot.borrow_mut() = Some(owned);
    });
}

/// Pointer to the calling thread's last-error message, or the static empty
/// string. Backs the `electrobun_core_last_error` export.
pub fn last_error_ptr() -> *const c_char {
    LAST_ERROR.with(|slot| match slot.borrow().as_ref() {
        Some(message) => message.as_ptr(),
        None => EMPTY.as_ptr() as *const c_char,
    })
}

/// Reclaim a string previously produced by `CString::into_raw` (the host-queue
/// messages handed to Bun). Safe to call with null (no-op), matching
/// `freeCoreString(null)`.
///
/// # Safety
/// `value` must be either null or a pointer returned by `CString::into_raw`
/// (from [`crate::host_queue`]) that has not already been freed.
pub unsafe fn free_owned_c_string(value: *mut c_char) {
    if value.is_null() {
        return;
    }
    // SAFETY: contract delegated to the caller — the pointer originates from
    // CString::into_raw and is freed exactly once.
    drop(unsafe { CString::from_raw(value) });
}
