# zendriver-rs — Phase 6: Release Polish + crates.io Publish

**Date:** 2026-05-23
**Status:** Approved (delegate-mode brainstorming complete, ready for implementation plan)
**Phase:** 6 of 6 — final phase. After this, zendriver-rs is published + maintained on standard semver cadence.
**Depends on:** Phases 1-5, all on `main`

## Summary

Last phase before v0.1.0 ships to crates.io. Three big workstreams:

1. **Deep API review pass.** Trait-extract shared `Tab` / `Frame` / `Element` query + evaluation surface so we don't ship parallel methods with subtly different signatures. Naming pass — audit `_raw`/`_main` suffixes and other suffixes for consistency. Audit `#[non_exhaustive]` (drop on stable-shape enums per the brainstorming decision; keep on error + protocol-driven enums). Add missing trait derives (Clone, Debug, Send + Sync bounds where reasonable).
2. **Documentation buildout.** Rustdoc on every public method/type/field with at least one `no_run` doctest per method that takes args. mdBook docs site hosted on GitHub Pages with chapters: install, quick-start, stealth-tuning, multi-tab+frames, interception, expect(), cloudflare, fetcher, migration-from-Playwright, FAQ.
3. **Publish workflow.** Version bump `0.1.0-dev → 0.1.0`. CHANGELOG.md synthesized from P1-P5 commits. SEMVER.md policy doc (pre-1.0 churn allowed). README final polish (badges, feature matrix table, install snippet per use case). Workspace publish order resolved (transport → stealth → interception/fetcher → cloudflare → zendriver) with a `cargo-release` config or manual publish script. Dry-run gate then actual `cargo publish` to crates.io.

Phase 6 exit criterion: zendriver 0.1.0 published on crates.io; docs.rs successfully builds the published artifact; mdBook site live at `https://turtiesocks.github.io/zendriver-rs/`; CHANGELOG.md + SEMVER.md committed; tag `v0.1.0` pushed.

## Goals

- **Trait extraction:** `Queryable` trait with `find()` / `find_all()` shared by Tab + Frame + Element. `Evaluable` trait with `evaluate<T>()` / `evaluate_main<T>()` shared by Tab + Frame (Element keeps its own — different receiver semantics). Existing types implement the traits + keep their inherent methods (no breakage; traits are additive).
- **Naming audit:** rename `_raw` suffix to `_fast` (less jargon-y, "non-realistic" → "fast path"). Keep `_main` (clear meaning: "in main world", not just "default" suffix). Document the policy in CONTRIBUTING.md.
- **`#[non_exhaustive]` audit:** drop on `Platform`, `Channel`, `MouseButton`, `SpecialKey`, `ClearanceOutcome`, `ProfileKind`, `DialogType`, `DownloadProgressState`, `FetcherPhase`, `RequestStage`, `Format`. Keep on all `*Error` enums + `AriaRole` (has Other escape hatch) + `ResourceType` (CDP-driven) + `AbortReason` (CDP-driven).
- **Trait derive audit:** ensure `Debug + Clone` on all public structs where feasible. `Send + Sync` bounds explicit where needed. `Display` for enums where appropriate.
- **Rustdoc:** every `pub` item in every `lib.rs` accessible-from-root path has `///` doc with: brief sentence, optional `Examples` section, optional `Errors` section. At least one `no_run` doctest per method that takes args + returns Result.
- **mdBook site:** `docs/book/` with `book.toml` + chapter sources. CI workflow `.github/workflows/docs.yml` builds + deploys to `gh-pages` branch on push to main.
- **README:** badges (crates.io, docs.rs, MSRV, license, CI status), feature matrix table (default features + per-feature table with use case + dep cost), install snippet per use case ("just want to browse" / "stealth automation" / "scraping with interception"), quick example, link to mdBook.
- **CHANGELOG.md:** human-written summary per phase grouped under v0.1.0 entry. Includes breaking changes (since pre-1.0, every phase had them — bullet-list the major ones from P1→P5). Follow Keep a Changelog format.
- **SEMVER.md:** short policy doc — pre-1.0 we may break in minor bumps; post-1.0 standard semver. Document the `#[non_exhaustive]` enums as the "may grow without major bump" set.
- **Workspace publish order + script:** `scripts/publish.sh` or `cargo-release` config that publishes in dep-graph topological order (transport first, zendriver last). Includes dry-run mode. Handles 30s+ propagation wait between dependent publishes (crates.io index needs time).
- **Version bump:** `0.1.0-dev` → `0.1.0` everywhere via workspace inheritance.
- **Tag + publish:** `git tag v0.1.0` + push tag + `scripts/publish.sh` + verify `https://crates.io/crates/zendriver` shows 0.1.0 + verify docs.rs builds.

