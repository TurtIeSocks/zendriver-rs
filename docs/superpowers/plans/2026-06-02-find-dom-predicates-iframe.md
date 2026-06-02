# Find/DOM — Predicate Builder + Nested-iframe Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add bs4-like combinable predicate matchers (tag/attr/text) to `FindBuilder`/`FindAllBuilder`, plus `select`/`select_all` aliases, and verify+lock nested-iframe traversal.

**Architecture:** Predicates accumulate in a new `PredicateSet` (pure, unit-tested compilation to a CSS selector + a JS post-filter). The terminal compiles to ONE `Runtime.evaluate`/`callFunctionOn` per scope, reusing the existing `QueryScope` dispatch + `execution_context_id` (so it crosses frames) + `extract_array_refs`. Single-selector vs predicate mixing is a runtime error at the terminal (1A). Nested-iframe is verify-first: a test drives whether any fix is needed.

**Tech Stack:** Rust, `serde_json::json!` for safe JS-string embedding, CDP `Runtime.evaluate`/`callFunctionOn`, existing `QueryScope`/`RemoteRef`/`extract_array_refs` helpers in `query/selectors.rs`.

**Spec:** `docs/superpowers/specs/2026-06-02-find-dom-predicates-iframe-design.md`

---

## File Structure

- **Create** `crates/zendriver/src/query/predicate.rs` — `PredicateSet`, `AttrPred`, `TextPred`, `to_css_selector()`, `to_js_filter()`. Pure, no I/O, fully unit-tested.
- **Modify** `crates/zendriver/src/query/mod.rs` — add `predicates: PredicateSet` field + a `predicate_methods!` macro (10 methods) invoked on both builders; terminal conflict check + predicate dispatch.
- **Modify** `crates/zendriver/src/query/selectors.rs` — `resolve_predicate_one` / `resolve_predicate_many`.
- **Modify** `crates/zendriver/src/error.rs` — `ConflictingSelectors` variant.
- **Modify** `crates/zendriver/src/tab.rs`, `crates/zendriver/src/frame/mod.rs`, `crates/zendriver/src/element/mod.rs` — `select` / `select_all`.
- **Create** `crates/zendriver/tests/find_predicate_iframe.rs` — gated headful integration tests.
- **Modify** `CHANGELOG.md`.

---

## Task 1: `PredicateSet` types + CSS compilation

**Files:**
- Create: `crates/zendriver/src/query/predicate.rs`
- Modify: `crates/zendriver/src/query/mod.rs` (add `pub(crate) mod predicate;`)

- [ ] **Step 1: Write the failing test**

