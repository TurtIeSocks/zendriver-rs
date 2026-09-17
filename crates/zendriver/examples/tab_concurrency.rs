//! Rough benchmark for many tabs driven at once.
//!
//! `cargo run --release --example tab_concurrency -- <commands|loads> <tabs>`
//!
//! Prints one JSON line. Build it on two commits to compare them.

use std::time::{Duration, Instant};

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use zendriver::{Browser, Tab, ZendriverError};

const EVALS_PER_TAB: usize = 2000;
const LOADS_PER_TAB: usize = 5;
const FETCHES_PER_LOAD: usize = 50;
const QUIET_WINDOW: Duration = Duration::from_millis(100);
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
// Longer than the quiet window, so an idle wait that misses network events
// returns before this finishes and gets counted as `early`.
const SLOW_FETCH: Duration = Duration::from_millis(300);

#[derive(Default)]
struct TabResult {
    latencies: Vec<Duration>,
    timeouts: usize,
    early: usize,
}

#[tokio::main]
#[allow(clippy::result_large_err)] // example boundary; users wrap in their own Error
async fn main() -> zendriver::Result<()> {
    let mut args = std::env::args().skip(1);
    let mode = args.next().unwrap_or_else(|| "commands".to_string());
    let tab_count: usize = args.next().and_then(|n| n.parse().ok()).unwrap_or(4);

    let browser = Browser::builder().headless(true).launch().await?;
    let mut tabs = vec![browser.main_tab()];
    for _ in 1..tab_count {
        tabs.push(browser.new_tab().await?);
    }
    let root_sockets = tabs.iter().filter(|t| t.session().is_root()).count();

    let started = Instant::now();
    let handles: Vec<_> = tabs
        .iter()
        .cloned()
        .map(|tab| {
            let mode = mode.clone();
            tokio::spawn(async move {
                match mode.as_str() {
                    "loads" => run_loads(tab).await,
                    _ => run_commands(tab).await,
                }
            })
        })
        .collect();

    let mut total = TabResult::default();
    for handle in handles {
        let result = handle.await.expect("tab task panicked")?;
        total.latencies.extend(result.latencies);
        total.timeouts += result.timeouts;
        total.early += result.early;
    }
    let wall = started.elapsed();

    total.latencies.sort();
    let pct = |p: usize| {
        total
            .latencies
            .get((total.latencies.len() * p / 100).min(total.latencies.len().saturating_sub(1)))
            .map_or(0.0, |d| d.as_secs_f64() * 1000.0)
    };
    println!(
        "{}",
        json!({
            "mode": mode,
            "tabs": tab_count,
            "root_sockets": root_sockets,
            "wall_ms": wall.as_millis(),
            "ops": total.latencies.len(),
            "p50_ms": pct(50),
            "p99_ms": pct(99),
            "max_ms": pct(100),
            "timeouts": total.timeouts,
            "early": total.early,
        })
    );

    browser.close().await?;
    Ok(())
}

#[allow(clippy::result_large_err)]
async fn run_commands(tab: Tab) -> zendriver::Result<TabResult> {
    let _: usize = tab.evaluate("0").await?;
    let mut result = TabResult::default();
    for i in 0..EVALS_PER_TAB {
        let started = Instant::now();
        let echoed: usize = tab.evaluate(i.to_string()).await?;
        assert_eq!(echoed, i);
        result.latencies.push(started.elapsed());
    }
    Ok(result)
}

#[allow(clippy::result_large_err)]
async fn run_loads(tab: Tab) -> zendriver::Result<TabResult> {
    // One server per tab. Chrome caps connections per host:port, and a shared
    // port would make that pool the bottleneck instead of the transport.
    let server = MockServer::start().await;
    let page = format!(
        "<!doctype html><script>
        window.__done = 0;
        const urls = Array.from({{ length: {FETCHES_PER_LOAD} }}, (_, i) => '/r?i=' + i);
        urls.push('/slow');
        for (const url of urls) {{
          fetch(url + (url.includes('?') ? '&' : '?') + 'n=' + Math.random(), {{ cache: 'no-store' }})
            .then(r => r.text())
            .then(() => {{ window.__done++; }});
        }}
        </script>"
    );
    Mock::given(method("GET"))
        .and(path("/page"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(page, "text/html"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/r"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("ok")
                .set_delay(SLOW_FETCH),
        )
        .mount(&server)
        .await;
    let url = format!("{}/page", server.uri());

    // Warm-up load, not measured: enables the domains and the idle tracker.
    tab.goto(&url).await?;
    tab.wait_for_idle_with(IDLE_TIMEOUT, QUIET_WINDOW).await?;

    let mut result = TabResult::default();
    for _ in 0..LOADS_PER_TAB {
        let started = Instant::now();
        tab.goto(&url).await?;
        match tab.wait_for_idle_with(IDLE_TIMEOUT, QUIET_WINDOW).await {
            Ok(()) => result.latencies.push(started.elapsed()),
            Err(ZendriverError::Timeout(_)) => {
                result.timeouts += 1;
                continue;
            }
            Err(err) => return Err(err),
        }
        let done: usize = tab.evaluate_main("window.__done").await?;
        if done != FETCHES_PER_LOAD + 1 {
            result.early += 1;
        }
    }
    Ok(result)
}