## Non-goals

- Migration from earlier `zendriver-rs` releases (no earlier releases exist).
- 1.0 stability commitment (deliberately staying 0.x).
- Cross-publishing to other Rust registries (cratesio only).
- Auto-publish CI workflow (manual `scripts/publish.sh` invocation is fine for v0.1; revisit if release cadence becomes monthly+).
- Bundled tutorial videos or hosted examples runner — pure docs are enough for v0.1.
- Translated docs (English only).
- crates.io owners other than the single project owner — invite contributors after launch.

## Architecture

### File layout delta

```
zendriver-rs/
├── Cargo.toml                          # version: 0.1.0-dev → 0.1.0
├── README.md                           # final polish: badges + feature matrix + install snippets
├── CHANGELOG.md                        # NEW
├── SEMVER.md                           # NEW
├── CONTRIBUTING.md                     # NEW (covers naming policy + how to add features)
├── crates/zendriver/src/
│   ├── lib.rs                          # final naming pass + non_exhaustive audit + rustdoc on root
│   ├── traits/                         # NEW directory
│   │   ├── mod.rs                      # Queryable + Evaluable trait definitions
│   │   ├── queryable.rs                # find / find_all surface
│   │   └── evaluable.rs                # evaluate / evaluate_main surface
│   └── (all existing P1-P5 files)      # add rustdoc + doctests to every pub item
├── crates/zendriver-{transport,stealth,interception,cloudflare,fetcher}/src/
│   └── lib.rs                          # rustdoc pass on each crate root + non_exhaustive audit
├── docs/book/                          # NEW directory (mdBook source)
│   ├── book.toml                       # mdBook config + theme + linkcheck
│   ├── src/
│   │   ├── SUMMARY.md                  # chapter list
│   │   ├── introduction.md             # what is zendriver-rs + comparison table
│   │   ├── install.md                  # cargo add zendriver --features ...
│   │   ├── quickstart.md               # 30-line hello world
│   │   ├── stealth.md                  # native vs spoofed + fingerprint customization
│   │   ├── multi-tab.md                # Browser::new_tab + Tab::activate
│   │   ├── frames.md                   # Frame as first-class + OOPIF
│   │   ├── input.md                    # realistic vs fast input
│   │   ├── interception.md             # rule-based + stream
│   │   ├── expect.md                   # pre-register + await
│   │   ├── cloudflare.md               # Turnstile bypass
│   │   ├── fetcher.md                  # Chrome download
│   │   ├── migration-playwright.md     # Playwright user crosswalk
│   │   ├── architecture.md             # CDP actor + observer + auto-refresh
│   │   ├── faq.md                      # 10-15 common questions
│   │   └── error-reference.md          # ZendriverError variant table
├── scripts/
│   └── publish.sh                      # NEW topological publish script
└── .github/workflows/
    ├── ci.yml                          # unchanged
    └── docs.yml                        # NEW; builds mdBook + deploys to gh-pages
```

### Trait architecture

