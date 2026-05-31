//! Per-platform resolution of the application-support root directory.
//!
//! Mirrors the Zig extractor exactly:
//!   - Windows: `%LOCALAPPDATA%` → `%APPDATA%` → `%USERPROFILE%\AppData\Local`
//!   - Linux:   `$XDG_DATA_HOME` → `$HOME/.local/share`
//!   - macOS:   `~/Library/Application Support` (matches `std.fs.getAppDataDir`)

use std::io;
use std::path::PathBuf;

#[cfg(target_os = "windows")]
pub fn app_data_dir() -> io::Result<PathBuf> {
    use std::env;

    if let Ok(local) = env::var("LOCALAPPDATA") {
        return Ok(PathBuf::from(local));
    }
    if let Ok(roaming) = env::var("APPDATA") {
        return Ok(PathBuf::from(roaming));
    }
    let userprofile = env::var("USERPROFILE")
        .map_err(|_| io::Error::new(io::ErrorKind::NotFound, "USERPROFILE not set"))?;
    Ok(PathBuf::from(userprofile).join("AppData").join("Local"))
}

#[cfg(target_os = "linux")]
pub fn app_data_dir() -> io::Result<PathBuf> {
    use std::env;

    if let Ok(xdg) = env::var("XDG_DATA_HOME") {
        return Ok(PathBuf::from(xdg));
    }
    let home =
        env::var("HOME").map_err(|_| io::Error::new(io::ErrorKind::NotFound, "HOME not set"))?;
    Ok(PathBuf::from(home).join(".local").join("share"))
}

#[cfg(target_os = "macos")]
pub fn app_data_dir() -> io::Result<PathBuf> {
    dirs::data_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "could not resolve data dir"))
}
