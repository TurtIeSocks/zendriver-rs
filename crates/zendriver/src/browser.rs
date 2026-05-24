//! Browser lifecycle: executable discovery, subprocess spawn, WS attach,
//! graceful teardown.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

use serde_json::json;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::timeout;
use tracing::{debug, info, warn};
use zendriver_stealth::{StealthObserver, StealthProfile};
use zendriver_transport::{
    Connection, ObserverError, PausedSession, SessionHandle, TargetObserver,
};

use crate::error::{BrowserError, ZendriverError};
use crate::input::InputController;
use crate::tab::Tab;

/// Look for a Chromium-family binary on PATH and in conventional locations.
/// Returns the first path that exists.
pub fn find_chrome_executable() -> Result<PathBuf, BrowserError> {
    let candidates = candidate_paths();
    for c in &candidates {
        if c.exists() {
            return Ok(c.clone());
        }
    }
    Err(BrowserError::ExecutableNotFound {
        searched: candidates,
    })
}

fn candidate_paths() -> Vec<PathBuf> {
    let mut v = Vec::new();

    // PATH lookups.
    for name in [
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "chrome",
    ] {
        if let Some(p) = which_on_path(name) {
            v.push(p);
        }
    }

    // Platform-specific known locations.
    #[cfg(target_os = "macos")]
    {
        v.push(PathBuf::from(
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        ));
        v.push(PathBuf::from(
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
        ));
    }
    #[cfg(target_os = "linux")]
    {
        v.push(PathBuf::from("/usr/bin/google-chrome"));
        v.push(PathBuf::from("/usr/bin/chromium"));
        v.push(PathBuf::from("/usr/bin/chromium-browser"));
        v.push(PathBuf::from("/snap/bin/chromium"));
    }
    #[cfg(target_os = "windows")]
    {
        v.push(PathBuf::from(
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        ));
        v.push(PathBuf::from(
            r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        ));
    }

    v
}

fn which_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let full = dir.join(name);
        if full.is_file() {
            return Some(full);
        }
        #[cfg(target_os = "windows")]
        {
            let with_exe = dir.join(format!("{name}.exe"));
            if with_exe.is_file() {
                return Some(with_exe);
            }
        }
    }
    None
}

/// Parse a `DevTools listening on ws://...` line from Chrome's stderr.
pub(crate) fn parse_devtools_line(line: &str) -> Option<String> {
    // Format: `DevTools listening on ws://127.0.0.1:NNNN/devtools/browser/UUID`
    let needle = "DevTools listening on ";
    let idx = line.find(needle)?;
    let rest = &line[idx + needle.len()..];
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let url = rest[..end].trim();
    if url.starts_with("ws://") || url.starts_with("wss://") {
        Some(url.to_string())
    } else {
        None
    }
}

#[derive(Default, Clone)]
pub struct BrowserBuilder {
    pub(crate) headless: Option<bool>,
    pub(crate) executable: Option<PathBuf>,
    pub(crate) user_data_dir: Option<PathBuf>,
    pub(crate) extra_args: Vec<String>,
    pub(crate) stealth: Option<StealthProfile>,
    pub(crate) extra_observers: Vec<Arc<dyn TargetObserver>>,
}

