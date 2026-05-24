//! Browser lifecycle: executable discovery, subprocess spawn, WS attach,
//! graceful teardown.
//!
//! Entry point is [`Browser::builder`] — start there for any zendriver
//! workflow. The launched [`Browser`] owns the Chrome subprocess and the
//! transport actor; dropping it terminates Chrome. Spawn additional pages
//! via [`Browser::new_tab`] and reach the initial page via
//! [`Browser::main_tab`].
//!
//! ```no_run
//! # async fn ex() -> zendriver::Result<()> {
//! let browser = zendriver::Browser::builder()
//!     .headless(true)
//!     .launch().await?;
//! let tab = browser.main_tab();
//! tab.goto("https://example.com").await?;
//! browser.close().await?;
//! # Ok(()) }
//! ```

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
///
/// Returns the first path that exists. Checks PATH for the canonical
/// binaries (`google-chrome`, `chromium`, `chrome`) then platform-specific
/// install locations (macOS `/Applications`, Linux `/usr/bin`, Windows
/// `Program Files`).
///
/// # Errors
///
/// Returns [`BrowserError::ExecutableNotFound`] with the full list of
/// searched paths when no installation is found.
///
/// # Examples
///
/// ```no_run
/// match zendriver::browser::find_chrome_executable() {
///     Ok(p) => println!("found chrome at {}", p.display()),
///     Err(e) => eprintln!("no chrome installed: {e}"),
/// }
/// ```
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

/// Fluent builder for a [`Browser`] launch.
///
/// Start with [`Browser::builder`] (seeded with [`zendriver_stealth::StealthProfile::native`]),
/// chain configuration calls, terminate with [`BrowserBuilder::launch`].
///
/// # Examples
///
/// ```no_run
/// # async fn ex() -> zendriver::Result<()> {
/// let browser = zendriver::Browser::builder()
///     .headless(true)
///     .arg("--lang=en-US")
///     .launch().await?;
/// # browser.close().await?;
/// # Ok(()) }
/// ```
#[derive(Default, Clone)]
pub struct BrowserBuilder {
    pub(crate) headless: Option<bool>,
    pub(crate) executable: Option<PathBuf>,
    pub(crate) user_data_dir: Option<PathBuf>,
    pub(crate) downloads_dir: Option<PathBuf>,
    pub(crate) extra_args: Vec<String>,
    pub(crate) stealth: Option<StealthProfile>,
    pub(crate) extra_observers: Vec<Arc<dyn TargetObserver>>,
    /// Optional `(username, password)` for proxy / HTTP basic-auth handling.
    /// Only honored when the `interception` feature is enabled; when present
    /// at launch, an interception actor is spawned on the main tab session
    /// that auto-replies to `Fetch.authRequired`. See cdpdriver/zendriver#208.
    #[cfg(feature = "interception")]
    pub(crate) proxy_auth: Option<(String, String)>,
}

// Hand-rolled `Debug` because `Vec<Arc<dyn TargetObserver>>` doesn't derive
// (trait objects are intentionally not `Debug`-bounded). Renders the observers
// field as `<N observers>` so the rest of the builder stays inspectable.
impl std::fmt::Debug for BrowserBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("BrowserBuilder");
        s.field("headless", &self.headless)
            .field("executable", &self.executable)
            .field("user_data_dir", &self.user_data_dir)
            .field("downloads_dir", &self.downloads_dir)
            .field("extra_args", &self.extra_args)
            .field("stealth", &self.stealth)
            .field(
                "extra_observers",
                &format_args!("<{} observers>", self.extra_observers.len()),
            );
        #[cfg(feature = "interception")]
        s.field(
            "proxy_auth",
            &self
                .proxy_auth
                .as_ref()
                .map(|(u, _)| format!("Some({u:?}, <redacted>)"))
                .unwrap_or_else(|| "None".into()),
        );
        s.finish()
    }
}

