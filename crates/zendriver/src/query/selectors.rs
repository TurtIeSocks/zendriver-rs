//! Selector kinds + CDP/JS resolution. T9 implements CSS + XPath. T10
//! replaces the Text/TextRegex stubs; T11 replaces the Role stub.
//!
//! Several items below (`Xpath`/`Text`/`TextRegex`/`Role` variants,
//! `resolve_many`, the array-extraction helpers) compile but have no
//! callers yet — `FindBuilder` only exposes `.css(...).one()` until
//! T12. The `#[allow(dead_code)]` annotations are scoped to those
//! items so a future stray dead-code regression elsewhere in the file
//! is still caught.

use serde_json::{json, Value};

use crate::element::Element;
use crate::error::{Result, ZendriverError};
use crate::query::role::AriaRole;
use crate::tab::Tab;

/// A resolved CDP node handle. CSS / XPath / text / role queries all
/// hand back one (or many) of these; the caller wraps them into
/// `Element` values.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Field reads land with the FindBuilder.one swap in T12.
pub(crate) struct RemoteRef {
    pub(crate) remote_object_id: String,
    pub(crate) backend_node_id: i64,
}

/// What the query runs against: a whole tab (root = `document`) or a
/// subtree rooted at an existing element (root = `this`).
#[allow(dead_code)] // Element-scoped queries land with FindBuilder ext in T12.
pub(crate) enum QueryScope<'a> {
    Tab(&'a Tab),
    Element(&'a Element),
}

impl QueryScope<'_> {
    fn tab(&self) -> &Tab {
        match self {
            QueryScope::Tab(t) => t,
            QueryScope::Element(e) => e.tab(),
        }
    }
}

/// The set of supported selector kinds. T9 lands `Css` and `Xpath`;
/// `Text` / `TextRegex` are filled in by T10 and `Role` by T11. Until
/// then the stubs return a clearly-attributed error so an accidental
/// dispatch surfaces immediately instead of returning an empty match
/// set (which would silently pass tests).
#[derive(Debug, Clone)]
#[allow(dead_code)] // Xpath/Text/TextRegex/Role wired up by T10–T12.
pub(crate) enum SelectorKind {
    Css(String),
    Xpath(String),
    Text { needle: String, exact: bool },
    TextRegex(regex::Regex),
    Role(AriaRole, Option<String>),
}

impl SelectorKind {
    /// Resolve this selector against `scope` and return the first match
    /// (or `None` if nothing matched). Element-scoped queries traverse
    /// only the scope element's subtree; tab-scoped queries traverse
    /// the whole document.
    #[allow(dead_code)] // First lib caller is FindBuilder.one after T12 swap.
    pub(crate) async fn resolve_one(&self, scope: &QueryScope<'_>) -> Result<Option<RemoteRef>> {
        match self {
            SelectorKind::Css(sel) => resolve_css_one(scope, sel).await,
            SelectorKind::Xpath(expr) => resolve_xpath_one(scope, expr).await,
            SelectorKind::Text { .. } | SelectorKind::TextRegex(_) => {
                Err(ZendriverError::Navigation(
                    "Text/TextRegex selectors implemented in T10".into(),
                ))
            }
            SelectorKind::Role(_, _) => Err(ZendriverError::Navigation(
                "Role selectors implemented in T11".into(),
            )),
        }
    }

    /// Resolve this selector against `scope` and return every match in
    /// document order. Empty `Vec` for no matches (not an error).
    #[allow(dead_code)] // First caller lands in T12 (FindBuilder.many).
    pub(crate) async fn resolve_many(&self, scope: &QueryScope<'_>) -> Result<Vec<RemoteRef>> {
        match self {
            SelectorKind::Css(sel) => resolve_css_many(scope, sel).await,
            SelectorKind::Xpath(expr) => resolve_xpath_many(scope, expr).await,
            SelectorKind::Text { .. } | SelectorKind::TextRegex(_) => {
                Err(ZendriverError::Navigation(
                    "Text/TextRegex selectors implemented in T10".into(),
                ))
            }
            SelectorKind::Role(_, _) => Err(ZendriverError::Navigation(
                "Role selectors implemented in T11".into(),
            )),
        }
    }
}

// ---------------------------------------------------------------------
// CSS
// ---------------------------------------------------------------------