impl BrowserBuilder {
    /// Builder seeded with the default `StealthProfile::native()` profile.
    /// Pass `.stealth(StealthProfile::off())` to opt out, or
    /// `.stealth(StealthProfile::spoofed())` for the full anti-detection set.
    #[must_use]
    pub fn new() -> Self {
        Self {
            stealth: Some(StealthProfile::native()),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn headless(mut self, on: bool) -> Self {
        self.headless = Some(on);
        self
    }

    #[must_use]
    pub fn executable(mut self, path: impl Into<PathBuf>) -> Self {
        self.executable = Some(path.into());
        self
    }

    #[must_use]
    pub fn user_data_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.user_data_dir = Some(path.into());
        self
    }

    #[must_use]
    pub fn arg(mut self, flag: impl Into<String>) -> Self {
        self.extra_args.push(flag.into());
        self
    }

    #[must_use]
    pub fn args(mut self, flags: impl IntoIterator<Item = String>) -> Self {
        self.extra_args.extend(flags);
        self
    }

    /// Override the default `StealthProfile::native()` profile. Pass
    /// `StealthProfile::off()` to disable stealth entirely.
    #[must_use]
    pub fn stealth(mut self, profile: StealthProfile) -> Self {
        self.stealth = Some(profile);
        self
    }

    /// Register an additional `TargetObserver` that fires on each new attached
    /// page target. The stealth observer (if any) is added before user observers.
    #[must_use]
    pub fn observer(mut self, obs: Arc<dyn TargetObserver>) -> Self {
        self.extra_observers.push(obs);
        self
    }

    /// Compute the full argv that would be passed to Chrome. Exposed to
    /// tests + snapshots; called internally by `launch`.
    pub(crate) fn build_flags(&self, user_data_dir: &Path) -> Vec<String> {
        let mut v = Vec::with_capacity(8 + self.extra_args.len());
        v.push("--remote-debugging-port=0".to_string());
        v.push(format!("--user-data-dir={}", user_data_dir.display()));
        v.push("--no-first-run".to_string());
        v.push("--no-default-browser-check".to_string());
        if self.headless.unwrap_or(true) {
            v.push("--headless=new".to_string());
            v.push("--disable-gpu".to_string());
        }
        v.extend(self.extra_args.iter().cloned());
        v
    }
}

#[derive(Clone)]
pub struct Browser {
    pub(crate) inner: Arc<BrowserInner>,
}

pub(crate) struct BrowserInner {
    pub(crate) conn: Connection,
    pub(crate) main_tab: Tab,
    pub(crate) child: tokio::sync::Mutex<Option<Child>>,
    pub(crate) _user_data: Option<TempDir>,
    /// Cached `InputProfile` from the active `StealthProfile` (or
    /// `InputProfile::native` when stealth is off). `Browser::new_tab` and
    /// the [`TabRegistrar`] observer read this to build a fresh per-Tab
    /// [`InputController`] for each new tab without re-resolving the
    /// stealth profile.
    ///
    /// Currently consumed only by the [`TabRegistrar`] (via the clone
    /// stashed inside the registrar at construction time); a direct
    /// `Browser::new_tab` path will read this field once T3 lands, so
    /// `dead_code` is suppressed in the interim.
    #[allow(dead_code)]
    pub(crate) stealth_input_profile: zendriver_stealth::InputProfile,
    /// Browser-wide tab registry keyed by `sessionId`. Populated by the
    /// [`TabRegistrar`] observer on `Target.attachedToTarget` (and the
    /// initial main tab, inserted manually after construction); pruned on
    /// `Target.detachedFromTarget`. Used by `Browser::new_tab` to discover
    /// the [`Tab`] handle for a freshly-created page target and by
    /// `Browser::tabs` / `Browser::tab_count` for snapshot reads.
    pub(crate) tabs: tokio::sync::RwLock<HashMap<String, Tab>>,
}

/// [`TargetObserver`] that maintains [`BrowserInner::tabs`] in step with
/// CDP target lifecycle events.
///
/// On `Target.attachedToTarget` with `target_info.kind == "page"`, it builds
/// a fresh [`Tab`] for the new session (with its own [`InputController`]
/// seeded from the cached [`zendriver_stealth::InputProfile`]) and inserts
/// it into the registry. On `Target.detachedFromTarget`, the matching entry
/// is removed.
///
/// The observer holds a [`Weak`] reference to [`BrowserInner`] so the
/// observer chain does not extend the browser's lifetime — if the browser is
/// dropped before a target event arrives, the upgrade fails silently and
/// the event is ignored. The weak ref is wired in via [`OnceLock::set`]
/// after the surrounding [`Arc::new_cyclic`] resolves; before that point
/// the registrar is constructed empty.
///
/// Registered AFTER [`StealthObserver`] in the observer chain so stealth
/// patches apply before any user code sees the new tab.
pub(crate) struct TabRegistrar {
    browser: OnceLock<Weak<BrowserInner>>,
    input_profile: zendriver_stealth::InputProfile,
}

impl TabRegistrar {
    fn new(input_profile: zendriver_stealth::InputProfile) -> Self {
        Self {
            browser: OnceLock::new(),
            input_profile,
        }
    }

    /// Wire the weak [`BrowserInner`] ref. Called once after
    /// [`Arc::new_cyclic`] resolves; subsequent calls are silently ignored.
    fn set_browser(&self, browser: Weak<BrowserInner>) {
        let _ = self.browser.set(browser);
    }
}

#[async_trait::async_trait]
impl TargetObserver for TabRegistrar {
    fn name(&self) -> &'static str {
        "tab-registrar"
    }

    async fn on_target_attached(&self, session: PausedSession<'_>) -> Result<(), ObserverError> {
        // Only page targets become tabs. Iframes (OOPIF) and workers are
        // handled separately in later P4 tasks.
        if session.target_info.kind != "page" {
            return Ok(());
        }
        let Some(weak) = self.browser.get() else {
            // Registrar wired into observer chain before `set_browser` ran.
            // Should not happen in practice because launch wires the weak
            // before any observer can fire — log + bail gracefully if it
            // ever does.
            warn!("TabRegistrar fired before browser weak ref was wired; skipping");
            return Ok(());
        };
        let Some(browser) = weak.upgrade() else {
            // Browser dropped between event arrival and observer body —
            // nothing to register against.
            return Ok(());
        };

        let conn = session.connection().clone();
        let new_session = SessionHandle::new(conn, session.session_id.to_string());
        let input = InputController::new(self.input_profile.clone());
        let weak_inner = Arc::downgrade(&browser);
        let tab = Tab::new(
            new_session,
            weak_inner,
            input,
            session.target_info.target_id.clone(),
        );

        browser
            .tabs
            .write()
            .await
            .insert(session.session_id.to_string(), tab);
        Ok(())
    }

