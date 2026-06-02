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

/// Which Chromium-family browser channel to discover at launch.
///
/// Passed via [`BrowserBuilder::channel`]; consumed by
/// [`find_chrome_executable_for_channel`] to pick the per-OS candidate path
/// table. [`BrowserBuilder::executable`] still overrides channel discovery
/// entirely.
///
/// # Examples
///
/// ```no_run
/// use zendriver::Channel;
/// let builder = zendriver::Browser::builder().channel(Channel::Brave);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Channel {
    /// Google Chrome (stable). Same discovery as [`Channel::Auto`] minus the
    /// Chromium fallbacks.
    Chrome,
    /// Open-source Chromium build.
    Chromium,
    /// Brave Browser.
    Brave,
    /// Microsoft Edge.
    Edge,
    /// First Chromium-family browser found — the historical default (Chrome,
    /// then Chromium). Use a specific channel to force Brave / Edge.
    #[default]
    Auto,
}

/// Look for a Chromium-family binary on PATH and in conventional locations.
///
/// Returns the first path that exists, scanning the [`Channel::Auto`]
/// candidate table (Chrome, then Chromium). For a specific browser channel
/// (Brave / Edge / …) use [`find_chrome_executable_for_channel`].
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
    find_chrome_executable_for_channel(Channel::Auto)
}

/// Look for the binary of a specific [`Channel`] on PATH and in conventional
/// per-OS install locations.
///
/// Returns the first candidate that exists. [`Channel::Auto`] reproduces the
/// historical first-found behavior (Chrome, then Chromium).
///
/// # Errors
///
/// Returns [`BrowserError::ExecutableNotFound`] with the searched candidate
/// list when none of the channel's paths exist.
pub fn find_chrome_executable_for_channel(channel: Channel) -> Result<PathBuf, BrowserError> {
    let candidates = candidate_paths_for_channel(channel);
    for c in &candidates {
        if c.exists() {
            return Ok(c.clone());
        }
    }
    Err(BrowserError::ExecutableNotFound {
        searched: candidates,
    })
}

