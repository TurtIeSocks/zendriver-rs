# Design: timezone-from-geo derivation

**Date:** 2026-07-16 · **Status:** delegate-mode (assumptions flagged) · **Crates:** `locale-gen` (generator), `zendriver-stealth` (geo module)
**Backlog:** §1 geo — "timezone-from-geo derivation (country → IANA tz, sharing the `geo` module)".

## Problem
`geo::persona(country)` (`crates/zendriver-stealth/src/geo/mod.rs`) derives `locale` + `languages` but leaves `Persona.timezone = None`. The timezone plumbing already exists end-to-end: `Persona.timezone: Option<String>` → applied via `Emulation.setTimezoneOverride { timezoneId }` (`observer.rs:142`). So a country-derived persona (`geo_locale("US")`, and transitively `geo_auto()`) reports **no** timezone, a coherence gap vs its locale. This closes it.

## Design
Add a generated **country → representative IANA timezone** table and have `geo::persona` set the field.

### Data source + generation (extends `locale-gen`)
`locale-gen` already fetches CLDR `territoryInfo.json` (pinned tag) and emits `geo/table.rs`'s `COUNTRIES`. Extend it to ALSO fetch IANA **`zone1970.tab`** (from `eggert/tz`, pinned tag — verified present, ~17.6 KB) and emit a `TIMEZONES: &[(&str, &str)]` table (country code → IANA zone), sorted for `binary_search`.

- **Parse rule:** iterate `zone1970.tab` rows in order; column 1 is a comma-list of ISO country codes, column 3 is the IANA zone. For each country code, the FIRST row it appears in gives its representative zone. (Single-tz countries: exact. Multi-tz: first-listed, a reasonable representative.)
- **Curated override** (in the generator, NOT the generated file — preserves "generated output is hand-edit-free"): a small `PRIMARY_TZ_OVERRIDES: &[(&str,&str)]` for large multi-tz countries whose most-populous zone ≠ zone1970's first row. Seed with the clearly-wrong big cases (at minimum `RU → Europe/Moscow`; verify/add `US`, `CN`, `BR`, `AU`, `CA`, `ID`, `MX` against what the parse yields and override only where the parsed value is not the most-populous). Overrides win.
- **Provenance:** emit a `TZDATA_VERSION` const (the pinned tz release tag) next to `CLDR_VERSION`, exposed via a `pub fn tzdata_version()` (keeps the const a live symbol — same pattern as `cldr_version()`, avoids a generated-file `#[allow]`).
- **Auto-sync:** the existing weekly `sync-cldr-locale.yml` workflow regenerates both tables; add the tz sanity guards (every value parses as `Area/Location`, row-count bounds, version monotonic) to its existing guard set.

### Wiring
In `geo::persona(country)`, after setting locale/languages, look up `timezone_for(cc)` (binary_search the `TIMEZONES` table) and set `timezone: Some(zone)` when found. Unknown → leave `None` (+ the existing warn already covers the no-mapping case; add a tz-specific debug if locale mapped but tz didn't). Never locks — overridable via explicit `persona.timezone` / `persona_overlay`.

## Non-goals / follow-ups
- **`geo_auto` using ip-api's exact `timezone` field** — `ip-api.com/json` returns a precise `timezone` (e.g. `America/New_York`), more accurate than the country-representative zone. Using it would expand the `GeoResolver` trait contract (return a tz alongside `Country`). **Deferred** — noted as a follow-up in the backlog. For now `geo_auto` inherits the country-representative tz for free via `geo::persona`.
- Per-subdivision / DST-aware zone selection. Out of scope.

## Testing
- `locale-gen` unit: `zone1970.tab` fixture → parsed rows; first-occurrence rule; overrides win; emitted Rust shape valid (mirror the existing `rows_from_cldr` / `emit_is_valid_rust_shape` tests).
- `geo::persona`: `persona("US").timezone == Some("America/New_York")`, `persona("RU").timezone == Some("Europe/Moscow")` (override), an unknown/unmapped country → `None`, and locale/languages unchanged from before.
- Table integrity: every `TIMEZONES` value contains a `/` and round-trips; `TIMEZONES` is sorted (binary_search precondition).

## Docs / backlog
- Rustdoc on `geo::persona` (now sets timezone) + `tzdata_version()`. mdBook geo chapter: note timezone is now derived. README geo bullet if it enumerates derived fields.
- Flip the §1 "timezone-from-geo" backlog item to closed; add the ip-api-exact-tz follow-up under §1 geo.
- **Relabel #25** (parked this session): move §0's "🐞 live failure (fix first)" pool-URL item to a calmer framing — it is a *clean, clearly-errored unavailability*, not a crash; blocked on the A2 dataset-curation decision (see the #25 discussion). Note generative is the working default.

## Assumptions (delegate calls — veto at review)
1. **Representative zone via IANA `zone1970.tab` first-occurrence + curated overrides for big multi-tz countries.** (Alternative: a fuller CLDR primaryZones/metaZones derivation — rejected as heavier and not cleanly per-country.)
2. **Overrides live in the generator, seeded minimally** (only where the parse is demonstrably not the most-populous zone).
3. **ip-api exact-tz deferred** (trait-contract change), country-representative tz for now.
