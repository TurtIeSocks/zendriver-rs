//! `Element` read methods: attribute access, inner/outer markup, layout
//! geometry, and visibility/enabled state.
//!
//! Each method routes through [`Element::with_refresh`] so a stale CDP
//! handle (post-navigation, post-React-rerender) triggers exactly one
//! transparent re-resolve via [`Element::refresh`] and retry. `is_visible`
//! and `is_enabled` are thin wrappers around the
//! [`crate::query::actionability`] predicates that the actionability gate
//! also consumes — keeping a single source of truth for "what counts as
//! visible/enabled."
//!
//! `inner_text` and `outer_html` shipped in P2 inside `element/mod.rs` and
//! moved here so all reads live in one file; the wrap in `with_refresh`
//! is the only behavior change (P2 left them un-wrapped because the
//! refresh path didn't exist yet).
//!
//! `bounding_box` returns a viewport-relative [`BoundingBox`] derived
//! from the `content` quad of `DOM.getBoxModel`. The CDP response shape
//! is `{ model: { content: [x1,y1,x2,y2,x3,y3,x4,y4], width, height, .. } }`
//! with quad points in clock-wise order starting top-left, so
//! `(x, y) = (content[0], content[1])` and `(width, height)` come from
//! the top-level fields. Returns `Ok(None)` when the node has no box
//! (e.g. `display: none`) — Chrome surfaces this as a `-32000 "Could
//! not compute box model"` Cdp error rather than a stale-node signal,
//! so we map that specific error to `None` instead of bubbling.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::element::Element;
use crate::error::{Result, ZendriverError};
use crate::query::actionability;
use crate::query::BoundingBox;

impl Element {
    /// Return the value of attribute `name`, or `None` if the element
    /// does not have the attribute. Routes through `el.getAttribute(name)`
    /// in JS so HTML attributes (`href`, `data-*`) and ARIA attributes
    /// resolve identically to user code reading them in DevTools.
    pub async fn attr(&self, name: impl AsRef<str>) -> Result<Option<String>> {
        let name = name.as_ref().to_string();
        self.with_refresh(|| {
            let name = name.clone();
            async move {
                let res = self
                    .call_on(
                        "function(n){ return this.getAttribute(n); }",
                        json!([{ "value": name }]),
                    )
                    .await?;
                // `getAttribute` returns `null` for missing attributes →
                // `value` field is missing/null on the RemoteObject.
                match res.get("value") {
                    Some(Value::String(s)) => Ok(Some(s.clone())),
                    _ => Ok(None),
                }
            }
        })
        .await
    }

    /// Return a snapshot of every attribute on the element as a
    /// `HashMap<name, value>`. Built from `el.attributes` (live
    /// `NamedNodeMap`) on the JS side, materialized into a plain object
    /// before crossing the CDP boundary so `returnByValue` can serialize
    /// it.
    pub async fn attrs(&self) -> Result<HashMap<String, String>> {
        self.with_refresh(|| async move {
            let js = r"
                function() {
                    const out = {};
                    for (const a of this.attributes) { out[a.name] = a.value; }
                    return out;
                }
            ";
            let res = self.call_on(js, json!([])).await?;
            let value = res.get("value").cloned().unwrap_or(Value::Null);
            let obj = value.as_object().cloned().unwrap_or_default();
            let mut out = HashMap::with_capacity(obj.len());
            for (k, v) in obj {
                if let Some(s) = v.as_str() {
                    out.insert(k, s.to_string());
                }
            }
            Ok(out)
        })
        .await
    }

    /// Return the element's `innerText` (rendered text content, excluding
    /// hidden subtrees). Moved here from `element/mod.rs` in T18 and
    /// wrapped in `with_refresh` so post-navigation reads transparently
    /// recover from a stale handle.
    pub async fn inner_text(&self) -> Result<String> {
        self.with_refresh(|| async move {
            let res = self
                .call_on("function(){ return this.innerText; }", json!([]))
                .await?;
            Ok(res["value"].as_str().unwrap_or("").to_string())
        })
        .await
    }

    /// Return the element's `innerHTML` (serialized child markup).
    pub async fn inner_html(&self) -> Result<String> {
        self.with_refresh(|| async move {
            let res = self
                .call_on("function(){ return this.innerHTML; }", json!([]))
                .await?;
            Ok(res["value"].as_str().unwrap_or("").to_string())
        })
        .await
    }

    /// Return the element's `outerHTML` (serialized element + child
    /// markup). Moved here from `element/mod.rs` in T18 and wrapped in
    /// `with_refresh`.
    pub async fn outer_html(&self) -> Result<String> {
        self.with_refresh(|| async move {
            let res = self
                .call_on("function(){ return this.outerHTML; }", json!([]))
                .await?;
            Ok(res["value"].as_str().unwrap_or("").to_string())
        })
        .await
    }

