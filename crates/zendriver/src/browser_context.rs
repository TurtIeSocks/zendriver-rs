//! Per-`BrowserContext` lifecycle on top of CDP `Target.createBrowserContext`.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use crate::browser::BrowserInner;
use crate::error::ZendriverError;
use crate::tab::Tab;

pub struct BrowserContext {
    pub(crate) browser: Arc<BrowserInner>,
    pub(crate) id: String,
}

impl BrowserContext {
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Open a new tab navigated to `about:blank` **inside this context**.
    ///
    /// Convenience wrapper over [`BrowserContext::new_tab_at`] with
    /// `"about:blank"`.
    ///
    /// # Errors
    ///
    /// Same as [`BrowserContext::new_tab_at`].
    pub async fn new_tab(&self) -> Result<Tab, ZendriverError> {
        self.new_tab_at("about:blank").await
    }

    /// Open a new tab navigated to `url` **inside this context**.
    ///
    /// Mirrors [`crate::Browser::new_tab_at`] but threads
    /// `browserContextId` into the `Target.createTarget` params so the new
    /// target lands in this context rather than the default one. Without
    /// that field the per-context isolation (proxy, storage) the
    /// `BrowserContext` was created for would not apply to the tab.
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::Navigation`] if the CDP response is
    /// missing `targetId`, and [`ZendriverError::TabNotFound`] if the
    /// internal tab registrar fails to register the new tab within 5s.
    pub async fn new_tab_at(&self, url: impl Into<String>) -> Result<Tab, ZendriverError> {
        let url = url.into();
        let res = self
            .browser
            .conn
            .call_raw(
                "Target.createTarget",
                json!({ "url": url, "browserContextId": self.id }),
                None,
            )
            .await?;
        let target_id = res
            .get("targetId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ZendriverError::Navigation("Target.createTarget returned no targetId".into())
            })?
            .to_string();

        // Mirror the wait pattern used by `Browser::new_tab_at`: enable a
        // `Notify` subscription before reading the tabs map so a notify
        // that lands between the read and the wait is still delivered.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let notif = self.browser.tabs_changed.notified();
            tokio::pin!(notif);
            notif.as_mut().enable();

            {
                let tabs = self.browser.tabs.read().await;
                if let Some(tab) = tabs.values().find(|t| t.target_id() == target_id) {
                    return Ok(tab.clone());
                }
            }

            if tokio::time::Instant::now() >= deadline {
                return Err(ZendriverError::TabNotFound(target_id));
            }

            tokio::select! {
                () = notif => {}
                () = tokio::time::sleep_until(deadline) => {}
            }
        }
    }
}

/// Builder for an isolated [`BrowserContext`] with an optional proxy and
/// per-context proxy credentials. Created via [`crate::Browser::browser_context`].
pub struct BrowserContextBuilder {
    browser: Arc<BrowserInner>,
    proxy: Option<String>,
    bypass: Option<String>,
    #[cfg(feature = "interception")]
    explicit_auth: Option<(String, String)>,
}

impl BrowserContextBuilder {
    pub(crate) fn new(browser: Arc<BrowserInner>) -> Self {
        Self {
            browser,
            proxy: None,
            bypass: None,
            #[cfg(feature = "interception")]
            explicit_auth: None,
        }
    }

    /// Upstream proxy `scheme://[user[:pass]@]host:port`. Embedded userinfo is
    /// used as the auth credentials (and stripped from the `proxyServer` sent
    /// to Chrome, which ignores it there) unless overridden by
    /// [`Self::proxy_auth`].
    #[must_use]
    pub fn proxy(mut self, proxy: impl Into<String>) -> Self {
        self.proxy = Some(proxy.into());
        self
    }

    /// Hosts matching this pattern bypass the proxy (e.g. `"<-loopback>"`).
    #[must_use]
    pub fn proxy_bypass(mut self, bypass: impl Into<String>) -> Self {
        self.bypass = Some(bypass.into());
        self
    }

    /// Explicit proxy credentials; overrides any userinfo embedded in
    /// [`Self::proxy`].
    #[cfg(feature = "interception")]
    #[must_use]
    pub fn proxy_auth(mut self, user: impl Into<String>, pass: impl Into<String>) -> Self {
        self.explicit_auth = Some((user.into(), pass.into()));
        self
    }

