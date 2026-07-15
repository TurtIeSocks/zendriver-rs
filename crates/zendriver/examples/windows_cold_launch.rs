//! Cold-launch harness for the Windows "blank window, never navigates" stall.
//!
//! Mirrors zeus's `create_context` path — headed + `StealthProfile::native()` —
//! because stealth is what pulls in the Chrome version probe, and the probe is
//! what has historically wedged the launch future. It deliberately does NOT
//! light a pilot-light: the pilot-light is a mitigation that warms away the very
//! symptom being measured.
//!
//! Run it on a COLD box (no `chrome.exe` anywhere) — that is the whole point:
//!
//! ```powershell
//! Get-Process chrome -ErrorAction SilentlyContinue   # must be empty first
//! cargo run --example windows_cold_launch
//! ```
//!
//! Each phase is timed and printed as `PHASE <name> <ms>`. A phase that never
//! prints is the one that hung, which is the measurement this exists to take:
//! the failure mode under investigation is a *silent* hang, so the last line
//! printed localises the block to a specific span of the launch path.
//!
//! `--stealth=false` isolates whether the stall lives in the stealth/fingerprint
//! path or in the raw launch itself.

use std::time::{Duration, Instant};

use zendriver::{Browser, stealth::StealthProfile};

/// Well above `HANDSHAKE_TIMEOUT` (30s) and `WS_ENDPOINT_TIMEOUT` (15s), so a
/// working guard reports *itself* rather than being pre-empted by this backstop.
/// If this fires, every in-crate guard failed to — which is itself the finding.
const OUTER_BUDGET: Duration = Duration::from_secs(120);

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "zendriver=debug,info".into()),
        )
        .with_target(true)
        .init();

    let use_stealth = !std::env::args().any(|a| a == "--stealth=false");
    let url = std::env::args()
        .find_map(|a| a.strip_prefix("--url=").map(str::to_string))
        .unwrap_or_else(|| "https://example.com".to_string());

    println!("== cold-launch harness ==");
    println!("stealth : {use_stealth}");
    println!("url     : {url}");
    println!("headless: false (mandatory — a headed window is the deliverable)");

    let t0 = Instant::now();

    // Phase 1 — build. Stealth's fingerprint probe runs inside `launch()`, not
    // here, so a hang in the probe still lands in the launch phase below.
    let mut builder = Browser::builder().headless(false);
    if use_stealth {
        builder = builder.stealth(StealthProfile::native());
    }
    builder = builder
        .arg("--no-first-run")
        .arg("--no-default-browser-check");
    println!("PHASE build {}ms", t0.elapsed().as_millis());

    // Phase 2 — launch. This is where the stall has always lived: process spawn,
    // the stderr `DevTools listening on ws://` read, the WS dial, and
    // `finish_connect`'s Target.* handshake.
    let t_launch = Instant::now();
    println!(">> launching (outer budget {OUTER_BUDGET:?}) ...");
    let browser = match tokio::time::timeout(OUTER_BUDGET, builder.launch()).await {
        Ok(Ok(b)) => {
            println!("PHASE launch {}ms", t_launch.elapsed().as_millis());
            b
        }
        Ok(Err(e)) => {
            // A *named* error is a good outcome: it means a guard fired and the
            // hang is no longer silent.
            println!("FAIL launch {}ms — {e}", t_launch.elapsed().as_millis());
            std::process::exit(2);
        }
        Err(_) => {
            // The outer budget firing means no in-crate guard did. If the
            // process also refuses to exit here, the launch future is wedged on
            // a blocking call rather than parked on an await.
            println!(
                "HUNG launch — outer budget {OUTER_BUDGET:?} expired with no in-crate guard firing"
            );
            std::process::exit(3);
        }
    };

    // Phase 3 — main_tab. On a cold Chrome the initial target may not exist yet
    // when the handshake enumerates targets; if this is where it dies, the race
    // is in target discovery, not navigation.
    let t_tab = Instant::now();
    let tab = browser.main_tab();
    println!("PHASE main_tab {}ms", t_tab.elapsed().as_millis());

    // Phase 4 — navigate. The reported symptom is precisely "window opens, never
    // navigates", so this is the phase that reproduces it.
    let t_goto = Instant::now();
    println!(">> navigating to {url} ...");
    match tokio::time::timeout(OUTER_BUDGET, tab.goto(&url)).await {
        Ok(Ok(_)) => println!("PHASE goto {}ms", t_goto.elapsed().as_millis()),
        Ok(Err(e)) => {
            println!("FAIL goto {}ms — {e}", t_goto.elapsed().as_millis());
            let _ = browser.close().await;
            std::process::exit(4);
        }
        Err(_) => {
            println!("HUNG goto — outer budget expired (this IS the reported symptom)");
            std::process::exit(5);
        }
    }

    let t_load = Instant::now();
    match tokio::time::timeout(Duration::from_secs(30), tab.wait_for_load()).await {
        Ok(Ok(())) => println!("PHASE wait_for_load {}ms", t_load.elapsed().as_millis()),
        Ok(Err(e)) => println!("WARN wait_for_load — {e}"),
        Err(_) => println!("WARN wait_for_load timed out at 30s"),
    }

    // Prove the page is real. A window that is "open" but not navigated still
    // yields a blank/about:blank URL here, so this is what distinguishes a true
    // success from the symptom.
    match tab.evaluate::<String>("document.location.href").await {
        Ok(v) => println!("OK url={v:?}"),
        Err(e) => println!("WARN evaluate — {e}"),
    }
    match tab.evaluate::<String>("document.title").await {
        Ok(v) => println!("OK title={v:?}"),
        Err(e) => println!("WARN evaluate title — {e}"),
    }

    // Phase 5 — close. Verify with `Get-Process chrome` afterwards; anything
    // surviving is an orphan the Job Object should have reaped.
    let t_close = Instant::now();
    match tokio::time::timeout(Duration::from_secs(30), browser.close()).await {
        Ok(Ok(())) => println!("PHASE close {}ms", t_close.elapsed().as_millis()),
        Ok(Err(e)) => println!("FAIL close {}ms — {e}", t_close.elapsed().as_millis()),
        Err(_) => println!("HUNG close — 30s expired"),
    }

    println!("== done in {}ms ==", t0.elapsed().as_millis());
}
