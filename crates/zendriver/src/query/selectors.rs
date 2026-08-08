//! Selector kinds + CDP/JS resolution. Covers CSS, XPath, Text,
//! TextRegex, and Role selectors.
//!
//! Every entry point resolves through `SelectorKind::resolve_many_inner`:
//! `FindBuilder`'s `.one()` takes the best-match head, `FindAllBuilder`'s
//! `.many()` returns the whole vec, and `Element::refresh` drives
//! `resolve_many`. There is deliberately **no** single-match resolver
//! family. One existed until it was deleted: a parallel `resolve_one` /
//! `resolve_*_one` set that production never called, kept alive by
//! `#[allow(dead_code)]` and pointed at by six tests. It silently drifted
//! from the shipping path — a needle-normalization fix landed in both
//! copies, and only the dead one was covered, so deleting the fix from the
//! live path left the whole suite green. Resolver tests must therefore
//! drive `resolve_many` / `resolve_many_inner`; if a single-match
//! specialization is ever wanted for its cheaper wire shape, it belongs
//! behind `resolve_many_inner` rather than beside it.
//!
//! Role resolution: role-only queries compile to a `[role="..."]`
//! CSS attribute selector and reuse `resolve_css_many`
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
pub(crate) struct RemoteRef {
    pub(crate) remote_object_id: String,
    pub(crate) backend_node_id: i64,
}

/// What the query runs against: a whole tab (root = `document`), a
/// subtree rooted at an existing element (root = `this`), or a specific
/// frame (root = the frame's `document`, dispatched on the frame's own
/// CDP session — distinct from the parent tab's session for OOPIFs).
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
pub(crate) enum SelectorKind {
    Css(String),
    Xpath(String),
    Text { needle: String, exact: bool },
    TextRegex { pattern: String, flags: String },
    Role(AriaRole, Option<String>),
}

impl SelectorKind {
    /// Resolve this selector against `scope` and return every match in
    /// document order. Empty `Vec` for no matches (not an error).
    ///
    /// This is the only resolution entry point. `.one()` is
    /// [`Self::resolve_many_inner`] plus a take-first, so there is no
    /// separate single-match implementation that could drift from it —
    /// see the module header.
    pub(crate) async fn resolve_many(&self, scope: &QueryScope<'_>) -> Result<Vec<RemoteRef>> {
        self.resolve_many_inner(scope, false).await
    }

