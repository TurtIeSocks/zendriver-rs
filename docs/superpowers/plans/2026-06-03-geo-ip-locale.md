# Geo-IP-derived locale Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Derive a coherent `locale` + q-weighted `Accept-Language` + `navigator.languages` from an explicit country code, sourced from a vendored, auto-synced Unicode CLDR table.

**Architecture:** Locale primitives live in `zendriver-stealth` (the crate both `zendriver` and `zendriver-fingerprints` depend on). Always-on layer: a `languages` list on `Persona`/`Fingerprint` + a pure q-weight formatter. Feature-gated `geo` layer: a `Country` type + a generated CLDR table + a `country -> Persona` mapping. A non-published `locale-gen` bin regenerates the table; a weekly workflow opens an auto-merging PR gated on a sanity guard.

**Tech Stack:** Rust (edition 2024), `serde`, `tracing`, `async-trait`; generator uses `reqwest` (blocking) + `serde_json`; CI uses GitHub Actions + `peter-evans/create-pull-request`.

**Spec:** `docs/superpowers/specs/2026-06-03-geo-ip-locale-design.md`

---

## File Structure

| File | Responsibility | Task |
|------|----------------|------|
| `crates/zendriver-stealth/src/lang.rs` | **new** — `accept_language()` formatter + `resolve_languages()` precedence helper | T1, T4 |
| `crates/zendriver-stealth/src/persona/mod.rs` | `Persona.languages` field, overlay arm, builder setter | T2 |
| `crates/zendriver-stealth/src/fingerprint.rs` | `Fingerprint.languages` field | T3 |
| `crates/zendriver-stealth/src/profile.rs` | `per_field.languages`, `.languages()` setter, resolve copy | T3 |
| `crates/zendriver-stealth/src/patches.rs` | `navigator.languages` via `resolve_languages` | T5 |
| `crates/zendriver-stealth/src/observer.rs` | `acceptLanguage` via `accept_language` | T5 |
| `crates/zendriver-stealth/src/geo/mod.rs` | **new** — `Country`, `languages_for`, `persona()`, `GeoResolver` seam, guard test | T6, T8, T9, T10, T13 |
| `crates/zendriver-stealth/src/geo/table.rs` | **new, generated** — `CLDR_VERSION`, `COUNTRIES` | T8 |
| `crates/zendriver-stealth/Cargo.toml` | `geo` feature | T6 |
| `crates/zendriver-stealth/src/lib.rs` | module decls + re-exports | T1, T6 |
| `crates/locale-gen/` | **new** non-published generator bin | T7, T8 |
| `crates/zendriver/src/browser.rs` | `BrowserBuilder::geo_locale` | T11 |
| `crates/zendriver/Cargo.toml` | `geo` feature | T11 |
| `crates/zendriver-fingerprints/src/generative/mod.rs` | `Generator::generate_geo` | T12 |
| `crates/zendriver-fingerprints/Cargo.toml` | `geo` feature | T12 |
| `crates/zendriver-mcp/src/...` | `geo_country` override + apply | T14 |
| `.github/workflows/sync-cldr-locale.yml` | **new** weekly sync + guard | T16 |
| `Cargo.toml` (workspace) | add `locale-gen` member | T7 |
| baseline + ledger | public-api regen + coverage | T15 |

**Phases:** T1–T5 always-on primitives (ship coherent multi-language Accept-Language even without geo). T6–T10 the `geo` data layer. T11–T15 consumers + coverage. T16 CI automation.

---

## Task 1: `accept_language()` q-weight formatter

**Files:**
- Create: `crates/zendriver-stealth/src/lang.rs`
- Modify: `crates/zendriver-stealth/src/lib.rs:45` (add `pub mod lang;` after `pub mod input_profile;`) and add re-export

- [ ] **Step 1: Write the failing test**

Create `crates/zendriver-stealth/src/lang.rs`:

```rust
//! Locale list -> header/JS derivations. Always available (no feature gate);
//! the observer formats `Accept-Language` from these.

/// Format an ordered language list as an `Accept-Language` header value.
///
/// Index 0 carries implicit `q=1.0`; each later entry gets
/// `q = (1.0 - 0.1*i)` clamped to `>= 0.1`, one decimal. Empty/duplicate
/// entries are dropped (order preserved). Empty input -> `""`.
pub fn accept_language(langs: &[String]) -> String {
    let mut seen = std::collections::HashSet::new();
    langs
        .iter()
        .filter(|l| !l.is_empty() && seen.insert(l.as_str()))
        .enumerate()
        .map(|(i, l)| {
            if i == 0 {
                l.clone()
            } else {
                let q = (1.0 - 0.1 * i as f64).max(0.1);
                format!("{l};q={q:.1}")
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn formats_q_weights() {
        assert_eq!(accept_language(&v(&["en-US", "en"])), "en-US,en;q=0.9");
        assert_eq!(
            accept_language(&v(&["de-DE", "de", "en"])),
            "de-DE,de;q=0.9,en;q=0.8"
        );
    }

    #[test]
    fn single_and_empty() {
        assert_eq!(accept_language(&v(&["fr-FR"])), "fr-FR");
        assert_eq!(accept_language(&[]), "");
    }

    #[test]
    fn dedups_and_floors_q() {
        assert_eq!(accept_language(&v(&["en", "en", "fr"])), "en,fr;q=0.9");
        let many: Vec<String> = (0..12).map(|i| format!("l{i}")).collect();
        let out = accept_language(&many);
        assert!(out.ends_with("l11;q=0.1"), "q must floor at 0.1: {out}");
    }
}
```

Add to `crates/zendriver-stealth/src/lib.rs` after line 40 (`pub mod input_profile;`):

```rust
pub mod lang;
```

and after the re-exports block (line 57):

```rust
pub use lang::accept_language;
```

- [ ] **Step 2: Run test to verify it passes (impl is co-located)**

Run: `cargo test -p zendriver-stealth lang:: --locked`
Expected: 3 tests pass. (The impl ships with the test in one step because it is a pure function — keep them together.)

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver-stealth/src/lang.rs crates/zendriver-stealth/src/lib.rs
git commit -m "feat(stealth): accept_language q-weight formatter"
```

---

## Task 2: `Persona.languages` field

**Files:**
- Modify: `crates/zendriver-stealth/src/persona/mod.rs:27` (field), `:99` (overlay), `:62-65` (builder)

- [ ] **Step 1: Write the failing test**

Append to the `#[cfg(test)] mod tests` in `crates/zendriver-stealth/src/persona/mod.rs`:

