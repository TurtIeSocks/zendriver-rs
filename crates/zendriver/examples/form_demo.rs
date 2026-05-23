//! Fill out a small HTML form rendered via `data:` URL — demonstrates the
//! P3 input surface end-to-end without depending on any third-party site.
//!
//! Equivalent in spirit to the form-fill snippets scattered through the
//! Python `examples/` directory (`network_monitor.py`'s search-and-submit
//! flow, `imgur_upload_image.py`'s title-field fill). Picks `data:` over
//! a third-party form so the example stays deterministic across runs.
//!
//! Sequence:
//!   1. CSS-select the inputs and submit button.
//!   2. [`Element::type_text`] simulates per-character key events with the
//!      Bezier/jitter realism from the [`StealthProfile`]'s `InputProfile`.
//!   3. [`Element::click`] dispatches a real `mousedown` + `mouseup` via
//!      `Input.dispatchMouseEvent` after running the actionability gates.
//!   4. Read back the form's serialized state via `evaluate_main` to prove
//!      the inputs took our values.

use zendriver::Browser;

const FORM_HTML: &str = "data:text/html,\
<!doctype html><html><body>\
<form id='f' onsubmit='window.submitted=true;return false'>\
<input id='user' name='user' />\
<input id='pass' name='pass' type='password' />\
<button id='go' type='submit'>Submit</button>\
</form></body></html>";

#[tokio::main]
#[allow(clippy::result_large_err)] // example boundary; users wrap in their own Error
async fn main() -> zendriver::Result<()> {
    tracing_subscriber::fmt::init();

    let browser = Browser::builder().headless(true).launch().await?;
    let tab = browser.main_tab();
    tab.goto(FORM_HTML).await?;
    tab.wait_for_load().await?;

    let user = tab.find().css("#user").one().await?;
    user.type_text("rin").await?;

    let pass = tab.find().css("#pass").one().await?;
    pass.type_text("hunter2").await?;

    let go = tab.find().css("#go").one().await?;
    go.click().await?;

    let user_val: String = tab
        .evaluate_main("document.getElementById('user').value")
        .await?;
    let submitted: bool = tab.evaluate_main("window.submitted === true").await?;
    println!("user field = {user_val:?}, submitted = {submitted}");

    browser.close().await?;
    Ok(())
}
