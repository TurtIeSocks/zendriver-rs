# Timezone-from-Geo Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Derive a representative IANA timezone from a country code so `geo::persona(country)` sets `Persona.timezone` (already wired to `Emulation.setTimezoneOverride`).

**Architecture:** Extend the `locale-gen` crate to fetch IANA `zone1970.tab` and emit a generated `TIMEZONES` table (country → representative zone) + a curated override list; regenerate `geo/table.rs`; set the field in `geo::persona`.

**Tech stack:** Rust 2024; `locale-gen` (existing generator); vendored/generated tables; the geo module.

## Global Constraints
- **Generated files are hand-edit-free** — all logic (parse rule, overrides) lives in `locale-gen`, never in the emitted `geo/table.rs`. Keep every emitted symbol live (expose `tzdata_version()` like `cldr_version()`).
- **Never lock** — unknown country → `timezone: None` + warn; always overridable.
- **`TIMEZONES` must be sorted** (binary_search precondition), same as `COUNTRIES`.
- **Feature:** all under the existing `geo` feature; default build unaffected.
- **Before commit:** `cargo fmt --all`; `cargo clippy --workspace --all-targets --locked -- -D warnings`; `cargo clippy -p zendriver-stealth --all-targets --features geo -- -D warnings`.

---

### Task 1: `locale-gen` emits the `TIMEZONES` table

**Files:** Modify `crates/locale-gen/src/lib.rs` (parse + emit), `crates/locale-gen/src/main.rs` (fetch zone1970.tab), add a `crates/locale-gen/tests/fixtures/zone1970-mini.tab`.

**Interfaces:**
- Produces: `pub fn tz_rows_from_zone_tab(raw: &str) -> Vec<(String, String)>` (sorted country→zone, first-occurrence + overrides applied); extend `emit_rust` (or add `emit_timezones`) to render `TIMEZONES: &[(&str,&str)]` + `TZDATA_VERSION`.

- [ ] **Step 1: Failing tests** in `lib.rs`:
```rust
#[test]
fn tz_first_occurrence_per_country() {
    // fixture rows: "US ... America/New_York", "US ... America/Chicago" → US = New_York
    let rows = tz_rows_from_zone_tab(TZ_FIXTURE);
    assert_eq!(lookup(&rows, "US"), Some("America/New_York"));
    assert_eq!(lookup(&rows, "FR"), Some("Europe/Paris"));
}
#[test]
fn tz_override_wins() {
    // fixture has RU first row = Europe/Kaliningrad; override forces Moscow
    let rows = tz_rows_from_zone_tab(TZ_FIXTURE);
    assert_eq!(lookup(&rows, "RU"), Some("Europe/Moscow"));
}
#[test]
fn tz_rows_sorted() {
    let rows = tz_rows_from_zone_tab(TZ_FIXTURE);
    assert!(rows.windows(2).all(|w| w[0].0 <= w[1].0));
}
```
Add `crates/locale-gen/tests/fixtures/zone1970-mini.tab` with a few real-format rows (comment lines starting `#`, tab-separated: `codes\tcoords\tTZ\tcomments`), including a multi-code shared zone row and RU rows (Kaliningrad before Moscow) to exercise the override.

- [ ] **Step 2: Run — fails.** `cargo test -p locale-gen tz_`

- [ ] **Step 3: Implement** in `lib.rs`:
```rust
/// Curated primary-zone overrides for large multi-timezone countries whose
/// `zone1970.tab` first row is not the most-populous zone. Kept minimal.
const PRIMARY_TZ_OVERRIDES: &[(&str, &str)] = &[
    ("RU", "Europe/Moscow"),
    // Add others ONLY where the parsed first-occurrence is demonstrably not
    // the most-populous zone; verify against a real zone1970.tab before adding.
];

/// Parse `zone1970.tab` into a sorted `(country, IANA zone)` list: each
/// country's FIRST-appearing row wins, then `PRIMARY_TZ_OVERRIDES` are applied.
pub fn tz_rows_from_zone_tab(raw: &str) -> Vec<(String, String)> {
    use std::collections::BTreeMap;
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    for line in raw.lines() {
        if line.starts_with('#') || line.trim().is_empty() { continue; }
        let mut cols = line.split('\t');
        let codes = cols.next().unwrap_or("");
        let _coords = cols.next();
        let zone = cols.next().unwrap_or("").trim();
        if zone.is_empty() { continue; }
        for cc in codes.split(',') {
            map.entry(cc.to_string()).or_insert_with(|| zone.to_string()); // first wins
        }
    }
    for (cc, z) in PRIMARY_TZ_OVERRIDES {
        map.insert((*cc).to_string(), (*z).to_string());
    }
    map.into_iter().collect() // BTreeMap → already sorted by country
}
```
Extend the emitter to render (append to the same generated file body):
```rust
pub(crate) static TZDATA_VERSION: &str = "<tag>";
pub(crate) static TIMEZONES: &[(&str, &str)] = &[
    ("AD", "Europe/Andorra"),
    // ...
];
```
(A `lookup` test helper = `rows.iter().find(|(c,_)| c==cc).map(|(_,z)| z.as_str())`.)

- [ ] **Step 4: `main.rs`** — fetch `zone1970.tab` from a PINNED `eggert/tz` tag (mirror the existing CLDR_TAG pattern; add `TZDATA_TAG`), call `tz_rows_from_zone_tab`, pass to the emitter. Verify the raw URL resolves (`https://raw.githubusercontent.com/eggert/tz/<tag>/zone1970.tab`).

- [ ] **Step 5: Run — passes.** `cargo test -p locale-gen`

- [ ] **Step 6: Commit** — `feat(locale-gen): emit country->IANA timezone table from zone1970.tab`