impl BrowserBuilder {
    /// Builder seeded with the default [`StealthProfile::native`] profile.
    ///
    /// Pass `.stealth(StealthProfile::off())` to opt out, or
    /// `.stealth(StealthProfile::spoofed())` for the full anti-detection set.
    /// Equivalent to [`Browser::builder`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let builder = zendriver::BrowserBuilder::new();
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            stealth: Some(StealthProfile::native()),
            ..Self::default()
        }
    }

    /// Toggle headless mode (default: `true`).
    ///
    /// When `on`, Chrome runs with `--headless=new --disable-gpu`. Pass
    /// `false` to launch a visible window (useful for local debugging).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let builder = zendriver::Browser::builder().headless(false);
    /// ```
    #[must_use]
    pub fn headless(mut self, on: bool) -> Self {
        self.headless = Some(on);
        self
    }

    /// Override the Chrome executable path.
    ///
    /// When unset, [`find_chrome_executable`] discovers one at launch time.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let builder = zendriver::Browser::builder()
    ///     .executable("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome");
    /// ```
    #[must_use]
    pub fn executable(mut self, path: impl Into<PathBuf>) -> Self {
        self.executable = Some(path.into());
        self
    }

    /// Override the `--user-data-dir` for the launched Chrome instance.
    ///
    /// When unset, a fresh tempdir is created and cleaned up on
    /// [`Browser::close`] / drop.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let builder = zendriver::Browser::builder()
    ///     .user_data_dir("/tmp/zendriver-profile");
    /// ```
    #[must_use]
    pub fn user_data_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.user_data_dir = Some(path.into());
        self
    }

    /// Auto-respond to proxy / HTTP basic-auth challenges with
    /// `(username, password)`.
    ///
    /// At launch, an interception actor is spawned on the main tab session
    /// that sends `Fetch.enable { handleAuthRequests: true }` and answers
    /// every `Fetch.authRequired` event with `Fetch.continueWithAuth`
    /// carrying these credentials. Combine with `.arg("--proxy-server=...")`
    /// to drive Chrome through an authenticated upstream proxy without the
    /// extension-based workarounds the upstream Python project requires.
    ///
    /// Scope: applies to the main tab only — tabs opened later via
    /// [`Browser::new_tab`] do **not** inherit auth handling. For those,
    /// wire `tab.intercept().handle_auth(user, pass).start()` yourself.
    ///
    /// See cdpdriver/zendriver#208.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// let browser = zendriver::Browser::builder()
    ///     .arg("--proxy-server=http://my-proxy.example:3128")
    ///     .proxy_auth("user", "pass")
    ///     .launch().await?;
    /// # browser.close().await?;
    /// # Ok(()) }
    /// ```
    #[cfg(feature = "interception")]
    #[must_use]
    pub fn proxy_auth(mut self, user: impl Into<String>, pass: impl Into<String>) -> Self {
        self.proxy_auth = Some((user.into(), pass.into()));
        self
    }

    /// Direct file downloads to `path` instead of the OS default Downloads
    /// folder.
    ///
    /// When set, `launch` sends `Browser.setDownloadBehavior {behavior:"allow",
    /// downloadPath}` at browser scope after Chrome is ready, so every tab —
    /// including new tabs opened later — saves files into `path`. The directory
    /// is **not** created for you; ensure it exists before launching.
    ///
    /// See <https://github.com/cdpdriver/zendriver/issues/88>.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// let browser = zendriver::Browser::builder()
    ///     .downloads_dir("/tmp/zendriver-downloads")
    ///     .launch().await?;
    /// # browser.close().await?;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn downloads_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.downloads_dir = Some(path.into());
        self
    }

    /// Append a single command-line flag to the Chrome launch argv.
    ///
    /// Flags accumulate; later calls do NOT replace earlier ones.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let builder = zendriver::Browser::builder()
    ///     .arg("--proxy-server=http://localhost:8080")
    ///     .arg("--lang=en-US");
    /// ```
    #[must_use]
    pub fn arg(mut self, flag: impl Into<String>) -> Self {
        self.extra_args.push(flag.into());
        self
    }

    /// Append multiple command-line flags to the Chrome launch argv.
    ///
    /// Equivalent to calling [`BrowserBuilder::arg`] for each entry.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let builder = zendriver::Browser::builder().args([
    ///     "--lang=en-US".to_string(),
    ///     "--window-size=1280,800".to_string(),
    /// ]);
    /// ```
    #[must_use]
    pub fn args(mut self, flags: impl IntoIterator<Item = String>) -> Self {
        self.extra_args.extend(flags);
        self
    }

    /// Override the default [`StealthProfile::native`] profile.
    ///
    /// Pass [`StealthProfile::off`](zendriver_stealth::StealthProfile::off) to
    /// disable stealth entirely or
    /// [`StealthProfile::spoofed`](zendriver_stealth::StealthProfile::spoofed)
    /// for the full anti-detection set.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zendriver::stealth::StealthProfile;
    /// let builder = zendriver::Browser::builder().stealth(StealthProfile::spoofed());
    /// ```
    #[must_use]
    pub fn stealth(mut self, profile: StealthProfile) -> Self {
        self.stealth = Some(profile);
        self
    }

    /// Register an additional [`TargetObserver`].
    ///
    /// Observers fire on each new attached page target. The stealth observer
    /// (if any) is added before user observers; user observers run in the
    /// order they were registered.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::sync::Arc;
    /// # use zendriver_transport::TargetObserver;
    /// # fn ex(my_obs: Arc<dyn TargetObserver>) {
    /// let builder = zendriver::Browser::builder().observer(my_obs);
    /// # }
    /// ```
    #[must_use]
    pub fn observer(mut self, obs: Arc<dyn TargetObserver>) -> Self {
        self.extra_observers.push(obs);
        self
    }

    /// Compute the full argv that would be passed to Chrome. Exposed to
    /// tests + snapshots; called internally by `launch`.
    pub(crate) fn build_flags(&self, user_data_dir: &Path) -> Vec<String> {
        let mut v = Vec::with_capacity(10 + self.extra_args.len());
        v.push("--remote-debugging-port=0".to_string());
        v.push(format!("--user-data-dir={}", user_data_dir.display()));
        v.push("--no-first-run".to_string());
        v.push("--no-default-browser-check".to_string());
        // Suppress the Chrome "Save password?" / autofill bubbles + onboarding
        // popups that otherwise hijack focus inside automated runs.
        // See cdpdriver/zendriver#13.
        v.push("--password-store=basic".to_string());
        v.push("--disable-save-password-bubble".to_string());
        v.push("--disable-features=PasswordManagerOnboarding,AutofillServerCommunication".to_string());
        if self.headless.unwrap_or(true) {
            v.push("--headless=new".to_string());
            v.push("--disable-gpu".to_string());
        }
        v.extend(self.extra_args.iter().cloned());
        v
    }
}

