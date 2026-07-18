//! Browser lifecycle handlers — `browser_open`, `browser_close`,
//! `browser_status`.
//!
//! Each handler is a free async fn that locks the shared
//! [`SessionState`][crate::state::SessionState] internally and returns a
//! typed output (or an [`rmcp::ErrorData`]). The thin `#[tool]` wrappers in
//! [`crate::server`] forward to these.

use std::sync::Arc;

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use zendriver::Browser;
use zendriver::stealth::{Platform, StealthProfile};

use crate::errors::{McpServerError, map_error};
use crate::state::{
    InputProfileChoice, SessionState, StealthOverrides, StealthPlatformChoice, StealthProfileChoice,
};
use crate::tools::common::EmptyInput;

// ---------- browser_open --------------------------------------------------

/// Input for `browser_open`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenInput {
    /// Run Chrome with `--headless=new` (default: `true`).
    #[serde(default = "default_true")]
    pub headless: bool,
    /// Override the session's default stealth profile for this launch.
    /// When `None`, the session-wide default (set via CLI / construct time)
    /// is used.
    #[serde(default)]
    pub stealth_profile: Option<StealthProfileChoice>,
    /// Select the input-timing profile for synthesized keyboard/mouse
    /// events, independent of `stealth_profile`. Defaults to `native`
    /// (zero-overhead, deterministic timing) regardless of the stealth
    /// setting — pass `coherent` to opt into human-paced typing and jittery
    /// mouse motion without needing to also spoof the stealth surface.
    #[serde(default)]
    pub input_profile: Option<InputProfileChoice>,
    /// Chrome profile preferences merged into `Default/Preferences` at launch
    /// (dotted keys → nested objects). See `BrowserBuilder::preference`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferences: Option<std::collections::HashMap<String, serde_json::Value>>,
    /// A fingerprint Persona JSON (as produced by `browser_fingerprint_generate`
    /// or hand-built). Parsed via `Persona::try_from_json`. Opaque on the wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persona: Option<serde_json::Value>,
    /// Enable the bundled curated tracker/fingerprinter blocklist (passive
    /// third-party fingerprinters + cross-site trackers; excludes anti-bot
    /// challenge vendors). Off by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_trackers: Option<bool>,
    /// Custom tracker-blocklist source (url | file | inline domains). Supplying
    /// this implicitly enables blocking; combine with `block_trackers: true`
    /// to also include the bundled list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracker_blocklist: Option<TrackerBlocklist>,
    /// Route the browser through an upstream proxy
    /// (`scheme://[user:pass@]host:port`). Userinfo is auto-split into proxy
    /// auth credentials and answered via the `Fetch.authRequired` handshake
    /// (requires the `interception` feature — without it the credentials are
    /// simply not auto-wired). See `BrowserBuilder::proxy`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<String>,
    /// Auto-derive `locale`/`languages`/`timezone` from the exit IP via a
    /// proxied probe to an IP-geolocation service (default `ip-api.com`).
    /// Opt-in; makes at most ONE outbound HTTP request, at launch, and only
    /// when this is `true`. Mirrors `proxy` above when both are set, so the
    /// probe reports the same exit IP Chrome will actually use. The resolved
    /// timezone is the EXACT IANA zone the probe reports for the exit IP
    /// (not a country-representative one), so multi-timezone countries (US,
    /// RU, CA, AU, BR, ...) get the visitor's real local zone. Overridden by
    /// an explicit `persona`/locale, which also skips the probe. Always
    /// present in the schema so the wire shape is feature-stable; only takes
    /// effect when the server is built with the `geo` feature (otherwise
    /// ignored — a warning is logged).
    #[serde(default)]
    pub geo_auto: bool,
    /// Override the geo-probe endpoint (default `http://ip-api.com/json`).
    /// Only meaningful together with `geo_auto: true`; ignored otherwise.
    /// Same feature gating as `geo_auto`. Note: this bypasses proxy
    /// mirroring — the probe against a custom endpoint is NOT routed through
    /// `proxy` above (the underlying `IpApiResolver::with_proxy` wiring is
    /// crate-private to `zendriver`); only the bundled default endpoint gets
    /// proxy mirroring.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geo_endpoint: Option<String>,
}