```rust
#[test]
fn languages_overlay_and_builder() {
    let base = Persona::builder()
        .languages(["en-US".to_string(), "en".to_string()])
        .build();
    assert_eq!(
        base.languages.as_deref(),
        Some(["en-US".to_string(), "en".to_string()].as_slice())
    );
    // `Some` in the overlay wins; `None` inherits.
    let over = Persona {
        languages: Some(vec!["de-DE".into(), "de".into()]),
        ..Default::default()
    };
    let merged = base.clone().overlay(over);
    assert_eq!(merged.languages.unwrap(), vec!["de-DE", "de"]);
    let merged2 = base.overlay(Persona::default());
    assert_eq!(merged2.languages.unwrap(), vec!["en-US", "en"]);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zendriver-stealth persona::tests::languages_overlay_and_builder --locked`
Expected: FAIL — `no field 'languages'` / `no method 'languages'`.

- [ ] **Step 3: Add the field, overlay arm, and builder setter**

In `crates/zendriver-stealth/src/persona/mod.rs`, add to the `Persona` struct immediately after `pub locale: Option<String>,` (line 27):

```rust
    /// Ordered language list (e.g. `["de-DE", "de"]`). Drives
    /// `navigator.languages` and the q-weighted `Accept-Language`. When unset,
    /// derived from [`locale`](Self::locale).
    pub languages: Option<Vec<String>>,
```

In `overlay()`, add after the `locale:` line (line 99):

```rust
            languages: over.languages.or(self.languages),
```

In `impl PersonaBuilder`, add after the `locale` setter (line 65):

```rust
    pub fn languages(mut self, langs: impl IntoIterator<Item = String>) -> Self {
        self.0.languages = Some(langs.into_iter().collect());
        self
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zendriver-stealth persona:: --locked`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zendriver-stealth/src/persona/mod.rs
git commit -m "feat(stealth): add Persona.languages field"
```

---

## Task 3: `Fingerprint.languages` + `StealthProfile` plumbing

**Files:**
- Modify: `crates/zendriver-stealth/src/fingerprint.rs:103` (field), `:130-132` + `:122-132` (auto_detect init)
- Modify: `crates/zendriver-stealth/src/profile.rs` (`per_field.languages`, `.languages()` setter, resolve copy at `:325`)

- [ ] **Step 1: Write the failing test**

Append to the test module in `crates/zendriver-stealth/src/profile.rs`:

```rust
#[test]
fn profile_languages_resolve_into_fingerprint() {
    let profile = StealthProfile::native().languages(["fr-FR".into(), "fr".into()]);
    let fp = profile
        .resolve(std::path::Path::new("/nonexistent-chrome"))
        .expect("resolve ok");
    assert_eq!(fp.languages.unwrap(), vec!["fr-FR", "fr"]);
}
```

> If `resolve()` has a different name/signature, grep `fn resolve` in `profile.rs` and match it; the test only needs the resolved `Fingerprint`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zendriver-stealth profile_languages_resolve_into_fingerprint --locked`
Expected: FAIL — `no field 'languages'` / `no method 'languages'`.

- [ ] **Step 3: Implement**

In `crates/zendriver-stealth/src/fingerprint.rs`, add to the `Fingerprint` struct after `pub locale: Option<String>,` (line 103):

```rust
    pub languages: Option<Vec<String>>,
```

In `auto_detect()`'s returned struct literal, after `locale: None,` (line 131):

```rust
            languages: None,
```

In `crates/zendriver-stealth/src/profile.rs`, add a `languages` field to the per-field override struct (find the struct holding `locale: Option<String>`; add a sibling):

```rust
    languages: Option<Vec<String>>,
```

Add a builder setter next to `locale()` (after line 221):

```rust
    /// Override the reported language list (drives `navigator.languages` +
    /// q-weighted `Accept-Language`). When unset, derived from
    /// [`locale`](Self::locale).
    #[must_use]
    pub fn languages(mut self, langs: impl IntoIterator<Item = String>) -> Self {
        self.per_field.languages = Some(langs.into_iter().collect());
        self
    }
```

In `resolve()`, copy it after the `locale` copy (line 327):

```rust
        if let Some(ref langs) = self.per_field.languages {
            fp.languages = Some(langs.clone());
        }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zendriver-stealth profile:: --locked`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zendriver-stealth/src/fingerprint.rs crates/zendriver-stealth/src/profile.rs
git commit -m "feat(stealth): plumb languages through Fingerprint + StealthProfile"
```

---

## Task 4: `resolve_languages()` precedence helper

**Files:**
- Modify: `crates/zendriver-stealth/src/lang.rs`

- [ ] **Step 1: Write the failing test**

Add to `crates/zendriver-stealth/src/lang.rs` (above `#[cfg(test)]`):

```rust
use crate::{Fingerprint, Persona};

/// Resolve the effective language list at apply time.
///
/// Precedence: `persona.languages` -> `fingerprint.languages` ->
/// derive from the primary locale -> `["en-US", "en"]`. Deriving from a
/// region locale yields `[locale, base_lang]` where `base_lang` is the
/// subtag before `-` (e.g. `"fr-FR"` -> `["fr-FR", "fr"]`); a bare locale
/// (no `-`) yields a single entry.
pub(crate) fn resolve_languages(persona: &Persona, fp: &Fingerprint) -> Vec<String> {
    if let Some(langs) = persona.languages.as_ref().filter(|v| !v.is_empty()) {
        return langs.clone();
    }
    if let Some(langs) = fp.languages.as_ref().filter(|v| !v.is_empty()) {
        return langs.clone();
    }
    let locale = persona.locale.clone().or_else(|| fp.locale.clone());
    match locale {
        Some(loc) => {
            let base = loc.split('-').next().unwrap_or(&loc).to_string();
            if base != loc {
                vec![loc, base]
            } else {
                vec![loc]
            }
        }
        None => vec!["en-US".to_string(), "en".to_string()],
    }
}
```

Add tests inside `mod tests`:

```rust
    #[test]
    fn resolve_precedence() {
        use crate::Fingerprint;
        let fp = Fingerprint::auto_detect(std::path::Path::new("/nonexistent"))
            .unwrap_or_else(|_| panic!("auto_detect should fall back"));
        // persona.languages wins
        let p = Persona {
            languages: Some(vec!["es-ES".into(), "es".into()]),
            ..Default::default()
        };
        assert_eq!(resolve_languages(&p, &fp), vec!["es-ES", "es"]);
        // derive from locale: the fr-FR regression (was hardcoded "en")
        let p = Persona {
            locale: Some("fr-FR".into()),
            ..Default::default()
        };
        assert_eq!(resolve_languages(&p, &fp), vec!["fr-FR", "fr"]);
        // bare locale -> single entry
        let p = Persona {
            locale: Some("en".into()),
            ..Default::default()
        };
        assert_eq!(resolve_languages(&p, &fp), vec!["en"]);
        // nothing set -> default
        let p = Persona::default();
        assert_eq!(resolve_languages(&p, &fp), vec!["en-US", "en"]);
    }
```

> If `Fingerprint::auto_detect` errors without a real Chrome, build a `Fingerprint` literal instead (all fields are `pub`); the test only needs `locale: None, languages: None`.

- [ ] **Step 2: Run test to verify it passes**

