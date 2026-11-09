//! Real-Chrome regressions for the font allow-list's observable behavior.
#![cfg(feature = "integration-tests")]

use std::collections::BTreeMap;
use std::time::Duration;

use zendriver::stealth::StealthProfile;
use zendriver::{Browser, Persona};
use zendriver_stealth::FontSpec;

#[tokio::test]
async fn font_allow_list_preserves_generic_families() {
    let browser = Browser::builder()
        .headless(true)
        .stealth(StealthProfile::spoofed())
        .persona(Persona {
            fonts: Some(FontSpec {
                available: Some(vec!["Arial".into()]),
                ..FontSpec::default()
            }),
            ..Persona::default()
        })
        .launch()
        .await
        .unwrap();
    let tab = browser.main_tab();
    tab.goto("about:blank").await.unwrap();
    tab.wait_for_load().await.unwrap();

    let results: BTreeMap<String, bool> = tab
        .evaluate_main(
            r#"Object.fromEntries([
                'Verdana', 'serif', 'sans-serif', 'monospace', 'cursive',
                'fantasy', 'system-ui', 'ui-serif', 'ui-sans-serif',
                'ui-monospace', 'ui-rounded', 'math', 'emoji', 'fangsong'
            ].map(family => [family, document.fonts.check('12px ' + family)]))"#,
        )
        .await
        .unwrap();
    browser.close().await.unwrap();

    assert_eq!(results.len(), 14);
    assert!(!results["Verdana"], "the font allow-list must be active");
    for (family, visible) in results {
        if family != "Verdana" {
            assert!(visible, "generic family must remain visible: {family}");
        }
    }
}

#[tokio::test]
async fn font_allow_list_preserves_native_syntax_errors() {
    let browser = Browser::builder()
        .headless(true)
        .stealth(StealthProfile::spoofed())
        .persona(Persona {
            fonts: Some(FontSpec {
                available: Some(vec!["Arial".into()]),
                ..FontSpec::default()
            }),
            ..Persona::default()
        })
        .launch()
        .await
        .unwrap();
    let tab = browser.main_tab();
    tab.goto("about:blank").await.unwrap();
    tab.wait_for_load().await.unwrap();

    // Wait for the allow-list to take effect so native behavior alone cannot
    // make the syntax-error assertions pass before the patch is installed.
    let mut patch_active = false;
    for _ in 0..20 {
        let visible: bool = tab
            .evaluate_main("document.fonts.check('12px Verdana')")
            .await
            .unwrap();
        if !visible {
            patch_active = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let results: BTreeMap<String, String> = tab
        .evaluate_main(
            r#"Object.fromEntries([
                'inherit', 'initial', 'unset', 'revert', 'revert-layer',
                '  InHeRiT  ', '', 'not-a-font'
            ].map(font => {
                try { return [font, String(document.fonts.check(font))]; }
                catch (error) { return [font, error.name]; }
            }))"#,
        )
        .await
        .unwrap();
    browser.close().await.unwrap();

    assert!(patch_active, "the font allow-list never became active");
    assert_eq!(results.len(), 8);
    for (font, result) in results {
        assert_eq!(result, "SyntaxError", "invalid font: {font:?}");
    }
}