    /// Return the viewport-relative bounding box, or `None` if the
    /// element has no box (e.g. `display: none` — Chrome reports this as
    /// `-32000 "Could not compute box model"`).
    ///
    /// Coordinates come from `DOM.getBoxModel`'s `content` quad. The quad
    /// is clock-wise starting top-left, so `(x, y) = (content[0],
    /// content[1])`; `(width, height)` come from the top-level fields of
    /// the box-model response (which are integer CSS px, widened to
    /// `f64` for arithmetic-friendliness with click coordinates).
    pub async fn bounding_box(&self) -> Result<Option<BoundingBox>> {
        self.with_refresh(|| async move {
            let backend_node_id = self.backend_node_id_cloned().await?;
            let res = self
                .inner
                .tab
                .call(
                    "DOM.getBoxModel",
                    json!({ "backendNodeId": backend_node_id }),
                )
                .await;
            let res = match res {
                Ok(v) => v,
                // No box (display: none, detached, etc.) → None.
                Err(ZendriverError::Cdp { ref message, .. })
                    if message.contains("Could not compute box model") =>
                {
                    return Ok(None);
                }
                Err(e) => return Err(e),
            };
            let model = match res.get("model") {
                Some(m) => m,
                None => return Ok(None),
            };
            let content = match model.get("content").and_then(|v| v.as_array()) {
                Some(c) if c.len() >= 2 => c,
                _ => return Ok(None),
            };
            let x = content[0].as_f64().unwrap_or(0.0);
            let y = content[1].as_f64().unwrap_or(0.0);
            let width = model.get("width").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let height = model.get("height").and_then(|v| v.as_f64()).unwrap_or(0.0);
            Ok(Some(BoundingBox {
                x,
                y,
                width,
                height,
            }))
        })
        .await
    }

    /// `true` iff the element is currently rendered + visible — attached
    /// to the document, has a positive bounding box, and is not hidden
    /// via `display`, `visibility`, or `opacity: 0`. Delegates to
    /// [`crate::query::actionability::check_visible`] so the same
    /// definition powers the `wait_actionable` gate.
    pub async fn is_visible(&self) -> Result<bool> {
        self.with_refresh(|| async move { actionability::check_visible(self).await })
            .await
    }

    /// `true` iff the element is not disabled — native `el.disabled` is
    /// false-ish AND `aria-disabled` is not `'true'`. Non-form elements
    /// are considered enabled. Delegates to
    /// [`crate::query::actionability::check_enabled`].
    pub async fn is_enabled(&self) -> Result<bool> {
        self.with_refresh(|| async move { actionability::check_enabled(self).await })
            .await
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::tab::Tab;
    use zendriver_transport::testing::MockConnection;
    use zendriver_transport::SessionHandle;

    #[tokio::test]
    async fn attr_returns_some_when_attribute_present() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new(sess, std::sync::Weak::new());
        let el = Element::from_jsret(tab, 1, "R1".to_string());

        let fut = tokio::spawn({
            let e = el.clone();
            async move { e.attr("href").await }
        });

        let id = mock.expect_cmd("Runtime.callFunctionOn").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["objectId"], "R1");
        assert!(sent["params"]["functionDeclaration"]
            .as_str()
            .unwrap()
            .contains("getAttribute"));
        assert_eq!(sent["params"]["arguments"][0]["value"], "href");
        mock.reply(
            id,
            json!({ "result": { "value": "/login", "type": "string" } }),
        )
        .await;

        let got = fut.await.unwrap().unwrap();
        assert_eq!(got, Some("/login".to_string()));
        conn.shutdown();
    }

    #[tokio::test]
    async fn attrs_returns_hashmap_of_all_attributes() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new(sess, std::sync::Weak::new());
        let el = Element::from_jsret(tab, 1, "R1".to_string());

        let fut = tokio::spawn({
            let e = el.clone();
            async move { e.attrs().await }
        });

        let id = mock.expect_cmd("Runtime.callFunctionOn").await;
        let sent = mock.last_sent();
        assert!(sent["params"]["functionDeclaration"]
            .as_str()
            .unwrap()
            .contains("attributes"));
        mock.reply(
            id,
            json!({
                "result": {
                    "value": { "id": "btn", "class": "primary", "data-x": "42" },
                    "type": "object"
                }
            }),
        )
        .await;

        let map = fut.await.unwrap().unwrap();
        assert_eq!(map.get("id").map(String::as_str), Some("btn"));
        assert_eq!(map.get("class").map(String::as_str), Some("primary"));
        assert_eq!(map.get("data-x").map(String::as_str), Some("42"));
        assert_eq!(map.len(), 3);
        conn.shutdown();
    }

    #[tokio::test]
    async fn bounding_box_parses_dom_get_box_model_content_quad() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new(sess, std::sync::Weak::new());
        let el = Element::from_jsret(tab, 42, "R1".to_string());

        let fut = tokio::spawn({
            let e = el.clone();
            async move { e.bounding_box().await }
        });

        let id = mock.expect_cmd("DOM.getBoxModel").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["backendNodeId"], 42);
        mock.reply(
            id,
            json!({
                "model": {
                    // Quad: top-left (10,20), top-right (110,20),
                    // bottom-right (110,70), bottom-left (10,70).
                    "content": [10.0, 20.0, 110.0, 20.0, 110.0, 70.0, 10.0, 70.0],
                    "padding": [10.0, 20.0, 110.0, 20.0, 110.0, 70.0, 10.0, 70.0],
                    "border":  [10.0, 20.0, 110.0, 20.0, 110.0, 70.0, 10.0, 70.0],
                    "margin":  [10.0, 20.0, 110.0, 20.0, 110.0, 70.0, 10.0, 70.0],
                    "width":  100,
                    "height": 50
                }
            }),
        )
        .await;

        let bbox = fut.await.unwrap().unwrap().expect("should be Some");
        assert!((bbox.x - 10.0).abs() < 1e-9);
        assert!((bbox.y - 20.0).abs() < 1e-9);
        assert!((bbox.width - 100.0).abs() < 1e-9);
        assert!((bbox.height - 50.0).abs() < 1e-9);
        conn.shutdown();
    }
}
