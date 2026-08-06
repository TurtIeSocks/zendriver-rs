//! Zip archive extraction.
//!
//! Unzips a Chromium archive into a destination directory. Chrome for Testing
//! and the Chromium snapshot bucket ship a `.zip` on every platform, as does
//! ungoogled-chromium on Windows, so a single sync `zip::ZipArchive` walk
//! wrapped in [`tokio::task::spawn_blocking`] handles all of them.
//! ungoogled's other two packagings are [`crate::archive`]'s problem.
//!
//! On Unix, executable bits from the archive's `unix_mode()` are preserved
//! so the extracted Chrome binary stays runnable without a chmod pass.
//!
//! # Trust boundary
//!
//! The archives reaching here come from Google's Chrome for Testing CDN, the
//! Chromium snapshot bucket, and GitHub release assets.
//! We trust the *content* of those archives — they may include arbitrary
//! files and executable bits because that's what running Chrome requires.
//! We do **not** trust the archive's *paths*: a malicious or corrupt zip
//! could ship absolute paths, `..` segments, or symlinks that try to
//! write outside `dest_dir`. The extractor defends against those classes
//! of attack:
//!
//! 1. `zip::read::ZipFile::enclosed_name()` refuses an absolute entry path,
//!    and any path whose running depth goes negative. Note what it does NOT
//!    do: it returns what it accepts **unnormalized**, so `a/b/../../c`
//!    survives with its `..` intact. Nothing here may assume otherwise.
//! 2. After joining with `dest_dir`, the resolved path is verified to still
//!    sit under `dest_dir` — defense in depth against any future change
//!    to `enclosed_name`'s semantics. The probe that finds an existing
//!    ancestor to canonicalize uses `symlink_metadata`, which does not follow
//!    links, so a DANGLING link is seen rather than stepped over.
//! 3. Symlink entries (detected via `unix_mode() & S_IFMT == S_IFLNK`) are
//!    extracted only after their TARGET is validated to stay inside the
//!    archive's top-level directory; anything else is refused.
//!
//!    This used to refuse every symlink, justified as "Chrome for Testing
//!    archives never ship symlinks". That is FALSE on macOS: every `.app`
//!    bundle carries framework `Versions/Current`-style links, so
//!    `Fetcher::ensure_chrome()` had never once succeeded there —
//!
//!    ```text
//!    zip entry "chrome-mac-arm64/Google Chrome for Testing.app/Contents/
//!    Frameworks/Google Chrome for Testing Framework.framework/Resources"
//!    is a symlink; refusing for safety
//!    ```
//!
//!    The refusal existed for a real reason and the replacement keeps it
//!    closed. Symlinks are the primary FOLLOW-ON vector for zip-slip: extract
//!    `evil -> /etc`, then extract `evil/passwd` and the write lands outside
//!    while every path in the archive looks innocent.
//!
//!    So a target is accepted only when it is relative AND resolves, from the
//!    directory that really contains the link, to somewhere still inside the
//!    top-level directory — checked at every step of the climb, not just at
//!    the end.
//!
//!    **Reasoning lexically about a path the kernel resolves differently was
//!    the bug, twice.** An earlier entry can plant a symlink, and a later entry
//!    can reach through it two ways — by its own entry path, or by its target
//!    string. Both were exploitable and both are now closed by following the
//!    filesystem rather than the archive's text:
//!
//!    - the link's parent is `canonicalize`d, not taken from the entry path
//!      (`a_symlink_reached_through_an_earlier_symlink_cannot_escape`);
//!    - each target component that ALREADY EXISTS as a symlink is resolved as
//!      the walk pushes it, so `u1/u2/u3/up/../../../X` cannot pop three levels
//!      lexically from `up` while the kernel pops them from wherever `up`
//!      pointed (`a_symlink_target_walking_through_an_earlier_symlink_cannot_escape`).
//!
//!    Components that do not exist yet stay lexical, and that is what keeps
//!    legitimate links working: `Versions/Current` is written before
//!    `Versions/A`, so stat-ing the whole target would reject it.
//!
//!    Windows keeps skipping them: creating a symlink there needs privileges,
//!    and no Windows Chrome archive contains one.
//! 4. Optional `expected_top_prefix` parameter requires every non-empty
//!    entry to live under a single named top-level directory (e.g.
//!    `chrome-linux64/`); enforced from the fetcher to lock the archive
//!    to the CfT layout we expect.