Create `crates/zendriver/src/query/predicate.rs`:
```rust
//! bs4-like combinable predicate matchers. A `PredicateSet` compiles to a
//! CSS selector (structural parts) + a JS boolean post-filter (regex/text).
//! Pure — no CDP, fully unit-testable.

use serde_json::json;

#[derive(Debug, Clone, Default)]
pub(crate) struct PredicateSet {
    pub(crate) tag: Option<String>,
    pub(crate) attrs: Vec<AttrPred>,
    pub(crate) texts: Vec<TextPred>,
}

#[derive(Debug, Clone)]
pub(crate) enum AttrPred {
    Exact(String, String),
    Contains(String, String),
    StartsWith(String, String),
    EndsWith(String, String),
    Has(String),
    Regex(String, String), // (name, pattern) — JS post-filter, not CSS
}

#[derive(Debug, Clone)]
pub(crate) enum TextPred {
    Contains(String),
    Equals(String),
    Matches(String), // regex pattern
}

/// Quote a value as a JSON string (`"v"`) — safe for both CSS attribute
/// values and JS string literals (the selector is later JSON-embedded into
/// the JS source, double-escaping correctly).
fn q(v: &str) -> String {
    json!(v).to_string()
}

impl PredicateSet {
    pub(crate) fn is_empty(&self) -> bool {
        self.tag.is_none() && self.attrs.is_empty() && self.texts.is_empty()
    }

    /// Structural predicates → a CSS selector. `attr_regex` + text predicates
    /// are post-filters and are NOT emitted here. Empty set → `"*"`.
    pub(crate) fn to_css_selector(&self) -> String {
        let mut s = self.tag.clone().unwrap_or_default();
        for a in &self.attrs {
            match a {
                AttrPred::Exact(n, v) => s.push_str(&format!("[{n}={}]", q(v))),
                AttrPred::Contains(n, v) => s.push_str(&format!("[{n}*={}]", q(v))),
                AttrPred::StartsWith(n, v) => s.push_str(&format!("[{n}^={}]", q(v))),
                AttrPred::EndsWith(n, v) => s.push_str(&format!("[{n}$={}]", q(v))),
                AttrPred::Has(n) => s.push_str(&format!("[{n}]")),
                AttrPred::Regex(..) => {}
            }
        }
        if s.is_empty() { "*".to_string() } else { s }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_set_compiles_to_star() {
        assert_eq!(PredicateSet::default().to_css_selector(), "*");
    }

    #[test]
    fn tag_and_attrs_compile_to_css() {
        let p = PredicateSet {
            tag: Some("div".into()),
            attrs: vec![
                AttrPred::Exact("data-role".into(), "card".into()),
                AttrPred::Contains("class".into(), "active".into()),
                AttrPred::StartsWith("id".into(), "item-".into()),
                AttrPred::EndsWith("data-x".into(), "-end".into()),
                AttrPred::Has("data-ready".into()),
            ],
            texts: vec![],
        };
        assert_eq!(
            p.to_css_selector(),
            r#"div[data-role="card"][class*="active"][id^="item-"][data-x$="-end"][data-ready]"#
        );
    }

    #[test]
    fn attr_regex_is_not_in_css() {
        let p = PredicateSet {
            tag: Some("a".into()),
            attrs: vec![AttrPred::Regex("href".into(), r"\d+".into())],
            texts: vec![],
        };
        assert_eq!(p.to_css_selector(), "a");
    }
}
```

Add to `crates/zendriver/src/query/mod.rs` near the other `pub mod` lines (around line 29):
```rust
pub(crate) mod predicate;
```

- [ ] **Step 2: Run test to verify it passes**

Run: `cargo test -p zendriver predicate::tests`
Expected: 3 passed.

- [ ] **Step 3: Commit**
```bash
git add crates/zendriver/src/query/predicate.rs crates/zendriver/src/query/mod.rs
git commit -m "feat(query): PredicateSet types + CSS compilation"
```

---

## Task 2: JS post-filter compilation

**Files:**
- Modify: `crates/zendriver/src/query/predicate.rs`

- [ ] **Step 1: Write the failing test**

Add to `predicate.rs` `tests` mod:
```rust
#[test]
fn empty_filter_is_true() {
    assert_eq!(PredicateSet::default().to_js_filter(), "true");
}

#[test]
fn regex_and_text_compile_to_js_checks() {
    let p = PredicateSet {
        tag: None,
        attrs: vec![AttrPred::Regex("href".into(), r"\d+".into())],
        texts: vec![
            TextPred::Contains("Buy".into()),
            TextPred::Equals("OK".into()),
            TextPred::Matches(r"^\$".into()),
        ],
    };
    let f = p.to_js_filter();
    assert!(f.contains(r#"new RegExp("\\d+").test(el.getAttribute("href")||"")"#), "{f}");
    assert!(f.contains(r#"(el.innerText||el.textContent||"").includes("Buy")"#), "{f}");
    assert!(f.contains(r#"(el.innerText||el.textContent||"").trim()==="OK""#), "{f}");
    assert!(f.contains(r#"new RegExp("^\\$").test((el.innerText||el.textContent||""))"#), "{f}");
    assert!(f.contains("&&"), "checks are AND-joined: {f}");
}
```

