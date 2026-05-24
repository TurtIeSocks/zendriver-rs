# zendriver-rs Phase 6 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use `- [ ]` checkbox syntax.

**Goal:** Polish + publish v0.1.0 to crates.io. Deep API review (trait extraction + naming + non_exhaustive audit), rustdoc + mdBook coverage, README final pass, CHANGELOG/SEMVER docs, topological publish script, dry-run gate, tag + ship.

**Architecture:** No new functional surface. Mostly mechanical (rename, doc-pass, audit). New: `crates/zendriver/src/traits/` (Queryable + Evaluable), `docs/book/` (mdBook), `scripts/publish.sh`, root `CHANGELOG.md` + `SEMVER.md` + `CONTRIBUTING.md`, `.github/workflows/docs.yml`.

**Tech Stack:** mdBook + mdbook-linkcheck for docs; bash for publish script; cargo workspace inheritance for version bump.

**Spec:** [docs/superpowers/specs/2026-05-23-zendriver-rs-phase6-release-design.md](../specs/2026-05-23-zendriver-rs-phase6-release-design.md)

---

## File structure

See spec for full layout. New: `docs/book/`, `scripts/`, `crates/zendriver/src/traits/`, root `CHANGELOG.md`/`SEMVER.md`/`CONTRIBUTING.md`, `.github/workflows/docs.yml`.

## Task list