---

### Task 2: Regenerate `geo/table.rs` + set timezone in `geo::persona`

**Files:** Run `locale-gen` → regenerate `crates/zendriver-stealth/src/geo/table.rs`; Modify `crates/zendriver-stealth/src/geo/mod.rs` (`timezone_for` + `persona` + `tzdata_version()`); Test: `geo/mod.rs`.

**Interfaces:** Consumes `table::{TIMEZONES, TZDATA_VERSION}`; Produces `pub fn tzdata_version() -> &'static str`, `pub(crate) fn timezone_for(cc: &str) -> Option<&'static str>`.

- [ ] **Step 1: Regenerate the table.** Run the `locale-gen` binary (per its README/main) so `geo/table.rs` gains `TIMEZONES` + `TZDATA_VERSION`. Do NOT hand-edit the generated file. Confirm `cargo build -p zendriver-stealth --features geo`.

- [ ] **Step 2: Failing tests** in `geo/mod.rs`:
```rust
#[test]
fn persona_sets_timezone_from_country() {
    assert_eq!(persona(Country::try_from("US").unwrap()).timezone.as_deref(), Some("America/New_York"));
}
#[test]
fn persona_timezone_uses_override() {
    assert_eq!(persona(Country::try_from("RU").unwrap()).timezone.as_deref(), Some("Europe/Moscow"));
}
#[test]
fn persona_locale_unchanged() {
    // existing locale/languages assertions still hold (regression)
    let p = persona(Country::try_from("US").unwrap());
    assert_eq!(p.locale.as_deref(), Some("en-US"));
}
```

- [ ] **Step 3: Run — fails** (timezone None).

- [ ] **Step 4: Implement** in `geo/mod.rs`:
```rust
/// The IANA tz-database release the vendored `TIMEZONES` table was generated
/// from. Keeps `TZDATA_VERSION` a live symbol (no generated-file `#[allow]`).
pub fn tzdata_version() -> &'static str { table::TZDATA_VERSION }

/// Representative IANA timezone for a country, or `None` if unmapped.
pub(crate) fn timezone_for(cc: &str) -> Option<&'static str> {
    table::TIMEZONES.binary_search_by(|(c, _)| (*c).cmp(cc)).ok().map(|i| table::TIMEZONES[i].1)
}
```
In `persona`, after building the locale/languages `Persona`, set its `timezone`:
```rust
let mut p = /* existing Persona with locale + languages */;
p.timezone = timezone_for(cc).map(str::to_string);
if p.locale.is_some() && p.timezone.is_none() {
    tracing::debug!("geo: no timezone mapping for country {cc}");
}
p
```
(Adapt to the existing `persona` structure — it currently returns the `Persona { locale, languages, ..Default::default() }` literal; add `timezone` there instead if cleaner. Keep the unknown-country early-return path returning `Persona::default()` unchanged.)

- [ ] **Step 5: Run — passes.** `cargo test -p zendriver-stealth --features geo geo::`

- [ ] **Step 6: Commit** — `feat(geo): derive representative timezone from country in geo::persona`

---

### Task 3: Docs, backlog, workflow guards

**Files:** `crates/zendriver-stealth/src/geo/mod.rs` (rustdoc — already partly in T2), `docs/book/src/*geo*`, `README.md`, `docs/superpowers/deferred-backlog.md`, `.github/workflows/sync-cldr-locale.yml`.

- [ ] **Step 1: Workflow guards.** In `sync-cldr-locale.yml`, add sanity checks for the regenerated tz table to its existing guard set: every `TIMEZONES` value matches `^[A-Za-z_]+/[A-Za-z0-9_+\-/]+$`, row count within bounds, `TZDATA_VERSION` non-empty. (Follow the existing CLDR guard pattern.)
- [ ] **Step 2: Docs.** mdBook geo chapter: note timezone is now derived from country (with the representative-zone caveat). README geo bullet if it lists derived fields. Confirm `mdbook build docs/book`.
- [ ] **Step 3: Backlog.**
  - Move the §1 geo "timezone-from-geo derivation" item to `✅ closed since snapshot` (shipped via `geo::persona` + `TIMEZONES`).
  - Add a new §1 geo follow-up: "`geo_auto` exact timezone from ip-api's `timezone` field (more precise than country-representative; needs a `GeoResolver` contract change)."
  - **Relabel #25:** move the §0 pool-URL item out of "🐞 Live failure (fix first)"; reframe as parked — a *clean, clearly-errored unavailability* (not a crash), blocked on the A2 dataset-curation decision; generative is the working default. (Retitle §0 or move the item to a new "⏸ Parked" note.)
- [ ] **Step 4: Final gates + commit.** `cargo fmt --all --check`; `cargo clippy --workspace --all-targets --locked -- -D warnings`; `cargo clippy -p zendriver-stealth --all-targets --features geo -- -D warnings`; `cargo test -p zendriver-stealth --features geo`; `cargo test -p locale-gen`. Commit `docs(geo): document derived timezone, sync backlog, park #25`.

---

## Self-Review
- Generator (T1) → table + overrides + version, fixture-tested. ✓
- Regenerate + wire persona (T2), regression on locale. ✓
- Docs + backlog + guards + #25 relabel (T3). ✓
- Generated-file-hand-edit-free honored (overrides in generator; `tzdata_version()` keeps the symbol live). ✓
- Types: `tz_rows_from_zone_tab`, `TIMEZONES`/`TZDATA_VERSION`, `timezone_for`/`tzdata_version` consistent across tasks. ✓
- Open risk: the exact `eggert/tz` tag + that `zone1970.tab`'s column order matches the parse (codes\tcoords\tzone\t…) — T1 Step 4 verifies the URL; T1 fixture must use the real column layout.
