//! Windows/Linux self-extracting installer logic.
//!
//! Two strategies, tried in order:
//!   1. (Windows only) An adjacent `<stem>.tar.zst` + `<stem>.metadata.json`,
//!      looked for first in a `.installer/` subdirectory then alongside the exe.
//!   2. The installer reads its OWN bytes and locates the *second* occurrence of
//!      the metadata marker, then the archive marker. The second-occurrence rule
//!      matters: the marker string also lives in the binary's `.rodata` (it is a
//!      string literal in this very file), so the first hit is the code copy and
//!      the second is the payload the CLI appended.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use memchr::memmem;

use crate::appdata;
use crate::install;
use crate::metadata::AppMetadata;

const ARCHIVE_MARKER: &[u8] = b"ELECTROBUN_ARCHIVE_V1";
const METADATA_MARKER: &[u8] = b"ELECTROBUN_METADATA_V1";

/// Entry point for Windows/Linux. Returns `true` if a valid payload was found
/// and installed, `false` if this binary carries no embedded archive.
pub fn extract_from_self() -> io::Result<bool> {
    let exe_path = std::env::current_exe()?;

    #[cfg(target_os = "windows")]
    if let Some(result) = try_adjacent_archive(&exe_path)? {
        return Ok(result);
    }

    extract_from_embedded(&exe_path)
}

/// Compute the `<app_base>/{self-extraction,app}` directory pair.
fn install_dirs(metadata: &AppMetadata) -> io::Result<(PathBuf, PathBuf)> {
    let data_dir = appdata::app_data_dir()?;
    let app_base = data_dir.join(&metadata.identifier).join(&metadata.channel);
    let self_extraction = app_base.join("self-extraction");
    let app_dir = app_base.join("app");
    Ok((self_extraction, app_dir))
}

/// Windows: install from a sidecar archive if present. Returns `None` when no
/// adjacent metadata/archive pair exists (so the caller falls back to embedded).
#[cfg(target_os = "windows")]
fn try_adjacent_archive(exe_path: &Path) -> io::Result<Option<bool>> {
    let exe_dir = match exe_path.parent() {
        Some(d) => d,
        None => return Ok(None),
    };
    let stem = match exe_path.file_stem() {
        Some(s) => s.to_string_lossy().into_owned(),
        None => return Ok(None),
    };

    let archive_name = format!("{stem}.tar.zst");
    let metadata_name = format!("{stem}.metadata.json");

    // `.installer/` subdirectory wins over the legacy adjacent location.
    let archive_path = first_existing(&[
        exe_dir.join(".installer").join(&archive_name),
        exe_dir.join(&archive_name),
    ]);
    let metadata_path = first_existing(&[
        exe_dir.join(".installer").join(&metadata_name),
        exe_dir.join(&metadata_name),
    ]);

    let (Some(archive_path), Some(metadata_path)) = (archive_path, metadata_path) else {
        return Ok(None);
    };

    let metadata_bytes = std::fs::read(&metadata_path)?;
    let metadata = AppMetadata::parse(&metadata_bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

    let (self_extraction_dir, app_dir) = install_dirs(&metadata)?;

    let archive_file = File::open(&archive_path)?;
    let result =
        install::extract_and_install(archive_file, &metadata, &self_extraction_dir, &app_dir)?;
    Ok(Some(result))
}

#[cfg(target_os = "windows")]
fn first_existing(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates.iter().find(|p| p.exists()).cloned()
}

/// Read this binary's bytes, locate the appended payload, and install it.
fn extract_from_embedded(exe_path: &Path) -> io::Result<bool> {
    let mut file = File::open(exe_path)?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;

    let Some((metadata_start, archive_marker_pos)) = locate_payload(&buf) else {
        return Ok(false);
    };

    let metadata_bytes = &buf[metadata_start..archive_marker_pos];
    if metadata_bytes.len() > 4096 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "metadata too large",
        ));
    }
    let metadata = AppMetadata::parse(metadata_bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

    let archive_start = archive_marker_pos + ARCHIVE_MARKER.len();

    let (self_extraction_dir, app_dir) = install_dirs(&metadata)?;

    // Stream the compressed payload straight from the file at the archive
    // offset rather than copying it into a second buffer.
    file.seek(SeekFrom::Start(archive_start as u64))?;

    install::extract_and_install(&mut file, &metadata, &self_extraction_dir, &app_dir)
}

/// Find `(metadata_start, archive_marker_pos)` in the binary.
///
/// `metadata_start` is the byte just after the *second* metadata marker;
/// `archive_marker_pos` is where the archive marker begins (i.e. the end of the
/// metadata JSON). Returns `None` if the payload markers are absent.
fn locate_payload(buf: &[u8]) -> Option<(usize, usize)> {
    let finder = memmem::Finder::new(METADATA_MARKER);
    let mut it = finder.find_iter(buf);

    let _first = it.next()?; // code-section copy
    let second = it.next()?; // appended payload

    let metadata_start = second + METADATA_MARKER.len();
    let archive_marker_offset = memmem::find(&buf[metadata_start..], ARCHIVE_MARKER)?;
    let archive_marker_pos = metadata_start + archive_marker_offset;

    Some((metadata_start, archive_marker_pos))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_second_marker_and_archive() {
        // Simulate: [code-copy of marker][padding][payload marker][json][archive marker][data]
        let mut buf = Vec::new();
        buf.extend_from_slice(b"some code referencing ");
        buf.extend_from_slice(METADATA_MARKER); // first (code) occurrence
        buf.extend_from_slice(b" ... more code ...");
        let payload_marker_pos = buf.len();
        buf.extend_from_slice(METADATA_MARKER); // second (payload) occurrence
        let json = br#"{"identifier":"a","name":"b","channel":"stable","hash":"h"}"#;
        buf.extend_from_slice(json);
        let archive_pos = buf.len();
        buf.extend_from_slice(ARCHIVE_MARKER);
        buf.extend_from_slice(b"ZSTDDATA");

        let (metadata_start, archive_marker_pos) = locate_payload(&buf).unwrap();
        assert_eq!(metadata_start, payload_marker_pos + METADATA_MARKER.len());
        assert_eq!(archive_marker_pos, archive_pos);
        assert_eq!(&buf[metadata_start..archive_marker_pos], json);
    }

    #[test]
    fn no_second_marker_returns_none() {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"only one ");
        buf.extend_from_slice(METADATA_MARKER);
        buf.extend_from_slice(b" occurrence");
        assert!(locate_payload(&buf).is_none());
    }

    #[test]
    fn no_marker_returns_none() {
        assert!(locate_payload(b"no markers here").is_none());
    }
}