use std::io;
use std::path::{Path, PathBuf};

use crate::error::FetcherError;

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

        let out_path: PathBuf = dest_dir.join(&rel_path);

        // Symlinks: validate the target, then create. See the trust-boundary
        // note at the top of this module for why this is not a blanket refusal.
        #[cfg(unix)]
        if is_symlink(&entry) {
            let mut target = String::new();
            io::Read::read_to_string(&mut entry, &mut target)?;

            // The parent chain has to exist before the target can be resolved:
            // resolution runs against where this link REALLY lands, which means
            // canonicalizing its parent.
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            // Files are already locked to the archive's single top-level
            // directory; hold links to it too. A link that leaves it stays
            // inside dest_dir, so it is not a zip-slip, but it can only point
            // at something this archive does not own — and dest_dir is shared
            // with other installs.
            let containment_root = match expected_top_prefix {
                Some(top) => dest_canonical.join(top),
                None => dest_canonical.clone(),
            };
            resolve_target(&out_path, Path::new(&target), &containment_root).ok_or_else(|| {
                FetcherError::Extraction(format!(
                    "zip entry {rel_path:?} is a symlink to {target:?}, which escapes {}; \
                     refusing",
                    containment_root.display()
                ))
            })?;

            // A re-extraction over a populated dir would otherwise fail with
            // EEXIST; the fetcher extracts into a fresh staging dir, but this
            // keeps the function idempotent rather than order-dependent.
            let _ = std::fs::remove_file(&out_path);
            std::os::unix::fs::symlink(&target, &out_path)?;
            continue;
        }
        // Windows: no privileges to create one, and no Windows archive has any.
        #[cfg(not(unix))]
        if is_symlink(&entry) {
            continue;
        }

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
        // `symlink_metadata`, not `exists()`: `exists` follows links, so a
        // DANGLING one reads as absent and the walk steps straight over it to a
        // parent that passes — then `File::create` follows it and writes
        // wherever it pointed. Not following means a dangling link is seen,
        // and `assert_under_dest` then fails to canonicalize it and refuses.
        while std::fs::symlink_metadata(&probe_path).is_err() {
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
            std::fs::set_permissions(&out_path, std::fs::Permissions::from_mode(mode))?;
        }
    }

    Ok(())
}

/// True when a zip entry's mode marks it a symlink.
fn is_symlink(entry: &zip::read::ZipFile<'_>) -> bool {
    const S_IFMT: u32 = 0o170_000;
    const S_IFLNK: u32 = 0o120_000;
    entry
        .unix_mode()
        .is_some_and(|mode| mode & S_IFMT == S_IFLNK)
}