    async fn on_target_detached(&self, session_id: &str) {
        let Some(weak) = self.browser.get() else {
            return;
        };
        let Some(browser) = weak.upgrade() else {
            return;
        };
        browser.tabs.write().await.remove(session_id);
    }
}

const WS_ENDPOINT_TIMEOUT: Duration = Duration::from_secs(15);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

impl BrowserBuilder {
    /// Spawn Chrome and attach. Returns once the main tab is bound.
    ///
    /// When a `StealthProfile` is set (the default), this:
    /// 1. Resolves a `Fingerprint` from the resolved Chrome executable.
    /// 2. Prepends the profile's `StealthObserver` to the observer chain.
    /// 3. Appends the profile's stealth flags to the launch argv.
    /// 4. Sends `Target.setAutoAttach { waitForDebuggerOnStart: true }` at
    ///    browser scope so the actor can route pauses through observers
    ///    before any page script runs.
    pub async fn launch(self) -> Result<Browser, ZendriverError> {
        // 1. Resolve Chrome executable.
        let exe = match self.executable.clone() {
            Some(p) => p,
            None => find_chrome_executable()?,
        };

        // 2. Resolve the per-tab InputProfile from the active StealthProfile
        // (or zero-overhead `native` when stealth is off). Cached on
        // `BrowserInner` so `Browser::new_tab` + the `TabRegistrar` observer
        // can build fresh per-Tab controllers without re-resolving the
        // profile each time.
        let input_profile = self
            .stealth
            .as_ref()
            .map_or_else(zendriver_stealth::InputProfile::native, |sp| {
                sp.input_profile()
            });

        // 3. Build the `TabRegistrar` observer. Holds a `OnceLock` for the
        // `Weak<BrowserInner>` that gets wired in step 10 — observers must
        // be passed to `connect_with_observers` before the cyclic `Arc` is
        // resolved, so the weak ref is filled in later. Retained here so
        // we can `set_browser` after construction.
        let registrar = Arc::new(TabRegistrar::new(input_profile.clone()));

        // 4. Resolve fingerprint + build observer chain + profile flags.
        // Observer order: stealth (patches each new target) → tab registrar
        // (records the resulting Tab handle) → user-supplied observers.
        let (observers, extra_flags): (Vec<Arc<dyn TargetObserver>>, Vec<String>) =
            if let Some(ref profile) = self.stealth {
                let fp = profile.resolve_fingerprint(&exe)?;
                let stealth_obs: Arc<dyn TargetObserver> =
                    Arc::new(StealthObserver::new(profile.clone(), fp));
                let mut obs_vec = Vec::with_capacity(2 + self.extra_observers.len());
                obs_vec.push(stealth_obs);
                obs_vec.push(registrar.clone() as Arc<dyn TargetObserver>);
                obs_vec.extend(self.extra_observers.iter().cloned());
                (obs_vec, profile.build_flags())
            } else {
                let mut obs_vec = Vec::with_capacity(1 + self.extra_observers.len());
                obs_vec.push(registrar.clone() as Arc<dyn TargetObserver>);
                obs_vec.extend(self.extra_observers.iter().cloned());
                (obs_vec, Vec::new())
            };

        // 5. Allocate user_data_dir (or use a TempDir we keep alive until shutdown).
        let (user_data_path, owned_tmp) = match self.user_data_dir.clone() {
            Some(p) => (p, None),
            None => {
                let td = tempfile::Builder::new()
                    .prefix("zendriver-")
                    .tempdir()
                    .map_err(BrowserError::SpawnFailed)?;
                (td.path().to_path_buf(), Some(td))
            }
        };

        let mut flags = self.build_flags(&user_data_path);
        flags.extend(extra_flags);
        info!(executable = %exe.display(), "launching chrome");

        // 6. Spawn chrome + parse WS URL.
        let mut cmd = Command::new(&exe);
        cmd.args(&flags)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd.spawn().map_err(BrowserError::SpawnFailed)?;

        // Read stderr line-by-line until we see the DevTools URL.
        let stderr = child.stderr.take().ok_or(BrowserError::DevtoolsParse)?;
        let mut lines = BufReader::new(stderr).lines();

        let ws_url = timeout(WS_ENDPOINT_TIMEOUT, async {
            while let Ok(Some(line)) = lines.next_line().await {
                debug!(line = %line, "chrome stderr");
                if let Some(url) = parse_devtools_line(&line) {
                    return Ok::<String, ZendriverError>(url);
                }
            }
            Err(BrowserError::DevtoolsParse.into())
        })
        .await
        .map_err(|_| BrowserError::WsTimeout)??;

        // 7. Connect with observers.
        debug!(ws_url = %ws_url, "connecting to chrome");
        let conn = zendriver_transport::connect_with_observers(&ws_url, observers).await?;

        // 8. Enable auto-attach with debugger-pause BEFORE attaching to the
        // initial target. Sent at browser scope (no session_id) so it covers
        // both the initial target and any subsequently-opened pages/iframes.
        conn.call_raw(
            "Target.setAutoAttach",
            json!({
                "autoAttach": true,
                "waitForDebuggerOnStart": true,
                "flatten": true,
            }),
            None,
        )
        .await?;

        // 9. Discover initial target via Target.getTargets.
        let list = conn.call_raw("Target.getTargets", json!({}), None).await?;
        let target_id = list["targetInfos"]
            .as_array()
            .and_then(|arr| {
                arr.iter()
                    .find(|t| t["type"] == "page")
                    .or_else(|| arr.first())
            })
            .and_then(|t| t["targetId"].as_str())
            .ok_or_else(|| ZendriverError::Navigation("no initial target found".into()))?
            .to_string();

        // 10. Attach to the initial target. This triggers `Target.attachedToTarget`
        // which the actor routes through observers (`on_target_attached`) and
        // then releases via `Runtime.runIfWaitingForDebugger`.
        //
        // The `TabRegistrar` observer (in the chain) will try to insert into
        // `BrowserInner.tabs` for the main tab too. That insertion is a
        // no-op because the weak ref isn't wired yet (`OnceLock` empty →
        // observer warns + skips). We re-insert the main tab manually in
        // step 12 so the registry is consistent post-launch.
        let attach = conn
            .call_raw(
                "Target.attachToTarget",
                json!({ "targetId": target_id, "flatten": true }),
                None,
            )
            .await?;
        let session_id = attach["sessionId"]
            .as_str()
            .ok_or_else(|| ZendriverError::Navigation("attach returned no sessionId".into()))?
            .to_string();

        // 11. Wrap session in Tab; build BrowserInner.
        //
        // `Arc::new_cyclic` is the canonical pattern for building
        // self-referential Arc graphs: the inner closure receives a
        // `Weak<BrowserInner>` it can hand to the Tab. The Tab uses that
        // weak ref for later access to Browser-wide resources (CookieJar,
        // tabs registry); the per-Tab `InputController` is constructed
        // inline here from the cached `input_profile`.
        let session_id_for_registry = session_id.clone();
        let target_id_for_main_tab = target_id.clone();
        let inner = Arc::new_cyclic(|weak: &std::sync::Weak<BrowserInner>| {
            let session = SessionHandle::new(conn.clone(), session_id);
            let main_tab_input = InputController::new(input_profile.clone());
            let main_tab = Tab::new(
                session,
                weak.clone(),
                main_tab_input,
                target_id_for_main_tab,
            );
            BrowserInner {
                conn,
                main_tab,
                child: tokio::sync::Mutex::new(Some(child)),
                _user_data: owned_tmp,
                stealth_input_profile: input_profile,
                tabs: tokio::sync::RwLock::new(HashMap::new()),
            }
        });

        // 12. Wire the registrar's weak ref + manually insert the main tab.
        //
        // The main tab was attached BEFORE the registrar had a usable weak
        // ref (the `Arc::new_cyclic` block above only just resolved), so
        // the registrar's `on_target_attached` ran with `OnceLock` empty
        // and bailed early. Backfill the registry here so callers see the
        // expected 1-entry state immediately after `launch`.
        registrar.set_browser(Arc::downgrade(&inner));
        inner
            .tabs
            .write()
            .await
            .insert(session_id_for_registry, inner.main_tab.clone());

        Ok(Browser { inner })
    }
}