    /// `resolve_many` with the cross-cutting `best_match` flag. It only
    /// affects text selectors (`Text` / `TextRegex`), where the JS
    /// collector re-sorts candidates by closest text length; it is a
    /// no-op for css/xpath/role. When set, the returned Vec is ordered
    /// closest-length first, so `.one()` taking `[0]` lands on the
    /// nearest match.
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
// Shared eval-in-scope helper
// ---------------------------------------------------------------------

/// Evaluate `expression` via `Runtime.evaluate`, pinned to `scope`'s
/// execution context.
///
/// For a `Frame` scope this sets `contextId` to the frame's isolated-world
/// context (see [`QueryScope::execution_context_id`]) so a bare
/// `document`/`document.querySelectorAll(...)` inside `expression`
/// resolves against the *frame's* document rather than the parent tab's
/// default context — the bug this helper exists to prevent recurring.
/// For `Tab`/`Element` scope `execution_context_id()` is always `None`,
/// so `contextId` is omitted and CDP falls back to the session's default
/// (main-frame main-world) context, matching prior behavior exactly.
///
/// Every `Tab`/`Frame` match arm below (css/xpath/text/text_regex/
/// predicate) routes through this one function instead of hand-rolling the
/// `contextId` dance per resolver.
async fn eval_expr_in_scope(scope: &QueryScope<'_>, expression: String) -> Result<Value> {
    let session = scope.session();
    let ctx = scope.execution_context_id().await?;
    let mut params = json!({
        "expression": expression,
        "returnByValue": false,
    });
    if let Some(id) = ctx {
        params["contextId"] = json!(id);
    }
    Ok(session.call("Runtime.evaluate", params).await?)
}

// ---------------------------------------------------------------------
// CSS
// ---------------------------------------------------------------------

async fn resolve_css_many(scope: &QueryScope<'_>, selector: &str) -> Result<Vec<RemoteRef>> {
    let session = scope.session();
    let result = match scope {
        QueryScope::Tab(_) | QueryScope::Frame(_) => {
            eval_expr_in_scope(
                scope,
                format!("Array.from(document.querySelectorAll({}))", json!(selector)),
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
    let css = pred.to_css_selector();
    let filter = pred.to_js_filter();
    let result = match scope {
        QueryScope::Tab(_) | QueryScope::Frame(_) => {
            let expr = format!(
                "Array.from(document.querySelectorAll({})).filter(function(el){{return {};}})",
                json!(css),
                filter,
            );
            eval_expr_in_scope(scope, expr).await?
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

async fn resolve_xpath_many(scope: &QueryScope<'_>, expr: &str) -> Result<Vec<RemoteRef>> {
    // Build an Array of nodes from an ORDERED_NODE_SNAPSHOT_TYPE result so
    // `extract_array_refs` can enumerate it via `Runtime.getProperties`.
    let session = scope.session();
    let result = match scope {
        QueryScope::Tab(_) | QueryScope::Frame(_) => {
            eval_expr_in_scope(
                scope,
                format!(
                    "(function(){{var r=document.evaluate({}, document, null, XPathResult.ORDERED_NODE_SNAPSHOT_TYPE, null);var a=[];for(var i=0;i<r.snapshotLength;i++)a.push(r.snapshotItem(i));return a;}})()",
                    json!(expr)
                ),
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
//   - exact=true  -> an XPath built in Rust by `text_exact_xpath` and
//     evaluated with `ORDERED_NODE_SNAPSHOT_TYPE`. The needle is rendered as
//     an XPath literal by `xpath_string_literal` (which has the escaping
//     rule and the `concat()` fallback — do not restate it here). Tab and
//     frame scope emit `//*` against `document`; element scope emits the
//     relative `.//*` against `this`, or the walk leaves the element's
//     subtree.
//   - exact=false -> JS tree walk:
//     `Array.from(ctx.querySelectorAll('*')).filter(el => (el.innerText||el.textContent).toLowerCase().includes(needle.toLowerCase()))`.
//     Both paths return an array; `.one()` takes its first element.
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

/// Render `s` as an XPath 1.0 string literal, quotes and all.
///
/// XPath 1.0 has no escape mechanism inside a string literal, so a needle
/// containing both quote kinds cannot be written as one literal at all;
/// `concat()` over alternating pieces is the standard workaround.
///
/// Reaching the `concat()` branch means `s` carries both quote kinds, so
/// splitting on `"` yields at least two chunks and the loop pushes at least
/// one `'"'` separator alongside the chunk holding the apostrophe:
/// `concat()`'s two-argument minimum is satisfied by construction, not by a
/// guard.
///
/// This replaced `JSON.stringify(n).replace(/"/g,"'")`, which was not an
/// escaping strategy but a quote swap — it only moved which character
/// broke the expression. `text_exact("it's")` built
/// `//*[normalize-space(.)='it's']`, whose literal ends at the third
/// quote, so `document.evaluate` threw a `SyntaxError` and the caller got
/// a `JsException` instead of a match or an empty result. Double quotes
/// fared no better: `JSON.stringify` escaped an embedded `"` as `\"` and
/// the swap turned that into `\'`, which XPath 1.0 does not recognize.
fn xpath_string_literal(s: &str) -> String {
    if !s.contains('"') {
        return format!("\"{s}\"");
    }
    if !s.contains('\'') {
        return format!("'{s}'");
    }
    // Both kinds present. Splitting on `"` leaves pieces that contain
    // none, so each is safe inside double quotes; the separators are
    // re-introduced as single-quoted `"` characters.
    let chunks: Vec<&str> = s.split('"').collect();
    let mut parts: Vec<String> = Vec::new();
    for (i, chunk) in chunks.iter().enumerate() {
        if !chunk.is_empty() {
            parts.push(format!("\"{chunk}\""));
        }
        if i + 1 < chunks.len() {
            parts.push("'\"'".to_string());
        }
    }
    debug_assert!(parts.len() >= 2, "concat() needs two arguments: {parts:?}");
    format!("concat({})", parts.join(","))
}

/// Which node a text-exact expression walks down from.
///
/// `//` abbreviates `/descendant-or-self::node()/`, and that leading `/` is
/// the root of the document containing the context node — so `//*` ignores
/// whatever node `document.evaluate` was handed and enumerates the whole
/// page. `.//` starts the same walk at the context node. The distinction is
/// invisible until an element-scoped query returns a match from somewhere
/// else on the page.
#[derive(Clone, Copy)]
enum TextExactAnchor {
    /// Whole document. Tab and frame scope, evaluated against `document`.
    Document,
    /// The context node's own subtree. Element scope, evaluated against
    /// `this`.
    ContextNode,
}

impl TextExactAnchor {
    fn prefix(self) -> &'static str {
        match self {
            Self::Document => "//",
            Self::ContextNode => ".//",
        }
    }
}

/// The `//*[normalize-space(.)=<literal>]` expression for `needle`, or its
/// relative `.//*` form (see [`TextExactAnchor`]), with the needle rendered
/// by [`xpath_string_literal`]. Built here in Rust rather than assembled on
/// the page so the quoting is unit-testable without a browser.
fn text_exact_xpath(needle: &str, anchor: TextExactAnchor) -> String {
    format!(
        "{}*[normalize-space(.)={}]",
        anchor.prefix(),
        xpath_string_literal(needle)
    )
}

fn build_text_exact_xpath_js_tab(needle: &str, best_match: bool) -> String {
    // Snapshot form: returns an Array of every match. When `best_match` is
    // set the array is re-sorted by closest text length (see
    // `best_match_sort_js`).
    let sort = if best_match {
        format!(";{}", best_match_sort_js("a", needle.chars().count()))
    } else {
        String::new()
    };
    format!(
        "(function(){{var xp={xp};var r=document.evaluate(xp,document,null,XPathResult.ORDERED_NODE_SNAPSHOT_TYPE,null);var a=[];for(var i=0;i<r.snapshotLength;i++)a.push(r.snapshotItem(i));{sort};return a;}})()",
        xp = json!(text_exact_xpath(needle, TextExactAnchor::Document)),
        sort = sort,
    )
}

fn build_text_exact_xpath_fn_body(needle: &str, best_match: bool) -> String {
    // Element scope: `this` is the context node, so the expression has to be
    // relative or the walk runs over the whole document and the scope means
    // nothing. Declares no parameters — the needle is baked into the
    // expression, so the caller sends no `arguments` either.
    let sort = if best_match {
        format!(";{}", best_match_sort_js("a", needle.chars().count()))
    } else {
        String::new()
    };
    format!(
        "function(){{var xp={xp};var r=document.evaluate(xp,this,null,XPathResult.ORDERED_NODE_SNAPSHOT_TYPE,null);var a=[];for(var i=0;i<r.snapshotLength;i++)a.push(r.snapshotItem(i));{sort};return a;}}",
        xp = json!(text_exact_xpath(needle, TextExactAnchor::ContextNode)),
        sort = sort,
    )
}

async fn resolve_text_many(
    scope: &QueryScope<'_>,
    needle: &str,
    exact: bool,
    best_match: bool,
) -> Result<Vec<RemoteRef>> {
    // XPath compares `normalize-space(.)` against the needle, so the page side
    // was folded but the needle was not: `text_exact("Hello   world")` could
    // never match anything. Fold it the same way, using the one rule
    // `TextPred::Equals` uses so the two stay in agreement. Substring and regex
    // needles keep their raw text — interior whitespace can be meaningful there.
    let normalized;
    let needle = if exact {
        normalized = crate::query::predicate::normalize_space(needle);
        normalized.as_str()
    } else {
        needle
    };
    let session = scope.session();
    let result = if exact {
        match scope {
            QueryScope::Tab(_) | QueryScope::Frame(_) => {
                eval_expr_in_scope(scope, build_text_exact_xpath_js_tab(needle, best_match)).await?
            }
            QueryScope::Element(el) => {
                let object_id = el.remote_object_id_cloned().await?;
                session
                    .call(
                        "Runtime.callFunctionOn",
                        json!({
                            "objectId": object_id,
                            "functionDeclaration": build_text_exact_xpath_fn_body(needle, best_match),
                            "returnByValue": false,
                        }),
                    )
                    .await?
            }
        }
    } else {
        match scope {
            QueryScope::Tab(_) | QueryScope::Frame(_) => {
                eval_expr_in_scope(scope, build_text_substring_js_tab(needle, best_match)).await?
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
//   `Array.from(ctx.querySelectorAll('*')).filter(el => new RegExp(pat, flags).test(el.innerText||el.textContent))`,
//   then narrowed to the innermost matching element (see
//   `regex_narrowing_js`) — an ancestor whose only text-bearing
//   descendant is the match otherwise has an identical `innerText` and
//   also passes the filter, ranking first in document order.
//
// The regex is *re-parsed* on the JS side via `new RegExp`, so the
// pattern must use JS-flavored regex syntax (which is essentially the
// same as Rust's `regex` crate for the common subset). Flags are passed
// verbatim — caller is responsible for valid JS flag chars (e.g. "i",
// "im", "gi", etc.). Empty flags string is fine.
//
// We construct the RegExp *once outside* the filter callback so the
// pattern is only compiled per query rather than per element.

/// JS fragment implementing the same "narrowest match" filter the
/// substring builders use (see `build_text_substring_js_tab`), but with
/// the regex predicate (`r.test(t)`, reusing the already-constructed
/// `RegExp` `r`) in place of `.includes(lc)`. `matches_var` is the
/// in-scope identifier holding the raw (un-narrowed) candidate array;
/// the fragment declares `narrowed` as the result with any element that
/// has a descendant also matching `r` subtracted out. Shared between
/// `build_text_regex_js_tab` and `build_text_regex_fn_body` (kept
/// separate from the substring builders' fragment so their existing,
/// tested JS output stays untouched).
fn regex_narrowing_js(matches_var: &str) -> String {
    format!(
        "var narrowed={m}.filter(function(el){{\
            return !Array.from(el.querySelectorAll('*')).some(function(c){{\
                var t=c.innerText||c.textContent||'';\
                return r.test(t);\
            }});\
        }})",
        m = matches_var,
    )
}

fn build_text_regex_js_tab(pattern: &str, flags: &str, best_match: bool) -> String {
    // `best_match` has no literal needle for a regex; we use the pattern
    // length as the closest-length proxy (the only well-defined analog
    // for the regex case). The NARROWED array is re-sorted ascending by
    // `abs(len(text) - len(pattern))` when set, mirroring the substring
    // builder's order-of-operations (narrow first, then sort the
    // leaves).
    let sort = if best_match {
        format!(
            ";{}",
            best_match_sort_js("narrowed", pattern.chars().count())
        )
    } else {
        String::new()
    };
    format!(
        "(function(){{\
            var r=new RegExp({p}, {f});\
            var m=Array.from(document.querySelectorAll('*')).filter(function(el){{\
                var t=el.innerText||el.textContent||'';\
                return r.test(t);\
            }});\
            {narrowing}{sort};\
            return narrowed;\
        }})()",
        p = json!(pattern),
        f = json!(flags),
        narrowing = regex_narrowing_js("m"),
        sort = sort,
    )
}

fn build_text_regex_fn_body(pattern: &str, best_match: bool) -> String {
    // Element scope: `this` is the scope element. Pattern + flags
    // passed as arguments. See `build_text_regex_js_tab` for the
    // best_match proxy rationale and the narrowing semantics.
    let sort = if best_match {
        format!(
            ";{}",
            best_match_sort_js("narrowed", pattern.chars().count())
        )
    } else {
        String::new()
    };
    format!(
        "function(p,f){{\
            var r=new RegExp(p,f);\
            var m=Array.from(this.querySelectorAll('*')).filter(function(el){{\
                var t=el.innerText||el.textContent||'';\
                return r.test(t);\
            }});\
            {narrowing}{sort};\
            return narrowed;\
        }}",
        narrowing = regex_narrowing_js("m"),
        sort = sort,
    )
}

async fn resolve_text_regex_many(
    scope: &QueryScope<'_>,
    pattern: &str,
    flags: &str,
    best_match: bool,
) -> Result<Vec<RemoteRef>> {
    let session = scope.session();
    let result = match scope {
        QueryScope::Tab(_) | QueryScope::Frame(_) => {
            eval_expr_in_scope(scope, build_text_regex_js_tab(pattern, flags, best_match)).await?
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

async fn resolve_role_many(
    scope: &QueryScope<'_>,
    role: AriaRole,
    name: Option<&str>,
) -> Result<Vec<RemoteRef>> {
    // Both the name-filter and the no-filter path share this one candidate
    // enumeration. Delegating to `resolve_css_many` also means role queries
    // inherit its `contextId` pinning for free — a Frame-scoped role query
    // correctly enumerates from the frame's own document, not the parent
    // tab's.
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
    use crate::test_support::expect;
    use zendriver_transport::SessionHandle;
    use zendriver_transport::testing::MockConnection;

    /// Drive the selector the way production does and hand back the JS that
    /// went out on the wire.
    ///
    /// Every resolver test goes through `resolve_many`, which is the only
    /// resolution entry point there is (`.one()` is this plus a take-first).
    /// The reply is a null subtype, which `extract_array_refs` short-circuits
    /// to an empty vec, so no `Runtime.getProperties` / `DOM.describeNode`
    /// dance is needed to inspect the dispatched expression.
    async fn dispatched_expression(kind: SelectorKind) -> String {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move {
                let scope = QueryScope::Tab(&t);
                kind.resolve_many(&scope).await
            }
        });

        let id = expect(&mut mock, "Runtime.evaluate").await;
        let expr = mock.last_sent()["params"]["expression"]
            .as_str()
            .unwrap()
            .to_string();
        mock.reply(
            id,
            json!({ "result": { "type": "object", "subtype": "null" } }),
        )
        .await;
        let hits = fut.await.unwrap().unwrap();
        assert!(hits.is_empty(), "null subtype must yield no matches");
        conn.shutdown();
        expr
    }

    /// A query that matches nothing resolves to an empty vec, through the
    /// real decode path.
    ///
    /// `dispatched_expression` above short-circuits with a `null` subtype,
    /// which `extract_array_refs` early-returns on before it ever calls
    /// `Runtime.getProperties`. That is the right shortcut for tests whose
    /// job is the dispatched expression, but it leaves the ordinary
    /// empty-result path — a genuine array object, enumerated and found to
    /// hold nothing but `length` — unexercised. This covers it.
    #[tokio::test]
    async fn an_empty_result_array_resolves_to_no_matches() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move {
                let scope = QueryScope::Tab(&t);
                SelectorKind::Css("#absent".into())
                    .resolve_many(&scope)
                    .await
            }
        });

        let id_q = expect(&mut mock, "Runtime.evaluate").await;
        mock.reply(
            id_q,
            json!({ "result": { "objectId": "REmpty", "type": "object", "subtype": "array" } }),
        )
        .await;

        // A real (empty) array: `length` only, no numeric entries. The
        // resolver must enumerate it and stop, without a `DOM.describeNode`.
        let id_p = expect(&mut mock, "Runtime.getProperties").await;
        assert_eq!(mock.last_sent()["params"]["objectId"], "REmpty");
        mock.reply(
            id_p,
            json!({ "result": [{ "name": "length", "value": { "value": 0, "type": "number" } }] }),
        )
        .await;

        let hits = fut.await.unwrap().unwrap();
        assert!(hits.is_empty(), "an empty array must yield no matches");
        conn.shutdown();
    }

    /// The XPath folds the page side with `normalize-space(.)`, so an
    /// author-written run of spaces in the needle could never match. Both
    /// sides must be folded by the same rule `TextPred::Equals` uses.
    ///
    /// This drives `resolve_many`, the path `.one()` and `.many()` both
    /// take. An earlier version of this test drove a single-match resolver
    /// that production never called, so deleting the fold from the shipping
    /// path left it green; that resolver is gone (see the module header).
    #[tokio::test]
    async fn text_exact_folds_whitespace_in_the_needle_before_building_the_xpath() {
        let expr = dispatched_expression(SelectorKind::Text {
            needle: "  Hello \t\n  world  ".into(),
            exact: true,
        })
        .await;

        // The whole expression the builder should emit, not a substring of
        // it: asserting the exact fragment pins the folding *and* the
        // quoting, and subsumes a negative check for surviving raw tabs.
        assert!(
            expr.contains(r#"var xp="//*[normalize-space(.)=\"Hello world\"]";"#),
            "needle should be normalize-space folded before embedding, got: {expr}"
        );
    }

    /// XPath 1.0 string literals have no escape mechanism, so a needle
    /// carrying a quote has to change how the literal is built rather than
    /// be escaped inside it. Quote-swapping used to produce
    /// `//*[normalize-space(.)='it's']`, which `document.evaluate` rejects
    /// with a `SyntaxError` — the caller saw a `JsException` instead of a
    /// match or an empty result, and apostrophes are ordinary in button and
    /// link text.
    #[tokio::test]
    async fn text_exact_needle_with_an_apostrophe_builds_a_valid_xpath_literal() {
        let expr = dispatched_expression(SelectorKind::Text {
            needle: "it's".into(),
            exact: true,
        })
        .await;

        assert!(
            expr.contains(r#"var xp="//*[normalize-space(.)=\"it's\"]";"#),
            "an apostrophe needle must switch the literal to double quotes, got: {expr}"
        );
    }

    /// The double-quote case was broken by the same swap for a different
    /// reason: `JSON.stringify` escaped the embedded `"` as `\"` and the
    /// replace turned that into `\'`, which XPath 1.0 does not recognize as
    /// an escape at all.
    #[tokio::test]
    async fn text_exact_needle_with_a_double_quote_builds_a_valid_xpath_literal() {
        let expr = dispatched_expression(SelectorKind::Text {
            needle: r#"say "hi""#.into(),
            exact: true,
        })
        .await;

        assert!(
            expr.contains(r#"var xp="//*[normalize-space(.)='say \"hi\"']";"#),
            "a double-quote needle must switch the literal to single quotes, got: {expr}"
        );
    }

    /// A needle carrying both quote kinds cannot be written as a single
    /// XPath literal, so the builder has to fall back to `concat()`.
    #[tokio::test]
    async fn text_exact_needle_with_both_quote_kinds_falls_back_to_concat() {
        let expr = dispatched_expression(SelectorKind::Text {
            needle: r#"it's a "quote""#.into(),
            exact: true,
        })
        .await;

        assert!(
            expr.contains(
                r#"var xp="//*[normalize-space(.)=concat(\"it's a \",'\"',\"quote\",'\"')]";"#
            ),
            "a needle with both quote kinds must compile to concat(), got: {expr}"
        );
    }

    /// The two text-exact builders must not share an anchor.
    ///
    /// `//*` resolves against the document root whatever context node
    /// `document.evaluate` is handed, so the element-scoped builder has to
    /// emit `.//*` or its scoping is decorative. Pinned here as well as in
    /// `tests/element_world_regressions.rs` so a mis-wired anchor fails
    /// without a browser; only that test proves the anchors reach the nodes
    /// this one assumes they do.
    #[test]
    fn only_the_element_scoped_expression_is_relative_to_its_context_node() {
        let tab_scope = build_text_exact_xpath_js_tab("Cancel", false);
        assert!(
            tab_scope.contains(r#"var xp="//*[normalize-space(.)=\"Cancel\"]""#),
            "tab scope evaluates against `document` and stays document-anchored, got: {tab_scope}"
        );

        let element_scope = build_text_exact_xpath_fn_body("Cancel", false);
        assert!(
            element_scope.contains(r#"var xp=".//*[normalize-space(.)=\"Cancel\"]""#),
            "element scope evaluates against `this` and must be relative, got: {element_scope}"
        );
    }

    #[tokio::test]
    async fn css_sends_query_selector_all_and_resolves_each_hit() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move {
                let scope = QueryScope::Tab(&t);
                SelectorKind::Css("#btn".into()).resolve_many(&scope).await
            }
        });

        let id_q = expect(&mut mock, "Runtime.evaluate").await;
        let sent = mock.last_sent()["params"]["expression"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            sent.contains("document.querySelectorAll") && sent.contains("#btn"),
            "expression should call document.querySelectorAll with the selector, got: {sent}"
        );
        mock.reply(
            id_q,
            json!({ "result": { "objectId": "RArr", "type": "object", "subtype": "array" } }),
        )
        .await;

        let id_p = expect(&mut mock, "Runtime.getProperties").await;
        assert_eq!(mock.last_sent()["params"]["objectId"], "RArr");
        mock.reply(
            id_p,
            json!({
                "result": [
                    { "name": "0", "value": { "objectId": "R7", "type": "object", "subtype": "node" } },
                    { "name": "length", "value": { "value": 1, "type": "number" } }
                ]
            }),
        )
        .await;

        let id_d = expect(&mut mock, "DOM.describeNode").await;
        assert_eq!(mock.last_sent()["params"]["objectId"], "R7");
        mock.reply(id_d, json!({ "node": { "backendNodeId": 99 } }))
            .await;

        let hits = fut.await.unwrap().unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].remote_object_id, "R7");
        assert_eq!(hits[0].backend_node_id, 99);
        conn.shutdown();
    }

    /// Non-exact text selector: the dispatched JS must carry the
    /// lowercase-fold and the needle verbatim, so the case-insensitive
    /// substring contract survives. Unlike the exact path, the needle is
    /// *not* whitespace-folded — interior runs can be meaningful in a
    /// substring.
    #[tokio::test]
    async fn text_substring_eval_lowercases_and_includes_needle() {
        let expr = dispatched_expression(SelectorKind::Text {
            needle: "Sign In".into(),
            exact: false,
        })
        .await;

        assert!(
            expr.contains(".toLowerCase()"),
            "substring path must lowercase-fold both sides; got: {expr}"
        );
        assert!(
            expr.contains("Sign In"),
            "substring path must embed the needle verbatim; got: {expr}"
        );
        assert!(
            expr.contains(".includes("),
            "substring path must call .includes; got: {expr}"
        );
    }

    /// TextRegex selector: the dispatched JS builds `new RegExp(<pat>,
    /// <flags>)` with both strings present.
    #[tokio::test]
    async fn text_regex_eval_constructs_new_regexp_with_pattern_and_flags() {
        let expr = dispatched_expression(SelectorKind::TextRegex {
            pattern: "hello.*world".into(),
            flags: "im".into(),
        })
        .await;

        assert!(
            expr.contains("new RegExp"),
            "regex path must instantiate `new RegExp`; got: {expr}"
        );
        assert!(
            expr.contains("hello.*world"),
            "regex path must embed the pattern; got: {expr}"
        );
        assert!(
            expr.contains("im"),
            "regex path must embed the flags string; got: {expr}"
        );
    }

    /// Regression test: `.text_regex()` used to have no "narrowest match"
    /// step (unlike `.text()` / `.text_exact()`), so an ancestor
    /// (`<html>` / `<body>`) whose only text-bearing descendant is the real
    /// match ends up with an identical `innerText`, also passes the regex
    /// filter, ranks first in document order, and wins `.one()`'s `[0]`
    /// instead of the intended leaf. The dispatched expression must
    /// subtract any element that has a matching descendant (mirroring
    /// `build_text_substring_js_tab`'s narrowing), reusing the same
    /// `RegExp` object as the predicate.
    #[tokio::test]
    async fn text_regex_eval_narrows_to_innermost_matching_element() {
        let expr = dispatched_expression(SelectorKind::TextRegex {
            pattern: "unique-frame-text".into(),
            flags: "".into(),
        })
        .await;

        assert!(
            expr.contains(".querySelectorAll('*')).some("),
            "regex path must narrow via descendant-subtraction (some()); got: {expr}"
        );
        assert!(
            expr.matches("r.test(").count() >= 2,
            "narrowing predicate must reuse the same RegExp object `r` (once for the initial \
             filter, once for the descendant check); got: {expr}"
        );
    }

    #[tokio::test]
    async fn role_button_without_name_dispatches_attribute_selector_and_resolves_hits() {
        // Role(Button, None) should:
        //   1. Runtime.evaluate `Array.from(document.querySelectorAll('[role="button"]'))`
        //   2. Runtime.getProperties on the returned Array
        //   3. DOM.describeNode on each array element to fetch backendNodeId
        // and return a RemoteRef per hit.
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move {
                let scope = QueryScope::Tab(&t);
                SelectorKind::Role(AriaRole::Button, None)
                    .resolve_many(&scope)
                    .await
            }
        });

        let id_q = expect(&mut mock, "Runtime.evaluate").await;
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

        let id_p = expect(&mut mock, "Runtime.getProperties").await;
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

        let id_d = expect(&mut mock, "DOM.describeNode").await;
        assert_eq!(mock.last_sent()["params"]["objectId"], "RN0");
        mock.reply(id_d, json!({ "node": { "backendNodeId": 42 } }))
            .await;

        let hits = fut.await.unwrap().unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].remote_object_id, "RN0");
        assert_eq!(hits[0].backend_node_id, 42);
        conn.shutdown();
    }

    // -------------------------------------------------------------------
    // Frame-scope `contextId` pinning (regression coverage for the bug
    // where xpath/text/text_regex `_many` resolvers silently queried the
    // main-frame document instead of the scoped frame's).
    // -------------------------------------------------------------------

    /// Build a synthetic `Frame` whose session sits on the supplied mock
    /// connection, with no parent tab/frame — sufficient for exercising
    /// `QueryScope::Frame` dispatch without a real `Tab`.
    fn frame_on(session: SessionHandle, frame_id: &str) -> Frame {
        Frame::new(
            frame_id.to_string(),
            None,
            String::new(),
            None,
            session,
            std::sync::Weak::new(),
        )
    }

    #[tokio::test]
    async fn xpath_many_frame_scope_pins_context_id() {
        // Frame scope must allocate/reuse the frame's isolated-world
        // context via `Page.createIsolatedWorld` and pin the follow-up
        // `Runtime.evaluate` to it via `contextId` — otherwise
        // `document.evaluate(...)` inside the expression walks the
        // parent tab's document instead of the frame's.
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let frame = frame_on(sess, "F1");

        let fut = tokio::spawn(async move {
            let scope = QueryScope::Frame(&frame);
            SelectorKind::Xpath("//button".into())
                .resolve_many(&scope)
                .await
        });

        let id_iso = mock.expect_cmd("Page.createIsolatedWorld").await;
        assert_eq!(mock.last_sent()["params"]["frameId"], "F1");
        mock.reply(id_iso, json!({ "executionContextId": 777 }))
            .await;

        let id_q = mock.expect_cmd("Runtime.evaluate").await;
        assert_eq!(
            mock.last_sent()["params"]["contextId"],
            777,
            "xpath_many must pin Runtime.evaluate to the frame's isolated-world contextId"
        );
        mock.reply(
            id_q,
            json!({ "result": { "type": "object", "subtype": "null" } }),
        )
        .await;

        let r = fut.await.unwrap().unwrap();
        assert!(r.is_empty(), "null subtype must yield an empty Vec");
        conn.shutdown();
    }

    #[tokio::test]
    async fn text_many_frame_scope_pins_context_id() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let frame = frame_on(sess, "F1");

        let fut = tokio::spawn(async move {
            let scope = QueryScope::Frame(&frame);
            SelectorKind::Text {
                needle: "hello".into(),
                exact: false,
            }
            .resolve_many(&scope)
            .await
        });

        let id_iso = mock.expect_cmd("Page.createIsolatedWorld").await;
        assert_eq!(mock.last_sent()["params"]["frameId"], "F1");
        mock.reply(id_iso, json!({ "executionContextId": 778 }))
            .await;

        let id_q = mock.expect_cmd("Runtime.evaluate").await;
        assert_eq!(
            mock.last_sent()["params"]["contextId"],
            778,
            "text_many must pin Runtime.evaluate to the frame's isolated-world contextId"
        );
        mock.reply(
            id_q,
            json!({ "result": { "type": "object", "subtype": "null" } }),
        )
        .await;

        let r = fut.await.unwrap().unwrap();
        assert!(r.is_empty(), "null subtype must yield an empty Vec");
        conn.shutdown();
    }

    #[tokio::test]
    async fn text_regex_many_frame_scope_pins_context_id() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let frame = frame_on(sess, "F1");

        let fut = tokio::spawn(async move {
            let scope = QueryScope::Frame(&frame);
            SelectorKind::TextRegex {
                pattern: "hello.*world".into(),
                flags: "i".into(),
            }
            .resolve_many(&scope)
            .await
        });

        let id_iso = mock.expect_cmd("Page.createIsolatedWorld").await;
        assert_eq!(mock.last_sent()["params"]["frameId"], "F1");
        mock.reply(id_iso, json!({ "executionContextId": 779 }))
            .await;

        let id_q = mock.expect_cmd("Runtime.evaluate").await;
        assert_eq!(
            mock.last_sent()["params"]["contextId"],
            779,
            "text_regex_many must pin Runtime.evaluate to the frame's isolated-world contextId"
        );
        mock.reply(
            id_q,
            json!({ "result": { "type": "object", "subtype": "null" } }),
        )
        .await;

        let r = fut.await.unwrap().unwrap();
        assert!(r.is_empty(), "null subtype must yield an empty Vec");
        conn.shutdown();
    }

    #[tokio::test]
    async fn role_many_frame_scope_pins_context_id_via_css_many_delegation() {
        // resolve_role_many delegates to resolve_css_many, which already
        // set contextId correctly before this fix. This test locks that
        // (already-correct) transitive behavior so a future refactor of
        // the role path can't silently drop it.
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let frame = frame_on(sess, "F1");

        let fut = tokio::spawn(async move {
            let scope = QueryScope::Frame(&frame);
            SelectorKind::Role(AriaRole::Button, None)
                .resolve_many(&scope)
                .await
        });

        let id_iso = mock.expect_cmd("Page.createIsolatedWorld").await;
        assert_eq!(mock.last_sent()["params"]["frameId"], "F1");
        mock.reply(id_iso, json!({ "executionContextId": 780 }))
            .await;

        let id_q = mock.expect_cmd("Runtime.evaluate").await;
        assert_eq!(
            mock.last_sent()["params"]["contextId"],
            780,
            "role_many must pin Runtime.evaluate to the frame's isolated-world contextId"
        );
        mock.reply(
            id_q,
            json!({ "result": { "type": "object", "subtype": "null" } }),
        )
        .await;

        let r = fut.await.unwrap().unwrap();
        assert!(r.is_empty(), "null subtype must yield an empty Vec");
        conn.shutdown();
    }
}
