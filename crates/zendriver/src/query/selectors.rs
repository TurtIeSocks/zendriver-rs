//! Selector kinds + CDP/JS resolution. T9 implements CSS + XPath; T10
//! implements Text + TextRegex; T11 lands Role.
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
/// T10 lands `Text` and `TextRegex`; `Role` is filled in by T11. Until
/// then the Role stub returns a clearly-attributed error so an
/// accidental dispatch surfaces immediately instead of returning an
/// empty match set (which would silently pass tests).
///
/// `TextRegex` stores pattern + flags as separate strings (rather than
/// a `regex::Regex`) so the JS-side `new RegExp(pat, flags)` mirrors
/// the user's intent exactly — `text_regex(re)` plumbs `re.as_str()`
/// + empty flags, while `text_regex_with_flags` (T12) plumbs both.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Xpath/Text/TextRegex/Role wired up by T11/T12.
pub(crate) enum SelectorKind {
    Css(String),
    Xpath(String),
    Text { needle: String, exact: bool },
    TextRegex { pattern: String, flags: String },
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
            SelectorKind::Text { needle, exact } => resolve_text_one(scope, needle, *exact).await,
            SelectorKind::TextRegex { pattern, flags } => {
                resolve_text_regex_one(scope, pattern, flags).await
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
            SelectorKind::Text { needle, exact } => resolve_text_many(scope, needle, *exact).await,
            SelectorKind::TextRegex { pattern, flags } => {
                resolve_text_regex_many(scope, pattern, flags).await
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
// Text (case-insensitive substring or whitespace-collapsed exact)
// ---------------------------------------------------------------------
//
// Two paths:
//   - exact=true  -> XPath `//*[normalize-space(.)=<needle>]` with
//     `singleNodeValue` for `_one` / `ORDERED_NODE_SNAPSHOT_TYPE` for
//     `_many`. The XPath string is constructed *in JS* via
//     `JSON.stringify(needle)` (then `"`->`'`) so multi-quote needles
//     don't break XPath literal escaping.
//   - exact=false -> JS tree walk:
//     `Array.from(ctx.querySelectorAll('*')).filter(el => (el.innerText||el.textContent).toLowerCase().includes(needle.toLowerCase()))`.
//     `_one` slices `[0] || null`; `_many` returns the array.
//
// `innerText||textContent` matches Playwright's `getByText` and is
// resilient to hidden elements (which have `innerText === ""` but
// non-empty `textContent`).

fn build_text_substring_js_tab(needle: &str) -> String {
    // `Array.from(document.querySelectorAll('*')).filter(...)`.
    format!(
        "Array.from(document.querySelectorAll('*')).filter(function(el){{var n={n};var t=el.innerText||el.textContent||'';return t.toLowerCase().includes(n.toLowerCase());}})",
        n = json!(needle),
    )
}

fn build_text_substring_fn_body() -> &'static str {
    // Element scope: `this` is the scope element. Used via
    // `Runtime.callFunctionOn` with the needle as the sole argument.
    "function(n){return Array.from(this.querySelectorAll('*')).filter(function(el){var t=el.innerText||el.textContent||'';return t.toLowerCase().includes(n.toLowerCase());});}"
}

fn build_text_exact_xpath_js_tab(needle: &str, snapshot: bool) -> String {
    // Construct the XPath in JS so the needle literal is escaped by
    // JSON.stringify -> single-quoted XPath string. snapshot=true
    // returns an Array of all matches; snapshot=false returns the
    // first match or null.
    if snapshot {
        format!(
            "(function(){{var n={n};var xp=\"//*[normalize-space(.)=\"+JSON.stringify(n).replace(/\"/g,\"'\")+\"]\";var r=document.evaluate(xp,document,null,XPathResult.ORDERED_NODE_SNAPSHOT_TYPE,null);var a=[];for(var i=0;i<r.snapshotLength;i++)a.push(r.snapshotItem(i));return a;}})()",
            n = json!(needle),
        )
    } else {
        format!(
            "(function(){{var n={n};var xp=\"//*[normalize-space(.)=\"+JSON.stringify(n).replace(/\"/g,\"'\")+\"]\";return document.evaluate(xp,document,null,XPathResult.FIRST_ORDERED_NODE_TYPE,null).singleNodeValue;}})()",
            n = json!(needle),
        )
    }
}

fn build_text_exact_xpath_fn_body(snapshot: bool) -> &'static str {
    // Element scope: `this` is the context node. Needle passed as arg.
    if snapshot {
        "function(n){var xp=\"//*[normalize-space(.)=\"+JSON.stringify(n).replace(/\"/g,\"'\")+\"]\";var r=document.evaluate(xp,this,null,XPathResult.ORDERED_NODE_SNAPSHOT_TYPE,null);var a=[];for(var i=0;i<r.snapshotLength;i++)a.push(r.snapshotItem(i));return a;}"
    } else {
        "function(n){var xp=\"//*[normalize-space(.)=\"+JSON.stringify(n).replace(/\"/g,\"'\")+\"]\";return document.evaluate(xp,this,null,XPathResult.FIRST_ORDERED_NODE_TYPE,null).singleNodeValue;}"
    }
}

#[allow(dead_code)] // Reached via SelectorKind::resolve_one, gated until T12.
async fn resolve_text_one(
    scope: &QueryScope<'_>,
    needle: &str,
    exact: bool,
) -> Result<Option<RemoteRef>> {
    if exact {
        // XPath path returns a single node or null.
        let result = match scope {
            QueryScope::Tab(tab) => {
                tab.call(
                    "Runtime.evaluate",
                    json!({
                        "expression": build_text_exact_xpath_js_tab(needle, false),
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
                            "functionDeclaration": build_text_exact_xpath_fn_body(false),
                            "arguments": [{ "value": needle }],
                            "returnByValue": false,
                        }),
                    )
                    .await?
            }
        };
        extract_node_ref(scope.tab(), &result["result"]).await
    } else {
        // Substring path returns an Array; pick first match.
        let result = match scope {
            QueryScope::Tab(tab) => {
                tab.call(
                    "Runtime.evaluate",
                    json!({
                        "expression": format!("({})[0] || null", build_text_substring_js_tab(needle)),
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
                            "functionDeclaration": format!(
                                "function(n){{return ({})[0] || null;}}",
                                // Build the substring filter body inline so
                                // `this` resolves to the scope element.
                                "Array.from(this.querySelectorAll('*')).filter(function(el){var t=el.innerText||el.textContent||'';return t.toLowerCase().includes(n.toLowerCase());})"
                            ),
                            "arguments": [{ "value": needle }],
                            "returnByValue": false,
                        }),
                    )
                    .await?
            }
        };
        extract_node_ref(scope.tab(), &result["result"]).await
    }
}

#[allow(dead_code)] // Reached via SelectorKind::resolve_many, gated until T12.
async fn resolve_text_many(
    scope: &QueryScope<'_>,
    needle: &str,
    exact: bool,
) -> Result<Vec<RemoteRef>> {
    let result = if exact {
        match scope {
            QueryScope::Tab(tab) => {
                tab.call(
                    "Runtime.evaluate",
                    json!({
                        "expression": build_text_exact_xpath_js_tab(needle, true),
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
                            "functionDeclaration": build_text_exact_xpath_fn_body(true),
                            "arguments": [{ "value": needle }],
                            "returnByValue": false,
                        }),
                    )
                    .await?
            }
        }
    } else {
        match scope {
            QueryScope::Tab(tab) => {
                tab.call(
                    "Runtime.evaluate",
                    json!({
                        "expression": build_text_substring_js_tab(needle),
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
                            "functionDeclaration": build_text_substring_fn_body(),
                            "arguments": [{ "value": needle }],
                            "returnByValue": false,
                        }),
                    )
                    .await?
            }
        }
    };
    extract_array_refs(scope.tab(), &result["result"]).await
}

// ---------------------------------------------------------------------
// TextRegex (serialized as JS `new RegExp(pattern, flags)`)
// ---------------------------------------------------------------------
//
// JS path:
//   `Array.from(ctx.querySelectorAll('*')).filter(el => new RegExp(pat, flags).test(el.innerText||el.textContent))`
//
// The regex is *re-parsed* on the JS side via `new RegExp`, so the
// pattern must use JS-flavored regex syntax (which is essentially the
// same as Rust's `regex` crate for the common subset). Flags are passed
// verbatim — caller is responsible for valid JS flag chars (e.g. "i",
// "im", "gi", etc.). Empty flags string is fine.
//
// We construct the RegExp *once outside* the filter callback so the
// pattern is only compiled per query rather than per element.

fn build_text_regex_js_tab(pattern: &str, flags: &str) -> String {
    format!(
        "(function(){{var r=new RegExp({p}, {f});return Array.from(document.querySelectorAll('*')).filter(function(el){{var t=el.innerText||el.textContent||'';return r.test(t);}});}})()",
        p = json!(pattern),
        f = json!(flags),
    )
}

fn build_text_regex_fn_body() -> &'static str {
    // Element scope: `this` is the scope element. Pattern + flags
    // passed as arguments.
    "function(p,f){var r=new RegExp(p,f);return Array.from(this.querySelectorAll('*')).filter(function(el){var t=el.innerText||el.textContent||'';return r.test(t);});}"
}

