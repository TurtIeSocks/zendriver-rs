# PR plan — remaining audit work, re-cut

Supersedes the "Delivery: 8 themed PRs" section of the implementation plan for everything
after PR 1. Decisions those PRs depend on live in [decisions.md](decisions.md).

## Why this exists

Triage recommended thirteen PRs. The plan overrode that to eight against a "~7 ceiling",
on the argument that PR 1 was *"48 mechanical bug fixes with no API surface"* and could lean
on commit granularity instead.

That argument was wrong, and PR 1 disproved it: 48 items in one diff was unreviewable, and it
took **six** PRs to become mergeable — #150 transport, #152 solver terminals, #155 io/cookies,
#156 frame/geo, #157 input, #158 query/visibility. Review cost tracks diff size, not
conceptual difficulty. Mechanical changes are not cheaper to review; there are just more of
them.

Six PRs for 48 items is the only sizing evidence this project has. Applied to the 183 items
left, it lands near **25**, not 13 — thirteen was itself a compromise against the ceiling.

## The rule, which matters more than the count

Cut a PR at **one subsystem with one narrative**, roughly 8–12 items. If a reviewer would have
to hold two unrelated mental models at once, it is two PRs. The six PR-1 splits worked because
each was one subsystem, independently verifiable, and independently revertable — not because
they hit a number.

Where a crate boundary is available, use it: it is the cleanest possible seam and makes the
diff self-evidently scoped.

## The cut

| PR | Scope | Notes |
|---|---|---|
| **2a** | Persona reaches CDP — timezone, locale, screen, Accept-Language folded into `Fingerprint` once at launch/connect | fixes four symptoms in one place |
| **2b** | Launch-path validation — proxy fails closed, `ci_defaults(bool)` replacing the `CI` sniff, drop the `cpu_count`/`memory_gb`/build-digit clamps, `CHROME_BIN` only when `channel == Auto` | |
| **2c** | Download coordination, named downgrades (`CloseOutcome`, `GeoPolicy`, `resolved_surface_strategy`), inert public fields | |
| **3a** | `visible_only` = on-screen, `is_visible` = rendered; both `checkVisibility` option spellings | decisions [#1](decisions.md), [#10](decisions.md); gates 3c and 7b |
| **3b** | `LoadOptions` / `ReadyStateOptions` / `GotoOptions`, `DEFAULT_LOAD_TIMEOUT` hoisted to one place | |
| **3c** | `ActionabilityCheck` public, `ClickOptions::force` → `checks`, `ActionOptions`, `ScrollPolicy` | the loud breaking change |
| **3d** | `IdleOptions` gains `max_inflight` / `ignore` / `poll_interval`; `RequestEvent` carries `request.url`; fix the exhaustive construction at `navigation.rs:331` | |
| **3e** | Query builders — `RoleMatch`, `NameMatch`, `AriaRole::Other`, `FindAllBuilder`, `ClearOptions`, `ScrollOptions::anchor`; element reads through the isolated world | |
| **4a** | `impl Default for InputProfile`, then the nine new axes across all three presets | Default must land first or every later field is breaking |
| **4b** | Route `type_keys` / `press` / `press_with` and both `mouse_drag`s through the profile; `input_seed` | |
| **4c** | `StealthProfile::{focus_emulation, patch_workers, default_languages}`; `Surface::{Plugins, Codecs, BrokenImage}` | |
| **4d** | `ScreenSpec`, `BatterySpec`, `MediaDeviceSpec`, `VoiceSpec`; probe `Browser.getVersion` instead of the pinned Chrome 148 | |
| **5a-1** | `ignore_default_args` / `ignore_all_default_args` / `default_args()` | |
| **5a-2** | `remote_debugging_port` / `_address` + the `parse_devtools_active_port` hardcode, `proxy_bypass`, `profile_directory`, `stray_targets` | |
| **5a-3** | `Timeouts` / `ShutdownTimeouts`, `Browser::{ws_url, user_data_dir, detach, quit}`, `REDIAL_TIMEOUT` as a setting | decision [#3](decisions.md) |
| **5a-4** | Transport — `observer_timeout`, `event_bus_capacity`, `KeepAlive`, reconnect deadline, `CallError::Timeout` gains `stage` | decision [#4](decisions.md) |
| **5b-1** | Interception — `intercept_rules` chaining into one actor, `RuleFilter`, `AbortReason`, `HostMatcher::with_exceptions`, `tracker_block_resources`, `start_ready` / `on_error` | chain into ONE `InterceptBuilder`, never a second actor |
| **5b-2** | Expect / monitor — hoist `DEFAULT_EXPECT_TIMEOUT`, `.predicate()`, `channel_capacity`, bounded bodies documented honestly | |
| **5b-3** | Output — `PdfBuilder`, screenshot `scale` on the clip path, `Element::screenshot_builder` | |
| **5b-4** | State — `DownloadBehavior`, `Storage::for_origin`, `CookieDelete`, `Element::{frame, hide_overlay}`, `Tab::go`, `evaluate_fn` | |
| **6a** | Cloudflare — `TurnstileSelectors` + `ClickPolicy` + `on_click`; click-once latch, `ChallengeGone` gating | own crate |
| **6b** | Imperva — `ImpervaMarkers` + `clearance_criterion` + `on_inject_solution`; shadow-root walk, mid-flight escalation | own crate |
| **6c** | DataDome — `DataDomeMarkers`, `trust_dd_host`, `with_interception` on the captcha path, escalation | own crate |
| **6d** | Fetcher — `Clone` + `ensure_chrome(&self)`, `http_client` + bounded default, `NetworkPolicy`, `Mirror`, `Artifact`, `FetchOutcome`; harden the two duplicated temp-file writes | decision [#11](decisions.md) |
| **7a** | `browser_open` exposes the 18 unreachable setters; add `browser_connect` | no ledger work — all grandfathered |
| **7b** | Per-tool fixes — `browser_click` pass-throughs, `browser_request` bounded bodies, `browser_goto` timeout, `wait_for_idle` quiet window, `find_all` preset, intercept resource types, expect body caps | |
| **7c** | Uniform inline-blob cap, trim opt-out, `--http-allowed-host`, `source_url`; ledger sweep + schema snapshots | |

**25 PRs.**

## Order

`tab.rs` and `browser.rs` are the conflict hot spots — 2, 3 and 5a all touch both, so land those
groups in sequence and rebase each on the previous rather than opening them in parallel.

Hard edges:

- **3a before 3c and 7b** — both consume the split predicate.
- **4a before 4b–4d** — `impl Default` is what makes every later field addition non-breaking.
- **3c before 7b** — `browser_click`'s `checks` pass-through needs the library type to exist.
- **6a–6d are free.** Four separate crates, no dependency on anything above. Run them whenever,
  in parallel, including alongside the sequential groups.

## Cost

25 PRs is not 25 releases. release-plz accumulates merges into a single open release PR, so
several landing before it merges batch into one version. The ceremony that does scale is CI:
five required checks per PR, roughly seven minutes. That is the price of a diff you are willing
to merge, which is what PR 1 established this project does not otherwise get.
