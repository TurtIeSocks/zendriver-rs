//! Zip archive extraction.
//!
//! Unzips a Chrome for Testing archive into a destination directory.
//! Chrome for Testing ships a `.zip` on every platform (Linux, macOS, Windows),
//! so a single sync `zip::ZipArchive` walk wrapped in
//! [`tokio::task::spawn_blocking`] handles all three.
//!
//! On Unix, executable bits from the archive's `unix_mode()` are preserved
//! (masked to the standard rwx triplet) so the extracted Chrome binary stays
//! runnable without a chmod pass.
//!
//! # Trust boundary
//!
//! The archives we accept come from the Chrome for Testing CDN (Google).
//! We trust the *content* of those archives — they may include arbitrary
//! files and executable bits because that's what running Chrome requires.
//! We do **not** trust the archive's *paths*: a malicious or corrupt zip
//! could ship absolute paths, `..` segments, or symlinks that try to
//! write outside `dest_dir`. The extractor defends against those classes
//! of attack:
//!
//! 1. `zip::read::ZipFile::enclosed_name()` rejects any entry whose
//!    normalized path escapes the archive root (absolute, parent-traversal).
//! 2. After joining with `dest_dir`, the resolved path is verified to still
//!    sit under `dest_dir` — defense in depth against any future change
//!    to `enclosed_name`'s semantics.
//! 3. Symlink entries (detected via `unix_mode() & S_IFMT == S_IFLNK`) are
//!    skipped — Chrome for Testing archives never ship symlinks, and
//!    accepting them would let an attacker plant a follow-on write that
//!    escapes the directory via symlink resolution.
//! 4. Optional `expected_top_prefix` parameter requires every non-empty
//!    entry to live under a single named top-level directory (e.g.
//!    `chrome-linux64/`); enforced from the fetcher to lock the archive
//!    to the CfT layout we expect.
//! 5. Mode bits are masked with [`ARCHIVE_MODE_MASK`] before they reach
//!    `set_permissions`, so a setuid / setgid / sticky bit in the archive
//!    can never land on an extracted file.

use std::io;
use std::path::{Path, PathBuf};

use crate::error::FetcherError;

/// Permission bits kept from an archive entry's `unix_mode()`.
///
/// Chrome for Testing only needs the standard rwx triplet, so the mask drops
/// the file-type bits *and* setuid / setgid / sticky (`0o7000`). Without it a
/// hostile or malformed archive could get a setuid-root or setgid binary
/// written into the cache directory.
#[cfg(unix)]
pub(crate) const ARCHIVE_MODE_MASK: u32 = 0o0777;

/// Unzips `archive_path` into `dest_dir`, preserving directory layout and
/// (on Unix) executable bits from the archive.
///
/// `dest_dir` must already exist. If `expected_top_prefix` is `Some`, every
/// entry must live under that single top-level directory (matches the CfT
/// `chrome-<platform>/...` layout). Per-entry errors (corrupt zip, IO
/// failure, unsafe path, missing prefix) surface as
/// [`FetcherError::Extraction`] or [`FetcherError::Io`].
pub(crate) async fn extract(
    archive_path: &Path,
    dest_dir: &Path,
    expected_top_prefix: Option<&str>,
) -> Result<(), FetcherError> {
    let archive_path = archive_path.to_path_buf();
    let dest_dir = dest_dir.to_path_buf();
    let expected_top_prefix = expected_top_prefix.map(str::to_owned);

    tokio::task::spawn_blocking(move || {
        extract_blocking(&archive_path, &dest_dir, expected_top_prefix.as_deref())
    })
    .await
    .map_err(|e| FetcherError::Extraction(format!("join error: {e}")))?
}

