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

use std::path::Path;

use tokio::fs;

use crate::cookies::CookieJar;
use crate::error::Result;

impl CookieJar {
    /// Snapshot the cookie store to a JSON file at `path`.
    ///
    /// Issues a single `Network.getAllCookies` round-trip, then writes the
    /// pretty-printed array via [`tokio::fs::write`]. Bubbles up any
    /// transport / CDP / IO error unchanged.
    ///
    /// The file is overwritten if it already exists. Parent directories must
    /// already exist — `save_to_file` does not create them (matches
    /// `tokio::fs::write` semantics; an `Err(ZendriverError::Io(_))` is
    /// returned if the path is unwritable).
    pub async fn save_to_file(&self, path: impl AsRef<Path>) -> Result<()> {
        let cookies = self.all().await?;
        let bytes = serde_json::to_vec_pretty(&cookies)?;
        fs::write(path, bytes).await?;
        Ok(())
    }

    /// Hydrate the browser cookie store from a JSON file at `path`.
    ///
    /// Reads the file, deserializes a [`Vec<Cookie>`], and dispatches a
    /// single `Network.setCookies` bulk-set (one CDP round-trip regardless
    /// of cookie count). Existing cookies in the browser are not cleared
    /// first — callers that want a fresh slate should call
    /// [`CookieJar::clear`] before `load_from_file`.
    pub async fn load_from_file(&self, path: impl AsRef<Path>) -> Result<()> {
        let bytes = fs::read(path).await?;
        let cookies: Vec<crate::cookies::Cookie> = serde_json::from_slice(&bytes)?;
        self.set_many(cookies).await
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use serde_json::json;
    use zendriver_transport::testing::MockConnection;

    use crate::cookies::{CookieJar, SameSite};
    use crate::error::ZendriverError;

    /// End-to-end round-trip: dump the cookie store to disk, then load it back
    /// into a fresh jar. The mock receives `Network.getAllCookies` on save,
    /// then `Network.setCookies` on load — assert the payload preserves both
    /// entries with their CDP camelCase fields intact.
    #[tokio::test]
    async fn save_and_load_roundtrip_preserves_cookies() {
        let (mut mock, conn) = MockConnection::pair();
        let jar = CookieJar::new(conn.clone());
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();

        // --- Save half: Network.getAllCookies → write to tempfile.
        let save = tokio::spawn({
            let j = jar.clone();
            let p = path.clone();
            async move { j.save_to_file(p).await }
        });

        let id = mock.expect_cmd("Network.getAllCookies").await;
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

        // --- Load half: read tempfile → Network.setCookies bulk-set.
        let load = tokio::spawn({
            let j = jar.clone();
            let p = path.clone();
            async move { j.load_from_file(p).await }
        });

        let id = mock.expect_cmd("Network.setCookies").await;
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
            let id = mock.expect_cmd("Network.getAllCookies").await;
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
}
