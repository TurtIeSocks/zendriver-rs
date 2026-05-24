//! v0.1.0 pre-release regression locks for upstream Python zendriver bugs.
//!
//! Each test in this file pins a behavior that broke in the Python port and
//! that the Rust port's architecture is meant to make impossible. They are
//! verification tests, not feature tests — if they regress, the architectural
//! win has been lost and the fix belongs at the design layer, not by relaxing
//! the assertion.
//!
//! Gated behind `integration-tests` because each test launches real Chrome.
//! Test 2 is additionally Unix-only because its process-leak audit shells out
//! to `ps`.
//!
//! Coverage map:
//! - `ua_override_persists_across_new_tabs` → cdpdriver/zendriver#107
//! - `repeated_launch_close_leaves_no_orphan_chrome_processes` → #33, #198
//! - `browser_close_during_inflight_cdp_calls_does_not_hang` → #89

#![cfg(feature = "integration-tests")]
#![allow(clippy::panic, clippy::unwrap_used)]

use std::time::Duration;

use serial_test::serial;
use zendriver::Browser;
use zendriver::stealth::StealthProfile;

/// cdpdriver/zendriver#107: in headless mode, a UA override set on the main
/// tab failed to propagate to tabs opened after launch. The Rust port wires
/// the UA override through `StealthObserver`, which attaches per-target — so
/// every new tab should see the same UA without any per-tab re-application
/// by the caller.
#[tokio::test]
#[serial]
async fn ua_override_persists_across_new_tabs() {
    const UA: &str = "ZendriverPortUA/0.1";

    let browser = Browser::builder()
        .headless(true)
        .stealth(StealthProfile::native().user_agent(UA))
        .launch()
        .await
        .expect("launch");

    let main = browser.main_tab();
    let main_ua: String = main
        .evaluate("navigator.userAgent")
        .await
        .expect("read UA on main tab");
    assert_eq!(
        main_ua, UA,
        "main tab must report the overridden UA, got {main_ua:?}"
    );

    for i in 0..3 {
        let tab = browser
            .new_tab()
            .await
            .unwrap_or_else(|e| panic!("open new tab #{i}: {e}"));
        let ua: String = tab
            .evaluate("navigator.userAgent")
            .await
            .unwrap_or_else(|e| panic!("read UA on new tab #{i}: {e}"));
        assert_eq!(
            ua, UA,
            "new tab #{i} must inherit overridden UA, got {ua:?}"
        );
    }

    browser.close().await.expect("close");
}

/// Snapshot all live `chrome` / `chromium` PIDs on the system. Unix-only —
/// shells out to `ps` (which Windows lacks). Used by the leak audit below to
/// diff the set before/after a repeated launch+close loop.
#[cfg(unix)]
fn ps_chrome_pids() -> std::collections::HashSet<u32> {
    use std::process::Command;
    let out = Command::new("ps")
        .args(["axo", "pid,comm"])
        .output()
        .expect("ps");
    let s = String::from_utf8_lossy(&out.stdout);
    s.lines()
        .filter(|l| {
            let lower = l.to_lowercase();
            lower.contains("chrome") || lower.contains("chromium")
        })
        .filter_map(|l| l.split_whitespace().next()?.parse::<u32>().ok())
        .collect()
}

/// cdpdriver/zendriver#33 + #198: long loops of launch/close in the Python
/// port accumulated zombie Chrome processes until the host OOMed. The Rust
/// port pairs `Browser::close` (graceful SIGTERM-then-SIGKILL) with a `Drop`
/// impl that relies on `kill_on_drop(true)` for panic safety. This test
/// asserts the combination reaps cleanly across many cycles by diffing the
/// system-wide `chrome` PID set.
///
/// 25 cycles is enough to surface a single-process-per-iteration leak
/// without making CI brutal. Unix-only because the audit shells out to `ps`.
#[cfg(unix)]
#[tokio::test]
#[serial]
async fn repeated_launch_close_leaves_no_orphan_chrome_processes() {
    let before = ps_chrome_pids();

    for i in 0..25 {
        let browser = Browser::builder()
            .headless(true)
            .launch()
            .await
            .unwrap_or_else(|e| panic!("launch iter {i}: {e}"));
        // Best-effort navigation; about:blank should never fail, but if it
        // does we still want the close path to run so we don't poison the
        // leak count with our own failed-test process.
        let _ = browser.main_tab().goto("about:blank").await;
        browser
            .close()
            .await
            .unwrap_or_else(|e| panic!("close iter {i}: {e}"));
    }

    // Stragglers — Chrome's helper processes can take a moment to be reaped
    // by the kernel after the parent exits.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let after = ps_chrome_pids();
    let leaked: Vec<u32> = after.difference(&before).copied().collect();
    assert!(
        leaked.is_empty(),
        "{} Chrome PIDs leaked after 25 launch/close cycles: {:?}",
        leaked.len(),
        leaked,
    );
}

/// cdpdriver/zendriver#89: `tab.send(...)` in the Python port could hang
/// indefinitely if the browser closed while a CDP call was in flight — the
/// pending future was never resolved. The Rust port uses a transport actor
/// that signals shutdown to every in-flight call, so each pending future
/// MUST resolve (Ok or Err) once `Browser::close` runs. This test races N
/// long-promise evaluations against an early close and asserts none of them
/// hang past a generous 5s timeout.
#[tokio::test]
#[serial]
async fn browser_close_during_inflight_cdp_calls_does_not_hang() {
    use std::sync::Arc;
    use tokio::time::timeout;

    const N: usize = 10;

    let browser = Browser::builder()
        .headless(true)
        .launch()
        .await
        .expect("launch");

    // Drive the main tab to a real (non about:blank) execution context so
    // the isolated-world resolver has a stable target.
    let main = browser.main_tab();
    main.goto("about:blank").await.expect("goto");

    // Shared so closures captured by tasks see the same handle without
    // requiring `'static` lifetimes against a stack-local clone vec.
    let tab = Arc::new(main);

    let mut handles = Vec::with_capacity(N);
    for i in 0..N {
        let tab = Arc::clone(&tab);
        handles.push(tokio::spawn(async move {
            // 1s server-side delay. Some calls finish before close, others
            // race the close; both outcomes are valid — the contract is
            // "resolves, doesn't hang", not "succeeds".
            let result: Result<serde_json::Value, _> = tab
                .evaluate("new Promise(r => setTimeout(() => r(42), 1000))")
                .await;
            (i, result)
        }));
    }

    // Brief headstart so the calls are demonstrably in flight when close
    // lands, not racing the spawn loop.
    tokio::time::sleep(Duration::from_millis(50)).await;
    browser.close().await.expect("close");

    let mut hung = Vec::new();
    for h in handles {
        match timeout(Duration::from_secs(5), h).await {
            Ok(Ok((_i, _call_result))) => {
                // Either Ok(_) (raced and won) or Err(Transport/Cdp) (raced
                // and lost) is fine — both are "resolved, not hung".
            }
            Ok(Err(join_err)) => {
                panic!("task panicked while awaiting in-flight call: {join_err}");
            }
            Err(_elapsed) => {
                hung.push(());
            }
        }
    }

    assert!(
        hung.is_empty(),
        "{} of {} in-flight CDP calls hung past close (5s timeout)",
        hung.len(),
        N,
    );
}