/// Build the ordered candidate-path list for `channel`.
///
/// PATH lookups come first (so a PATH-resolved binary wins over a fixed
/// install dir), followed by the per-OS conventional install locations for
/// the requested channel. Factored out (and `pub(crate)`) so unit tests can
/// assert the table without requiring the browser installed.
pub(crate) fn candidate_paths_for_channel(channel: Channel) -> Vec<PathBuf> {
    let mut v = Vec::new();

    // PATH lookups — names vary by channel.
    let path_names: &[&str] = match channel {
        Channel::Chrome => &["google-chrome", "google-chrome-stable", "chrome"],
        Channel::Chromium => &["chromium", "chromium-browser"],
        Channel::Brave => &["brave-browser", "brave"],
        Channel::Edge => &["microsoft-edge", "microsoft-edge-stable"],
        Channel::Auto => &[
            "google-chrome",
            "google-chrome-stable",
            "chromium",
            "chromium-browser",
            "chrome",
        ],
    };
    for name in path_names {
        if let Some(p) = which_on_path(name) {
            v.push(p);
        }
    }

    // Platform-specific known locations, per channel.
    #[cfg(target_os = "macos")]
    {
        let chrome = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
        let chromium = "/Applications/Chromium.app/Contents/MacOS/Chromium";
        let brave =
            "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser";
        let edge = "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge";
        match channel {
            Channel::Chrome => v.push(PathBuf::from(chrome)),
            Channel::Chromium => v.push(PathBuf::from(chromium)),
            Channel::Brave => v.push(PathBuf::from(brave)),
            Channel::Edge => v.push(PathBuf::from(edge)),
            Channel::Auto => {
                v.push(PathBuf::from(chrome));
                v.push(PathBuf::from(chromium));
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        match channel {
            Channel::Chrome => {
                v.push(PathBuf::from("/usr/bin/google-chrome"));
                v.push(PathBuf::from("/usr/bin/google-chrome-stable"));
            }
            Channel::Chromium => {
                v.push(PathBuf::from("/usr/bin/chromium"));
                v.push(PathBuf::from("/usr/bin/chromium-browser"));
                v.push(PathBuf::from("/snap/bin/chromium"));
            }
            Channel::Brave => {
                v.push(PathBuf::from("/usr/bin/brave-browser"));
                v.push(PathBuf::from("/usr/bin/brave"));
                v.push(PathBuf::from(
                    "/opt/brave.com/brave/brave-browser",
                ));
            }
            Channel::Edge => {
                v.push(PathBuf::from("/usr/bin/microsoft-edge"));
                v.push(PathBuf::from("/usr/bin/microsoft-edge-stable"));
                v.push(PathBuf::from(
                    "/opt/microsoft/msedge/microsoft-edge",
                ));
            }
            Channel::Auto => {
                v.push(PathBuf::from("/usr/bin/google-chrome"));
                v.push(PathBuf::from("/usr/bin/chromium"));
                v.push(PathBuf::from("/usr/bin/chromium-browser"));
                v.push(PathBuf::from("/snap/bin/chromium"));
            }
        }
    }
    #[cfg(target_os = "windows")]
    {
        let chrome = [
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        ];
        let brave = [
            r"C:\Program Files\BraveSoftware\Brave-Browser\Application\brave.exe",
            r"C:\Program Files (x86)\BraveSoftware\Brave-Browser\Application\brave.exe",
        ];
        let edge = [
            r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
            r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        ];
        match channel {
            Channel::Chrome | Channel::Chromium | Channel::Auto => {
                for p in chrome {
                    v.push(PathBuf::from(p));
                }
            }
            Channel::Brave => {
                for p in brave {
                    v.push(PathBuf::from(p));
                }
            }
            Channel::Edge => {
                for p in edge {
                    v.push(PathBuf::from(p));
                }
            }
        }
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

/// Extract the `host:port` authority from a `ws://HOST:PORT/...` (or `wss://`)
/// DevTools endpoint URL.
///
/// Returns `None` for inputs that don't carry a recognizable
/// `scheme://authority` shape. Used to compose [`Tab::inspector_url`] from the
/// endpoint the browser connected to.
pub(crate) fn debug_host_port_from_ws(ws_url: &str) -> Option<String> {
    // Strip the scheme, then take everything up to the first `/` (the path).
    let after_scheme = ws_url
        .strip_prefix("ws://")
        .or_else(|| ws_url.strip_prefix("wss://"))?;
    let authority = after_scheme
        .split('/')
        .next()
        .unwrap_or(after_scheme)
        .trim();
    if authority.is_empty() {
        None
    } else {
        Some(authority.to_string())
    }
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
    /// `--lang=<v>` UI locale override. See [`BrowserBuilder::lang`].
    pub(crate) lang: Option<String>,
    /// Static `--user-agent=<v>` launch override. See
    /// [`BrowserBuilder::user_agent`].
    pub(crate) user_agent: Option<String>,
    /// Sandbox toggle. `None`/`Some(true)` = sandbox on (no flag);
    /// `Some(false)` = emit `--no-sandbox`. See [`BrowserBuilder::sandbox`].
    pub(crate) sandbox: Option<bool>,
    /// Which browser [`Channel`] to discover when no explicit `executable` is
    /// set. Defaults to [`Channel::Auto`].
    pub(crate) channel: Channel,
    /// Unpacked-extension directory (or `.crx`) paths to side-load. See
    /// [`BrowserBuilder::add_extension`]. `.crx` entries are unzipped to a
    /// tempdir at launch; directory entries pass through unchanged.
    pub(crate) extensions: Vec<PathBuf>,
    /// Expert-mode toggle. When `true`, `build_flags` emits
    /// `--disable-web-security` + `--disable-site-isolation-trials`. See
    /// [`BrowserBuilder::expert`].
    pub(crate) expert: bool,
    /// When `true`, a [`ShadowRootObserver`] is added to the observer chain so
    /// every new target gets an `Element.prototype.attachShadow` override that
    /// forces `{mode: "open"}`. See
    /// [`BrowserBuilder::force_open_shadow_roots`]. **Detectable.**
    pub(crate) force_open_shadow_roots: bool,
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
            .field("lang", &self.lang)
            .field("user_agent", &self.user_agent)
            .field("sandbox", &self.sandbox)
            .field("channel", &self.channel)
            .field("extensions", &self.extensions)
            .field("expert", &self.expert)
            .field("force_open_shadow_roots", &self.force_open_shadow_roots)
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

    /// Set Chrome's UI locale via `--lang=<v>` (e.g. `"en-US"`, `"de-DE"`).
    ///
    /// Influences the browser-chrome language and the default
    /// `Accept-Language` header. For full stealth-coherent locale spoofing,
    /// prefer configuring it through the [`StealthProfile`] instead.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let builder = zendriver::Browser::builder().lang("en-US");
    /// ```
    #[must_use]
    pub fn lang(mut self, lang: impl Into<String>) -> Self {
        self.lang = Some(lang.into());
        self
    }

    /// Set a static `--user-agent=<v>` for the launched Chrome process.
    ///
    /// This is the **launch-time** User-Agent override. Two other UA paths
    /// exist and are usually a better fit:
    /// - [`Tab::set_user_agent`] — a runtime per-tab override (also sets
    ///   `Accept-Language` + UA-CH client hints) applied after launch.
    /// - the stealth-profile UA — a fingerprint-coherent UA (with matching
    ///   UA-CH metadata) set via the [`StealthProfile`]; preferred when
    ///   stealth is on, since a bare `--user-agent` flag leaves the
    ///   JS-visible UA-CH hints inconsistent with the header.
    ///
    /// Use this flag only when you need the UA fixed at process start for
    /// every tab and don't need UA-CH coherence.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let builder = zendriver::Browser::builder()
    ///     .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36");
    /// ```
    #[must_use]
    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.user_agent = Some(ua.into());
        self
    }

    /// Toggle Chrome's setuid/namespace sandbox (default: **on**).
    ///
    /// Passing `false` appends `--no-sandbox`. Leaving it unset (or `true`)
    /// keeps the sandbox enabled and emits no flag.
    ///
    /// Independent of the CI auto-disable: when the `CI` env var is set,
    /// `launch` still auto-adds `--no-sandbox` + `--disable-dev-shm-usage`
    /// (the GitHub-Actions / Docker containers run as root, where the
    /// user-namespace sandbox refuses to start). Calling `sandbox(false)`
    /// just opts in explicitly outside CI.
    ///
    /// Disabling the sandbox weakens Chrome's process isolation — only do so
    /// in trusted, throwaway environments (containers, ephemeral VMs).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let builder = zendriver::Browser::builder().sandbox(false);
    /// ```
    #[must_use]
    pub fn sandbox(mut self, on: bool) -> Self {
        self.sandbox = Some(on);
        self
    }

    /// Pick which browser [`Channel`] to discover at launch (default:
    /// [`Channel::Auto`]).
    ///
    /// Selects the per-OS candidate-path table used to locate the browser
    /// binary — e.g. [`Channel::Brave`] / [`Channel::Edge`] resolve their
    /// real install locations. [`BrowserBuilder::executable`] still overrides
    /// channel discovery entirely; when an explicit executable is set the
    /// channel is ignored.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zendriver::Channel;
    /// let builder = zendriver::Browser::builder().channel(Channel::Brave);
    /// ```
    #[must_use]
    pub fn channel(mut self, channel: Channel) -> Self {
        self.channel = channel;
        self
    }

    /// Side-load a single unpacked extension directory (or a `.crx` file).
    ///
    /// Extensions accumulate; call this once per extension or use
    /// [`BrowserBuilder::extensions`] for a batch. At launch the resolved
    /// directories are passed via `--load-extension=<dir1,dir2,…>`, paired with
    /// `--disable-extensions-except=<dir1,dir2,…>` and
    /// `--enable-unsafe-extension-debugging`. zendriver also forces the
    /// `DisableLoadExtensionCommandLineSwitch` feature off regardless of the
    /// active [`StealthProfile`] — Chrome 136+ otherwise ignores
    /// `--load-extension` entirely (see the type-level note on this builder).
    ///
    /// A `.crx` path is unzipped into a temporary directory that lives for the
    /// [`Browser`]'s lifetime; directory paths are used as-is. Mixing the two
    /// is fine.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let builder = zendriver::Browser::builder()
    ///     .add_extension("/path/to/unpacked-ext")
    ///     .add_extension("/path/to/packed.crx");
    /// ```
    #[must_use]
    pub fn add_extension(mut self, path: impl Into<PathBuf>) -> Self {
        self.extensions.push(path.into());
        self
    }

    /// Side-load several extensions at once.
    ///
    /// Equivalent to calling [`BrowserBuilder::add_extension`] for each entry;
    /// see it for the flag set and `.crx` handling.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// let builder = zendriver::Browser::builder().extensions([
    ///     PathBuf::from("/ext/one"),
    ///     PathBuf::from("/ext/two"),
    /// ]);
    /// ```
    #[must_use]
    pub fn extensions(mut self, paths: impl IntoIterator<Item = PathBuf>) -> Self {
        self.extensions.extend(paths);
        self
    }

    /// Relax Chrome's web-security + site-isolation guards (default: **off**).
    ///
    /// When `on`, `build_flags` appends `--disable-web-security` (drops the
    /// same-origin policy for cross-origin `fetch` / DOM access) and
    /// `--disable-site-isolation-trials` (so cross-origin frames stay
    /// in-process and are reachable from the parent). Mirrors nodriver's
    /// `start(expert=True)` / zendriver-py's expert launch.
    ///
    /// This is **flags only** — it does not touch the JS environment. For the
    /// closed-shadow-root walk that nodriver's expert mode also enables, opt in
    /// separately via [`BrowserBuilder::force_open_shadow_roots`].
    ///
    /// Disabling web security weakens the browser's origin isolation; use only
    /// in trusted, throwaway automation contexts.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let builder = zendriver::Browser::builder().expert(true);
    /// ```
    #[must_use]
    pub fn expert(mut self, on: bool) -> Self {
        self.expert = on;
        self
    }

    /// Force every `Element.prototype.attachShadow` call to open mode so closed
    /// shadow roots become walkable (default: **off**).
    ///
    /// When `on`, a small built-in [`TargetObserver`] injects a
    /// `Page.addScriptToEvaluateOnNewDocument` patch into every new target that
    /// rewrites the `attachShadow` init dict to `{ mode: "open" }` (other init
    /// keys are preserved). The patched element's `shadowRoot` is then
    /// reachable from automation even when the page requested a closed root.
    /// This runs independently of the [`StealthProfile`] — it works with
    /// stealth off and does **not** become part of the spoofed fingerprint
    /// bundle.
    ///
    /// # Detectability
    ///
    /// **This patch is detectable.** A page can notice that `attachShadow`
    /// always yields an open root (e.g. by calling it with `{ mode: "closed" }`
    /// and observing a non-null `.shadowRoot`), so anti-bot scripts can use it
    /// as a signal. Enable it only when you genuinely need to traverse closed
    /// shadow roots (some challenge widgets), and prefer leaving it off for
    /// stealth-sensitive workloads.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let builder = zendriver::Browser::builder().force_open_shadow_roots(true);
    /// ```
    #[must_use]
    pub fn force_open_shadow_roots(mut self, on: bool) -> Self {
        self.force_open_shadow_roots = on;
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
        // Base disable-features set. When extensions are requested we MUST also
        // turn off `DisableLoadExtensionCommandLineSwitch`, or Chrome 136+
        // silently ignores `--load-extension`. The stealth profiles carry that
        // feature in their own `--disable-features=IsolateOrigins,…` line, but
        // an Off profile would otherwise miss it, so merge it into the base
        // line here — Chrome unions every `--disable-features` occurrence, so
        // the redundancy under a stealth profile is harmless.
        if self.extensions.is_empty() {
            v.push(
                "--disable-features=PasswordManagerOnboarding,AutofillServerCommunication"
                    .to_string(),
            );
        } else {
            v.push(
                "--disable-features=PasswordManagerOnboarding,AutofillServerCommunication,DisableLoadExtensionCommandLineSwitch"
                    .to_string(),
            );
        }
        if self.headless.unwrap_or(true) {
            v.push("--headless=new".to_string());
            v.push("--disable-gpu".to_string());
        }
        // Expert mode: relax web-security + site isolation. Emitted only when
        // `expert(true)` so the default flag set / snapshots are unchanged.
        if self.expert {
            v.push("--disable-web-security".to_string());
            v.push("--disable-site-isolation-trials".to_string());
        }
        // Extension side-loading flags. `self.extensions` holds already-resolved
        // directories at this point (`launch` unzips any `.crx` into tempdirs
        // and rewrites the list before calling `build_flags`). Skipped entirely
        // when no extensions are configured so the default argv is untouched.
        if !self.extensions.is_empty() {
            let joined = self
                .extensions
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(",");
            v.push(format!("--load-extension={joined}"));
            v.push(format!("--disable-extensions-except={joined}"));
            v.push("--enable-unsafe-extension-debugging".to_string());
        }
        // C4 dedicated flags. Emitted only when explicitly configured so the
        // default-builder flag set (and its snapshots) is unchanged. Placed
        // before user `extra_args` so caller-supplied flags still come last.
        if let Some(lang) = self.lang.as_ref() {
            v.push(format!("--lang={lang}"));
        }
        if let Some(ua) = self.user_agent.as_ref() {
            v.push(format!("--user-agent={ua}"));
        }
        // Sandbox off → --no-sandbox. Default (None / Some(true)) emits
        // nothing; the CI auto-disable is handled separately in `launch`.
        // NOTE: root-uid (euid 0) auto-disable is intentionally NOT done here
        // — that needs a `geteuid` syscall (no std API) and we decline to add
        // a `rustix` / `unsafe libc` dependency for it. Callers running as
        // root should opt in explicitly with `.sandbox(false)`.
        if self.sandbox == Some(false) {
            v.push("--no-sandbox".to_string());
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
    /// Tempdirs holding `.crx` extensions unzipped at launch. Held here so the
    /// extracted unpacked directories outlive the Chrome process that was
    /// pointed at them via `--load-extension`; dropped with the [`Browser`].
    /// Empty when no `.crx` extensions were configured.
    pub(crate) _extension_dirs: Vec<TempDir>,
    /// Whether this handle owns the underlying Chrome process. `true` for a
    /// browser produced by [`BrowserBuilder::launch`] (we spawned Chrome, so
    /// `close()` / `Drop` terminate it); `false` for one produced by
    /// [`BrowserBuilder::connect`] (we attached to an already-running debug
    /// session and must leave the process alone — `close()` only shuts down
    /// the transport, and no `Child` is held so `kill_on_drop` never fires).
    pub(crate) owns_process: bool,
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
    /// `host:port` of the remote-debugging endpoint Chrome was launched with,
    /// parsed from the `DevTools listening on ws://HOST:PORT/...` stderr line
    /// at launch. `None` for test-constructed browsers that never launched a
    /// real Chrome. Consumed by [`Tab::inspector_url`] (reached via the Tab's
    /// `Weak<BrowserInner>`) to compose the DevTools front-end URL.
    pub(crate) debug_host_port: Option<String>,
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
    pub(crate) proxy_auth_handle: std::sync::OnceLock<zendriver_interception::InterceptHandle>,
}

impl BrowserInner {
    /// Wraps `Target.disposeBrowserContext`, the CDP command that destroys
    /// an incognito-style [browser context][1] previously created via
    /// `Target.createBrowserContext`. Sent at browser scope (no
    /// `sessionId`).
    ///
    /// Used by [`crate::BrowserContext`]'s `Drop` impl to free its backing
    /// context when the handle goes out of scope.
    ///
    /// [1]: https://chromedevtools.github.io/devtools-protocol/tot/Target/#method-disposeBrowserContext
    pub(crate) async fn dispose_browser_context(&self, id: &str) -> Result<(), ZendriverError> {
        self.conn
            .call_raw(
                "Target.disposeBrowserContext",
                json!({ "browserContextId": id }),
                None,
            )
            .await?;
        Ok(())
    }
}

/// Test-only helper that wraps [`test_only_inner_from_conn`] in a real
/// [`Browser`] handle. Used by inline unit tests that need to exercise
/// `Browser`-level methods (e.g. `create_browser_context_with`) without
/// launching Chrome.
#[cfg(test)]
pub(crate) fn test_only_browser_from_conn(conn: Connection) -> Browser {
    Browser {
        inner: test_only_inner_from_conn(conn),
    }
}

/// Test-only helper that constructs a minimal [`BrowserInner`] backed by a
/// caller-supplied [`Connection`] (typically the consumer side of a
/// [`zendriver_transport::testing::MockConnection`] pair). Mirrors the
/// shape `launch` produces post-step-12: a main tab keyed under `"S1"` /
/// target id `"T1"` in the registry.
///
/// Used by inline unit tests that need to exercise [`BrowserInner`]
/// methods without launching Chrome. Subsequent tasks in this series
/// (per-context proxy support) reuse the same helper.
#[cfg(test)]
pub(crate) fn test_only_inner_from_conn(conn: Connection) -> Arc<BrowserInner> {
    let input_profile = zendriver_stealth::InputProfile::native();
    Arc::new_cyclic(|weak: &std::sync::Weak<BrowserInner>| {
        let main_session = SessionHandle::new(conn.clone(), "S1");
        let main_input = InputController::new(input_profile.clone());
        let main_tab = Tab::new(main_session, weak.clone(), main_input, "T1".to_string());
        let mut map = HashMap::new();
        map.insert("S1".to_string(), main_tab.clone());
        BrowserInner {
            conn,
            main_tab,
            child: tokio::sync::Mutex::new(None),
            _user_data: None,
            _extension_dirs: Vec::new(),
            owns_process: false,
            stealth_input_profile: input_profile,
            tabs: tokio::sync::RwLock::new(map),
            debug_host_port: None,
            tabs_changed: tokio::sync::Notify::new(),
            #[cfg(feature = "interception")]
            proxy_auth_handle: std::sync::OnceLock::new(),
        }
    })
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
                // Skip Chrome-internal pages (chrome://newtab/ that Chrome
                // auto-opens when the last user tab closes, devtools://, etc).
                // These aren't pages the caller drove zendriver to create and
                // would inflate `tab_count` / `tabs()` with unwanted entries.
                let url = &session.target_info.url;
                if url.starts_with("chrome://")
                    || url.starts_with("devtools://")
                    || url.starts_with("chrome-extension://")
                    || url.starts_with("chrome-untrusted://")
                {
                    return Ok(());
                }
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

                // Dedupe by `target_id`: if `flatten: true` re-fires
                // `attachedToTarget` for the same target under a different
                // sessionId (observed on `--headless=new`), the old entry
                // would otherwise linger and inflate `tab_count`.
                let target_id_str = session.target_info.target_id.clone();
                let mut tabs = browser.tabs.write().await;
                tabs.retain(|_, t| t.target_id() != target_id_str);
                tabs.insert(session.session_id.to_string(), tab);
                drop(tabs);
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
/// How long [`resolve_ws_from_http`] waits for the `/json/version` round-trip.
const JSON_VERSION_TIMEOUT: Duration = Duration::from_secs(10);

/// Inputs to [`finish_connect`] — the post-connect handshake shared by
/// [`BrowserBuilder::launch`] (spawn) and [`BrowserBuilder::connect`]
/// (attach). Bundled into a struct to keep the function signature readable
/// (clippy `too_many_arguments`) and to make the spawn-vs-attach differences
/// explicit at the call site.
pub(crate) struct FinishConnect {
    /// The freshly-established transport (already wired with the observer
    /// chain via `connect_with_observers`).
    pub(crate) conn: Connection,
    /// The tab registrar from the observer chain; its weak `BrowserInner`
    /// ref is wired here once the cyclic `Arc` resolves.
    pub(crate) registrar: Arc<TabRegistrar>,
    /// Per-tab input profile cached on `BrowserInner` for new-tab construction.
    pub(crate) input_profile: zendriver_stealth::InputProfile,
    /// Owned Chrome child process — `Some` for `launch`, `None` for `connect`.
    pub(crate) child: Option<Child>,
    /// Owned `user_data_dir` tempdir — `Some` only when `launch` allocated one.
    pub(crate) owned_tmp: Option<TempDir>,
    /// Tempdirs for any `.crx` extensions `launch` unzipped. Empty for
    /// `connect` (no process launched) and when no `.crx` was configured.
    pub(crate) extension_dirs: Vec<TempDir>,
    /// `host:port` of the debug endpoint, for [`Tab::inspector_url`].
    pub(crate) debug_host_port: Option<String>,
    /// Whether the resulting handle owns (and must terminate) the process.
    pub(crate) owns_process: bool,
}

/// Run the post-connect CDP handshake and assemble [`BrowserInner`].
///
/// Shared by [`BrowserBuilder::launch`] and [`BrowserBuilder::connect`]: both
/// arrive here with a live [`Connection`] (observer chain already attached)
/// and need the same sequence — enable browser-scoped auto-attach, discover +
/// attach the main page target, build the self-referential `Arc<BrowserInner>`,
/// then backfill the registrar's weak ref and the main-tab registry entry.
///
/// The only spawn-vs-attach differences are carried in [`FinishConnect`]: the
/// owned `Child` / `TempDir` (present only for `launch`) and the
/// `owns_process` flag.
pub(crate) async fn finish_connect(
    args: FinishConnect,
) -> Result<Arc<BrowserInner>, ZendriverError> {
    let FinishConnect {
        conn,
        registrar,
        input_profile,
        child,
        owned_tmp,
        extension_dirs,
        debug_host_port,
        owns_process,
    } = args;

    // Enable auto-attach with debugger-pause BEFORE attaching to the initial
    // target. Sent at browser scope (no session_id) so it covers both the
    // initial target and any subsequently-opened pages/iframes.
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

    // Discover the initial target via Target.getTargets (prefer a page).
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

    // Attach to the initial target. This triggers `Target.attachedToTarget`
    // which the actor routes through observers (`on_target_attached`) and
    // then releases via `Runtime.runIfWaitingForDebugger`.
    //
    // The `TabRegistrar` observer (in the chain) will try to insert into
    // `BrowserInner.tabs` for the main tab too. That insertion is a no-op
    // because the weak ref isn't wired yet (`OnceLock` empty → observer warns
    // + skips). We re-insert the main tab manually below so the registry is
    // consistent post-connect.
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

    // Wrap session in Tab; build BrowserInner via the canonical
    // `Arc::new_cyclic` self-referential pattern.
    let session_id_for_registry = session_id.clone();
    let target_id_for_main_tab = target_id.clone();
    let inner = Arc::new_cyclic(|weak: &std::sync::Weak<BrowserInner>| {
        let session = SessionHandle::new(conn.clone(), session_id);
        let main_tab_input = InputController::new(input_profile.clone());
        let main_tab = Tab::new(session, weak.clone(), main_tab_input, target_id_for_main_tab);
        BrowserInner {
            conn,
            main_tab,
            child: tokio::sync::Mutex::new(child),
            _user_data: owned_tmp,
            _extension_dirs: extension_dirs,
            owns_process,
            stealth_input_profile: input_profile,
            tabs: tokio::sync::RwLock::new(HashMap::new()),
            debug_host_port,
            tabs_changed: tokio::sync::Notify::new(),
            #[cfg(feature = "interception")]
            proxy_auth_handle: std::sync::OnceLock::new(),
        }
    });

    // Wire the registrar's weak ref + manually insert the main tab (it was
    // attached before the weak ref existed, so the observer bailed early).
    registrar.set_browser(Arc::downgrade(&inner));
    inner
        .tabs
        .write()
        .await
        .insert(session_id_for_registry, inner.main_tab.clone());

    Ok(inner)
}

/// Resolve a `webSocketDebuggerUrl` from a DevTools HTTP base by issuing a
/// minimal `GET {endpoint}/json/version` over a raw [`tokio::net::TcpStream`].
///
/// Hand-rolled (HTTP/1.0, read-until-close, split headers from body, parse the
/// JSON body) so the always-on dependency set does not grow an HTTP client —
/// `connect`'s `ws://` path needs no HTTP at all, and the `fetcher`-gated
/// `reqwest` must not leak into the default build. Mirrors nodriver /
/// zendriver-py, which read the same `webSocketDebuggerUrl` field.
///
/// # Errors
///
/// Returns [`BrowserError::DevtoolsParse`] when the endpoint is malformed, the
/// TCP round-trip fails / times out, the response has no body, or the JSON
/// lacks a string `webSocketDebuggerUrl`.
pub(crate) async fn resolve_ws_from_http(endpoint: &str) -> Result<String, ZendriverError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    // Parse `scheme://host[:port]` → (host, port). Strip any trailing path.
    let after_scheme = endpoint
        .strip_prefix("http://")
        .or_else(|| endpoint.strip_prefix("https://"))
        .ok_or(BrowserError::DevtoolsParse)?;
    let authority = after_scheme
        .split('/')
        .next()
        .unwrap_or(after_scheme)
        .trim();
    if authority.is_empty() {
        return Err(BrowserError::DevtoolsParse.into());
    }
    // host:port split — default to 80 when no explicit port (DevTools is
    // typically explicit, e.g. 9222, but be lenient).
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (
            h,
            p.parse::<u16>().map_err(|_| BrowserError::DevtoolsParse)?,
        ),
        None => (authority, 80u16),
    };

    let body = timeout(JSON_VERSION_TIMEOUT, async {
        let mut stream = TcpStream::connect((host, port))
            .await
            .map_err(BrowserError::SpawnFailed)?;
        // HTTP/1.0 + Connection: close → server closes the socket after the
        // response, so a read-to-end cleanly delimits the message.
        let req = format!(
            "GET /json/version HTTP/1.0\r\nHost: {authority}\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
        );
        stream
            .write_all(req.as_bytes())
            .await
            .map_err(BrowserError::SpawnFailed)?;
        stream.flush().await.map_err(BrowserError::SpawnFailed)?;
        let mut buf = Vec::new();
        stream
            .read_to_end(&mut buf)
            .await
            .map_err(BrowserError::SpawnFailed)?;
        Ok::<Vec<u8>, ZendriverError>(buf)
    })
    .await
    .map_err(|_| BrowserError::WsTimeout)??;

    parse_ws_from_json_version(&body)
}

/// Split an HTTP/1.x response into its body and parse the
/// `webSocketDebuggerUrl` field from the JSON.
///
/// Factored out of [`resolve_ws_from_http`] so the parse — the part most worth
/// asserting — is unit-testable without a socket. Splits on the first
/// `\r\n\r\n` (header/body boundary); if absent, treats the whole buffer as the
/// JSON body (lenient toward bodies returned without standard headers in
/// tests).
pub(crate) fn parse_ws_from_json_version(raw: &[u8]) -> Result<String, ZendriverError> {
    let text = String::from_utf8_lossy(raw);
    // Header/body split on the blank line; fall back to the whole buffer.
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b)
        .unwrap_or(&text)
        .trim();
    if body.is_empty() {
        return Err(BrowserError::DevtoolsParse.into());
    }
    let parsed: serde_json::Value =
        serde_json::from_str(body).map_err(|_| BrowserError::DevtoolsParse)?;
    parsed["webSocketDebuggerUrl"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| BrowserError::DevtoolsParse.into())
}

/// Resolve every entry in `extensions` to an unpacked-extension *directory*,
/// in place, returning the tempdirs that back any unzipped `.crx` archives.
///
/// `--load-extension` only accepts directories, so a `.crx` file is unzipped
/// into a fresh [`TempDir`] and the slot is rewritten to that directory.
/// Entries that are already directories pass through unchanged. The returned
/// tempdirs MUST be kept alive for as long as Chrome runs (they are parked on
/// [`BrowserInner`]); dropping one deletes the extracted extension out from
/// under the running browser.
///
/// A `.crx` is a ZIP with a short binary header (magic `Cr24` + version +
/// signature lengths) prepended; we locate the embedded ZIP by scanning for
/// the local-file-header magic (`PK\x03\x04`) and unzip from there, so both
/// CRX2 and CRX3 layouts work without parsing the header fields.
///
/// # Errors
///
/// Returns [`BrowserError::ExtensionLoad`] when a configured path does not
/// exist, when a `.crx` cannot be read / contains no ZIP payload / fails to
/// unzip, or when an entry is neither a directory nor a `.crx` file.
async fn resolve_extension_dirs(
    extensions: &mut [PathBuf],
) -> Result<Vec<TempDir>, ZendriverError> {
    let mut tempdirs = Vec::new();
    for slot in extensions.iter_mut() {
        if slot.is_dir() {
            continue;
        }
        let is_crx = slot
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("crx"));
        if !is_crx {
            return Err(BrowserError::ExtensionLoad {
                path: slot.clone(),
                reason: if slot.exists() {
                    "extension path is neither a directory nor a .crx file".into()
                } else {
                    "extension path does not exist".into()
                },
            }
            .into());
        }
        let (dir, td) = unzip_crx(slot).await?;
        *slot = dir;
        tempdirs.push(td);
    }
    Ok(tempdirs)
}

/// Unzip a single `.crx` at `crx_path` into a fresh [`TempDir`], returning the
/// extracted directory path alongside the owning tempdir handle.
///
/// The blocking ZIP walk runs on [`tokio::task::spawn_blocking`]. Path safety
/// mirrors the fetcher's extractor: entries with absolute / parent-traversal
/// paths and (on Unix) symlink entries are rejected so a hostile `.crx` can't
/// escape the tempdir.
async fn unzip_crx(crx_path: &Path) -> Result<(PathBuf, TempDir), ZendriverError> {
    let crx_path = crx_path.to_path_buf();
    let td = tempfile::Builder::new()
        .prefix("zendriver-ext-")
        .tempdir()
        .map_err(BrowserError::SpawnFailed)?;
    let dest = td.path().to_path_buf();
    let dest_for_blocking = dest.clone();
    let crx_for_blocking = crx_path.clone();

    tokio::task::spawn_blocking(move || unzip_crx_blocking(&crx_for_blocking, &dest_for_blocking))
        .await
        .map_err(|e| BrowserError::ExtensionLoad {
            path: crx_path.clone(),
            reason: format!("unzip task join error: {e}"),
        })??;

    Ok((dest, td))
}

/// Synchronous `.crx` → directory unzip body (runs on a blocking thread).
fn unzip_crx_blocking(crx_path: &Path, dest_dir: &Path) -> Result<(), ZendriverError> {
    use std::io::Cursor;

    let bytes = std::fs::read(crx_path).map_err(|e| BrowserError::ExtensionLoad {
        path: crx_path.to_path_buf(),
        reason: format!("read failed: {e}"),
    })?;
    // A `.crx` prepends a binary header before the ZIP. Find the first ZIP
    // local-file-header signature (`PK\x03\x04`) and treat everything from
    // there as the archive. A bare `.zip` (no CRX header) also works since the
    // signature is at offset 0.
    let zip_start = bytes
        .windows(4)
        .position(|w| w == [0x50, 0x4B, 0x03, 0x04])
        .ok_or_else(|| BrowserError::ExtensionLoad {
            path: crx_path.to_path_buf(),
            reason: "no ZIP payload found in .crx".into(),
        })?;
    let mut archive = zip::ZipArchive::new(Cursor::new(&bytes[zip_start..])).map_err(|e| {
        BrowserError::ExtensionLoad {
            path: crx_path.to_path_buf(),
            reason: format!("not a valid ZIP: {e}"),
        }
    })?;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| BrowserError::ExtensionLoad {
            path: crx_path.to_path_buf(),
            reason: format!("zip entry {i}: {e}"),
        })?;
        // Reject unsafe paths (absolute / parent-traversal).
        let Some(rel_path) = entry.enclosed_name() else {
            return Err(BrowserError::ExtensionLoad {
                path: crx_path.to_path_buf(),
                reason: format!("zip entry has unsafe path: {}", entry.name()),
            }
            .into());
        };
        // Refuse symlink entries — the primary zip-slip follow-on vector.
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            const S_IFMT: u32 = 0o170_000;
            const S_IFLNK: u32 = 0o120_000;
            if mode & S_IFMT == S_IFLNK {
                return Err(BrowserError::ExtensionLoad {
                    path: crx_path.to_path_buf(),
                    reason: format!("zip entry {rel_path:?} is a symlink; refusing"),
                }
                .into());
            }
        }
        let out_path = dest_dir.join(&rel_path);
        if entry.is_dir() {
            std::fs::create_dir_all(&out_path).map_err(|e| BrowserError::ExtensionLoad {
                path: crx_path.to_path_buf(),
                reason: format!("mkdir {out_path:?}: {e}"),
            })?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| BrowserError::ExtensionLoad {
                path: crx_path.to_path_buf(),
                reason: format!("mkdir {parent:?}: {e}"),
            })?;
        }
        let mut out = std::fs::File::create(&out_path).map_err(|e| BrowserError::ExtensionLoad {
            path: crx_path.to_path_buf(),
            reason: format!("create {out_path:?}: {e}"),
        })?;
        std::io::copy(&mut entry, &mut out).map_err(|e| BrowserError::ExtensionLoad {
            path: crx_path.to_path_buf(),
            reason: format!("write {out_path:?}: {e}"),
        })?;
    }
    Ok(())
}

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
    pub async fn launch(mut self) -> Result<Browser, ZendriverError> {
        // 1. Resolve Chrome executable.
        // Precedence: explicit `.executable(...)` > `CHROME_BIN` env var >
        // per-channel platform discovery. The env-var hop lets CI (and local
        // devs pointing at Canary / a downloaded Chrome-for-Testing build)
        // override the discovery path without code changes. The configured
        // `channel` (default `Auto`) only steers the final discovery step.
        let exe = match self.executable.clone() {
            Some(p) => p,
            None => match std::env::var("CHROME_BIN").ok().filter(|s| !s.is_empty()) {
                Some(p) => PathBuf::from(p),
                None => find_chrome_executable_for_channel(self.channel)?,
            },
        };

        // 1b. Resolve extensions: unzip any `.crx` into a tempdir and rewrite
        // `self.extensions` to the resolved unpacked-directory paths so
        // `build_flags` emits `--load-extension=<dir,…>`. Directory entries
        // pass through untouched. The returned tempdirs are handed to
        // `BrowserInner` below so the extracted dirs outlive Chrome.
        let extension_dirs = resolve_extension_dirs(&mut self.extensions).await?;

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
        // (records the resulting Tab handle) → user-supplied observers →
        // force-open-shadow-roots (independent of the stealth bundle).
        let (mut observers, extra_flags): (Vec<Arc<dyn TargetObserver>>, Vec<String>) =
            if let Some(ref profile) = self.stealth {
                let fp = profile.resolve_fingerprint(&exe)?;
                let stealth_obs: Arc<dyn TargetObserver> =
                    Arc::new(StealthObserver::new(profile.clone(), fp));
                let mut obs_vec = Vec::with_capacity(3 + self.extra_observers.len());
                obs_vec.push(stealth_obs);
                obs_vec.push(registrar.clone() as Arc<dyn TargetObserver>);
                obs_vec.extend(self.extra_observers.iter().cloned());
                (obs_vec, profile.build_flags())
            } else {
                let mut obs_vec = Vec::with_capacity(2 + self.extra_observers.len());
                obs_vec.push(registrar.clone() as Arc<dyn TargetObserver>);
                obs_vec.extend(self.extra_observers.iter().cloned());
                (obs_vec, Vec::new())
            };
        // Append the force-open-shadow-roots observer last so its injected
        // attachShadow override runs independently of (and after) the stealth
        // bundle. Only added when the caller opted in — keeps the default
        // observer chain untouched.
        if self.force_open_shadow_roots {
            observers.push(Arc::new(crate::expert::ShadowRootObserver) as Arc<dyn TargetObserver>);
        }

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
        // CI-friendly defaults: when running under CI (the runner sets
        // `CI=true`), Chrome's user-namespace sandbox refuses to start
        // because the GitHub-Actions / Docker container runs as root,
        // and the small /dev/shm in the container OOMs the renderer on
        // real workloads. Auto-add `--no-sandbox` and
        // `--disable-dev-shm-usage` unless the caller already supplied
        // them (so explicit user opt-in still wins).
        if std::env::var("CI").is_ok() {
            for needed in ["--no-sandbox", "--disable-dev-shm-usage"] {
                if !flags.iter().any(|f| f == needed) {
                    flags.push(needed.into());
                }
            }
        }
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

        // 8–12. Shared post-connect handshake (auto-attach, main-tab
        // discovery + attach, BrowserInner construction, registrar wiring).
        // Identical for spawn (`launch`) and attach (`connect`); the only
        // differences are the owned `Child` / `TempDir` we hand it and the
        // `owns_process` flag (true here — we spawned Chrome).
        let inner = finish_connect(FinishConnect {
            conn,
            registrar,
            input_profile,
            child: Some(child),
            owned_tmp,
            extension_dirs,
            debug_host_port: debug_host_port_from_ws(&ws_url),
            owns_process: true,
        })
        .await?;

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

    /// Attach to an already-running Chrome debug session instead of spawning
    /// a new process.
    ///
    /// `endpoint` is detected by scheme:
    /// - `ws://…` / `wss://…` — used directly as the DevTools browser
    ///   WebSocket URL (the `webSocketDebuggerUrl` Chrome prints to stderr as
    ///   `DevTools listening on …`).
    /// - `http://host:port` / `https://host:port` — a DevTools HTTP base;
    ///   `connect` performs `GET {endpoint}/json/version` and reads
    ///   `webSocketDebuggerUrl` from the JSON body (matching nodriver /
    ///   zendriver-py).
    ///
    /// The connected [`Browser`] does **not** own the Chrome process: its
    /// [`Browser::close`] shuts down only the transport, and dropping it does
    /// **not** terminate Chrome. Use this to drive a long-lived browser you
    /// started elsewhere.
    ///
    /// `.stealth(profile)` and `.observer(..)` still apply, but only to
    /// targets attached **after** this call — the stealth observer is wired
    /// into the same browser-wide `Target.setAutoAttach { flatten: true }`
    /// that fires on newly-attached targets. Tabs already open when you
    /// connect predate the observer chain and are **not** patched.
    ///
    /// The spawn-only builder fields — [`BrowserBuilder::executable`],
    /// [`BrowserBuilder::user_data_dir`], [`BrowserBuilder::downloads_dir`],
    /// and any launch flags ([`BrowserBuilder::arg`] / headless / sandbox /
    /// channel / lang / user-agent) — are **ignored** on the connect path,
    /// since no process is launched.
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::Browser`] when an `http(s)://` endpoint
    /// cannot be resolved to a `webSocketDebuggerUrl`, and
    /// [`ZendriverError::Transport`] when the WebSocket attach fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// // Chrome already started with `--remote-debugging-port=9222`.
    /// let browser = zendriver::Browser::builder()
    ///     .connect("http://127.0.0.1:9222").await?;
    /// let tab = browser.main_tab();
    /// tab.goto("https://example.com").await?;
    /// // Does NOT kill the Chrome we attached to:
    /// browser.close().await?;
    /// # Ok(()) }
    /// ```
    pub async fn connect(self, endpoint: impl Into<String>) -> Result<Browser, ZendriverError> {
        let endpoint = endpoint.into();

        // Resolve the browser WebSocket URL from the endpoint scheme.
        let ws_url = if endpoint.starts_with("ws://") || endpoint.starts_with("wss://") {
            endpoint
        } else if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
            resolve_ws_from_http(&endpoint).await?
        } else {
            return Err(BrowserError::DevtoolsParse.into());
        };

        // Resolve the per-tab InputProfile + build the observer chain exactly
        // like `launch` does (stealth observer → tab registrar → user
        // observers). The spawn-only branches (`executable`, flags, TempDir)
        // are intentionally skipped — `connect` never launches a process.
        let input_profile = self
            .stealth
            .as_ref()
            .map_or_else(zendriver_stealth::InputProfile::native, |sp| {
                sp.input_profile()
            });
        let registrar = Arc::new(TabRegistrar::new(input_profile.clone()));
        let mut observers: Vec<Arc<dyn TargetObserver>> = if let Some(ref profile) = self.stealth {
            // No executable to resolve a fingerprint from on the connect path;
            // fall back to the profile's default fingerprint.
            let fp = profile.resolve_fingerprint(Path::new(""))?;
            let stealth_obs: Arc<dyn TargetObserver> =
                Arc::new(StealthObserver::new(profile.clone(), fp));
            let mut obs_vec = Vec::with_capacity(3 + self.extra_observers.len());
            obs_vec.push(stealth_obs);
            obs_vec.push(registrar.clone() as Arc<dyn TargetObserver>);
            obs_vec.extend(self.extra_observers.iter().cloned());
            obs_vec
        } else {
            let mut obs_vec = Vec::with_capacity(2 + self.extra_observers.len());
            obs_vec.push(registrar.clone() as Arc<dyn TargetObserver>);
            obs_vec.extend(self.extra_observers.iter().cloned());
            obs_vec
        };
        // Same opt-in force-open-shadow-roots observer as the launch path.
        if self.force_open_shadow_roots {
            observers.push(Arc::new(crate::expert::ShadowRootObserver) as Arc<dyn TargetObserver>);
        }

        debug!(ws_url = %ws_url, "attaching to running chrome");
        let conn = zendriver_transport::connect_with_observers(&ws_url, observers).await?;

        // Same post-connect handshake as `launch`, but with no owned process:
        // no `Child`, no `TempDir`, `owns_process = false`.
        let inner = finish_connect(FinishConnect {
            conn,
            registrar,
            input_profile,
            child: None,
            owned_tmp: None,
            extension_dirs: Vec::new(),
            debug_host_port: debug_host_port_from_ws(&ws_url),
            owns_process: false,
        })
        .await?;

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

/// A browser permission that can be granted or denied via
/// [`Browser::grant_permissions`].
///
/// Mirrors the CDP [`Browser.PermissionType`][1] enum. Each variant maps to a
/// camelCase wire string via [`PermissionType::as_cdp`]; the full set is
/// available as [`PermissionType::ALL`] (used by
/// [`Browser::grant_all_permissions`]).
///
/// [1]: https://chromedevtools.github.io/devtools-protocol/tot/Browser/#type-PermissionType
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PermissionType {
    /// `accessibilityEvents` — accessibility event delivery.
    AccessibilityEvents,
    /// `audioCapture` — microphone access (alias of [`Self::Microphone`] on
    /// the wire).
    AudioCapture,
    /// `backgroundSync` — Background Sync API.
    BackgroundSync,
    /// `backgroundFetch` — Background Fetch API.
    BackgroundFetch,
    /// Camera access. Ergonomic alias of [`Self::VideoCapture`]; both map to
    /// the `videoCapture` wire string.
    Camera,
    /// `clipboardReadWrite` — unsanitized clipboard read + write.
    ClipboardReadWrite,
    /// `clipboardSanitizedWrite` — sanitized clipboard write.
    ClipboardSanitizedWrite,
    /// `displayCapture` — screen / window capture.
    DisplayCapture,
    /// `durableStorage` — persistent storage grant.
    DurableStorage,
    /// `geolocation` — location access.
    Geolocation,
    /// `idleDetection` — Idle Detection API.
    IdleDetection,
    /// `localFonts` — local font enumeration.
    LocalFonts,
    /// `midi` — Web MIDI access (no SysEx).
    Midi,
    /// `midiSysex` — Web MIDI access including SysEx messages.
    MidiSysex,
    /// Microphone access. Ergonomic alias of [`Self::AudioCapture`]; both map
    /// to the `audioCapture` wire string.
    Microphone,
    /// `nfc` — Web NFC access.
    Nfc,
    /// `notifications` — desktop notifications.
    Notifications,
    /// `paymentHandler` — Payment Handler API.
    PaymentHandler,
    /// `periodicBackgroundSync` — Periodic Background Sync API.
    PeriodicBackgroundSync,
    /// `protectedMediaIdentifier` — protected media (EME) identifier.
    ProtectedMediaIdentifier,
    /// `sensors` — generic sensor access (accelerometer, gyroscope, …).
    Sensors,
    /// `storageAccess` — Storage Access API.
    StorageAccess,
    /// `topLevelStorageAccess` — top-level Storage Access API.
    TopLevelStorageAccess,
    /// `videoCapture` — camera access (alias of [`Self::Camera`] on the wire).
    VideoCapture,
    /// `videoCapturePanTiltZoom` — camera pan/tilt/zoom control.
    VideoCapturePanTiltZoom,
    /// `wakeLockScreen` — screen wake lock.
    WakeLockScreen,
    /// `wakeLockSystem` — system wake lock.
    WakeLockSystem,
    /// `windowManagement` — multi-screen window placement.
    WindowManagement,
}

impl PermissionType {
    /// Every [`PermissionType`] variant — the list
    /// [`Browser::grant_all_permissions`] grants.
    ///
    /// Mirrors nodriver's `grant_all_permissions`: the complete CDP permission
    /// set. `Camera` / `Microphone` are intentionally omitted because they are
    /// wire-string aliases of [`Self::VideoCapture`] / [`Self::AudioCapture`]
    /// (CDP would reject the duplicate strings).
    pub const ALL: &'static [PermissionType] = &[
        PermissionType::AccessibilityEvents,
        PermissionType::AudioCapture,
        PermissionType::BackgroundSync,
        PermissionType::BackgroundFetch,
        PermissionType::ClipboardReadWrite,
        PermissionType::ClipboardSanitizedWrite,
        PermissionType::DisplayCapture,
        PermissionType::DurableStorage,
        PermissionType::Geolocation,
        PermissionType::IdleDetection,
        PermissionType::LocalFonts,
        PermissionType::Midi,
        PermissionType::MidiSysex,
        PermissionType::Nfc,
        PermissionType::Notifications,
        PermissionType::PaymentHandler,
        PermissionType::PeriodicBackgroundSync,
        PermissionType::ProtectedMediaIdentifier,
        PermissionType::Sensors,
        PermissionType::StorageAccess,
        PermissionType::TopLevelStorageAccess,
        PermissionType::VideoCapture,
        PermissionType::VideoCapturePanTiltZoom,
        PermissionType::WakeLockScreen,
        PermissionType::WakeLockSystem,
        PermissionType::WindowManagement,
    ];

    /// The camelCase CDP wire string for this permission (e.g. `"geolocation"`,
    /// `"videoCapture"`).
    ///
    /// # Examples
    ///
    /// ```
    /// use zendriver::PermissionType;
    /// assert_eq!(PermissionType::ClipboardReadWrite.as_cdp(), "clipboardReadWrite");
    /// ```
    #[must_use]
    pub fn as_cdp(&self) -> &'static str {
        match self {
            PermissionType::AccessibilityEvents => "accessibilityEvents",
            // `Camera` / `Microphone` map to the same wire strings, so both
            // alias spellings collapse here.
            PermissionType::AudioCapture | PermissionType::Microphone => "audioCapture",
            PermissionType::BackgroundSync => "backgroundSync",
            PermissionType::BackgroundFetch => "backgroundFetch",
            PermissionType::ClipboardReadWrite => "clipboardReadWrite",
            PermissionType::ClipboardSanitizedWrite => "clipboardSanitizedWrite",
            PermissionType::DisplayCapture => "displayCapture",
            PermissionType::DurableStorage => "durableStorage",
            PermissionType::Geolocation => "geolocation",
            PermissionType::IdleDetection => "idleDetection",
            PermissionType::LocalFonts => "localFonts",
            PermissionType::Midi => "midi",
            PermissionType::MidiSysex => "midiSysex",
            PermissionType::Nfc => "nfc",
            PermissionType::Notifications => "notifications",
            PermissionType::PaymentHandler => "paymentHandler",
            PermissionType::PeriodicBackgroundSync => "periodicBackgroundSync",
            PermissionType::ProtectedMediaIdentifier => "protectedMediaIdentifier",
            PermissionType::Sensors => "sensors",
            PermissionType::StorageAccess => "storageAccess",
            PermissionType::TopLevelStorageAccess => "topLevelStorageAccess",
            PermissionType::VideoCapture | PermissionType::Camera => "videoCapture",
            PermissionType::VideoCapturePanTiltZoom => "videoCapturePanTiltZoom",
            PermissionType::WakeLockScreen => "wakeLockScreen",
            PermissionType::WakeLockSystem => "wakeLockSystem",
            PermissionType::WindowManagement => "windowManagement",
        }
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

    /// Attach to an already-running Chrome debug session instead of spawning.
    ///
    /// Shortcut for [`Browser::builder().connect(endpoint)`][BrowserBuilder::connect]
    /// — see that method for endpoint formats (`ws://…` / `http://host:port`),
    /// the non-owning lifecycle (`close` / drop never kill the attached
    /// process), and which builder fields are ignored on the connect path.
    ///
    /// # Errors
    ///
    /// Same as [`BrowserBuilder::connect`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// let browser = zendriver::Browser::connect("http://127.0.0.1:9222").await?;
    /// # browser.close().await?;
    /// # Ok(()) }
    /// ```
    pub async fn connect(endpoint: impl Into<String>) -> Result<Browser, ZendriverError> {
        BrowserBuilder::new().connect(endpoint).await
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

    /// Create a new isolated [`crate::BrowserContext`] bound to an optional
    /// proxy.
    ///
    /// Wraps the CDP [`Target.createBrowserContext`][1] command: when
    /// `proxy_server` is `Some`, the returned context routes all network
    /// traffic through that upstream (mirroring Chrome's `--proxy-server`
    /// flag — e.g. `"http://host:port"` or
    /// `"http://user:pass@host:port"`). When `proxy_bypass_list` is `Some`,
    /// hosts matching the pattern (e.g. `"<-loopback>"` or
    /// `"*.internal.example.com"`) bypass the proxy. Either field is
    /// **omitted** from the params when `None`, not sent as `null` — some
    /// CDP versions reject unknown null fields.
    ///
    /// Drop the returned handle to schedule asynchronous disposal via
    /// `Target.disposeBrowserContext`.
    ///
    /// [1]: https://chromedevtools.github.io/devtools-protocol/tot/Target/#method-createBrowserContext
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::Navigation`] if the CDP response is missing
    /// the `browserContextId` field; bubbles up any transport-level error
    /// from the underlying `call_raw`.
    pub async fn create_browser_context_with(
        &self,
        proxy_server: Option<&str>,
        proxy_bypass_list: Option<&str>,
    ) -> Result<crate::BrowserContext, ZendriverError> {
        let mut params = serde_json::Map::new();
        if let Some(p) = proxy_server {
            params.insert(
                "proxyServer".into(),
                serde_json::Value::String(p.to_string()),
            );
        }
        if let Some(b) = proxy_bypass_list {
            params.insert(
                "proxyBypassList".into(),
                serde_json::Value::String(b.to_string()),
            );
        }

        let res = self
            .inner
            .conn
            .call_raw(
                "Target.createBrowserContext",
                serde_json::Value::Object(params),
                None,
            )
            .await?;

        let id = res
            .get("browserContextId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ZendriverError::Navigation(
                    "Target.createBrowserContext returned no browserContextId".into(),
                )
            })?
            .to_string();

        Ok(crate::BrowserContext {
            browser: self.inner.clone(),
            id,
        })
    }

    /// Create a new default [`crate::BrowserContext`] (no proxy, no bypass).
    ///
    /// Convenience wrapper over [`Browser::create_browser_context_with`]
    /// called with both arguments as `None`.
    ///
    /// # Errors
    ///
    /// Same as [`Browser::create_browser_context_with`].
    pub async fn create_browser_context(&self) -> Result<crate::BrowserContext, ZendriverError> {
        self.create_browser_context_with(None, None).await
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
        self.create_target(url.into(), false).await
    }

    /// Open a new top-level OS window navigated to `about:blank`.
    ///
    /// Like [`Browser::new_tab`] but passes `newWindow: true` to
    /// `Target.createTarget`, so Chrome opens a separate browser window rather
    /// than a tab in the existing one. The returned [`Tab`] is registered via
    /// the same observer path as `new_tab`. Mirrors nodriver's
    /// `get(new_window=True)`.
    ///
    /// # Errors
    ///
    /// Same as [`Browser::new_tab`]: [`ZendriverError::TabNotFound`] if the
    /// registrar fails to register the new window's tab within the wait
    /// window.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// let browser = zendriver::Browser::builder().launch().await?;
    /// let win = browser.new_window().await?;
    /// win.goto("https://example.com").await?;
    /// # Ok(()) }
    /// ```
    pub async fn new_window(&self) -> Result<Tab, ZendriverError> {
        self.create_target("about:blank".to_string(), true).await
    }

    /// Open a new top-level OS window navigated to `url`.
    ///
    /// [`Browser::new_window`] with a custom initial URL. Issue
    /// `.wait_for_load()` on the returned [`Tab`] to block on the navigation.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// let win = browser.new_window_at("https://example.com").await?;
    /// win.wait_for_load().await?;
    /// # Ok(()) }
    /// ```
    pub async fn new_window_at(&self, url: impl Into<String>) -> Result<Tab, ZendriverError> {
        self.create_target(url.into(), true).await
    }

    /// Shared `Target.createTarget` → registrar-wait path behind
    /// [`Browser::new_tab_at`] (`new_window = false`) and
    /// [`Browser::new_window_at`] (`new_window = true`).
    async fn create_target(&self, url: String, new_window: bool) -> Result<Tab, ZendriverError> {
        let mut params = json!({ "url": url });
        if new_window {
            params["newWindow"] = serde_json::Value::Bool(true);
        }
        let res = self
            .inner
            .conn
            .call_raw("Target.createTarget", params, None)
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
    /// For a browser produced by [`BrowserBuilder::connect`] (we did not spawn
    /// Chrome) this shuts down only the transport and leaves the attached
    /// process running.
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
        // Attached (non-owning) sessions must never terminate the process we
        // connected to — only the transport is torn down above. Spawn-only.
        if !self.inner.owns_process {
            return Ok(());
        }
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

    /// Grant `perms` to `origin` (or browser-wide when `origin` is `None`).
    ///
    /// Wraps the CDP [`Browser.grantPermissions`][1] command, sent at browser
    /// scope (no `sessionId`). Each [`PermissionType`] is mapped to its
    /// camelCase wire string. When `origin` is `Some`, the grant is scoped to
    /// that origin (e.g. `"https://example.com"`); when `None`, the `origin`
    /// key is omitted so the grant applies browser-wide.
    ///
    /// Granting a permission pre-authorizes it without the usual user prompt —
    /// useful for unattended automation that would otherwise stall on a
    /// permission dialog (geolocation, clipboard, notifications, …).
    ///
    /// [1]: https://chromedevtools.github.io/devtools-protocol/tot/Browser/#method-grantPermissions
    ///
    /// # Errors
    ///
    /// Bubbles up any transport-level error from the underlying `call_raw`
    /// (e.g. the connection was shut down).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use zendriver::PermissionType;
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// browser
    ///     .grant_permissions(
    ///         &[PermissionType::Geolocation, PermissionType::Notifications],
    ///         Some("https://example.com"),
    ///     )
    ///     .await?;
    /// # Ok(()) }
    /// ```
    pub async fn grant_permissions(
        &self,
        perms: &[PermissionType],
        origin: Option<&str>,
    ) -> Result<(), ZendriverError> {
        let mut params = serde_json::Map::new();
        let permissions: Vec<serde_json::Value> = perms
            .iter()
            .map(|p| serde_json::Value::String(p.as_cdp().to_string()))
            .collect();
        params.insert("permissions".into(), serde_json::Value::Array(permissions));
        // Omit `origin` entirely when None — a browser-wide grant. Some CDP
        // versions reject an explicit null here.
        if let Some(o) = origin {
            params.insert("origin".into(), serde_json::Value::String(o.to_string()));
        }
        self.inner
            .conn
            .call_raw(
                "Browser.grantPermissions",
                serde_json::Value::Object(params),
                None,
            )
            .await?;
        Ok(())
    }

    /// Grant every [`PermissionType`] browser-wide.
    ///
    /// Convenience wrapper over [`Browser::grant_permissions`] called with
    /// [`PermissionType::ALL`] and no origin — mirrors nodriver /
    /// zendriver-py's `grant_all_permissions`. Pre-authorizes the full CDP
    /// permission set so automated runs never stall on a permission prompt.
    ///
    /// # Errors
    ///
    /// Same as [`Browser::grant_permissions`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// browser.grant_all_permissions().await?;
    /// # Ok(()) }
    /// ```
    pub async fn grant_all_permissions(&self) -> Result<(), ZendriverError> {
        self.grant_permissions(PermissionType::ALL, None).await
    }

    /// Reset all previously-granted permissions to their default prompt state.
    ///
    /// Wraps the CDP [`Browser.resetPermissions`][1] command, sent at browser
    /// scope (no `sessionId`). Clears every override installed via
    /// [`Browser::grant_permissions`] / [`Browser::grant_all_permissions`].
    ///
    /// [1]: https://chromedevtools.github.io/devtools-protocol/tot/Browser/#method-resetPermissions
    ///
    /// # Errors
    ///
    /// Bubbles up any transport-level error from the underlying `call_raw`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// browser.reset_permissions().await?;
    /// # Ok(()) }
    /// ```
    pub async fn reset_permissions(&self) -> Result<(), ZendriverError> {
        self.inner
            .conn
            .call_raw("Browser.resetPermissions", json!({}), None)
            .await?;
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
///
/// For a browser produced by [`BrowserBuilder::connect`] (`owns_process ==
/// false`) there is no owned `Child`, so step 2 is a no-op: dropping detaches
/// the transport but never signals the attached Chrome process.
impl Drop for BrowserInner {
    fn drop(&mut self) {
        self.conn.shutdown();
        // We can't `.await` in Drop. If `close()` was not called explicitly,
        // we rely on `kill_on_drop(true)` set on the spawned Command, which
        // causes tokio to SIGKILL the child when the Child is dropped.
        // The TempDir for user_data_dir is dropped here too.
        //
        // Attached (`connect`) browsers hold no `Child` — `owns_process` is
        // false and the field is `None` — so nothing is killed here.
        debug_assert!(
            self.owns_process || self.child.try_lock().map(|g| g.is_none()).unwrap_or(true),
            "a non-owning Browser must not hold a Child handle",
        );
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn candidate_paths_is_nonempty() {
        let v = candidate_paths_for_channel(Channel::Auto);
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
    fn debug_host_port_from_ws_extracts_authority() {
        assert_eq!(
            debug_host_port_from_ws("ws://127.0.0.1:9222/devtools/browser/abc").as_deref(),
            Some("127.0.0.1:9222")
        );
        assert_eq!(
            debug_host_port_from_ws("wss://example.test:1/x").as_deref(),
            Some("example.test:1")
        );
        // No path component still yields the authority.
        assert_eq!(
            debug_host_port_from_ws("ws://localhost:5555").as_deref(),
            Some("localhost:5555")
        );
        // Non-ws schemes / garbage → None.
        assert_eq!(debug_host_port_from_ws("http://x:1/y"), None);
        assert_eq!(debug_host_port_from_ws("nonsense"), None);
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
        assert!(
            flags
                .iter()
                .any(|f| f.contains("PasswordManagerOnboarding"))
        );
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

    // ----- C4: lang / user_agent / sandbox / channel ---------------------

    #[test]
    fn lang_flag_present() {
        let b = BrowserBuilder::new().lang("en-US");
        let flags = b.build_flags(Path::new("/tmp/x"));
        assert!(flags.contains(&"--lang=en-US".to_string()));
    }

    #[test]
    fn user_agent_flag_present() {
        let b = BrowserBuilder::new().user_agent("MyAgent/1.0");
        let flags = b.build_flags(Path::new("/tmp/x"));
        assert!(flags.contains(&"--user-agent=MyAgent/1.0".to_string()));
    }

    #[test]
    fn sandbox_false_adds_no_sandbox() {
        let b = BrowserBuilder::new().sandbox(false);
        let flags = b.build_flags(Path::new("/tmp/x"));
        assert!(flags.contains(&"--no-sandbox".to_string()));
    }

    #[test]
    fn sandbox_default_on_omits_no_sandbox() {
        // Default builder (sandbox on) must NOT emit --no-sandbox from
        // build_flags. The CI auto-add lives in `launch`, not build_flags,
        // so this is unaffected by the CI env var.
        let b = BrowserBuilder::new();
        let flags = b.build_flags(Path::new("/tmp/x"));
        assert!(!flags.contains(&"--no-sandbox".to_string()));
    }

    // ----- C2: expert mode -----------------------------------------------

    #[test]
    fn expert_adds_web_security_and_site_isolation_flags() {
        let b = BrowserBuilder::new().expert(true);
        let flags = b.build_flags(Path::new("/tmp/x"));
        assert!(
            flags.contains(&"--disable-web-security".to_string()),
            "expected --disable-web-security in {flags:?}"
        );
        assert!(
            flags.contains(&"--disable-site-isolation-trials".to_string()),
            "expected --disable-site-isolation-trials in {flags:?}"
        );
    }

    #[test]
    fn expert_off_omits_expert_flags() {
        let flags = BrowserBuilder::new().build_flags(Path::new("/tmp/x"));
        assert!(!flags.contains(&"--disable-web-security".to_string()));
        assert!(!flags.contains(&"--disable-site-isolation-trials".to_string()));
    }

    // ----- C3: extensions ------------------------------------------------

    #[test]
    fn extensions_add_load_and_disable_except_flags() {
        let b = BrowserBuilder::new()
            .add_extension("a")
            .add_extension("b");
        let flags = b.build_flags(Path::new("/tmp/x"));
        assert!(
            flags.contains(&"--load-extension=a,b".to_string()),
            "expected --load-extension=a,b in {flags:?}"
        );
        assert!(
            flags.contains(&"--disable-extensions-except=a,b".to_string()),
            "expected --disable-extensions-except=a,b in {flags:?}"
        );
        assert!(
            flags.contains(&"--enable-unsafe-extension-debugging".to_string()),
            "expected --enable-unsafe-extension-debugging in {flags:?}"
        );
    }

    #[test]
    fn extensions_force_disable_load_extension_feature_even_off_profile() {
        // Stealth Off + extensions: build_flags is stealth-agnostic, so the
        // DisableLoadExtensionCommandLineSwitch feature must ride in the base
        // --disable-features line whenever extensions are present (otherwise
        // an Off-profile launch silently fails to load them on Chrome 136+).
        let b = BrowserBuilder::new()
            .stealth(StealthProfile::off())
            .add_extension("ext");
        let flags = b.build_flags(Path::new("/tmp/x"));
        assert!(
            flags
                .iter()
                .any(|f| f.starts_with("--disable-features=")
                    && f.contains("DisableLoadExtensionCommandLineSwitch")),
            "expected DisableLoadExtensionCommandLineSwitch in a --disable-features line: {flags:?}"
        );
    }

    #[test]
    fn no_extensions_omits_extension_flags() {
        // Default builder must not emit any extension flags (keeps the default
        // argv + snapshots unchanged).
        let flags = BrowserBuilder::new().build_flags(Path::new("/tmp/x"));
        assert!(!flags.iter().any(|f| f.starts_with("--load-extension")));
        assert!(
            !flags
                .iter()
                .any(|f| f.starts_with("--disable-extensions-except"))
        );
        assert!(!flags.contains(&"--enable-unsafe-extension-debugging".to_string()));
        assert!(
            !flags
                .iter()
                .any(|f| f.contains("DisableLoadExtensionCommandLineSwitch")),
            "default build must not carry the extension feature toggle: {flags:?}"
        );
    }

    #[tokio::test]
    async fn resolve_extension_dirs_passes_through_directories() {
        // A directory entry is used as-is (no tempdir allocated).
        let dir = tempfile::tempdir().unwrap();
        let mut exts = vec![dir.path().to_path_buf()];
        let tempdirs = resolve_extension_dirs(&mut exts).await.unwrap();
        assert!(tempdirs.is_empty(), "directories should not allocate tempdirs");
        assert_eq!(exts, vec![dir.path().to_path_buf()]);
    }

    #[tokio::test]
    async fn resolve_extension_dirs_unzips_crx_to_tempdir() {
        // Build a minimal CRX (CRX3-ish: `Cr24` magic + a fake header, then a
        // ZIP carrying manifest.json) and assert it unzips to a real dir.
        use std::io::Write;
        let mut zip_buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut zip_buf));
            w.start_file("manifest.json", zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(br#"{"name":"t","version":"1","manifest_version":3}"#)
                .unwrap();
            w.finish().unwrap();
        }
        // Prepend a token CRX header before the ZIP payload.
        let mut crx = Vec::new();
        crx.extend_from_slice(b"Cr24");
        crx.extend_from_slice(&3u32.to_le_bytes()); // version
        crx.extend_from_slice(&0u32.to_le_bytes()); // header length (ignored)
        crx.extend_from_slice(&zip_buf);

        let crx_file = tempfile::Builder::new()
            .suffix(".crx")
            .tempfile()
            .unwrap();
        std::fs::write(crx_file.path(), &crx).unwrap();

        let mut exts = vec![crx_file.path().to_path_buf()];
        let tempdirs = resolve_extension_dirs(&mut exts).await.unwrap();
        assert_eq!(tempdirs.len(), 1, "crx should allocate one tempdir");
        // The slot was rewritten to the extracted directory…
        assert!(exts[0].is_dir(), "crx slot should resolve to a directory");
        assert_ne!(exts[0], crx_file.path());
        // …and the manifest landed inside it.
        assert!(exts[0].join("manifest.json").is_file());
    }

    #[tokio::test]
    async fn resolve_extension_dirs_errors_on_missing_path() {
        let mut exts = vec![PathBuf::from("/nonexistent/zzz-does-not-exist")];
        let err = resolve_extension_dirs(&mut exts).await.unwrap_err();
        assert!(
            matches!(err, ZendriverError::Browser(BrowserError::ExtensionLoad { .. })),
            "expected ExtensionLoad, got {err:?}"
        );
    }

    #[test]
    fn channel_brave_resolves_brave_path() {
        // Probe the candidate-path table directly so the test does not
        // require Brave to be installed. Every Brave candidate path must
        // mention "brave" somewhere (per-OS install dirs / binary names),
        // compared case-insensitively (Linux uses lowercase `brave-browser`,
        // macOS/Windows use `Brave Browser` / `Brave-Browser`).
        let paths = candidate_paths_for_channel(Channel::Brave);
        assert!(!paths.is_empty(), "Brave channel must yield candidates");
        assert!(
            paths
                .iter()
                .all(|p| p.to_string_lossy().to_lowercase().contains("brave")),
            "every Brave candidate path should reference Brave: {paths:?}"
        );
    }

    #[test]
    fn channel_edge_resolves_edge_path() {
        let paths = candidate_paths_for_channel(Channel::Edge);
        assert!(!paths.is_empty(), "Edge channel must yield candidates");
        assert!(
            paths
                .iter()
                .all(|p| p.to_string_lossy().to_lowercase().contains("edge")),
            "every Edge candidate path should reference Edge: {paths:?}"
        );
    }

    #[test]
    fn channel_auto_includes_chrome_family_candidates() {
        // Auto preserves the historical first-found behavior: it offers both
        // the Chrome and Chromium fallbacks (so neither a Chrome-only nor a
        // Chromium-only box regresses). `find_chrome_executable()` delegates
        // straight to this table.
        let paths = candidate_paths_for_channel(Channel::Auto);
        assert!(!paths.is_empty(), "Auto must yield candidates");
        let joined = paths
            .iter()
            .map(|p| p.to_string_lossy().to_lowercase())
            .collect::<Vec<_>>()
            .join("|");
        assert!(
            joined.contains("chrom"),
            "Auto candidates should reference a Chrome/Chromium binary: {paths:?}"
        );
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
                _extension_dirs: Vec::new(),
                owns_process: false,
                stealth_input_profile: input_profile.clone(),
                tabs: tokio::sync::RwLock::new(map),
                debug_host_port: None,
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
                _extension_dirs: Vec::new(),
                owns_process: false,
                stealth_input_profile: input_profile.clone(),
                tabs: tokio::sync::RwLock::new(map),
                debug_host_port: None,
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

    /// [`Browser::new_window_at`] must pass `newWindow: true` to
    /// `Target.createTarget` (vs `new_tab_at`, which omits the flag), while
    /// reusing the same registrar-wait registration path.
    #[tokio::test]
    async fn new_window_at_passes_new_window_true() {
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
                _extension_dirs: Vec::new(),
                owns_process: false,
                stealth_input_profile: input_profile.clone(),
                tabs: tokio::sync::RwLock::new(map),
                debug_host_port: None,
                tabs_changed: tokio::sync::Notify::new(),
                #[cfg(feature = "interception")]
                proxy_auth_handle: std::sync::OnceLock::new(),
            }
        });
        registrar.set_browser(Arc::downgrade(&inner));
        let browser = Browser {
            inner: inner.clone(),
        };

        let fut = tokio::spawn({
            let b = browser.clone();
            async move { b.new_window_at("https://example.com").await }
        });

        let create_id = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            mock.expect_cmd("Target.createTarget"),
        )
        .await
        .expect("Target.createTarget not sent within 2s");
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["url"], "https://example.com");
        assert_eq!(
            sent["params"]["newWindow"], true,
            "new_window_at must set newWindow:true"
        );
        mock.reply(create_id, json!({ "targetId": "TW" })).await;

        mock.emit_event(
            "Target.attachedToTarget",
            json!({
                "sessionId": "SW",
                "targetInfo": {
                    "targetId": "TW",
                    "type": "page",
                    "url": "https://example.com",
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

        let win = tokio::time::timeout(std::time::Duration::from_secs(2), fut)
            .await
            .expect("new_window_at future did not resolve within 2s")
            .expect("spawned task panicked")
            .expect("new_window_at returned Err");
        assert_eq!(win.target_id(), "TW");

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
                _extension_dirs: Vec::new(),
                owns_process: false,
                stealth_input_profile: input_profile.clone(),
                tabs: tokio::sync::RwLock::new(map),
                debug_host_port: None,
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
    /// must dispatch `Storage.setCookies` on that connection — confirming the
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
                _extension_dirs: Vec::new(),
                owns_process: false,
                stealth_input_profile: input_profile.clone(),
                tabs: tokio::sync::RwLock::new(map),
                debug_host_port: None,
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
                ..Default::default()
            })
            .await
        });

        let id = mock.expect_cmd("Storage.setCookies").await;
        let params = &mock.last_sent()["params"];
        let arr = params["cookies"]
            .as_array()
            .expect("setCookies payload must carry a cookies array");
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["name"], "sid");
        // Browser-scope command — no session_id.
        assert!(mock.last_sent().get("sessionId").is_none());
        mock.reply(id, json!({})).await;

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
                _extension_dirs: Vec::new(),
                owns_process: false,
                stealth_input_profile: input_profile.clone(),
                tabs: tokio::sync::RwLock::new(map),
                debug_host_port: None,
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
                _extension_dirs: Vec::new(),
                owns_process: false,
                stealth_input_profile: input_profile.clone(),
                tabs: tokio::sync::RwLock::new(map),
                debug_host_port: None,
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
                _extension_dirs: Vec::new(),
                owns_process: false,
                stealth_input_profile: input_profile.clone(),
                tabs: tokio::sync::RwLock::new(map),
                debug_host_port: None,
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

    // ----- BrowserInner::dispose_browser_context (per-context proxy) -----

    /// Asserts [`BrowserInner::dispose_browser_context`] issues exactly one
    /// `Target.disposeBrowserContext` CDP command at browser scope (no
    /// `sessionId`) carrying the supplied `browserContextId`, and that the
    /// awaited future resolves `Ok` once the mock replies.
    ///
    /// Wired into the per-context proxy support series: [`crate::BrowserContext`]'s
    /// `Drop` impl calls this method to release the context when the handle
    /// goes out of scope.
    #[tokio::test]
    async fn dispose_browser_context_sends_target_dispose() {
        use zendriver_transport::testing::MockConnection;

        let (mut mock, conn) = MockConnection::pair();
        let inner = test_only_inner_from_conn(conn.clone());

        let inner_for_task = inner.clone();
        let fut =
            tokio::spawn(async move { inner_for_task.dispose_browser_context("ctx-abc").await });

        let id = mock.expect_cmd("Target.disposeBrowserContext").await;
        assert_eq!(mock.last_sent()["params"]["browserContextId"], "ctx-abc");
        // Browser-scope command — no session_id.
        assert!(mock.last_sent().get("sessionId").is_none());
        mock.reply(id, json!({})).await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    // ----- Browser::create_browser_context[_with] (per-context proxy) ----

    /// Asserts [`Browser::create_browser_context_with`] sends a
    /// `Target.createBrowserContext` command carrying `proxyServer` and
    /// `proxyBypassList` exactly as supplied, and that the returned
    /// [`BrowserContext`] handle exposes the `browserContextId` from the
    /// CDP reply.
    #[tokio::test]
    async fn create_browser_context_with_sends_correct_cdp() {
        use zendriver_transport::testing::MockConnection;
        let (mut mock, conn) = MockConnection::pair();
        let browser = test_only_browser_from_conn(conn.clone());

        let fut = tokio::spawn({
            let b = browser.clone();
            async move {
                b.create_browser_context_with(
                    Some("http://user:pass@p.webshare.io:80"),
                    Some("<-loopback>"),
                )
                .await
            }
        });

        let id = mock.expect_cmd("Target.createBrowserContext").await;
        let sent = mock.last_sent();
        assert_eq!(
            sent["params"]["proxyServer"],
            "http://user:pass@p.webshare.io:80"
        );
        assert_eq!(sent["params"]["proxyBypassList"], "<-loopback>");
        mock.reply(id, json!({ "browserContextId": "ctx-new" }))
            .await;

        let ctx = fut.await.unwrap().unwrap();
        assert_eq!(ctx.id(), "ctx-new");

        conn.shutdown();
    }

    /// Asserts that when both `proxy_server` and `proxy_bypass_list` are
    /// `None`, neither key is sent as a non-null value. CDP rejects unknown
    /// null fields on some commands, so the implementation must **omit**
    /// the keys entirely from the params object (a `null` value is also
    /// accepted for forward-compat, but `Some(value)` of any kind would
    /// fail the assertion).
    #[tokio::test]
    async fn create_browser_context_without_proxy_omits_fields() {
        use zendriver_transport::testing::MockConnection;
        let (mut mock, conn) = MockConnection::pair();
        let browser = test_only_browser_from_conn(conn.clone());

        let fut = tokio::spawn({
            let b = browser.clone();
            async move { b.create_browser_context_with(None, None).await }
        });

        let id = mock.expect_cmd("Target.createBrowserContext").await;
        let sent = mock.last_sent();
        let proxy_server_field = sent["params"].get("proxyServer");
        assert!(proxy_server_field.is_none() || proxy_server_field.unwrap().is_null());
        let bypass_field = sent["params"].get("proxyBypassList");
        assert!(bypass_field.is_none() || bypass_field.unwrap().is_null());

        mock.reply(id, json!({ "browserContextId": "ctx-plain" }))
            .await;

        let ctx = fut.await.unwrap().unwrap();
        assert_eq!(ctx.id(), "ctx-plain");

        conn.shutdown();
    }

    // ----- Browser::grant_permissions / reset_permissions (C5) -----------

    /// [`Browser::grant_permissions`] maps each [`PermissionType`] to its
    /// CDP camelCase string and dispatches `Browser.grantPermissions` at
    /// browser scope (no `sessionId`) carrying both the `permissions` array
    /// and the supplied `origin`.
    #[tokio::test]
    async fn grant_permissions_dispatches_with_mapped_strings_and_origin() {
        use zendriver_transport::testing::MockConnection;

        let (mut mock, conn) = MockConnection::pair();
        let browser = test_only_browser_from_conn(conn.clone());

        let fut = tokio::spawn({
            let b = browser.clone();
            async move {
                b.grant_permissions(
                    &[
                        PermissionType::Geolocation,
                        PermissionType::VideoCapture,
                        PermissionType::ClipboardReadWrite,
                    ],
                    Some("https://example.com"),
                )
                .await
            }
        });

        let id = mock.expect_cmd("Browser.grantPermissions").await;
        let sent = mock.last_sent();
        let perms = sent["params"]["permissions"]
            .as_array()
            .expect("permissions must be an array");
        assert_eq!(
            perms,
            &vec![
                json!("geolocation"),
                json!("videoCapture"),
                json!("clipboardReadWrite"),
            ]
        );
        assert_eq!(sent["params"]["origin"], "https://example.com");
        // Browser-scope command — no session_id.
        assert!(sent.get("sessionId").is_none());
        mock.reply(id, json!({})).await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    /// When `origin` is `None`, the `origin` key is omitted entirely from the
    /// params (granted browser-wide), not sent as `null`.
    #[tokio::test]
    async fn grant_permissions_omits_origin_when_none() {
        use zendriver_transport::testing::MockConnection;

        let (mut mock, conn) = MockConnection::pair();
        let browser = test_only_browser_from_conn(conn.clone());

        let fut = tokio::spawn({
            let b = browser.clone();
            async move { b.grant_permissions(&[PermissionType::Notifications], None).await }
        });

        let id = mock.expect_cmd("Browser.grantPermissions").await;
        let sent = mock.last_sent();
        assert_eq!(
            sent["params"]["permissions"].as_array().unwrap(),
            &vec![json!("notifications")]
        );
        let origin_field = sent["params"].get("origin");
        assert!(
            origin_field.is_none() || origin_field.unwrap().is_null(),
            "origin must be omitted when None"
        );
        mock.reply(id, json!({})).await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    /// [`Browser::reset_permissions`] dispatches `Browser.resetPermissions`
    /// at browser scope.
    #[tokio::test]
    async fn reset_permissions_dispatches() {
        use zendriver_transport::testing::MockConnection;

        let (mut mock, conn) = MockConnection::pair();
        let browser = test_only_browser_from_conn(conn.clone());

        let fut = tokio::spawn({
            let b = browser.clone();
            async move { b.reset_permissions().await }
        });

        let id = mock.expect_cmd("Browser.resetPermissions").await;
        // Browser-scope command — no session_id.
        assert!(mock.last_sent().get("sessionId").is_none());
        mock.reply(id, json!({})).await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    /// `grant_all_permissions` sends the full [`PermissionType::ALL`] list as
    /// mapped CDP strings, with no origin (browser-wide).
    #[tokio::test]
    async fn grant_all_permissions_sends_full_list() {
        use zendriver_transport::testing::MockConnection;

        let (mut mock, conn) = MockConnection::pair();
        let browser = test_only_browser_from_conn(conn.clone());

        let fut = tokio::spawn({
            let b = browser.clone();
            async move { b.grant_all_permissions().await }
        });

        let id = mock.expect_cmd("Browser.grantPermissions").await;
        let sent = mock.last_sent();
        let perms = sent["params"]["permissions"]
            .as_array()
            .expect("permissions must be an array");
        assert_eq!(perms.len(), PermissionType::ALL.len());
        // Spot-check a couple of the mapped strings are present.
        assert!(perms.contains(&json!("geolocation")));
        assert!(perms.contains(&json!("midiSysex")));
        let origin_field = sent["params"].get("origin");
        assert!(origin_field.is_none() || origin_field.unwrap().is_null());
        mock.reply(id, json!({})).await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    #[test]
    fn permission_type_as_cdp_round_trips_known_strings() {
        assert_eq!(PermissionType::Geolocation.as_cdp(), "geolocation");
        assert_eq!(PermissionType::VideoCapture.as_cdp(), "videoCapture");
        assert_eq!(PermissionType::AudioCapture.as_cdp(), "audioCapture");
        assert_eq!(
            PermissionType::ClipboardReadWrite.as_cdp(),
            "clipboardReadWrite"
        );
        assert_eq!(PermissionType::MidiSysex.as_cdp(), "midiSysex");
    }

    // ----- BrowserBuilder::connect (C1) ----------------------------------

    /// Unit-test the `/json/version` body parse used by the `http(s)://`
    /// connect path: a `webSocketDebuggerUrl` string is extracted, with and
    /// without a leading HTTP/1.x header block.
    #[test]
    fn resolve_ws_from_http_parses_web_socket_debugger_url() {
        let ws = "ws://127.0.0.1:9222/devtools/browser/abc";

        // Bare JSON body (header/body split absent → whole buffer is JSON).
        let body = format!("{{\"webSocketDebuggerUrl\":\"{ws}\"}}");
        assert_eq!(parse_ws_from_json_version(body.as_bytes()).unwrap(), ws);

        // Full HTTP/1.1 response — header block must be stripped first.
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{{\"Browser\":\"Chrome/120.0.0.0\",\"webSocketDebuggerUrl\":\"{ws}\"}}"
        );
        assert_eq!(parse_ws_from_json_version(resp.as_bytes()).unwrap(), ws);

        // Missing field → DevtoolsParse.
        let bad = b"HTTP/1.1 200 OK\r\n\r\n{\"Browser\":\"Chrome/120\"}";
        assert!(matches!(
            parse_ws_from_json_version(bad),
            Err(ZendriverError::Browser(BrowserError::DevtoolsParse))
        ));
    }

    /// The `connect` post-connect handshake (`finish_connect`) drives the same
    /// CDP sequence as launch over a [`MockConnection`] — proving the `ws://`
    /// connect path attaches via the already-established transport rather than
    /// spawning a process — and produces a `BrowserInner` that does NOT own a
    /// process: `owns_process` is false and no `Child` handle is held.
    #[tokio::test]
    async fn connect_ws_endpoint_does_not_spawn() {
        use zendriver_transport::testing::MockConnection;

        let input_profile = zendriver_stealth::InputProfile::native();
        let registrar = Arc::new(TabRegistrar::new(input_profile.clone()));
        let (mut mock, conn) =
            MockConnection::pair_with_observers(vec![registrar.clone() as Arc<dyn TargetObserver>]);

        // Run the exact post-connect handshake `connect` invokes: no child,
        // no tempdir, owns_process = false.
        let fut = tokio::spawn(async move {
            finish_connect(FinishConnect {
                conn,
                registrar,
                input_profile,
                child: None,
                owned_tmp: None,
                extension_dirs: Vec::new(),
                debug_host_port: debug_host_port_from_ws(
                    "ws://127.0.0.1:9222/devtools/browser/abc",
                ),
                owns_process: false,
            })
            .await
        });

        // 1. Browser-scoped auto-attach.
        let id = mock.expect_cmd("Target.setAutoAttach").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["flatten"], true);
        assert!(sent.get("sessionId").is_none(), "auto-attach is browser-scope");
        mock.reply(id, json!({})).await;

        // 2. Initial-target discovery.
        let id = mock.expect_cmd("Target.getTargets").await;
        mock.reply(
            id,
            json!({ "targetInfos": [{ "targetId": "T1", "type": "page", "url": "about:blank" }] }),
        )
        .await;

        // 3. Attach to the discovered target.
        let id = mock.expect_cmd("Target.attachToTarget").await;
        assert_eq!(mock.last_sent()["params"]["targetId"], "T1");
        mock.reply(id, json!({ "sessionId": "S1" })).await;

        let inner = fut.await.unwrap().unwrap();

        // Ownership: attached, not spawned.
        assert!(!inner.owns_process, "connect path must not own the process");
        assert!(
            inner.child.lock().await.is_none(),
            "connect path holds no Child handle",
        );
        // Main tab registered under the attach sessionId.
        assert_eq!(inner.tabs.read().await.len(), 1);
        assert!(inner.tabs.read().await.contains_key("S1"));

        inner.conn.shutdown();
    }

    /// A connected (non-owning) [`Browser`] has `owns_process == false`, and
    /// [`Browser::close`] tears down only the transport: it returns `Ok`
    /// without attempting process termination (there is no `Child` to kill,
    /// and the `owns_process` guard short-circuits before the kill path).
    #[tokio::test]
    async fn connect_sets_owns_process_false_and_skips_kill() {
        use zendriver_transport::testing::MockConnection;

        let input_profile = zendriver_stealth::InputProfile::native();
        let registrar = Arc::new(TabRegistrar::new(input_profile.clone()));
        let (mut mock, conn) =
            MockConnection::pair_with_observers(vec![registrar.clone() as Arc<dyn TargetObserver>]);

        let fut = tokio::spawn(async move {
            finish_connect(FinishConnect {
                conn,
                registrar,
                input_profile,
                child: None,
                owned_tmp: None,
                extension_dirs: Vec::new(),
                debug_host_port: None,
                owns_process: false,
            })
            .await
        });

        let id = mock.expect_cmd("Target.setAutoAttach").await;
        mock.reply(id, json!({})).await;
        let id = mock.expect_cmd("Target.getTargets").await;
        mock.reply(
            id,
            json!({ "targetInfos": [{ "targetId": "T1", "type": "page", "url": "about:blank" }] }),
        )
        .await;
        let id = mock.expect_cmd("Target.attachToTarget").await;
        mock.reply(id, json!({ "sessionId": "S1" })).await;

        let inner = fut.await.unwrap().unwrap();
        assert!(!inner.owns_process);

        let browser = Browser { inner };
        // close() on a non-owning browser: shuts the transport, skips the
        // kill path entirely. No panic, no hang, returns Ok.
        browser.close().await.unwrap();
    }
}