| # | Title | Files |
|---|---|---|
| 0 | Naming pass: `_raw` → `_fast` | crates/zendriver/src/element/{actions,input}.rs, tests, examples |
| 1 | `#[non_exhaustive]` audit drops (11 enums) | various src/ across crates |
| 2 | Trait extraction: Queryable + Evaluable | crates/zendriver/src/traits/{mod,queryable,evaluable}.rs, tab.rs, frame/mod.rs, element/mod.rs, lib.rs |
| 3 | Trait derive audit (Debug/Clone/Send+Sync) | various across crates |
| 4 | Rustdoc coverage: zendriver core | crates/zendriver/src/{lib,browser,tab,element/*,frame/*,query/*,cookies/*,storage/*,screenshot/*,input/*}.rs |
| 5 | Rustdoc coverage: sub-crates | crates/zendriver-{transport,stealth,interception,cloudflare,fetcher}/src/**/*.rs |
| 6 | mdBook scaffolding | docs/book/book.toml + src/SUMMARY.md + stub chapters |
| 7 | mdBook chapters: core | introduction.md, install.md, quickstart.md, stealth.md, multi-tab.md, frames.md, input.md |
| 8 | mdBook chapters: optional features + reference | interception.md, expect.md, cloudflare.md, fetcher.md, migration-playwright.md, architecture.md, faq.md, error-reference.md |
| 9 | docs.yml CI workflow | .github/workflows/docs.yml |
| 10 | README final polish | README.md |
| 11 | CHANGELOG.md + SEMVER.md + CONTRIBUTING.md | root |
| 12 | scripts/publish.sh + Cargo.toml metadata polish | scripts/publish.sh, all crates' Cargo.toml |
| 13 | Version bump 0.1.0-dev → 0.1.0 | root Cargo.toml |
| 14 | Dry-run publish + verify | run `scripts/publish.sh --dry-run` |
| 15 | Tag v0.1.0 + actual publish | git tag + scripts/publish.sh |

---

## Task 0: Naming pass — `_raw` → `_fast`

**Files:** `crates/zendriver/src/element/{actions,input}.rs`, all tests/examples that call them, `crates/zendriver/src/lib.rs` re-exports if any.

- [ ] **Step 1: grep + rename**

```bash
grep -rln "click_raw\|hover_raw\|type_text_raw" crates/ | xargs sed -i.bak \
    -e 's/click_raw/click_fast/g' \
    -e 's/hover_raw/hover_fast/g' \
    -e 's/type_text_raw/type_text_fast/g'
find crates/ -name "*.bak" -delete
```

Verify the sed didn't catch unrelated matches (`type_text_raw` in a comment about CDP for example — those should be updated too since they refer to the API).

- [ ] **Step 2: Verify**

```bash
cargo build --workspace --all-features --locked
cargo test --workspace --lib --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo fmt --all --check
```

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "refactor(zendriver): rename Element::*_raw → *_fast across the API

The _raw suffix read as 'low-level CDP escape hatch' but the actual
semantic is 'skip realistic timing'. _fast communicates that better.
Pre-1.0 rename; no deprecation alias.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 1: `#[non_exhaustive]` audit drops

**Files:** Various across crates (see spec table for full list).

11 enums to drop the attribute from: Platform, Channel, ProfileKind, MouseButton, SpecialKey, DialogType, DownloadProgressState, FetcherPhase, RequestStage, Format, ClearanceOutcome.

11 enums KEEP: all `*Error`, `AriaRole`, `ResourceType`, `AbortReason`.

- [ ] **Step 1: Grep for current location of each target enum**

```bash
for e in Platform Channel ProfileKind MouseButton SpecialKey DialogType DownloadProgressState FetcherPhase RequestStage Format ClearanceOutcome; do
    echo "--- $e ---"
    grep -rln "pub enum $e" crates/
done
```

- [ ] **Step 2: For each target enum, delete the `#[non_exhaustive]` line directly above the `pub enum` declaration**

Use Edit per file. Don't bulk-sed since `#[non_exhaustive]` also appears above some structs we want to keep.

- [ ] **Step 3: Verify**

```bash
cargo build --workspace --all-features --locked
cargo test --workspace --lib --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

If any test was using a wildcard `_` arm that's now an unreachable pattern warning, remove it (clippy will flag).

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "refactor: drop #[non_exhaustive] from 11 stable-shape enums

Drops on Platform/Channel/ProfileKind/MouseButton/SpecialKey/DialogType/
DownloadProgressState/FetcherPhase/RequestStage/Format/ClearanceOutcome.
Variant sets conceptually closed. Users get exhaustive matching without
_ wildcard. KEEP on all error enums + AriaRole (has Other escape hatch
already) + ResourceType (CDP-driven) + AbortReason (CDP-driven).

Documented in SEMVER.md (T11): #[non_exhaustive] enums may grow in
minor bumps post-1.0; the dropped ones are committed-stable.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 2: Trait extraction — Queryable + Evaluable

**Files:**
- Create: `crates/zendriver/src/traits/mod.rs`
- Create: `crates/zendriver/src/traits/queryable.rs`
- Create: `crates/zendriver/src/traits/evaluable.rs`
- Modify: `crates/zendriver/src/tab.rs` (impl both traits)
- Modify: `crates/zendriver/src/frame/mod.rs` (impl both traits)
- Modify: `crates/zendriver/src/element/mod.rs` (impl Queryable only)
- Modify: `crates/zendriver/src/lib.rs` (pub mod traits + re-exports)

- [ ] **Step 1: Create trait module**

`crates/zendriver/src/traits/mod.rs`:

```rust
//! Public traits enabling generic code over Tab + Frame + Element.

pub mod evaluable;
pub mod queryable;

pub use evaluable::Evaluable;
pub use queryable::Queryable;
```

`crates/zendriver/src/traits/queryable.rs`:

```rust
//! Shared element-query surface across types that own a query scope.

use crate::query::{FindAllBuilder, FindBuilder};

/// Types that expose `find()` + `find_all()` queries scoped to themselves.
///
/// Implemented by `Tab` (queries scoped to main frame), `Frame` (queries
/// scoped to that frame's contextId), and `Element` (queries scoped to
/// the element's subtree).
pub trait Queryable {
    /// Start a single-element query.
    fn find(&self) -> FindBuilder<'_>;

    /// Start a multi-element query.
    fn find_all(&self) -> FindAllBuilder<'_>;
}
```

`crates/zendriver/src/traits/evaluable.rs`:

```rust
//! Shared JS-evaluation surface across types that own a CDP session/contextId.

use serde::de::DeserializeOwned;

use crate::error::Result;

/// Types that can evaluate JavaScript in their context.
///
/// Implemented by `Tab` (main frame) and `Frame` (per-frame contextId).
/// Element evaluation has a different shape (binds `el` parameter) and
/// has its own `Element::evaluate` / `Element::evaluate_main` methods.
#[async_trait::async_trait]
pub trait Evaluable {
    /// Evaluate JS in an isolated world (sandbox; no page globals visible).
    /// Default for stealth-safe execution.
    async fn evaluate<T>(&self, js: &str) -> Result<T>
    where
        T: DeserializeOwned + Send + 'static;

    /// Evaluate JS in the main world (page globals accessible).
    async fn evaluate_main<T>(&self, js: &str) -> Result<T>
    where
        T: DeserializeOwned + Send + 'static;
}
```

Note: trait methods take `&str` not `impl AsRef<str>` for object safety. Inherent methods on Tab/Frame keep the impl-AsRef shape; trait impls forward as `self.evaluate(js)` where `js: &str`.

- [ ] **Step 2: Wire impls**

In `crates/zendriver/src/tab.rs`:

```rust
impl crate::traits::Queryable for Tab {
    fn find(&self) -> crate::query::FindBuilder<'_> { Tab::find(self) }
    fn find_all(&self) -> crate::query::FindAllBuilder<'_> { Tab::find_all(self) }
}

#[async_trait::async_trait]
impl crate::traits::Evaluable for Tab {
    async fn evaluate<T>(&self, js: &str) -> crate::error::Result<T>
    where T: serde::de::DeserializeOwned + Send + 'static {
        Tab::evaluate(self, js).await
    }
    async fn evaluate_main<T>(&self, js: &str) -> crate::error::Result<T>
    where T: serde::de::DeserializeOwned + Send + 'static {
        Tab::evaluate_main(self, js).await
    }
}
```

Same shape for `Frame` (find/find_all + evaluate/evaluate_main).

For `Element` — only `Queryable` (find/find_all element-scoped):

```rust
impl crate::traits::Queryable for Element {
    fn find(&self) -> crate::query::FindBuilder<'_> { Element::find(self) }
    fn find_all(&self) -> crate::query::FindAllBuilder<'_> { Element::find_all(self) }
}
```

- [ ] **Step 3: lib.rs re-exports**

```rust
pub mod traits;
pub use traits::{Evaluable, Queryable};
```

- [ ] **Step 4: Add tests demonstrating generic usage**

Append to `crates/zendriver/src/traits/mod.rs::tests`:

```rust
#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Compile-only: verify a generic function over Queryable accepts
    /// Tab, Frame, and Element.
    #[allow(dead_code)]
    async fn _accepts_queryable<Q: Queryable + Sync>(q: &Q) {
        let _ = q.find();
        let _ = q.find_all();
    }

    /// Compile-only: verify a generic function over Evaluable accepts
    /// Tab and Frame.
    #[allow(dead_code)]
    async fn _accepts_evaluable<E: Evaluable + Sync>(e: &E) {
        let _: crate::Result<i32> = e.evaluate("1+1").await;
        let _: crate::Result<i32> = e.evaluate_main("1+1").await;
    }
}
```

- [ ] **Step 5: Verify**

```bash
cargo build --workspace --all-features --locked
cargo test --workspace --lib --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(zendriver): Queryable + Evaluable traits over Tab/Frame/Element