- [ ] **Step 2: Implement**

Add to `impl PredicateSet` in `predicate.rs`:
```rust
    /// Post-filter predicates (`attr_regex` + all text predicates) → a JS
    /// boolean expression over a bound `el`. Returns `"true"` when there are
    /// no post-filters (so the caller can always `.filter(el => <expr>)`).
    pub(crate) fn to_js_filter(&self) -> String {
        const TXT: &str = r#"(el.innerText||el.textContent||"")"#;
        let mut checks: Vec<String> = Vec::new();
        for a in &self.attrs {
            if let AttrPred::Regex(n, p) = a {
                checks.push(format!(
                    "new RegExp({}).test(el.getAttribute({})||\"\")",
                    q(p), q(n)
                ));
            }
        }
        for t in &self.texts {
            match t {
                TextPred::Contains(s) => checks.push(format!("{TXT}.includes({})", q(s))),
                TextPred::Equals(s) => checks.push(format!("{TXT}.trim()==={}", q(s))),
                TextPred::Matches(p) => checks.push(format!("new RegExp({}).test({TXT})", q(p))),
            }
        }
        if checks.is_empty() { "true".to_string() } else { checks.join("&&") }
    }
```

- [ ] **Step 3: Run + commit**

Run: `cargo test -p zendriver predicate::tests`
Expected: 5 passed.
```bash
git add crates/zendriver/src/query/predicate.rs
git commit -m "feat(query): predicate JS post-filter compilation"
```

---

## Task 3: `ConflictingSelectors` error variant

**Files:**
- Modify: `crates/zendriver/src/error.rs`

- [ ] **Step 1: Write the failing test**

Add to `crates/zendriver/src/error.rs` (in or after the existing `#[cfg(test)]` mod; if none, add one):
```rust
#[test]
fn conflicting_selectors_message() {
    let e = ZendriverError::ConflictingSelectors;
    assert!(e.to_string().contains("predicate"));
    assert!(e.to_string().contains("css"));
}
```

- [ ] **Step 2: Implement**

Add a variant to `pub enum ZendriverError` (after the `ElementNotFound` variant near line 68):
```rust
    /// A predicate method (`.tag`/`.attr`/`.containing_text`/…) was combined
    /// with a single-selector method (`.css`/`.xpath`/`.text`/`.role`) on one
    /// query. Use one selector style per query.
    #[error("predicate methods (.tag/.attr/…) cannot be combined with .css()/.xpath()/.text()/.role(); use one selector style per query")]
    ConflictingSelectors,
```

- [ ] **Step 3: Run + commit**

Run: `cargo test -p zendriver conflicting_selectors_message`
Expected: PASS.
```bash
git add crates/zendriver/src/error.rs
git commit -m "feat(error): ConflictingSelectors variant"
```

---

## Task 4: Predicate methods on `FindBuilder` (+ shared macro)

**Files:**
- Modify: `crates/zendriver/src/query/mod.rs`

- [ ] **Step 1: Write the failing test**

Add a unit test to `mod.rs` (in a `#[cfg(test)] mod predicate_builder_tests`):
```rust
#[cfg(test)]
mod predicate_builder_tests {
    use super::*;
    use crate::query::predicate::{AttrPred, TextPred};

    // A builder with no Tab is fine for inspecting accumulated predicates;
    // we never call a terminal here.
    fn bare() -> FindBuilder<'static> {
        FindBuilder {
            tab: None, element: None, frame: None,
            selector: None, predicates: Default::default(),
            timeout: DEFAULT_TIMEOUT, nth: None, visible_only: false,
            in_frame: None, include_frames: false, best_match: false,
        }
    }

    #[test]
    fn predicate_methods_accumulate() {
        let b = bare().tag("div").attr("data-x", "y").attr_contains("class", "z")
            .has_attr("ready").attr_regex("id", r"\d+").containing_text("Buy")
            .text_equals("OK").text_matches(r"^\$");
        assert_eq!(b.predicates.tag.as_deref(), Some("div"));
        assert_eq!(b.predicates.attrs.len(), 4);
        assert_eq!(b.predicates.texts.len(), 3);
        assert!(matches!(b.predicates.attrs[0], AttrPred::Exact(_, _)));
        assert!(matches!(b.predicates.texts[2], TextPred::Matches(_)));
    }
}
```