Run: `cargo test -p zendriver-stealth lang::tests::resolve_precedence --locked`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver-stealth/src/lang.rs
git commit -m "feat(stealth): resolve_languages precedence helper (fixes hardcoded-en bug)"
```

---

## Task 5: Wire the apply path

**Files:**
- Modify: `crates/zendriver-stealth/src/patches.rs:125-128` (navigator.languages)
- Modify: `crates/zendriver-stealth/src/observer.rs:91-94` (acceptLanguage)

- [ ] **Step 1: Write the failing test**

Append to the test module in `crates/zendriver-stealth/src/patches.rs` (the bootstrap builder takes `(&Persona, &Fingerprint)` — match the existing `bootstrap_script` test helpers):

```rust
#[test]
fn navigator_languages_derives_base_lang_not_en() {
    let persona = Persona {
        locale: Some("fr-FR".into()),
        ..Default::default()
    };
    let fp = test_fingerprint(); // existing helper; else build a Fingerprint literal
    let script = bootstrap_script(&persona, &fp);
    assert!(
        script.contains(r#"["fr-FR","fr"]"#) || script.contains(r#"["fr-FR", "fr"]"#),
        "languages should derive base lang, not hardcode en: {script}"
    );
    assert!(!script.contains(r#"["fr-FR","en"]"#));
}
```

> If no `test_fingerprint()` helper exists, construct `Fingerprint { locale: None, languages: None, .. }` with the other fields from `Fingerprint::auto_detect` or literals.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zendriver-stealth navigator_languages_derives_base_lang_not_en --locked`
Expected: FAIL — script still contains the hardcoded `"en"`.

- [ ] **Step 3: Implement — patches.rs**

In `crates/zendriver-stealth/src/patches.rs`, replace the `let locale = ...` + `"languages": ...` block (lines 117 and 125-128) with:

```rust
    let languages = crate::lang::resolve_languages(persona, identity);
```

and in the `json!` literal replace the `"languages": ...` entry (lines 125-128) with:

```rust
        "languages":       languages,
```

(Delete the now-unused `let locale = persona.locale.clone().or_else(...)` binding if nothing else uses it; if `locale` is still referenced below, keep it.)

- [ ] **Step 4: Implement — observer.rs**

In `crates/zendriver-stealth/src/observer.rs`, the `Emulation.setUserAgentOverride` call (lines 86-99). Before the `session.call(...)`, compute the header. The observer has `self.fingerprint`; it does not hold the `Persona` directly, so derive from an empty persona overlaid on the fingerprint's own values:

```rust
        let accept_language = {
            let langs = crate::lang::resolve_languages(&Persona::default(), &self.fingerprint);
            crate::lang::accept_language(&langs)
        };
```

Then change the `"acceptLanguage"` value (lines 91-94) to:

```rust
                    "acceptLanguage": accept_language,
```

> `resolve_languages(&Persona::default(), &self.fingerprint)` falls through to `fingerprint.languages` then `fingerprint.locale` — which is exactly what the observer should send, because `StealthProfile::resolve` already folded the persona's languages/locale into the `Fingerprint`. Import `Persona` at the top of `observer.rs` if not already in scope.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p zendriver-stealth --locked`
Expected: PASS (new test + existing observer/patches tests). If an existing snapshot test asserts the old `acceptLanguage`/`languages`, update it: `cargo insta accept --all` after reviewing the diff.

- [ ] **Step 6: Commit**

```bash
git add crates/zendriver-stealth/src/patches.rs crates/zendriver-stealth/src/observer.rs
git commit -m "feat(stealth): apply resolved languages to navigator.languages + Accept-Language"
```

---

## Task 6: `geo` feature + `Country` type

**Files:**
- Modify: `crates/zendriver-stealth/Cargo.toml` (add `[features]`)
- Modify: `crates/zendriver-stealth/src/lib.rs` (cfg module + re-export)
- Create: `crates/zendriver-stealth/src/geo/mod.rs`

- [ ] **Step 1: Add the feature**

In `crates/zendriver-stealth/Cargo.toml`, add after the `[lints]` block (line 21):

```toml
[features]
default = []
# Country -> locale derivation from a vendored CLDR table.
geo = []
```

- [ ] **Step 2: Write the failing test**

Create `crates/zendriver-stealth/src/geo/mod.rs`:

```rust
//! Country -> locale/languages derivation from a vendored Unicode CLDR table.
//!
//! Opt-in (`--features geo`). The single user-facing entry point is
//! [`persona`], which returns a [`Persona`](crate::Persona) overlay carrying a
//! coherent `locale` + `languages` for the given [`Country`]. Everything stays
//! overridable; an unknown country leaves the persona untouched.

use std::fmt;

/// An ISO 3166-1 alpha-2 country code (uppercase, validated).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Country([u8; 2]);

/// Error returned when a string is not a 2-letter ASCII country code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidCountry(pub String);

impl fmt::Display for InvalidCountry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid country code: {:?} (want 2 ASCII letters)", self.0)
    }
}
impl std::error::Error for InvalidCountry {}

impl Country {
    /// The uppercased 2-letter code as a string slice.
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0).unwrap_or("")
    }
}

impl TryFrom<&str> for Country {
    type Error = InvalidCountry;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        let b = s.as_bytes();
        if b.len() == 2 && b.iter().all(u8::is_ascii_alphabetic) {
            Ok(Country([b[0].to_ascii_uppercase(), b[1].to_ascii_uppercase()]))
        } else {
            Err(InvalidCountry(s.to_string()))
        }
    }
}

impl std::str::FromStr for Country {
    type Err = InvalidCountry;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Country::try_from(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn country_parse() {
        assert_eq!(Country::try_from("us").unwrap().as_str(), "US");
        assert_eq!(Country::try_from("DE").unwrap().as_str(), "DE");
        assert!(Country::try_from("USA").is_err());
        assert!(Country::try_from("u").is_err());
        assert!(Country::try_from("u1").is_err());
        assert!("ch".parse::<Country>().is_ok());
    }
}
```

In `crates/zendriver-stealth/src/lib.rs`, add after `pub mod lang;`:

```rust
#[cfg(feature = "geo")]
pub mod geo;
```

- [ ] **Step 3: Run test to verify it passes**

Run: `cargo test -p zendriver-stealth --features geo geo::tests::country_parse --locked`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/zendriver-stealth/Cargo.toml crates/zendriver-stealth/src/lib.rs crates/zendriver-stealth/src/geo/mod.rs
git commit -m "feat(stealth): geo feature + Country type"
```

---

## Task 7: `locale-gen` generator (pure transform + emitter)

**Files:**
- Create: `crates/locale-gen/Cargo.toml`, `crates/locale-gen/src/lib.rs`, `crates/locale-gen/src/main.rs`, `crates/locale-gen/tests/fixtures/territoryInfo-mini.json`
- Modify: workspace root `Cargo.toml` (`members`)

- [ ] **Step 1: Scaffold the crate**

Create `crates/locale-gen/Cargo.toml`:

```toml
[package]
name = "locale-gen"
version = "0.0.0"
edition.workspace = true
publish = false

[lints]
workspace = true

[dependencies]
reqwest = { workspace = true, features = ["blocking", "json"] }
serde_json.workspace = true

[dev-dependencies]
# none
```

Add `"crates/locale-gen"` to the `members` list in the workspace root `Cargo.toml`.

Create the test fixture `crates/locale-gen/tests/fixtures/territoryInfo-mini.json`:

```json
{
  "supplemental": {
    "territoryInfo": {
      "US": { "languagePopulation": {
        "en": { "_populationPercent": "96", "_officialStatus": "official" },
        "es": { "_populationPercent": "9.6" }
      }},
      "CH": { "languagePopulation": {
        "de": { "_populationPercent": "63", "_officialStatus": "official" },
        "fr": { "_populationPercent": "23", "_officialStatus": "official" },
        "it": { "_populationPercent": "8", "_officialStatus": "official_regional" },
        "en": { "_populationPercent": "45" }
      }},
      "001": { "languagePopulation": { "en": { "_populationPercent": "100" } } }
    }
  }
}
```

- [ ] **Step 2: Write the failing test**

Create `crates/locale-gen/src/lib.rs`:

```rust
//! Offline generator for the vendored CLDR country -> language table.
//! Run via `cargo run -p locale-gen`. NOT published.

use serde_json::Value;

/// One emitted table row: an uppercase country code and its ranked language
/// subtags (dominant first).
#[derive(Debug, PartialEq, Eq)]
pub struct CountryRow {
    pub cc: String,
    pub langs: Vec<String>,
}

/// Keep a language if it is official-ish OR spoken by at least this percent.
const POP_THRESHOLD: f64 = 20.0;

/// Transform CLDR `territoryInfo` JSON into sorted, ranked rows.
///
/// Skips non-country territory keys (e.g. `"001"` world region). Within a
/// country, keeps official languages and any language >= [`POP_THRESHOLD`],
/// ranks official-before-unofficial then by population desc, and reduces each
/// to its primary subtag (`zh_Hant` -> `zh`).
pub fn rows_from_cldr(v: &Value) -> Vec<CountryRow> {
    let mut rows = Vec::new();
    let Some(map) = v["supplemental"]["territoryInfo"].as_object() else {
        return rows;
    };
    for (cc, info) in map {
        if cc.len() != 2 || !cc.bytes().all(|b| b.is_ascii_alphabetic()) {
            continue; // skip "001" etc.
        }
        let Some(lp) = info.get("languagePopulation").and_then(Value::as_object) else {
            continue;
        };
        let mut langs: Vec<(String, f64, bool)> = lp
            .iter()
            .filter_map(|(lang, d)| {
                let pop = d
                    .get("_populationPercent")
                    .and_then(Value::as_str)
                    .and_then(|s| s.parse::<f64>().ok())
                    .unwrap_or(0.0);
                let official = d
                    .get("_officialStatus")
                    .and_then(Value::as_str)
                    .is_some_and(|s| s.starts_with("official") || s == "de_facto_official");
                if official || pop >= POP_THRESHOLD {
                    let subtag = lang.split('_').next().unwrap_or(lang).to_lowercase();
                    Some((subtag, pop, official))
                } else {
                    None
                }
            })
            .collect();
        // official first, then population desc
        langs.sort_by(|a, b| {
            b.2.cmp(&a.2)
                .then(b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal))
        });
        let mut seen = std::collections::HashSet::new();
        let ranked: Vec<String> = langs
            .into_iter()
            .map(|(s, _, _)| s)
            .filter(|s| seen.insert(s.clone()))
            .collect();
        if !ranked.is_empty() {
            rows.push(CountryRow {
                cc: cc.to_uppercase(),
                langs: ranked,
            });
        }
    }
    rows.sort_by(|a, b| a.cc.cmp(&b.cc));
    rows
}

/// Emit the `table.rs` source for the stealth crate.
pub fn emit_rust(version: &str, rows: &[CountryRow]) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "// CLDR {version}. Generated by `cargo run -p locale-gen`. DO NOT EDIT.\n"
    ));
    s.push_str(&format!(
        "pub(crate) static CLDR_VERSION: &str = \"{version}\";\n\n"
    ));
    s.push_str("/// Sorted by country code (binary-searchable). Ranked language subtags, dominant first.\n");
    s.push_str("pub(crate) static COUNTRIES: &[(&str, &[&str])] = &[\n");
    for r in rows {
        let langs = r
            .langs
            .iter()
            .map(|l| format!("\"{l}\""))
            .collect::<Vec<_>>()
            .join(", ");
        s.push_str(&format!("    (\"{}\", &[{}]),\n", r.cc, langs));
    }
    s.push_str("];\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Value {
        serde_json::from_str(include_str!("../tests/fixtures/territoryInfo-mini.json")).unwrap()
    }

    #[test]
    fn rows_skip_world_region_and_rank() {
        let rows = rows_from_cldr(&fixture());
        let ccs: Vec<&str> = rows.iter().map(|r| r.cc.as_str()).collect();
        assert_eq!(ccs, vec!["CH", "US"]); // sorted, "001" skipped
        let us = rows.iter().find(|r| r.cc == "US").unwrap();
        assert_eq!(us.langs, vec!["en"]); // es below threshold (9.6 < 20, not official)
        let ch = rows.iter().find(|r| r.cc == "CH").unwrap();
        assert_eq!(ch.langs[0], "de"); // dominant official first
        assert!(ch.langs.contains(&"fr".to_string()));
        assert!(ch.langs.contains(&"it".to_string()));
    }

    #[test]
    fn emit_is_valid_rust_shape() {
        let rows = rows_from_cldr(&fixture());
        let out = emit_rust("46.0.0", &rows);
        assert!(out.contains("CLDR_VERSION: &str = \"46.0.0\""));
        assert!(out.contains("(\"US\", &[\"en\"]),"));
        assert!(out.contains("(\"CH\", &["));
    }
}
```

- [ ] **Step 3: Run test to verify it passes**

Run: `cargo test -p locale-gen --locked`
Expected: 2 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/locale-gen Cargo.toml
git commit -m "feat(locale-gen): CLDR territoryInfo -> table transform + emitter"
```