impl Browser {
    pub fn builder() -> BrowserBuilder {
        BrowserBuilder::new()
    }

    pub fn main_tab(&self) -> Tab {
        self.inner.main_tab.clone()
    }

    pub fn cdp(&self) -> &Connection {
        &self.inner.conn
    }

    /// Browser-wide cookie store handle. Cookies in Chrome are stored once
    /// per profile and shared across all tabs, so the jar binds to the
    /// browser's root [`Connection`] and dispatches `Network.*Cookies*`
    /// commands at browser scope (no `sessionId`).
    ///
    /// Cheap to call — [`crate::CookieJar`] is an `Arc`-backed handle, and
    /// each invocation constructs a fresh wrapper around the cloned
    /// connection.
    #[must_use]
    pub fn cookies(&self) -> crate::CookieJar {
        crate::CookieJar::new(self.inner.conn.clone())
    }

    /// Open a new tab navigated to `about:blank`. Returns once the
    /// [`TabRegistrar`] has registered the new [`Tab`] in the browser's tab
    /// registry — typically within a few milliseconds of
    /// `Target.createTarget`'s response.
    ///
    /// Internally:
    /// 1. Sends `Target.createTarget { url: "about:blank" }` at browser
    ///    scope (no session_id) — the response includes the new `targetId`.
    /// 2. Polls [`BrowserInner::tabs`] every 50ms for up to 5s, looking for
    ///    a [`Tab`] whose [`Tab::target_id`] matches. The
    ///    [`TabRegistrar`] populates that entry asynchronously when the
    ///    `Target.attachedToTarget` event arrives (auto-attach + flatten
    ///    are enabled at launch time).
    /// 3. Returns the matching [`Tab`] on success.
    ///
    /// Returns [`ZendriverError::TabNotFound`] if the registrar fails to
    /// register the new tab within the 5s window — usually a sign that
    /// auto-attach is misconfigured or the registrar observer crashed.
    pub async fn new_tab(&self) -> Result<Tab, ZendriverError> {
        self.new_tab_at("about:blank").await
    }

