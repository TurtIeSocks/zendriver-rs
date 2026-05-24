//! Per-Tab DOM storage handle (localStorage / sessionStorage) backed by CDP
//! `DOMStorage.*` methods.
//!
//! Each [`Storage`] handle is configured at construction with an `is_local`
//! flag selecting either localStorage (true) or sessionStorage (false) for
//! the owning [`crate::Tab`]'s current origin. Obtain handles via
//! [`crate::Tab::local_storage`] / [`crate::Tab::session_storage`].
//!
//! ```no_run
//! # async fn ex() -> zendriver::Result<()> {
//! # let browser = zendriver::Browser::builder().launch().await?;
//! # let tab = browser.main_tab();
//! tab.goto("https://example.com").await?;
//! let ls = tab.local_storage();
//! ls.set("theme", "dark").await?;
//! assert_eq!(ls.get("theme").await?.as_deref(), Some("dark"));
//! # Ok(()) }
//! ```
//!
//! ## Why fetch the URL on every call
//!
//! CDP's `DOMStorage` domain identifies a storage area by `StorageId {
//! securityOrigin, isLocalStorage }`. The origin must reflect the tab's
//! *current* document — a navigation between calls would otherwise read
//! storage from the wrong origin. Rather than cache + invalidate on
//! `Page.frameNavigated`, the handle re-fetches the URL via
//! [`crate::tab::Tab::url`] (one cheap `Target.getTargetInfo` round-trip)
//! on each operation. Storage ops are infrequent enough that the extra
//! round-trip is preferable to the cache-coherency footgun.
//!
//! ## `DOMStorage.enable` once-per-handle
//!
//! Chrome rejects `DOMStorage` reads/writes until the domain is enabled on
//! the session. Each [`Storage`] handle gates the first call through a
//! [`tokio::sync::OnceCell`] so the enable round-trip fires exactly once
//! per handle — subsequent calls skip straight to the data op. The same
//! storage handle re-used across many calls thus pays the enable cost
//! exactly once.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use serde_json::{Value, json};
use tokio::sync::OnceCell;
use zendriver_transport::SessionHandle;

use crate::error::{Result, ZendriverError};
use crate::tab::TabInner;

/// Cheap-to-clone handle to a single DOM storage area.
///
/// localStorage or sessionStorage for the owning tab's current origin.
/// Construct via [`crate::Tab::local_storage`] /
/// [`crate::Tab::session_storage`]. Wrapping is `Arc`-cheap; pass the
/// handle around freely. All methods are async — each call performs at
/// least one CDP round-trip (origin discovery), plus the data op.
#[derive(Clone, Debug)]
pub struct Storage {
    inner: Arc<StorageInner>,
}

#[derive(Debug)]
struct StorageInner {
    /// Tab session used for `DOMStorage.*` calls. For both local and session
    /// storage this is the tab's primary session — DOMStorage is page-scoped,
    /// not per-frame.
    session: SessionHandle,
    /// `true` selects localStorage; `false` selects sessionStorage. Encoded
    /// into every CDP `storageId` payload.
    is_local: bool,
    /// Weak ref to the owning tab so we can call [`crate::tab::Tab::url`]
    /// on each op without forcing the tab to outlive the handle. If the
    /// tab has been dropped, storage ops error with
    /// [`ZendriverError::Storage`] rather than panic.
    tab: Weak<TabInner>,
    /// Gates the once-per-handle `DOMStorage.enable` call. Chrome rejects
    /// `getDOMStorageItems` / `setDOMStorageItem` / etc. until the domain
    /// is enabled on the session; we lazily enable on first use and cache
    /// the success so subsequent calls skip the round-trip.
    enable_once: OnceCell<()>,
}

impl Storage {
    /// Construct a storage handle. Typically called by
    /// [`crate::tab::Tab::local_storage`] / `session_storage` rather than
    /// user code — those carry the right [`SessionHandle`] +
    /// [`Weak<TabInner>`] for the owning tab.
    #[must_use]
    pub(crate) fn new(session: SessionHandle, is_local: bool, tab: Weak<TabInner>) -> Self {
        Self {
            inner: Arc::new(StorageInner {
                session,
                is_local,
                tab,
                enable_once: OnceCell::new(),
            }),
        }
    }

    /// Whether this handle targets `localStorage` (true) or
    /// `sessionStorage` (false).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// assert!(tab.local_storage().is_local());
    /// assert!(!tab.session_storage().is_local());
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn is_local(&self) -> bool {
        self.inner.is_local
    }

    /// Look up a single value by key. Returns `None` if the key is absent.
    ///
    /// Fetches the full storage area (CDP has no single-key getter) and
    /// scans for the match.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// if let Some(v) = tab.local_storage().get("theme").await? {
    ///     println!("theme: {v}");
    /// }
    /// # Ok(()) }
    /// ```
    pub async fn get(&self, key: &str) -> Result<Option<String>> {
        let all = self.get_all().await?;
        Ok(all.get(key).cloned())
    }

