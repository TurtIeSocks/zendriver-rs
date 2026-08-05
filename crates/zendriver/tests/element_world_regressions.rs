//! Real-Chrome guards for the element flows that a world change — or a
//! spoofed global — breaks.
//!
//! Every case below shipped broken, and the mock unit suite was green through
//! all of them — by construction, not by oversight.
//! `MockConnection` replays a scripted frame sequence, so a test written
//! against the wrong sequence asserts the wrong sequence just as happily as
//! the right one. None of these defects is visible without a browser:
//!
//! 1. **Reads after a cross-document navigation.** Element reads were moved
//!    into the tab's isolated world, whose `executionContextId` is cached per
//!    tab. A navigation destroys that context; the cache was never
//!    invalidated, and the recovery path matched only `"Cannot find context"`
//!    — the wording `Runtime.evaluate` produces. `DOM.resolveNode` instead
//!    answers `-32000 "Node with given id does not belong to the document"`,
//!    so nothing recovered and every element API on the tab stayed broken for
//!    the rest of its life, from the second page onward.
//!
//! 2. **Actionability inside an iframe.** `Tab::ensure_isolated_world`
//!    creates exactly one world, for the tab's *main* frame.
//!    `getBoundingClientRect()` on a node inside an iframe is
//!    iframe-relative, while `document.elementFromPoint` evaluated in the
//!    main-frame world hit-tests the top document — so the two never agreed
//!    and the gate reported `NotActionable("occluded by overlay")` for every
//!    `click` / `hover` / `tap` on any element inside any iframe.
//!
//! 3. **Visibility under a spoofed viewport.** The gate's on-screen clause
//!    read `window.innerHeight`, which `zendriver-stealth`'s `screen.js`
//!    rewrites to the persona's screen height minus a fixed 86px chrome
//!    inset, while `scroll_into_view` moves the element with Chrome's *real*
//!    viewport. Comparing a real-geometry scroll against a fabricated
//!    viewport left every element parked between the two heights permanently
//!    "not visible", so `focus` — and with it `type_text` / `press` /
//!    `type_keys` — failed on any below-the-fold field. That patch ships in
//!    every profile except `StealthProfile::off()`: `Browser::builder()`
//!    defaults to `StealthProfile::native()`, which installs the geometry
//!    bootstrap (`observer.rs` maps `ProfileKind::Native` to
//!    `geometry_bootstrap()` = `_native.js` + `screen.js`). Measured on a
//!    default-profile launch: `innerHeight` 994 against a real 1080. So this
//!    was the default-posture case rather than an edge one, and the existing
//!    cases here missed it twice over: they launch with no explicit profile
//!    and neither uses an element below the fold.
//!
//! 5. **The on-screen clause in quirks mode.** Its replacement read
//!    `document.documentElement.clientHeight || window.innerHeight`. In a
//!    `BackCompat` document `documentElement.clientHeight` reports the
//!    DOCUMENT's content height (6024 on the fixture below) rather than the
//!    viewport's, so it is truthy, the fallback never fires, and every bbox
//!    got compared against the whole document — the viewport test was a
//!    silent no-op on any page without a doctype. It failed open, on exactly
//!    the sloppy legacy pages most likely to be carrying a honeypot.
//!
//! 4. **Whitespace in `type_text`.** Space and Enter went out as
//!    `rawKeyDown` — the CDP event type that means precisely "a keydown that
//!    generates no character" — carrying no `text`. So Chrome generated
//!    none: `type_text("a b")` silently produced `"ab"`, and `press(Enter)`
//!    never submitted a form. The same dispatch also stalled ~1.1s for the
//!    first one in a renderer against ~2ms for any ordinary character, and a
//!    second one in the same string killed the browser process outright,
//!    which is how it first surfaced — as a CDP timeout on a below-the-fold
//!    field rather than as wrong text. Nothing about it is page-shaped: the
//!    stall and the crash reproduce identically on a page with nothing to
//!    scroll, and `window.scrollY` never moves.
//!
//! None of these tests asserts *which* world the JS runs in; that is an
//! implementation choice. They assert the observable behavior any world
//! choice has to preserve, so they stay meaningful when the isolated-world
//! move is re-landed with per-frame world resolution.
//!
//! Deliberately NOT `#[ignore]`d: the `test-integration` job runs
//! `-E 'kind(test)'` (non-ignored) on every PR with Chrome provisioned, while
//! `#[ignore]` would route these to `nightly-ignored-tests`, which carries
//! `continue-on-error: true` on a daily cron. These guard a regression that
//! already shipped twice, so they have to block a merge rather than yellow-dot
//! one. Each launches headless Chrome against a loopback fixture and they
//! finish in a few seconds combined.
//!
//! Running these locally: give `cargo` a target directory that no *other*
//! checkout of this same package shares. Cargo keys an integration-test
//! binary on package + target name, not on source path, so pointing two trees
//! at one `CARGO_TARGET_DIR` lets a run silently execute the binary the other
//! tree built. That reads as a clean pass of code you never compiled — it
//! produced exactly one false "both pass" while these tests were being
//! written against the pre-fix revision.
//!
//! Gated behind the `integration-tests` feature so CI can skip on
//! Chrome-less runners; CI exercises these on the integration job.