/// A running Chrome instance under zendriver control.
///
/// `Browser` is `Clone` (cheap — wraps an `Arc`) and `Send + Sync`, so the
/// same handle can be passed across `tokio::spawn` boundaries. Dropping
/// the last clone shuts down the transport and terminates Chrome via
/// `kill_on_drop` (for a graceful SIGTERM-then-SIGKILL teardown call
/// [`Browser::close`] explicitly).
///
/// Build one via [`Browser::builder`].
#[derive(Clone, Debug)]
pub struct Browser {
    pub(crate) inner: Arc<BrowserInner>,
}

#[derive(Debug)]
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
    /// Fires every time the [`TabRegistrar`] observer mutates [`Self::tabs`]
    /// (insert on attach, remove on detach). [`Browser::new_tab_at`] waits
    /// on this in lieu of the previous 50ms polling loop — it arms the
    /// notification before reading the map so a fire that lands between
    /// the read and the wait is still delivered.
    pub(crate) tabs_changed: tokio::sync::Notify,
    /// Optional RAII guard for the proxy-auth interception actor spawned in
    /// [`BrowserBuilder::launch`] when `proxy_auth` is set. Held here so
    /// the actor lives for the entire `Browser` lifetime; on `Browser` drop
    /// the handle drops and cancels the actor cleanly. See
    /// cdpdriver/zendriver#208.
    #[cfg(feature = "interception")]
    #[allow(dead_code)]
    pub(crate) proxy_auth_handle:
        std::sync::OnceLock<zendriver_interception::InterceptHandle>,
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

        match session.target_info.kind.as_str() {
            "iframe" => {
                // Out-of-process iframe: register a Frame under the parent
                // tab's frames map. The OOPIF carries a distinct child
                // session; the helper resolves the parent tab by walking
                // the tabs registry for a matching frame_id (preferring
                // `opener_frame_id` when present, falling back to
                // `target_id`).
                let conn = session.connection().clone();
                let new_session = SessionHandle::new(conn, session.session_id.to_string());
                crate::frame::oopif::register_oopif_frame(
                    &browser,
                    session.target_info,
                    new_session,
                )
                .await;
                Ok(())
            }
            "page" => {
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
                // Wake any `new_tab_at` callers waiting on this insert.
                browser.tabs_changed.notify_waiters();
                Ok(())
            }
            _ => {
                // Workers / service workers / etc — out of scope for the
                // current registrar; ignored silently. Future P4 tasks
                // may add explicit handling.
                Ok(())
            }
        }
    }

    async fn on_target_detached(&self, session_id: &str) {
        let Some(weak) = self.browser.get() else {
            return;
        };
        let Some(browser) = weak.upgrade() else {
            return;
        };
        // Tab path first — if the detached session backs a Tab, remove it.
        // If it was an OOPIF (no matching tab), fall through to the OOPIF
        // sweep which walks each tab's frames map.
        let removed_tab = browser.tabs.write().await.remove(session_id).is_some();
        if removed_tab {
            // Mirror the insert-side notify so any future watchers (e.g.
            // a `wait_for_tab_count` helper) can listen on the same channel.
            browser.tabs_changed.notify_waiters();
        } else {
            let _ = crate::frame::oopif::deregister_oopif_frame(&browser, session_id).await;
        }
    }
}