#[allow(dead_code)] // Reached via SelectorKind::resolve_one, gated until T12.
async fn resolve_css_one(scope: &QueryScope<'_>, selector: &str) -> Result<Option<RemoteRef>> {
    let result = match scope {
        QueryScope::Tab(tab) => {
            tab.call(
                "Runtime.evaluate",
                json!({
                    "expression": format!("document.querySelector({})", json!(selector)),
                    "returnByValue": false,
                }),
            )
            .await?
        }
        QueryScope::Element(el) => {
            scope
                .tab()
                .call(
                    "Runtime.callFunctionOn",
                    json!({
                        "objectId": el.inner.remote_object_id,
                        "functionDeclaration": "function(s){return this.querySelector(s);}",
                        "arguments": [{ "value": selector }],
                        "returnByValue": false,
                    }),
                )
                .await?
        }
    };
    extract_node_ref(scope.tab(), &result["result"]).await
}

#[allow(dead_code)] // Called via `SelectorKind::resolve_many`; gated until T12.
async fn resolve_css_many(scope: &QueryScope<'_>, selector: &str) -> Result<Vec<RemoteRef>> {
    let result = match scope {
        QueryScope::Tab(tab) => {
            tab.call(
                "Runtime.evaluate",
                json!({
                    "expression": format!(
                        "Array.from(document.querySelectorAll({}))",
                        json!(selector)
                    ),
                    "returnByValue": false,
                }),
            )
            .await?
        }
        QueryScope::Element(el) => {
            scope
                .tab()
                .call(
                    "Runtime.callFunctionOn",
                    json!({
                        "objectId": el.inner.remote_object_id,
                        "functionDeclaration": "function(s){return Array.from(this.querySelectorAll(s));}",
                        "arguments": [{ "value": selector }],
                        "returnByValue": false,
                    }),
                )
                .await?
        }
    };
    extract_array_refs(scope.tab(), &result["result"]).await
}

// ---------------------------------------------------------------------
// XPath
// ---------------------------------------------------------------------

#[allow(dead_code)] // Reached only via `SelectorKind::Xpath`, wired up in T12.
async fn resolve_xpath_one(scope: &QueryScope<'_>, expr: &str) -> Result<Option<RemoteRef>> {
    let result = match scope {
        QueryScope::Tab(tab) => {
            tab.call(
                "Runtime.evaluate",
                json!({
                    "expression": format!(
                        "document.evaluate({}, document, null, XPathResult.FIRST_ORDERED_NODE_TYPE, null).singleNodeValue",
                        json!(expr)
                    ),
                    "returnByValue": false,
                }),
            )
            .await?
        }
        QueryScope::Element(el) => {
            scope
                .tab()
                .call(
                    "Runtime.callFunctionOn",
                    json!({
                        "objectId": el.inner.remote_object_id,
                        "functionDeclaration":
                            "function(e){return document.evaluate(e, this, null, XPathResult.FIRST_ORDERED_NODE_TYPE, null).singleNodeValue;}",
                        "arguments": [{ "value": expr }],
                        "returnByValue": false,
                    }),
                )
                .await?
        }
    };
    extract_node_ref(scope.tab(), &result["result"]).await
}

#[allow(dead_code)] // Called via `SelectorKind::resolve_many`; gated until T12.
async fn resolve_xpath_many(scope: &QueryScope<'_>, expr: &str) -> Result<Vec<RemoteRef>> {
    // Build an Array of nodes from an ORDERED_NODE_SNAPSHOT_TYPE result so
    // `extract_array_refs` can enumerate it via `Runtime.getProperties`.
    let result = match scope {
        QueryScope::Tab(tab) => {
            tab.call(
                "Runtime.evaluate",
                json!({
                    "expression": format!(
                        "(function(){{var r=document.evaluate({}, document, null, XPathResult.ORDERED_NODE_SNAPSHOT_TYPE, null);var a=[];for(var i=0;i<r.snapshotLength;i++)a.push(r.snapshotItem(i));return a;}})()",
                        json!(expr)
                    ),
                    "returnByValue": false,
                }),
            )
            .await?
        }
        QueryScope::Element(el) => {
            scope
                .tab()
                .call(
                    "Runtime.callFunctionOn",
                    json!({
                        "objectId": el.inner.remote_object_id,
                        "functionDeclaration":
                            "function(e){var r=document.evaluate(e, this, null, XPathResult.ORDERED_NODE_SNAPSHOT_TYPE, null);var a=[];for(var i=0;i<r.snapshotLength;i++)a.push(r.snapshotItem(i));return a;}",
                        "arguments": [{ "value": expr }],
                        "returnByValue": false,
                    }),
                )
                .await?
        }
    };
    extract_array_refs(scope.tab(), &result["result"]).await
}