#![cfg(feature = "integration-tests")]
#![allow(clippy::panic, clippy::unwrap_used)]

use serial_test::serial;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use zendriver::stealth::{InputProfile, StealthProfile};
use zendriver::{Browser, Key, SpecialKey};

/// Mount `html` at `at` on `mock`. These fixtures need several routes each
/// (a second page to navigate to, an iframe document to embed), so routes are
/// added one at a time rather than through a single-page helper.
async fn mount(mock: &MockServer, at: &str, html: &str) {
    Mock::given(method("GET"))
        .and(path(at))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(html.as_bytes().to_vec(), "text/html"),
        )
        .mount(mock)
        .await;
}

/// Every element API must keep working after a cross-document navigation.
///
/// `goto A → read → goto B → read` is about as ordinary as a scraper gets,
/// and it is the flow a per-tab context cache breaks: the first page looks
/// perfect, and everything after it fails. So the assertions here are on the
/// *second* and third pages, and they cover a read (`inner_text`), a
/// serialization (`outer_html`), an attribute lookup, a predicate
/// (`is_visible`) and a full gated action (`click`) — all four routes died
/// the same way, through the same funnel.
#[tokio::test]
#[serial]
async fn element_apis_survive_a_cross_document_navigation() {
    let mock = MockServer::start().await;
    mount(
        &mock,
        "/a",
        r#"<!doctype html><html><body>
             <div id="t" data-page="a">page A</div>
             <button id="b" onclick="this.textContent='clicked A'">press A</button>
           </body></html>"#,
    )
    .await;
    mount(
        &mock,
        "/b",
        r#"<!doctype html><html><body>
             <div id="t" data-page="b">page B</div>
             <button id="b" onclick="this.textContent='clicked B'">press B</button>
           </body></html>"#,
    )
    .await;

    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();

    // Page A: establishes whatever per-tab state the element path caches.
    tab.goto(&format!("{}/a", mock.uri())).await.unwrap();
    let el = tab.find().css("#t").one().await.unwrap();
    assert_eq!(el.inner_text().await.unwrap().trim(), "page A");

    // Page B is where a stale cache bites. Every one of these used to fail
    // with `-32000 "Node with given id does not belong to the document"`.
    tab.goto(&format!("{}/b", mock.uri())).await.unwrap();
    let el = tab.find().css("#t").one().await.unwrap();
    assert_eq!(
        el.inner_text().await.unwrap().trim(),
        "page B",
        "inner_text must still work after the first navigation"
    );
    assert!(
        el.outer_html().await.unwrap().contains("page B"),
        "outer_html must still work after the first navigation"
    );
    assert_eq!(el.attr("data-page").await.unwrap().as_deref(), Some("b"));
    assert!(
        el.is_visible().await.unwrap(),
        "is_visible must still work after the first navigation"
    );

    // The actionability gate reads through the same funnel, so a gated
    // action is the strongest single check that the whole path recovered.
    let button = tab.find().css("#b").one().await.unwrap();
    button.click().await.unwrap();
    assert_eq!(
        button.inner_text().await.unwrap().trim(),
        "clicked B",
        "a gated click must still land after the first navigation"
    );

    // A third hop, so a fix that merely survives one navigation cannot pass.
    tab.goto(&format!("{}/a", mock.uri())).await.unwrap();
    let el = tab.find().css("#t").one().await.unwrap();
    assert_eq!(el.inner_text().await.unwrap().trim(), "page A");
    assert_eq!(el.attr("data-page").await.unwrap().as_deref(), Some("a"));

    browser.close().await.unwrap();
}

