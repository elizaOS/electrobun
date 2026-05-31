//! Decompression (zstd) and tar extraction.
//!
//! `ruzstd` is a pure-Rust zstd decoder (cross-compiles cleanly to
//! riscv64, unlike the C `zstd` binding). We stream the decompressed bytes
//! straight to a `.tar` on disk rather than buffering the whole archive in
//! memory (the Zig version held the entire tar in an `ArrayList`).
//!
//! Tar extraction preserves unix modes and symlinks and rejects any path that
//! would escape the extraction directory (absolute paths, Windows drive
//! letters, or `..` components) — matching the hardened `fullFileName` check in
//! the Zig port.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use tar::{Archive, EntryType};

const COPY_BUF_LEN: usize = 64 * 1024;

/// Stream-decompress a zstd source into `dest` (a `.tar` file), returning the
/// number of decompressed bytes written.
pub fn decompress_zstd_to_file<R: Read>(source: R, dest: &Path) -> io::Result<u64> {
    let mut decoder = ruzstd::StreamingDecoder::new(source)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut out = File::create(dest)?;

    let mut buf = vec![0u8; COPY_BUF_LEN];
    let mut total: u64 = 0;
    loop {
        // `StreamingDecoder`'s `Read` impl already yields `io::Error`.
        let n = decoder.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        total += n as u64;
    }
    out.flush()?;
    Ok(total)
}

/// Reject any tar member path that could escape the extraction root.
///
/// Mirrors the Zig `fullFileName` security check: no absolute paths, no Windows
/// drive letters, no `..` components (covering both `/` and `\` separators).
fn safe_relative_path(raw: &Path) -> io::Result<PathBuf> {
    let s = raw.to_string_lossy();

    if let Some(first) = s.as_bytes().first() {
        if *first == b'/' || *first == b'\\' {
            return Err(traversal(&s));
        }
    }
    if s.as_bytes().get(1) == Some(&b':') {
        return Err(traversal(&s));
    }

    // Split on both separators so a `\`-bearing entry can't smuggle `..` past a
    // unix-only component scan.
    for component in s.split(['/', '\\']) {
        if component == ".." {
            return Err(traversal(&s));
        }
    }

    // Strip prefix/root defensively; only normal components survive.
    let mut clean = PathBuf::new();
    for comp in raw.components() {
        match comp {
            Component::Normal(part) => clean.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(traversal(&s));
            }
        }
    }
    Ok(clean)
}

fn traversal(path: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("path traversal rejected: {path}"),
    )
}

/// Extract a tar file into `dest_dir`.
///
/// Preserves unix permission bits and symlinks; hard links and other exotic
/// entry types are rejected (the Zig port returned `TarUnsupportedFileType`).
///
/// The caller owns `dest_dir`'s lifecycle and must clear it beforehand if a
/// clean extraction is required — this function does not wipe it, because both
/// callers write the source `.tar` *into* `dest_dir` before extracting.
pub fn extract_tar(tar_path: &Path, dest_dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dest_dir)?;

    let file = File::open(tar_path)?;
    let mut archive = Archive::new(file);
    archive.set_preserve_permissions(true);
    archive.set_preserve_mtime(false);
    archive.set_overwrite(true);

    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_type = entry.header().entry_type();
        let raw_path = entry.path()?.into_owned();
        let rel = safe_relative_path(&raw_path)?;
        let out_path = dest_dir.join(&rel);

        match entry_type {
            EntryType::Directory => {
                fs::create_dir_all(&out_path)?;
            }
            EntryType::Regular | EntryType::Continuous => {
                if let Some(parent) = out_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                entry.unpack(&out_path)?;
            }
            EntryType::Symlink => {
                if let Some(parent) = out_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                let target = entry.link_name()?.ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "symlink missing target")
                })?;
                create_symlink(&target, &out_path)?;
            }
            EntryType::GNULongName
            | EntryType::GNULongLink
            | EntryType::XHeader
            | EntryType::XGlobalHeader => {
                // PAX/GNU metadata entries: the `tar` crate folds them into the
                // following entry, so nothing to materialize here.
            }
            EntryType::Link => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "hard links are not supported",
                ));
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "unsupported tar entry type",
                ));
            }
        }
    }

    Ok(())
}

#[cfg(unix)]
fn create_symlink(target: &Path, link: &Path) -> io::Result<()> {
    use std::os::unix::fs::symlink;
    // Replace any pre-existing entry, mirroring the Zig delete-then-retry path.
    match symlink(target, link) {
        Ok(()) => Ok(()),
        Err(_) => {
            let _ = fs::remove_file(link);
            symlink(target, link)
        }
    }
}

#[cfg(windows)]
fn create_symlink(_target: &Path, _link: &Path) -> io::Result<()> {
    // Symlinks require elevated privileges on Windows; the Zig port skipped
    // them entirely. Treat as a no-op so extraction proceeds.
    Ok(())
}
