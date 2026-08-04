//! JSON persistence for [`CookieJar`].
//!
//! Round-trips the entire browser cookie store through a file on disk so
//! callers can hydrate a fresh browser session with cookies captured from a
//! prior run — e.g. resume an authenticated scrape without re-running the
//! login flow.
//!
//! The on-disk shape is the pretty-printed [`Vec<Cookie>`] (snake_case JSON
//! per the module-level docs in [`crate::cookies`]) — straightforward to
//! diff, edit by hand, or feed to other tools.
//!
//! ## Lossless round-trip
//!
//! The `url` field on [`crate::cookies::Cookie`] is input-only: CDP uses
//! it on `.set()` to infer `domain` / `path` / `secure`, but never emits
//! it on reads. `.save_to_file()` therefore serializes whatever
//! `.all()` returned — `url` always `None`, omitted by serde — and
//! `.load_from_file()` reads back the same shape. `domain` / `path` /
//! `secure` are populated explicitly on every cookie from `.all()`, so
//! `.set_many()` after a load reconstructs the store without needing
//! `url` re-inference. If you hand-author a JSON file with a non-null
//! `url`, it round-trips faithfully too (serde preserves `Some` values).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::fs;

use crate::cookies::CookieJar;
use crate::error::Result;

/// Write `bytes` to `path` atomically: fill a uniquely-named sibling temp
/// file, then `rename` it over the destination.
///
/// A same-directory `rename` is atomic on every platform zendriver targets, so
/// a process killed mid-write leaves either the previous file or the complete
/// new one — never a truncated prefix. That matters most for the cookie jar: a
/// half-written `cookies.json` fails to parse on the next `load_from_file`,
/// losing exactly the authenticated session the file existed to preserve.
///
/// The temp name carries the process id plus a per-process counter, so two
/// writers targeting the same destination cannot fill each other's temp file.
/// A failure at either step removes the temp file rather than leaving litter
/// next to the destination.
///
/// Scope: this buys atomicity against a killed process, not fsync-level
/// durability against a power loss (neither the temp file nor the containing
/// directory is synced). Losing an unsynced write leaves the previous file,
/// which is the same outcome as never having called `save_to_file`.
///
/// Shared with the `tracker-blocking` blocklist cache
/// (`crate::tracker::load_or_download_blocklist`), which wants the same
/// guarantee; it lives here because this module is compiled unconditionally
/// while that one is feature-gated.
pub(crate) async fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = temp_path(path);
    if let Err(e) = fs::write(&tmp, bytes).await {
        let _ = fs::remove_file(&tmp).await;
        return Err(e);
    }
    if let Err(e) = fs::rename(&tmp, path).await {
        let _ = fs::remove_file(&tmp).await;
        return Err(e);
    }
    Ok(())
}

/// Sibling temp path for [`write_atomic`] — same directory, so the `rename`
/// stays on one filesystem (and is therefore atomic), and unique per process
/// and per call.
fn temp_path(path: &Path) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let mut name = path
        .file_name()
        .unwrap_or_else(|| OsStr::new("zendriver"))
        .to_os_string();
    name.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    path.with_file_name(name)
}

impl CookieJar {
    /// Snapshot the cookie store to a JSON file at `path`.
    ///
    /// Issues a single `Storage.getCookies` round-trip, then writes the
    /// pretty-printed array. The write is atomic — a sibling temp file is
    /// filled and renamed over the destination — so an interrupted save
    /// leaves the previous file intact instead of a truncated one that no
    /// longer parses. The file is overwritten if it already exists. Parent
    /// directories must already exist — `save_to_file` does not create them.
    ///
    /// # Errors
    ///
    /// Returns [`crate::ZendriverError::Io`] if the path is unwritable;
    /// [`crate::ZendriverError::Transport`] / `Cdp` on CDP failures.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// browser.cookies().save_to_file("cookies.json").await?;
    /// # Ok(()) }
    /// ```
    pub async fn save_to_file(&self, path: impl AsRef<Path>) -> Result<()> {
        let cookies = self.all().await?;
        let bytes = serde_json::to_vec_pretty(&cookies)?;
        write_atomic(path.as_ref(), &bytes).await?;
        Ok(())
    }

