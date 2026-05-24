//! Phase 2 end-to-end stealth tests against real Chrome + wiremock.

#![cfg(feature = "integration-tests")]

use serial_test::serial;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use zendriver::Browser;
use zendriver::stealth::StealthProfile;

async fn fixture_with_html(html: &str) -> MockServer {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(html.as_bytes().to_vec(), "text/html"),
        )
        .mount(&mock)
        .await;
    mock
}

#[tokio::test]
#[serial]
async fn spoofed_profile_patches_navigator_webdriver_to_false() {
    let mock = fixture_with_html("<!doctype html><body>hello</body>").await;
    let browser = Browser::builder()
        .stealth(StealthProfile::spoofed())
        .launch()
        .await
        .unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();
    let wd: bool = tab.evaluate_main("navigator.webdriver").await.unwrap();
    assert!(!wd, "spoofed profile must hide webdriver");
    browser.close().await.unwrap();
}

#[tokio::test]
#[serial]
async fn native_profile_does_not_patch_navigator_webdriver() {
    let mock = fixture_with_html("<!doctype html><body>hello</body>").await;
    let browser = Browser::builder()
        .stealth(StealthProfile::native())
        .launch()
        .await
        .unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();
    let wd: bool = tab.evaluate_main("navigator.webdriver").await.unwrap();
    assert!(wd, "native profile leaves webdriver alone");
    browser.close().await.unwrap();
}

#[tokio::test]
#[serial]
async fn ua_string_no_longer_contains_headless_under_native() {
    let mock = fixture_with_html("<!doctype html><body>hello</body>").await;
    let browser = Browser::builder()
        .stealth(StealthProfile::native())
        .launch()
        .await
        .unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();
    let ua: String = tab.evaluate_main("navigator.userAgent").await.unwrap();
    assert!(!ua.contains("HeadlessChrome"), "got: {ua}");
    browser.close().await.unwrap();
}

#[tokio::test]
#[serial]
async fn isolated_world_eval_does_not_see_page_globals() {
    let mock = fixture_with_html(
        r#"
        <!doctype html><script>window.evil = "should not be visible";</script>
    "#,
    )
    .await;
    let browser = Browser::builder()
        .stealth(StealthProfile::off())
        .launch()
        .await
        .unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();
    let v: Option<String> = tab
        .evaluate("typeof window.evil === 'undefined' ? null : window.evil")
        .await
        .unwrap();
    assert_eq!(v, None, "isolated world should NOT see window.evil");
    let v: String = tab.evaluate_main("window.evil").await.unwrap();
    assert_eq!(
        v, "should not be visible",
        "main world DOES see window.evil"
    );
    browser.close().await.unwrap();
}
