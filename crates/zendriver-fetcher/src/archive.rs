//! What the downloaded file *is*, and how to turn it into a runnable tree.
//!
//! Chrome for Testing and the Chromium snapshot bucket ship a zip on every
//! platform, so for years "unpack" and "unzip" were the same verb.
//! ungoogled-chromium is built by three independent per-OS repos and each
//! packages differently:
//!
//! | Distribution | Platform | Archive |
//! |---|---|---|
//! | Chrome for Testing | all | zip |
//! | Chromium snapshot | all | zip |
//! | ungoogled-chromium | Windows | zip |
//! | ungoogled-chromium | Linux | AppImage |
//! | ungoogled-chromium | macOS | dmg |
//!
//! The Linux and macOS rows are why this module exists.
//!
//! **Linux** takes the `.AppImage` rather than the `.tar.xz` published beside
//! it. An AppImage *is* the executable — install is a move plus a chmod, and
//! no xz decompressor enters the dependency graph. (It does want FUSE at run
//! time; `--appimage-extract` is the escape hatch on hosts without it.)
//!
//! **macOS** has no zip option at all: ungoogled publishes `.dmg` only. A
//! disk image is unpacked with `hdiutil`, which exists only on macOS, so that
//! one combination is host-bound and says so via
//! [`FetcherError::UnsupportedArchive`].

use std::path::Path;

use crate::error::FetcherError;

/// Packaging of a resolved download, and the instructions for unpacking it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Archive {
    /// A zip in which every entry lives under one top-level directory.
    ///
    /// The prefix is pinned rather than inferred: it rejects a mislabeled or
    /// tampered archive before anything is written into the cache.
    Zip {
        /// Required top-level directory, e.g. `chrome-linux64`.
        top_dir: String,
    },

    /// A single self-contained executable (an AppImage). "Unpacking" is a
    /// move into the staging directory plus the executable bit.
    Executable {
        /// Filename to store it under inside the build directory.
        file_name: String,
    },

    /// A macOS disk image containing one `.app` bundle.
    AppBundleDmg {
        /// Bundle directory expected on the mounted volume, e.g.
        /// `Chromium.app`.
        app_dir: String,
    },
}

impl Archive {
    /// Extension for the temporary download file.
    ///
    /// `hdiutil` is content-sniffing and does not require `.dmg`, but a
    /// correctly-named temp file is what makes a half-finished cache
    /// directory legible when something goes wrong.
    pub(crate) fn tmp_extension(&self) -> &'static str {
        match self {
            Archive::Zip { .. } => "zip",
            Archive::Executable { .. } => "AppImage",
            Archive::AppBundleDmg { .. } => "dmg",
        }
    }
}

/// Unpack `downloaded` into the already-created staging directory `dest_dir`.
///
/// `dest_dir` is the `<build>.tmp/` sibling the fetcher promotes with a
/// single atomic rename, so a failure here leaves the published cache
/// untouched.
pub(crate) async fn install(
    archive: &Archive,
    downloaded: &Path,
    dest_dir: &Path,
) -> Result<(), FetcherError> {
    match archive {
        Archive::Zip { top_dir } => {
            crate::extract::extract(downloaded, dest_dir, Some(top_dir)).await
        }
        Archive::Executable { file_name } => {
            install_executable(downloaded, dest_dir, file_name).await
        }
        Archive::AppBundleDmg { app_dir } => install_dmg(downloaded, dest_dir, app_dir).await,
    }
}

/// Move a single-file executable (AppImage) into place and make it runnable.
async fn install_executable(
    downloaded: &Path,
    dest_dir: &Path,
    file_name: &str,
) -> Result<(), FetcherError> {
    let target = dest_dir.join(file_name);
    // Rename first (same filesystem: both live under the cache root); fall
    // back to copy so a cache dir spanning mount points still works.
    if tokio::fs::rename(downloaded, &target).await.is_err() {
        tokio::fs::copy(downloaded, &target).await?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut perms = tokio::fs::metadata(&target).await?.permissions();
        perms.set_mode(perms.mode() | 0o755);
        tokio::fs::set_permissions(&target, perms).await?;
    }

    Ok(())
}

/// Mount a `.dmg`, copy the `.app` bundle out of it, unmount.
///
/// macOS only — `hdiutil` ships with the OS and has no portable equivalent.
#[cfg(target_os = "macos")]
async fn install_dmg(
    downloaded: &Path,
    dest_dir: &Path,
    app_dir: &str,
) -> Result<(), FetcherError> {
    // A private mountpoint beside the staging dir, so two concurrent fetches
    // of different builds cannot collide and nothing lands in /Volumes.
    let mountpoint = crate::cache::with_suffix(dest_dir, ".mnt");
    let _ = tokio::fs::remove_dir_all(&mountpoint).await;
    tokio::fs::create_dir_all(&mountpoint).await?;

    // `-nobrowse` keeps it out of Finder; `-noverify` skips the multi-minute
    // checksum pass (the archive's integrity is the fetcher's own
    // `expected_sha256` job); `-noautoopen` stops the volume window opening.
    run(
        "hdiutil",
        &[
            "attach",
            &downloaded.display().to_string(),
            "-mountpoint",
            &mountpoint.display().to_string(),
            "-nobrowse",
            "-noverify",
            "-noautoopen",
            "-readonly",
        ],
    )
    .await?;

    // Copy inside a closure so the detach below runs on every path out.
    let copied = copy_app_bundle(&mountpoint, dest_dir, app_dir).await;

    let detached = run(
        "hdiutil",
        &["detach", &mountpoint.display().to_string(), "-force"],
    )
    .await;
    let _ = tokio::fs::remove_dir_all(&mountpoint).await;

    copied?;
    detached?;
    Ok(())
}

