# Robustness — CDP Forward-Compat + Popup Preferences Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Lock the port's (already-safe) CDP forward-compat deser with regression tests, bump the stale Chrome fallback const + a floor guard, and add a `Preferences` writer that robustly suppresses Chrome's password-manager/autofill popups (#13, nodriver #33/#34).

**Architecture:** A pure `merge_preferences` (dotted-key → nested-JSON merge) + a best-effort `write_preferences` that writes `<user_data_dir>/Default/Preferences` at launch — defaults auto-written only for a PORT-OWNED temp profile, user `.preference(...)` overrides always applied. Forward-compat is regression-tested (no production change); the Chrome const is bumped with a CI floor guard.

**Tech Stack:** Rust, `serde_json::Value` (pref merge), existing `BrowserBuilder` launch flow.

**Spec:** `docs/superpowers/specs/2026-06-02-robustness-forward-compat-prefs-design.md`

---

## File Structure

- **Create** `crates/zendriver/src/preferences.rs` — `merge_preferences` (pure, unit-tested) + `write_preferences` (IO, best-effort).
- **Modify** `crates/zendriver/src/browser.rs` — `preferences: Vec<(String, serde_json::Value)>` field + `.preference()` method; launch-flow write call (after line 1799, before spawn).
- **Modify** `crates/zendriver/src/lib.rs` — `pub(crate) mod preferences;`.
- **Modify** `crates/zendriver/src/cookies/mod.rs` — forward-compat regression test.
- **Modify** `crates/zendriver-stealth/src/fingerprint.rs` — bump `FALLBACK_CHROME_*` + floor-guard test.
- **Create** `crates/zendriver/tests/preferences_integration.rs` — gated headful tests.
- **Modify** `CHANGELOG.md`.

---

## Task 1: `merge_preferences` (pure, dotted-key merge)

**Files:**
- Create: `crates/zendriver/src/preferences.rs`
- Modify: `crates/zendriver/src/lib.rs` (`pub(crate) mod preferences;`)

- [ ] **Step 1: Write the failing test**

Create `crates/zendriver/src/preferences.rs`:
```rust
//! Chrome profile `Preferences` writer: merges suppression prefs (and user
//! overrides) into `<user_data_dir>/Default/Preferences` at launch. Default
//! suppression is written only for a port-owned (temp) profile; a
//! user-supplied profile is left untouched unless explicit prefs are given.

use serde_json::{Value, json};

/// The default popup-suppression preference set (password manager + autofill).
/// Written for port-owned profiles. Dotted keys expand to nested objects.
pub(crate) fn default_suppression() -> Vec<(String, Value)> {
    vec![
        ("credentials_enable_service".into(), json!(false)),
        ("profile.password_manager_enabled".into(), json!(false)),
        ("profile.password_manager_leak_detection".into(), json!(false)),
        ("autofill.profile_enabled".into(), json!(false)),
        ("autofill.credit_card_enabled".into(), json!(false)),
    ]
}

/// Merge `prefs` (dotted keys, last-wins) into `base` (a JSON object, or `{}`
/// if not an object). Returns the merged object. Existing unrelated keys are
/// preserved.
pub(crate) fn merge_preferences(mut base: Value, prefs: &[(String, Value)]) -> Value {
    if !base.is_object() {
        base = json!({});
    }
    for (key, val) in prefs {
        set_dotted(&mut base, key, val.clone());
    }
    base
}

fn set_dotted(root: &mut Value, dotted: &str, val: Value) {
    let parts: Vec<&str> = dotted.split('.').collect();
    let mut cur = root;
    for part in &parts[..parts.len().saturating_sub(1)] {
        if !cur.is_object() {
            *cur = json!({});
        }
        let Some(obj) = cur.as_object_mut() else { return };
        cur = obj.entry((*part).to_string()).or_insert_with(|| json!({}));
    }
    if !cur.is_object() {
        *cur = json!({});
    }
    if let (Some(obj), Some(last)) = (cur.as_object_mut(), parts.last()) {
        obj.insert((*last).to_string(), val);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotted_key_expands_to_nested() {
        let out = merge_preferences(json!({}), &[("profile.password_manager_enabled".into(), json!(false))]);
        assert_eq!(out["profile"]["password_manager_enabled"], json!(false));
    }

    #[test]
    fn merge_preserves_existing_keys() {
        let base = json!({ "foo": 1, "profile": { "name": "bob" } });
        let out = merge_preferences(base, &[("profile.password_manager_enabled".into(), json!(false))]);
        assert_eq!(out["foo"], json!(1));
        assert_eq!(out["profile"]["name"], json!("bob"));       // sibling preserved
        assert_eq!(out["profile"]["password_manager_enabled"], json!(false));
    }

    #[test]
    fn later_pref_overrides_earlier() {
        let out = merge_preferences(json!({}), &[
            ("credentials_enable_service".into(), json!(false)),
            ("credentials_enable_service".into(), json!(true)),   // user override wins
        ]);
        assert_eq!(out["credentials_enable_service"], json!(true));
    }

    #[test]
    fn non_object_base_becomes_object() {
        let out = merge_preferences(json!("garbage"), &[("a".into(), json!(1))]);
        assert_eq!(out["a"], json!(1));
    }

    #[test]
    fn non_object_intermediate_is_overwritten() {
        let base = json!({ "profile": 5 });   // "profile" is not an object
        let out = merge_preferences(base, &[("profile.x".into(), json!(true))]);
        assert_eq!(out["profile"]["x"], json!(true));
    }
}
```
Add to `lib.rs`: `pub(crate) mod preferences;`.

