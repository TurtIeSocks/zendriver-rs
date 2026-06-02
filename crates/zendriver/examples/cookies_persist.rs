//! Persist + rehydrate the browser cookie store across two [`Browser`]
//! lifetimes — the P4 cookie-jar round-trip.
//!
//! Sequence:
//!   1. Launch a browser, set two cookies on `example.test` (one Lax, one
//!      `same_site = None` to exercise both serialization paths through the
//!      CDP camelCase boundary).
//!   2. `save_to_file` the jar to a tempfile, close the browser.
//!   3. Launch a fresh browser, `load_from_file`, assert both cookies came
//!      back via `CookieJar::all`.
//!
//! Demonstrates:
//!   - [`Browser::cookies`] (browser-scoped jar — cookies are shared across
//!     every tab, so there's no per-tab variant).
//!   - [`CookieJar::set`], [`CookieJar::all`].
//!   - [`CookieJar::save_to_file`] / [`CookieJar::load_from_file`].
//!
//! Real-world flow: save once after a login, then reuse the file as a
//! login-bypass session token across runs.

use std::collections::HashSet;

use zendriver::{Browser, Cookie, SameSite};

#[tokio::main]
#[allow(clippy::result_large_err)] // example boundary; users wrap in their own Error
async fn main() -> zendriver::Result<()> {
    tracing_subscriber::fmt::init();

    let tmp = tempfile::NamedTempFile::new()?;
    let path = tmp.path().to_path_buf();

    // ---- Session 1: set + save ----
    {
        let browser = Browser::builder().headless(true).launch().await?;
        let jar = browser.cookies();

        jar.set(Cookie {
            name: "saved_a".into(),
            value: "v1".into(),
            domain: "example.test".into(),
            path: "/".into(),
            same_site: Some(SameSite::Lax),
            ..Default::default()
        })
        .await?;

        jar.set(Cookie {
            name: "saved_b".into(),
            value: "v2".into(),
            domain: "example.test".into(),
            path: "/api".into(),
            http_only: true,
            ..Default::default()
        })
        .await?;

        jar.save_to_file(&path).await?;
        println!(
            "saved {} cookies to {}",
            jar.all().await?.len(),
            path.display()
        );

        browser.close().await?;
    }

    // ---- Session 2: load + verify ----
    {
        let browser = Browser::builder().headless(true).launch().await?;
        let jar = browser.cookies();

        // Sanity: a fresh browser has no example.test cookies yet.
        let pristine: HashSet<String> = jar
            .all()
            .await?
            .into_iter()
            .filter(|c| c.domain == "example.test")
            .map(|c| c.name)
            .collect();
        assert!(
            pristine.is_empty(),
            "fresh browser should not have example.test cookies, got {pristine:?}"
        );

        jar.load_from_file(&path).await?;
        let names: HashSet<String> = jar.all().await?.into_iter().map(|c| c.name).collect();
        println!("loaded cookies: {names:?}");
        assert!(names.contains("saved_a"), "saved_a missing after load");
        assert!(names.contains("saved_b"), "saved_b missing after load");

        browser.close().await?;
    }

    println!("cookies round-tripped across two browser lifetimes");
    Ok(())
}