Enables generic helpers that work across query-scope types. Traits are
additive — inherent methods stay; impls forward. Element implements
only Queryable (evaluate has a different element-bound shape).

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 3: Trait derive audit

**Files:** various public structs

- [ ] **Step 1: Grep public structs missing Debug**

```bash
# Find pub structs that lack #[derive(Debug)] (or impl Debug)
for f in $(find crates/*/src -name "*.rs" -not -path "*/tests/*"); do
    grep -B 3 "^pub struct " "$f" | grep -L "Debug" | head -10
done
```

Manually inspect a handful and add `#[derive(Debug)]` or hand-rolled `impl Debug` where the struct contains non-Debug fields (Arc<dyn Fn>, SessionHandle, etc).

- [ ] **Step 2: Audit public enums missing Display where it would help**

Most error enums already have it via thiserror. Audit any non-error enum that gets surfaced in user output (e.g. AbortReason, MouseButton — debug printing them in user code is normal).

- [ ] **Step 3: Add Send + Sync bounds explicitly where missing**

Most public types in async code want `Send + Sync`. Add explicit `+ Send + Sync` to any pub trait that doesn't have it (Queryable already inherits from default; Evaluable's async-trait macro handles it via async_trait crate).

- [ ] **Step 4: Verify**

```bash
cargo build --workspace --all-features --locked
cargo test --workspace --lib --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "chore: add missing Debug/Display/Send+Sync derives on public types

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

# Tasks 4-15 — Compact form

Patterns from P1-P5 carry forward. Per-task: implement per spec, verify build + clippy + tests + fmt clean, commit. Spec at `docs/superpowers/specs/2026-05-23-zendriver-rs-phase6-release-design.md` is source of truth.

## Task 4: Rustdoc coverage — zendriver core

**Files:** `crates/zendriver/src/{lib,browser,tab,query/mod,query/role,element/mod,element/reads,element/actions,element/input,element/traversal,element/isolated_eval,element/screenshot,element/refresh,frame/mod,cookies/mod,storage/mod,screenshot/mod,input/{mod,keyboard,mouse},error,expect/{mod,request,response,dialog,download}}.rs`

**Implement:** Every `pub` item gets a `///` doc with brief summary + (for fn) Errors section + at least one `no_run` doctest where the function takes args. Pattern per spec section "Rustdoc coverage pass".

