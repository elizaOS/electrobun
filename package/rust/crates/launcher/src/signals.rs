//! Unix signal coordination for graceful shutdown.
//!
//! The launcher and its child share a process group, so a terminal Ctrl+C
//! (SIGINT) is delivered to the child too — the child runs its own graceful
//! quit. Our handler's job is purely supervisory:
//!
//! * First SIGINT  → arm a 10s safety timeout (`alarm(10)`) in case the child
//!   hangs while shutting down. We do not kill anything yet.
//! * Second SIGINT → cancel the timeout and force-kill the whole process group
//!   (`kill(0, SIGKILL)`).
//! * SIGALRM       → the safety timeout fired; force-kill the process group.
//! * SIGTERM/SIGHUP → forward to the child pid.
//!
//! Handlers run in an async-signal context, so they touch only atomics and
//! call only async-signal-safe libc functions (`alarm`, `kill`). This mirrors
//! `signalHandler` / `alarmHandler` in `launcher/main.zig`.

use std::sync::atomic::{AtomicI32, Ordering};

use nix::sys::signal::{self, SaFlags, SigAction, SigHandler, SigSet, Signal};
use nix::unistd::Pid;

/// Process-group target for `kill`: pid `0` means "every process in the
/// caller's process group".
const PROCESS_GROUP: Pid = Pid::from_raw(0);

/// PID of the spawned child, published by [`set_child_pid`] after spawn.
/// `0` means "not spawned yet"; signals that arrive before then are not
/// forwarded (forwarding to pid 0 would target the whole group, which the Zig
/// launcher avoids by only spawning after handler install but before the child
/// id is meaningful — guarding on non-zero is the faithful, safe equivalent).
static CHILD_PID: AtomicI32 = AtomicI32::new(0);

/// Number of SIGINTs seen, distinguishing the first (arm timeout) from the
/// second (force kill).
static SIGINT_COUNT: AtomicI32 = AtomicI32::new(0);

/// Record the spawned child's PID so SIGTERM / SIGHUP can be forwarded to it.
pub fn set_child_pid(pid: i32) {
    CHILD_PID.store(pid, Ordering::SeqCst);
}

/// Install handlers for SIGINT, SIGTERM, SIGHUP, and SIGALRM.
///
/// On the (vanishingly unlikely) failure to install a handler we leave the
/// default disposition in place for that signal rather than aborting startup —
/// the launcher is still able to supervise the child.
pub fn install() {
    install_handler(Signal::SIGINT, SigHandler::Handler(handle_signal));
    install_handler(Signal::SIGTERM, SigHandler::Handler(handle_signal));
    install_handler(Signal::SIGHUP, SigHandler::Handler(handle_signal));
    install_handler(Signal::SIGALRM, SigHandler::Handler(handle_alarm));
}

fn install_handler(signal: Signal, handler: SigHandler) {
    let action = SigAction::new(handler, SaFlags::empty(), SigSet::empty());
    // SAFETY: `handle_signal` / `handle_alarm` are async-signal-safe (they only
    // touch atomics and call `alarm`/`kill`), which is the contract for a
    // signal handler installed via `sigaction`.
    let _ = unsafe { signal::sigaction(signal, &action) };
}

/// Force-kill the entire process group. `kill(0, ...)` targets every process in
/// the caller's process group, which tears down the child (and any descendants)
/// even if it ignored softer signals.
fn kill_process_group() {
    let _ = signal::kill(PROCESS_GROUP, Signal::SIGKILL);
}

extern "C" fn handle_alarm(_sig: libc::c_int) {
    // Safety timeout fired: the app hung during shutdown.
    kill_process_group();
}

extern "C" fn handle_signal(sig: libc::c_int) {
    if sig == libc::SIGINT {
        // First Ctrl+C arms the safety timeout; second one force-kills.
        let prior = SIGINT_COUNT.fetch_add(1, Ordering::SeqCst);
        if prior == 0 {
            // SAFETY: `alarm` is async-signal-safe.
            unsafe { libc::alarm(10) };
        } else {
            // SAFETY: `alarm` is async-signal-safe.
            unsafe { libc::alarm(0) };
            kill_process_group();
        }
        return;
    }

    // SIGTERM / SIGHUP: forward the exact signal to the child if we have one.
    // `libc::kill` takes the raw signal number directly (matching the Zig
    // `c.kill(child_pid, sig)`), avoiding any lossy enum round-trip.
    let child_pid = CHILD_PID.load(Ordering::SeqCst);
    if child_pid > 0 {
        // SAFETY: `kill` is async-signal-safe; both arguments are plain ints.
        unsafe { libc::kill(child_pid, sig) };
    }
}
