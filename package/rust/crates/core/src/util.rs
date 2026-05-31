//! Small FFI helpers: panic-guarding, C-string conversion, and JS literal
//! encoding.

use std::ffi::CStr;
use std::os::raw::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};

/// Run `body` inside `catch_unwind`, returning `fallback` if it panics. Every
/// `#[no_mangle] extern "C"` export and every callback trampoline wraps its
/// body in this so a Rust panic never unwinds across the C ABI into Bun (which
/// would be undefined behavior).
pub fn guard<R>(fallback: R, body: impl FnOnce() -> R) -> R {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(value) => value,
        Err(_) => fallback,
    }
}

/// Borrow a `*const c_char` as `&str`, or `None` if null / not valid UTF-8.
/// The Zig used `std.mem.span` (raw bytes); we additionally require UTF-8,
/// which holds for every JSON / identifier string Bun passes.
///
/// # Safety
/// The lifetime of the returned `&str` is tied to the pointer; callers must not
/// retain it beyond the FFI call. (Marked safe-to-call but reads through the
/// pointer; only invoked with pointers Bun guarantees valid + NUL-terminated.)
pub fn cstr_to_str<'a>(ptr: *const c_char) -> Option<&'a str> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: Bun passes NUL-terminated C strings that outlive the call.
    let cstr = unsafe { CStr::from_ptr(ptr) };
    cstr.to_str().ok()
}

/// Borrow a `*const c_char` as bytes (NUL-terminated), or `None` if null.
///
/// # Safety
/// Same contract as [`cstr_to_str`].
pub fn cstr_to_bytes<'a>(ptr: *const c_char) -> Option<&'a [u8]> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: Bun passes NUL-terminated C strings that outlive the call.
    let cstr = unsafe { CStr::from_ptr(ptr) };
    Some(cstr.to_bytes())
}

/// Encode a string as a JSON string literal (double-quoted, escaped), matching
/// the Zig `std.json.stringifyAlloc(value, .{})` used for `quoteJavascriptString`
/// and request-id / payload encoding.
pub fn json_quote(value: &str) -> String {
    serde_json::Value::String(value.to_string()).to_string()
}