- [ ] **Step 2: Implement**

In `mod.rs`: add the field to the `FindBuilder` struct (after `selector`):
```rust
    pub(crate) selector: Option<SelectorKind>,
    /// Accumulated bs4-like predicates. Mutually exclusive with `selector`
    /// (enforced at the terminal — see `one`/`one_or_none`).
    pub(crate) predicates: crate::query::predicate::PredicateSet,
```
Add `predicates: Default::default(),` to all three constructors (`new_for_tab`, `new_for_element`, `new_for_frame`).

Define the shared macro once (top of `mod.rs`, after imports) — internal code-gen, not a user DSL:
```rust
use crate::query::predicate::{AttrPred, PredicateSet, TextPred};

/// Generates the 10 predicate setters shared by FindBuilder + FindAllBuilder.
/// Both structs have a `predicates: PredicateSet` field.
macro_rules! predicate_methods {
    () => {
        /// Match by tag name (compiles into the CSS selector). Predicate mode.
        #[must_use] pub fn tag(mut self, name: impl Into<String>) -> Self {
            self.predicates.tag = Some(name.into()); self
        }
        /// Match an exact attribute value `[name="value"]`.
        #[must_use] pub fn attr(mut self, name: &str, value: &str) -> Self {
            self.predicates.attrs.push(AttrPred::Exact(name.into(), value.into())); self
        }
        /// Match a substring of an attribute value `[name*="sub"]`.
        #[must_use] pub fn attr_contains(mut self, name: &str, sub: &str) -> Self {
            self.predicates.attrs.push(AttrPred::Contains(name.into(), sub.into())); self
        }
        /// Match an attribute value prefix `[name^="pre"]`.
        #[must_use] pub fn attr_starts_with(mut self, name: &str, pre: &str) -> Self {
            self.predicates.attrs.push(AttrPred::StartsWith(name.into(), pre.into())); self
        }
        /// Match an attribute value suffix `[name$="suf"]`.
        #[must_use] pub fn attr_ends_with(mut self, name: &str, suf: &str) -> Self {
            self.predicates.attrs.push(AttrPred::EndsWith(name.into(), suf.into())); self
        }
        /// Require the attribute be present `[name]`.
        #[must_use] pub fn has_attr(mut self, name: &str) -> Self {
            self.predicates.attrs.push(AttrPred::Has(name.into())); self
        }
        /// Match an attribute value against a JS regex (post-filter).
        #[must_use] pub fn attr_regex(mut self, name: &str, pattern: &str) -> Self {
            self.predicates.attrs.push(AttrPred::Regex(name.into(), pattern.into())); self
        }
        /// Match elements whose text contains `sub` (post-filter).
        #[must_use] pub fn containing_text(mut self, sub: &str) -> Self {
            self.predicates.texts.push(TextPred::Contains(sub.into())); self
        }
        /// Match elements whose trimmed text equals `exact` (post-filter).
        #[must_use] pub fn text_equals(mut self, exact: &str) -> Self {
            self.predicates.texts.push(TextPred::Equals(exact.into())); self
        }
        /// Match elements whose text matches a JS regex `pattern` (post-filter).
        #[must_use] pub fn text_matches(mut self, pattern: &str) -> Self {
            self.predicates.texts.push(TextPred::Matches(pattern.into())); self
        }
    };
}
```
Invoke it inside `impl<'scope> FindBuilder<'scope>` (alongside the existing selector methods):
```rust
impl<'scope> FindBuilder<'scope> {
    predicate_methods!{}
    // ... existing methods ...
}
```