    /// Open a new tab navigated to `url`. Behaves identically to
    /// [`Browser::new_tab`] but with a custom initial URL passed to
    /// `Target.createTarget`. The returned [`Tab`] handle is ready as soon
    /// as the [`TabRegistrar`] observer registers it; callers can issue
    /// `.wait_for_load()` if they need to block on the navigation.
    pub async fn new_tab_at(&self, url: impl Into<String>) -> Result<Tab, ZendriverError> {
        let url = url.into();
        let res = self
            .inner
            .conn
            .call_raw("Target.createTarget", json!({ "url": url }), None)
            .await?;
        let target_id = res
            .get("targetId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ZendriverError::Navigation("Target.createTarget returned no targetId".into())
            })?
            .to_string();

        // Poll the registrar-maintained tabs map for the new Tab.
        // The 5s window covers the typical CDP roundtrip + observer chain
        // (stealth → tab-registrar) latency with comfortable headroom.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            {
                let tabs = self.inner.tabs.read().await;
                if let Some(tab) = tabs.values().find(|t| t.target_id() == target_id) {
                    return Ok(tab.clone());
                }
            }
            if std::time::Instant::now() >= deadline {
                return Err(ZendriverError::TabNotFound(target_id));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Snapshot of all currently-registered tabs. Order is unspecified
    /// (the registry is a [`HashMap`] keyed by `sessionId`). Includes the
    /// main tab plus any tabs opened via [`Browser::new_tab`] or by page
    /// scripts (e.g. `window.open`) that auto-attach has wired into the
    /// registrar.
    pub async fn tabs(&self) -> Vec<Tab> {
        self.inner.tabs.read().await.values().cloned().collect()
    }

    /// Convenience accessor: count of currently-registered tabs.
    /// Equivalent to `self.tabs().await.len()` but avoids the
    /// `Vec` allocation.
    pub async fn tab_count(&self) -> usize {
        self.inner.tabs.read().await.len()
    }

    /// Graceful shutdown: cancel the transport, send SIGTERM to Chrome,
    /// wait up to `SHUTDOWN_GRACE`, then SIGKILL on timeout. Cleans up
    /// user_data_dir.
    pub async fn close(self) -> Result<(), ZendriverError> {
        self.inner.conn.shutdown();
        let mut child_guard = self.inner.child.lock().await;
        if let Some(mut child) = child_guard.take() {
            // Try graceful exit first. On Unix, tokio's `start_kill` is
            // `kill(pid, SIGKILL)` — too aggressive for graceful shutdown.
            // We send SIGTERM ourselves and fall back to SIGKILL on timeout.
            #[cfg(unix)]
            {
                if let Some(pid) = child.id() {
                    // SIGTERM gives Chrome a chance to flush + clean up; SIGKILL fallback below.
                    // Safety: just sending a signal to a known pid; no shared state.
                    #[allow(unsafe_code)]
                    unsafe {
                        libc::kill(pid as i32, libc::SIGTERM);
                    }
                }
            }
            #[cfg(not(unix))]
            {
                let _ = child.start_kill(); // best-effort on non-Unix
            }
            match timeout(SHUTDOWN_GRACE, child.wait()).await {
                Ok(Ok(_status)) => {}
                _ => {
                    let _ = child.kill().await;
                }
            }
        }
        Ok(())
    }
}

