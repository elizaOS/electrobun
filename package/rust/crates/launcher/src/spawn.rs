//! Spawns the child process and relays its exit status.
//!
//! Two strategies, matching the Zig launcher:
//!
//! * **Windows production** (GUI subsystem, not a dev build): launch via
//!   `CreateProcessW` with `CREATE_NO_WINDOW` so no console window appears,
//!   then wait and propagate the exit code.
//! * **Everything else** (all unix, plus Windows dev builds): a standard
//!   [`std::process::Command`] with inherited stdio. Windows dev builds first
//!   attach to the parent console so their output is visible.

use std::process::ExitCode;

use crate::LaunchPlan;

/// Launch the child per `plan` and translate its termination into our own exit
/// code. `is_dev_build` selects console vs. GUI behavior on Windows; it has no
/// effect on other platforms.
pub fn run(plan: &LaunchPlan, is_dev_build: bool) -> std::io::Result<ExitCode> {
    // Windows production builds get the no-console GUI launch path; every other
    // case (all unix, Windows dev) uses the inherited-stdio path.
    #[cfg(windows)]
    if !is_dev_build {
        return windows_gui::run(plan);
    }

    let _ = is_dev_build;
    inherited::run(plan)
}

/// Standard spawn with inherited stdio. Used for all unix builds and Windows
/// dev builds.
mod inherited {
    use std::process::{Command, ExitCode, Stdio};

    use crate::LaunchPlan;

    pub fn run(plan: &LaunchPlan) -> std::io::Result<ExitCode> {
        // Windows dev builds have no console of their own (the binary may be
        // built GUI-subsystem), so attach to the parent's so output is visible.
        #[cfg(windows)]
        super::windows_console::attach_parent();

        let (program, args) = plan.argv.split_first().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "empty argv for spawn")
        })?;

        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(&plan.cwd)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        for (key, value) in &plan.env {
            command.env(key, value);
        }

        let mut child = command.spawn()?;

        #[cfg(unix)]
        crate::signals::set_child_pid(child.id() as i32);

        eprintln!("Child process spawned with PID {}", child.id());

        let status = child.wait()?;
        Ok(exit_code_from_status(status))
    }

    /// Translate the child's [`ExitStatus`](std::process::ExitStatus) into the
    /// launcher's own exit code, mirroring the Zig launcher: a non-zero exit is
    /// propagated verbatim, and (on unix) death-by-signal becomes
    /// `128 + signal`.
    fn exit_code_from_status(status: std::process::ExitStatus) -> ExitCode {
        if let Some(code) = status.code() {
            // `code` is the platform exit code; the launcher only needs the low
            // 8 bits for its own process exit, matching `@intCast(code)`.
            return ExitCode::from(code as u8);
        }

        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            if let Some(sig) = status.signal() {
                // Expected during graceful shutdown — stay quiet for INT/TERM,
                // like the Zig launcher.
                if sig != libc::SIGINT && sig != libc::SIGTERM {
                    eprintln!("Child process terminated by signal: {sig}");
                }
                return ExitCode::from(128u8.wrapping_add(sig as u8));
            }
        }

        eprintln!("Child process terminated unexpectedly");
        ExitCode::FAILURE
    }
}

/// Windows console attachment for dev builds.
#[cfg(windows)]
mod windows_console {
    use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};

    /// Attach to the parent process's console so a GUI-subsystem dev build can
    /// still print to the terminal it was launched from. Best-effort: if there
    /// is no parent console the call simply fails and we carry on.
    pub fn attach_parent() {
        // SAFETY: `AttachConsole` is a parameterless win32 call with no
        // pointer arguments; passing the documented sentinel is always valid.
        let attached = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
        if attached != 0 {
            eprintln!("Attached to parent console");
        }
    }
}

/// Windows production launch path: `CreateProcessW` + `CREATE_NO_WINDOW`.
#[cfg(windows)]
mod windows_gui {
    use std::io;
    use std::process::ExitCode;

    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, GetExitCodeProcess, WaitForSingleObject, CREATE_NO_WINDOW, INFINITE,
        PROCESS_INFORMATION, STARTUPINFOW,
    };

    use crate::LaunchPlan;

    pub fn run(plan: &LaunchPlan) -> io::Result<ExitCode> {
        // Build the command line as a single quoted string, matching the Zig
        // launcher's `"{argv0}" "{argv1}"`. The bun-on-Windows plan always has
        // both elements; fall back gracefully if a future plan has only one.
        let cmd_line = match plan.argv.as_slice() {
            [program] => format!("\"{program}\""),
            [program, arg, ..] => format!("\"{program}\" \"{arg}\""),
            [] => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "empty argv for CreateProcessW",
                ))
            }
        };
        let mut cmd_line_w = to_wide_null(&cmd_line);
        let cwd_w = to_wide_null(&plan.cwd.to_string_lossy());

        let mut startup_info: STARTUPINFOW = unsafe { std::mem::zeroed() };
        startup_info.cb = std::mem::size_of::<STARTUPINFOW>() as u32;

        let mut process_info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };

        // SAFETY: `cmd_line_w` and `cwd_w` are valid, null-terminated UTF-16
        // buffers that outlive the call; `startup_info` is zero-initialized with
        // its `cb` set; the optional pointers are null. `lpCommandLine` may be
        // modified in place by the OS, hence the mutable buffer.
        let created = unsafe {
            CreateProcessW(
                std::ptr::null(),
                cmd_line_w.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0, // bInheritHandles = FALSE
                CREATE_NO_WINDOW,
                std::ptr::null(),
                cwd_w.as_ptr(),
                &startup_info,
                &mut process_info,
            )
        };

        if created == 0 {
            eprintln!("Failed to create process");
            return Err(io::Error::last_os_error());
        }

        eprintln!(
            "Child process spawned with PID {}",
            process_info.dwProcessId
        );

        // SAFETY: `hProcess` is a valid handle returned by `CreateProcessW`.
        unsafe { WaitForSingleObject(process_info.hProcess, INFINITE) };

        let mut exit_code: u32 = 0;
        // SAFETY: `hProcess` is valid and `exit_code` is a valid out pointer.
        unsafe { GetExitCodeProcess(process_info.hProcess, &mut exit_code) };

        // SAFETY: both handles came from `CreateProcessW` and are closed once.
        unsafe {
            CloseHandle(process_info.hProcess);
            CloseHandle(process_info.hThread);
        }

        eprintln!("Child process exited with code: {exit_code}");
        Ok(ExitCode::from(exit_code as u8))
    }

    /// Encode `s` as a null-terminated UTF-16 buffer for the wide win32 APIs.
    fn to_wide_null(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }
}
