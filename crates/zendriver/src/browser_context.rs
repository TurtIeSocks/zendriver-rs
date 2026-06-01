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

impl Drop for BrowserContext {
    fn drop(&mut self) {
        let browser = self.browser.clone();
        let id = std::mem::take(&mut self.id);
        if id.is_empty() {
            return; // already disposed (explicit dispose() will exist in a later task)
        }
        tokio::spawn(async move {
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