    /// Hydrate the browser cookie store from a JSON file at `path`.
    ///
    /// Reads the file, deserializes a `Vec<Cookie>`, and dispatches a
    /// single `Storage.setCookies` bulk-set. Existing cookies in the
    /// browser are NOT cleared first — call [`CookieJar::clear`] before
    /// this method for a fresh slate.
    ///
    /// # Errors
    ///
    /// Returns [`crate::ZendriverError::Io`] if the file is unreadable;
    /// [`crate::ZendriverError::Serde`] if the JSON is malformed.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// browser.cookies().load_from_file("cookies.json").await?;
    /// # Ok(()) }
    /// ```
    pub async fn load_from_file(&self, path: impl AsRef<Path>) -> Result<()> {
        let bytes = fs::read(path).await?;
        let cookies: Vec<crate::cookies::Cookie> = serde_json::from_slice(&bytes)?;
        self.set_many(cookies).await
    }

    /// Snapshot only the cookies matching `filter` to a JSON file at `path`.
    ///
    /// Like [`Self::save_to_file`] — including the atomic temp-file-then-
    /// rename write — but applies the `filter` predicate to the result of
    /// [`CookieJar::all`] before writing, handy for persisting just one
    /// site's cookies out of a shared store. The predicate receives each
    /// [`crate::cookies::Cookie`] by reference and returns `true` to keep it.
    ///
    /// # Errors
    ///
    /// Returns [`crate::ZendriverError::Io`] if the path is unwritable;
    /// [`crate::ZendriverError::Transport`] / `Cdp` on CDP failures.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// browser
    ///     .cookies()
    ///     .save_to_file_matching("example.json", |c| c.domain.contains("example.com"))
    ///     .await?;
    /// # Ok(()) }
    /// ```
    pub async fn save_to_file_matching(
        &self,
        path: impl AsRef<Path>,
        filter: impl Fn(&crate::cookies::Cookie) -> bool,
    ) -> Result<()> {
        let cookies: Vec<crate::cookies::Cookie> = self
            .all()
            .await?
            .into_iter()
            .filter(|c| filter(c))
            .collect();
        let bytes = serde_json::to_vec_pretty(&cookies)?;
        write_atomic(path.as_ref(), &bytes).await?;
        Ok(())
    }

    /// Hydrate only the cookies matching `filter` from a JSON file at `path`.
    ///
    /// Like [`Self::load_from_file`], but applies the `filter` predicate to
    /// the parsed `Vec<Cookie>` before the `Storage.setCookies` bulk-set —
    /// so a file holding many sites' cookies can be loaded selectively.
    /// Existing cookies are NOT cleared first; call [`CookieJar::clear`]
    /// beforehand for a fresh slate.
    ///
    /// # Errors
    ///
    /// Returns [`crate::ZendriverError::Io`] if the file is unreadable;
    /// [`crate::ZendriverError::Serde`] if the JSON is malformed.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// browser
    ///     .cookies()
    ///     .load_from_file_matching("cookies.json", |c| c.domain.contains("example.com"))
    ///     .await?;
    /// # Ok(()) }
    /// ```
    pub async fn load_from_file_matching(
        &self,
        path: impl AsRef<Path>,
        filter: impl Fn(&crate::cookies::Cookie) -> bool,
    ) -> Result<()> {
        let bytes = fs::read(path).await?;
        let cookies: Vec<crate::cookies::Cookie> = serde_json::from_slice(&bytes)?;
        let cookies: Vec<crate::cookies::Cookie> =
            cookies.into_iter().filter(|c| filter(c)).collect();
        self.set_many(cookies).await
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use serde_json::json;
    use zendriver_transport::testing::MockConnection;

    use super::write_atomic;
    use crate::cookies::{CookieJar, SameSite};
    use crate::error::ZendriverError;

    /// Count the entries in a directory — the cheap proxy for "the temp file
    /// was renamed, not left behind".
    fn dir_entry_count(dir: &std::path::Path) -> usize {
        std::fs::read_dir(dir).unwrap().count()
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
    /// simplest way to make `rename` fail after a successful temp write.
    #[tokio::test]
    async fn write_atomic_cleans_up_temp_file_when_rename_fails() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("occupied");
        std::fs::create_dir(&dest).unwrap();

        let err = write_atomic(&dest, b"fresh").await.unwrap_err();
        let _ = err;

        assert_eq!(
            dir_entry_count(dir.path()),
            1,
            "only the pre-existing directory should remain; the temp file must be removed"
        );
    }

    /// End-to-end round-trip: dump the cookie store to disk, then load it back
    /// into a fresh jar. The mock receives `Storage.getCookies` on save,
    /// then `Storage.setCookies` on load — assert the payload preserves both
    /// entries with their CDP camelCase fields intact.
    #[tokio::test]
    async fn save_and_load_roundtrip_preserves_cookies() {
        let (mut mock, conn) = MockConnection::pair();
        let jar = CookieJar::new(conn.clone());
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();

        // --- Save half: Storage.getCookies → write to tempfile.
        let save = tokio::spawn({
            let j = jar.clone();
            let p = path.clone();
            async move { j.save_to_file(p).await }
        });

        let id = mock.expect_cmd("Storage.getCookies").await;
        mock.reply(
            id,
            json!({
                "cookies": [
                    {
                        "name": "a",
                        "value": "1",
                        "domain": ".x.test",
                        "path": "/",
                        "expires": 1_700_000_000.0,
                        "httpOnly": true,
                        "secure": true,
                        "sameSite": "Lax",
                    },
                    {
                        "name": "b",
                        "value": "2",
                        "domain": "x.test",
                        "path": "/api",
                        "httpOnly": false,
                        "secure": false,
                    },
                ]
            }),
        )
        .await;
        save.await.unwrap().unwrap();

        // Sanity-check the on-disk shape — snake_case, two entries.
        let on_disk = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&on_disk).unwrap();
        let arr = parsed.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["name"], "a");
        assert_eq!(arr[0]["http_only"], true);
        assert_eq!(arr[0]["same_site"], "Lax");
        assert_eq!(arr[1]["name"], "b");

