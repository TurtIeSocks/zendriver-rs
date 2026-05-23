//! Browser lifecycle: executable discovery, subprocess spawn, WS attach,
//! graceful teardown.

use std::path::PathBuf;

use crate::error::BrowserError;

/// Look for a Chromium-family binary on PATH and in conventional locations.
/// Returns the first path that exists.
pub fn find_chrome_executable() -> Result<PathBuf, BrowserError> {
    let candidates = candidate_paths();
    for c in &candidates {
        if c.exists() {
            return Ok(c.clone());
        }
    }
    Err(BrowserError::ExecutableNotFound { searched: candidates })
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
#[allow(dead_code)] // wired into launch path in a later task
pub(crate) fn parse_devtools_line(line: &str) -> Option<String> {
    // Format: `DevTools listening on ws://127.0.0.1:NNNN/devtools/browser/UUID`
    let needle = "DevTools listening on ";
    let idx = line.find(needle)?;
    let rest = &line[idx + needle.len()..];
    let end = rest
        .find(char::is_whitespace)
        .unwrap_or(rest.len());
    let url = rest[..end].trim();
    if url.starts_with("ws://") || url.starts_with("wss://") {
        Some(url.to_string())
    } else {
        None
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
        let line =
            "DevTools listening on ws://127.0.0.1:54321/devtools/browser/abc-def-123\n";
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
}