const WS_ENDPOINT_TIMEOUT: Duration = Duration::from_secs(15);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

impl BrowserBuilder {
    /// Spawn Chrome and attach. Returns once the main tab is bound.
    ///
    /// When a [`StealthProfile`] is set (the default), this:
    /// 1. Resolves a [`zendriver_stealth::Fingerprint`] from the resolved
    ///    Chrome executable.
    /// 2. Prepends the profile's [`StealthObserver`] to the observer chain.
    /// 3. Appends the profile's stealth flags to the launch argv.
    /// 4. Sends `Target.setAutoAttach { waitForDebuggerOnStart: true }` at
    ///    browser scope so the actor can route pauses through observers
    ///    before any page script runs.
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::Browser`] when Chrome can't be discovered
    /// or spawned, [`ZendriverError::Stealth`] when fingerprint resolution
    /// fails, [`ZendriverError::Transport`] when the WebSocket attach times
    /// out.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// let browser = zendriver::Browser::builder().launch().await?;
    /// # browser.close().await?;
    /// # Ok(()) }
    /// ```
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
                tabs_changed: tokio::sync::Notify::new(),
                #[cfg(feature = "interception")]
                proxy_auth_handle: std::sync::OnceLock::new(),
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

        // 13. If a custom downloads_dir was set, configure browser-scoped
        // download behavior so all tabs (current + future) save into it.
        // See cdpdriver/zendriver#88.
        if let Some(dir) = self.downloads_dir.as_ref() {
            inner
                .conn
                .call_raw(
                    "Browser.setDownloadBehavior",
                    json!({
                        "behavior": "allow",
                        "downloadPath": dir.display().to_string(),
                        "eventsEnabled": true,
                    }),
                    None,
                )
                .await?;
        }