/// Pointer actions must reach an element inside a same-origin iframe.
///
/// The iframe is deliberately offset from the top-left. That is the whole
/// point of the fixture: the button's own rect is iframe-relative, so a
/// hit-test performed against the TOP document at those same numbers lands on
/// the outer page's spacer instead of the button, and the gate rejects a
/// perfectly clickable element as occluded. An iframe pinned at (0, 0) would
/// hide the bug, since both coordinate spaces would agree.
#[tokio::test]
#[serial]
async fn hover_and_click_reach_an_element_inside_an_iframe() {
    let mock = MockServer::start().await;
    mount(
        &mock,
        "/outer",
        r#"<!doctype html><html><body style="margin:0">
             <div style="height:200px">spacer</div>
             <iframe src="/inner" style="position:absolute;top:200px;left:100px;
                     width:400px;height:300px;border:0"></iframe>
           </body></html>"#,
    )
    .await;
    mount(
        &mock,
        "/inner",
        r#"<!doctype html><html><body style="margin:0">
             <button id="go" style="width:200px;height:60px"
                     onmouseover="this.dataset.hovered='yes'"
                     onclick="this.textContent='clicked'">click me</button>
           </body></html>"#,
    )
    .await;

    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&format!("{}/outer", mock.uri())).await.unwrap();

    let button = tab
        .find()
        .css("#go")
        .include_frames()
        .one()
        .await
        .expect("the button inside the iframe must be findable");
    assert_eq!(button.inner_text().await.unwrap().trim(), "click me");

    // Both of these used to fail with NotActionable("occluded by overlay").
    // Asserting the DOM side effect, not just the absence of an error: a gate
    // that passes while the synthesized event lands somewhere else entirely
    // is still broken, just more quietly.
    button
        .hover()
        .await
        .expect("hovering an element inside an iframe must pass the actionability gate");
    assert_eq!(
        button.attr("data-hovered").await.unwrap().as_deref(),
        Some("yes"),
        "the hover must actually reach the button"
    );

    button
        .click()
        .await
        .expect("clicking an element inside an iframe must pass the actionability gate");
    assert_eq!(
        button.inner_text().await.unwrap().trim(),
        "clicked",
        "the click must actually land on the button"
    );

    browser.close().await.unwrap();
}

