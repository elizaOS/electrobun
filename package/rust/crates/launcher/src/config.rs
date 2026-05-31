//! Reads the bundle's `build.json` / `version.json` to decide how to launch.
//!
//! Both files live at `<exe_dir>/../Resources/`. Any read or parse failure is
//! treated as "use the default" exactly like the Zig launcher's `catch return`
//! arms: a missing/garbage `build.json` means `Bun`, a missing/garbage
//! `version.json` means "not a dev build".

use std::path::Path;

use serde_json::Value;

use crate::MainProcess;

/// Upper bound on the config files we read, matching the Zig launcher's
/// `readToEndAlloc(.., 1024 * 10)`. These files are tiny; anything larger is
/// treated as unreadable.
const MAX_CONFIG_BYTES: u64 = 1024 * 10;

/// Determine the main process from `Resources/build.json`.
///
/// `mainProcess == "zig"` selects the native binary. Everything else —
/// including a missing file, a parse error, or any other value such as
/// `"bun"`/`"native"` — defaults to [`MainProcess::Bun`], matching the Zig
/// launcher which only special-cases the literal `"zig"`.
pub fn detect_main_process(exe_dir: &Path) -> MainProcess {
    let Some(value) = read_resource_json(exe_dir, "build.json") else {
        return MainProcess::Bun;
    };

    match value.get("mainProcess").and_then(Value::as_str) {
        Some("zig") => MainProcess::Native,
        _ => MainProcess::Bun,
    }
}

/// Returns `true` when `Resources/version.json` has `"channel": "dev"`.
///
/// A missing file, parse error, or any other channel value returns `false`.
pub fn is_dev_build(exe_dir: &Path) -> bool {
    let Some(value) = read_resource_json(exe_dir, "version.json") else {
        return false;
    };

    value.get("channel").and_then(Value::as_str) == Some("dev")
}

/// Read and JSON-parse `<exe_dir>/../Resources/<name>`, returning `None` on any
/// IO or parse failure (the "use the default" signal for callers).
fn read_resource_json(exe_dir: &Path, name: &str) -> Option<Value> {
    let path = exe_dir.join("..").join("Resources").join(name);

    let metadata = std::fs::metadata(&path).ok()?;
    if metadata.len() > MAX_CONFIG_BYTES {
        return None;
    }

    let content = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}