For internal `pub(crate)` items, doc is optional but encouraged.

For long enums (AbortReason has 14 variants), one-line doc per variant suffices.

**Verify:**
- `cargo doc --workspace --no-deps --all-features` zero warnings
- `cargo test --workspace --doc --all-features --locked` all pass
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` clean

This is a multi-hour pass — expect 200+ doc additions. Implementer breaks into logical chunks (per-file commits) for review.

**Commit:** `docs(zendriver): rustdoc + doctests for public API surface`

## Task 5: Rustdoc coverage — sub-crates

**Files:** `crates/zendriver-{transport,stealth,interception,cloudflare,fetcher}/src/**/*.rs`

**Implement:** same pass over each sub-crate. Internal-only crates (transport) get rustdoc on `pub` items but doctests can be sparse (calling them requires the full Browser stack — hard to write a useful no_run example). Other sub-crates need doctests where call sites can be reasonably illustrated.

**Verify:** same as T4 across all crates.

**Commit:** `docs(sub-crates): rustdoc + doctests for transport/stealth/interception/cloudflare/fetcher`

## Task 6: mdBook scaffolding

**Files:** `docs/book/book.toml`, `docs/book/src/SUMMARY.md`, stub chapter files

**Implement:**
1. `book.toml` per spec section "mdBook chapter index" (with linkcheck output).
2. `SUMMARY.md` lists all 14 chapters per spec.
3. Each chapter file gets a single-line stub (`# <title>\n\nTODO`) — content lands in T7/T8.
4. `docs/book/.gitignore` excludes `book/` build output.

Install + build verification:

```bash
cargo install mdbook mdbook-linkcheck  # one-time
mdbook build docs/book
```

Expect successful build with placeholder content.

**Commit:** `docs(book): mdBook scaffolding with SUMMARY + stub chapters`

## Task 7: mdBook core chapters

**Files:** `docs/book/src/{introduction,install,quickstart,stealth,multi-tab,frames,input}.md`

**Implement:** flesh out each per spec. Use `{{#include ../../../crates/zendriver/examples/<file>.rs:tag}}` to embed code from real examples (mdBook include directive) so docs stay in sync with code. Each chapter ~400-1200 words.

**Verify:** `mdbook build docs/book` zero warnings; `mdbook-linkcheck` clean.

**Commit:** `docs(book): core chapters (introduction/install/quickstart/stealth/multi-tab/frames/input)`

## Task 8: mdBook reference + feature chapters

**Files:** `docs/book/src/{interception,expect,cloudflare,fetcher,migration-playwright,architecture,faq,error-reference}.md`