/// A below-the-fold field must stay reachable while stealth spoofs the
/// viewport.
///
/// The `window.inner*` rewrite is NOT exclusive to `spoofed`: it lives in
/// `screen.js`, which `observer.rs` installs for `ProfileKind::Native` too
/// (`geometry_bootstrap()` = `_native.js` + `screen.js`), and
/// `Browser::builder()` defaults to `StealthProfile::native()`. Only
/// `StealthProfile::off()` leaves `window.inner*` alone — measured on a
/// default-profile launch, `innerHeight` reads 994 against a real 1080,
/// identical to `spoofed`. `spoofed` is the profile under test because it is
/// the full shipping posture, and because its humanized `InputProfile` is
/// what the typing at the end of this test exercises.
///
/// On the stock 1920x1080 persona it claims `innerHeight` 1080 - 86 (browser
/// chrome) = 994, while the layout viewport stays at Chrome's real 1080.
/// `scroll_into_view` scrolls with the real number, so anything landing in
/// that 86px band read as off-screen: `is_visible` answered `false` for a
/// field plainly on screen, and `focus` — the first step of `type_text`,
/// `press` and `type_keys` — died with `NotActionable(5s, "not visible")`.
///
/// The fixture puts the field LAST in a document taller than any viewport, so
/// `scrollIntoView({ block: 'center' })` clamps at the document bottom
/// instead of centring and parks the field against the real viewport's lower
/// edge — inside the band. A field that *can* be centred lands mid-viewport
/// and hides the bug entirely, which is why the geometry is measured at
/// runtime rather than assumed: a fixture that stops landing in the band
/// fails loudly instead of passing vacuously.
#[tokio::test]
#[serial]
async fn focus_reaches_a_below_the_fold_field_under_a_spoofed_viewport() {
    let mock = MockServer::start().await;
    // The spacer is deliberately far taller than any persona's screen rather
    // than tuned to one: all it has to guarantee is that the page scrolls.
    // The field's own height is what has to stay under the chrome inset, and
    // 24px is comfortably under it at any resolution — the inset is a fixed
    // pixel constant, not a fraction of the screen.
    mount(
        &mock,
        "/deep",
        r#"<!doctype html><html><body style="margin:0">
             <div style="height:6000px">a very long page</div>
             <input id="deep" style="display:block;width:200px;height:24px;
                    box-sizing:border-box">
           </body></html>"#,
    )
    .await;

    // Stealth spoofed, and nothing else pinned: the whole shipping default,
    // keystroke timing included. `spoofed`'s humanized profile adds
    // per-character delays and a 3% typo simulation that types a wrong key
    // and Backspaces it, which costs this test a couple of seconds and
    // exercises a path a pinned profile would skip. It used to be pinned to
    // `InputProfile::native()`, because that extra special-key dispatch was
    // a hazard back when a special-key dispatch could wedge the browser.
    let browser = Browser::builder()
        .stealth(StealthProfile::spoofed())
        .headless(true)
        .launch()
        .await
        .unwrap();
    let tab = browser.main_tab();
    tab.goto(&format!("{}/deep", mock.uri())).await.unwrap();

    let field = tab.find().css("#deep").one().await.unwrap();

    // Scroll explicitly first — it carries no actionability gate, so this
    // reproduces the exact state every gated action reaches its gate in, and
    // lets the geometry below be checked before anything can fail on it.
    field.scroll_into_view().await.unwrap();

    // Read the post-scroll geometry in the MAIN world: that is where the
    // persona's patch lives, and where the actionability probe runs. The
    // isolated world would report Chrome's real `innerHeight` and quietly
    // prove nothing.
    let (top, spoofed_vh, real_vh): (f64, f64, f64) = tab
        .evaluate_main(
            "(() => {
               const r = document.getElementById('deep').getBoundingClientRect();
               return [r.top, window.innerHeight, document.documentElement.clientHeight];
             })()",
        )
        .await
        .unwrap();

    assert!(
        spoofed_vh < real_vh,
        "the persona must actually be spoofing the viewport, or this test \
         guards nothing: window.innerHeight {spoofed_vh}, \
         documentElement.clientHeight {real_vh}"
    );
    assert!(
        top >= spoofed_vh && top < real_vh,
        "the fixture must park the field inside the spoof band — on screen \
         for the real viewport, past the bottom of the spoofed one; got \
         rect.top {top} against innerHeight {spoofed_vh} / clientHeight \
         {real_vh}"
    );

    // A public API returning the wrong answer, ahead of any gate: the field
    // is on screen and `is_visible` used to say otherwise.
    assert!(
        field.is_visible().await.unwrap(),
        "is_visible must answer for the real viewport, not the spoofed one"
    );

    field
        .focus()
        .await
        .expect("focus must pass the actionability gate for a below-the-fold field under stealth");

    // Assert the DOM side effect, not just the absence of an error: typing
    // that reports success while the keystrokes go nowhere is still broken.
    //
    // Spaces, deliberately. This string carried hyphens while spaces were
    // dispatched as a text-less `rawKeyDown`, because that dispatch wedged
    // the browser and would have decided this test's outcome for a reason
    // that has nothing to do with visibility. The dispatch is fixed and the
    // whitespace case below guards it directly, so the ordinary string is
    // back.
    field.type_text("typed below the fold").await.unwrap();
    let value: String = tab
        .evaluate_main("document.getElementById('deep').value")
        .await
        .unwrap();
    assert_eq!(
        value, "typed below the fold",
        "the keystrokes must land in the below-the-fold field"
    );

    browser.close().await.unwrap();
}