/// A custom tracker-blocklist source for `browser_open`.
///
/// One of: a remote `url` (fetched+cached at launch), a local `file` path, or
/// inline `domains`. Supplying any source implicitly enables blocking.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
pub enum TrackerBlocklist {
    /// Fetch + cache a newline-delimited host list from this URL.
    Url { url: String },
    /// Read a newline-delimited host list from this local file path.
    File { path: String },
    /// Block these hostnames directly.
    Domains { domains: Vec<String> },
}

const fn default_true() -> bool {
    true
}

/// Output of `browser_open`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct OpenOutput {
    /// Detected Chrome version string. Empty in v0 (the zendriver lib does
    /// not expose a version accessor); reserved for a follow-up dispatch.
    pub chrome_version: String,
    /// Effective headless flag for the launched browser.
    pub headless: bool,
    /// Effective stealth profile for the launched browser.
    pub profile: StealthProfileChoice,
    /// Effective input-timing profile for the launched browser (`native`
    /// unless `input_profile: "coherent"` was requested).
    pub input_profile: InputProfileChoice,
}

/// Launch Chrome with stealth defaults.
///
/// Records the resulting `Browser` and the id of its initial tab in the
/// session state. Returns [`McpServerError::BrowserAlreadyOpen`] if a
/// browser is already attached.
pub async fn open(
    state: Arc<Mutex<SessionState>>,
    input: OpenInput,
) -> Result<OpenOutput, ErrorData> {
    let mut s = state.lock().await;
    if s.browser.is_some() {
        return Err(map_error(McpServerError::BrowserAlreadyOpen));
    }
    let profile = input.stealth_profile.unwrap_or(s.stealth_profile_choice);
    let stealth = apply_overrides(stealth_profile_for(profile), &s.stealth_overrides);
    let input_profile_choice = input.input_profile.unwrap_or_default();
    let mut builder = Browser::builder()
        .headless(input.headless)
        .stealth(stealth)
        .input_profile(input_profile_for(input_profile_choice));
    if let Some(prefs) = &input.preferences {
        for (k, v) in prefs {
            builder = builder.preference(k.clone(), v.clone());
        }
    }
    if let Some(p) = &input.persona {
        let persona = zendriver::Persona::try_from_json(&p.to_string())
            .map_err(|e| ErrorData::invalid_params(format!("invalid persona JSON: {e}"), None))?;
        builder = builder.persona(persona);
    }
    if let Some(proxy) = &input.proxy {
        builder = builder.proxy(proxy.clone());
    }
    #[cfg(feature = "geo")]
    {
        if input.geo_auto {
            builder = match &input.geo_endpoint {
                Some(endpoint) => {
                    if input.proxy.is_some() {
                        tracing::warn!(
                            "geo_endpoint overrides the default ip-api.com probe but is NOT proxy-mirrored (unlike geo_auto's bundled default) — the probe will hit this endpoint directly from the host, not through `proxy`, so its exit IP may not match the browser's"
                        );
                    }
                    builder.geo_resolver(zendriver::IpApiResolver::new().endpoint(endpoint.clone()))
                }
                None => builder.geo_auto(),
            };
        }
    }
    #[cfg(not(feature = "geo"))]
    if input.geo_auto || input.geo_endpoint.is_some() {
        tracing::warn!(
            "geo_auto/geo_endpoint requested but this server was built without the `geo` feature; ignoring"
        );
    }
    #[cfg(feature = "tracker-blocking")]
    {
        if input.block_trackers.unwrap_or(false) {
            builder = builder.block_trackers(true);
        }
        if let Some(bl) = &input.tracker_blocklist {
            builder = match bl {
                TrackerBlocklist::Url { url } => builder.tracker_blocklist_url(url.clone()),
                TrackerBlocklist::File { path } => {
                    builder.tracker_blocklist_file(std::path::PathBuf::from(path))
                }
                TrackerBlocklist::Domains { domains } => {
                    builder.tracker_blocklist_add(domains.clone())
                }
            };
        }
    }
    #[cfg(not(feature = "tracker-blocking"))]
    if input.block_trackers.unwrap_or(false) || input.tracker_blocklist.is_some() {
        return Err(ErrorData::invalid_params(
            "tracker blocking requested but this server was built without the `tracker-blocking` feature".to_string(),
            None,
        ));
    }
    let browser = builder
        .launch()
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    let tabs = browser.tabs().await;
    s.current_tab_id = tabs.first().map(|t| t.target_id().to_string());
    s.browser = Some(browser);
    s.stealth_profile_choice = profile;
    Ok(OpenOutput {
        chrome_version: String::new(),
        headless: input.headless,
        profile,
        input_profile: input_profile_choice,
    })
}