- [ ] **Step 2: Run + commit**

Run: `cargo test -p zendriver preferences::tests`
Expected: 5 passed.
```bash
git add crates/zendriver/src/preferences.rs crates/zendriver/src/lib.rs
git commit -m "feat(preferences): dotted-key merge for Chrome prefs"
```

---

## Task 2: `write_preferences` (IO, best-effort)

**Files:**
- Modify: `crates/zendriver/src/preferences.rs`

- [ ] **Step 1: Write the failing test**

Add to `preferences.rs`:
```rust
use std::path::Path;

/// Write the resolved prefs into `<user_data_dir>/Default/Preferences`,
/// merging with any existing file. `owned` = the port created this profile
/// (temp dir) → the default suppression set is included; a user-supplied
/// profile (`owned == false`) gets ONLY `user_prefs` (and nothing at all if
/// `user_prefs` is empty). Best-effort: any IO/parse failure is logged and
/// ignored (flags still suppress at the flag level).
pub(crate) fn write_preferences(user_data_dir: &Path, owned: bool, user_prefs: &[(String, Value)]) {
    let mut prefs: Vec<(String, Value)> = Vec::new();
    if owned {
        prefs.extend(default_suppression());
    } else if user_prefs.is_empty() {
        return; // supplied profile + no explicit prefs → don't touch it
    }
    prefs.extend(user_prefs.iter().cloned());

    let default_dir = user_data_dir.join("Default");
    let path = default_dir.join("Preferences");
    let base = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .unwrap_or_else(|| json!({}));
    let merged = merge_preferences(base, &prefs);
    if let Err(e) = std::fs::create_dir_all(&default_dir)
        .and_then(|()| std::fs::write(&path, merged.to_string()))
    {
        tracing::warn!(error = %e, path = %path.display(),
            "failed to write Chrome Preferences; relying on flag-level suppression");
    }
}

#[cfg(test)]
mod io_tests {
    use super::*;

    #[test]
    fn owned_writes_default_suppression() {
        let dir = tempfile::tempdir().unwrap();
        write_preferences(dir.path(), true, &[]);
        let s = std::fs::read_to_string(dir.path().join("Default/Preferences")).unwrap();
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["credentials_enable_service"], json!(false));
        assert_eq!(v["profile"]["password_manager_enabled"], json!(false));
    }

    #[test]
    fn supplied_without_prefs_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        write_preferences(dir.path(), false, &[]);
        assert!(!dir.path().join("Default/Preferences").exists());
    }

    #[test]
    fn supplied_preserves_existing_and_adds_user_pref() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Default")).unwrap();
        std::fs::write(dir.path().join("Default/Preferences"), r#"{"foo":1}"#).unwrap();
        write_preferences(dir.path(), false, &[("x.y".into(), json!(true))]);
        let v: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("Default/Preferences")).unwrap()).unwrap();
        assert_eq!(v["foo"], json!(1));                  // preserved
        assert_eq!(v["x"]["y"], json!(true));            // user pref added
        assert!(v.get("credentials_enable_service").is_none()); // no defaults for supplied
    }
}
```
> `tempfile` is already a dep of `zendriver`. The `#[cfg(test)]` mods may trip `clippy::unwrap_used` — if the crate denies it in tests, add `#![allow(clippy::unwrap_used)]` at the test-mod level, matching the codebase's test convention.