    /// Snapshot the entire storage area as a `HashMap`.
    ///
    /// Maps to `DOMStorage.getDOMStorageItems`. Order is unspecified.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// for (k, v) in tab.local_storage().get_all().await? {
    ///     println!("{k}={v}");
    /// }
    /// # Ok(()) }
    /// ```
    pub async fn get_all(&self) -> Result<HashMap<String, String>> {
        self.ensure_enabled().await?;
        // Hold a strong ref to the Tab across the full op so the session
        // can't be torn down between the `storage_id` URL fetch and the
        // `DOMStorage.*` call.
        let tab = self.upgraded_tab()?;
        let storage_id = self.build_storage_id(&tab).await?;
        let resp = self
            .inner
            .session
            .call(
                "DOMStorage.getDOMStorageItems",
                json!({ "storageId": storage_id }),
            )
            .await?;
        drop(tab);
        parse_entries(&resp)
    }

    /// Insert or replace a single key-value pair.
    ///
    /// Maps to `DOMStorage.setDOMStorageItem` — Chrome treats the value as
    /// opaque text (per the Storage API spec; non-string values must be
    /// stringified by the caller).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.local_storage().set("theme", "dark").await?;
    /// # Ok(()) }
    /// ```
    pub async fn set(&self, key: &str, value: &str) -> Result<()> {
        self.ensure_enabled().await?;
        let tab = self.upgraded_tab()?;
        let storage_id = self.build_storage_id(&tab).await?;
        self.inner
            .session
            .call(
                "DOMStorage.setDOMStorageItem",
                json!({
                    "storageId": storage_id,
                    "key": key,
                    "value": value,
                }),
            )
            .await?;
        drop(tab);
        Ok(())
    }

    /// Set many entries.
    ///
    /// Convenience wrapper over [`Self::set`] — there is no CDP bulk-set for
    /// DOMStorage, so this issues N round-trips. Order of dispatch follows
    /// the `HashMap`'s iteration order (unspecified).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::collections::HashMap;
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let mut items = HashMap::new();
    /// items.insert("theme".to_string(), "dark".to_string());
    /// items.insert("lang".to_string(), "en".to_string());
    /// tab.local_storage().set_many(&items).await?;
    /// # Ok(()) }
    /// ```
    pub async fn set_many(&self, items: &HashMap<String, String>) -> Result<()> {
        for (k, v) in items {
            self.set(k, v).await?;
        }
        Ok(())
    }

    /// Remove a single key.
    ///
    /// Maps to `DOMStorage.removeDOMStorageItem`. Missing keys are silently
    /// ignored (matches the Storage API `removeItem` semantics).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.local_storage().remove("theme").await?;
    /// # Ok(()) }
    /// ```
    pub async fn remove(&self, key: &str) -> Result<()> {
        self.ensure_enabled().await?;
        let tab = self.upgraded_tab()?;
        let storage_id = self.build_storage_id(&tab).await?;
        self.inner
            .session
            .call(
                "DOMStorage.removeDOMStorageItem",
                json!({ "storageId": storage_id, "key": key }),
            )
            .await?;
        drop(tab);
        Ok(())
    }

    /// Empty the entire storage area for this origin.
    ///
    /// Maps to `DOMStorage.clear`. Equivalent to calling
    /// `localStorage.clear()` / `sessionStorage.clear()` from page JS.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.local_storage().clear().await?;
    /// # Ok(()) }
    /// ```
    pub async fn clear(&self) -> Result<()> {
        self.ensure_enabled().await?;
        let tab = self.upgraded_tab()?;
        let storage_id = self.build_storage_id(&tab).await?;
        self.inner
            .session
            .call("DOMStorage.clear", json!({ "storageId": storage_id }))
            .await?;
        drop(tab);
        Ok(())
    }

    /// Number of entries in this storage area.
    ///
    /// Implemented as [`Self::get_all`] + `.len()` — CDP has no dedicated
    /// length op.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let n = tab.local_storage().len().await?;
    /// println!("{n} entries");
    /// # Ok(()) }
    /// ```
    pub async fn len(&self) -> Result<usize> {
        Ok(self.get_all().await?.len())
    }

    /// Lazily dispatch `DOMStorage.enable` exactly once per handle. The
    /// gate is a [`tokio::sync::OnceCell`] — concurrent callers race to
    /// the same single dispatch and all see the same result.
    async fn ensure_enabled(&self) -> Result<()> {
        self.inner
            .enable_once
            .get_or_try_init(|| async {
                self.inner
                    .session
                    .call("DOMStorage.enable", json!({}))
                    .await?;
                Ok::<(), ZendriverError>(())
            })
            .await?;
        Ok(())
    }

    /// Upgrade the cached [`Weak<TabInner>`] into a strong [`crate::Tab`]
    /// handle. The caller is expected to hold the returned `Tab` across
    /// the full storage op so the underlying session can't be torn down
    /// between the origin lookup and the `DOMStorage.*` dispatch.
    fn upgraded_tab(&self) -> Result<crate::tab::Tab> {
        let tab_inner = self
            .inner
            .tab
            .upgrade()
            .ok_or_else(|| ZendriverError::Storage("owning tab has been dropped".into()))?;
        Ok(crate::tab::Tab { inner: tab_inner })
    }