/// Map the wire-level [`StealthProfileChoice`] to a concrete
/// [`StealthProfile`].
///
/// `Auto` and `Native` both call [`StealthProfile::native`] (auto-detects
/// platform via `sysinfo`). The `Spoof*` variants build a `spoofed()`
/// profile and pin the platform.
fn stealth_profile_for(choice: StealthProfileChoice) -> StealthProfile {
    match choice {
        StealthProfileChoice::Auto | StealthProfileChoice::Native => StealthProfile::native(),
        StealthProfileChoice::SpoofMacos => StealthProfile::spoofed().platform(Platform::MacIntel),
        StealthProfileChoice::SpoofLinux => {
            StealthProfile::spoofed().platform(Platform::LinuxX86_64)
        }
        StealthProfileChoice::SpoofWindows => StealthProfile::spoofed().platform(Platform::Win32),
    }
}

/// Map the wire-level [`InputProfileChoice`] to a concrete
/// [`zendriver::stealth::InputProfile`].
///
/// Resolved independently of `stealth_profile_for` above — the whole point
/// of `BrowserBuilder::input_profile` is that input timing no longer rides
/// along with the stealth choice.
fn input_profile_for(choice: InputProfileChoice) -> zendriver::stealth::InputProfile {
    match choice {
        InputProfileChoice::Native => zendriver::stealth::InputProfile::native(),
        InputProfileChoice::Coherent => zendriver::stealth::InputProfile::coherent(),
    }
}

impl From<StealthPlatformChoice> for Platform {
    fn from(p: StealthPlatformChoice) -> Self {
        match p {
            StealthPlatformChoice::Win32 => Platform::Win32,
            StealthPlatformChoice::MacIntel => Platform::MacIntel,
            StealthPlatformChoice::LinuxX86_64 => Platform::LinuxX86_64,
        }
    }
}

/// Layer fine-grained [`StealthOverrides`] onto a base [`StealthProfile`].
///
/// Each set field overrides the base profile via the corresponding builder
/// method; unset fields leave the base value untouched.
fn apply_overrides(mut profile: StealthProfile, overrides: &StealthOverrides) -> StealthProfile {
    if let Some(platform) = overrides.platform {
        profile = profile.platform(platform.into());
    }
    #[cfg(feature = "geo")]
    if let Some(ref cc) = overrides.geo_country {
        match zendriver_stealth::geo::Country::try_from(cc.as_str()) {
            Ok(country) => {
                let derived = zendriver_stealth::geo::persona(country);
                if let Some(locale) = derived.locale {
                    profile = profile.locale(locale);
                }
                if let Some(langs) = derived.languages {
                    profile = profile.languages(langs);
                }
            }
            Err(_) => tracing::warn!("geo_country {cc:?} is not a valid country code; ignoring"),
        }
    }
    if let Some(ref locale) = overrides.locale {
        profile = profile.locale(locale);
    }
    if let Some(ref timezone) = overrides.timezone {
        profile = profile.timezone(timezone);
    }
    if let Some(memory_gb) = overrides.memory_gb {
        profile = profile.memory_gb(memory_gb);
    }
    if let Some(cpu_count) = overrides.cpu_count {
        profile = profile.cpu_count(cpu_count);
    }
    if let Some(chrome_version) = overrides.chrome_version {
        profile = profile.chrome_version(chrome_version);
    }
    if let Some(ref user_agent) = overrides.user_agent {
        profile = profile.user_agent(user_agent);
    }
    if let Some(bypass_csp) = overrides.bypass_csp {
        profile = profile.bypass_csp(bypass_csp);
    }
    profile
}

// ---------- browser_close -------------------------------------------------

/// Output of `browser_close`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CloseOutput {
    /// Always `true`. Present so the tool's structured output is
    /// non-empty (rmcp clients sometimes treat `{}` as "no payload").
    pub ok: bool,
}

