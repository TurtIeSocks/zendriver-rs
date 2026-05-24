//! [`Element`] read methods: attribute access, inner/outer markup, layout
//! geometry, and visibility/enabled state.
//!
//! Each method routes through an internal "refresh on stale" wrapper so a
//! stale CDP handle (post-navigation, post-React-rerender) triggers exactly
//! one transparent re-resolve and retry. `is_visible` and `is_enabled` are
//! thin wrappers around the actionability predicates that the actionability
//! gate also consumes — keeping a single source of truth for "what counts
//! as visible/enabled."
//!
//! `bounding_box` returns a viewport-relative [`BoundingBox`] derived from
//! the `content` quad of `DOM.getBoxModel`. Returns `Ok(None)` when the
//! node has no box (e.g. `display: none`) — Chrome surfaces this as a
//! `-32000 "Could not compute box model"` Cdp error rather than a stale-node
//! signal, so we map that specific error to `None` instead of bubbling.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::element::Element;
use crate::error::{Result, ZendriverError};
use crate::query::actionability;
use crate::query::BoundingBox;

impl Element {
    /// Return the value of attribute `name`, or `None` when absent.
    ///
    /// Routes through `el.getAttribute(name)` in JS so HTML attributes
    /// (`href`, `data-*`) and ARIA attributes resolve identically to user
    /// code reading them in DevTools.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let link = tab.find().css("a").one().await?;
    /// if let Some(href) = link.attr("href").await? {
    ///     println!("link goes to {href}");
    /// }
    /// # Ok(()) }
    /// ```
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

    /// Return a snapshot of every attribute on the element.
    ///
    /// Built from `el.attributes` (live `NamedNodeMap`) on the JS side,
    /// materialized into a plain object before crossing the CDP boundary
    /// so `returnByValue` can serialize it.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let el = tab.find().css("input").one().await?;
    /// for (k, v) in el.attrs().await? {
    ///     println!("{k}={v}");
    /// }
    /// # Ok(()) }
    /// ```
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

    /// Return the element's `innerText` (rendered text, excluding hidden
    /// subtrees).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let h1 = tab.find().css("h1").one().await?;
    /// assert_eq!(h1.inner_text().await?, "Example Domain");
    /// # Ok(()) }
    /// ```
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
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let div = tab.find().css("div.content").one().await?;
    /// println!("{}", div.inner_html().await?);
    /// # Ok(()) }
    /// ```
    pub async fn inner_html(&self) -> Result<String> {
        self.with_refresh(|| async move {
            let res = self
                .call_on("function(){ return this.innerHTML; }", json!([]))
                .await?;
            Ok(res["value"].as_str().unwrap_or("").to_string())
        })
        .await
    }

    /// Return the element's `outerHTML` (serialized element + children).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let h1 = tab.find().css("h1").one().await?;
    /// assert!(h1.outer_html().await?.starts_with("<h1"));
    /// # Ok(()) }
    /// ```
    pub async fn outer_html(&self) -> Result<String> {
        self.with_refresh(|| async move {
            let res = self
                .call_on("function(){ return this.outerHTML; }", json!([]))
                .await?;
            Ok(res["value"].as_str().unwrap_or("").to_string())
        })
        .await
    }

    /// Return the viewport-relative bounding box, or `None` when absent.
    ///
    /// `None` typically means the element has no box (e.g. `display: none`)
    /// — Chrome reports this as `-32000 "Could not compute box model"` which
    /// is mapped to `None` rather than bubbling.
    ///
    /// Coordinates come from `DOM.getBoxModel`'s `content` quad (clock-wise
    /// starting top-left); `(width, height)` come from the top-level fields
    /// of the box-model response.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let btn = tab.find().css("button").one().await?;
    /// if let Some(bbox) = btn.bounding_box().await? {
    ///     println!("button at ({}, {}) size {}x{}", bbox.x, bbox.y, bbox.width, bbox.height);
    /// }
    /// # Ok(()) }
    /// ```
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

    /// Returns `true` iff the element is rendered + visible.
    ///
    /// Attached to the document, has a positive bounding box, and is not
    /// hidden via `display`, `visibility`, or `opacity: 0`. Uses the same
    /// internal visibility predicate that powers the actionability gate.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let modal = tab.find().css(".modal").one().await?;
    /// if !modal.is_visible().await? {
    ///     println!("modal is hidden");
    /// }
    /// # Ok(()) }
    /// ```
    pub async fn is_visible(&self) -> Result<bool> {
        self.with_refresh(|| async move { actionability::check_visible(self).await })
            .await
    }

    /// Returns `true` iff the element is not disabled.
    ///
    /// Native `el.disabled` is false-ish AND `aria-disabled` is not
    /// `'true'`. Non-form elements are considered enabled.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let submit = tab.find().css("button[type=submit]").one().await?;
    /// if submit.is_enabled().await? {
    ///     submit.click().await?;
    /// }
    /// # Ok(()) }
    /// ```
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
        let tab = Tab::new_for_test(sess);
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
        let tab = Tab::new_for_test(sess);
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
        let tab = Tab::new_for_test(sess);
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