    /// Build the CDP `StorageId` payload — `{ securityOrigin, isLocalStorage }`
    /// — from the supplied tab's current URL. The caller passes a live
    /// `&Tab` (typically the one held by [`Self::upgraded_tab`]) so the
    /// URL read and any follow-on CDP call see the same session.
    async fn build_storage_id(&self, tab: &crate::tab::Tab) -> Result<Value> {
        let url = tab.url().await?;
        let origin = url.origin().ascii_serialization();
        Ok(json!({
            "securityOrigin": origin,
            "isLocalStorage": self.inner.is_local,
        }))
    }
}

/// Parse `DOMStorage.getDOMStorageItems` response. The `entries` field is
/// an array of `[key, value]` two-element string arrays.
#[allow(clippy::result_large_err)] // ZendriverError variance is the project-wide return type
fn parse_entries(resp: &Value) -> Result<HashMap<String, String>> {
    let arr = resp
        .get("entries")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ZendriverError::Storage("response missing `entries` array".into()))?;
    let mut out = HashMap::with_capacity(arr.len());
    for entry in arr {
        let pair = entry.as_array().ok_or_else(|| {
            ZendriverError::Storage("`entries` element is not a [key, value] array".into())
        })?;
        if pair.len() < 2 {
            return Err(ZendriverError::Storage(
                "`entries` element has fewer than 2 fields".into(),
            ));
        }
        let k = pair[0]
            .as_str()
            .ok_or_else(|| ZendriverError::Storage("entry key is not a string".into()))?
            .to_string();
        let v = pair[1]
            .as_str()
            .ok_or_else(|| ZendriverError::Storage("entry value is not a string".into()))?
            .to_string();
        out.insert(k, v);
    }
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;
    use zendriver_transport::testing::MockConnection;

    use crate::tab::Tab;

    /// [`Storage::get`] dispatches `DOMStorage.enable` exactly once (on
    /// first use), then resolves the storage origin via
    /// `Target.getTargetInfo` and reads via `DOMStorage.getDOMStorageItems`
    /// with `isLocalStorage: true`.
    #[tokio::test]
    async fn get_enables_domstorage_then_reads_with_local_flag() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess.clone());
        let storage = Storage::new(sess, true, Arc::downgrade(&tab.inner));

        let fut = tokio::spawn({
            let s = storage.clone();
            async move { s.get("theme").await }
        });

        // 1. DOMStorage.enable (once-per-handle gate).
        let id_enable = mock.expect_cmd("DOMStorage.enable").await;
        mock.reply(id_enable, json!({})).await;

        // 2. Target.getTargetInfo for origin discovery (Tab::url).
        let id_info = mock.expect_cmd("Target.getTargetInfo").await;
        mock.reply(
            id_info,
            json!({ "targetInfo": { "url": "https://x.test/page" } }),
        )
        .await;

        // 3. DOMStorage.getDOMStorageItems with the derived storageId.
        let id_get = mock.expect_cmd("DOMStorage.getDOMStorageItems").await;
        let sent = mock.last_sent();
        assert_eq!(
            sent["params"]["storageId"]["securityOrigin"],
            "https://x.test"
        );
        assert_eq!(sent["params"]["storageId"]["isLocalStorage"], true);
        mock.reply(
            id_get,
            json!({ "entries": [["theme", "dark"], ["lang", "en"]] }),
        )
        .await;

        let got = fut.await.unwrap().unwrap();
        assert_eq!(got, Some("dark".to_string()));

        conn.shutdown();
    }

    /// [`Storage::set`] dispatches `DOMStorage.setDOMStorageItem` with the
    /// per-tab origin in the `storageId` and the key/value at the top level.
    /// Verifies the once-per-handle `DOMStorage.enable` precedes the write.
    #[tokio::test]
    async fn set_dispatches_set_dom_storage_item_with_key_and_value() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess.clone());
        // sessionStorage variant to exercise the is_local=false code path.
        let storage = Storage::new(sess, false, Arc::downgrade(&tab.inner));

        let fut = tokio::spawn({
            let s = storage.clone();
            async move { s.set("token", "abc123").await }
        });

        let id_enable = mock.expect_cmd("DOMStorage.enable").await;
        mock.reply(id_enable, json!({})).await;

        let id_info = mock.expect_cmd("Target.getTargetInfo").await;
        mock.reply(
            id_info,
            json!({ "targetInfo": { "url": "https://x.test/page" } }),
        )
        .await;

        let id_set = mock.expect_cmd("DOMStorage.setDOMStorageItem").await;
        let sent = mock.last_sent();
        assert_eq!(
            sent["params"]["storageId"]["securityOrigin"],
            "https://x.test"
        );
        assert_eq!(sent["params"]["storageId"]["isLocalStorage"], false);
        assert_eq!(sent["params"]["key"], "token");
        assert_eq!(sent["params"]["value"], "abc123");
        mock.reply(id_set, json!({})).await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }
}
