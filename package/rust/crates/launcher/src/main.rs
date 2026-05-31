//! Electrobun launcher.
//!
//! Thin supervisor that lives in the app bundle's executable directory. It
//! resolves the configured main process (the `bun` runtime or a native `zig`
//! binary), spawns it with the correct working directory and environment, and
//! relays the child's lifetime back to the OS:
//!
//! * On unix it installs signal handlers so the first Ctrl+C arms a 10s safety
//!   timeout (the child runs its own graceful-quit sequence because it shares
//!   our process group), a second Ctrl+C force-kills the whole group, and
//!   SIGTERM/SIGHUP are forwarded to the child.
//! * On production Windows it uses `CreateProcessW` with `CREATE_NO_WINDOW`
//!   (GUI subsystem, no console flash); dev Windows builds attach to the parent
//!   console and spawn with inherited stdio like every other platform.
//!
//! Port of `package/src/launcher/main.zig`.

// Production Windows builds use the GUI subsystem so launching the app does not
// flash a console window. Dev builds keep the default console subsystem so the
// CLI stays interactive (mirrors `launcher/build.zig`: `exe.subsystem =
// .Windows` only when not Debug). `debug_assertions` is the cargo analogue of
// Zig's `optimize == .Debug`.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

mod config;
#[cfg(unix)]
mod signals;
mod spawn;

/// Which runtime the launcher hands control to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MainProcess {
    /// The bundled `bun` runtime executing `Resources/main.js`.
    Bun,
    /// A native binary (`main`) built directly into the bundle. Covers the
    /// `"zig"` / `"native"` value of `build.json`'s `mainProcess`.
    Native,
}

/// Everything the spawn paths need to launch the child, computed once up front.
pub struct LaunchPlan {
    /// argv passed to the child. `argv[0]` is the program, the rest are args.
    pub argv: Vec<String>,
    /// Working directory for the child (always the executable's directory).
    pub cwd: PathBuf,
    /// Environment overrides layered on top of the inherited environment.
    /// macOS inherits unchanged, so this is empty there.
    pub env: Vec<(String, String)>,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("Launcher error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> std::io::Result<ExitCode> {
    let exe_dir = current_exe_dir()?;

    eprintln!("Launcher starting on {}...", std::env::consts::OS);
    eprintln!("Current directory: {}", exe_dir.display());

    // Install signal handlers before spawning so an early Ctrl+C is coordinated
    // rather than killing us outright. No-op on Windows.
    #[cfg(unix)]
    signals::install();

    let main_process = config::detect_main_process(&exe_dir);
    let plan = build_plan(main_process, &exe_dir);

    let arg0 = plan.argv.first().map(String::as_str).unwrap_or("");
    let arg1 = plan.argv.get(1).map(String::as_str).unwrap_or("");
    eprintln!("Spawning: {arg0} {arg1}");

    // Console mode is forced via env, otherwise inferred from version.json.
    let force_console = std::env::var("ELECTROBUN_CONSOLE").as_deref() == Ok("1");
    let is_dev_build = force_console || config::is_dev_build(&exe_dir);
    if force_console {
        eprintln!("Console mode forced via ELECTROBUN_CONSOLE=1");
    } else if is_dev_build {
        eprintln!("Dev build detected - console output enabled");
    }

    spawn::run(&plan, is_dev_build)
}

/// Resolve the directory containing the running executable.
fn current_exe_dir() -> std::io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    exe.parent().map(Path::to_path_buf).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "executable has no parent directory",
        )
    })
}

/// Build the argv / cwd / env plan for the resolved main process.
///
/// Path shapes match the Zig launcher exactly:
/// * bun + macOS: `["./bun", "../Resources/main.js"]` (bun found via cwd)
/// * bun + linux/windows: `[<exe_dir>/bun[.exe], "../Resources/main.js"]`
/// * native: `[<exe_dir>/main[.exe]]`
fn build_plan(main_process: MainProcess, exe_dir: &Path) -> LaunchPlan {
    let argv = match main_process {
        MainProcess::Bun => {
            let main_js = join_resources(exe_dir, "main.js");
            if cfg!(target_os = "macos") {
                vec!["./bun".to_string(), main_js]
            } else {
                let bun_name = if cfg!(windows) { "bun.exe" } else { "bun" };
                vec![path_str(&exe_dir.join(bun_name)), main_js]
            }
        }
        MainProcess::Native => {
            let main_name = if cfg!(windows) { "main.exe" } else { "main" };
            vec![path_str(&exe_dir.join(main_name))]
        }
    };

    LaunchPlan {
        argv,
        cwd: exe_dir.to_path_buf(),
        env: build_env(exe_dir),
    }
}

/// Compute environment overrides for the child process.
///
/// * Linux: prepend `exe_dir` to `LD_LIBRARY_PATH`; set `LD_PRELOAD` to any of
///   `./libcef.so` / `./libvk_swiftshader.so` that exist; set `ICU_DATA`.
/// * Windows: set `ICU_DATA`.
/// * macOS: inherit unchanged (uses system ICU) — no overrides.
#[cfg(target_os = "linux")]
fn build_env(exe_dir: &Path) -> Vec<(String, String)> {
    let exe_dir_str = path_str(exe_dir);
    let mut env = Vec::new();

    let ld_library_path = match std::env::var("LD_LIBRARY_PATH") {
        Ok(existing) if !existing.is_empty() => format!("{exe_dir_str}:{existing}"),
        _ => exe_dir_str.clone(),
    };
    env.push(("LD_LIBRARY_PATH".to_string(), ld_library_path));

    let mut preload_libs = Vec::new();
    if exe_dir.join("libcef.so").exists() {
        preload_libs.push("./libcef.so");
    }
    if exe_dir.join("libvk_swiftshader.so").exists() {
        preload_libs.push("./libvk_swiftshader.so");
    }
    if !preload_libs.is_empty() {
        let ld_preload = preload_libs.join(":");
        eprintln!("Setting LD_PRELOAD: {ld_preload}");
        env.push(("LD_PRELOAD".to_string(), ld_preload));
    }

    env.push(("ICU_DATA".to_string(), exe_dir_str));
    env
}

#[cfg(target_os = "windows")]
fn build_env(exe_dir: &Path) -> Vec<(String, String)> {
    vec![("ICU_DATA".to_string(), path_str(exe_dir))]
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn build_env(_exe_dir: &Path) -> Vec<(String, String)> {
    Vec::new()
}

/// Join `<exe_dir>/../Resources/<name>` the same way the Zig launcher does,
/// preserving the literal `..` segment so the path matches byte-for-byte.
fn join_resources(exe_dir: &Path, name: &str) -> String {
    path_str(&exe_dir.join("..").join("Resources").join(name))
}

/// Render a path to a `String`. Paths originate from `current_exe` and fixed
/// ASCII segments, so lossy conversion never actually loses data here.
fn path_str(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