        // --- Load half: read tempfile → Storage.setCookies bulk-set.
        let load = tokio::spawn({
            let j = jar.clone();
            let p = path.clone();
            async move { j.load_from_file(p).await }
        });

        let id = mock.expect_cmd("Storage.setCookies").await;
        let params = &mock.last_sent()["params"];
        let cookies = params["cookies"].as_array().unwrap();
        assert_eq!(cookies.len(), 2);
        assert_eq!(cookies[0]["name"], "a");
        assert_eq!(cookies[0]["value"], "1");
        assert_eq!(cookies[0]["domain"], ".x.test");
        assert_eq!(cookies[0]["httpOnly"], true);
        assert_eq!(cookies[0]["sameSite"], "Lax");
        // No snake_case leakage on the wire.
        assert!(cookies[0].get("http_only").is_none());
        assert!(cookies[0].get("same_site").is_none());
        assert_eq!(cookies[1]["name"], "b");
        assert_eq!(cookies[1]["path"], "/api");

        mock.reply(id, json!({})).await;
        load.await.unwrap().unwrap();

        // SameSite preserved through the full round-trip.
        let reparsed: Vec<crate::cookies::Cookie> = serde_json::from_str(&on_disk).unwrap();
        assert_eq!(reparsed[0].same_site, Some(SameSite::Lax));
        assert_eq!(reparsed[1].same_site, None);

