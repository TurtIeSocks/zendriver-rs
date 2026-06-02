# Find/DOM — Predicate Builder + Nested-iframe — Design (PR2 / Group A)

- **Date:** 2026-06-02
- **Status:** Approved (brainstorming), pending implementation plan
- **Upstream drivers:** zendriver #55 (bs4-like find), #239 + PR #246 (nested-iframe select_all)
- **Scope:** Group A of the PR2 batch (the 7 upstream TODOs split into ~4 cohesive sub-PRs). This spec covers **Find/DOM only**. Groups B (network), C (robustness), D (datadome) are separate spec→plan→PR cycles.

---

## 1. Context

The port already has rich finders ([`crates/zendriver/src/query/mod.rs`](../../../crates/zendriver/src/query/mod.rs)):

- `FindBuilder` (single) + `FindAllBuilder` (multi), each with **8 mutually-exclusive selector kinds** (`css`, `xpath`, `text`, `text_exact`, `text_regex`, `text_regex_with_flags`, `role`, `role_named`) plus modifiers (`nth`, `visible_only`, `in_frame`, `include_frames`, `best_match`, `timeout`) and terminals (`one`/`one_or_none`, `many`/`many_or_empty`).
- Queries run through `Runtime.evaluate` / `Runtime.callFunctionOn` of JS `querySelectorAll` (document-bound), **not** CDP `DOM.querySelectorAll`.
- Cross-frame `include_frames()` (`one_across_frames`) iterates the `Tab::frames()` registry, querying each frame in **its own session / execution context** — architecturally different from Python zendriver's single-document query.
- No `select`/`select_all` aliases.

Two gaps this sub-PR closes:

1. **#55 (bs4-like):** find by *any combination* of tag + attributes + text. Today the selector kinds are mutually exclusive — you cannot AND css + text in one query.
2. **#239/#246 (nested iframe):** ensure `find`/`find_all` with `include_frames()` reach elements in arbitrarily-nested iframes. Python's bug is `querySelectorAll` not crossing `content_document` boundaries; Rust's per-frame-context architecture *may already cover this* — so the work is **verify-first**, not a blind port.

---

## 2. Decisions locked in brainstorming

- **Mixing guard: 1A — runtime error** (not compile-time type-state). One `FindBuilder` type; mixing a predicate method with a single-selector method is rejected at the terminal with a clear error. (Type-state 1B rejected for this domain — it is hostile to runtime-chosen selectors, the common scraping case; see §8.)
- **Predicate API: combinable builder methods** directly on `FindBuilder`/`FindAllBuilder`.
- **Matchers in scope:** `tag`, `attr`, `attr_contains`, `attr_starts_with`, `attr_ends_with`, `has_attr`, `attr_regex`, `containing_text`, `text_equals`, `text_matches`. (Case-insensitive variants deferred.)
- **Predicate-text names: 3A** — `containing_text` / `text_equals` / `text_matches` (distinct from the standalone `text`/`text_exact`/`text_regex` modes to avoid collision).
- **`select`/`select_all` aliases: IN, minimal** (no-modifier CSS conveniences for Python migration parity).
- **Nested-iframe: verify-first** (test → fix only the gap → content_document walk is a fallback only if needed).

---

## 3. Predicate builder — public API

New combinable methods on **both** `FindBuilder` and `FindAllBuilder` (all AND-ed):

```rust
// structural — compile into the CSS selector
pub fn tag(self, name: impl Into<String>) -> Self;            // div
pub fn attr(self, name: &str, value: &str) -> Self;          // [name="value"]
pub fn attr_contains(self, name: &str, sub: &str) -> Self;   // [name*="sub"]
pub fn attr_starts_with(self, name: &str, pre: &str) -> Self;// [name^="pre"]
pub fn attr_ends_with(self, name: &str, suf: &str) -> Self;  // [name$="suf"]
pub fn has_attr(self, name: &str) -> Self;                   // [name]

// post-filters — applied to the candidate set in the same JS pass
pub fn attr_regex(self, name: &str, pattern: &str) -> Self;  // RegExp(pattern).test(getAttribute(name))
pub fn containing_text(self, sub: &str) -> Self;             // (innerText||textContent).includes(sub)
pub fn text_equals(self, exact: &str) -> Self;               // trimmed text === exact
pub fn text_matches(self, pattern: &str) -> Self;            // RegExp(pattern).test(text)
```

Usage:
```rust
let card = tab.find()
    .tag("div")
    .attr("data-role", "card")
    .attr_contains("class", "active")
    .has_attr("data-ready")
    .containing_text("Buy")
    .one().await?;
```

Composes with all existing modifiers (`nth`, `visible_only`, `include_frames`, `best_match`, `timeout`) and all scopes (`tab.find()`, `frame.find()`, `element.find()`).

### Coexistence + mixing guard (1A)

The builder tracks an internal selector spec:

```rust
enum SelectorSpec {
    None,
    Single(SingleSelector),   // css | xpath | text* | role*
    Predicate(PredicateSet),  // accumulated predicates above
}
```

- Single-selector setters and predicate setters target different variants. If both groups get touched, a `conflict` flag is set.
- The terminal (`one`/`one_or_none`/`many`/`many_or_empty`) returns `Err(ZendriverError::ConflictingSelectors)` with message: *"predicate methods (.tag/.attr/.containing_text/…) cannot be combined with .css()/.xpath()/.text()/.role(); use one selector style per query."*
- Within the predicate group, methods accumulate (AND). Within the single group, last-wins (unchanged from today).
- Empty spec at terminal → existing `Err(..NoSelector)` (unchanged).

---

## 4. Compilation (one CDP round-trip)