    /// Create the context: sends `Target.createBrowserContext` with a
    /// userinfo-free `proxyServer`, registers credentials (if any), and
    /// returns the [`BrowserContext`].
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::Navigation`] on an invalid proxy URL or a
    /// response missing `browserContextId`.
    pub async fn build(self) -> Result<BrowserContext, ZendriverError> {
        let (proxy_server, embedded_creds) = match self.proxy.as_deref() {
            Some(p) => {
                let parsed = crate::proxy::split_proxy_url(p)?;
                (Some(parsed.server), parsed.credentials)
            }
            None => (None, None),
        };

        let id = self
            .browser
            .create_browser_context_raw(proxy_server.as_deref(), self.bypass.as_deref())
            .await?;

        #[cfg(feature = "interception")]
        {
            if let Some(creds) = self.explicit_auth.or(embedded_creds) {
                self.browser
                    .context_proxy_auth
                    .lock()
                    .await
                    .insert(id.clone(), creds);
            }
        }
        #[cfg(not(feature = "interception"))]
        {
            if embedded_creds.is_some() {
                tracing::warn!(
                    context_id = %id,
                    "proxy credentials supplied but the `interception` feature is off; \
                     per-context proxy auth is inactive"
                );
            }
        }

        Ok(BrowserContext {
            browser: self.browser,
            id,
        })
    }
}

impl Drop for BrowserContext {
    fn drop(&mut self) {
        let browser = self.browser.clone();
        let id = std::mem::take(&mut self.id);
        if id.is_empty() {
            return; // already disposed (explicit dispose() will exist in a later task)
        }
        tokio::spawn(async move {
            #[cfg(feature = "interception")]
            browser.context_proxy_auth.lock().await.remove(&id);
            if let Err(e) = browser.dispose_browser_context(&id).await {
                tracing::warn!(context_id = %id, error = %e, "BrowserContext dispose failed");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_context_exposes_id() {
        fn _accept(_: &BrowserContext) {}
    }
}

#[cfg(test)]
mod drop_tests {
    use super::*;
    use zendriver_transport::testing::MockConnection;

    /// Asserts that dropping a [`BrowserContext`] spawns a background task
    /// which issues `Target.disposeBrowserContext` carrying the context's
    /// id. Models the lifecycle parity guarantee: a `BrowserContext` handle
    /// owns its CDP context for the duration of its scope.
    #[tokio::test]
    async fn drop_schedules_dispose_call() {
        let (mut mock, conn) = MockConnection::pair();
        let inner = crate::browser::test_only_inner_from_conn(conn.clone());

        {
            let _ctx = BrowserContext {
                browser: inner.clone(),
                id: "ctx-drop-test".into(),
            };
        }

        // Drop has spawned a task — wait for it to land on the mock.
        let id = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            mock.expect_cmd("Target.disposeBrowserContext"),
        )
        .await
        .expect("dispose did not fire within 2s");

        let sent = mock.last_sent();
        assert_eq!(sent["params"]["browserContextId"], "ctx-drop-test");
        mock.reply(id, serde_json::json!({})).await;

        conn.shutdown();
    }
}

#[cfg(test)]
mod new_tab_tests {
    use super::*;
    use zendriver_transport::testing::MockConnection;

    /// Asserts `BrowserContext::new_tab_at` issues a
    /// `Target.createTarget` whose params include the context's id as
    /// `browserContextId`. Without that field the new tab lands in the
    /// default context — defeating the isolation guarantee.
    ///
    /// The test only verifies the outbound CDP shape; the registrar wait
    /// inside `new_tab_at` blocks on a `Target.attachedToTarget` event the
    /// mock doesn't dispatch, so the future is bounded by a short timeout.
    #[tokio::test]
    async fn new_tab_at_passes_browser_context_id() {
        let (mut mock, conn) = MockConnection::pair();
        let inner = crate::browser::test_only_inner_from_conn(conn.clone());
        let ctx = BrowserContext {
            browser: inner,
            id: "ctx-tab-test".into(),
        };

        let fut = tokio::spawn(async move { ctx.new_tab_at("about:blank").await });

        let id = mock.expect_cmd("Target.createTarget").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["url"], "about:blank");
        assert_eq!(sent["params"]["browserContextId"], "ctx-tab-test");
        mock.reply(id, serde_json::json!({ "targetId": "target-1" }))
            .await;

        // Registrar wait blocks on a `Target.attachedToTarget` event the
        // mock won't fire; bound the test with a short timeout.
        let _ = tokio::time::timeout(std::time::Duration::from_millis(200), fut).await;

        conn.shutdown();
    }
}

#[cfg(all(test, feature = "interception"))]
mod auth_cleanup_tests {
    use super::*;
    use zendriver_transport::testing::MockConnection;

    /// Dropping a `BrowserContext` removes its entry from the browser's
    /// `context_proxy_auth` registry.
    #[tokio::test]
    async fn drop_unregisters_context_credentials() {
        let (mut mock, conn) = MockConnection::pair();
        let inner = crate::browser::test_only_inner_from_conn(conn.clone());
        inner
            .context_proxy_auth
            .lock()
            .await
            .insert("CTX1".to_string(), ("bob".into(), "s3cret".into()));

        {
            let _ctx = BrowserContext {
                browser: inner.clone(),
                id: "CTX1".into(),
            };
        } // drop here

        // Drop spawns a task; wait for the dispose call to confirm it ran.
        let id = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            mock.expect_cmd("Target.disposeBrowserContext"),
        )
        .await
        .expect("dispose did not fire");
        mock.reply(id, serde_json::json!({})).await;

        // Give the spawned task a moment to complete the removal.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(inner.context_proxy_auth.lock().await.get("CTX1").is_none());
        conn.shutdown();
    }
}

#[cfg(test)]
mod builder_tests {
    use zendriver_transport::testing::MockConnection;

    /// `.proxy("http://user:pass@host:port")` sends a **userinfo-free**
    /// `proxyServer` to `Target.createBrowserContext`.
    #[tokio::test]
    async fn build_strips_userinfo_from_proxy_server() {
        let (mut mock, conn) = MockConnection::pair();
        let inner = crate::browser::test_only_inner_from_conn(conn.clone());
        let browser = crate::Browser::test_only_from_inner(inner);

        let fut = tokio::spawn(async move {
            browser
                .browser_context()
                .proxy("http://bob:s3cret@proxy.example:8080")
                .proxy_bypass("<-loopback>")
                .build()
                .await
        });

        let id = mock.expect_cmd("Target.createBrowserContext").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["proxyServer"], "http://proxy.example:8080");
        assert_eq!(sent["params"]["proxyBypassList"], "<-loopback>");
        mock.reply(id, serde_json::json!({ "browserContextId": "CTX1" }))
            .await;

        let ctx = tokio::time::timeout(std::time::Duration::from_secs(2), fut)
            .await
            .expect("build timed out")
            .unwrap()
            .unwrap();
        assert_eq!(ctx.id(), "CTX1");
        conn.shutdown();
    }

    /// Under `interception`, embedded userinfo is registered as the context's
    /// proxy credentials.
    #[cfg(feature = "interception")]
    #[tokio::test]
    async fn build_registers_embedded_credentials() {
        let (mut mock, conn) = MockConnection::pair();
        let inner = crate::browser::test_only_inner_from_conn(conn.clone());
        let browser = crate::Browser::test_only_from_inner(inner.clone());

        let fut = tokio::spawn(async move {
            browser
                .browser_context()
                .proxy("http://bob:s3cret@proxy.example:8080")
                .build()
                .await
        });

        let id = mock.expect_cmd("Target.createBrowserContext").await;
        mock.reply(id, serde_json::json!({ "browserContextId": "CTX1" }))
            .await;
        let _ctx = tokio::time::timeout(std::time::Duration::from_secs(2), fut)
            .await
            .expect("build timed out")
            .unwrap()
            .unwrap();

        let creds = inner.context_proxy_auth.lock().await.get("CTX1").cloned();
        assert_eq!(creds, Some(("bob".into(), "s3cret".into())));
        conn.shutdown();
    }

    /// Explicit `.proxy_auth()` overrides embedded userinfo.
    #[cfg(feature = "interception")]
    #[tokio::test]
    async fn explicit_proxy_auth_overrides_userinfo() {
        let (mut mock, conn) = MockConnection::pair();
        let inner = crate::browser::test_only_inner_from_conn(conn.clone());
        let browser = crate::Browser::test_only_from_inner(inner.clone());

        let fut = tokio::spawn(async move {
            browser
                .browser_context()
                .proxy("http://bob:s3cret@proxy.example:8080")
                .proxy_auth("alice", "hunter2")
                .build()
                .await
        });

        let id = mock.expect_cmd("Target.createBrowserContext").await;
        mock.reply(id, serde_json::json!({ "browserContextId": "CTX1" }))
            .await;
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), fut).await;

        let creds = inner.context_proxy_auth.lock().await.get("CTX1").cloned();
        assert_eq!(creds, Some(("alice".into(), "hunter2".into())));
        conn.shutdown();
    }
}