impl Drop for BrowserInner {
    fn drop(&mut self) {
        self.conn.shutdown();
        // We can't `.await` in Drop. If `close()` was not called explicitly,
        // we rely on `kill_on_drop(true)` set on the spawned Command, which
        // causes tokio to SIGKILL the child when the Child is dropped.
        // The TempDir for user_data_dir is dropped here too.
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn candidate_paths_is_nonempty() {
        let v = candidate_paths();
        assert!(!v.is_empty());
    }

    #[test]
    fn find_chrome_executable_returns_err_when_none_exist() {
        // Force an empty PATH and assert ExecutableNotFound on a system
        // without any default-location binaries. We can't reliably do this
        // cross-platform without mocking, so we just test the type signature
        // by calling the function in a save way:
        let _ = find_chrome_executable();
    }

    #[test]
    fn parse_devtools_line_extracts_ws_url() {
        let line = "DevTools listening on ws://127.0.0.1:54321/devtools/browser/abc-def-123\n";
        assert_eq!(
            parse_devtools_line(line).as_deref(),
            Some("ws://127.0.0.1:54321/devtools/browser/abc-def-123")
        );
    }

    #[test]
    fn parse_devtools_line_returns_none_for_unrelated() {
        assert!(parse_devtools_line("loading extension foo").is_none());
        assert!(parse_devtools_line("DevTools listening on http://x").is_none());
    }

    #[test]
    fn parse_devtools_line_handles_prefixed_log_lines() {
        // Real Chrome stderr is sometimes prefixed with [pid:tid:date:level].
        let line = "[12345:1234:0102/030405.000000:INFO:browser.cc] DevTools listening on ws://localhost:1/devtools/browser/x";
        assert_eq!(
            parse_devtools_line(line).as_deref(),
            Some("ws://localhost:1/devtools/browser/x")
        );
    }

    #[test]
    fn build_flags_default_is_headless() {
        let b = BrowserBuilder::new();
        let flags = b.build_flags(Path::new("/tmp/x"));
        assert!(flags.contains(&"--headless=new".to_string()));
        assert!(flags.contains(&"--disable-gpu".to_string()));
        assert!(flags.contains(&"--user-data-dir=/tmp/x".to_string()));
        assert!(flags.contains(&"--remote-debugging-port=0".to_string()));
    }

    #[test]
    fn build_flags_no_headless_when_disabled() {
        let b = BrowserBuilder::new().headless(false);
        let flags = b.build_flags(Path::new("/tmp/x"));
        assert!(!flags.iter().any(|f| f.starts_with("--headless")));
        assert!(!flags.contains(&"--disable-gpu".to_string()));
    }

    #[test]
    fn build_flags_includes_extra_args_in_order() {
        let b = BrowserBuilder::new()
            .arg("--proxy-server=http://x")
            .arg("--lang=en-US");
        let flags = b.build_flags(Path::new("/tmp/x"));
        let proxy = flags
            .iter()
            .position(|f| f == "--proxy-server=http://x")
            .unwrap();
        let lang = flags.iter().position(|f| f == "--lang=en-US").unwrap();
        assert!(proxy < lang);
    }

    #[test]
    fn default_launch_flags_snapshot() {
        let b = BrowserBuilder::new();
        let flags = b.build_flags(std::path::Path::new("/tmp/test-user-data"));
        insta::assert_yaml_snapshot!("default_launch_flags", flags);
    }

    #[test]
    fn non_headless_launch_flags_snapshot() {
        let b = BrowserBuilder::new().headless(false);
        let flags = b.build_flags(std::path::Path::new("/tmp/test-user-data"));
        insta::assert_yaml_snapshot!("non_headless_launch_flags", flags);
    }

    // ----- TabRegistrar observer (P4 T2) ---------------------------------

    /// Mock-drive a `Target.attachedToTarget` event with `type=page` and
    /// assert the [`TabRegistrar`] inserts the new [`Tab`] into the
    /// browser-wide tabs registry. The initial main tab (manually seeded
    /// by `launch` step 12) accounts for the first entry; this test
    /// confirms a second attach grows the map to 2.
    #[tokio::test]
    async fn tab_registrar_inserts_page_target_into_tabs_map() {
        use zendriver_transport::testing::MockConnection;

        let input_profile = zendriver_stealth::InputProfile::native();
        let registrar = Arc::new(TabRegistrar::new(input_profile.clone()));
        let (mut mock, conn) =
            MockConnection::pair_with_observers(vec![registrar.clone() as Arc<dyn TargetObserver>]);

        // Seed a `BrowserInner` carrying a synthetic "main" tab — same
        // shape `launch` produces after step 12 (the main tab is inserted
        // under its real `sessionId`; here we use "S1" for the simulated
        // initial target).
        let inner = Arc::new_cyclic(|weak: &Weak<BrowserInner>| {
            let main_session = SessionHandle::new(conn.clone(), "S1");
            let main_input = InputController::new(input_profile.clone());
            let main_tab = Tab::new(main_session, weak.clone(), main_input, "T1".to_string());
            let mut map = HashMap::new();
            map.insert("S1".to_string(), main_tab.clone());
            BrowserInner {
                conn: conn.clone(),
                main_tab,
                child: tokio::sync::Mutex::new(None),
                _user_data: None,
                stealth_input_profile: input_profile.clone(),
                tabs: tokio::sync::RwLock::new(map),
            }
        });
        registrar.set_browser(Arc::downgrade(&inner));

        // Sanity: one entry to start.
        assert_eq!(inner.tabs.read().await.len(), 1);

        // Emit the attach event for a second page target. The actor will
        // dispatch the registrar observer; once it returns Ok the actor
        // releases the debugger via `Runtime.runIfWaitingForDebugger`,
        // which is our signal that the observer body finished.
        mock.emit_event(
            "Target.attachedToTarget",
            json!({
                "sessionId": "S2",
                "targetInfo": {
                    "targetId": "T2",
                    "type": "page",
                    "url": "about:blank",
                    "attached": true,
                },
                "waitingForDebugger": true,
            }),
        )
        .await;

        let release_id = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            mock.expect_cmd("Runtime.runIfWaitingForDebugger"),
        )
        .await
        .expect("debugger-release did not fire within 2s");
        mock.reply(release_id, json!({})).await;

        // Give the actor a moment to drop its strong ref to the observer's
        // upgraded Arc and let our `inner.tabs` write land.
        for _ in 0..20 {
            if inner.tabs.read().await.len() == 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let tabs = inner.tabs.read().await;
        assert_eq!(tabs.len(), 2, "expected main + new tab in registry");
        assert!(tabs.contains_key("S1"), "main tab still registered");
        assert!(tabs.contains_key("S2"), "new tab registered by observer");

        drop(tabs);
        conn.shutdown();
    }

    // ----- Browser::new_tab + tabs + tab_count (P4 T3) -------------------

    /// End-to-end mock-drive of [`Browser::new_tab`]: send `Target.createTarget`,
    /// emit the corresponding `Target.attachedToTarget`, and assert the
    /// returned [`Tab`] matches the target_id from the create response while
    /// [`Browser::tabs`] grows to 2 entries.
    #[tokio::test]
    async fn new_tab_creates_target_then_returns_registered_tab() {
        use zendriver_transport::testing::MockConnection;

        let input_profile = zendriver_stealth::InputProfile::native();
        let registrar = Arc::new(TabRegistrar::new(input_profile.clone()));
        let (mut mock, conn) =
            MockConnection::pair_with_observers(vec![registrar.clone() as Arc<dyn TargetObserver>]);

        // Same launch-step-12 shape as the T2 test: synthetic main tab seeded
        // under S1/T1 so the registry starts at len=1.
        let inner = Arc::new_cyclic(|weak: &Weak<BrowserInner>| {
            let main_session = SessionHandle::new(conn.clone(), "S1");
            let main_input = InputController::new(input_profile.clone());
            let main_tab = Tab::new(main_session, weak.clone(), main_input, "T1".to_string());
            let mut map = HashMap::new();
            map.insert("S1".to_string(), main_tab.clone());
            BrowserInner {
                conn: conn.clone(),
                main_tab,
                child: tokio::sync::Mutex::new(None),
                _user_data: None,
                stealth_input_profile: input_profile.clone(),
                tabs: tokio::sync::RwLock::new(map),
            }
        });
        registrar.set_browser(Arc::downgrade(&inner));
        let browser = Browser {
            inner: inner.clone(),
        };

        // Drive `Browser::new_tab` from a spawned task so we can satisfy
        // both the `Target.createTarget` reply AND the
        // `Runtime.runIfWaitingForDebugger` reply from this thread.
        let fut = tokio::spawn({
            let b = browser.clone();
            async move { b.new_tab().await }
        });

        // Satisfy Target.createTarget with the targetId we will use in the
        // attach event below.
        let create_id = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            mock.expect_cmd("Target.createTarget"),
        )
        .await
        .expect("Target.createTarget not sent within 2s");
        assert_eq!(mock.last_sent()["params"]["url"], "about:blank");
        mock.reply(create_id, json!({ "targetId": "T2" })).await;

        // Emit the attach event for the new target — this fires the
        // TabRegistrar observer which inserts T2 into `inner.tabs`.
        mock.emit_event(
            "Target.attachedToTarget",
            json!({
                "sessionId": "S2",
                "targetInfo": {
                    "targetId": "T2",
                    "type": "page",
                    "url": "about:blank",
                    "attached": true,
                },
                "waitingForDebugger": true,
            }),
        )
        .await;

        // Actor releases the debugger after the observer returns Ok.
        let release_id = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            mock.expect_cmd("Runtime.runIfWaitingForDebugger"),
        )
        .await
        .expect("debugger-release did not fire within 2s");
        mock.reply(release_id, json!({})).await;

