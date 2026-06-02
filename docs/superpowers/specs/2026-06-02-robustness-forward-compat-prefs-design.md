# Robustness — CDP Forward-Compat + Popup Preferences — Design (PR2 / Group C)

- **Date:** 2026-06-02
- **Status:** Approved (brainstorming), pending implementation plan
- **Upstream drivers:** nodriver #33/#34 (Chrome-146 cookie deser break), zendriver #13 (disable "Save password" popup)
- **Scope:** Group C of the PR2 batch — the smallest group. Three robustness items, one PR. Group D (datadome) is separate.

---

## 1. Context

Investigation found the port is **already robust** to the headline failures:

- **CDP deserialization forward-compat (#33/#34):** core CDP wire types carry no `#[serde(deny_unknown_fields)]`, so unknown future fields are ignored. The [`Cookie`](../../../crates/zendriver/src/cookies/mod.rs:106)/`CdpCookie` types have required bare `name/value/domain/path` plus every variable field (`same_party`, `partition_key`, `source_scheme`, …) as `Option` + `#[serde(default)]`. An existing test (`cookie_read_missing_new_fields_is_none`, cookies/mod.rs:692) documents this as "the crash-immunity contract that protected rs from the Chrome-146 `sameParty` regression." Transport stores params/result as `serde_json::Value` — drift is absorbed; no `chromiumoxide_cdp` typed deser on hot paths. (MCP DTOs *do* use `deny_unknown_fields`, but that is strict **tool-input** validation, unrelated to Chrome drift.)
- **Popup suppression (#13):** default launch flags already include `--disable-save-password-bubble`, `--password-store=basic`, `--disable-infobars`, `--disable-features=PasswordManagerOnboarding,…`. But it is **flags-only** — no user-data-dir `Preferences` JSON is written, and `--disable-save-password-bubble` is unreliable in recent Chrome.

So Group C is hardening + one net-new capability:
1. Regression tests that **lock** the forward-compat behavior.
2. **Bump** the stale Chrome fallback const + a staleness guard.
3. A **Preferences writer** for robust popup suppression (the only real new code).

The builder already distinguishes owned vs supplied profiles: `user_data_dir: None` → the port creates + owns a `TempDir` (`Browser::_user_data`, RAII-cleaned); `Some(path)` → user-supplied. This existing owns/supplied distinction drives the Preferences-writer behavior.

---

## 2. Forward-compat regression tests

Pure regression safety — no production change. In `cookies/mod.rs` tests:
- `cookie_read_ignores_unknown_future_field` — deserialize a `CdpCookie` JSON containing an invented `"someChrome147Field": 1` → succeeds, field ignored, no panic.
- Keep/extend the existing `cookie_read_missing_new_fields_is_none`.
- (Optional) a small test over one network-monitor / expect event struct asserting an unknown field deserializes fine, documenting the same contract for event payloads.

These fail loudly if anyone later adds `deny_unknown_fields` or a required-bare field to a wire type.

---

## 3. Chrome version freshness

`FALLBACK_CHROME_FULL = "120.0.6099.234"` / `FALLBACK_CHROME_MAJOR = 120` (fingerprint.rs:89-90) is ~10 months stale; used only when the `chrome --version` probe fails. A stale UA on probe-failure is a fingerprint signal.

- **Bump** both constants to a current stable. The implementer sets the value from the actually-installed Chrome (`chrome --version`) or the latest known stable at build time — the spec does not hard-code a version number.
- **Staleness guard:** a unit test `fallback_chrome_is_not_ancient` asserting `FALLBACK_CHROME_MAJOR >= <floor>` (a floor a few majors below the bumped value), forcing a conscious bump when it eventually trips. Cheap CI tripwire.
- Keep the existing `// Bump on each release` comment; no auto-fetch mechanism (network dep + stealth→fetcher coupling; YAGNI — the probe covers the normal path).

---

## 4. Preferences writer (#13)

A new `preferences` capability on the browser builder writing Chrome profile prefs.

### Public API

```rust
Browser::builder()
    // Owned (temp) profile: default suppression prefs auto-written.
    // Add or override ANY pref (dotted key + JSON value):
    .preference("profile.password_manager_enabled", serde_json::json!(false))
    .launch().await?;
```
```rust
impl BrowserBuilder {
    /// Set a Chrome preference (dotted key, e.g. "credentials_enable_service"
    /// or "profile.password_manager_enabled"). Merged into the profile's
    /// `Default/Preferences` at launch; user prefs override the defaults.
    pub fn preference(self, key: impl Into<String>, value: serde_json::Value) -> Self;
}
```

### Behavior (resolved decision: owns/supplied + default-on + override)

- **Port-owned profile** (`user_data_dir` is `None` → temp dir): at launch, after the temp dir is created and **before** Chrome spawns, write the **default suppression set** (below) merged with any user `.preference(...)` calls into `<dir>/Default/Preferences`.
- **User-supplied profile** (`user_data_dir` is `Some`): do **not** auto-write the defaults (non-invasive — don't mutate a real profile). If the user added explicit `.preference(...)` calls, write **only those** (merged into existing `Preferences`) — they asked for it.
- `.preference()` values always win over the defaults (merged last).

### Default suppression set (owned profiles)

```json
{ "credentials_enable_service": false,
  "profile.password_manager_enabled": false,
  "profile.password_manager_leak_detection": false,
  "autofill.profile_enabled": false,
  "autofill.credit_card_enabled": false }
```
Minimal — targets the password-manager + autofill bubbles #13 is about. Notifications/geolocation stay flag/content-setting controlled (out of scope).

### Merge mechanics (`src/preferences.rs`)

- A dotted key expands to a nested object: `profile.password_manager_enabled` → `{"profile":{"password_manager_enabled": false}}`.
- Read `<dir>/Default/Preferences` (parse as `serde_json::Value::Object`, or `{}` if absent/empty/corrupt), set each dotted key into the nested tree (creating intermediate objects), write back (create `<dir>/Default/` if needed). Existing unrelated keys preserved.
- Pure, unit-testable: `fn merge_preferences(base: Value, prefs: &[(String, Value)]) -> Value`.

### Launch integration

In the launch flow (`browser.rs`), once the resolved `user_data_dir` is known:
- owned (temp): write `defaults + user prefs`.
- supplied + user prefs present: write `user prefs` only.
- supplied + no user prefs: skip.
Write `<dir>/Default/Preferences` before spawning Chrome (Chrome reads it at startup). Failure to write is logged + non-fatal (best-effort; the flags still suppress at the flag level) — consistent with the codebase's best-effort observability contract.

---

## 5. Testing

- **Unit (no browser):**
  - `merge_preferences`: dotted-key → nested expansion; merge preserves existing keys; user pref overrides a default; multiple keys under the same parent merge (not clobber).
  - `cookie_read_ignores_unknown_future_field` (§2); `fallback_chrome_is_not_ancient` (§3).
- **Integration (gated headful):**
  - Launch with an owned temp dir → `<dir>/Default/Preferences` contains `credentials_enable_service: false`.
  - Launch with a supplied dir pre-seeded with `{"foo": 1}` and no `.preference()` → file still has `foo: 1`, no suppression keys added (non-invasive).
  - Launch supplied dir + an explicit `.preference("x.y", true)` → `foo` preserved AND `x.y` written.

---

## 6. Out of scope

- Auto-fetching the latest Chrome version (network + coupling; YAGNI).
- Notification/geolocation/content-setting prefs (flag-controlled; add later if asked).
- Reworking existing default flags (already suppress at flag level; prefs are additive).
- Group D (datadome).

---

## 7. Open questions for the plan stage

- Exact current Chrome stable version for the §3 bump (probe at implementation time).
- Confirm the precise point in the `browser.rs` launch sequence where the resolved `user_data_dir` (owned temp or supplied) is known and Chrome has not yet spawned, to slot the Preferences write.
- Whether `.preference()` storage on the builder is `Vec<(String, Value)>` (ordered, last-wins) — yes, to keep override order deterministic.