#[allow(dead_code)] // Reached via SelectorKind::resolve_one, gated until T12.
async fn resolve_text_regex_one(
    scope: &QueryScope<'_>,
    pattern: &str,
    flags: &str,
) -> Result<Option<RemoteRef>> {
    let result = match scope {
        QueryScope::Tab(tab) => {
            tab.call(
                "Runtime.evaluate",
                json!({
                    "expression": format!("({})[0] || null", build_text_regex_js_tab(pattern, flags)),
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
                        "functionDeclaration": format!(
                            "function(p,f){{return ({})[0] || null;}}",
                            "(function(p,f){var r=new RegExp(p,f);return Array.from(this.querySelectorAll('*')).filter(function(el){var t=el.innerText||el.textContent||'';return r.test(t);});}).call(this,p,f)"
                        ),
                        "arguments": [{ "value": pattern }, { "value": flags }],
                        "returnByValue": false,
                    }),
                )
                .await?
        }
    };
    extract_node_ref(scope.tab(), &result["result"]).await
}

#[allow(dead_code)] // Reached via SelectorKind::resolve_many, gated until T12.
async fn resolve_text_regex_many(
    scope: &QueryScope<'_>,
    pattern: &str,
    flags: &str,
) -> Result<Vec<RemoteRef>> {
    let result = match scope {
        QueryScope::Tab(tab) => {
            tab.call(
                "Runtime.evaluate",
                json!({
                    "expression": build_text_regex_js_tab(pattern, flags),
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
                        "functionDeclaration": build_text_regex_fn_body(),
                        "arguments": [{ "value": pattern }, { "value": flags }],
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

    #[tokio::test]
    async fn text_substring_eval_lowercases_and_includes_needle() {
        // Non-exact text selector: confirm the dispatched JS expression
        // contains the lowercase-fold (`.toLowerCase()`) + the needle
        // verbatim so the case-insensitive substring contract is
        // preserved. We respond with `null` so the future completes
        // immediately without needing the full describeNode dance.
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new(sess, std::sync::Weak::new());

        let fut = tokio::spawn({
            let t = tab.clone();
            async move {
                let scope = QueryScope::Tab(&t);
                SelectorKind::Text {
                    needle: "Sign In".into(),
                    exact: false,
                }
                .resolve_one(&scope)
                .await
            }
        });

        let id_q = mock.expect_cmd("Runtime.evaluate").await;
        let sent = mock.last_sent()["params"]["expression"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            sent.contains(".toLowerCase()"),
            "substring path must lowercase-fold both sides; got: {sent}"
        );
        assert!(
            sent.contains("Sign In"),
            "substring path must embed the needle verbatim; got: {sent}"
        );
        assert!(
            sent.contains(".includes("),
            "substring path must call .includes; got: {sent}"
        );

        // null-out the result so resolve_one short-circuits to Ok(None).
        mock.reply(
            id_q,
            json!({ "result": { "type": "object", "subtype": "null" } }),
        )
        .await;

        let r = fut.await.unwrap().unwrap();
        assert!(r.is_none(), "null subtype must yield Ok(None)");
        conn.shutdown();
    }

    #[tokio::test]
    async fn text_regex_eval_constructs_new_regexp_with_pattern_and_flags() {
        // TextRegex selector: confirm the dispatched JS expression
        // builds `new RegExp(<pat>, <flags>)` with both strings present.
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new(sess, std::sync::Weak::new());

        let fut = tokio::spawn({
            let t = tab.clone();
            async move {
                let scope = QueryScope::Tab(&t);
                SelectorKind::TextRegex {
                    pattern: "hello.*world".into(),
                    flags: "im".into(),
                }
                .resolve_one(&scope)
                .await
            }
        });

        let id_q = mock.expect_cmd("Runtime.evaluate").await;
        let sent = mock.last_sent()["params"]["expression"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            sent.contains("new RegExp"),
            "regex path must instantiate `new RegExp`; got: {sent}"
        );
        assert!(
            sent.contains("hello.*world"),
            "regex path must embed the pattern; got: {sent}"
        );
        assert!(
            sent.contains("im"),
            "regex path must embed the flags string; got: {sent}"
        );

        mock.reply(
            id_q,
            json!({ "result": { "type": "object", "subtype": "null" } }),
        )
        .await;

        let r = fut.await.unwrap().unwrap();
        assert!(r.is_none(), "null subtype must yield Ok(None)");
        conn.shutdown();
    }
}