        // `Browser::new_tab` polls every 50ms — give it up to 2s wall-clock
        // to observe the registrar insertion. In practice it lands on the
        // first or second poll.
        let new_tab = tokio::time::timeout(std::time::Duration::from_secs(2), fut)
            .await
            .expect("new_tab future did not resolve within 2s")
            .expect("spawned task panicked")
            .expect("new_tab returned Err");

        assert_eq!(new_tab.target_id(), "T2");
        assert_eq!(browser.tab_count().await, 2);
        let all = browser.tabs().await;
        assert_eq!(all.len(), 2);
        let target_ids: std::collections::HashSet<_> =
            all.iter().map(|t| t.target_id().to_string()).collect();
        assert!(target_ids.contains("T1"));
        assert!(target_ids.contains("T2"));

        conn.shutdown();
    }

    /// Mock-drive a `Target.detachedFromTarget` event with the second tab's
    /// `sessionId` and assert the [`TabRegistrar::on_target_detached`]
    /// handler removes it from the browser-wide tabs registry. Counterpart
    /// to the attach-event test above; together they cover the registry's
    /// full lifecycle wired through actor → observer.
    #[tokio::test]
    async fn tab_registrar_removes_session_on_detached_event() {
        use zendriver_transport::testing::MockConnection;

        let input_profile = zendriver_stealth::InputProfile::native();
        let registrar = Arc::new(TabRegistrar::new(input_profile.clone()));
        let (mock, conn) =
            MockConnection::pair_with_observers(vec![registrar.clone() as Arc<dyn TargetObserver>]);

        // Seed `BrowserInner` with two tabs (S1/T1 = main, S2/T2 = extra).
        let inner = Arc::new_cyclic(|weak: &Weak<BrowserInner>| {
            let main_session = SessionHandle::new(conn.clone(), "S1");
            let main_input = InputController::new(input_profile.clone());
            let main_tab = Tab::new(main_session, weak.clone(), main_input, "T1".to_string());
            let extra_session = SessionHandle::new(conn.clone(), "S2");
            let extra_input = InputController::new(input_profile.clone());
            let extra_tab = Tab::new(extra_session, weak.clone(), extra_input, "T2".to_string());
            let mut map = HashMap::new();
            map.insert("S1".to_string(), main_tab.clone());
            map.insert("S2".to_string(), extra_tab);
            BrowserInner {
                conn: conn.clone(),
                main_tab,
                child: tokio::sync::Mutex::new(None),
                _user_data: None,
                stealth_input_profile: input_profile.clone(),
                tabs: tokio::sync::RwLock::new(map),
            }
        });
        registrar.set_browser(Arc::downgrade(&inner));

        assert_eq!(inner.tabs.read().await.len(), 2);

        // The actor dispatches `on_target_detached` from a background
        // `tokio::spawn`, so emit the event then poll until the registry
        // shrinks (same pattern as the attach test above).
        mock.emit_event(
            "Target.detachedFromTarget",
            json!({ "sessionId": "S2", "targetId": "T2" }),
        )
        .await;

        for _ in 0..50 {
            if inner.tabs.read().await.len() == 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let tabs = inner.tabs.read().await;
        assert_eq!(tabs.len(), 1, "expected S2 to be removed from registry");
        assert!(tabs.contains_key("S1"), "main tab still registered");
        assert!(!tabs.contains_key("S2"), "detached tab removed");

        drop(tabs);
        conn.shutdown();
    }

    // ----- Browser::cookies (P4 T10) -------------------------------------

    /// [`Browser::cookies`] returns a [`crate::CookieJar`] bound to the
    /// browser's root [`zendriver_transport::Connection`]. A `.set(...)` call
    /// must dispatch `Network.setCookie` on that connection — confirming the
    /// jar shares the browser's CDP channel (not a per-tab session channel).
    #[tokio::test]
    async fn browser_cookies_returns_jar_bound_to_browser_connection() {
        use crate::cookies::Cookie;
        use zendriver_transport::testing::MockConnection;

        let input_profile = zendriver_stealth::InputProfile::native();
        let (mut mock, conn) = MockConnection::pair();

        let inner = Arc::new_cyclic(|weak: &Weak<BrowserInner>| {
            let main_session = SessionHandle::new(conn.clone(), "S1");
            let main_input = InputController::new(input_profile.clone());
            let main_tab = Tab::new(main_session, weak.clone(), main_input, "T1".to_string());
            let mut map = HashMap::new();
            map.insert("S1".to_string(), main_tab.clone());
            BrowserInner {
                conn: conn.clone(),
                main_tab,
                child: tokio::sync::Mutex::new(None),
                _user_data: None,
                stealth_input_profile: input_profile.clone(),
                tabs: tokio::sync::RwLock::new(map),
            }
        });
        let browser = Browser { inner };
        let jar = browser.cookies();

        let fut = tokio::spawn(async move {
            jar.set(Cookie {
                name: "sid".into(),
                value: "abc".into(),
                domain: ".example.com".into(),
                path: "/".into(),
                expires: None,
                http_only: false,
                secure: false,
                same_site: None,
                url: None,
            })
            .await
        });

        let id = mock.expect_cmd("Network.setCookie").await;
        assert_eq!(mock.last_sent()["params"]["name"], "sid");
        // Browser-scope command — no session_id.
        assert!(mock.last_sent().get("sessionId").is_none());
        mock.reply(id, json!({ "success": true })).await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    /// Smoke test for the empty-tabs case: [`Browser::tabs`] returns a
    /// (typically 1-entry) snapshot and [`Browser::tab_count`] agrees.
    #[tokio::test]
    async fn tabs_and_tab_count_agree_on_initial_state() {
        use zendriver_transport::testing::MockConnection;

        let input_profile = zendriver_stealth::InputProfile::native();
        let registrar = Arc::new(TabRegistrar::new(input_profile.clone()));
        let (_mock, conn) =
            MockConnection::pair_with_observers(vec![registrar.clone() as Arc<dyn TargetObserver>]);

        let inner = Arc::new_cyclic(|weak: &Weak<BrowserInner>| {
            let main_session = SessionHandle::new(conn.clone(), "S1");
            let main_input = InputController::new(input_profile.clone());
            let main_tab = Tab::new(main_session, weak.clone(), main_input, "T1".to_string());
            let mut map = HashMap::new();
            map.insert("S1".to_string(), main_tab.clone());
            BrowserInner {
                conn: conn.clone(),
                main_tab,
                child: tokio::sync::Mutex::new(None),
                _user_data: None,
                stealth_input_profile: input_profile.clone(),
                tabs: tokio::sync::RwLock::new(map),
            }
        });
        let browser = Browser { inner };

        assert_eq!(browser.tab_count().await, 1);
        let tabs = browser.tabs().await;
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[0].target_id(), "T1");

        conn.shutdown();
    }
}