/// `ditto` the bundle out of the mounted volume, verifying it is the layout
/// the resolver promised.
#[cfg(target_os = "macos")]
async fn copy_app_bundle(
    mountpoint: &Path,
    dest_dir: &Path,
    app_dir: &str,
) -> Result<(), FetcherError> {
    let src = mountpoint.join(app_dir);
    if !src.is_dir() {
        // Name what *is* there — a renamed bundle upstream should read as a
        // layout change, not as a mysterious missing file.
        let found = list_dir_names(mountpoint).await;
        return Err(FetcherError::Extraction(format!(
            "expected {app_dir:?} on the mounted image, found {found:?}"
        )));
    }

    // `ditto`, not `cp -R`: an app bundle carries symlinked framework
    // versions and code-signing metadata, and ditto is the tool Apple
    // documents for copying one intact.
    run(
        "ditto",
        &[
            &src.display().to_string(),
            &dest_dir.join(app_dir).display().to_string(),
        ],
    )
    .await
}

/// Entry names directly inside `dir`, for error messages.
#[cfg(target_os = "macos")]
async fn list_dir_names(dir: &Path) -> Vec<String> {
    let mut names = Vec::new();
    if let Ok(mut rd) = tokio::fs::read_dir(dir).await {
        while let Ok(Some(entry)) = rd.next_entry().await {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    names
}

/// Run a command to completion, turning a non-zero exit into an error that
/// carries stderr.
#[cfg(target_os = "macos")]
async fn run(program: &str, args: &[&str]) -> Result<(), FetcherError> {
    let output = tokio::process::Command::new(program)
        .args(args)
        .output()
        .await
        .map_err(|e| FetcherError::Extraction(format!("failed to run {program}: {e}")))?;

    if !output.status.success() {
        return Err(FetcherError::Extraction(format!(
            "{program} exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

/// Disk images are a macOS packaging format and `hdiutil` is a macOS tool.
/// Cross-platform fetching still works for every other combination — only
/// ungoogled-chromium *on* macOS is host-bound.
#[cfg(not(target_os = "macos"))]
async fn install_dmg(
    _downloaded: &Path,
    _dest_dir: &Path,
    app_dir: &str,
) -> Result<(), FetcherError> {
    Err(FetcherError::UnsupportedArchive(format!(
        "cannot unpack {app_dir} from a .dmg on this host — disk images need macOS's `hdiutil`. \
         ungoogled-chromium publishes .dmg only for macOS; fetch it from a Mac, or pick a \
         different --platform"
    )))
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn tmp_extension_matches_the_packaging() {
        assert_eq!(
            Archive::Zip {
                top_dir: "chrome-linux64".into()
            }
            .tmp_extension(),
            "zip"
        );
        assert_eq!(
            Archive::Executable {
                file_name: "chrome.AppImage".into()
            }
            .tmp_extension(),
            "AppImage"
        );
        assert_eq!(
            Archive::AppBundleDmg {
                app_dir: "Chromium.app".into()
            }
            .tmp_extension(),
            "dmg"
        );
    }

    #[tokio::test]
    async fn executable_install_moves_the_file_and_marks_it_runnable() {
        let root = tempfile::tempdir().unwrap();
        let downloaded = root.path().join("build.tmp.AppImage");
        tokio::fs::write(&downloaded, b"#!/bin/sh\necho appimage\n")
            .await
            .unwrap();
        let dest = root.path().join("build.tmp");
        tokio::fs::create_dir_all(&dest).await.unwrap();

        install(
            &Archive::Executable {
                file_name: "chrome.AppImage".into(),
            },
            &downloaded,
            &dest,
        )
        .await
        .unwrap();

        let installed = dest.join("chrome.AppImage");
        assert_eq!(
            tokio::fs::read(&installed).await.unwrap(),
            b"#!/bin/sh\necho appimage\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let meta = tokio::fs::metadata(&installed).await.unwrap();
            assert!(meta.permissions().mode() & 0o111 != 0);
        }
    }

    /// On a non-macOS host the dmg path must fail with a message that says
    /// why and what to do, not with a generic IO error.
    #[cfg(not(target_os = "macos"))]
    #[tokio::test]
    async fn dmg_on_a_non_mac_host_explains_itself() {
        let root = tempfile::tempdir().unwrap();
        let err = install(
            &Archive::AppBundleDmg {
                app_dir: "Chromium.app".into(),
            },
            &root.path().join("x.dmg"),
            root.path(),
        )
        .await
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("hdiutil"), "{msg}");
        assert!(msg.contains("--platform"), "{msg}");
    }
}