        // 14. If proxy_auth was set, spawn an interception actor on the main
        // tab session that auto-answers Fetch.authRequired challenges with
        // the stored credentials. The InterceptHandle is parked on
        // BrowserInner so the actor lives as long as the Browser does;
        // dropping BrowserInner drops the handle which cancels the actor.
        // See cdpdriver/zendriver#208.
        #[cfg(feature = "interception")]
        if let Some((user, pass)) = self.proxy_auth.clone() {
            let main_session = inner.main_tab.session().clone();
            let handle = zendriver_interception::InterceptBuilder::new(&main_session)
                .handle_auth(user, pass)
                .start();
            let _ = inner.proxy_auth_handle.set(handle);
        }

        Ok(Browser { inner })
    }
}

#[cfg(feature = "fetcher")]
impl BrowserBuilder {
    /// Ensure Chrome is downloaded + cached, then use its path.
    ///
    /// The default [`zendriver_fetcher::Fetcher`] resolves the latest stable
    /// Chrome for Testing build for the host platform and caches it under
    /// the OS-conventional cache dir. For custom version pinning, channel
    /// selection, or cache placement, build a [`zendriver_fetcher::Fetcher`]
    /// directly and call `.executable(path)` yourself.
    ///
    /// Gated by the `fetcher` cargo feature.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// let browser = zendriver::Browser::builder()
    ///     .ensure_chrome().await?
    ///     .launch().await?;
    /// # browser.close().await?;
    /// # Ok(()) }
    /// ```
    pub async fn ensure_chrome(self) -> Result<Self, ZendriverError> {
        let path = zendriver_fetcher::Fetcher::new().ensure_chrome().await?;
        Ok(self.executable(path))
    }
}