/// Where does a symlink at `link_path` pointing at `target` actually land?
/// `None` means it leaves `containment_root`.
///
/// `link_path` is the link's absolute path on disk, and its parent must already
/// exist. The target resolves against the DIRECTORY CONTAINING the link — not
/// the link itself. Getting that wrong by one level makes every sibling link
/// look like an escape, and makes a real escape look like a sibling.
///
/// That containing directory is **canonicalized**, not taken from the entry's
/// declared path, and the difference is the whole defence. An archive is
/// extracted in order, so an earlier entry can plant a symlink that a later
/// entry's own path then runs through. Reading the parent off the archive text
/// makes `top/a/b/c/up/hop -> ../../../../X` look like it lands on `top/X`,
/// while the kernel — for which `up` is already a link to `top` — creates it at
/// `top/hop` and resolves four levels up from there, clear of the destination.
/// Canonicalizing collapses that gap: the parent is wherever the kernel says it
/// is, and there is no second opinion to disagree with.
///
/// The target itself is still resolved lexically from there, with no stat, and
/// the containment check runs at every step of the climb rather than on the
/// final result. Both matter: the target routinely does not exist yet at
/// extraction time (`Versions/Current` precedes `Versions/A` in a macOS
/// framework), so anything that stats it would reject legitimate links; and
/// checking only the endpoint would admit `../../a/b`, which leaves and returns.
///
/// Unix-only, because only the Unix arm creates symlinks — Windows skips those
/// entries outright, and an ungated definition is dead code there.
#[cfg(unix)]
fn resolve_target(link_path: &Path, target: &Path, containment_root: &Path) -> Option<PathBuf> {
    use std::path::Component;

    // An absolute target ignores the destination entirely: `evil -> /etc` then
    // `evil/passwd` writes to /etc/passwd with no `..` anywhere in the archive.
    if target.is_absolute() || target.as_os_str().is_empty() {
        return None;
    }

    let mut resolved = std::fs::canonicalize(link_path.parent()?).ok()?;
    if !resolved.starts_with(containment_root) {
        return None;
    }

    for comp in target.components() {
        match comp {
            // Follow rather than assume. If this component already exists and is
            // itself a symlink, the kernel follows it when resolving this
            // target — so the walk has to follow it too, or every component
            // after this one is reasoning about a directory the kernel is not
            // standing in. That is the same lexical-vs-real gap canonicalizing
            // the parent closes, reached through the target instead:
            // `u1/u2/u3/up/../../../X` pops three lexically from `u1/u2/u3/up`
            // and lands inside, while the kernel pops them from wherever `up`
            // pointed and leaves.
            //
            // A component that does not exist yet stays lexical, which is what
            // keeps `Versions/Current -> A` working: `A` is written after the
            // link that names it.
            Component::Normal(name) => {
                resolved.push(name);
                if std::fs::symlink_metadata(&resolved).is_ok_and(|md| md.file_type().is_symlink())
                {
                    resolved = std::fs::canonicalize(&resolved).ok()?;
                    if !resolved.starts_with(containment_root) {
                        return None;
                    }
                }
            }
            Component::CurDir => {}
            // `pop` fails at the filesystem root; leaving the containment root
            // is the escape. Checked as we climb rather than on the final
            // result, so `../../a/b` is refused even though it ends up inside.
            Component::ParentDir => {
                if !resolved.pop() || !resolved.starts_with(containment_root) {
                    return None;
                }
            }
            // A root or drive prefix mid-path is not something a relative
            // target can legitimately contain.
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(resolved)
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

    /// Build a zip containing one symlink entry pointing at `target`, plus any
    /// extra plain files. The target is the entry's CONTENT, and the S_IFLNK
    /// mode bit is what marks it — both are what a real macOS `.app` archive
    /// carries.
    #[cfg(unix)]
    fn zip_with_symlink(link_name: &str, target: &str, extra: &[(&str, &str)]) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buf);
            for (name, body) in extra {
                writer
                    .start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(body.as_bytes()).unwrap();
            }
            writer
                .add_symlink(link_name, target, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.finish().unwrap();
        }
        buf.into_inner()
    }

    #[cfg(unix)]
    async fn extract_bytes(
        zip_bytes: Vec<u8>,
    ) -> (tempfile::TempDir, PathBuf, Result<(), FetcherError>) {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("t.zip");
        let dest = dir.path().join("out");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(&zip_path, zip_bytes).unwrap();
        let r = extract(&zip_path, &dest, None).await;
        (dir, dest, r)
    }

    /// The case that had never worked: a macOS framework's `Versions/Current`
    /// style link, pointing at a sibling INSIDE the archive.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_relative_symlink_inside_the_archive_is_extracted() {
        let z = zip_with_symlink("fw/Versions/Current", "A", &[("fw/Versions/A/lib", "real")]);
        let (_d, dest, r) = extract_bytes(z).await;
        r.expect("a contained symlink must extract");

        let link = dest.join("fw/Versions/Current");
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "it must be a real symlink, not a regular file holding the target text"
        );
        assert_eq!(std::fs::read_link(&link).unwrap(), Path::new("A"));
        // And it resolves to the sibling, which is the whole point.
        assert_eq!(
            std::fs::read_to_string(dest.join("fw/Versions/Current/lib")).unwrap(),
            "real"
        );
    }

    /// `evil -> /etc`, then `evil/passwd`. No `..` anywhere in the archive.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_absolute_target_is_refused() {
        let (_d, _dest, r) = extract_bytes(zip_with_symlink("evil", "/etc", &[])).await;
        let e = r.expect_err("an absolute symlink target must be refused");
        assert!(format!("{e}").contains("escapes"), "{e}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_parent_traversing_target_is_refused() {
        let (_d, _dest, r) =
            extract_bytes(zip_with_symlink("a/b/evil", "../../../../etc", &[])).await;
        let e = r.expect_err("a target climbing past the root must be refused");
        assert!(format!("{e}").contains("escapes"), "{e}");
    }

    /// THE REGRESSION THAT MATTERS. A symlinked directory is extracted first,
    /// then a later entry is written THROUGH it. The write must land inside
    /// dest_dir — which is what makes the containment claim about the whole
    /// archive rather than about each entry in isolation.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_write_through_an_extracted_symlink_stays_inside() {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buf);
            writer
                .start_file("real/keep", zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"x").unwrap();
            writer
                .add_symlink("link", "real", zip::write::SimpleFileOptions::default())
                .unwrap();
            writer
                .start_file("link/through", zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"written through the link").unwrap();
            writer.finish().unwrap();
        }
        let (_d, dest, r) = extract_bytes(buf.into_inner()).await;
        r.expect("writing through a contained symlink is legitimate");

        // It followed the link, so the byte landed in `real/`, and `real/` is
        // inside dest. Both halves matter: that it resolved, and that it stayed.
        assert_eq!(
            std::fs::read_to_string(dest.join("real/through")).unwrap(),
            "written through the link"
        );
        let resolved = std::fs::canonicalize(dest.join("link/through")).unwrap();
        assert!(
            resolved.starts_with(std::fs::canonicalize(&dest).unwrap()),
            "{resolved:?}"
        );
    }

    /// A symlink whose own ENTRY PATH runs through a symlink an earlier entry
    /// created. The archive is extracted in order, so the attacker controls
    /// that ordering.
    ///
    /// Validating the target against the entry's *lexical* parent says
    /// `top/a/b/c/up/hop -> ../../../../ESCAPED` lands on `top/ESCAPED`. The
    /// kernel disagrees: `up` is already a link to `top`, so the link is really
    /// created at `top/hop` and its target climbs four levels from there —
    /// clear of the destination. A third entry writing to `top/hop` then
    /// follows it and lands outside.
    ///
    /// `zip`'s `enclosed_name` is no help: it only refuses a path whose running
    /// depth goes negative, and returns what it accepts unnormalized.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlink_reached_through_an_earlier_symlink_cannot_escape() {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let opts = zip::write::SimpleFileOptions::default();
            let mut writer = zip::ZipWriter::new(&mut buf);
            // 1. A link back up to the top-level dir. Contained, and legal.
            writer
                .add_symlink("top/a/b/c/up", "../../..", opts)
                .unwrap();
            // 2. Created THROUGH it, so it really lands at `top/hop`.
            writer
                .add_symlink("top/a/b/c/up/hop", "../../../../ESCAPED", opts)
                .unwrap();
            // 3. Written through the link planted by 2.
            writer
                .start_file("top/hop", opts.unix_permissions(0o755))
                .unwrap();
            writer.write_all(b"payload").unwrap();
            writer.finish().unwrap();
        }

        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("evil.zip");
        // Nested deep enough that anything this archive manages to escape to
        // still lands inside the tempdir, and so is both detectable here and
        // cleaned up afterwards rather than left in the system temp root.
        let dest = dir.path().join("l1/l2/l3/l4/out");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(&zip_path, buf.into_inner()).unwrap();

        let result = extract(&zip_path, &dest, Some("top")).await;

        // Nothing may land outside the destination, whatever the outcome.
        let mut probe = dest.clone();
        for _ in 0..4 {
            probe.pop();
            assert!(
                !probe.join("ESCAPED").exists(),
                "payload escaped to {}",
                probe.join("ESCAPED").display()
            );
        }
        result.expect_err("an archive that escapes through a chained symlink must be refused");
    }

    /// The same divergence as the test above, one axis over: this time the
    /// planted link is walked through by the TARGET STRING rather than by the
    /// entry path.
    ///
    /// `chrome` resolves lexically to `chrome-linux64/u1/VICTIM` — pushed four,
    /// popped three, comfortably inside. The kernel instead follows `up` to
    /// `chrome-linux64` and *then* spends the three `..`, landing two levels
    /// above the destination. Canonicalizing the link's parent does not help
    /// here; the symlink is in the middle of the target.
    ///
    /// Nothing is written outside either way — a file entry through an escaping
    /// link is still refused — but the link itself is what `ensure_chrome`
    /// returns for the caller to *execute*, and phase 4 `chmod +x`'s whatever it
    /// points at.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlink_target_walking_through_an_earlier_symlink_cannot_escape() {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let opts = zip::write::SimpleFileOptions::default();
            let mut writer = zip::ZipWriter::new(&mut buf);
            writer
                .add_symlink("chrome-linux64/u1/u2/u3/up", "../../..", opts)
                .unwrap();
            writer
                .add_symlink("chrome-linux64/chrome", "u1/u2/u3/up/../../../VICTIM", opts)
                .unwrap();
            writer.finish().unwrap();
        }

        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let zip_path = root.join("evil.zip");
        let dest = root.join("l1/l2/out");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(&zip_path, buf.into_inner()).unwrap();
        // Where the kernel actually lands: `up` reaches `chrome-linux64`, and
        // the three `..` are spent from there — out, l2, l1.
        std::fs::write(root.join("l1/VICTIM"), b"not the browser").unwrap();

        let result = extract(&zip_path, &dest, Some("chrome-linux64")).await;

        // Whatever the outcome, the path a caller would launch must not resolve
        // out of the destination.
        let launched = dest.join("chrome-linux64/chrome");
        if let Ok(real) = std::fs::canonicalize(&launched) {
            assert!(
                real.starts_with(std::fs::canonicalize(&dest).unwrap()),
                "returned browser path resolves to {}, outside the cache",
                real.display()
            );
        }
        result.expect_err("a target walking through a planted symlink must be refused");
    }

    /// The containment predicate on its own, including the shapes that are
    /// easy to get wrong by one level. The directories are real, because
    /// resolution runs against the link's canonicalized parent.
    #[cfg(unix)]
    #[test]
    fn target_containment_rules() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(root.join("fw/Versions")).unwrap();
        std::fs::create_dir_all(root.join("a/b")).unwrap();

        let inside = |link: &str, target: &str| {
            resolve_target(&root.join(link), Path::new(target), &root).is_some()
        };

        // A target resolves relative to the link's PARENT, not the link.
        assert!(inside("fw/Versions/Current", "A"));
        assert!(inside("a/b/link", "../sibling"));
        assert!(inside("a/b/link", "./x"));
        assert!(inside("link", "x/y/z"));
        // Escapes.
        assert!(!inside("link", "/etc"));
        assert!(!inside("link", ".."));
        assert!(!inside("a/b/link", "../../../etc"));
        assert!(!inside("link", ""));
        // Refused even though it ENDS inside — the climb itself is the escape,
        // which is why containment is checked as we walk rather than at the end.
        assert!(!inside("a/link", "../../a/b"));
    }

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

    /// A symlink that stays inside dest_dir but leaves the archive's declared
    /// top-level directory is still refused — it can only point at something
    /// this archive does not own.
    #[cfg(unix)]
    #[tokio::test]
    async fn extract_rejects_symlink_leaving_the_top_level_dir() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("symlink.zip");
        let dest_dir = dir.path().join("out");
        std::fs::create_dir_all(&dest_dir).unwrap();

        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buf);
            writer
                .add_symlink(
                    "chrome-linux64/link",
                    "../escape",
                    zip::write::SimpleFileOptions::default(),
                )
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
}