- [ ] **Step 3: Run + commit**

Run: `cargo test -p zendriver predicate_builder_tests`
Expected: PASS.
```bash
git add crates/zendriver/src/query/mod.rs
git commit -m "feat(query): predicate methods on FindBuilder"
```

---

## Task 5: Predicate methods on `FindAllBuilder`

**Files:**
- Modify: `crates/zendriver/src/query/mod.rs`

- [ ] **Step 1: Implement (the macro makes this mechanical)**

Add the same field to the `FindAllBuilder` struct:
```rust
    pub(crate) predicates: crate::query::predicate::PredicateSet,
```
Add `predicates: Default::default(),` to its three constructors (`new_for_tab`/`new_for_element`/`new_for_frame` for `FindAllBuilder`).
Invoke the macro in its impl:
```rust
impl<'scope> FindAllBuilder<'scope> {
    predicate_methods!{}
    // ... existing methods ...
}
```

- [ ] **Step 2: Write + run the test**

Add to `predicate_builder_tests`:
```rust
fn bare_all() -> FindAllBuilder<'static> {
    FindAllBuilder {
        tab: None, element: None, frame: None,
        selector: None, predicates: Default::default(),
        timeout: DEFAULT_TIMEOUT, visible_only: false,
        in_frame: None, include_frames: false, best_match: false,
    }
}
#[test]
fn find_all_predicates_accumulate() {
    let b = bare_all().tag("a").has_attr("href").containing_text("Next");
    assert_eq!(b.predicates.tag.as_deref(), Some("a"));
    assert_eq!(b.predicates.attrs.len(), 1);
    assert_eq!(b.predicates.texts.len(), 1);
}
```
> Adapt `bare_all()`'s field list to the real `FindAllBuilder` struct fields (it lacks `nth`; confirm against mod.rs:697+).

Run: `cargo test -p zendriver predicate_builder_tests`
Expected: PASS.

- [ ] **Step 3: Commit**
```bash
git add crates/zendriver/src/query/mod.rs
git commit -m "feat(query): predicate methods on FindAllBuilder"
```

---

## Task 6: Predicate resolution + terminal wiring (conflict guard)

**Files:**
- Modify: `crates/zendriver/src/query/selectors.rs` (add resolvers)
- Modify: `crates/zendriver/src/query/mod.rs` (terminal dispatch + conflict check)

- [ ] **Step 1: Implement the resolvers** in `selectors.rs` (model on `resolve_css_many`, selectors.rs:232):
```rust
use crate::query::predicate::PredicateSet;

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
                json!(css), filter
            );
            let mut params = json!({ "expression": expr, "returnByValue": false });
            if let Some(id) = ctx { params["contextId"] = json!(id); }
            session.call("Runtime.evaluate", params).await?
        }
        QueryScope::Element(el) => {
            let object_id = el.remote_object_id_cloned().await?;
            let func = format!(
                "function(){{return Array.from(this.querySelectorAll({})).filter(function(el){{return {};}});}}",
                json!(css), filter
            );
            session.call("Runtime.callFunctionOn", json!({
                "objectId": object_id,
                "functionDeclaration": func,
                "returnByValue": false,
            })).await?
        }
    };
    extract_array_refs(session, &result["result"]).await
}

pub(crate) async fn resolve_predicate_one(
    scope: &QueryScope<'_>,
    pred: &PredicateSet,
) -> Result<Option<RemoteRef>> {
    Ok(resolve_predicate_many(scope, pred).await?.into_iter().next())
}
```
> Confirm `extract_array_refs` + `remote_object_id_cloned` signatures match the css resolver's usage (selectors.rs:264, :214). Reuse verbatim.