**Implement:** flesh out each. `migration-playwright.md` is a Playwright→zendriver-rs crosswalk table. `architecture.md` covers CDP actor + observer + auto-refresh design. `faq.md` answers 10-15 questions ("how do I run headed?", "why am I getting NotActionable?", "does this work on M1 Mac?", etc). `error-reference.md` is a table mapping each ZendriverError variant to common causes + fixes.

**Verify:** `mdbook build docs/book` zero warnings; linkcheck clean.

**Commit:** `docs(book): reference chapters (interception/expect/cloudflare/fetcher/migration/architecture/faq/error-reference)`

## Task 9: docs.yml CI workflow

**Files:** `.github/workflows/docs.yml`

**Implement:** workflow per spec — triggers on push to main + on `docs/book/**` or `crates/**/src/**` changes. Steps:
1. Install Rust toolchain.
2. Install mdbook + mdbook-linkcheck.
3. `cargo doc --workspace --no-deps --all-features` (for rustdoc deploy under `/api/`).
4. `mdbook build docs/book`.
5. Combine `target/doc/` (rustdoc) under `book/api/` subdirectory of the mdBook output.
6. Deploy via `peaceiris/actions-gh-pages@v3` to `gh-pages` branch.

Plus a PR-only job that runs the build without deploy (validate-docs).

