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

use std::path::Path;

use tokio::fs;

use crate::cookies::CookieJar;
use crate::error::Result;
use crate::io::write_atomic;

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
    /// On Unix an existing file keeps its permissions across saves, and a file
    /// created by this call is `0600`: it holds live session cookies, so the
    /// default is owner-only rather than whatever the process umask allows.
    /// Because the write finishes with a `rename`, a `path` that is a *symlink*
    /// is replaced by a regular file — the link target stops receiving saves.
    /// Pass the real path if you were relying on that indirection.
    ///
    /// # Errors
    ///
    /// Returns [`crate::ZendriverError::Io`] if the path is unwritable;
    /// [`crate::ZendriverError::Transport`] / `Cdp` on CDP failures.
    ///
    /// Because the save fills a sibling temp file and renames it over the
    /// destination, it needs more than a writable destination: the *parent
    /// directory* must be writable and executable, and the rename must be able
    /// to replace what is at `path`. A destination that cannot be replaced by a
    /// rename therefore fails even though it is perfectly writable — a
    /// bind-mounted single file (`docker run -v $PWD/cookies.json:/app/cookies.json`)
    /// is the common case, and a file held open by another process is the
    /// Windows one.
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
    /// rename write and its permission/symlink behavior — but applies the
    /// `filter` predicate to the result of
    /// [`CookieJar::all`] before writing, handy for persisting just one
    /// site's cookies out of a shared store. The predicate receives each
    /// [`crate::cookies::Cookie`] by reference and returns `true` to keep it.
    ///
    /// # Errors
    ///
    /// Returns [`crate::ZendriverError::Io`] if the path is unwritable;
    /// [`crate::ZendriverError::Transport`] / `Cdp` on CDP failures. The
    /// temp-file-and-rename requirements are [`Self::save_to_file`]'s, in
    /// full: a writable, executable parent directory, and a destination a
    /// rename can replace.
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
#[allow(clippy::unwrap_used)]
mod tests {
    use serde_json::json;
    use zendriver_transport::testing::MockConnection;

    use crate::cookies::{CookieJar, SameSite};
    use crate::error::ZendriverError;

    /// Count the entries in a directory — the cheap proxy for "the temp file
    /// was renamed, not left behind".
    fn dir_entry_count(dir: &std::path::Path) -> usize {
        std::fs::read_dir(dir).unwrap().count()
    }

