//! Filesystem primitives shared across the crate.
//!
//! One thing lives here today: [`write_atomic`], the temp-file-then-rename
//! write used by the cookie jar ([`crate::cookies::persistence`]) and by the
//! `tracker-blocking` blocklist cache (`crate::tracker`). Neither of those
//! owns it, so it sits at the crate root instead of inside whichever module
//! happened to need it first.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::fs;
use tokio::io::AsyncWriteExt;

/// Write `bytes` to `path` atomically: fill a uniquely-named sibling temp
/// file, then `rename` it over the destination.
///
/// On Unix a same-directory `rename` is atomic, so a process killed mid-write
/// leaves either the previous file or the complete new one — never a truncated
/// prefix. That matters most for the cookie jar: a half-written `cookies.json`
/// fails to parse on the next `load_from_file`, losing exactly the
/// authenticated session the file existed to preserve. Windows offers no
/// documented atomic replace here, and the rename additionally fails outright
/// when the destination is held open by another process without
/// `FILE_SHARE_DELETE`, or is marked read-only.
///
/// The temp name carries the process id, a per-process counter and the
/// sub-second clock, and the file is created with `O_EXCL`. So two writers
/// targeting the same destination cannot fill each other's temp file, nothing
/// pre-planted at the path is followed or truncated, and a stray file left
/// behind by a killed process would have to match the pid, the counter *and*
/// the nanosecond to get in the way. Once the temp file exists, a failure at
/// any later step removes it rather than leaving litter next to the
/// destination.
///
/// Scope: this buys atomicity against a killed process, not fsync-level
/// durability against a power loss (neither the temp file nor the containing
/// directory is synced). Losing an unsynced write leaves the previous file,
/// which is the same outcome as never having called `save_to_file`.
///
/// # Permissions and symlinks
///
/// The rename installs the *temp file's* inode at `path`, so the destination
/// afterwards is a different file than it was before. Two consequences that a
/// plain `fs::write` does not have:
///
/// - **The mode comes from the temp file.** It is created `0600`, before the
///   first payload byte reaches the disk, rather than created at the process
///   umask and narrowed afterwards: Unix checks permissions at `open()`, so
///   another local uid that opened the file during a wide window would keep
///   reading through that descriptor however the mode ended up.
///   [`apply_destination_mode`] then stamps an *existing* destination's mode
///   onto it, so a jar the user deliberately widened stays wide and one they
///   deliberately narrowed stays narrow; a destination that does not exist yet
///   keeps the `0600` it was created with. No-op on non-Unix targets, which
///   have no mode to carry over.
/// - **A symlinked destination is replaced, not followed.** If `path` is a
///   symlink, the rename unlinks it and leaves a regular file in its place —
///   the link target keeps its old contents and stops receiving writes.
///   `fs::write` followed the link and wrote through to the target, so a
///   caller who symlinked their cookie jar at some canonical store must point
///   zendriver at the real path instead. (The mode is still read through the
///   link, so the replacement inherits the target's permissions.)
pub(crate) async fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = temp_path(path)?;

    // Deliberately outside the cleanup below. A failed `create_new` means the
    // path was already occupied by something this call did not create, and
    // removing that something would be destroying a stranger's file.
    let file = create_temp_file(&tmp).await?;

    match fill_then_rename(file, &tmp, path, bytes).await {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp).await;
            Err(e)
        }
    }
}

/// The fallible middle of [`write_atomic`], split out so the temp-file cleanup
/// is written once rather than once per step that can fail.
async fn fill_then_rename(
    mut file: fs::File,
    tmp: &Path,
    dest: &Path,
    bytes: &[u8],
) -> std::io::Result<()> {
    file.write_all(bytes).await?;
    // `tokio::fs::File` buffers and its `Drop` does not wait for the in-flight
    // write, so the rename could otherwise publish a short file.
    file.flush().await?;
    drop(file);
    apply_destination_mode(tmp, dest).await?;
    fs::rename(tmp, dest).await
}

/// Mode the temp file is created with, before any payload byte exists on disk.
///
/// `0600` rather than the process umask because the caller that matters here —
/// [`crate::cookies::CookieJar::save_to_file`] — writes authentication
/// material: session cookies equivalent to a password for as long as they
/// live. A user who genuinely wants the file shared can widen the destination
/// once, and [`apply_destination_mode`] preserves that choice on every later
/// save.
#[cfg(unix)]
const TEMP_FILE_MODE: u32 = 0o600;

/// Create the temp file, refusing to touch a path that already exists.
///
/// `create_new` brings `O_EXCL`, which is what makes a temp name derived from
/// the destination safe: without it the open follows a symlink planted at that
/// path and writes the payload through to a target of someone else's choosing,
/// with this process's privileges.
#[cfg(unix)]
async fn create_temp_file(tmp: &Path) -> std::io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(TEMP_FILE_MODE)
        .open(tmp)
        .await
}

