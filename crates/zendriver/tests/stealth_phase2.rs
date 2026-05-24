//! Nightly stealth tests against real-internet sites.
//!
//! Gated behind `stealth-tests` feature (which also requires `integration-tests`).
//! Run in CI on cron `0 6 * * *`. Failures are not blocking (`continue-on-error: true`).

#![cfg(feature = "stealth-tests")]

use serial_test::serial;
use std::time::Duration;
use zendriver::Browser;
use zendriver::stealth::StealthProfile;

#[tokio::test]
#[serial]
async fn spoofed_passes_sannysoft_intoli_block() {
    let browser = Browser::builder()
        .stealth(StealthProfile::spoofed())
        .headless(true)
        .launch()
        .await
        .expect("launch");
    let tab = browser.main_tab();
    tab.goto("https://bot.sannysoft.com").await.expect("goto");
    tab.wait_for_load().await.expect("load");
    // Some sannysoft tests are async; give them a moment.
    tokio::time::sleep(Duration::from_secs(5)).await;

    let results: Vec<(String, bool)> = tab
        .evaluate_main(
            r#"
        Array.from(document.querySelectorAll('table tr')).map(tr => {
            const cells = tr.querySelectorAll('td');
            if (cells.length < 2) return null;
            const name = cells[0].textContent.trim();
            const bg = window.getComputedStyle(cells[1]).backgroundColor;
            // sannysoft uses green (passing) / red (failing) backgrounds.
            const passed = bg.includes('0, 255, 0') || bg.includes('128, 255, 0')
                        || bg.includes('0, 128, 0') || bg.includes('rgb(0, 255, 0)');
            return [name, passed];
        }).filter(x => x !== null)
    "#,
        )
        .await
        .expect("scrape");

    let intoli_test_names = [
        "User Agent",
        "WebDriver",
        "Chrome",
        "Permissions",
        "Plugins Length",
        "Languages",
        "WebGL Vendor",
        "WebGL Renderer",
        "Broken Image Dimensions",
    ];
    let intoli_failures: Vec<_> = results
        .iter()
        .filter(|(name, ok)| !ok && intoli_test_names.iter().any(|t| name.contains(t)))
        .map(|(name, _)| name.as_str())
        .collect();
    assert!(
        intoli_failures.is_empty(),
        "spoofed profile failed Intoli rows: {intoli_failures:?}"
    );
    browser.close().await.expect("close");
}

#[tokio::test]
#[serial]
async fn spoofed_passes_areyouheadless() {
    let browser = Browser::builder()
        .stealth(StealthProfile::spoofed())
        .headless(true)
        .launch()
        .await
        .expect("launch");
    let tab = browser.main_tab();
    tab.goto("https://arh.antoinevastel.com/bots/areyouheadless")
        .await
        .expect("goto");
    tab.wait_for_load().await.expect("load");
    tokio::time::sleep(Duration::from_secs(2)).await;
    let result: String = tab
        .evaluate_main("document.querySelector('#res').textContent")
        .await
        .expect("scrape");
    assert!(
        result.contains("not Chrome headless"),
        "areyouheadless flagged us: {result}"
    );
    browser.close().await.expect("close");
}

#[tokio::test]
#[serial]
async fn spoofed_passes_intoli_basic_test() {
    let browser = Browser::builder()
        .stealth(StealthProfile::spoofed())
        .headless(true)
        .launch()
        .await
        .expect("launch");
    let tab = browser.main_tab();
    tab.goto(
        "https://intoli.com/blog/not-possible-to-block-chrome-headless/chrome-headless-test.html",
    )
    .await
    .expect("goto");
    tab.wait_for_load().await.expect("load");
    tokio::time::sleep(Duration::from_secs(3)).await;

    let results: Vec<(String, String)> = tab
        .evaluate_main(
            r#"
        Array.from(document.querySelectorAll('#results tr')).map(tr => {
            const cells = tr.querySelectorAll('td');
            if (cells.length < 2) return null;
            return [cells[0].textContent.trim(), cells[1].textContent.trim()];
        }).filter(x => x !== null)
    "#,
        )
        .await
        .expect("scrape");

    let fails: Vec<_> = results
        .iter()
        .filter(|(_, status)| status.to_lowercase().contains("fail"))
        .collect();
    assert!(fails.is_empty(), "intoli basic test fails: {fails:?}");
    browser.close().await.expect("close");
}

#[tokio::test]
#[serial]
async fn native_fails_sannysoft_navigator_webdriver_but_passes_user_agent() {
    // Opposite-direction assertion: the native profile honors its
    // "no JS patches" contract while still scrubbing the headless UA.
    let browser = Browser::builder()
        .stealth(StealthProfile::native())
        .headless(true)
        .launch()
        .await
        .expect("launch");
    let tab = browser.main_tab();
    tab.goto("https://bot.sannysoft.com").await.expect("goto");
    tab.wait_for_load().await.expect("load");
    tokio::time::sleep(Duration::from_secs(5)).await;

    // UA row should pass (HeadlessChrome scrubbed).
    let ua: String = tab.evaluate_main("navigator.userAgent").await.expect("ua");
    assert!(
        !ua.contains("HeadlessChrome"),
        "native profile must scrub UA: got {ua}"
    );

    // WebDriver row should fail (native applies no JS patch for webdriver).
    let wd: bool = tab.evaluate_main("navigator.webdriver").await.expect("wd");
    assert!(
        wd,
        "native profile must NOT hide webdriver (would defeat the 'no JS' contract)"
    );

    browser.close().await.expect("close");
}
