//! Windows/Linux install pipeline: decompress → untar → relocate the bundle →
//! fix permissions/symlinks → platform shortcuts.
//!
//! macOS does not use this path; it replaces the surrounding `.app` bundle in
//! place (see `macos.rs`). The decompressed `.tar` is retained here, named by
//! hash, for the Updater API to diff against later.

use std::fs;
use std::io::{self, Read};
use std::path::Path;

use crate::archive;
use crate::metadata::AppMetadata;
use crate::platform;

/// Decompress `source` (a zstd stream), untar it, move the extracted bundle to
/// `app_dir`, and run all post-install fixups.
///
/// `self_extraction_dir` is the scratch area where the archive is unpacked and
/// where the decompressed `.tar` is retained for the Updater.
pub fn extract_and_install<R: Read>(
    source: R,
    metadata: &AppMetadata,
    self_extraction_dir: &Path,
    app_dir: &Path,
) -> io::Result<bool> {
    // Clean the scratch dir up front so no stale files survive, then recreate
    // it before we write the `.tar` into it.
    let _ = fs::remove_dir_all(self_extraction_dir);
    fs::create_dir_all(self_extraction_dir)?;

    // Decompress straight to a `.tar` on disk, kept (named by hash) for the
    // Updater API.
    let tar_name = match &metadata.hash {
        Some(hash) => format!("{hash}.tar"),
        None => "current.tar".to_string(),
    };
    let tar_path = self_extraction_dir.join(&tar_name);

    println!("Decompressing...");
    let bytes = archive::decompress_zstd_to_file(source, &tar_path)?;
    println!("Decompressed {bytes} bytes");

    println!("Extracting files...");
    archive::extract_tar(&tar_path, self_extraction_dir)?;

    // The bundle inside self-extraction is named `<sanitized>[-channel]`.
    let bundle_name = metadata.bundle_base_name();
    let extracted_app_path = self_extraction_dir.join(&bundle_name);
    if !extracted_app_path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "extracted bundle not found at {}",
                extracted_app_path.display()
            ),
        ));
    }

    // Replace any prior install.
    let _ = fs::remove_dir_all(app_dir);

    relocate_bundle(&extracted_app_path, app_dir)?;

    platform::fix_executable_permissions(app_dir)?;
    platform::fix_cef_symlinks(app_dir)?;

    #[cfg(target_os = "linux")]
    platform::create_desktop_shortcut(app_dir, metadata)?;

    #[cfg(target_os = "windows")]
    platform::create_windows_shortcut(app_dir, metadata)?;

    println!("Installation completed successfully!");
    Ok(true)
}

/// Move the extracted bundle into place. Unix can `rename` across directories;
/// Windows cannot do so reliably, so it copies then removes the source.
fn relocate_bundle(src: &Path, dest: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        platform::copy_directory(src, dest)?;
        let _ = fs::remove_dir_all(src);
        Ok(())
    }
    #[cfg(not(windows))]
    {
        fs::rename(src, dest)
    }
}