/// Close the open browser. Idempotent — no error if no browser is open.
///
/// Resource cleanup ordering (when applicable cargo features are on):
/// 1. **Expectations**: drain `s.expectations`, calling `.abort()` on each
///    spawned tokio task. Without this, the task's inner `.matched()`
///    future keeps a `Network.*` / `Page.*` subscription alive against a
///    session that's about to disappear, and the task only unwinds when
///    its `pre_await_timeout_ms` finally fires (up to 60s of orphaned work
///    per expectation).
/// 2. **Interception rules**: `s.rules.clear()` drops every stored
///    `InterceptRuleHandle`, whose embedded `InterceptHandle::Drop`
///    cancels the actor (fire-and-forget `Fetch.disable`). Doing this
///    *before* `Browser::close` means the actor's teardown happens while
///    the transport is still live, so the disable round-trip actually
///    lands instead of being eaten by a closed connection.
/// 3. **Browser**: only then do we `b.close().await`, which tears down
///    the transport and the CDP connection.
pub async fn close(
    state: Arc<Mutex<SessionState>>,
    _: EmptyInput,
) -> Result<CloseOutput, ErrorData> {
    let mut s = state.lock().await;

    #[cfg(feature = "expect")]
    {
        // Drain + abort: dropping the handle alone leaks the spawned task
        // until its pre_await_timeout fires. `.abort()` is the only way to
        // promptly tear down the `.matched()` future the task is parked
        // on.
        for (_, h) in s.expectations.drain() {
            h.task.abort();
        }
    }

    #[cfg(feature = "interception")]
    {
        // `InterceptHandle::Drop` cancels the per-rule actor (fire-and-
        // forget `Fetch.disable`). Doing this before `Browser::close`
        // gives each `Fetch.disable` a live transport to land on; doing
        // it after would race a closed connection.
        s.rules.clear();
    }

    #[cfg(feature = "monitor")]
    {
        // Stop every running monitor before closing. `MonitorState::Drop`
        // cancels each drain task (and the lib's `NetworkMonitor` correlator);
        // doing it before `Browser::close` lets the cancels land on a live
        // session, mirroring the interception-rules teardown above.
        s.monitors.clear();
    }

    if let Some(b) = s.browser.take() {
        b.close()
            .await
            .map_err(|e| map_error(McpServerError::from(e)))?;
    }
    s.current_tab_id = None;
    Ok(CloseOutput { ok: true })
}

// ---------- browser_status ------------------------------------------------

/// Lightweight summary of the current tab — returned inside [`StatusOutput`].
#[derive(Debug, Serialize, JsonSchema)]
pub struct TabSummary {
    pub id: String,
    pub url: String,
    pub title: String,
}

/// Output of `browser_status`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct StatusOutput {
    /// `true` iff a Browser is currently launched in this session.
    pub open: bool,
    /// Number of live tabs (0 when no browser is open).
    pub tab_count: usize,
    /// `id` / `url` / `title` of the currently-focused tab, or `null`.
    pub current_tab: Option<TabSummary>,
    /// Chrome DevTools inspector URL for the focused tab, when one is open.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inspector_url: Option<String>,
    /// Configured stealth profile choice for this session.
    pub profile: StealthProfileChoice,
}