**Verify:** YAML syntax valid; push to feature branch and verify the validate-docs job runs successfully (deploy job won't fire from PR — correct).

**Commit:** `ci: docs workflow builds mdBook + rustdoc and deploys to gh-pages`

## Task 10: README final polish

**Files:** `README.md`

**Implement:** structure per spec section "README polish":
1. Title + tagline ("async-first, undetectable browser automation via CDP")
2. Status badges (crates.io, docs.rs, MSRV, license dual, CI status, nightly-stealth status)
3. Quick example (15 lines showing find-by-text + click + evaluate_main)
4. Feature matrix table — default + interception + expect + cloudflare + fetcher + use case + dep cost
5. Install snippets (3 use cases per spec)
6. Phases summary (linked to mdBook)
7. Comparison table — vs chromiumoxide / fantoccini / headless_chrome / thirtyfour (rows: API ergonomics / stealth out-of-box / multi-tab / interception / license / Send+Sync correctness / async runtime); mark subjective rows
8. Contributing + License pointers

Badges:
- `https://img.shields.io/crates/v/zendriver.svg`
- `https://docs.rs/zendriver/badge.svg`
- `https://img.shields.io/badge/rustc-1.75+-lightgray.svg`
- `https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg`
- `https://github.com/TurtIeSocks/zendriver-rs/actions/workflows/ci.yml/badge.svg`

**Verify:** Render README locally (`grip` or just `cat`) to confirm tables format correctly.

**Commit:** `docs: README final polish (badges + feature matrix + comparison + install snippets)`

## Task 11: CHANGELOG.md + SEMVER.md + CONTRIBUTING.md

**Files:** `CHANGELOG.md`, `SEMVER.md`, `CONTRIBUTING.md` (all NEW at repo root)

**Implement:**

`CHANGELOG.md` — Keep a Changelog format. Single 0.1.0 entry:

```markdown
# Changelog

All notable changes to this project documented here. Format: [Keep a
Changelog](https://keepachangelog.com/en/1.1.0/). Adheres to [SEMVER.md].

## [Unreleased]

## [0.1.0] - 2026-MM-DD

First public release. Built across 6 phases of internal development.

### Added
- CDP transport actor + minimal Browser/Tab/Element surface (Phase 1)
- Anti-detection (StealthProfile::native + spoofed; chaser-oxide-derived
  protocol patches; nightly stealth tests) (Phase 2)
- Full Element + FindBuilder surface (xpath/text/role selectors, hover/
  focus/scroll, type_text with realistic Bezier mouse + per-key typing,
  file upload, attributes, traversal, true isolated-world evaluate,
  actionability checks, auto-refresh on stale handles, screenshots)
  (Phase 3)
- Multi-tab management + Frame as first-class type (OOPIF supported)
  + CookieJar (CRUD + JSON persistence) + per-Tab Storage + nav history
  + wait_for_idle + traversal-origin refresh chain + per-Tab Input
  (Phase 4)
- Optional gated features: zendriver-interception (Fetch.* CDP wrapper
  with rule-based + Stream API), in-tree expect() module (Playwright-
  style pre-register + await), zendriver-cloudflare (Turnstile bypass),
  zendriver-fetcher (custom Chrome download via Chrome for Testing JSON
  API) (Phase 5)
- Trait extraction (Queryable + Evaluable across Tab/Frame/Element);
  rustdoc + doctest coverage; mdBook docs site; CHANGELOG + SEMVER;
  topological publish script (Phase 6)

### Internal phases (pre-release)
Phases 1-5 were internal development with API churn allowed.
No upgrade path needed; v0.1.0 is the first published release.
```

`SEMVER.md` — 1 page:

```markdown
# Versioning Policy

zendriver-rs follows [Semantic Versioning](https://semver.org/).

## Pre-1.0 (0.x.y)

While the major version is 0:
- Minor bumps (0.X.0) MAY include breaking changes.
- Patch bumps (0.x.Y) are non-breaking bug fixes.
- We aim to minimize churn but won't artificially delay improvements.

## Post-1.0

Standard SemVer. Breaking changes require a major bump.

## `#[non_exhaustive]` enums

Enums marked `#[non_exhaustive]` MAY gain variants in minor bumps even
post-1.0 — adding a variant is not considered a breaking change for these
types. As of 0.1.0, this set is: all `*Error` types, `AriaRole`,
`ResourceType`, `AbortReason`.

Other enums (Platform, Channel, ProfileKind, MouseButton, SpecialKey,
DialogType, DownloadProgressState, FetcherPhase, RequestStage, Format,
ClearanceOutcome) are committed-stable; adding variants is a SemVer
break.

## Internal crates

`zendriver-transport` is internal and its API may change in any minor
release without warning. Depend on the `zendriver` crate's re-exports
instead.

## MSRV

We support Rust 1.75 minimum. MSRV bumps follow the same SemVer rules
as API changes — minor bump for MSRV bump.
```

`CONTRIBUTING.md` — short:

```markdown
# Contributing

Issues and PRs welcome.

## Naming policy

- Builder pattern for configurable APIs (`tab.find().css("...").one().await`).
- `_fast` suffix for "skip realism, prefer speed" variants of input methods.
- `_main` suffix only for "in main world" JS evaluation (not a quality suffix).
- Avoid `_raw`, `_simple`, `_default` suffixes (ambiguous).

## Adding features

Each optional surface gates behind a Cargo feature flag (see
zendriver/Cargo.toml [features]). Examples in `crates/zendriver/examples/`
gated via `required-features`.

## Tests

- Unit tests: `cargo test --workspace --lib --locked`
- Integration tests: `cargo test --workspace --features integration-tests
  --test '*' --locked -- --test-threads=1` (requires Chrome installed)
- Nightly stealth: cron `0 6 * * *` against sannysoft + areyouheadless

## Releases

`scripts/publish.sh --dry-run` for verification; `scripts/publish.sh` for
actual publish (after `git tag vX.Y.Z`).
```

**Commit:** `docs: CHANGELOG.md + SEMVER.md + CONTRIBUTING.md`

## Task 12: scripts/publish.sh + Cargo.toml metadata polish

**Files:** `scripts/publish.sh` (NEW), all 6 `crates/*/Cargo.toml`

**Implement:**

1. `scripts/publish.sh` per spec section "Publish script". chmod +x.

2. Per-crate `Cargo.toml` metadata audit:
- `[package]` block: ensure `description`, `keywords` (≤5), `categories` (e.g. `["web-programming::browsers", "web-programming"]`), `readme = "../../README.md"`, `homepage`, `documentation = "https://docs.rs/<crate-name>"` are set.
- `[package.metadata.docs.rs]` block for docs.rs to enable all features: `all-features = true` + `rustdoc-args = ["--cfg", "docsrs"]`.
- Verify no `version = "0.1.0-dev"` lingering — all should use `version.workspace = true`.

3. Update `[package.metadata.docs.rs]` config so each crate's docs.rs build enables relevant features.

**Verify:** `cargo publish --dry-run --locked` for each crate (run from each crate dir) succeeds — validates Cargo.toml manifest.

**Commit:** `chore: scripts/publish.sh + per-crate Cargo.toml metadata for crates.io`

## Task 13: Version bump 0.1.0-dev → 0.1.0

**Files:** root `Cargo.toml`

**Implement:** workspace package `version = "0.1.0-dev"` → `version = "0.1.0"`. Run `cargo update -p zendriver -p zendriver-transport ...` if needed to refresh lockfile.

**Verify:**
- `cargo build --workspace --all-features --locked` clean
- `cargo test --workspace --all-features --locked` all pass
- All Cargo.lock entries for our crates show 0.1.0

**Commit:** `chore: version 0.1.0-dev → 0.1.0 for first public release`

## Task 14: Dry-run publish + verify

**Implement (manual, not a code change):**
1. `./scripts/publish.sh --dry-run` from repo root. Expected output: each of 6 crates runs `cargo publish --dry-run --locked` successfully.
2. Verify docs.rs metadata: `cargo doc --workspace --no-deps --all-features` produces clean output with rustdoc for every public API.
3. `cargo package --workspace --all-features --locked` validates manifest constraints.

If any crate fails dry-run, fix Cargo.toml + retry. Common issues: missing description, keywords exceeding 5 entries, categories not in crates.io list, README path wrong.

**No commit** if no fixes needed. Otherwise commit fix and re-verify.

## Task 15: Tag v0.1.0 + actual publish

**Implement (manual, after T14 passes):**

1. Verify on `main` branch with no uncommitted changes (post-merge).
2. `git tag v0.1.0`
3. `git push origin v0.1.0`
4. `./scripts/publish.sh` (no --dry-run)
5. Verify each crate appears on crates.io: `https://crates.io/crates/zendriver` and the 5 sub-crates.
6. Wait 5-10 minutes; check `https://docs.rs/zendriver/0.1.0` builds successfully.
7. Verify `https://turtiesocks.github.io/zendriver-rs/` is live (T9 docs.yml deploys to gh-pages).
8. Smoke test: in a fresh Rust project, `cargo add zendriver --features stealth` + write hello-world example + `cargo run`. Verify it works against real example.com.

If any publish step fails (network, ownership, etc), debug + retry from the failed crate in the topological list.

**No commit** — this is a release operation, not a code change. Optionally write a release notes blog/discussion post afterward.

---

## Self-review checklist

**Spec coverage:** T0 = naming. T1 = non_exhaustive. T2 = traits. T3 = derives. T4-T5 = rustdoc. T6-T8 = mdBook. T9 = docs CI. T10 = README. T11 = CHANGELOG/SEMVER/CONTRIBUTING. T12-T13 = publish prep. T14-T15 = ship.

**Placeholder scan:** none. T4/T5/T6/T7/T8 are large doc tasks but the scope is well-defined (every pub item / every chapter listed in spec).

**Type consistency:** `Queryable`/`Evaluable` traits used consistently. `_fast` (not `_raw`) used in tests + examples after T0.

---

## Notes for the implementing engineer

1. **T4 (rustdoc) is the longest task.** Could touch 200+ items. Implementer breaks into per-file commits for review-ability; each commit covers one source file or a tight module subset.
2. **T7 + T8 (mdBook chapters) are content-heavy.** Each chapter ~400-1200 words. The included examples (`{{#include}}`) keep doc + code in sync but require the examples directory to be clean first.
3. **T15 is irreversible.** Once a version is published to crates.io, you cannot replace it (you can only yank, which is destructive). Triple-check the dry-run output before invoking the real publish.
4. **Order matters in T15.** Run scripts/publish.sh — DON'T parallelize. crates.io needs index propagation between dependent crates (transport must be live before stealth can depend on it).
5. **GitHub Pages requires repository config:** Settings → Pages → Source = gh-pages branch. Verify before T15 or the deploy will succeed but the URL will 404.
6. **`mdbook-linkcheck` may flag external URLs that 404.** Initially `continue-on-error: true`; flag them in the chapter content for cleanup later.
7. **Branch is `worktree-phase6-release`** in worktree under `.claude/worktrees/phase6-release/`.