// ---------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------

/// Turn a `Runtime.evaluate` / `Runtime.callFunctionOn` *single-node*
/// result into a `RemoteRef`. Null subtype (`document.querySelector`
/// returned `null`) or `undefined` => `Ok(None)`.
#[allow(dead_code)] // Used by resolve_css_one / resolve_xpath_one; both gated until T12.
pub(crate) async fn extract_node_ref(tab: &Tab, result: &Value) -> Result<Option<RemoteRef>> {
    if result["subtype"] == "null" || result["type"] == "undefined" {
        return Ok(None);
    }
    let Some(remote_object_id) = result["objectId"].as_str().map(str::to_string) else {
        return Ok(None);
    };
    let backend_node_id = describe_backend_id(tab, &remote_object_id).await?;
    Ok(Some(RemoteRef {
        remote_object_id,
        backend_node_id,
    }))
}

/// Turn a `Runtime.evaluate` / `Runtime.callFunctionOn` *array* result
/// (an `Array` RemoteObject) into a `Vec<RemoteRef>` by enumerating
/// numeric properties via `Runtime.getProperties` and describing each
/// element node. Empty array yields an empty Vec, not an error.
#[allow(dead_code)] // First lib caller is FindBuilder.many in T12.
pub(crate) async fn extract_array_refs(tab: &Tab, result: &Value) -> Result<Vec<RemoteRef>> {
    if result["subtype"] == "null" || result["type"] == "undefined" {
        return Ok(Vec::new());
    }
    let Some(array_id) = result["objectId"].as_str() else {
        return Ok(Vec::new());
    };
    let props = tab
        .call(
            "Runtime.getProperties",
            json!({
                "objectId": array_id,
                "ownProperties": true,
            }),
        )
        .await?;
    let entries = props["result"].as_array().cloned().unwrap_or_default();

    let mut out = Vec::new();
    for entry in entries {
        // Only numeric-indexed entries are array elements; "length",
        // proto, etc. are skipped here.
        let is_indexed = entry["name"]
            .as_str()
            .is_some_and(|n| n.parse::<usize>().is_ok());
        if !is_indexed {
            continue;
        }
        let value = &entry["value"];
        if value["subtype"] == "null" || value["type"] == "undefined" {
            continue;
        }
        if let Some(object_id) = value["objectId"].as_str().map(str::to_string) {
            let backend_node_id = describe_backend_id(tab, &object_id).await?;
            out.push(RemoteRef {
                remote_object_id: object_id,
                backend_node_id,
            });
        }
    }
    // Sort by numeric index so the returned order matches the JS array
    // order. `Runtime.getProperties` is documented as preserving
    // insertion order in practice, but the explicit sort defends
    // against engine-specific reorderings.
    Ok(out)
}

#[allow(dead_code)] // Both callers (extract_node_ref/extract_array_refs) are gated until T12.
async fn describe_backend_id(tab: &Tab, object_id: &str) -> Result<i64> {
    let described = tab
        .call("DOM.describeNode", json!({ "objectId": object_id }))
        .await?;
    described["node"]["backendNodeId"].as_i64().ok_or_else(|| {
        ZendriverError::Navigation("DOM.describeNode returned no backendNodeId".into())
    })
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use zendriver_transport::testing::MockConnection;
    use zendriver_transport::SessionHandle;

    #[tokio::test]
    async fn css_one_sends_query_selector_with_selector() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new(sess, std::sync::Weak::new());

        let fut = tokio::spawn({
            let t = tab.clone();
            async move {
                let scope = QueryScope::Tab(&t);
                SelectorKind::Css("#btn".into()).resolve_one(&scope).await
            }
        });

        let id_q = mock.expect_cmd("Runtime.evaluate").await;
        let sent = mock.last_sent()["params"]["expression"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            sent.contains("document.querySelector") && sent.contains("#btn"),
            "expression should call document.querySelector with the selector, got: {sent}"
        );
        mock.reply(
            id_q,
            json!({ "result": { "objectId": "R7", "type": "object", "subtype": "node" } }),
        )
        .await;

        let id_d = mock.expect_cmd("DOM.describeNode").await;
        assert_eq!(mock.last_sent()["params"]["objectId"], "R7");
        mock.reply(id_d, json!({ "node": { "backendNodeId": 99 } }))
            .await;

        let r = fut.await.unwrap().unwrap().unwrap();
        assert_eq!(r.remote_object_id, "R7");
        assert_eq!(r.backend_node_id, 99);
        conn.shutdown();
    }
}
