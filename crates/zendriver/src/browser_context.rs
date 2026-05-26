//! Per-`BrowserContext` lifecycle on top of CDP `Target.createBrowserContext`.

use std::sync::Arc;
use crate::browser::BrowserInner;

pub struct BrowserContext {
    pub(crate) browser: Arc<BrowserInner>,
    pub(crate) id: String,
}

impl BrowserContext {
    pub fn id(&self) -> &str {
        &self.id
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
            let _ctx = BrowserContext { browser: inner.clone(), id: "ctx-drop-test".into() };
        }

        // Drop has spawned a task — wait for it to land on the mock.
        let id = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            mock.expect_cmd("Target.disposeBrowserContext"),
        ).await.expect("dispose did not fire within 2s");

        let sent = mock.last_sent();
        assert_eq!(sent["params"]["browserContextId"], "ctx-drop-test");
        mock.reply(id, serde_json::json!({})).await;

        conn.shutdown();
    }
}
