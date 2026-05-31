//! Inbound webview → Bun message queue plus the self-pipe wakeup.
//!
//! Webview transport messages (decrypted in [`crate::transport`]) and any
//! other host-bound payloads are pushed onto a FIFO. A single byte is written
//! to the write end of an OS pipe to wake the Bun worker, which has wrapped the
//! read end in `createReadStream(fd)` (see `bun/proc/native.ts`). The worker
//! then drains via `popNextQueuedHostMessage`. The `signaled` flag coalesces
//! wakeups: exactly one pending byte until the queue is fully drained, matching
//! the Zig.
//!
//! On Windows the Zig returned `-1` (no pipe), so Bun falls back to 16ms
//! polling; we mirror that by never initializing the pipe on Windows.

use std::collections::VecDeque;
use std::ffi::CString;
use std::sync::Mutex;

use once_cell::sync::Lazy;

struct PendingHostMessage {
    webview_id: u32,
    message: CString,
}

struct WakeupState {
    initialized: bool,
    read_fd: i32,
    write_fd: i32,
    signaled: bool,
}

static QUEUE: Lazy<Mutex<VecDeque<PendingHostMessage>>> = Lazy::new(|| Mutex::new(VecDeque::new()));

static WAKEUP: Lazy<Mutex<WakeupState>> = Lazy::new(|| {
    Mutex::new(WakeupState {
        initialized: false,
        read_fd: -1,
        write_fd: -1,
        signaled: false,
    })
});

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poison| poison.into_inner())
}

#[cfg(unix)]
fn create_pipe() -> Option<(i32, i32)> {
    use std::os::fd::IntoRawFd;
    // nix returns owned fds; convert to raw ints and keep them open for the
    // process lifetime (the Zig never closed them).
    match nix::unistd::pipe() {
        Ok((read, write)) => Some((read.into_raw_fd(), write.into_raw_fd())),
        Err(_) => None,
    }
}

#[cfg(not(unix))]
fn create_pipe() -> Option<(i32, i32)> {
    // Windows: no self-pipe; Bun polls. Mirrors the Zig early return.
    None
}

/// Ensure the wakeup pipe exists. Returns false on Windows or pipe failure.
fn ensure_wakeup_initialized() -> bool {
    let mut state = lock(&WAKEUP);
    if state.initialized {
        return true;
    }
    let Some((read_fd, write_fd)) = create_pipe() else {
        return false;
    };
    state.read_fd = read_fd;
    state.write_fd = write_fd;
    state.initialized = true;
    state.signaled = false;
    true
}

/// Write the single wakeup byte unless one is already pending. Mirrors
/// `signalHostMessageWakeup`.
fn signal_wakeup() {
    if !ensure_wakeup_initialized() {
        return;
    }
    let mut state = lock(&WAKEUP);
    if state.signaled {
        return;
    }
    let byte: [u8; 1] = [1];
    #[cfg(unix)]
    {
        // SAFETY: write_fd is the valid write end of our pipe.
        let ret = unsafe { libc::write(state.write_fd, byte.as_ptr() as *const _, 1) };
        if ret < 0 {
            return;
        }
    }
    #[cfg(not(unix))]
    {
        // Windows has no self-pipe; Bun polls the queue. Marking signaled keeps
        // the coalescing invariant consistent (it is simply never observed).
        let _ = byte;
    }
    state.signaled = true;
}

/// Enqueue a host-bound message and signal the worker. Mirrors
/// `enqueuePendingHostMessage`. A message with an interior NUL is dropped
/// (cannot occur for JSON), matching the Zig's silent `dupeZ` failure path.
pub fn enqueue(webview_id: u32, message: &str) {
    let Ok(owned) = CString::new(message) else {
        return;
    };
    {
        let mut queue = lock(&QUEUE);
        queue.push_back(PendingHostMessage {
            webview_id,
            message: owned,
        });
    }
    signal_wakeup();
}

/// Pop the next queued message, transferring ownership of the string to the
/// caller (Bun frees it via `freeCoreString`). Writes the originating webview
/// id into `out_webview_id`. Returns null when the queue is empty, clearing the
/// wakeup flag on the final drain. Mirrors `popNextQueuedHostMessage`.
///
/// # Safety
/// `out_webview_id` must be a valid, writable `*mut u32`.
pub unsafe fn pop_next(out_webview_id: *mut u32) -> *mut std::ffi::c_char {
    let mut queue = lock(&QUEUE);
    let Some(entry) = queue.pop_front() else {
        return std::ptr::null_mut();
    };
    if queue.is_empty() {
        let mut state = lock(&WAKEUP);
        state.signaled = false;
    }
    if !out_webview_id.is_null() {
        // SAFETY: caller guarantees the pointer is writable.
        unsafe {
            *out_webview_id = entry.webview_id;
        }
    }
    entry.message.into_raw()
}

/// Read fd for the wakeup pipe, or `-1` if unavailable (Windows / failure).
/// Backs `getHostMessageWakeupReadFD`.
pub fn wakeup_read_fd() -> i32 {
    if !ensure_wakeup_initialized() {
        return -1;
    }
    lock(&WAKEUP).read_fd
}