/// Synchronous unzip body — runs on a blocking thread.
fn extract_blocking(
    archive_path: &Path,
    dest_dir: &Path,
    expected_top_prefix: Option<&str>,
) -> Result<(), FetcherError> {
    let file = std::fs::File::open(archive_path)?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| FetcherError::Extraction(e.to_string()))?;

    // Canonicalize dest_dir up front for the containment check below.
    // `dest_dir` is created by the fetcher before invoking extract, so
    // canonicalize is expected to succeed.
    let dest_canonical = std::fs::canonicalize(dest_dir)
        .map_err(|e| FetcherError::Extraction(format!("dest_dir canonicalize: {e}")))?;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| FetcherError::Extraction(e.to_string()))?;

        // Reject entries with unsafe paths (absolute / parent-traversal).
        let Some(rel_path) = entry.enclosed_name() else {
            return Err(FetcherError::Extraction(format!(
                "zip entry has unsafe path: {}",
                entry.name()
            )));
        };

        // Enforce CfT top-level directory when the caller supplies one.
        if let Some(prefix) = expected_top_prefix {
            let top = rel_path.components().next().and_then(|c| match c {
                std::path::Component::Normal(s) => s.to_str(),
                _ => None,
            });
            if top != Some(prefix) {
                return Err(FetcherError::Extraction(format!(
                    "zip entry {:?} not under expected top-level {:?}",
                    rel_path, prefix
                )));
            }
        }

        // Refuse symlink entries — CfT archives don't use them and they
        // are the primary follow-on vector for zip-slip on Unix.
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            const S_IFMT: u32 = 0o170_000;
            const S_IFLNK: u32 = 0o120_000;
            if mode & S_IFMT == S_IFLNK {
                return Err(FetcherError::Extraction(format!(
                    "zip entry {:?} is a symlink; refusing for safety",
                    rel_path
                )));
            }
        }

        let out_path: PathBuf = dest_dir.join(&rel_path);

        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)?;
            assert_under_dest(&out_path, &dest_canonical)?;
            continue;
        }

        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Defense in depth: verify the resolved path is still under
        // `dest_canonical`. Catches future regressions in `enclosed_name`
        // semantics or surprise from filesystem-level path resolution.
        let mut probe_path = out_path.clone();
        while !probe_path.exists() {
            // Walk up until we hit a directory that exists, so canonicalize
            // can succeed. The leaf file doesn't exist yet (we're about to
            // create it), so canonicalize its parent chain.
            if !probe_path.pop() {
                break;
            }
        }
        assert_under_dest(&probe_path, &dest_canonical)?;

        let mut out_file = std::fs::File::create(&out_path)?;
        io::copy(&mut entry, &mut out_file)?;

        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt as _;
            let safe_mode = mode & ARCHIVE_MODE_MASK;
            std::fs::set_permissions(&out_path, std::fs::Permissions::from_mode(safe_mode))?;
        }
    }

    Ok(())
}