        conn.shutdown();
    }

    /// IO failures surface as [`ZendriverError::Io`] via the `From<io::Error>`
    /// impl on `ZendriverError` — writing into a nonexistent directory is the
    /// simplest reproducer.
    #[tokio::test]
    async fn save_errors_on_bad_path() {
        let (_mock, conn) = MockConnection::pair();
        let jar = CookieJar::new(conn.clone());

        // The error must come from the filesystem, not the cookie fetch —
        // shortcut the `all()` call by replying immediately on a background
        // task. (The pre-existing `_mock` would otherwise stall the call.)
        let reply = tokio::spawn(async move {
            let mut mock = _mock;
            let id = mock.expect_cmd("Storage.getCookies").await;
            mock.reply(id, json!({ "cookies": [] })).await;
        });

        let err = jar
            .save_to_file("/nonexistent_dir_xyz_123/file.json")
            .await
            .unwrap_err();
        assert!(
            matches!(err, ZendriverError::Io(_)),
            "expected Io, got {err:?}"
        );

        reply.await.unwrap();
        conn.shutdown();
    }

    /// `save_to_file_matching` filters the result of `all()` before writing —
    /// a jar reporting two cookies plus a predicate that keeps one yields a
    /// single-entry on-disk file.
    #[tokio::test]
    async fn save_to_file_matching_writes_only_matching() {
        let (mut mock, conn) = MockConnection::pair();
        let jar = CookieJar::new(conn.clone());
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();

        let save = tokio::spawn({
            let j = jar.clone();
            let p = path.clone();
            // Keep only the keep.test cookie.
            async move {
                j.save_to_file_matching(p, |c| c.domain.contains("keep.test"))
                    .await
            }
        });

        let id = mock.expect_cmd("Storage.getCookies").await;
        mock.reply(
            id,
            json!({
                "cookies": [
                    { "name": "a", "value": "1", "domain": ".keep.test", "path": "/",
                      "httpOnly": false, "secure": false },
                    { "name": "b", "value": "2", "domain": ".drop.test", "path": "/",
                      "httpOnly": false, "secure": false },
                ]
            }),
        )
        .await;
        save.await.unwrap().unwrap();

        let on_disk = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&on_disk).unwrap();
        let arr = parsed.as_array().unwrap();
        assert_eq!(arr.len(), 1, "only the matching cookie should be written");
        assert_eq!(arr[0]["name"], "a");
        assert_eq!(arr[0]["domain"], ".keep.test");

        conn.shutdown();
    }

    /// `load_from_file_matching` filters the parsed `Vec<Cookie>` before the
    /// `Storage.setCookies` bulk-set — a two-entry file plus a predicate that
    /// keeps one results in a single-cookie wire payload.
    #[tokio::test]
    async fn load_from_file_matching_filters_before_set() {
        let (mut mock, conn) = MockConnection::pair();
        let jar = CookieJar::new(conn.clone());
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();

        // Author a two-cookie file (public snake_case shape).
        std::fs::write(
            &path,
            json!([
                { "name": "a", "value": "1", "domain": ".keep.test", "path": "/" },
                { "name": "b", "value": "2", "domain": ".drop.test", "path": "/" },
            ])
            .to_string(),
        )
        .unwrap();

        let load = tokio::spawn({
            let j = jar.clone();
            let p = path.clone();
            async move {
                j.load_from_file_matching(p, |c| c.domain.contains("keep.test"))
                    .await
            }
        });

        let id = mock.expect_cmd("Storage.setCookies").await;
        let cookies = mock.last_sent()["params"]["cookies"].as_array().unwrap();
        assert_eq!(cookies.len(), 1, "only the matching cookie should be set");
        assert_eq!(cookies[0]["name"], "a");
        assert_eq!(cookies[0]["domain"], ".keep.test");

        mock.reply(id, json!({})).await;
        load.await.unwrap().unwrap();

        conn.shutdown();
    }

    /// Both save paths must go through the atomic helper: the destination
    /// ends up holding the full pretty-printed JSON, and no `.tmp` sibling is
    /// left in the directory. Writing over an existing file also proves the
    /// rename replaces stale content rather than appending to it.
    #[tokio::test]
    async fn both_save_paths_write_atomically() {
        for matching in [false, true] {
            let (mut mock, conn) = MockConnection::pair();
            let jar = CookieJar::new(conn.clone());
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("cookies.json");
            std::fs::write(&path, b"stale-and-longer-than-the-new-content").unwrap();

            let save = tokio::spawn({
                let j = jar.clone();
                let p = path.clone();
                async move {
                    if matching {
                        j.save_to_file_matching(p, |_| true).await
                    } else {
                        j.save_to_file(p).await
                    }
                }
            });

            let id = mock.expect_cmd("Storage.getCookies").await;
            mock.reply(
                id,
                json!({
                    "cookies": [
                        { "name": "a", "value": "1", "domain": ".x.test", "path": "/",
                          "httpOnly": false, "secure": false },
                    ]
                }),
            )
            .await;
            save.await.unwrap().unwrap();

            let parsed: Vec<crate::cookies::Cookie> =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(parsed.len(), 1, "matching={matching}");
            assert_eq!(parsed[0].name, "a");
            assert_eq!(
                dir_entry_count(dir.path()),
                1,
                "matching={matching}: save must leave no temp file behind"
            );

            conn.shutdown();
        }
    }
}