- [ ] **Step 2: Run + commit**

Run: `cargo test -p zendriver preferences::`
Expected: 5 + 3 passed.
```bash
git add crates/zendriver/src/preferences.rs
git commit -m "feat(preferences): write_preferences (owned default-on / supplied opt-in)"
```

---

## Task 3: Builder `.preference()` + launch integration

**Files:**
- Modify: `crates/zendriver/src/browser.rs`

- [ ] **Step 1: Add the field + method**

Add a field near the other `BrowserBuilder` fields (~line 328):
```rust
    /// Chrome profile preferences (dotted key + JSON value), merged into the
    /// profile's `Default/Preferences` at launch. User entries override the
    /// default suppression set. See [`BrowserBuilder::preference`].
    pub(crate) preferences: Vec<(String, serde_json::Value)>,
```
Initialize `preferences: Vec::new()` wherever `BrowserBuilder` is constructed (the `Default`/`new` ctor — find it; there is one near the other field defaults).
Add the method (near `args()`, ~line 769):
```rust
    /// Set a Chrome profile preference, e.g.
    /// `.preference("profile.password_manager_enabled", serde_json::json!(false))`.
    ///
    /// Merged into `<user_data_dir>/Default/Preferences` at launch (dotted keys
    /// expand to nested objects). User preferences override the default
    /// popup-suppression set. For a *user-supplied* `user_data_dir`, ONLY your
    /// explicit preferences are written (the defaults are not, to avoid mutating
    /// a real profile); for a port-created temp profile, defaults + yours are
    /// written.
    #[must_use]
    pub fn preference(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.preferences.push((key.into(), value));
        self
    }
```

- [ ] **Step 2: Wire the write into `launch()`**