/// Verify `path`, after `canonicalize`, still has `dest_canonical` as a
/// prefix. Used to enforce the dest_dir containment invariant.
fn assert_under_dest(path: &Path, dest_canonical: &Path) -> Result<(), FetcherError> {
    let resolved = std::fs::canonicalize(path)
        .map_err(|e| FetcherError::Extraction(format!("path canonicalize: {e}")))?;
    if !resolved.starts_with(dest_canonical) {
        return Err(FetcherError::Extraction(format!(
            "zip entry resolves to {:?} which is outside dest_dir {:?}",
            resolved, dest_canonical
        )));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[tokio::test]
    async fn extract_recovers_single_file_contents() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("test.zip");
        let dest_dir = dir.path().join("out");
        std::fs::create_dir_all(&dest_dir).unwrap();

        // Build an in-memory zip with one file "test.txt" -> "hello world".
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buf);
            writer
                .start_file("test.txt", zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"hello world").unwrap();
            writer.finish().unwrap();
        }
        std::fs::write(&zip_path, buf.into_inner()).unwrap();

        extract(&zip_path, &dest_dir, None).await.unwrap();

        let extracted = std::fs::read(dest_dir.join("test.txt")).unwrap();
        assert_eq!(extracted, b"hello world");
    }

    /// `expected_top_prefix` enforces every entry lives under the named
    /// top-level dir. A bare-file zip (no leading directory) is rejected.
    #[tokio::test]
    async fn extract_rejects_entries_outside_expected_top_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("badroot.zip");
        let dest_dir = dir.path().join("out");
        std::fs::create_dir_all(&dest_dir).unwrap();

        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buf);
            writer
                .start_file(
                    "not-chrome/chrome",
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
            writer.write_all(b"x").unwrap();
            writer.finish().unwrap();
        }
        std::fs::write(&zip_path, buf.into_inner()).unwrap();

        let err = extract(&zip_path, &dest_dir, Some("chrome-linux64"))
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("expected top-level"),
            "unexpected error: {msg}"
        );
    }

    /// A symlink entry (Unix mode bits include `S_IFLNK`) is rejected even
    /// when its name is a safe relative path. CfT archives never use them.
    #[cfg(unix)]
    #[tokio::test]
    async fn extract_rejects_symlink_entries() {
        use zip::write::SimpleFileOptions;
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("symlink.zip");
        let dest_dir = dir.path().join("out");
        std::fs::create_dir_all(&dest_dir).unwrap();

        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buf);
            // 0o120_000 | 0o777 marks the entry as a symlink in the
            // archive's Unix mode bits.
            let opts = SimpleFileOptions::default().unix_permissions(0o777);
            writer
                .add_symlink("chrome-linux64/link", "../escape", opts)
                .unwrap();
            writer.finish().unwrap();
        }
        std::fs::write(&zip_path, buf.into_inner()).unwrap();

        let err = extract(&zip_path, &dest_dir, Some("chrome-linux64"))
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("symlink"),
            "unexpected error: {err}"
        );
    }

    /// Overwrite the external-attributes field of every central-directory
    /// header in `zip_bytes` with `mode`, shifted into the high half where
    /// `ZipFile::unix_mode` reads it.
    ///
    /// The `zip` writer masks `unix_permissions` down to `0o777`, so a
    /// setuid archive cannot be produced through its API — patching the
    /// bytes is the only way to model a hostile or malformed archive.
    /// Layout per APPNOTE 4.3.12: the header starts with `PK\x01\x02` and
    /// carries its 4-byte external attributes at offset 38.
    #[cfg(unix)]
    fn patch_central_dir_mode(zip_bytes: &mut [u8], mode: u32) {
        const CENTRAL_DIR_SIG: [u8; 4] = [b'P', b'K', 1, 2];
        const EXTERNAL_ATTRS_OFFSET: usize = 38;

        let attrs = (mode << 16).to_le_bytes();
        let mut patched = 0usize;
        for i in 0..zip_bytes.len().saturating_sub(EXTERNAL_ATTRS_OFFSET + 4) {
            if zip_bytes[i..i + 4] == CENTRAL_DIR_SIG {
                let at = i + EXTERNAL_ATTRS_OFFSET;
                zip_bytes[at..at + 4].copy_from_slice(&attrs);
                patched += 1;
            }
        }
        assert!(patched > 0, "no central directory header found in zip");
    }

    /// A hostile archive that asks for setuid + setgid + sticky gets only
    /// the rwx triplet applied: the extracted file must not be setuid.
    #[cfg(unix)]
    #[tokio::test]
    async fn extract_masks_setuid_setgid_and_sticky_bits() {
        use std::os::unix::fs::PermissionsExt as _;
        use zip::write::SimpleFileOptions;

        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("setuid.zip");
        let dest_dir = dir.path().join("out");
        std::fs::create_dir_all(&dest_dir).unwrap();

        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buf);
            writer
                .start_file(
                    "chrome-linux64/chrome",
                    SimpleFileOptions::default().unix_permissions(0o755),
                )
                .unwrap();
            writer.write_all(b"payload").unwrap();
            writer.finish().unwrap();
        }
        let mut zip_bytes = buf.into_inner();
        // S_IFREG | setuid | setgid | sticky | rwxr-xr-x
        patch_central_dir_mode(&mut zip_bytes, 0o100_000 | 0o7755);
        std::fs::write(&zip_path, zip_bytes).unwrap();

        extract(&zip_path, &dest_dir, Some("chrome-linux64"))
            .await
            .unwrap();

        let out = dest_dir.join("chrome-linux64/chrome");
        let mode = std::fs::metadata(&out).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o7777,
            0o755,
            "archive mode bits must be masked to {ARCHIVE_MODE_MASK:o}, got {:o}",
            mode & 0o7777
        );
    }

    /// The mask must not cost us the executable bit on a well-formed CfT
    /// archive — a plain `0o755` entry still extracts as `0o755`.
    #[cfg(unix)]
    #[tokio::test]
    async fn extract_preserves_ordinary_executable_bits() {
        use std::os::unix::fs::PermissionsExt as _;
        use zip::write::SimpleFileOptions;

        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("exec.zip");
        let dest_dir = dir.path().join("out");
        std::fs::create_dir_all(&dest_dir).unwrap();

        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buf);
            writer
                .start_file(
                    "chrome-linux64/chrome",
                    SimpleFileOptions::default().unix_permissions(0o755),
                )
                .unwrap();
            writer.write_all(b"payload").unwrap();
            writer.finish().unwrap();
        }
        std::fs::write(&zip_path, buf.into_inner()).unwrap();

        extract(&zip_path, &dest_dir, Some("chrome-linux64"))
            .await
            .unwrap();

        let out = dest_dir.join("chrome-linux64/chrome");
        let mode = std::fs::metadata(&out).unwrap().permissions().mode();
        assert_eq!(mode & 0o7777, 0o755, "got {:o}", mode & 0o7777);
    }
}
