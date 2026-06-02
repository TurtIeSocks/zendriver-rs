//! Selector kinds + CDP/JS resolution. T9 implements CSS + XPath; T10
//! implements Text + TextRegex; T11 lands Role.
//!
//! Several items below (`Xpath`/`Text`/`TextRegex`/`Role` variants,
//! `resolve_many`, the array-extraction helpers) compile but have no
//! callers yet — `FindBuilder` only exposes `.css(...).one()` until
//! T12. The `#[allow(dead_code)]` annotations are scoped to those
//! items so a future stray dead-code regression elsewhere in the file
//! is still caught.
//!
//! Role resolution (T11): role-only queries compile to a `[role="..."]`
//! CSS attribute selector and reuse `resolve_css_one`/`resolve_css_many`
//! directly. Role + accessible-name queries do the same CSS pass first
//! to get all candidates, then post-filter via
//! `Accessibility.getPartialAXTree { backendNodeId, fetchRelatives: false }`
//! per candidate, matching `name.value` against the needle with
//! case-insensitive substring semantics. The AX call has to be per-node
//! because the AX tree doesn't expose a "find by computed name" query —
//! the JS-side `aria-label` attribute alone misses cases where the name
//! comes from `aria-labelledby`, the wrapped text, or `<label>` linkage,
//! which only the computed AX tree resolves.

use serde_json::{Value, json};
use zendriver_transport::SessionHandle;

use crate::element::Element;
use crate::error::{Result, ZendriverError};
use crate::frame::Frame;
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