/// The on-screen clause must still be a test in quirks mode.
///
/// The fixture below carries no doctype, so it renders in `BackCompat`, where
/// `document.documentElement.clientHeight` reports the DOCUMENT's content
/// height instead of the viewport's — 6024 here, against a real 1080. The
/// clause read `documentElement.clientHeight || window.innerHeight`,
/// justified by a comment claiming `window.inner*` survived "only as a
/// quirks-mode fallback, where `documentElement` reports 0". It reports 6024,
/// which is truthy, so the fallback never fired and the bbox got compared
/// against the whole document: the viewport test was a no-op on every
/// doctype-less page.
///
/// Nothing surfaced, because it failed open — `is_visible` simply answered
/// `true` for a field 6000px below the fold, and the off-screen and honeypot
/// filtering `visible_only(true)` promises was silently absent on exactly the
/// sloppy legacy pages most likely to be carrying a honeypot.
///
/// Hence the assertion on the *pre-scroll* `false`. The post-scroll `true`
/// guards the opposite direction: a clause that rejected everything in quirks
/// mode would satisfy the first assertion by accident. The geometry between
/// them is read at runtime so a fixture that stops reproducing the trap fails
/// loudly instead of passing vacuously.
///
/// Launched with no explicit profile: this defect is posture-independent
/// (quirks mode is the page's doing, not the persona's), and the default is
/// the configuration a caller gets without asking for anything.
#[tokio::test]
#[serial]
async fn is_visible_rejects_a_below_the_fold_field_on_a_doctype_less_page() {
    let mock = MockServer::start().await;
    // No doctype — that is the entire point of the fixture. The spacer is
    // taller than any viewport so the field starts far below the fold.
    mount(
        &mock,
        "/quirks",
        r#"<html><body style="margin:0">
             <div style="height:6000px">a very long page, and no doctype</div>
             <input id="deep" style="display:block;width:200px;height:24px;
                    box-sizing:border-box">
           </body></html>"#,
    )
    .await;

    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&format!("{}/quirks", mock.uri())).await.unwrap();

    let field = tab.find().css("#deep").one().await.unwrap();

    // `body.clientHeight` is the layout viewport in `BackCompat`;
    // `documentElement.clientHeight` is the content height that made the old
    // expression's `||` unreachable.
    let (mode, top, content_h, viewport_h): (String, f64, f64, f64) = tab
        .evaluate_main(
            "(() => {
               const r = document.getElementById('deep').getBoundingClientRect();
               return [document.compatMode, r.top,
                       document.documentElement.clientHeight,
                       document.body.clientHeight];
             })()",
        )
        .await
        .unwrap();

    assert_eq!(
        mode, "BackCompat",
        "the fixture must render in quirks mode or this test guards nothing"
    );
    assert!(
        top >= viewport_h,
        "the field must start below the fold: rect.top {top} against a \
         {viewport_h}px layout viewport"
    );
    assert!(
        content_h > viewport_h && top < content_h,
        "the trap must be live: documentElement.clientHeight {content_h} has \
         to be a truthy content height that still swallows rect.top {top}, or \
         the old expression would have rejected the field for the right \
         reason by accident"
    );

    assert!(
        !field.is_visible().await.unwrap(),
        "a field {top}px down a quirks-mode page must not read as visible in \
         a {viewport_h}px viewport"
    );

    // The other direction: the clause must still pass what is genuinely on
    // screen, in the same rendering mode.
    field.scroll_into_view().await.unwrap();
    assert!(
        field.is_visible().await.unwrap(),
        "the same field must read as visible once scrolled into view"
    );

    browser.close().await.unwrap();
}

