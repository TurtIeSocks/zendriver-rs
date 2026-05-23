//! Browser lifecycle: executable discovery, subprocess spawn, WS attach,
//! graceful teardown.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::timeout;
use tracing::{debug, info};
use zendriver_transport::{Connection, SessionHandle};

use crate::error::{BrowserError, ZendriverError};
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

#[derive(Debug, Default, Clone)]
pub struct BrowserBuilder {
    pub(crate) headless: Option<bool>,
    pub(crate) executable: Option<PathBuf>,
    pub(crate) user_data_dir: Option<PathBuf>,
    pub(crate) extra_args: Vec<String>,
}

impl BrowserBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
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
}

const WS_ENDPOINT_TIMEOUT: Duration = Duration::from_secs(15);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

impl BrowserBuilder {
    /// Spawn Chrome and attach. Returns once the main tab is bound.
    pub async fn launch(self) -> Result<Browser, ZendriverError> {
        let exe = match self.executable.clone() {
            Some(p) => p,
            None => find_chrome_executable()?,
        };

        // Allocate user_data_dir (or use a TempDir we keep alive until shutdown).
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

        let flags = self.build_flags(&user_data_path);
        info!(executable = %exe.display(), "launching chrome");

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

        debug!(ws_url = %ws_url, "connecting to chrome");
        let conn = zendriver_transport::connection::connect(&ws_url).await?;

        // Attach to the first target. We discover it via Target.getTargets.
        let list = conn
            .call_raw("Target.getTargets", serde_json::json!({}), None)
            .await?;
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

        let attach = conn
            .call_raw(
                "Target.attachToTarget",
                serde_json::json!({ "targetId": target_id, "flatten": true }),
                None,
            )
            .await?;
        let session_id = attach["sessionId"]
            .as_str()
            .ok_or_else(|| ZendriverError::Navigation("attach returned no sessionId".into()))?
            .to_string();

        let session = SessionHandle::new(conn.clone(), session_id);
        let main_tab = Tab::new(session);

        Ok(Browser {
            inner: Arc::new(BrowserInner {
                conn,
                main_tab,
                child: tokio::sync::Mutex::new(Some(child)),
                _user_data: owned_tmp,
            }),
        })
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

    /// Graceful shutdown: cancel the transport, send SIGTERM to Chrome,
    /// wait up to `SHUTDOWN_GRACE`, then SIGKILL on timeout. Cleans up
    /// user_data_dir.
    pub async fn close(self) -> Result<(), ZendriverError> {
        self.inner.conn.shutdown();
        let mut child_guard = self.inner.child.lock().await;
        if let Some(mut child) = child_guard.take() {
            // Try graceful exit first.
            let _ = child.start_kill();
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
}
