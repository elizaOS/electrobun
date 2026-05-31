//! Loader + symbol resolver for `libNativeWrapper.{dylib,so,dll}`.
//!
//! Mirrors the Zig `ensureNativeWrapperLoaded` / `lookupNativeSymbol`:
//! resolves the wrapper next to the running executable, eagerly binds the two
//! always-present entrypoints (`startEventLoop`, `forceExit`), and resolves
//! every other native symbol lazily by name. A missing symbol records a
//! last-error and yields `None`, exactly like the Zig.
//!
//! The loaded `Library` is stored in a process-global `OnceCell` and lives for
//! the remainder of the process, so resolved symbol addresses stay valid; we
//! return them as bare `*const c_void` that callers transmute to the concrete
//! `extern "C"` fn-pointer type for the call site.

use std::ffi::{c_void, CString};
use std::sync::Mutex;

use libloading::Library;
use once_cell::sync::OnceCell;

use crate::error::set_last_error;
use crate::ffi_types::{ForceExitFn, StartEventLoopFn};

struct NativeWrapper {
    library: Library,
    start_event_loop: StartEventLoopFn,
    force_exit: ForceExitFn,
}

// SAFETY: `Library` is a handle to a dlopen'd module and the bound fn-pointers
// are plain code addresses; both are safe to share across threads. Core is
// loaded on the main + worker threads and resolves symbols from either.
unsafe impl Send for NativeWrapper {}
unsafe impl Sync for NativeWrapper {}

/// Holds the wrapper once it loads successfully. A *successful* load is
/// terminal, so a `OnceCell` is the right shape; *failed* loads do not poison
/// it, allowing later calls to retry — matching the Zig, which keeps
/// `native_wrapper_loaded = false` after a failure.
static NATIVE_WRAPPER: OnceCell<NativeWrapper> = OnceCell::new();
/// Serializes load attempts so concurrent callers don't race the dlopen.
static LOAD_LOCK: Mutex<()> = Mutex::new(());

fn native_wrapper_file_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "libNativeWrapper.dll"
    } else if cfg!(target_os = "macos") {
        "libNativeWrapper.dylib"
    } else {
        "libNativeWrapper.so"
    }
}

fn resolve_native_wrapper_path() -> Result<std::path::PathBuf, String> {
    let exe = std::env::current_exe()
        .map_err(|err| format!("Failed to resolve native wrapper path: {err}"))?;
    let dir = exe
        .parent()
        .ok_or_else(|| "Failed to resolve native wrapper path: InvalidExePath".to_string())?;
    Ok(dir.join(native_wrapper_file_name()))
}

/// Resolve a symbol from an already-bound `Library` as a raw code address.
fn resolve_symbol(library: &Library, name: &str) -> Option<*const c_void> {
    let mut symbol_name = name.as_bytes().to_vec();
    symbol_name.push(0);
    // SAFETY: we only read the symbol's address; we never call it here. The
    // address is valid for the lifetime of the (process-global) library.
    unsafe {
        library
            .get::<*const c_void>(&symbol_name)
            .ok()
            .map(|sym| *sym)
    }
}

fn load_native_wrapper() -> Option<NativeWrapper> {
    let path = match resolve_native_wrapper_path() {
        Ok(path) => path,
        Err(message) => {
            set_last_error(message);
            return None;
        }
    };

    // SAFETY: loading a shared library runs its initializers; the path is the
    // bundled wrapper shipped beside the executable.
    let library = match unsafe { Library::new(&path) } {
        Ok(library) => library,
        Err(err) => {
            set_last_error(format!(
                "Failed to open native wrapper at {}: {err}",
                path.display()
            ));
            return None;
        }
    };

    let start_event_loop = match resolve_symbol(&library, "startEventLoop") {
        Some(addr) => unsafe { std::mem::transmute::<*const c_void, StartEventLoopFn>(addr) },
        None => {
            set_last_error("Native wrapper is missing startEventLoop");
            return None;
        }
    };

    let force_exit = match resolve_symbol(&library, "forceExit") {
        Some(addr) => unsafe { std::mem::transmute::<*const c_void, ForceExitFn>(addr) },
        None => {
            set_last_error("Native wrapper is missing forceExit");
            return None;
        }
    };

    Some(NativeWrapper {
        library,
        start_event_loop,
        force_exit,
    })
}

/// Ensure the native wrapper is loaded, retrying on prior failure. Returns a
/// reference to the loaded wrapper or `None` (with last-error set).
fn ensure_loaded() -> Option<&'static NativeWrapper> {
    if let Some(wrapper) = NATIVE_WRAPPER.get() {
        return Some(wrapper);
    }

    let _guard = LOAD_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());

    // Re-check: another thread may have loaded it while we waited.
    if let Some(wrapper) = NATIVE_WRAPPER.get() {
        return Some(wrapper);
    }

    // On failure, leave the cell empty so a later call retries the dlopen.
    let loaded = load_native_wrapper()?;
    let _ = NATIVE_WRAPPER.set(loaded);
    NATIVE_WRAPPER.get()
}

/// Resolve a native-wrapper symbol by name. On a missing symbol, records the
/// same last-error message the Zig used and returns `None`.
///
/// The returned pointer is a raw code address; the caller transmutes it to the
/// concrete `extern "C"` fn-pointer type and invokes it.
pub fn lookup_native_symbol(name: &str) -> Option<*const c_void> {
    let wrapper = ensure_loaded()?;
    match resolve_symbol(&wrapper.library, name) {
        Some(addr) => Some(addr),
        None => {
            set_last_error(format!("Native wrapper is missing {name}"));
            None
        }
    }
}

/// Invoke the eagerly-bound `startEventLoop`. Returns `false` (with last-error
/// set) if the wrapper could not be loaded.
pub fn run_start_event_loop(identifier: &CString, name: &CString, channel: &CString) -> bool {
    let Some(wrapper) = ensure_loaded() else {
        return false;
    };
    // SAFETY: fn-pointer was bound from the loaded wrapper; the CStrings stay
    // alive for the duration of the (blocking) call.
    unsafe {
        (wrapper.start_event_loop)(identifier.as_ptr(), name.as_ptr(), channel.as_ptr());
    }
    true
}

/// Invoke the eagerly-bound `forceExit`. No-op (returns `false`) if the wrapper
/// is unavailable.
pub fn run_force_exit(code: std::os::raw::c_int) -> bool {
    let Some(wrapper) = ensure_loaded() else {
        return false;
    };
    // SAFETY: fn-pointer bound from the loaded wrapper.
    unsafe {
        (wrapper.force_exit)(code);
    }
    true
}