```rust
// crates/zendriver/src/traits/queryable.rs
pub trait Queryable {
    fn find(&self) -> FindBuilder<'_>;
    fn find_all(&self) -> FindAllBuilder<'_>;
}

// crates/zendriver/src/traits/evaluable.rs
#[async_trait::async_trait]
pub trait Evaluable {
    /// Evaluate JS in an isolated world (sandbox). Default for stealth.
    async fn evaluate<T: DeserializeOwned + Send>(&self, js: impl AsRef<str> + Send) -> Result<T>;

    /// Evaluate JS in the main world (page globals accessible).
    async fn evaluate_main<T: DeserializeOwned + Send>(&self, js: impl AsRef<str> + Send) -> Result<T>;
}
```

`Tab` + `Frame` impl both traits. `Element` impls `Queryable` (element-scoped) + has its own `evaluate` methods (different — needs to bind `el` parameter, which the trait can't capture cleanly).

The existing inherent methods stay — traits are purely additive. Users can write generic code like:

```rust
async fn click_first<Q: Queryable + Sync>(q: &Q, sel: &str) -> Result<()> {
    q.find().css(sel).one().await?.click().await
}
```

### `#[non_exhaustive]` audit — decisions per type

| Type | Has `#[non_exhaustive]` now? | Decision |
|---|---|---|
| `ZendriverError` | Yes | KEEP (will add variants) |
| `BrowserError` | Yes | KEEP (will add variants) |
| `TransportError` | Yes | KEEP (CDP transport edge cases) |
| `CallError` | Yes | KEEP (CDP RPC errors are rich) |
| `ObserverError` | Yes | KEEP (future observer kinds) |
| `StealthError` | Yes | KEEP |
| `InterceptionError` | Yes | KEEP |
| `CloudflareError` | Yes | KEEP |
| `FetcherError` | Yes | KEEP |
| `Platform` | (probably yes) | DROP — 5 OS variants stable |
| `Channel` | (yes) | DROP — 4 Chrome channels stable |
| `ProfileKind` | (yes) | DROP — Off/Native/Spoofed final |
| `MouseButton` | unknown | DROP — 5 standard mouse buttons |
| `SpecialKey` | unknown | DROP — adding keys is rare; exhaustive matches user-friendly |
| `DialogType` | (yes) | DROP — 4 HTML dialog types final per spec |
| `DownloadProgressState` | (yes) | DROP — 3 states |
| `FetcherPhase` | (yes) | DROP — 5 phases |
| `RequestStage` | (yes) | DROP — 2 stages (Request/Response) |
| `Format` | unknown | DROP — PNG/JPEG/WebP cover image formats |
| `ClearanceOutcome` | (yes) | DROP — 2 outcomes |
| `AriaRole` | (yes) | KEEP — `Other(&'static str)` escape hatch covers extension; dropping non_exhaustive doesn't hurt much; but new W3C roles do get added |
| `ResourceType` | unknown | KEEP — CDP-driven |
| `AbortReason` | unknown | KEEP — CDP-driven |

Net: drop on 11 enums, keep on 11. Roughly half-half.

### Naming pass — concrete changes

| From | To | Rationale |
|---|---|---|
| `click_raw` | `click_fast` | "raw" reads as low-level CDP; "fast" reads as "skip realistic timing" |
| `hover_raw` | `hover_fast` | same |
| `type_text_raw` | `type_text_fast` | same |
| `evaluate_main` | unchanged | "main" = main world, accurate; not a quality suffix |
| `screenshot` | unchanged | obvious default |
| `screenshot_builder` | unchanged | clear "use builder for options" |
| `wait_for_idle` | unchanged | clear |
| `wait_for_load` | unchanged | clear |
| `wait_for_clearance` | unchanged | clear |
| `is_challenge_present` | unchanged | reads naturally |
| `Tab::expect_request` etc | unchanged | matches Playwright; established pattern |

The `_raw` → `_fast` rename is the only widespread change. Add `#[deprecated(note = "renamed to *_fast")]` aliases to soften the break (but break is fine per pre-1.0 policy; aliases are courtesy for any pre-publish users).

Actually — given memory `api-churn-acceptable-pre-release` + nothing has been published yet, skip the deprecation aliases. Clean rename.

### mdBook chapter index

```toml
# docs/book/book.toml
[book]
title = "zendriver-rs"
authors = ["zendriver-rs contributors"]
language = "en"
src = "src"

[output.html]
git-repository-url = "https://github.com/TurtIeSocks/zendriver-rs"
edit-url-template = "https://github.com/TurtIeSocks/zendriver-rs/edit/main/docs/book/{path}"
default-theme = "ayu"

[output.linkcheck]
# Check internal + external links during build
```

Add `mdbook-linkcheck` dep in CI for the docs job.

### Publish script

```bash
#!/usr/bin/env bash
# scripts/publish.sh
# Topological publish order. Run from repo root.
# Usage: ./scripts/publish.sh [--dry-run]
set -euo pipefail

DRY_RUN=""
if [[ "${1:-}" == "--dry-run" ]]; then DRY_RUN="--dry-run"; fi

CRATES=(
    zendriver-transport
    zendriver-stealth
    zendriver-interception
    zendriver-fetcher
    zendriver-cloudflare
    zendriver
)

for crate in "${CRATES[@]}"; do
    echo "==> Publishing $crate $DRY_RUN"
    (cd "crates/$crate" && cargo publish $DRY_RUN --locked)
    if [[ -z "$DRY_RUN" ]]; then
        echo "Waiting 30s for crates.io index propagation..."
        sleep 30
    fi
done
```

Order rationale: transport has no zendriver-* deps; stealth depends on transport; interception depends on transport; fetcher depends on nothing internal; cloudflare depends on transport+interception; zendriver depends on all five.

### CI changes

- New `.github/workflows/docs.yml`: builds mdBook on push to main + deploys to gh-pages branch via `peaceiris/actions-gh-pages`. Triggers on changes to `docs/book/**` or any `crates/**/src/**/*.rs` (rustdoc could change).
- Existing `.github/workflows/ci.yml`: add a `docs-build` job that builds mdBook in PR + runs `cargo doc --workspace --no-deps --all-features` (verifies rustdoc compiles cleanly).

## Components — naming pass (4 renames)

- `Element::click_raw` → `Element::click_fast`
- `Element::hover_raw` → `Element::hover_fast`
- `Element::type_text_raw` → `Element::type_text_fast`
- (no other `_raw` suffixes in public API)

Touched files: `crates/zendriver/src/element/{actions,input}.rs` + their tests + any examples + docs.

## Components — `#[non_exhaustive]` audit (11 drops)

Mechanical edit: delete `#[non_exhaustive]` line above the listed 11 enums. Re-run clippy + tests.

## Components — trait extraction

New module `crates/zendriver/src/traits/` with `Queryable` + `Evaluable` traits per architecture. Tab + Frame + Element get `impl Queryable for ...` blocks. Tab + Frame get `impl Evaluable for ...`. All trait methods delegate to existing inherent methods.

Re-export from `crates/zendriver/src/lib.rs`: `pub use traits::{Evaluable, Queryable};`.

## Components — rustdoc coverage pass

Every `pub fn` / `pub struct` / `pub enum` / `pub field` in every crate root reachable from `lib.rs` gets:

1. Brief one-line summary (imperative, no period).
2. Optional paragraph expansion for nuanced behavior.
3. `# Errors` section for `Result`-returning methods (which errors + when).
4. `# Panics` section for any method that can panic.
5. `# Examples` section with at least one `no_run` doctest (compile-tested via `cargo test --doc`).

Doctests must use `no_run` (don't spawn Chrome in test runs). Pattern:

```rust
/// Click this element.
///
/// # Examples
/// ```no_run
/// # use zendriver::Browser;
/// # async fn ex() -> zendriver::Result<()> {
/// let browser = Browser::builder().launch().await?;
/// let tab = browser.main_tab();
/// tab.goto("https://example.com").await?;
/// let btn = tab.find().css("button").one().await?;
/// btn.click().await?;
/// # Ok(()) }
/// ```
pub async fn click(&self) -> Result<()> { /* ... */ }
```

For crates that are internal (transport), rustdoc still required on public items but doctests can be skipped if calling them requires the full Browser stack.

## Components — mdBook content

Each chapter ~400-1200 words. Include code samples from `crates/zendriver/examples/` directly (mdbook's `{{#include}}` directive) so examples stay in sync.

Chapter dependencies:
- introduction → install → quickstart : foundational onramp
- stealth → multi-tab → frames → input : core API in dependency order
- interception → expect → cloudflare → fetcher : optional features by feature flag
- migration-playwright + architecture + faq + error-reference : reference material

## Components — README polish

Structure:

1. **Title + tagline**
2. **Status badges** — crates.io / docs.rs / MSRV / license / CI / nightly stealth status (manual update or shields.io custom)
3. **Quick example** — 15-line snippet (find by text + click + evaluate read-back)
4. **Feature matrix** — table of `interception`/`expect`/`cloudflare`/`fetcher` features with use case + extra deps
5. **Install snippets** — three: "minimal browse" / "stealth + multi-tab" / "everything"
6. **Phases summary** — link to mdBook chapters for details
7. **Comparison table** — vs chromiumoxide, fantoccini, headless_chrome, thirtyfour (rows: API ergonomics, stealth out-of-box, multi-tab, interception, license)
8. **Contributing + License**

## Components — CHANGELOG + SEMVER

`CHANGELOG.md` follows Keep a Changelog. Single 0.1.0 entry with subsections:

- Added (per-phase major features)
- Changed (per-phase API changes that broke previous internal-only iterations)
- Note: pre-1.0 release; all previous changes were internal pre-release; no upgrade path needed.

`SEMVER.md` — 1-page doc:

- Pre-1.0 policy: minor bumps may break.
- Post-1.0 policy: standard cargo/semver.
- `#[non_exhaustive]` enums may grow in minor bumps (won't be SemVer break).
- Internal modules (`zendriver-transport`) not subject to API stability promises.

## Error handling

P6 doesn't introduce new error variants. Audit existing for consistency:

- All `*Error` enums use `thiserror::Error`.
- Display strings start lowercase + don't end in period (Rust convention).
- Source chains via `#[source]` or `#[from]` everywhere.
- No `panic!` in any non-test code path.

`ZendriverError::Cdp` and similar "rich data" variants stay structured — don't flatten into String.

## Testing

P6 doesn't add test logic. Instead:

1. **Full sweep:** run `cargo test --workspace --all-features --locked` and `cargo test --workspace --doc --all-features --locked`. Both must pass after every renaming task.
2. **Doc lint:** `cargo doc --workspace --no-deps --all-features` must produce zero warnings (no broken intra-doc links).
3. **Publish dry-run:** `scripts/publish.sh --dry-run` from a clean tree must succeed for all 6 crates in order. cargo's `--dry-run` validates Cargo.toml metadata + manifest constraints without actually publishing.
4. **mdBook build:** `mdbook build docs/book` must produce zero warnings (mdbook-linkcheck catches broken links).
5. **Real publish:** done from main after final verification, NOT from a feature branch. Tag `v0.1.0` first, then `scripts/publish.sh`.

## Assumptions (delegate mode — judgement calls)

1. **`_raw` → `_fast` rename without deprecation aliases.** Clean break; no users to support. Pre-1.0 policy in effect.
2. **Trait extraction is additive.** Existing inherent methods stay. Users keep working; the traits enable generic helpers without forcing migration.
3. **Element doesn't implement `Evaluable`** — its `evaluate` takes a different shape (binds `el` parameter, returns a value from an expression-of-element). Different enough that the trait would either misrepresent it or need a different name. Skip; Element keeps its inherent `evaluate` + `evaluate_main`.
4. **`#[non_exhaustive]` drops are pure deletions.** No alternative encoding (e.g. `_phantom` field) needed. Compiler enforces exhaustive matches; future variant additions require a major-minor bump per SEMVER.md.
5. **mdBook hosted on `gh-pages` branch** via `peaceiris/actions-gh-pages` — most standard Rust ecosystem pattern. GitHub Pages serves from gh-pages by convention.
6. **`mdbook-linkcheck` runs in CI** but is `continue-on-error: true` initially. External link rot shouldn't block PRs; revisit if too noisy.
7. **CHANGELOG.md grouping** is single 0.1.0 entry, not per-phase entries. Reason: phases were internal scaffolding; users see "v0.1.0 had X features" not "P1 added Y, P2 added Z". Phases get a brief mention in the introduction.
8. **SEMVER.md is short** — single page. Detailed policy can land later if the project grows.
9. **`scripts/publish.sh` is bash** — not a Rust binary. Smaller, fewer deps. Run manually for v0.1; automate later if cadence demands.
10. **30s sleep between publishes** is a magic number based on crates.io index lag. Real number varies (10s to 2min). 30s is safe-ish; bump to 60s if first publish has failures.
11. **No `cargo-release` dep** — script suffices. cargo-release is great but adds setup overhead for a 6-crate workspace; manual script is auditable.
12. **`docs/book/` lives in repo root**, not in a `crates/*/docs/` path. Reason: docs cover the whole project, not any single crate.
13. **Trait method signatures use `impl AsRef<str> + Send`** for JS expression args — matches existing inherent methods. Generic on `T: DeserializeOwned + Send`.
14. **The traits live in `zendriver` crate**, not in `zendriver-transport`. Reason: they parameterize over `Tab`/`Frame`/`Element` which all live in zendriver core.
15. **The mdBook content is hand-written**, not auto-generated from rustdoc. Reason: tutorials need narrative; rustdoc is reference.
16. **GitHub Pages deploys on push to main only**, not on PRs. PR builds run mdBook in `--no-deploy` mode to verify it compiles but skip publishing.
17. **Tag format is `v0.1.0`** (with `v` prefix). Matches Rust ecosystem convention (cargo-release default; chromiumoxide + tokio etc).
18. **Comparison table in README** is best-effort (some rows are subjective). Mark subjective rows with "*opinion*" footnote.

## Roadmap

| Phase | Status | Goal |
|---|---|---|
| 1 | DONE | Foundation |
| 2 | DONE | Stealth |
| 3 | DONE | Element + input + actionability |
| 4 | DONE | Multi-tab + cookies + storage + frames |
| 5 | DONE | Gated optional features |
| **6 (this spec)** | IN PROGRESS | Release polish + crates.io publish |

After P6: standard semver-driven release cadence. 0.2.0 will absorb early community feedback (renames, ergonomic fixes, missing features). 1.0.0 lands when API has been stable for ~3 minor releases.

Sizing: 2-3 weeks solo (smaller than P3/P4/P5 — mostly mechanical docs + audit work).

## Brainstorm cross-ref

Decisions locked during brainstorming:
- **Version:** `0.1.0` (pre-1.0, API churn allowed).
- **Docs:** Rustdoc + mdBook on GitHub Pages.
- **`#[non_exhaustive]` audit:** selective drop on stable-shape enums (~11); keep on error + CDP-driven enums (~11).
- **API review:** deep — trait extraction (`Queryable`, `Evaluable`) + naming pass (`_raw` → `_fast`) + audit derives.