- [ ] **Step 2: Wire the terminal + conflict guard** in `mod.rs`. In `FindBuilder::one_or_none` (the resolution core; find the existing body that builds a `QueryScope` and calls `selector.resolve_one(...)`), add at the top, before selector resolution:
```rust
        if self.selector.is_some() && !self.predicates.is_empty() {
            return Err(ZendriverError::ConflictingSelectors);
        }
        if !self.predicates.is_empty() {
            // predicate path — mirror the selector path's scope construction,
            // nth/visible_only/timeout handling, and Element synthesis.
            // (Reuse the same QueryScope + poll loop; swap resolve_one for
            //  crate::query::selectors::resolve_predicate_one(&scope, &self.predicates).)
        }
```
> The existing `one_or_none`/`one` already build a `QueryScope`, run a timeout poll loop, apply `nth`/`visible_only`, and synthesize `Element`s from `RemoteRef`s. Refactor that resolution body so it accepts EITHER `selector.resolve_one`/`resolve_many` OR `resolve_predicate_one`/`many` — e.g. extract the candidate-fetch into a closure/match on `(selector, predicates)`. Do the SAME in `include_frames` fan-out (`one_across_frames`) so predicates work across frames. Keep `nth`/`visible_only`/`best_match`/`timeout` applied identically.