    /// Mode of `path`, permission bits only. Unix-only helper for the
    /// permission tests below. (The helper's exhaustive coverage — the
    /// setuid/setgid carry-over, the mid-write window, the exclusive create —
    /// lives with the helper itself in [`crate::io`]; what is left here is the
    /// jar's own end-to-end behavior.)
    #[cfg(unix)]
    fn mode_of(path: &std::path::Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    /// Drive one save through the chosen public entry point, answering the
    /// `Storage.getCookies` round-trip with a single cookie, and return once
    /// the file is on disk.
    ///
    /// `matching` picks `save_to_file_matching` (with a keep-everything
    /// predicate) over `save_to_file`, so a caller that loops over both values
    /// asserts its property against *both* save paths — which is the whole
    /// point of the tests below, since the two call the write helper
    /// independently and only one of them might be changed.
    async fn save_one_cookie(path: &std::path::Path, matching: bool) {
        let (mut mock, conn) = MockConnection::pair();
        let jar = CookieJar::new(conn.clone());

        let save = tokio::spawn({
            let j = jar.clone();
            let p = path.to_path_buf();
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

        conn.shutdown();
    }

    /// Read back what [`save_one_cookie`] wrote.
    fn saved_cookies(path: &std::path::Path) -> Vec<crate::cookies::Cookie> {
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    /// A mode the user chose on the jar survives a save.
    ///
    /// `0640` rather than `0600` on purpose: it is both narrower than the
    /// `0644` a default umask produces *and* different from the `0600` the temp
    /// file is created with, so this is the fixture that notices if the
    /// destination-mode carry-over inside [`crate::io::write_atomic`] is
    /// dropped. A `0600` jar would come back `0600` either way and assert
    /// nothing.
    ///
    /// It says nothing about *which* write the jar used — `fs::write` on an
    /// existing inode preserves its mode too. The two tests that can tell the
    /// writes apart are [`both_save_paths_write_atomically`] and
    /// [`both_save_paths_replace_a_symlinked_destination`].
    #[cfg(unix)]
    #[tokio::test]
    async fn save_to_file_preserves_the_destination_mode() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cookies.json");
        std::fs::write(&path, b"stale").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();

        save_one_cookie(&path, false).await;

        let mode = mode_of(&path);
        assert_eq!(
            mode, 0o640,
            "a save must carry the destination's own mode over, got {mode:o}"
        );
        assert_eq!(dir_entry_count(dir.path()), 1, "no temp file may be left");
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

    /// Both save paths must reach the destination through
    /// [`crate::io::write_atomic`] rather than a plain `fs::write`, and the
    /// assertion that separates the two is the mode of a **jar that did not
    /// exist yet**: the helper creates its temp file `0600` whatever the
    /// ambient umask is, while `fs::write` creates at `0666 & ~umask`.
    ///
    /// Asserting on an *existing* destination cannot make that distinction —
    /// `O_WRONLY|O_CREAT|O_TRUNC` on a live inode ignores its mode argument, so
    /// both writes leave the previous permissions in place. That is why this
    /// test starts from an empty directory.
    ///
    /// One environment caveat on the discriminating power, the same one
    /// [`crate::io`]'s own mid-write test carries: under a `0077` umask or
    /// tighter `fs::write` also lands on `0600` and this proves nothing.
    /// [`both_save_paths_replace_a_symlinked_destination`] is the half that
    /// holds regardless of umask.
    ///
    /// The second save, over a longer stale file, then shows the rename
    /// replacing the content rather than overwriting a prefix of it, and the
    /// directory count shows the temp file was renamed rather than abandoned.
    #[tokio::test]
    async fn both_save_paths_write_atomically() {
        for matching in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("cookies.json");
            assert!(
                !path.exists(),
                "matching={matching}: the mode assertion below only means something \
                 for a destination this save creates"
            );

            save_one_cookie(&path, matching).await;

            #[cfg(unix)]
            {
                let mode = mode_of(&path);
                assert_eq!(
                    mode, 0o600,
                    "matching={matching}: a jar created by a save holds live session \
                     cookies and must be owner-only, got {mode:o}"
                );
            }
            assert_eq!(saved_cookies(&path).len(), 1, "matching={matching}");
            assert_eq!(
                dir_entry_count(dir.path()),
                1,
                "matching={matching}: save must leave no temp file behind"
            );

            std::fs::write(&path, b"stale-and-longer-than-the-new-content").unwrap();
            save_one_cookie(&path, matching).await;

            let parsed = saved_cookies(&path);
            assert_eq!(parsed.len(), 1, "matching={matching}");
            assert_eq!(parsed[0].name, "a", "matching={matching}");
            assert_eq!(
                dir_entry_count(dir.path()),
                1,
                "matching={matching}: the second save must leave no temp file either"
            );
        }
    }

    /// The umask-independent half: `fs::write` follows a symlinked destination
    /// and writes through to its target, while the atomic write finishes with a
    /// `rename` that unlinks the symlink and leaves a regular file in its
    /// place. No ambient setting changes that, so this separates the two writes
    /// on every box.
    ///
    /// It doubles as the pin for the behavior change `save_to_file`'s rustdoc
    /// warns about: a caller who symlinked their jar at some canonical store
    /// stops feeding that store.
    #[cfg(unix)]
    #[tokio::test]
    async fn both_save_paths_replace_a_symlinked_destination() {
        for matching in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let real = dir.path().join("real.json");
            let link = dir.path().join("cookies.json");
            std::fs::write(&real, b"original").unwrap();
            std::os::unix::fs::symlink(&real, &link).unwrap();

            save_one_cookie(&link, matching).await;

            assert!(
                !std::fs::symlink_metadata(&link).unwrap().is_symlink(),
                "matching={matching}: the rename must leave a regular file where the link was"
            );
            assert_eq!(saved_cookies(&link).len(), 1, "matching={matching}");
            assert_eq!(
                std::fs::read(&real).unwrap(),
                b"original",
                "matching={matching}: the link target must stop receiving saves"
            );
        }
    }
}