impl Browser {
    /// Construct a fresh [`BrowserBuilder`] (with stealth on by default).
    ///
    /// Equivalent to [`BrowserBuilder::new`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// let browser = zendriver::Browser::builder().launch().await?;
    /// # browser.close().await?;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn builder() -> BrowserBuilder {
        BrowserBuilder::new()
    }

    /// Handle for the tab that exists when Chrome launches.
    ///
    /// The main tab is registered eagerly at launch time and is the same
    /// [`Tab`] across every clone of this [`Browser`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// let tab = browser.main_tab();
    /// tab.goto("https://example.com").await?;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn main_tab(&self) -> Tab {
        self.inner.main_tab.clone()
    }

    /// Raw root [`Connection`] for browser-scope CDP commands.
    ///
    /// Escape hatch for advanced users who need to send CDP commands at
    /// browser scope (no `sessionId`) that the high-level API doesn't wrap.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// let conn = browser.cdp();
    /// let info = conn.call_raw("SystemInfo.getInfo", serde_json::json!({}), None).await?;
    /// println!("{info}");
    /// # Ok(()) }
    /// ```
    #[must_use]
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

    /// Open a new tab navigated to `about:blank`.
    ///
    /// Returns once an internal tab registrar has registered the new [`Tab`]
    /// in the browser's tab registry — typically within a few milliseconds
    /// of `Target.createTarget`'s response.
    ///
    /// Internally:
    /// 1. Sends `Target.createTarget { url: "about:blank" }` at browser
    ///    scope (no session_id) — the response includes the new `targetId`.
    /// 2. Polls the internal tabs registry every 50ms for up to 5s, looking
    ///    for a [`Tab`] whose [`Tab::target_id`] matches. The tab registrar
    ///    populates that entry asynchronously when the
    ///    `Target.attachedToTarget` event arrives.
    /// 3. Returns the matching [`Tab`] on success.
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::TabNotFound`] if the registrar fails to
    /// register the new tab within the 5s window — usually a sign that
    /// auto-attach is misconfigured or the registrar observer crashed.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// let browser = zendriver::Browser::builder().launch().await?;
    /// let tab = browser.new_tab().await?;
    /// tab.goto("https://example.com").await?;
    /// # Ok(()) }
    /// ```
    pub async fn new_tab(&self) -> Result<Tab, ZendriverError> {
        self.new_tab_at("about:blank").await
    }

    /// Open a new tab navigated to `url`.
    ///
    /// Behaves identically to [`Browser::new_tab`] but with a custom initial
    /// URL passed to `Target.createTarget`. The returned [`Tab`] handle is
    /// ready as soon as the internal tab registrar observer registers it;
    /// callers can issue `.wait_for_load()` if they need to block on the
    /// navigation.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// let tab = browser.new_tab_at("https://example.com").await?;
    /// tab.wait_for_load().await?;
    /// # Ok(()) }
    /// ```
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

        // Wait for the TabRegistrar observer to insert the new Tab.
        // The 5s outer bound covers the typical CDP roundtrip + observer
        // chain latency (stealth → tab-registrar) with comfortable
        // headroom; instead of polling, we wait on
        // `BrowserInner::tabs_changed`, which the registrar fires on every
        // tabs-map mutation. The notification is `enable()`d before each
        // read so a notify that lands between the read and the wait is
        // still delivered.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let notif = self.inner.tabs_changed.notified();
            tokio::pin!(notif);
            notif.as_mut().enable();

            {
                let tabs = self.inner.tabs.read().await;
                if let Some(tab) = tabs.values().find(|t| t.target_id() == target_id) {
                    return Ok(tab.clone());
                }
            }

            if tokio::time::Instant::now() >= deadline {
                return Err(ZendriverError::TabNotFound(target_id));
            }

            tokio::select! {
                () = notif => {}
                () = tokio::time::sleep_until(deadline) => {}
            }
        }
    }

    /// Snapshot of all currently-registered tabs.
    ///
    /// Order is unspecified (the registry is a [`HashMap`] keyed by
    /// `sessionId`). Includes the main tab plus any tabs opened via
    /// [`Browser::new_tab`] or by page scripts (e.g. `window.open`) that
    /// auto-attach has wired into the registrar.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// for tab in browser.tabs().await {
    ///     println!("tab {}: {}", tab.target_id(), tab.url().await?);
    /// }
    /// # Ok(()) }
    /// ```
    pub async fn tabs(&self) -> Vec<Tab> {
        self.inner.tabs.read().await.values().cloned().collect()
    }

    /// Count of currently-registered tabs.
    ///
    /// Equivalent to `self.tabs().await.len()` but avoids the `Vec`
    /// allocation.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// assert_eq!(browser.tab_count().await, 1);
    /// # Ok(()) }
    /// ```
    pub async fn tab_count(&self) -> usize {
        self.inner.tabs.read().await.len()
    }

    /// Graceful shutdown of the Chrome subprocess.
    ///
    /// Cancels the transport, sends SIGTERM to Chrome, waits up to 5s, then
    /// SIGKILLs on timeout. Cleans up the `user_data_dir` tempdir if one was
    /// allocated at launch time.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// let browser = zendriver::Browser::builder().launch().await?;
    /// // ... drive the browser ...
    /// browser.close().await?;
    /// # Ok(()) }
    /// ```
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