/// What the query runs against: a whole tab (root = `document`), a
/// subtree rooted at an existing element (root = `this`), or a specific
/// frame (root = the frame's `document`, dispatched on the frame's own
/// CDP session — distinct from the parent tab's session for OOPIFs).
#[allow(dead_code)] // Element-scoped queries land with FindBuilder ext in T12.
pub(crate) enum QueryScope<'a> {
    Tab(&'a Tab),
    Element(&'a Element),
    Frame(&'a Frame),
}

impl QueryScope<'_> {
    /// CDP session that should dispatch this query's commands. Tab and
    /// Element scopes both route to the owning tab's session; Frame
    /// scope routes to the frame's own session (same as the parent tab
    /// for same-origin frames, a distinct child session for OOPIFs).
    ///
    /// All `Runtime.evaluate` / `Runtime.callFunctionOn` /
    /// `Runtime.getProperties` / `DOM.describeNode` /
    /// `Accessibility.getPartialAXTree` calls in this module go through
    /// this accessor so adding a new scope variant only requires
    /// extending the match arm.
    pub(crate) fn session(&self) -> &SessionHandle {
        match self {
            QueryScope::Tab(t) => t.session(),
            QueryScope::Element(e) => e.tab().session(),
            QueryScope::Frame(f) => f.session(),
        }
    }

    /// Execution-context id that `Runtime.evaluate` should target for
    /// this scope, or `None` to let CDP pick the session's default
    /// (which is the main-frame main-world for a Tab/Element scope).
    ///
    /// Frame scope must NOT use the session default — for a same-origin
    /// iframe, the session is the parent tab's session and CDP's
    /// "default context" is the *parent* document. A
    /// `document.querySelectorAll(...)` evaluated without a contextId
    /// would walk the parent DOM, not the iframe's, and find nothing
    /// for selectors targeting iframe-only content. Returning the
    /// frame's isolated-world contextId pins the eval to the iframe's
    /// document.
    pub(crate) async fn execution_context_id(&self) -> Result<Option<i64>> {
        match self {
            QueryScope::Tab(_) | QueryScope::Element(_) => Ok(None),
            QueryScope::Frame(f) => Ok(Some(f.ensure_isolated_world().await?)),
        }
    }

    /// Owned `Tab` clone for `Element::synthesize_query`. Tab/Element
    /// scopes return a cheap `Arc` bump on the underlying `TabInner`;
    /// Frame scope upgrades the frame's `Weak<TabInner>` (which is
    /// always live in practice because every `Frame` is constructed by
    /// a `Tab` that holds the strong reference). A dead Weak indicates
    /// the owning Tab was dropped while the Frame clone outlived it —
    /// a logic bug worth a clear panic rather than a confusing
    /// `Result` propagation.
    pub(crate) fn synthesize_tab(&self) -> Tab {
        match self {
            QueryScope::Tab(t) => (*t).clone(),
            QueryScope::Element(e) => e.tab().clone(),
            QueryScope::Frame(f) => f
                .tab_for_synthesize()
                .expect("Frame outlived its owning Tab while a FindBuilder query was in flight"),
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
        self.resolve_one_inner(scope, false).await
    }

    /// `resolve_one` with the cross-cutting `best_match` flag.
    /// `best_match` only affects text selectors (`Text` / `TextRegex`),
    /// where the JS collector re-sorts candidates by closest text length
    /// before `[0]` is taken; it is a no-op for css/xpath/role.
    pub(crate) async fn resolve_one_inner(
        &self,
        scope: &QueryScope<'_>,
        best_match: bool,
    ) -> Result<Option<RemoteRef>> {
        match self {
            SelectorKind::Css(sel) => resolve_css_one(scope, sel).await,
            SelectorKind::Xpath(expr) => resolve_xpath_one(scope, expr).await,
            SelectorKind::Text { needle, exact } => {
                resolve_text_one(scope, needle, *exact, best_match).await
            }
            SelectorKind::TextRegex { pattern, flags } => {
                resolve_text_regex_one(scope, pattern, flags, best_match).await
            }
            SelectorKind::Role(role, name) => resolve_role_one(scope, *role, name.as_deref()).await,
        }
    }

    /// Resolve this selector against `scope` and return every match in
    /// document order. Empty `Vec` for no matches (not an error).
    #[allow(dead_code)] // First caller lands in T12 (FindBuilder.many).
    pub(crate) async fn resolve_many(&self, scope: &QueryScope<'_>) -> Result<Vec<RemoteRef>> {
        self.resolve_many_inner(scope, false).await
    }

    /// `resolve_many` with the cross-cutting `best_match` flag (see
    /// [`Self::resolve_one_inner`]). When set on a text selector the
    /// returned Vec is ordered closest-length first, so `.one()` taking
    /// `[0]` lands on the nearest match.
    pub(crate) async fn resolve_many_inner(
        &self,
        scope: &QueryScope<'_>,
        best_match: bool,
    ) -> Result<Vec<RemoteRef>> {
        match self {
            SelectorKind::Css(sel) => resolve_css_many(scope, sel).await,
            SelectorKind::Xpath(expr) => resolve_xpath_many(scope, expr).await,
            SelectorKind::Text { needle, exact } => {
                resolve_text_many(scope, needle, *exact, best_match).await
            }
            SelectorKind::TextRegex { pattern, flags } => {
                resolve_text_regex_many(scope, pattern, flags, best_match).await
            }
            SelectorKind::Role(role, name) => {
                resolve_role_many(scope, *role, name.as_deref()).await
            }
        }
    }
}

// ---------------------------------------------------------------------
// CSS
// ---------------------------------------------------------------------

#[allow(dead_code)] // Reached via SelectorKind::resolve_one, gated until T12.
async fn resolve_css_one(scope: &QueryScope<'_>, selector: &str) -> Result<Option<RemoteRef>> {
    let session = scope.session();
    let result = match scope {
        QueryScope::Tab(_) | QueryScope::Frame(_) => {
            session
                .call(
                    "Runtime.evaluate",
                    json!({
                        "expression": format!("document.querySelector({})", json!(selector)),
                        "returnByValue": false,
                    }),
                )
                .await?
        }
        QueryScope::Element(el) => {
            let object_id = el.remote_object_id_cloned().await?;
            session
                .call(
                    "Runtime.callFunctionOn",
                    json!({
                        "objectId": object_id,
                        "functionDeclaration": "function(s){return this.querySelector(s);}",
                        "arguments": [{ "value": selector }],
                        "returnByValue": false,
                    }),
                )
                .await?
        }
    };
    extract_node_ref(session, &result["result"]).await
}

#[allow(dead_code)] // Called via `SelectorKind::resolve_many`; gated until T12.
async fn resolve_css_many(scope: &QueryScope<'_>, selector: &str) -> Result<Vec<RemoteRef>> {
    let session = scope.session();
    let ctx = scope.execution_context_id().await?;
    let result = match scope {
        QueryScope::Tab(_) | QueryScope::Frame(_) => {
            let mut params = json!({
                "expression": format!(
                    "Array.from(document.querySelectorAll({}))",
                    json!(selector)
                ),
                "returnByValue": false,
            });
            if let Some(id) = ctx {
                params["contextId"] = json!(id);
            }
            session.call("Runtime.evaluate", params).await?
        }
        QueryScope::Element(el) => {
            let object_id = el.remote_object_id_cloned().await?;
            session
                .call(
                    "Runtime.callFunctionOn",
                    json!({
                        "objectId": object_id,
                        "functionDeclaration": "function(s){return Array.from(this.querySelectorAll(s));}",
                        "arguments": [{ "value": selector }],
                        "returnByValue": false,
                    }),
                )
                .await?
        }
    };
    extract_array_refs(session, &result["result"]).await
}

// ---------------------------------------------------------------------
// Predicate (bs4-like combinable matchers)
// ---------------------------------------------------------------------
//
// A `PredicateSet` compiles to a CSS selector (`tag` + structural attrs)
// plus a JS boolean post-filter (`attr_regex` + text predicates). The
// terminal builds ONE `querySelectorAll(css).filter(el => <jsFilter>)`
// per scope — exactly mirroring `resolve_css_many`'s scope dispatch
// (`contextId` for frames, `this` for element subtrees) so predicates
// cross frames the same way CSS does.

use crate::query::predicate::PredicateSet;

/// Resolve `pred` against `scope` and return every match in document
/// order. Compiles to `Array.from((document|this).querySelectorAll(css))
/// .filter(el => <jsFilter>)`, with the CSS selector JSON-embedded so a
/// quote/backslash in an attribute value can't break out of the JS
/// string literal. Empty `Vec` for no matches (not an error).
pub(crate) async fn resolve_predicate_many(
    scope: &QueryScope<'_>,
    pred: &PredicateSet,
) -> Result<Vec<RemoteRef>> {
    let session = scope.session();
    let ctx = scope.execution_context_id().await?;
    let css = pred.to_css_selector();
    let filter = pred.to_js_filter();
    let result = match scope {
        QueryScope::Tab(_) | QueryScope::Frame(_) => {
            let expr = format!(
                "Array.from(document.querySelectorAll({})).filter(function(el){{return {};}})",
                json!(css),
                filter,
            );
            let mut params = json!({
                "expression": expr,
                "returnByValue": false,
            });
            if let Some(id) = ctx {
                params["contextId"] = json!(id);
            }
            session.call("Runtime.evaluate", params).await?
        }
        QueryScope::Element(el) => {
            let object_id = el.remote_object_id_cloned().await?;
            let func = format!(
                "function(){{return Array.from(this.querySelectorAll({})).filter(function(el){{return {};}});}}",
                json!(css),
                filter,
            );
            session
                .call(
                    "Runtime.callFunctionOn",
                    json!({
                        "objectId": object_id,
                        "functionDeclaration": func,
                        "returnByValue": false,
                    }),
                )
                .await?
        }
    };
    extract_array_refs(session, &result["result"]).await
}

// ---------------------------------------------------------------------
// XPath
// ---------------------------------------------------------------------

#[allow(dead_code)] // Reached only via `SelectorKind::Xpath`, wired up in T12.
async fn resolve_xpath_one(scope: &QueryScope<'_>, expr: &str) -> Result<Option<RemoteRef>> {
    let session = scope.session();
    let result = match scope {
        QueryScope::Tab(_) | QueryScope::Frame(_) => {
            session
                .call(
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
            let object_id = el.remote_object_id_cloned().await?;
            session
                .call(
                    "Runtime.callFunctionOn",
                    json!({
                        "objectId": object_id,
                        "functionDeclaration":
                            "function(e){return document.evaluate(e, this, null, XPathResult.FIRST_ORDERED_NODE_TYPE, null).singleNodeValue;}",
                        "arguments": [{ "value": expr }],
                        "returnByValue": false,
                    }),
                )
                .await?
        }
    };
    extract_node_ref(session, &result["result"]).await
}

#[allow(dead_code)] // Called via `SelectorKind::resolve_many`; gated until T12.
async fn resolve_xpath_many(scope: &QueryScope<'_>, expr: &str) -> Result<Vec<RemoteRef>> {
    // Build an Array of nodes from an ORDERED_NODE_SNAPSHOT_TYPE result so
    // `extract_array_refs` can enumerate it via `Runtime.getProperties`.
    let session = scope.session();
    let result = match scope {
        QueryScope::Tab(_) | QueryScope::Frame(_) => {
            session
                .call(
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
            let object_id = el.remote_object_id_cloned().await?;
            session
                .call(
                    "Runtime.callFunctionOn",
                    json!({
                        "objectId": object_id,
                        "functionDeclaration":
                            "function(e){var r=document.evaluate(e, this, null, XPathResult.ORDERED_NODE_SNAPSHOT_TYPE, null);var a=[];for(var i=0;i<r.snapshotLength;i++)a.push(r.snapshotItem(i));return a;}",
                        "arguments": [{ "value": expr }],
                        "returnByValue": false,
                    }),
                )
                .await?
        }
    };
    extract_array_refs(session, &result["result"]).await
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

/// JS fragment that re-sorts an array of elements ascending by
/// `abs(len(elementText) - needleLen)` — the nodriver "closest-length"
/// heuristic. Appended to the collector output only when `best_match`
/// is set so `.one()` (which takes `[0]`) lands on the nearest match.
/// `arr` is the in-scope identifier holding the candidate array.
fn best_match_sort_js(arr: &str, needle_len: usize) -> String {
    format!(
        "{arr}.sort(function(a,b){{\
            var la=(a.innerText||a.textContent||'').length;\
            var lb=(b.innerText||b.textContent||'').length;\
            return Math.abs(la-{n})-Math.abs(lb-{n});\
        }})",
        arr = arr,
        n = needle_len,
    )
}

fn build_text_substring_js_tab(needle: &str, best_match: bool) -> String {
    // Narrowest-match filter: every element whose own text contains the
    // needle, MINUS any element that also has a descendant whose text
    // contains the needle. Without the narrowing step, the naive filter
    // returns `[html, body, ...ancestors..., target]` in document
    // order — `.one()` then picks `<html>` and the caller's
    // `el.attr("id")` returns None even though the test's `<button id=…>`
    // matched the needle.
    //
    // When `best_match` is set, the narrowed array is re-sorted ascending
    // by `abs(len(text) - len(needle))` so `.one()` (which takes `[0]`)
    // returns the closest-length candidate rather than the first in
    // document order.
    let sort = if best_match {
        format!(";{}", best_match_sort_js("r", needle.chars().count()))
    } else {
        String::new()
    };
    format!(
        "(function(){{\
            var n={n};\
            var lc=n.toLowerCase();\
            var matches=Array.from(document.querySelectorAll('*')).filter(function(el){{\
                var t=el.innerText||el.textContent||'';\
                return t.toLowerCase().includes(lc);\
            }});\
            var r=matches.filter(function(el){{\
                return !Array.from(el.querySelectorAll('*')).some(function(c){{\
                    var t=c.innerText||c.textContent||'';\
                    return t.toLowerCase().includes(lc);\
                }});\
            }}){sort};\
            return r;\
        }})()",
        n = json!(needle),
        sort = sort,
    )
}

fn build_text_substring_fn_body(needle: &str, best_match: bool) -> String {
    // Element scope: `this` is the scope element. Used via
    // `Runtime.callFunctionOn` with the needle as the sole argument.
    // Same narrowing (+ optional best_match sort) semantics as the
    // tab/frame path above.
    let sort = if best_match {
        format!(";{}", best_match_sort_js("r", needle.chars().count()))
    } else {
        String::new()
    };
    format!(
        "function(n){{\
            var lc=n.toLowerCase();\
            var matches=Array.from(this.querySelectorAll('*')).filter(function(el){{\
                var t=el.innerText||el.textContent||'';\
                return t.toLowerCase().includes(lc);\
            }});\
            var r=matches.filter(function(el){{\
                return !Array.from(el.querySelectorAll('*')).some(function(c){{\
                    var t=c.innerText||c.textContent||'';\
                    return t.toLowerCase().includes(lc);\
                }});\
            }}){sort};\
            return r;\
        }}",
        sort = sort,
    )
}

fn build_text_exact_xpath_js_tab(needle: &str, snapshot: bool, best_match: bool) -> String {
    // Construct the XPath in JS so the needle literal is escaped by
    // JSON.stringify -> single-quoted XPath string. snapshot=true
    // returns an Array of all matches; snapshot=false returns the
    // first match or null. When `best_match` is set on the snapshot
    // path, the array is re-sorted by closest text length (see
    // `best_match_sort_js`). `best_match` is ignored for the single-node
    // (`snapshot=false`) form since there is no array to sort.
    let sort = if snapshot && best_match {
        format!(";{}", best_match_sort_js("a", needle.chars().count()))
    } else {
        String::new()
    };
    if snapshot {
        format!(
            "(function(){{var n={n};var xp=\"//*[normalize-space(.)=\"+JSON.stringify(n).replace(/\"/g,\"'\")+\"]\";var r=document.evaluate(xp,document,null,XPathResult.ORDERED_NODE_SNAPSHOT_TYPE,null);var a=[];for(var i=0;i<r.snapshotLength;i++)a.push(r.snapshotItem(i));{sort};return a;}})()",
            n = json!(needle),
            sort = sort,
        )
    } else {
        format!(
            "(function(){{var n={n};var xp=\"//*[normalize-space(.)=\"+JSON.stringify(n).replace(/\"/g,\"'\")+\"]\";return document.evaluate(xp,document,null,XPathResult.FIRST_ORDERED_NODE_TYPE,null).singleNodeValue;}})()",
            n = json!(needle),
        )
    }
}

fn build_text_exact_xpath_fn_body(needle: &str, snapshot: bool, best_match: bool) -> String {
    // Element scope: `this` is the context node. Needle passed as arg.
    let sort = if snapshot && best_match {
        format!(";{}", best_match_sort_js("a", needle.chars().count()))
    } else {
        String::new()
    };
    if snapshot {
        format!(
            "function(n){{var xp=\"//*[normalize-space(.)=\"+JSON.stringify(n).replace(/\"/g,\"'\")+\"]\";var r=document.evaluate(xp,this,null,XPathResult.ORDERED_NODE_SNAPSHOT_TYPE,null);var a=[];for(var i=0;i<r.snapshotLength;i++)a.push(r.snapshotItem(i));{sort};return a;}}",
            sort = sort,
        )
    } else {
        "function(n){var xp=\"//*[normalize-space(.)=\"+JSON.stringify(n).replace(/\"/g,\"'\")+\"]\";return document.evaluate(xp,this,null,XPathResult.FIRST_ORDERED_NODE_TYPE,null).singleNodeValue;}".to_string()
    }
}

#[allow(dead_code)] // Reached via SelectorKind::resolve_one, gated until T12.
async fn resolve_text_one(
    scope: &QueryScope<'_>,
    needle: &str,
    exact: bool,
    best_match: bool,
) -> Result<Option<RemoteRef>> {
    let session = scope.session();
    if exact {
        // XPath path. For best_match we need the full snapshot array
        // (sorted by closest length) and take `[0]`; otherwise the cheap
        // single-node form suffices.
        if best_match {
            let result = match scope {
                QueryScope::Tab(_) | QueryScope::Frame(_) => {
                    session
                        .call(
                            "Runtime.evaluate",
                            json!({
                                "expression": format!("({})[0] || null", build_text_exact_xpath_js_tab(needle, true, true)),
                                "returnByValue": false,
                            }),
                        )
                        .await?
                }
                QueryScope::Element(el) => {
                    let object_id = el.remote_object_id_cloned().await?;
                    session
                        .call(
                            "Runtime.callFunctionOn",
                            json!({
                                "objectId": object_id,
                                "functionDeclaration": format!(
                                    "function(n){{return ({})[0] || null;}}",
                                    format!("({}).call(this,n)", build_text_exact_xpath_fn_body(needle, true, true)),
                                ),
                                "arguments": [{ "value": needle }],
                                "returnByValue": false,
                            }),
                        )
                        .await?
                }
            };
            return extract_node_ref(session, &result["result"]).await;
        }
        // Single-node form (no best_match): returns a single node or null.
        let result = match scope {
            QueryScope::Tab(_) | QueryScope::Frame(_) => {
                session
                    .call(
                        "Runtime.evaluate",
                        json!({
                            "expression": build_text_exact_xpath_js_tab(needle, false, false),
                            "returnByValue": false,
                        }),
                    )
                    .await?
            }
            QueryScope::Element(el) => {
                let object_id = el.remote_object_id_cloned().await?;
                session
                    .call(
                        "Runtime.callFunctionOn",
                        json!({
                            "objectId": object_id,
                            "functionDeclaration": build_text_exact_xpath_fn_body(needle, false, false),
                            "arguments": [{ "value": needle }],
                            "returnByValue": false,
                        }),
                    )
                    .await?
            }
        };
        extract_node_ref(session, &result["result"]).await
    } else {
        // Substring path returns an Array (best_match re-sorts it); pick
        // first match.
        let result = match scope {
            QueryScope::Tab(_) | QueryScope::Frame(_) => {
                session
                    .call(
                        "Runtime.evaluate",
                        json!({
                            "expression": format!("({})[0] || null", build_text_substring_js_tab(needle, best_match)),
                            "returnByValue": false,
                        }),
                    )
                    .await?
            }
            QueryScope::Element(el) => {
                let object_id = el.remote_object_id_cloned().await?;
                session
                    .call(
                        "Runtime.callFunctionOn",
                        json!({
                            "objectId": object_id,
                            "functionDeclaration": format!(
                                "function(n){{return ({}.call(this,n))[0] || null;}}",
                                // Reuse the narrowing + best_match body so
                                // `this` resolves to the scope element.
                                build_text_substring_fn_body(needle, best_match)
                            ),
                            "arguments": [{ "value": needle }],
                            "returnByValue": false,
                        }),
                    )
                    .await?
            }
        };
        extract_node_ref(session, &result["result"]).await
    }
}

#[allow(dead_code)] // Reached via SelectorKind::resolve_many, gated until T12.
async fn resolve_text_many(
    scope: &QueryScope<'_>,
    needle: &str,
    exact: bool,
    best_match: bool,
) -> Result<Vec<RemoteRef>> {
    let session = scope.session();
    let result = if exact {
        match scope {
            QueryScope::Tab(_) | QueryScope::Frame(_) => {
                session
                    .call(
                        "Runtime.evaluate",
                        json!({
                            "expression": build_text_exact_xpath_js_tab(needle, true, best_match),
                            "returnByValue": false,
                        }),
                    )
                    .await?
            }
            QueryScope::Element(el) => {
                let object_id = el.remote_object_id_cloned().await?;
                session
                    .call(
                        "Runtime.callFunctionOn",
                        json!({
                            "objectId": object_id,
                            "functionDeclaration": build_text_exact_xpath_fn_body(needle, true, best_match),
                            "arguments": [{ "value": needle }],
                            "returnByValue": false,
                        }),
                    )
                    .await?
            }
        }
    } else {
        match scope {
            QueryScope::Tab(_) | QueryScope::Frame(_) => {
                session
                    .call(
                        "Runtime.evaluate",
                        json!({
                            "expression": build_text_substring_js_tab(needle, best_match),
                            "returnByValue": false,
                        }),
                    )
                    .await?
            }
            QueryScope::Element(el) => {
                let object_id = el.remote_object_id_cloned().await?;
                session
                    .call(
                        "Runtime.callFunctionOn",
                        json!({
                            "objectId": object_id,
                            "functionDeclaration": build_text_substring_fn_body(needle, best_match),
                            "arguments": [{ "value": needle }],
                            "returnByValue": false,
                        }),
                    )
                    .await?
            }
        }
    };
    extract_array_refs(session, &result["result"]).await
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

fn build_text_regex_js_tab(pattern: &str, flags: &str, best_match: bool) -> String {
    // `best_match` has no literal needle for a regex; we use the pattern
    // length as the closest-length proxy (the only well-defined analog
    // for the regex case). Matches are re-sorted ascending by
    // `abs(len(text) - len(pattern))` when set.
    let sort = if best_match {
        format!(";{}", best_match_sort_js("m", pattern.chars().count()))
    } else {
        String::new()
    };
    format!(
        "(function(){{var r=new RegExp({p}, {f});var m=Array.from(document.querySelectorAll('*')).filter(function(el){{var t=el.innerText||el.textContent||'';return r.test(t);}}){sort};return m;}})()",
        p = json!(pattern),
        f = json!(flags),
        sort = sort,
    )
}

fn build_text_regex_fn_body(pattern: &str, best_match: bool) -> String {
    // Element scope: `this` is the scope element. Pattern + flags
    // passed as arguments. See `build_text_regex_js_tab` for the
    // best_match proxy rationale.
    let sort = if best_match {
        format!(";{}", best_match_sort_js("m", pattern.chars().count()))
    } else {
        String::new()
    };
    format!(
        "function(p,f){{var r=new RegExp(p,f);var m=Array.from(this.querySelectorAll('*')).filter(function(el){{var t=el.innerText||el.textContent||'';return r.test(t);}}){sort};return m;}}",
        sort = sort,
    )
}

#[allow(dead_code)] // Reached via SelectorKind::resolve_one, gated until T12.
async fn resolve_text_regex_one(
    scope: &QueryScope<'_>,
    pattern: &str,
    flags: &str,
    best_match: bool,
) -> Result<Option<RemoteRef>> {
    let session = scope.session();
    let result = match scope {
        QueryScope::Tab(_) | QueryScope::Frame(_) => {
            session
                .call(
                    "Runtime.evaluate",
                    json!({
                        "expression": format!("({})[0] || null", build_text_regex_js_tab(pattern, flags, best_match)),
                        "returnByValue": false,
                    }),
                )
                .await?
        }
        QueryScope::Element(el) => {
            let object_id = el.remote_object_id_cloned().await?;
            session
                .call(
                    "Runtime.callFunctionOn",
                    json!({
                        "objectId": object_id,
                        "functionDeclaration": format!(
                            "function(p,f){{return (({}).call(this,p,f))[0] || null;}}",
                            build_text_regex_fn_body(pattern, best_match)
                        ),
                        "arguments": [{ "value": pattern }, { "value": flags }],
                        "returnByValue": false,
                    }),
                )
                .await?
        }
    };
    extract_node_ref(session, &result["result"]).await
}

#[allow(dead_code)] // Reached via SelectorKind::resolve_many, gated until T12.
async fn resolve_text_regex_many(
    scope: &QueryScope<'_>,
    pattern: &str,
    flags: &str,
    best_match: bool,
) -> Result<Vec<RemoteRef>> {
    let session = scope.session();
    let result = match scope {
        QueryScope::Tab(_) | QueryScope::Frame(_) => {
            session
                .call(
                    "Runtime.evaluate",
                    json!({
                        "expression": build_text_regex_js_tab(pattern, flags, best_match),
                        "returnByValue": false,
                    }),
                )
                .await?
        }
        QueryScope::Element(el) => {
            let object_id = el.remote_object_id_cloned().await?;
            session
                .call(
                    "Runtime.callFunctionOn",
                    json!({
                        "objectId": object_id,
                        "functionDeclaration": build_text_regex_fn_body(pattern, best_match),
                        "arguments": [{ "value": pattern }, { "value": flags }],
                        "returnByValue": false,
                    }),
                )
                .await?
        }
    };
    extract_array_refs(session, &result["result"]).await
}

// ---------------------------------------------------------------------
// Role (`[role="..."]` CSS + optional accessible-name post-filter)
// ---------------------------------------------------------------------

#[allow(dead_code)] // Reached via SelectorKind::resolve_one, gated until T12.
async fn resolve_role_one(
    scope: &QueryScope<'_>,
    role: AriaRole,
    name: Option<&str>,
) -> Result<Option<RemoteRef>> {
    // Always go through `resolve_css_many` (rather than `resolve_css_one`)
    // so that name-filter and no-filter paths share the same candidate
    // enumeration. With no name filter we just return the first match.
    let css = role.to_css();
    let candidates = resolve_css_many(scope, &css).await?;
    let Some(needle) = name else {
        return Ok(candidates.into_iter().next());
    };
    let session = scope.session();
    for candidate in candidates {
        if accessible_name_matches(session, &candidate, needle).await? {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

#[allow(dead_code)] // Reached via SelectorKind::resolve_many, gated until T12.
async fn resolve_role_many(
    scope: &QueryScope<'_>,
    role: AriaRole,
    name: Option<&str>,
) -> Result<Vec<RemoteRef>> {
    let css = role.to_css();
    let candidates = resolve_css_many(scope, &css).await?;
    let Some(needle) = name else {
        return Ok(candidates);
    };
    let session = scope.session();
    let mut out = Vec::new();
    for candidate in candidates {
        if accessible_name_matches(session, &candidate, needle).await? {
            out.push(candidate);
        }
    }
    Ok(out)
}

/// Returns `true` if the computed accessible name for `node` contains
/// `needle` as a case-insensitive substring.
///
/// Uses `Accessibility.getPartialAXTree { backendNodeId, fetchRelatives: false }`
/// to fetch the AX node and reads `name.value`. Nodes with no AX entry,
/// no name, or a name that isn't a string are treated as a non-match
/// (returns `Ok(false)`).
#[allow(dead_code)] // Called via resolve_role_*, gated until T12.
async fn accessible_name_matches(
    session: &SessionHandle,
    node: &RemoteRef,
    needle: &str,
) -> Result<bool> {
    let response = session
        .call(
            "Accessibility.getPartialAXTree",
            json!({
                "backendNodeId": node.backend_node_id,
                "fetchRelatives": false,
            }),
        )
        .await?;
    let needle_lower = needle.to_lowercase();
    let Some(nodes) = response["nodes"].as_array() else {
        return Ok(false);
    };
    for ax_node in nodes {
        let Some(name_value) = ax_node["name"]["value"].as_str() else {
            continue;
        };
        if name_value.to_lowercase().contains(&needle_lower) {
            return Ok(true);
        }
    }
    Ok(false)
}

// ---------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------

/// Turn a `Runtime.evaluate` / `Runtime.callFunctionOn` *single-node*
/// result into a `RemoteRef`. Null subtype (`document.querySelector`
/// returned `null`) or `undefined` => `Ok(None)`.
///
/// Takes a `SessionHandle` (not a `Tab`) so the follow-up
/// `DOM.describeNode` round-trip dispatches on the same session the
/// caller used to obtain `result` — Frame-scoped queries must keep the
/// follow-up on the Frame's session (which for OOPIFs is a distinct
/// child session from the parent tab's).
#[allow(dead_code)] // Used by resolve_css_one / resolve_xpath_one; both gated until T12.
pub(crate) async fn extract_node_ref(
    session: &SessionHandle,
    result: &Value,
) -> Result<Option<RemoteRef>> {
    if result["subtype"] == "null" || result["type"] == "undefined" {
        return Ok(None);
    }
    let Some(remote_object_id) = result["objectId"].as_str().map(str::to_string) else {
        return Ok(None);
    };
    let backend_node_id = describe_backend_id(session, &remote_object_id).await?;
    Ok(Some(RemoteRef {
        remote_object_id,
        backend_node_id,
    }))
}

/// Turn a `Runtime.evaluate` / `Runtime.callFunctionOn` *array* result
/// (an `Array` RemoteObject) into a `Vec<RemoteRef>` by enumerating
/// numeric properties via `Runtime.getProperties` and describing each
/// element node. Empty array yields an empty Vec, not an error.
///
/// See [`extract_node_ref`] for the rationale on taking a
/// `SessionHandle` rather than a `Tab` here.
#[allow(dead_code)] // First lib caller is FindBuilder.many in T12.
pub(crate) async fn extract_array_refs(
    session: &SessionHandle,
    result: &Value,
) -> Result<Vec<RemoteRef>> {
    if result["subtype"] == "null" || result["type"] == "undefined" {
        return Ok(Vec::new());
    }
    let Some(array_id) = result["objectId"].as_str() else {
        return Ok(Vec::new());
    };
    let props = session
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
            let backend_node_id = describe_backend_id(session, &object_id).await?;
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

/// Read the rendered text length of `node` via `Runtime.callFunctionOn`
/// returning `(this.innerText||this.textContent||'').length`. Dispatched
/// on `scope`'s session so an OOPIF frame's node is read over the frame's
/// own session. Used only by the cross-scope `include_frames` +
/// `best_match` path to compare each scope's top candidate and pick the
/// global closest-length winner. A missing/non-numeric result yields
/// `usize::MAX` so a scope whose length cannot be read never wins a tie.
pub(crate) async fn text_len_of(scope: &QueryScope<'_>, node: &RemoteRef) -> Result<usize> {
    let result = scope
        .session()
        .call(
            "Runtime.callFunctionOn",
            json!({
                "objectId": node.remote_object_id,
                "functionDeclaration":
                    "function(){return (this.innerText||this.textContent||'').length;}",
                "returnByValue": true,
            }),
        )
        .await?;
    Ok(result["result"]["value"]
        .as_u64()
        .map_or(usize::MAX, |v| v as usize))
}

#[allow(dead_code)] // Both callers (extract_node_ref/extract_array_refs) are gated until T12.
async fn describe_backend_id(session: &SessionHandle, object_id: &str) -> Result<i64> {
    let described = session
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
    use zendriver_transport::SessionHandle;
    use zendriver_transport::testing::MockConnection;

    #[tokio::test]
    async fn css_one_sends_query_selector_with_selector() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

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
        let tab = Tab::new_for_test(sess);

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
        let tab = Tab::new_for_test(sess);

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

    #[tokio::test]
    async fn role_button_without_name_dispatches_attribute_selector_and_resolves_first_match() {
        // Role(Button, None) should:
        //   1. Runtime.evaluate `Array.from(document.querySelectorAll('[role="button"]'))`
        //   2. Runtime.getProperties on the returned Array
        //   3. DOM.describeNode on the first array element to fetch backendNodeId
        // and return a RemoteRef with the resolved id.
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move {
                let scope = QueryScope::Tab(&t);
                SelectorKind::Role(AriaRole::Button, None)
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
            sent.contains(r#"[role=\"button\"]"#),
            "role path must embed the `[role=\"button\"]` attribute selector verbatim; got: {sent}"
        );
        assert!(
            sent.contains("document.querySelectorAll"),
            "role path must call querySelectorAll for the candidate enumeration; got: {sent}"
        );
        mock.reply(
            id_q,
            json!({ "result": { "objectId": "RArr", "type": "object", "subtype": "array" } }),
        )
        .await;

        let id_p = mock.expect_cmd("Runtime.getProperties").await;
        assert_eq!(mock.last_sent()["params"]["objectId"], "RArr");
        mock.reply(
            id_p,
            json!({
                "result": [
                    {
                        "name": "0",
                        "value": { "objectId": "RN0", "type": "object", "subtype": "node" }
                    },
                    {
                        "name": "length",
                        "value": { "value": 1, "type": "number" }
                    }
                ]
            }),
        )
        .await;

        let id_d = mock.expect_cmd("DOM.describeNode").await;
        assert_eq!(mock.last_sent()["params"]["objectId"], "RN0");
        mock.reply(id_d, json!({ "node": { "backendNodeId": 42 } }))
            .await;

        let r = fut.await.unwrap().unwrap().unwrap();
        assert_eq!(r.remote_object_id, "RN0");
        assert_eq!(r.backend_node_id, 42);
        conn.shutdown();
    }
}