/// Whitespace has to arrive as text, and quickly.
///
/// Space and Enter are printable keys with names, and they were dispatched
/// as though the name were the whole story: `rawKeyDown` with no `text`,
/// which asks Chrome for a keydown that generates *no character*. Chrome
/// obliged. `type_text("a b")` produced `"ab"` — a wrong string returned as
/// a success, the worst shape a defect can take — and Enter stopped
/// submitting forms. The timing was the louder half: the first such dispatch
/// in a renderer took ~1.1s against ~2ms for any ordinary character, and a
/// second one in the same string killed the browser process, surfacing as a
/// CDP timeout far from the input layer.
///
/// So this asserts both halves, on the shipping stealth default. The typed
/// string carries four spaces, because the failure escalated with the count:
/// one stalled, the second killed the browser process. Keystroke timing is
/// pinned to `native` (no per-character delay, no typo simulation) so the
/// elapsed bound below measures the dispatch rather than the humanized
/// profile's deliberate pauses.
///
/// Enter and Tab are here because they were the two keys the fix had to tell
/// apart. Enter inserts text and had to start carrying it; Tab genuinely
/// inserts nothing, so it stays a text-less `rawKeyDown` — the shape it needs
/// for focus traversal, and the shape that was never slow. Both default
/// actions are asserted directly.
#[tokio::test]
#[serial]
async fn type_text_types_whitespace_instead_of_dropping_it() {
    let mock = MockServer::start().await;
    // The form holds a single field and no submit button, which is the shape
    // the HTML spec allows to submit implicitly on Enter. The textarea
    // follows it in document order, so it is also the Tab target.
    mount(
        &mock,
        "/whitespace",
        r#"<!doctype html><html><body style="margin:0">
             <div style="height:6000px">a very long page</div>
             <form id="f" onsubmit="window.__submitted = 1; return false;">
               <input id="line" style="display:block;width:200px;height:24px;
                      box-sizing:border-box">
             </form>
             <textarea id="area" rows="3"></textarea>
           </body></html>"#,
    )
    .await;

    let browser = Browser::builder()
        .stealth(StealthProfile::spoofed())
        .input_profile(InputProfile::native())
        .headless(true)
        .launch()
        .await
        .unwrap();
    let tab = browser.main_tab();
    tab.goto(&format!("{}/whitespace", mock.uri()))
        .await
        .unwrap();

    let line = tab.find().css("#line").one().await.unwrap();
    let started = std::time::Instant::now();
    line.type_text("four spaces in this line")
        .await
        .expect("typing a string with spaces must not wedge the browser");
    let elapsed = started.elapsed();
    let value: String = tab
        .evaluate_main("document.getElementById('line').value")
        .await
        .unwrap();
    assert_eq!(
        value, "four spaces in this line",
        "every space must reach the field as a space"
    );
    // Twenty-four characters with no injected delay land in single-digit
    // milliseconds. The bound is loose on purpose — ten seconds is slack for
    // a loaded machine — because the sharp assertions are the value above and
    // the `expect` on the dispatch: pre-fix this string crashed the browser
    // rather than merely running slowly. The bound catches the variant that
    // stalls while somehow keeping the text right.
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "typing a 24-character string must not take {elapsed:?}"
    );

    // Enter carries a carriage return, which Chrome turns into a newline.
    let area = tab.find().css("#area").one().await.unwrap();
    area.type_text("two\nlines").await.unwrap();
    let text: String = tab
        .evaluate_main("document.getElementById('area').value")
        .await
        .unwrap();
    assert_eq!(
        text, "two\nlines",
        "Enter must insert a newline in a textarea"
    );

    // Tab keeps its default action: it moves focus rather than typing.
    line.press(Key::Special(SpecialKey::Tab)).await.unwrap();
    let focused: String = tab
        .evaluate_main("document.activeElement.id")
        .await
        .unwrap();
    assert_eq!(focused, "area", "Tab must still traverse focus");

    // Enter keeps its default action too — a text-less Enter never submitted.
    line.press(Key::Special(SpecialKey::Enter)).await.unwrap();
    let submitted: u32 = tab.evaluate_main("window.__submitted || 0").await.unwrap();
    assert_eq!(submitted, 1, "Enter must still submit the form");

    browser.close().await.unwrap();
}