/// Hard-shutdown fallback. `Drop` cannot be async, so it cannot perform the
/// SIGTERM-then-wait-then-SIGKILL dance [`Browser::close`] runs. Instead:
///
/// 1. [`Connection::shutdown`] signals the transport actor to stop reading;
///    pending CDP calls fail with a transport error.
/// 2. The child [`std::process::Child`] is dropped via tokio's
///    `kill_on_drop(true)` (set at spawn time), which sends `SIGKILL`
///    immediately — Chrome gets no grace period to flush state.
/// 3. The optional `user_data_dir` [`TempDir`] is dropped, deleting the
///    profile.
///
/// In short: dropping the [`Browser`] is the panic-safety / scope-exit path.
/// For a graceful shutdown that flushes Chrome state cleanly, call
/// [`Browser::close`] explicitly before the [`Browser`] goes out of scope.
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
    fn build_flags_suppresses_password_popups() {
        let b = BrowserBuilder::new();
        let flags = b.build_flags(Path::new("/tmp/x"));
        assert!(flags.contains(&"--password-store=basic".to_string()));
        assert!(flags.contains(&"--disable-save-password-bubble".to_string()));
        assert!(flags
            .iter()
            .any(|f| f.contains("PasswordManagerOnboarding")));
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
                tabs_changed: tokio::sync::Notify::new(),
                #[cfg(feature = "interception")]
                proxy_auth_handle: std::sync::OnceLock::new(),
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
                tabs_changed: tokio::sync::Notify::new(),
                #[cfg(feature = "interception")]
                proxy_auth_handle: std::sync::OnceLock::new(),
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
                tabs_changed: tokio::sync::Notify::new(),
                #[cfg(feature = "interception")]
                proxy_auth_handle: std::sync::OnceLock::new(),
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
                tabs_changed: tokio::sync::Notify::new(),
                #[cfg(feature = "interception")]
                proxy_auth_handle: std::sync::OnceLock::new(),
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
                tabs_changed: tokio::sync::Notify::new(),
                #[cfg(feature = "interception")]
                proxy_auth_handle: std::sync::OnceLock::new(),
            }
        });
        let browser = Browser { inner };

        assert_eq!(browser.tab_count().await, 1);
        let tabs = browser.tabs().await;
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[0].target_id(), "T1");

        conn.shutdown();
    }

    // ----- OOPIF Frame attach (P4 T16) -----------------------------------

    /// Mock-drive a `Target.attachedToTarget` event with `type=iframe` and
    /// a `targetId` that matches an already-known frame_id in the parent
    /// tab's frames map. Asserts the [`TabRegistrar`] dispatches to the
    /// OOPIF branch (instead of the page branch) and replaces the parent's
    /// same-id frame entry with a [`crate::frame::Frame`] whose session is
    /// the OOPIF's distinct child session (`S2` in the fixture, not the
    /// parent tab's `S1`).
    ///
    /// The pre-seeded parent-frame entry simulates the
    /// `Page.frameAttached` event a real Chrome would emit on the parent's
    /// session before announcing the OOPIF target — `register_oopif_frame`
    /// uses that entry to locate the owning tab.
    #[tokio::test]
    async fn tab_registrar_attaches_oopif_frame_under_parent_tab() {
        use crate::frame::Frame;
        use zendriver_transport::testing::MockConnection;

        let input_profile = zendriver_stealth::InputProfile::native();
        let registrar = Arc::new(TabRegistrar::new(input_profile.clone()));
        let (mut mock, conn) =
            MockConnection::pair_with_observers(vec![registrar.clone() as Arc<dyn TargetObserver>]);

        // Seed BrowserInner with one parent tab.
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
                tabs_changed: tokio::sync::Notify::new(),
                #[cfg(feature = "interception")]
                proxy_auth_handle: std::sync::OnceLock::new(),
            }
        });
        registrar.set_browser(Arc::downgrade(&inner));

        // Pre-seed the parent tab's frames map after `Arc::new_cyclic`
        // resolves (we need the live `Weak<TabInner>` for the placeholder
        // Frame). Simulates `Page.frameAttached` having already registered
        // the host iframe under frame_id "F_OOPIF" before the OOPIF
        // target announces itself. The placeholder Frame shares the
        // parent tab's session so we can later assert that the post-
        // attach entry carries a DIFFERENT session.
        {
            let main_tab = inner.tabs.read().await.get("S1").cloned().unwrap();
            let parent_session = main_tab.session().clone();
            let placeholder = Frame::new(
                "F_OOPIF".to_string(),
                Some("FROOT".to_string()),
                String::new(),
                None,
                parent_session,
                Arc::downgrade(&main_tab.inner),
            );
            main_tab
                .inner
                .frames
                .write()
                .await
                .insert("F_OOPIF".to_string(), placeholder);
        }

        // Sanity: parent tab is the only tab, parent has the placeholder
        // entry whose session matches the parent's "S1".
        assert_eq!(inner.tabs.read().await.len(), 1);
        let parent_tab = inner.tabs.read().await.get("S1").cloned().unwrap();
        {
            let frames = parent_tab.inner.frames.read().await;
            let placeholder = frames.get("F_OOPIF").expect("placeholder seeded");
            assert_eq!(placeholder.session().session_id(), "S1");
        }

        // Emit the OOPIF attach event. The actor will dispatch the
        // registrar's `on_target_attached` which routes to the iframe
        // branch; once it returns Ok the actor releases the debugger.
        mock.emit_event(
            "Target.attachedToTarget",
            json!({
                "sessionId": "S2",
                "targetInfo": {
                    "targetId": "F_OOPIF",
                    "type": "iframe",
                    "url": "https://oopif.example.com/",
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

        // Poll until the replacement lands.
        for _ in 0..50 {
            let frames = parent_tab.inner.frames.read().await;
            if frames
                .get("F_OOPIF")
                .is_some_and(|f| f.session().session_id() == "S2")
            {
                break;
            }
            drop(frames);
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        // Parent tab's frames map still holds an F_OOPIF entry — but now
        // bound to S2 (the OOPIF's distinct child session). Browser-wide
        // tabs map is unchanged (OOPIFs do NOT become new tabs).
        let frames = parent_tab.inner.frames.read().await;
        let oopif = frames
            .get("F_OOPIF")
            .expect("OOPIF frame registered on parent");
        assert_eq!(
            oopif.session().session_id(),
            "S2",
            "OOPIF frame must carry the child session, not the parent's",
        );
        assert_eq!(oopif.id(), "F_OOPIF");
        drop(frames);
        assert_eq!(
            inner.tabs.read().await.len(),
            1,
            "OOPIF must not be registered as a tab",
        );

        conn.shutdown();
    }

    /// Mock-drive a `Target.attachedToTarget` event with `type=iframe` whose
    /// `targetId` does NOT match any frame in any tab. Asserts the
    /// registrar logs + skips registration (no crash, no spurious entry).
    #[tokio::test]
    async fn tab_registrar_skips_oopif_when_parent_unknown() {
        use zendriver_transport::testing::MockConnection;

        let input_profile = zendriver_stealth::InputProfile::native();
        let registrar = Arc::new(TabRegistrar::new(input_profile.clone()));
        let (mut mock, conn) =
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
                tabs_changed: tokio::sync::Notify::new(),
                #[cfg(feature = "interception")]
                proxy_auth_handle: std::sync::OnceLock::new(),
            }
        });
        registrar.set_browser(Arc::downgrade(&inner));

        mock.emit_event(
            "Target.attachedToTarget",
            json!({
                "sessionId": "S_ORPHAN",
                "targetInfo": {
                    "targetId": "F_NOWHERE",
                    "type": "iframe",
                    "url": "https://orphan.example.com/",
                    "attached": true,
                },
                "waitingForDebugger": true,
            }),
        )
        .await;

        // Observer must still complete + release the debugger even when
        // the orphan branch warns and skips. Without this Chrome would
        // hang the OOPIF target indefinitely.
        let release_id = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            mock.expect_cmd("Runtime.runIfWaitingForDebugger"),
        )
        .await
        .expect("debugger-release did not fire within 2s");
        mock.reply(release_id, json!({})).await;

        // Browser-wide tabs registry is unchanged; the parent tab's
        // frames map is still empty (no placeholder was seeded).
        assert_eq!(inner.tabs.read().await.len(), 1);
        let parent_tab = inner.tabs.read().await.get("S1").cloned().unwrap();
        assert!(parent_tab.inner.frames.read().await.is_empty());

        conn.shutdown();
    }
}
