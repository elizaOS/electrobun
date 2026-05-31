//! macOS extraction flow.
//!
//! Unlike Windows/Linux, macOS does not embed the archive in the extractor.
//! The extractor lives at `<App>.app/Contents/Resources/<hash>.tar.zst` and:
//!   1. reads `../../Contents/Resources/metadata.json`,
//!   2. opens `../Resources/<hash>.tar.zst`,
//!   3. streams zstd → `<appdata>/<id>/<channel>/self-extraction/<hash>.tar`,
//!   4. untars into that directory,
//!   5. renames the freshly extracted `<name>[-channel].app` OVER the outer
//!      bundle (preserving its code signature — no chmod, no quarantine touch),
//!   6. relaunches the updated bundle via `open`.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::appdata;
use crate::archive;
use crate::metadata::AppMetadata;

pub fn run() -> io::Result<()> {
    let exe_dir = std::env::current_exe()?
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no exe parent"))?
        .to_path_buf();

    // Outer bundle root: <App>.app (two levels up from Contents/MacOS).
    let app_bundle_path = canonical_join(&exe_dir, "../../");

    let metadata_path = app_bundle_path
        .join("Contents")
        .join("Resources")
        .join("metadata.json");
    let metadata_bytes = fs::read(&metadata_path).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!("failed to read {}: {e}", metadata_path.display()),
        )
    })?;
    let metadata = AppMetadata::parse(&metadata_bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    let hash = metadata
        .hash
        .as_deref()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "macOS metadata missing hash"))?;

    let app_data_root = appdata::app_data_dir()?
        .join(&metadata.identifier)
        .join(&metadata.channel);

    let resources_path = canonical_join(&exe_dir, "../Resources/");
    let compressed_path = resources_path.join(format!("{hash}.tar.zst"));
    let compressed = File::open(&compressed_path)?;

    let self_extraction_path = app_data_root.join("self-extraction");
    if self_extraction_path.exists() {
        fs::remove_dir_all(&self_extraction_path)?;
    }
    fs::create_dir_all(&self_extraction_path)?;

    // Decompress to `<hash>.tar`, then untar.
    let tar_path = self_extraction_path.join(format!("{hash}.tar"));
    archive::decompress_zstd_to_file(compressed, &tar_path)?;
    archive::extract_tar(&tar_path, &self_extraction_path)?;

    let bundle_file_name = format!("{}.app", metadata.bundle_base_name());
    let new_bundle_path = self_extraction_path.join(&bundle_file_name);

    let _ = fs::remove_dir_all(&app_bundle_path);
    fs::rename(&new_bundle_path, &app_bundle_path)?;

    // Relaunch the updated bundle and detach.
    Command::new("open").arg(&app_bundle_path).spawn()?;

    Ok(())
}

/// Join `rel` onto `base` and lexically normalize, resolving `..`/`.` segments
/// without requiring the path to exist (the outer bundle may be mid-rename).
fn canonical_join(base: &Path, rel: &str) -> PathBuf {
    let mut out = base.to_path_buf();
    for part in rel.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}
