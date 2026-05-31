//! Electrobun self-extracting installer / updater.
//!
//! Two top-level flows:
//!   - Windows/Linux: the archive is appended to (or shipped beside) this very
//!     binary; see [`selfextract`].
//!   - macOS: the archive lives in the surrounding `.app` bundle's Resources;
//!     see [`macos`].
//!
//! Rust port of `package/src/extractor/main.zig`, preserving behaviour exactly
//! while dropping the Zig version's confirmed-dead helpers and debug spew.

mod appdata;
mod archive;
mod metadata;

// macOS replaces the surrounding `.app` bundle in place and relaunches; it
// never runs the shared install/self-extract pipeline.
#[cfg(target_os = "macos")]
mod macos;

// Windows/Linux append the archive to (or ship it beside) this binary and run
// the full install pipeline.
#[cfg(any(target_os = "windows", target_os = "linux"))]
mod install;
#[cfg(any(target_os = "windows", target_os = "linux"))]
mod platform;
#[cfg(any(target_os = "windows", target_os = "linux"))]
mod selfextract;

use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ERROR: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn run() -> std::io::Result<()> {
    if selfextract::extract_from_self()? {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "not a valid self-extracting installer",
        ))
    }
}

#[cfg(target_os = "macos")]
fn run() -> std::io::Result<()> {
    macos::run()
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
fn run() -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "unsupported platform",
    ))
}
