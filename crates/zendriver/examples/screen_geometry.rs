//! Does the geometry patch follow the CALLER's resolution, or assert its own?
//!
//! `cargo run -p zendriver --example screen_geometry -- 1366 768`
use zendriver::{Browser, Persona, stealth::StealthProfile};
use zendriver_stealth::ScreenSpec;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut a = std::env::args().skip(1);
    let w: u32 = a.next().and_then(|v| v.parse().ok()).unwrap_or(1366);
    let h: u32 = a.next().and_then(|v| v.parse().ok()).unwrap_or(768);

    let browser = Browser::builder()
        .headless(true)
        .stealth(StealthProfile::native())
        .persona_overlay(Persona {
            screen: Some(ScreenSpec::new(w, h, 1.0)),
            ..Persona::default()
        })
        .launch()
        .await?;
    let tab = browser.new_tab().await?;
    tab.goto("about:blank").await?;

    let out: String = tab
        .evaluate_main(
            r#"(()=>{const s=screen,w=window;return JSON.stringify({
                 screen:[s.width,s.height], avail:[s.availWidth,s.availHeight],
                 outer:[w.outerWidth,w.outerHeight], inner:[w.innerWidth,w.innerHeight],
                 coherent: w.outerWidth>=w.innerWidth && w.outerHeight>=w.innerHeight
                           && s.availHeight<s.height && w.outerHeight<=s.height})})()"#,
        )
        .await?;
    println!("requested {w}x{h}\n{out}");
    browser.close().await.ok();
    Ok(())
}
