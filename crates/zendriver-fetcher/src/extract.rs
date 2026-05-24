//! Zip archive extraction.
//!
//! Unzips a Chrome for Testing archive into a destination directory.
//! Chrome for Testing ships a `.zip` on every platform (Linux, macOS, Windows),
//! so a single sync `zip::ZipArchive` walk wrapped in
//! [`tokio::task::spawn_blocking`] handles all three.
//!
//! On Unix, executable bits from the archive's `unix_mode()` are preserved
//! so the extracted Chrome binary stays runnable without a chmod pass.

use std::io;
use std::path::{Path, PathBuf};

use crate::error::FetcherError;

/// Unzips `archive_path` into `dest_dir`, preserving directory layout and
/// (on Unix) executable bits from the archive.
///
/// `dest_dir` must already exist. Per-entry errors (corrupt zip, IO failure)
/// surface as [`FetcherError::Extraction`] or [`FetcherError::Io`].
#[allow(dead_code, reason = "consumed by Fetcher::ensure_chrome in Task 21")]
pub(crate) async fn extract(archive_path: &Path, dest_dir: &Path) -> Result<(), FetcherError> {
    let archive_path = archive_path.to_path_buf();
    let dest_dir = dest_dir.to_path_buf();

    tokio::task::spawn_blocking(move || extract_blocking(&archive_path, &dest_dir))
        .await
        .map_err(|e| FetcherError::Extraction(format!("join error: {e}")))?
}

/// Synchronous unzip body — runs on a blocking thread.
fn extract_blocking(archive_path: &Path, dest_dir: &Path) -> Result<(), FetcherError> {
    let file = std::fs::File::open(archive_path)?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| FetcherError::Extraction(e.to_string()))?;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| FetcherError::Extraction(e.to_string()))?;

        // Skip entries with unsafe paths (absolute / parent-traversal).
        let Some(rel_path) = entry.enclosed_name() else {
            continue;
        };
        let out_path: PathBuf = dest_dir.join(rel_path);

        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)?;
            continue;
        }

        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut out_file = std::fs::File::create(&out_path)?;
        io::copy(&mut entry, &mut out_file)?;

        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&out_path, std::fs::Permissions::from_mode(mode))?;
        }
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

        extract(&zip_path, &dest_dir).await.unwrap();

        let extracted = std::fs::read(dest_dir.join("test.txt")).unwrap();
        assert_eq!(extracted, b"hello world");
    }
}