In `launch()`, immediately AFTER the `(user_data_path, owned_tmp)` resolution (the `match self.user_data_dir.clone()` block ending ~line 1799) and BEFORE `build_flags`/spawn:
```rust
        // Write Chrome profile preferences (popup suppression for owned temp
        // profiles; explicit user prefs always). Best-effort — see preferences.rs.
        crate::preferences::write_preferences(&user_data_path, owned_tmp.is_some(), &self.preferences);
```
> `owned_tmp.is_some()` is the owned/supplied signal (it's `Some` only when `self.user_data_dir` was `None` → the port made the temp dir). Confirm the variable names `user_data_path` / `owned_tmp` at the real lines (~1790-1799).

- [ ] **Step 3: Build + a builder unit test**
```rust
#[test]
fn preference_accumulates_on_builder() {
    let b = Browser::builder()
        .preference("a.b", serde_json::json!(false))
        .preference("c", serde_json::json!(1));
    assert_eq!(b.preferences.len(), 2);
    assert_eq!(b.preferences[0].0, "a.b");
}
```
Run: `cargo test -p zendriver preference_accumulates_on_builder` + `cargo build -p zendriver`.
Expected: PASS + builds.

- [ ] **Step 4: Commit**
```bash
git add crates/zendriver/src/browser.rs
git commit -m "feat: BrowserBuilder.preference() + launch-time Preferences write"
```

---

## Task 4: Forward-compat regression test + Chrome const bump

**Files:**
- Modify: `crates/zendriver/src/cookies/mod.rs`, `crates/zendriver-stealth/src/fingerprint.rs`

- [ ] **Step 1: Forward-compat cookie test**

In `cookies/mod.rs` tests (next to `cookie_read_missing_new_fields_is_none`, ~line 692):
```rust
#[test]
fn cookie_read_ignores_unknown_future_field() {
    // A future Chrome adds a field rs doesn't model → must be ignored, not panic.
    let cdp: CdpCookie = serde_json::from_value(serde_json::json!({
        "name": "sid", "value": "xyz", "domain": ".example.com", "path": "/",
        "someChrome147Field": { "nested": true }, "anotherNewThing": 42
    })).expect("unknown fields must be ignored, not rejected");
    assert_eq!(cdp.name, "sid");
}
```
> Adapt `CdpCookie`'s real field names if the public test uses `Cookie` instead — match the existing `cookie_read_missing_new_fields_is_none` test's type + access pattern.

- [ ] **Step 2: Bump the Chrome fallback const + floor guard**

In `crates/zendriver-stealth/src/fingerprint.rs` (lines 89-90), determine the CURRENT Chrome stable: run `"$(which google-chrome || which chromium || echo /Applications/Google\ Chrome.app/Contents/MacOS/Google\ Chrome)" --version` (or check the machine's installed Chrome). Set:
```rust
const FALLBACK_CHROME_FULL: &str = "<current full version, e.g. 137.0.7151.x>";
const FALLBACK_CHROME_MAJOR: u32 = <current major>;
```
Use the actual installed Chrome's version (do NOT invent a number). Add a floor-guard test in that file:
```rust
#[test]
fn fallback_chrome_is_not_ancient() {
    // Tripwire: forces a conscious bump when Chrome moves well past this floor.
    // Set the floor a few majors below the current FALLBACK_CHROME_MAJOR.
    assert!(FALLBACK_CHROME_MAJOR >= <floor, e.g. 133>,
        "FALLBACK_CHROME_MAJOR is stale; bump it (and this floor) to current stable");
}
```

- [ ] **Step 3: Run + commit**

Run: `cargo test -p zendriver cookie_read_ignores_unknown_future_field` and `cargo test -p zendriver-stealth fallback_chrome_is_not_ancient`
Expected: PASS.
```bash
git add crates/zendriver/src/cookies/mod.rs crates/zendriver-stealth/src/fingerprint.rs
git commit -m "test: lock CDP forward-compat + bump Chrome fallback const with floor guard"
```

---

## Task 5: Integration tests (gated headful)

**Files:**
- Create: `crates/zendriver/tests/preferences_integration.rs`

- [ ] **Step 1: Write tests** (gate + launch pattern from `integration_phase4.rs`/`integration_phase5.rs`):
```rust
#![cfg(feature = "integration-tests")]
// owned temp profile → suppression prefs land in Default/Preferences
#[tokio::test] #[serial] #[ignore]
async fn owned_profile_gets_suppression_prefs() {
    let browser = Browser::builder().build().await.unwrap();   // adapt to real launch API
    // The temp dir is internal; assert via behavior OR expose the path. Simplest:
    // launch with an EXPLICIT temp dir we create + own-equivalent is not possible
    // (explicit dir = supplied). Instead assert the unit-level write_preferences
    // already covers owned; here verify Chrome STARTS cleanly with prefs written.
    let _tab = browser.new_tab("about:blank").await.unwrap();
    browser.close().await.unwrap();
}
// supplied profile is preserved
#[tokio::test] #[serial] #[ignore]
async fn supplied_profile_preserved() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("Default")).unwrap();
    std::fs::write(dir.path().join("Default/Preferences"), r#"{"foo":1}"#).unwrap();
    let browser = Browser::builder().user_data_dir(dir.path()).build().await.unwrap();
    browser.close().await.unwrap();
    let v: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("Default/Preferences")).unwrap()).unwrap();
    assert_eq!(v["foo"], serde_json::json!(1));   // Chrome may add keys, but foo stays
}
```
> The owned-profile assertion is awkward headful (the temp dir is internal). The owned path is fully covered by the `write_preferences` unit tests (Task 2). Keep the integration tests focused on: (a) Chrome launches cleanly with prefs written, (b) a supplied profile's existing keys survive. Adapt `Browser::builder()...build()/launch()`, `new_tab`, `close` to the real API (see `integration_phase4.rs`).

- [ ] **Step 2: Compile-check**

Run: `cargo test -p zendriver --features integration-tests --test preferences_integration --no-run`
Expected: compiles. Run `-- --ignored` locally with Chrome if available.

- [ ] **Step 3: Commit**
```bash
git add crates/zendriver/tests/preferences_integration.rs
git commit -m "test: Preferences writer integration (owned + supplied profiles)"
```

---

## Task 6: Docs + CHANGELOG + gates + PR

**Files:**
- Modify: `crates/zendriver/src/browser.rs` (doctest), `CHANGELOG.md`

- [ ] **Step 1: Doctest + CHANGELOG**

Add a `no_run` doctest on `.preference()` showing disabling the password manager. CHANGELOG under `[Unreleased] ### Added`:
```markdown
- `BrowserBuilder::preference(key, value)` — set Chrome profile preferences
  (merged into `Default/Preferences` at launch). Port-owned temp profiles get a
  default password-manager/autofill suppression set; user-supplied profiles are
  left untouched unless explicit preferences are given (#13).
```
And under `### Changed` (or a `### Robustness` note):
```markdown
- Bumped the Chrome version fallback used when `chrome --version` probing fails,
  with a CI floor guard against future staleness.
```

- [ ] **Step 2: Gates**
```bash
cargo fmt --all
cargo clippy --workspace --all-targets --locked --fix --allow-dirty --allow-staged
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test -p zendriver --features integration-tests --test preferences_integration --no-run
```
Expected: clean + green.

- [ ] **Step 3: Commit + PR**
```bash
git add -A && git commit -m "docs: BrowserBuilder.preference + robustness CHANGELOG"
gh pr create --base main \
  --title "feat: popup-suppression Preferences + CDP forward-compat hardening (#13, #33/#34)" \
  --body "PR2 Group C. See docs/superpowers/specs/2026-06-02-robustness-forward-compat-prefs-design.md"
```

---

## Self-Review (completed by plan author)

**Spec coverage:** §2 forward-compat tests → T4; §3 Chrome const bump + floor → T4; §4 Preferences writer (merge, owned/supplied behavior, `.preference()`, launch integration) → T1/T2/T3; §5 testing → T1/T2 (unit) + T5 (integration); §6 out-of-scope honored (no auto-version-fetch, no notification prefs, no flag rework). Covered.

**Placeholders:** none — full code per step. The Chrome version value in T4 is intentionally implementer-probed (a hard-coded number would itself go stale) — instruction is concrete (run `chrome --version`, set the floor below it). Adapt-points flagged: real `BrowserBuilder` ctor location, `user_data_path`/`owned_tmp` variable names (~1790-1799), `CdpCookie` vs `Cookie` test type, integration launch API.

**Type consistency:** `merge_preferences`/`set_dotted`/`default_suppression`/`write_preferences`, `preferences: Vec<(String, Value)>`, `.preference()`, `owned_tmp.is_some()` — consistent across tasks and matching the spec.