/// Non-Unix counterpart. `create_new` still applies — it is what refuses a
/// pre-planted path — but there is no mode to request.
#[cfg(not(unix))]
async fn create_temp_file(tmp: &Path) -> std::io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(tmp)
        .await
}

/// Give `tmp` the permissions `dest` should still have once the rename has
/// swapped one for the other.
///
/// An existing destination's mode wins outright: preserving what the user (or
/// their prior save) chose is the only behavior that keeps `write_atomic`
/// indistinguishable from the `fs::write` it replaced. With no destination to
/// inherit from there is nothing to do — the temp file already carries
/// [`TEMP_FILE_MODE`], which is the default this module wants.
#[cfg(unix)]
async fn apply_destination_mode(tmp: &Path, dest: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mode = match fs::metadata(dest).await {
        // Mask off the file-type bits; keep the full permission set (including
        // setuid/setgid/sticky) so nothing the user set is quietly dropped.
        // Carrying them over is best-effort rather than a guarantee: the
        // kernel clears `S_ISGID` on a `chmod` by a caller who is not in the
        // file's group, and a `nosuid` mount ignores the bits it stores.
        Ok(meta) => meta.permissions().mode() & 0o7777,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    fs::set_permissions(tmp, std::fs::Permissions::from_mode(mode)).await
}

/// No-op counterpart for targets without Unix mode bits.
///
/// Windows keeps a DACL on every file rather than on the directory alone, and
/// the rename installs a *new* inode at the destination. What the replacement
/// picks up is the set of inheritable ACEs the containing directory seeds into
/// files created in it — the same set the previous file got, for the default
/// case. What it does not pick up is an explicit DACL somebody set on the
/// destination itself (`icacls cookies.json /inheritance:r`); preserving that
/// would need `GetNamedSecurityInfo` / `SetNamedSecurityInfo` or
/// `ReplaceFileW`, neither of which is wired up.
#[cfg(not(unix))]
#[allow(clippy::unused_async)]
async fn apply_destination_mode(_tmp: &Path, _dest: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Sibling temp path for [`write_atomic`] — same directory, so the `rename`
/// stays on one filesystem (and is therefore atomic).
///
/// The suffix combines the process id, a per-process counter and the
/// sub-second clock. The counter separates concurrent writers inside one
/// process; the clock separates a run from a stray temp file an earlier,
/// SIGKILL'd run left behind, which matters because the exclusive create in
/// [`create_temp_file`] turns a collision into a hard error rather than a
/// silent overwrite.
///
/// Fails when `path` names no file — a trailing `/`, `.` or `..`, all of which
/// are directories that `write_atomic` could never have written to anyway.
/// Inventing a filename for those would put a stray file somewhere the caller
/// never named on the way to the failure it was always going to get.
fn temp_path(path: &Path) -> std::io::Result<PathBuf> {
    static SEQ: AtomicU64 = AtomicU64::new(0);

    let mut name = path
        .file_name()
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{} does not name a file to write", path.display()),
            )
        })?
        .to_os_string();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    name.push(format!(
        ".{}.{}.{nanos}.tmp",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    Ok(path.with_file_name(name))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::{temp_path, write_atomic};
    // Only the symlink-refusal test calls this directly, and that test is
    // `#[cfg(unix)]` — an unconditional import is an unused-import error on
    // Windows, where `-D warnings` makes it a build failure.
    #[cfg(unix)]
    use super::create_temp_file;

    /// Count the entries in a directory — the cheap proxy for "the temp file
    /// was renamed, not left behind".
    fn dir_entry_count(dir: &std::path::Path) -> usize {
        std::fs::read_dir(dir).unwrap().count()
    }

    /// Full mode of `path`, including the setuid/setgid/sticky bits the
    /// carry-over mask exists to preserve. Unix-only helper for the permission
    /// tests below.
    #[cfg(unix)]
    fn mode_of(path: &std::path::Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o7777
    }

    /// The atomic write replaces existing content in place and leaves no
    /// `.tmp` sibling behind: after the call the directory holds exactly the
    /// destination file, carrying the new bytes.
    #[tokio::test]
    async fn write_atomic_replaces_existing_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("cookies.json");
        std::fs::write(&dest, b"stale").unwrap();

        write_atomic(&dest, b"fresh").await.unwrap();

        assert_eq!(std::fs::read(&dest).unwrap(), b"fresh");
        assert_eq!(
            dir_entry_count(dir.path()),
            1,
            "temp file must be renamed away, not left next to the destination"
        );
    }

    /// When the `rename` step fails, the error surfaces AND the temp file is
    /// cleaned up. A destination that is an existing directory is the
    /// simplest way to make `rename` fail after a successful temp write — and
    /// the assertion names that step rather than settling for "some error",
    /// so the test cannot quietly start passing because an earlier step began
    /// failing instead.
    #[tokio::test]
    async fn write_atomic_cleans_up_temp_file_when_rename_fails() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("occupied");
        std::fs::create_dir(&dest).unwrap();

        let err = write_atomic(&dest, b"fresh").await.unwrap_err();

        // Naming the step the test is about: `rename` onto a directory is
        // `EISDIR` on Linux, `ENOTDIR` on macOS/BSD and access-denied on
        // Windows. The three earlier steps fail differently — `AlreadyExists`
        // from the exclusive create, `NotFound` from the metadata read — so
        // this cannot quietly start passing on a different failure.
        #[cfg(unix)]
        let expected: &[std::io::ErrorKind] = &[
            std::io::ErrorKind::IsADirectory,
            std::io::ErrorKind::NotADirectory,
        ];
        #[cfg(not(unix))]
        let expected: &[std::io::ErrorKind] = &[std::io::ErrorKind::PermissionDenied];

        assert!(
            expected.contains(&err.kind()),
            "expected the rename step to fail with one of {expected:?}, got {err:?}"
        );
        assert_eq!(
            dir_entry_count(dir.path()),
            1,
            "only the pre-existing directory should remain; the temp file must be removed"
        );
    }

    /// A jar deliberately created `chmod 600` must still be `0600` after a
    /// save. The rename installs the temp file's inode, so without an explicit
    /// carry-over the mode collapses to the process umask (0644 on a stock
    /// box) and a file full of live session cookies goes world-readable.
    #[cfg(unix)]
    #[tokio::test]
    async fn write_atomic_preserves_an_existing_restrictive_mode() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("cookies.json");
        std::fs::write(&dest, b"stale").unwrap();
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o600)).unwrap();

        write_atomic(&dest, b"fresh").await.unwrap();

        let mode = mode_of(&dest);
        assert_eq!(
            mode, 0o600,
            "a 0600 cookie jar must not widen across a save, got {mode:o}"
        );
        assert_eq!(std::fs::read(&dest).unwrap(), b"fresh");
        assert_eq!(dir_entry_count(dir.path()), 1, "no temp file may be left");
    }

    /// A mode the user widened on purpose is preserved too — the rule is
    /// "carry the destination's mode", not "force 0600 on everything".
    #[cfg(unix)]
    #[tokio::test]
    async fn write_atomic_preserves_an_existing_permissive_mode() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("trackers.txt");
        std::fs::write(&dest, b"stale").unwrap();
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o644)).unwrap();

        write_atomic(&dest, b"fresh").await.unwrap();

        let mode = mode_of(&dest);
        assert_eq!(mode, 0o644, "a widened mode must survive too, got {mode:o}");
    }

    /// The carry-over keeps the whole permission set, not just `rwx`. Setgid
    /// is the bit the `& 0o7777` mask exists for, and nothing else in the
    /// suite exercises it — narrow the mask to `0o777` and this is the only
    /// test that notices.
    #[cfg(unix)]
    #[tokio::test]
    async fn write_atomic_preserves_bits_outside_the_rwx_triplets() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("cookies.json");
        std::fs::write(&dest, b"stale").unwrap();
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o2600)).unwrap();
        assert_eq!(
            mode_of(&dest),
            0o2600,
            "precondition: this box lets a non-privileged chmod set setgid"
        );

        write_atomic(&dest, b"fresh").await.unwrap();

        let mode = mode_of(&dest);
        assert_eq!(mode, 0o2600, "setgid must survive the save, got {mode:o}");
    }

    /// With no destination to inherit from, the new file is owner-only rather
    /// than whatever the ambient umask would have allowed.
    #[cfg(unix)]
    #[tokio::test]
    async fn write_atomic_creates_a_new_file_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("cookies.json");
        assert!(!dest.exists());

        write_atomic(&dest, b"fresh").await.unwrap();

        let mode = mode_of(&dest);
        assert_eq!(
            mode, 0o600,
            "a freshly created cookie jar must default to owner-only, got {mode:o}"
        );
        assert_eq!(std::fs::read(&dest).unwrap(), b"fresh");
        assert_eq!(dir_entry_count(dir.path()), 1, "no temp file may be left");
    }

    /// The temp file must be *created* owner-only, not created at the process
    /// umask and narrowed once the payload is already on disk: Unix checks
    /// permissions at `open()`, so another local uid that opened the file
    /// during a wide window keeps reading through that descriptor no matter
    /// what the mode becomes afterwards.
    ///
    /// Samples the `.tmp` sibling *while the write is in flight*. Asserting
    /// only on the destination after the rename proves nothing here — the
    /// mode is corrected before the rename either way, so that assertion
    /// passes against the broken shape too.
    ///
    /// One environment caveat on the non-vacuity: the broken shape creates at
    /// `0666 & ~umask`, which coincides with `0600` on a box whose umask is
    /// `0077` or tighter. Under the `0022` every CI runner and default shell
    /// uses, it is `0644` and this fails.
    #[cfg(unix)]
    #[tokio::test]
    async fn temp_file_is_owner_only_for_the_whole_write() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("cookies.json");

        // Big enough that `tokio::fs` splits the write across several blocking
        // hops, each of which parks the writer and lets the sampler run.
        let payload = vec![b'c'; 2 * 1024 * 1024];
        let writer = {
            let dest = dest.clone();
            tokio::spawn(async move { write_atomic(&dest, &payload).await })
        };

        let mut samples: Vec<u32> = Vec::new();
        while !writer.is_finished() {
            for entry in std::fs::read_dir(dir.path()).unwrap().flatten() {
                let path = entry.path();
                if path.extension().and_then(std::ffi::OsStr::to_str) == Some("tmp") {
                    if let Ok(meta) = entry.metadata() {
                        use std::os::unix::fs::PermissionsExt;
                        samples.push(meta.permissions().mode() & 0o7777);
                    }
                }
            }
            tokio::task::yield_now().await;
        }
        writer.await.unwrap().unwrap();

        assert!(
            !samples.is_empty(),
            "the sampler never caught the temp file, so this test proved nothing"
        );
        let octal: Vec<String> = samples.iter().map(|m| format!("{m:o}")).collect();
        assert!(
            samples.iter().all(|m| *m == 0o600),
            "the temp file holds the full payload and must be owner-only for every \
             instant it exists; observed modes {octal:?} across {} samples",
            samples.len()
        );
    }

    /// The temp name is derived from the destination, so the create must be
    /// exclusive. Plant a symlink where the open is about to land: it has to
    /// be refused, not followed through to the decoy.
    #[cfg(unix)]
    #[tokio::test]
    async fn create_temp_file_refuses_a_planted_path_instead_of_following_it() {
        let dir = tempfile::tempdir().unwrap();
        let decoy = dir.path().join("decoy");
        std::fs::write(&decoy, b"do-not-touch").unwrap();
        let tmp = dir.path().join("cookies.json.4242.0.tmp");
        std::os::unix::fs::symlink(&decoy, &tmp).unwrap();

        let err = create_temp_file(&tmp).await.unwrap_err();

        assert_eq!(
            err.kind(),
            std::io::ErrorKind::AlreadyExists,
            "an occupied temp path must fail the open, got {err:?}"
        );
        assert_eq!(
            std::fs::read(&decoy).unwrap(),
            b"do-not-touch",
            "the symlink target must not be written through or truncated"
        );
    }

    /// Pins the behavior the rustdoc warns about: because the write ends in a
    /// `rename`, a symlinked destination is *replaced* by a regular file
    /// instead of written through. If this ever changes, the docs on
    /// `write_atomic` and `save_to_file` change with it.
    #[cfg(unix)]
    #[tokio::test]
    async fn write_atomic_replaces_a_symlinked_destination() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.json");
        let link = dir.path().join("cookies.json");
        std::fs::write(&real, b"original").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();

        write_atomic(&link, b"fresh").await.unwrap();

        assert!(
            !std::fs::symlink_metadata(&link).unwrap().is_symlink(),
            "the rename unlinks the symlink and leaves a regular file"
        );
        assert_eq!(std::fs::read(&link).unwrap(), b"fresh");
        assert_eq!(
            std::fs::read(&real).unwrap(),
            b"original",
            "the link target stops receiving writes"
        );
        // The mode is still read through the link, so the replacement keeps
        // the target's restrictive permissions.
        let mode = mode_of(&link);
        assert_eq!(
            mode, 0o600,
            "the replacement must inherit the link target's mode, got {mode:o}"
        );
    }

    /// A path that names no file is rejected up front instead of being handed
    /// a made-up filename the caller never asked for.
    #[test]
    fn temp_path_rejects_a_path_with_no_file_name() {
        for input in ["", "/", "/tmp/.."] {
            let err = temp_path(std::path::Path::new(input)).unwrap_err();
            assert_eq!(
                err.kind(),
                std::io::ErrorKind::InvalidInput,
                "{input} names no file and must be rejected, got {err:?}"
            );
        }
    }

    /// Two calls for the same destination never collide, so concurrent writers
    /// cannot fill each other's temp file.
    #[test]
    fn temp_path_is_unique_per_call() {
        let dest = std::path::Path::new("/tmp/cookies.json");
        let a = temp_path(dest).unwrap();
        let b = temp_path(dest).unwrap();
        assert_ne!(a, b);
        assert_eq!(a.parent(), dest.parent());
    }
}