---

## Task 8: Generate + vendor the real table; wire `languages_for`

**Files:**
- Create: `crates/locale-gen/src/main.rs`
- Create (generated): `crates/zendriver-stealth/src/geo/table.rs`
- Modify: `crates/zendriver-stealth/src/geo/mod.rs` (`mod table;` + `languages_for`)

- [ ] **Step 1: Write the fetch-and-write main**

Create `crates/locale-gen/src/main.rs`:

```rust
//! Fetch a pinned CLDR release and regenerate the stealth crate's locale table.

use locale_gen::{emit_rust, rows_from_cldr};

/// Pinned CLDR release tag. Bump to resync; the sync workflow does this.
const CLDR_TAG: &str = "46.0.0";
const OUT: &str = "crates/zendriver-stealth/src/geo/table.rs";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = format!(
        "https://raw.githubusercontent.com/unicode-org/cldr-json/{CLDR_TAG}/cldr-json/cldr-core/supplemental/territoryInfo.json"
    );
    eprintln!("fetching {url}");
    let v: serde_json::Value = reqwest::blocking::get(&url)?.error_for_status()?.json()?;
    let rows = rows_from_cldr(&v);
    eprintln!("emitting {} countries to {OUT}", rows.len());
    std::fs::write(OUT, emit_rust(CLDR_TAG, &rows))?;
    Ok(())
}
```

- [ ] **Step 2: Generate the table for real**

Run from the workspace root: `cargo run -p locale-gen`
Expected: writes `crates/zendriver-stealth/src/geo/table.rs` with ~200+ country rows. Then format it: `cargo fmt -p zendriver-stealth`.

> Offline fallback: if the fetch fails, hand-author `crates/zendriver-stealth/src/geo/table.rs` with the header + a representative subset (at minimum `US,DE,FR,CH,GB,JP,BR,IN` with correct dominant subtags) so the build and Task 9/10 tests pass; the workflow (Task 16) fills the full table later.

- [ ] **Step 3: Wire `languages_for` into geo/mod.rs**

In `crates/zendriver-stealth/src/geo/mod.rs`, add near the top (after the doc comment):

```rust
mod table;

pub(crate) use table::CLDR_VERSION;

/// Ranked language subtags for a country, or `None` if absent from the table.
pub(crate) fn languages_for(cc: &str) -> Option<&'static [&'static str]> {
    table::COUNTRIES
        .binary_search_by(|(c, _)| (*c).cmp(cc))
        .ok()
        .map(|i| table::COUNTRIES[i].1)
}
```

- [ ] **Step 4: Verify it compiles + lookups work**

Add to `geo/mod.rs` tests:

```rust
    #[test]
    fn languages_for_known_and_unknown() {
        assert_eq!(languages_for("US"), Some(["en"].as_slice()));
        assert!(languages_for("ZZ").is_none());
    }
```