- [ ] **Step 3: Write the conflict test** (`predicate_builder_tests`, needs a Tab — use the existing mock/integration harness if a unit Tab isn't constructible; otherwise assert via a thin helper that calls the guard). Minimal guard unit test that doesn't need a browser:
```rust
// If a no-browser Tab can't be built, assert the guard by calling the
// extracted check function directly:
#[test]
fn mixing_selector_and_predicate_is_conflict() {
    let mut b = bare();
    b.selector = Some(SelectorKind::Css("div".into()));
    b = b.tag("span");
    assert!(b.selector.is_some() && !b.predicates.is_empty());
    // resolve_conflict(&b) -> Err(ConflictingSelectors)   (extract this helper)
}
```
> Prefer extracting a tiny `fn has_conflict(sel: &Option<SelectorKind>, pred: &PredicateSet) -> bool` so it's unit-testable without a Tab; assert it returns true here and false for either-alone.

- [ ] **Step 4: Run + commit**

Run: `cargo test -p zendriver query::` and `cargo build -p zendriver`
Expected: green (unit + build).
```bash
git add crates/zendriver/src/query/
git commit -m "feat(query): predicate resolution + mixing guard at terminal"
```

---

## Task 7: `select` / `select_all` aliases

**Files:**
- Modify: `crates/zendriver/src/tab.rs`, `crates/zendriver/src/frame/mod.rs`, `crates/zendriver/src/element/mod.rs`

- [ ] **Step 1: Implement** (Tab shown; Frame + Element are identical with their own `find`/`find_all`):
```rust
// in impl Tab (and impl Frame, impl Element)
/// Find one element by CSS selector. Python-parity convenience for
/// `find().css(sel).one()`. For modifiers (frames/nth/timeout) use the builder.
pub async fn select(&self, css: &str) -> Result<Element> {
    self.find().css(css).one().await
}
/// Find all elements by CSS selector. Python-parity convenience for
/// `find_all().css(sel).many()`.
pub async fn select_all(&self, css: &str) -> Result<Vec<Element>> {
    self.find_all().css(css).many().await
}
```
> Confirm each type exposes `find()`/`find_all()` returning the builders (Tab:2166, Frame/mod.rs:458, Element traversal.rs:88). For `Element`, `select_all` scopes to its subtree (correct).

- [ ] **Step 2: Build + a doctest**

Add a `no_run` doctest on `Tab::select` mirroring the existing doctest style (module example at mod.rs:6). Run:
`cargo test -p zendriver --doc select`
Expected: compiles/passes.

- [ ] **Step 3: Commit**
```bash
git add crates/zendriver/src/tab.rs crates/zendriver/src/frame/mod.rs crates/zendriver/src/element/mod.rs
git commit -m "feat: select/select_all CSS aliases on Tab/Frame/Element"
```

---

## Task 8: Nested-iframe verify (TDD) — fix only if red

**Files:**
- Create: `crates/zendriver/tests/find_predicate_iframe.rs`

- [ ] **Step 1: Write the iframe-in-iframe test** (gate exactly like `crates/zendriver/tests/integration_phase4.rs` — copy its `#![cfg(feature = "integration-tests")]` / `#[tokio::test]` / `#[serial]` / serving pattern):
```rust
// Serve: outer.html embeds mid.html (iframe) which embeds inner.html (iframe)
// containing <div id="deep">found</div>. Use the same local-server helper the
// other integration tests use (see integration_phase4.rs for the fixture/server).
#[tokio::test]
#[ignore] // headful; run on the integration job
async fn include_frames_finds_element_two_iframes_deep() {
    // ... launch, goto outer, wait for frames ...
    let el = tab.find().css("#deep").include_frames().one().await
        .expect("element nested two iframes deep must be found");
    assert_eq!(el.text().await.unwrap(), "found");

    let all = tab.find_all().css("#deep").include_frames().many().await.unwrap();
    assert_eq!(all.len(), 1);
}
```

- [ ] **Step 2: Run it**

Run: `cargo test -p zendriver --features integration-tests --test find_predicate_iframe -- --ignored include_frames_finds_element_two`
- **PASS** → architecture already crosses nested frames. Skip to Step 4 (lock + commit).
- **FAIL** → Step 3.

- [ ] **Step 3 (only if red): fix the gap**

Inspect frame registration: `crates/zendriver/src/tab.rs` (frame registry / `Tab::frames`) + the `Page.frameAttached` / frame-tree handler. Ensure frames are registered at ALL depths (a nested iframe's `frameAttached` fires with a `parentId`; confirm the handler adds it). If same-process nested frames are NOT separate query scopes, add the fallback content_document walk to `resolve_css_*`/`resolve_predicate_*`: walk `document.querySelectorAll('iframe,frame')`, recurse into each `el.contentDocument` (guard `=== null` for cross-origin), collect matches. Add a unit test for the walk's null-guard if you add JS.

- [ ] **Step 4: Commit**
```bash
git add crates/zendriver/tests/find_predicate_iframe.rs  # + any fix
git commit -m "test: nested-iframe include_frames traversal (verify + lock)"
```

---

## Task 9: Predicate + select_all integration tests

**Files:**
- Modify: `crates/zendriver/tests/find_predicate_iframe.rs`

- [ ] **Step 1: Add tests** against a fixture page with known elements:
```rust
#[tokio::test]
#[ignore]
async fn predicate_finds_by_tag_attr_text() {
    // fixture: <button class="primary active" data-id="4821">Buy now</button>
    let el = tab.find()
        .tag("button").attr_contains("class", "active")
        .attr_regex("data-id", r"^\d{4}$").containing_text("Buy")
        .one().await.unwrap();
    assert!(el.text().await.unwrap().contains("Buy"));
}

#[tokio::test]
#[ignore]
async fn select_all_returns_all_matches() {
    let items = tab.select_all("ul li").await.unwrap();
    assert_eq!(items.len(), 3);
}

#[tokio::test]
#[ignore]
async fn mixing_predicate_and_css_errors() {
    let err = tab.find().css("div").tag("span").one().await.unwrap_err();
    assert!(matches!(err, zendriver::ZendriverError::ConflictingSelectors));
}
```
> Reuse the fixture/server helper from the iframe test. Adapt `el.text()` to the real accessor.

- [ ] **Step 2: Compile-check (defer headful run to CI)**

Run: `cargo test -p zendriver --features integration-tests --test find_predicate_iframe --no-run`
Expected: compiles.

- [ ] **Step 3: Commit**
```bash
git add crates/zendriver/tests/find_predicate_iframe.rs
git commit -m "test: predicate finds + select_all + mixing-guard integration"
```

---

## Task 10: Docs + CHANGELOG

**Files:**
- Modify: `crates/zendriver/src/query/mod.rs` (module docs), `CHANGELOG.md`

- [ ] **Step 1: Update module docs** — extend the `query/mod.rs` header doc (lines 22-27) to document predicate mode + the mixing rule:
```rust
//! Predicate mode (combinable, AND-ed): `.tag`, `.attr`, `.attr_contains`,
//! `.attr_starts_with`, `.attr_ends_with`, `.has_attr`, `.attr_regex`,
//! `.containing_text`, `.text_equals`, `.text_matches`. Predicate methods
//! cannot be combined with the single-selector kinds (`.css`/`.xpath`/
//! `.text*`/`.role*`) — doing so errors at the terminal with
//! `ConflictingSelectors`.
```
Add a `no_run` doctest showing a predicate find + a `select_all` call.

- [ ] **Step 2: CHANGELOG**

Add under `[Unreleased]` in `CHANGELOG.md`:
```markdown
### Added
- bs4-like combinable predicate finders on `find()`/`find_all()`: `tag`,
  `attr`, `attr_contains`, `attr_starts_with`, `attr_ends_with`, `has_attr`,
  `attr_regex`, `containing_text`, `text_equals`, `text_matches` (#55).
- `select`/`select_all` CSS convenience aliases on `Tab`/`Frame`/`Element`.
- Verified `include_frames()` finds elements in nested iframes (#239).
```

- [ ] **Step 3: Commit**
```bash
git add crates/zendriver/src/query/mod.rs CHANGELOG.md
git commit -m "docs: predicate finders + select aliases"
```

---

## Task 11: Gates + PR

- [ ] **Step 1: Format + clippy (per CLAUDE.md)**
```bash
cargo fmt --all
cargo clippy --workspace --all-targets --locked --fix --allow-dirty --allow-staged
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
```
Expected: clean. (Predicate/query code is default-feature; no extra feature clippy needed unless you touched gated code.)

- [ ] **Step 2: Tests**
```bash
cargo test --workspace --locked
cargo test -p zendriver --features integration-tests --test find_predicate_iframe --no-run
```
Expected: unit/doctests green; integration compiles. (Headful `--ignored` tests run on the integration job / locally with Chrome.)

- [ ] **Step 3: Commit + PR**
```bash
git add -A && git commit -m "chore: fmt + clippy for find/DOM predicates"
gh pr create --base main \
  --title "feat: bs4-like predicate finders + select_all + nested-iframe (#55, #239)" \
  --body "PR2 Group A. See docs/superpowers/specs/2026-06-02-find-dom-predicates-iframe-design.md"
```

---

## Self-Review (completed by plan author)

**Spec coverage:** §3 predicate API → T1/T2/T4/T5; §3 mixing guard 1A → T3/T6; §4 compilation → T1/T2/T6; §5 modifier composition → T6 (terminal refactor reuses nth/visible_only/include_frames); §6 nested-iframe verify → T8; §6 select/select_all → T7; §7 testing → T1/T2/T4/T6 (unit) + T8/T9 (integration); §8 out-of-scope honored (no macro DSL, no type-state). Covered.

**Placeholders:** the verify-fork in T8 (pass→lock / fail→fix) is intentional + bounded, not a placeholder. The T6 terminal refactor is described against the real existing `one_or_none`/`one_across_frames` bodies (reuse, not invent). Adapt-points flagged inline (real struct fields for `bare_all()`, `extract_array_refs`/`remote_object_id_cloned` signatures, `find()`/`text()` accessor names, integration-test gating/server helper).

**Type consistency:** `PredicateSet`/`AttrPred`/`TextPred`, `to_css_selector`/`to_js_filter`, `resolve_predicate_one`/`many`, `predicates` field, `ConflictingSelectors`, the 10 method names, `select`/`select_all` — used consistently across tasks and matching the spec.
