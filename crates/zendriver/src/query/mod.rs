//! `FindBuilder` — chainable element queries scoped to a `Tab`.

pub mod actionability;
pub mod modifiers;
pub mod role;
pub mod selectors;

use std::time::Duration;

use serde_json::json;
use tokio::time::Instant;

use crate::element::Element;
use crate::error::{Result, ZendriverError};
use crate::tab::Tab;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

pub struct FindBuilder<'tab> {
    pub(crate) tab: &'tab Tab,
    pub(crate) selector: Option<String>,
    pub(crate) timeout: Duration,
}

impl<'tab> FindBuilder<'tab> {
    pub(crate) fn new(tab: &'tab Tab) -> Self {
        Self {
            tab,
            selector: None,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    #[must_use]
    pub fn css(mut self, selector: impl Into<String>) -> Self {
        self.selector = Some(selector.into());
        self
    }

    #[must_use]
    pub fn timeout(mut self, dur: Duration) -> Self {
        self.timeout = dur;
        self
    }

    /// Wait for and return the first matching element. Errors with
    /// `ElementNotFound` if no element matches within the timeout.
    pub async fn one(self) -> Result<Element> {
        let sel = self.selector.ok_or_else(|| {
            ZendriverError::Navigation("FindBuilder requires a selector (.css(...))".into())
        })?;
        let deadline = Instant::now() + self.timeout;
        loop {
            if let Some(el) = try_query_selector(self.tab, &sel).await? {
                return Ok(el);
            }
            if Instant::now() >= deadline {
                return Err(ZendriverError::ElementNotFound { selector: sel });
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Like `one()`, but returns `None` instead of erroring when no element
    /// matches within the timeout.
    pub async fn one_or_none(self) -> Result<Option<Element>> {
        match self.one().await {
            Ok(el) => Ok(Some(el)),
            Err(ZendriverError::ElementNotFound { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

async fn try_query_selector(tab: &Tab, selector: &str) -> Result<Option<Element>> {
    // Use Runtime.evaluate to find the node and return a remote object handle.
    let res = tab
        .call(
            "Runtime.evaluate",
            json!({
                "expression": format!("document.querySelector({})", json!(selector)),
                "returnByValue": false,
            }),
        )
        .await?;
    let result = &res["result"];
    if result["subtype"] == "null" || result["type"] == "undefined" {
        return Ok(None);
    }
    let object_id = result["objectId"]
        .as_str()
        .ok_or_else(|| ZendriverError::Navigation("querySelector returned no objectId".into()))?
        .to_string();

    // Get the backend node id for later use (Element::call_on uses objectId,
    // but other operations need backend_node_id — we resolve once here).
    let describe = tab
        .call("DOM.describeNode", json!({ "objectId": object_id }))
        .await
        .ok();
    let backend_node_id = describe
        .as_ref()
        .and_then(|d| d["node"]["backendNodeId"].as_i64())
        .unwrap_or_default();

    Ok(Some(Element::new(tab.clone(), backend_node_id, object_id)))
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use zendriver_transport::testing::MockConnection;
    use zendriver_transport::SessionHandle;

    #[tokio::test]
    async fn one_returns_element_when_query_selector_matches() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new(sess, std::sync::Weak::new());

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.find().css("#b").one().await }
        });

        let id_q = mock.expect_cmd("Runtime.evaluate").await;
        assert!(mock.last_sent()["params"]["expression"]
            .as_str()
            .unwrap()
            .contains("document.querySelector"));
        mock.reply(
            id_q,
            json!({ "result": { "objectId": "R1", "type": "object", "subtype": "node" } }),
        )
        .await;

        let id_d = mock.expect_cmd("DOM.describeNode").await;
        mock.reply(id_d, json!({ "node": { "backendNodeId": 42 } }))
            .await;

        let el = fut.await.unwrap().unwrap();
        assert_eq!(el.inner.backend_node_id, 42);
        assert_eq!(el.inner.remote_object_id, "R1");
        conn.shutdown();
    }

    #[tokio::test]
    async fn one_returns_element_not_found_when_query_returns_null() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new(sess, std::sync::Weak::new());

        let fut = tokio::spawn({
            let t = tab.clone();
            async move {
                t.find()
                    .css("#missing")
                    .timeout(Duration::from_millis(150))
                    .one()
                    .await
            }
        });

        // The builder will poll a few times; reply null each time until timeout.
        loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(200)) => break,
                cmd = mock.expect_cmd("Runtime.evaluate") => {
                    mock.reply(cmd, json!({ "result": { "type": "object", "subtype": "null" } })).await;
                }
            }
        }

        let res = fut.await.unwrap();
        match res {
            Err(ZendriverError::ElementNotFound { selector }) => assert_eq!(selector, "#missing"),
            Err(e) => panic!("unexpected error: {e:?}"),
            Ok(_) => panic!("unexpected ok"),
        }
        conn.shutdown();
    }

    #[tokio::test]
    async fn one_or_none_returns_none_on_timeout() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new(sess, std::sync::Weak::new());

        let fut = tokio::spawn({
            let t = tab.clone();
            async move {
                t.find()
                    .css("#missing")
                    .timeout(Duration::from_millis(120))
                    .one_or_none()
                    .await
            }
        });

        loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(180)) => break,
                cmd = mock.expect_cmd("Runtime.evaluate") => {
                    mock.reply(cmd, json!({ "result": { "type": "object", "subtype": "null" } })).await;
                }
            }
        }

        let res = fut.await.unwrap().unwrap();
        assert!(res.is_none());
        conn.shutdown();
    }
}