Run: `cargo test -p zendriver-stealth --features geo geo:: --locked`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/locale-gen/src/main.rs crates/zendriver-stealth/src/geo/table.rs crates/zendriver-stealth/src/geo/mod.rs
git commit -m "feat(stealth): vendor generated CLDR locale table + languages_for lookup"
```

---

## Task 9: `geo::persona()` derivation

**Files:**
- Modify: `crates/zendriver-stealth/src/geo/mod.rs`

- [ ] **Step 1: Write the failing test**

Add to `geo/mod.rs` tests:

```rust
    #[test]
    fn persona_derives_locale_and_languages() {
        let p = persona(Country::try_from("US").unwrap());
        assert_eq!(p.locale.as_deref(), Some("en-US"));
        assert_eq!(p.languages.unwrap(), vec!["en-US", "en"]);

        let p = persona(Country::try_from("DE").unwrap());
        assert_eq!(p.locale.as_deref(), Some("de-DE"));

        // multilingual country -> single dominant language by default
        let ch = persona(Country::try_from("CH").unwrap());
        assert_eq!(ch.languages.unwrap().len(), 2); // [de-CH, de]
        assert_eq!(ch.locale.as_deref(), Some("de-CH"));

        // unknown country -> empty overlay, no panic
        let empty = persona(Country::try_from("ZZ").unwrap());
        assert!(empty.locale.is_none() && empty.languages.is_none());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zendriver-stealth --features geo persona_derives_locale_and_languages --locked`
Expected: FAIL — `cannot find function 'persona'`.

- [ ] **Step 3: Implement**

Add to `geo/mod.rs` (after `languages_for`):

```rust
use crate::Persona;

/// Build a [`Persona`] overlay carrying a coherent `locale` + `languages` for
/// `country`. Default policy: the country's dominant language only, formed as
/// `lang-COUNTRY` with its base subtag (e.g. `CH` -> `de-CH` + `["de-CH","de"]`).
/// An unknown country yields an empty overlay and a warning — never locks a value.
pub fn persona(country: Country) -> Persona {
    let cc = country.as_str();
    match languages_for(cc) {
        Some([primary, ..]) => {
            let locale = format!("{primary}-{cc}");
            Persona {
                locale: Some(locale.clone()),
                languages: Some(vec![locale, (*primary).to_string()]),
                ..Default::default()
            }
        }
        _ => {
            tracing::warn!("geo: no locale mapping for country {cc}; leaving locale unset");
            Persona::default()
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zendriver-stealth --features geo geo:: --locked`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zendriver-stealth/src/geo/mod.rs
git commit -m "feat(stealth): geo::persona country -> locale derivation"
```

---

## Task 10: CLDR table sanity guard test

**Files:**
- Modify: `crates/zendriver-stealth/src/geo/mod.rs`

- [ ] **Step 1: Write the test**

Add to `geo/mod.rs` tests:

```rust
    #[test]
    fn cldr_table_invariants() {
        let rows = table::COUNTRIES;
        assert!(
            (100..=1000).contains(&rows.len()),
            "row count {} out of [100,1000]",
            rows.len()
        );
        // sorted + unique (binary_search correctness)
        assert!(
            rows.windows(2).all(|w| w[0].0 < w[1].0),
            "COUNTRIES must be strictly sorted by code"
        );
        for (cc, langs) in rows {
            assert_eq!(cc.len(), 2, "{cc} not 2 chars");
            assert!(cc.bytes().all(|b| b.is_ascii_uppercase()), "{cc} not uppercase");
            assert!(!langs.is_empty(), "{cc} has no languages");
            for l in *langs {
                assert!(
                    l.len() >= 2 && l.bytes().all(|b| b.is_ascii_lowercase()),
                    "{cc}: bad lang subtag {l:?}"
                );
            }
            // every country forms a well-formed BCP-47 primary locale
            let _ = format!("{}-{}", langs[0], cc);
        }
        assert!(!CLDR_VERSION.is_empty());
    }
```

> The ">20% drop vs committed" delta check is enforced workflow-side (Task 16), not here — a pure unit test has no access to the previous table.

- [ ] **Step 2: Run test**

Run: `cargo test -p zendriver-stealth --features geo cldr_table_invariants --locked`
Expected: PASS (assuming Task 8 produced a full table; if you used the offline subset, the `>=100` bound fails — regenerate the real table before this task).

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver-stealth/src/geo/mod.rs
git commit -m "test(stealth): CLDR table sanity-guard invariants"
```

---

## Task 11: `BrowserBuilder::geo_locale`

**Files:**
- Modify: `crates/zendriver/Cargo.toml` (feature), `crates/zendriver/src/browser.rs` (method + test)

- [ ] **Step 1: Add the feature**

In `crates/zendriver/Cargo.toml` `[features]`, add:

```toml
# Country -> coherent locale/languages (BrowserBuilder::geo_locale).
geo = ["zendriver-stealth/geo"]
```

- [ ] **Step 2: Write the failing test**

Add to the test module in `crates/zendriver/src/browser.rs` (guard with the feature):

```rust
    #[cfg(feature = "geo")]
    #[test]
    fn geo_locale_sets_overlay() {
        let builder = crate::Browser::builder().geo_locale("US");
        let overlay = builder.persona_overlay_for_test();
        assert_eq!(overlay.and_then(|p| p.locale), Some("en-US".to_string()));
    }
```

> If the builder field `persona_overlay` is private and there is no test accessor, add a `#[cfg(test)] pub(crate) fn persona_overlay_for_test(&self) -> Option<Persona> { self.persona_overlay.clone() }`, or assert via the existing `resolved_persona()` path used by sibling persona tests — match whatever pattern the existing persona tests in this file use.

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p zendriver --features geo geo_locale_sets_overlay --locked`
Expected: FAIL — `no method 'geo_locale'`.

- [ ] **Step 4: Implement**

In `crates/zendriver/src/browser.rs`, add a builder method next to the existing `persona`/`persona_overlay` setters:

```rust
    /// Set a coherent `locale` + `languages` derived from a country code
    /// (ISO 3166-1 alpha-2, e.g. `"US"`, `"de"`). Layered as a persona overlay,
    /// so it composes with `.persona(..)` and is overridden by an explicit
    /// `.persona_overlay(..)` locale. An invalid/unknown country is ignored
    /// (logged) — never locks a value.
    #[cfg(feature = "geo")]
    #[must_use]
    pub fn geo_locale(
        mut self,
        country: impl TryInto<zendriver_stealth::geo::Country>,
    ) -> Self {
        match country.try_into() {
            Ok(c) => {
                let derived = zendriver_stealth::geo::persona(c);
                self.persona_overlay = Some(match self.persona_overlay.take() {
                    Some(existing) => existing.overlay(derived),
                    None => derived,
                });
            }
            Err(_) => tracing::warn!("geo_locale: invalid country code; ignoring"),
        }
        self
    }
```

> The `impl TryInto<Country>` bound accepts `&str` (via `Country: TryFrom<&str>`) and `Country` itself (identity `TryInto`). Confirm the `persona_overlay` field name by grepping `persona_overlay` in `browser.rs`; the design audit found it at the `BrowserBuilder` struct definition.

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p zendriver --features geo --locked geo_locale`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/zendriver/Cargo.toml crates/zendriver/src/browser.rs
git commit -m "feat(zendriver): BrowserBuilder::geo_locale (geo feature)"
```

---

## Task 12: `Generator::generate_geo`

**Files:**
- Modify: `crates/zendriver-fingerprints/Cargo.toml` (feature), `crates/zendriver-fingerprints/src/generative/mod.rs`

- [ ] **Step 1: Add the feature**

In `crates/zendriver-fingerprints/Cargo.toml` `[features]`, add:

```toml
# Overlay a coherent country locale onto a generated persona.
geo = ["zendriver-stealth/geo"]
```

- [ ] **Step 2: Write the failing test**

Add to the test module in `crates/zendriver-fingerprints/src/generative/mod.rs`:

```rust
    #[cfg(feature = "geo")]
    #[test]
    fn generate_geo_overlays_locale() {
        let gen = Generator::from_network_json(include_str!("fixtures/mini-network.json"))
            .expect("load mini network");
        let country = zendriver_stealth::geo::Country::try_from("DE").unwrap();
        let p = gen.generate_geo(Seed::from_u64(1), country);
        assert_eq!(p.locale.as_deref(), Some("de-DE"));
        assert_eq!(p.languages.unwrap(), vec!["de-DE", "de"]);
    }
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p zendriver-fingerprints --features geo generate_geo_overlays_locale --locked`
Expected: FAIL — `no method 'generate_geo'`.

- [ ] **Step 4: Implement**

In `crates/zendriver-fingerprints/src/generative/mod.rs`, add to `impl Generator` (after `generate`):

```rust
    /// Generate a persona, then overlay a coherent locale/languages for
    /// `country`. The geo overlay wins over the (locale-free) generated base.
    #[cfg(feature = "geo")]
    pub fn generate_geo(&self, seed: Seed, country: zendriver_stealth::geo::Country) -> Persona {
        self.generate(seed)
            .overlay(zendriver_stealth::geo::persona(country))
    }
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p zendriver-fingerprints --features geo --locked generate_geo`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/zendriver-fingerprints/Cargo.toml crates/zendriver-fingerprints/src/generative/mod.rs
git commit -m "feat(fingerprints): Generator::generate_geo locale overlay"
```

---

## Task 13: `GeoResolver` seam (no impl)

**Files:**
- Modify: `crates/zendriver-stealth/src/geo/mod.rs`

- [ ] **Step 1: Add the trait**

Add to `crates/zendriver-stealth/src/geo/mod.rs`:

```rust
/// Resolve a [`Country`] from ambient context (e.g. the exit IP behind a
/// proxy). The auto-resolving implementation (`IpApiResolver` + structured
/// proxy URL + outbound probe) lands in a follow-up PR; this seam exists now so
/// callers and the `BrowserBuilder` API are forward-compatible. Both the
/// caller-supplied path (`geo_locale("US")`) and a future resolver terminate in
/// the same [`persona`] mapping.
#[async_trait::async_trait]
pub trait GeoResolver: Send + Sync {
    /// Resolve the apparent country, or `None` if it cannot be determined.
    async fn country(&self) -> Option<Country>;
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p zendriver-stealth --features geo --locked`
Expected: builds clean.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver-stealth/src/geo/mod.rs
git commit -m "feat(stealth): GeoResolver seam (impl deferred to follow-up PR)"
```

---

## Task 14: MCP `geo_country` override + apply

**Files:**
- Modify: `crates/zendriver-mcp/Cargo.toml` (feature)
- Modify: the `StealthOverrides` struct (grep `struct StealthOverrides` — likely `crates/zendriver-mcp/src/models` or `src/state`)
- Modify: `crates/zendriver-mcp/src/tools/lifecycle.rs:134-160` (`apply_overrides`)

- [ ] **Step 1: Add the feature**

In `crates/zendriver-mcp/Cargo.toml` `[features]`, add (mirror the existing capability-feature pattern):

```toml
geo = ["zendriver/geo", "zendriver-stealth/geo"]
```

Ensure the mcp `all-features` schema/public-api jobs pick it up (they run `--all-features`).

- [ ] **Step 2: Add the override field**

In the `StealthOverrides` struct, next to the existing `locale` field, add:

```rust
    /// Derive a coherent `locale` + `languages` from a country code
    /// (ISO 3166-1 alpha-2, e.g. `"US"`). Overridden by an explicit `locale`.
    #[cfg(feature = "geo")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geo_country: Option<String>,
```

- [ ] **Step 3: Apply it**

In `apply_overrides` (`crates/zendriver-mcp/src/tools/lifecycle.rs`), add before the `profile` is returned (after the `bypass_csp` block, line 158). Apply geo first so an explicit `locale`/`languages` override still wins (later setters overwrite `per_field`):

```rust
    #[cfg(feature = "geo")]
    if let Some(ref cc) = overrides.geo_country {
        if let Ok(country) = zendriver_stealth::geo::Country::try_from(cc.as_str()) {
            let derived = zendriver_stealth::geo::persona(country);
            if let Some(locale) = derived.locale {
                profile = profile.locale(locale);
            }
            if let Some(langs) = derived.languages {
                profile = profile.languages(langs);
            }
        } else {
            tracing::warn!("geo_country {cc:?} is not a valid country code; ignoring");
        }
    }
```

> Move this block *above* the `overrides.locale` block (line 138) if you want an explicit `locale` to win; as written (geo applied last) geo wins. Per spec, an explicit value should win — so place the geo block **before** the `if let Some(ref locale) = overrides.locale` block.

- [ ] **Step 4: Add a test**

Add to the `lifecycle` test module:

```rust
    #[cfg(feature = "geo")]
    #[test]
    fn geo_country_sets_locale_and_languages() {
        let overrides = StealthOverrides {
            geo_country: Some("US".into()),
            ..Default::default()
        };
        let profile = apply_overrides(StealthProfile::native(), &overrides);
        let fp = profile
            .resolve(std::path::Path::new("/nonexistent"))
            .expect("resolve");
        assert_eq!(fp.locale.as_deref(), Some("en-US"));
        assert_eq!(fp.languages.unwrap(), vec!["en-US", "en"]);
    }
```

- [ ] **Step 5: Run tests + regenerate schema snapshots**

Run:
```bash
cargo test -p zendriver-mcp --features geo geo_country_sets_locale_and_languages --locked
cargo test -p zendriver-mcp --test schema_snapshots --all-features --locked
cargo insta accept --all
```
Expected: test passes; schema snapshot for the stealth-override input now includes `geo_country`; accept the snapshot diff.

- [ ] **Step 6: Commit**

```bash
git add crates/zendriver-mcp
git commit -m "feat(mcp): geo_country stealth override + schema snapshots"
```

---

## Task 15: public-api baseline + coverage ledger

**Files:**
- Modify: `crates/zendriver-mcp/public-api-baseline.txt`
- Modify: `crates/zendriver-mcp/mcp-coverage-ledger.toml`

- [ ] **Step 1: Regenerate the baseline**

The PR adds public items to `zendriver` (`BrowserBuilder::geo_locale`). Regenerate:

```bash
cargo +nightly public-api -p zendriver --all-features > crates/zendriver-mcp/public-api-baseline.txt
```

> Requires nightly + `cargo-public-api` v0.52.0 (see project CLAUDE.md).

- [ ] **Step 2: Add ledger entries**

In `crates/zendriver-mcp/mcp-coverage-ledger.toml`, add an entry covering the new builder API and record the deferred resolver as excluded. Match the file's existing entry shape; e.g.:

```toml
["BrowserBuilder::geo_locale"]
covered = "browser_set_stealth_profile"  # via the geo_country override

["geo::GeoResolver"]
excluded = "auto IP-geo resolution (option B) deferred to a follow-up PR; only the seam ships now"
```

> Run the public-api check to see exactly which item paths it expects, then key the ledger entries to those exact strings.

- [ ] **Step 3: Run the coverage check**

Run:
```bash
cargo +nightly test -p zendriver-mcp --features public-api-check --test public_api --locked
```
Expected: PASS (no uncovered new items).

- [ ] **Step 4: Commit**

```bash
git add crates/zendriver-mcp/public-api-baseline.txt crates/zendriver-mcp/mcp-coverage-ledger.toml
git commit -m "chore(mcp): public-api baseline + coverage ledger for geo locale"
```

---

## Task 16: Weekly CLDR sync workflow + delta guard

**Files:**
- Create: `.github/workflows/sync-cldr-locale.yml`

- [ ] **Step 1: Write the workflow**

Create `.github/workflows/sync-cldr-locale.yml`:

```yaml
name: sync-cldr-locale

on:
  schedule:
    - cron: "0 6 * * 1" # Mondays 06:00 UTC
  workflow_dispatch: {}

permissions:
  contents: write
  pull-requests: write

jobs:
  sync:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - uses: dtolnay/rust-toolchain@stable

      - name: Record committed row count
        id: before
        run: |
          before=$(grep -cE '^\s*\("' crates/zendriver-stealth/src/geo/table.rs || echo 0)
          echo "rows=$before" >> "$GITHUB_OUTPUT"

      - name: Regenerate table
        run: |
          cargo run -p locale-gen
          cargo fmt -p zendriver-stealth

      - name: Delta guard (>20% row drop fails)
        run: |
          after=$(grep -cE '^\s*\("' crates/zendriver-stealth/src/geo/table.rs || echo 0)
          before=${{ steps.before.outputs.rows }}
          echo "before=$before after=$after"
          if [ "$after" -lt "$(( before * 80 / 100 ))" ]; then
            echo "::error::row count dropped >20% ($before -> $after); refusing to sync"
            exit 1
          fi

      - name: Guard test
        run: cargo test -p zendriver-stealth --features geo cldr_table_invariants --locked

      - name: Create PR
        uses: peter-evans/create-pull-request@v6
        with:
          commit-message: "chore: sync CLDR locale table"
          title: "chore: sync CLDR locale table"
          body: |
            Automated weekly CLDR sync (`cargo run -p locale-gen`).
            Version + row changes visible in `crates/zendriver-stealth/src/geo/table.rs`.
            Auto-merges once CI + the table guard pass.
          branch: chore/sync-cldr-locale
          delete-branch: true

      - name: Enable auto-merge
        if: steps.cpr.outputs.pull-request-operation == 'created'
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: gh pr merge --auto --squash "${{ steps.cpr.outputs.pull-request-number }}"
```

> Give the `Create PR` step `id: cpr` so the auto-merge step can read its outputs; add `id: cpr` under `name: Create PR`. Auto-merge also requires branch protection with required status checks + "Allow auto-merge" enabled on the repo (a one-time repo setting, noted in the PR description for the maintainer). The delta guard re-derives the committed count from the table file shape (`("XX", &[...])` rows).

- [ ] **Step 2: Validate the workflow locally**

Run: `cargo run -p locale-gen && cargo fmt -p zendriver-stealth && git diff --stat`
Expected: regenerates the table with no unexpected diff (idempotent on the pinned tag). Lint the YAML if `actionlint` is available: `actionlint .github/workflows/sync-cldr-locale.yml`.

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/sync-cldr-locale.yml
git commit -m "ci: weekly CLDR locale table sync with auto-merge + delta guard"
```

---

## Task 17: Full verification gate (pre-PR)

**Files:** none (verification only)

- [ ] **Step 1: Format**

Run: `cargo fmt --all`

- [ ] **Step 2: Clippy (default + all-features) — run in parallel with tests**

Run (per project CLAUDE.md):
```bash
cargo clippy --workspace --all-targets --locked --fix --allow-dirty --allow-staged
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy -p zendriver-mcp --all-features --all-targets -- -D warnings
```
Expected: no warnings. Re-stage any `--fix` changes.

- [ ] **Step 3: Tests across the touched feature matrix**

Run:
```bash
cargo test -p zendriver-stealth --locked
cargo test -p zendriver-stealth --features geo --locked
cargo test -p locale-gen --locked
cargo test -p zendriver --features geo --locked
cargo test -p zendriver-fingerprints --features geo --locked
cargo test -p zendriver-mcp --all-features --locked
cargo test -p zendriver-mcp --test schema_snapshots --all-features --locked
cargo +nightly test -p zendriver-mcp --features public-api-check --test public_api --locked
```
Expected: all pass; no pending snapshots.

- [ ] **Step 4: Confirm fmt + clippy gates exactly as CI runs them**

Run:
```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
```
Expected: clean exit.

- [ ] **Step 5: Final commit (if fixes were applied)**

```bash
git add -A
git commit -m "chore: fmt + clippy for geo locale feature"
```

---

## Self-Review notes (for the executor)

- **Spec coverage:** §1 arch → T1/T6 placement; §2 types → T2/T3; §3 apply path → T1/T4/T5; §4 geo+derivation → T6/T8/T9; §5 generator+workflow+guard → T7/T8/T10/T16; §6 API/generative/MCP/baseline → T11/T12/T14/T15; §GeoResolver seam → T13; §testing → tests in each task + T17. All sections mapped.
- **`likelySubtags` not needed:** the design cited it, but forming `lang-COUNTRY` directly from the country code yields a valid BCP-47 locale, so the generator only consumes `territoryInfo.json`. (Refinement, consistent with the goal.)
- **Type consistency:** `languages: Option<Vec<String>>` on both `Persona` and `Fingerprint`; `Country::try_from(&str)`/`as_str`; `languages_for(&str) -> Option<&'static [&'static str]>`; `geo::persona(Country) -> Persona`; `Generator::generate_geo(Seed, Country)` — names match across tasks.
- **`>20%` drop guard** is workflow-side (T16); the in-crate guard (T10) covers absolute bounds + validity + sortedness.