/// Report whether a browser is open, the current tab (if any), and the
/// configured stealth profile.
pub async fn status(
    state: Arc<Mutex<SessionState>>,
    _: EmptyInput,
) -> Result<StatusOutput, ErrorData> {
    let s = state.lock().await;
    let Some(b) = s.browser.as_ref() else {
        return Ok(StatusOutput {
            open: false,
            tab_count: 0,
            current_tab: None,
            inspector_url: None,
            profile: s.stealth_profile_choice,
        });
    };
    let tabs = b.tabs().await;
    let mut inspector_url = None;
    let current_tab = match &s.current_tab_id {
        Some(id) => {
            let mut found = None;
            for t in &tabs {
                if t.target_id() == id {
                    let url = t.url().await.map(|u| u.to_string()).unwrap_or_default();
                    let title = t.title().await.unwrap_or_default();
                    // `inspector_url` is sync + best-effort; a failure just
                    // leaves the field absent.
                    inspector_url = t.inspector_url().ok();
                    found = Some(TabSummary {
                        id: t.target_id().to_string(),
                        url,
                        title,
                    });
                    break;
                }
            }
            found
        }
        None => None,
    };
    Ok(StatusOutput {
        open: true,
        tab_count: tabs.len(),
        current_tab,
        inspector_url,
        profile: s.stealth_profile_choice,
    })
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn close_with_no_browser_is_noop() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let out = close(state, EmptyInput {}).await.expect("close ok");
        assert!(out.ok);
    }

    #[tokio::test]
    async fn status_with_no_browser_reports_closed() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let out = status(state, EmptyInput {}).await.expect("status ok");
        assert!(!out.open);
        assert_eq!(out.tab_count, 0);
        assert!(out.current_tab.is_none());
        assert_eq!(out.profile, StealthProfileChoice::Auto);
    }

    /// `close` drains every entry in `s.expectations`, calling `.abort()`
    /// on each spawned task. Without this the orphan tasks remain parked
    /// on their `.matched()` futures until `pre_await_timeout_ms` fires.
    #[cfg(feature = "expect")]
    #[tokio::test]
    async fn close_drains_and_aborts_expectations() {
        use crate::state::ExpectationHandle;

        let state = Arc::new(Mutex::new(SessionState::new()));

        // Spawn a long-lived sentinel task. We hold the oneshot Sender so
        // the task can't complete on its own — `.abort()` is the only way
        // out. After `close` we check the task's join handle reports
        // cancellation.
        let (_tx_keep_alive, rx) =
            tokio::sync::oneshot::channel::<Result<serde_json::Value, String>>();
        let task = tokio::spawn(async move {
            // Park indefinitely. `close()` must abort us.
            std::future::pending::<()>().await;
        });
        let join_handle_for_check = task.abort_handle();
        {
            let mut s = state.lock().await;
            s.expectations.insert(
                "test-id".into(),
                ExpectationHandle {
                    kind: "request",
                    task,
                    rx,
                },
            );
            assert_eq!(s.expectations.len(), 1, "precondition: expectation present");
        }

        let out = close(state.clone(), EmptyInput {}).await.expect("close ok");
        assert!(out.ok);

        let s = state.lock().await;
        assert!(
            s.expectations.is_empty(),
            "expectations map must be empty after close (was: {})",
            s.expectations.len(),
        );
        // Yield once to let the abort signal propagate so the task is
        // observably finished.
        drop(s);
        tokio::task::yield_now().await;
        assert!(
            join_handle_for_check.is_finished(),
            "expectation task should be aborted (and thus finished) after close",
        );
    }

    /// `close` clears every entry in `s.rules`. Each
    /// `InterceptRuleHandle::_handle` drop fires the interception actor's
    /// cancel token (which would dispatch `Fetch.disable` against the live
    /// transport in a real session — here we just verify the map is
    /// emptied).
    #[cfg(feature = "geo")]
    #[test]
    fn geo_country_sets_locale_and_languages() {
        let overrides = StealthOverrides {
            geo_country: Some("US".into()),
            ..Default::default()
        };
        let profile = apply_overrides(StealthProfile::native(), &overrides);
        let flags = profile.build_flags();
        assert!(
            flags.iter().any(|f| f == "--lang=en-US"),
            "expected --lang=en-US in flags: {flags:?}",
        );
    }

    #[cfg(feature = "interception")]
    #[tokio::test]
    async fn close_clears_interception_rules() {
        use crate::state::InterceptRuleHandle;

        let state = Arc::new(Mutex::new(SessionState::new()));
        {
            let mut s = state.lock().await;
            s.rules.insert(
                "rule-a".into(),
                InterceptRuleHandle {
                    pattern: "*/ads/*".into(),
                    action_kind: "block",
                    _handle: zendriver_interception::InterceptHandle::for_tests(),
                },
            );
            s.rules.insert(
                "rule-b".into(),
                InterceptRuleHandle {
                    pattern: "*/api/*".into(),
                    action_kind: "respond",
                    _handle: zendriver_interception::InterceptHandle::for_tests(),
                },
            );
            assert_eq!(s.rules.len(), 2, "precondition: rules present");
        }

        let out = close(state.clone(), EmptyInput {}).await.expect("close ok");
        assert!(out.ok);

        let s = state.lock().await;
        assert!(
            s.rules.is_empty(),
            "rules map must be empty after close (was: {})",
            s.rules.len(),
        );
    }
}