A predicate set compiles to a **single JS function**, evaluated once per scope via the existing `Runtime.evaluate` / `Runtime.callFunctionOn` path in [`query/selectors.rs`](../../../crates/zendriver/src/query/selectors.rs):

1. **CSS-expressible parts** → one selector string. `tag` + `attr*`/`has_attr` →
   `div[data-role="card"][class*="active"][data-ready]`. (No structural predicate → `*`.)
2. `Array.from((scope).querySelectorAll(sel))` — `scope` is `document` (tab/frame) or `this` (element), matching the existing scope dispatch.
3. **Post-filters** applied in the same function, no extra round-trip:
   - `attr_regex(name, p)` → `new RegExp(p).test(el.getAttribute(name) ?? "")`
   - `containing_text` / `text_equals` / `text_matches` → against `el.innerText || el.textContent`, reusing the existing text-engine semantics (`text_matches` uses JS `RegExp`, consistent with the current `text_regex_with_flags`).
4. Return the filtered node array (FindAll) or first match (Find), reusing the existing `Runtime.getProperties` enumeration → `Element` wrapping.

Attribute values and patterns are JSON-escaped into the JS source (reuse the existing escaping helper used by the current selectors). This keeps predicate queries to the **same one round-trip per scope** as today's finders.

---

## 5. Composition with modifiers & scopes

Predicate resolution produces a candidate `Vec<Element>` (or first match); existing modifiers apply unchanged on top:

- `nth(i)` — index into candidates; `visible_only(true)` — filter by the existing visibility check; `timeout(d)` — existing poll loop; `best_match()` — existing text-length heuristic (meaningful when a text predicate is present).
- `include_frames()` — fans out the predicate query across the frame registry exactly as the single-selector path does (the compiled JS runs per scope/frame).
- Available identically on `FindBuilder` (`one`/`one_or_none`) and `FindAllBuilder` (`many`/`many_or_empty`), and for tab/frame/element scopes.

---

## 6. Nested-iframe (#239/#246) — verify-first

Rust's `include_frames()` queries each registered frame in its own context, unlike Python's document-bound query. So:

1. **Verify (TDD, first):** add a headful integration test — an element nested **two iframes deep** — and assert `tab.find().css(sel).include_frames().one()` and `find_all(...).many()` locate it.
   - **If it passes:** the architecture already crosses nested boundaries. Lock it with the test; done. No content_document walk.
   - **If it fails:** fix the gap in this order:
     a. Ensure the frame registry captures **nested same-process frames** (confirm `Page.frameAttached`/frame-tree handling registers frames at all depths), and that each resolves in its execution context.
     b. **Fallback only if (a) cannot cover same-process nested frames:** add a `content_document` DOM-walk (the #246 approach) for frames not represented as separate query scopes — walking the tree, collecting each iframe's `content_document`, querying each, guarding cross-origin `content_document === null`.
2. **`select` / `select_all` parity aliases** — thin CSS conveniences on `Tab`, `Frame`, `Element`:
   ```rust
   /// One element by CSS selector. Convenience for `find().css(sel).one()`.
   pub async fn select(&self, css: &str) -> Result<Element>;
   /// All elements by CSS selector. Convenience for `find_all().css(sel).many()`.
   pub async fn select_all(&self, css: &str) -> Result<Vec<Element>>;
   ```
   No-modifier by design; callers needing `include_frames`/`nth`/`timeout` use the builder. Eases Python→Rust migration (#239/#246 are framed around `select_all`).

---

## 7. Testing

- **Unit (no browser):**
  - Predicate → CSS-selector string compilation (assert generated selector for representative predicate sets; empty → `*`).
  - Predicate → JS function generation (snapshot/string-contains: `querySelectorAll`, the regex/text post-filters, escaping).
  - Mixing guard: predicate + `.css()` → `ConflictingSelectors` at terminal; empty → `NoSelector`.
- **Integration (gated headful, mirror `integration_phase4.rs` gating):**
  - **iframe-in-iframe** find/find_all across frames (§6 verify).
  - Predicate finds against a fixture page: tag+attr combos, `attr_contains`/`starts_with`/`ends_with`/`has_attr`, `attr_regex`, `containing_text`/`text_equals`/`text_matches`, and composition with `nth`/`visible_only`.
  - `select` / `select_all` (incl. across frames via the builder for the include_frames case).

---

## 8. Out of scope (future / other PRs)

- **`find!` DSL macro** — a terse `macro_rules!`/proc-macro query DSL (`find!(tab, tag="div", class*="x", text~"Buy")`). Real terseness value but adds a DSL + a second way to do one thing. Revisit if users ask. **Future.**
- **Type-state builders (1B)** — compile-time enforcement of the single-vs-predicate split via a `FindBuilder<Mode>` type parameter. Rejected for this domain: it is hostile to runtime-chosen selectors (each branch is a distinct type → cannot unify without erasure), the common scraping pattern. If revisited, it is its own deliberate API initiative across all builders (~1–2 wk), not part of Group A. **Future, separate.**
- **Case-insensitive matchers** (`[a="v" i]`, lowercased text compare). **Deferred.**
- **XPath predicate compilation** — predicates compile to CSS + JS filter only; `.xpath()` remains its own single-selector mode.
- **Groups B (network monitor #223 / browser-context HTTP #189), C (CDP freshness / popup flags), D (datadome #20)** — separate sub-PRs.

---

## 9. Open questions for the plan stage

- Confirm the exact existing escaping helper + scope-dispatch functions in `selectors.rs` to reuse for predicate JS generation.
- Confirm whether the frame registry already enumerates nested same-process frames (drives whether §6 step 2a/2b is needed) — resolved empirically by the §6 verify test at implementation time.
- Exact `ConflictingSelectors` error variant placement in the `ZendriverError` hierarchy.
