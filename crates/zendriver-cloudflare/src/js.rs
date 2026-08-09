//! In-page evaluators, templated from [`TurnstileSelectors`].
//!
//! Three scripts run against the page's main world, and every one of them is
//! built here from the caller's markers rather than from literals. That is
//! the point of the module: before it existed the Cloudflare hostname was
//! written out twice, in the poll evaluator and in a separate detector
//! script, and the two could drift.
//!
//! Each script is composed as *one IIFE* by [`main_world_expr`]. That is a
//! stealth requirement, not formatting. `Runtime.evaluate` with no
//! `contextId` runs a classic script in the page's main world, where a
//! top-level `function foo() {}` becomes a property of the global object —
//! so an unwrapped walker publishes `window.findChallengeIframe` and
//! `window.clickableRect` to the page on every poll tick, names belonging to
//! an automation library sitting in the same realm as the challenge script.
//! Inside the IIFE they are ordinary locals.
//!
//! # JavaScript style
//! Injected scripts declare with `var` and iterate with indexed loops, never
//! `for...of`. `for...of` over a `NodeList` enters through
//! `NodeList.prototype[Symbol.iterator]`, a writable, configurable property
//! the page can redefine to watch someone walk its DOM. That one is worth
//! declining: it is a single chokepoint, entered once per loop and then
//! driven by one `next()` per element, and a handler installed there can
//! read `new Error().stack` and filter the page's own iteration out of what
//! it collects. An indexed loop never reaches it.
//!
//! It buys that one signal and no more. Indexed access is *not*
//! unobservable: `NodeList.prototype.length` is a WebIDL accessor with
//! `configurable: true`, so a page can wrap the getter, delegate to the
//! original, and count every read with nothing of its own broken. The rest
//! of the walk is louder. Each tick calls `root.querySelectorAll("iframe")`
//! and then `root.querySelectorAll("*")`, both ordinary writable methods on
//! `Document` / `Element` / `DocumentFragment`, and the iframe test calls
//! `String.prototype.includes` — equally patchable — with the marker the
//! walker is hunting for. A hook there is handed that marker, which is a
//! sharper tell than the iterator hook this convention avoids.
//!
//! Measured on Chrome 151 with all four wrapped: the walk still returns the
//! right iframe, while the page observes both `querySelectorAll` selectors,
//! every `length` read and that marker, against zero `Symbol.iterator`
//! calls. So the convention removes one signal from a loop that stays
//! observable to a page that looks for it; it is not cover. This is a rule
//! about declarations and iteration only — modern methods are used freely,
//! since declining them would buy nothing either.

use crate::options::TurnstileSelectors;

/// Render `value` as a JavaScript string literal, escaped.
///
/// Mandatory rather than cosmetic: the default token selectors contain
/// double quotes (`[name="cf-turnstile-response"]`), so naive interpolation
/// terminates the literal early and injects whatever follows. JSON string
/// syntax is a subset of JavaScript's, and serializing a `&str` cannot fail
/// — the fallback is unreachable and exists only to keep this total.
fn js_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

/// Render `values` as a JavaScript array-of-strings literal. Same escaping
/// argument as [`js_string`].
fn js_string_array(values: &[String]) -> String {
    serde_json::to_string(values).unwrap_or_else(|_| "[]".to_string())
}

/// Shared prelude for every evaluator below. Declares:
///
/// - `findChallengeIframe(root)` — the first iframe reachable from `root`
///   whose `src` contains
///   [`iframe_src_contains`](TurnstileSelectors::iframe_src_contains),
///   descending into open shadow roots (Cloudflare sometimes hosts the
///   widget inside one), else null. An **empty** marker returns null
///   without walking anything: `"".includes("")` is true, so an unguarded
///   walk would hand back the page's first `src`-carrying iframe and the
///   driver would scroll to it and click it.
/// - `clickableRect(el)` — `el`'s viewport rect *only if it is a real click
///   target*: non-zero size and not hidden by `visibility` / `display` /
///   `opacity`. A zero-size or hidden iframe is not a target — invisible
///   Turnstile mounts a 0×0 iframe whose token is populated with no click,
///   and clicking it would dispatch mouse events at a meaningless point.
///
/// Never evaluated on its own; [`main_world_expr`] wraps it.
fn walker_js(selectors: &TurnstileSelectors) -> String {
    let marker = js_string(&selectors.iframe_src_contains);
    format!(
        r#"
var IFRAME_MARKER = {marker};
function findChallengeIframe(root) {{
    if (!IFRAME_MARKER) return null;
    var iframes = root.querySelectorAll ? root.querySelectorAll("iframe") : [];
    for (var i = 0; i < iframes.length; i++) {{
        var f = iframes[i];
        if (f.src && f.src.includes(IFRAME_MARKER)) return f;
    }}
    var all = root.querySelectorAll ? root.querySelectorAll("*") : [];
    for (var j = 0; j < all.length; j++) {{
        if (all[j].shadowRoot) {{
            var sub = findChallengeIframe(all[j].shadowRoot);
            if (sub) return sub;
        }}
    }}
    return null;
}}
function clickableRect(el) {{
    if (!el) return null;
    var r = el.getBoundingClientRect();
    if (!(r.width > 0 && r.height > 0)) return null;
    var view = el.ownerDocument && el.ownerDocument.defaultView;
    var style = view && view.getComputedStyle ? view.getComputedStyle(el) : null;
    if (style && (style.visibility === "hidden" || style.display === "none" || style.opacity === "0")) {{
        return null;
    }}
    return {{ x: r.left, y: r.top, width: r.width, height: r.height }};
}}
"#
    )
}

/// Compose the in-page source for `body`: the walker and `body` together
/// inside a single IIFE, whose completion value is whatever `body` returns.
/// See the module docs for why the wrapper is load-bearing.
pub(crate) fn main_world_expr(selectors: &TurnstileSelectors, body: &str) -> String {
    let walker = walker_js(selectors);
    format!("(function(){{{walker}{body}}})()")
}

/// The unified poll evaluator. Returns, in one round-trip:
/// - `token` — the first non-empty value among
///   [`token_inputs`](TurnstileSelectors::token_inputs), else null.
/// - `bbox` — the challenge iframe's rect when it is a valid click target,
///   else null.
/// - `hasMarkers` — true when *any* marker is present: the container, a
///   token input, or the challenge iframe (visible or not).
///
/// The token input is in light DOM by design (page JS reads it to submit
/// forms), so `document.querySelector` is sufficient there.
pub(crate) fn poll_expr(selectors: &TurnstileSelectors) -> String {
    let container = js_string(&selectors.container);
    let token_selectors = js_string_array(&selectors.token_inputs);
    main_world_expr(
        selectors,
        &format!(
            r#"
    var iframe = findChallengeIframe(document);
    var bbox = clickableRect(iframe);
    var tokenSelectors = {token_selectors};
    var input = null;
    for (var k = 0; k < tokenSelectors.length && !input; k++) {{
        if (tokenSelectors[k]) input = document.querySelector(tokenSelectors[k]);
    }}
    var token = (input && input.value) ? input.value : null;
    var containerSelector = {container};
    var hasContainer = containerSelector ? !!document.querySelector(containerSelector) : false;
    var hasMarkers = hasContainer || !!input || !!iframe;
    return {{ token: token, bbox: bbox, hasMarkers: hasMarkers }};
"#
        ),
    )
}

/// The scroll-and-measure evaluator. Brings the challenge iframe fully into
/// the viewport when it isn't already, then returns its *post-scroll* rect —
/// or null if the widget vanished or is not a valid click target.
///
/// `behavior: "instant"` is load-bearing: the default `"auto"` resolves to
/// the element's computed `scroll-behavior`, so on a page setting
/// `html { scroll-behavior: smooth }` the scroll animates and the
/// `clickableRect` call on the next synchronous line reads the *pre*-scroll
/// rect — the one thing this round-trip exists to avoid.
pub(crate) fn scroll_expr(selectors: &TurnstileSelectors) -> String {
    main_world_expr(
        selectors,
        r#"
    var iframe = findChallengeIframe(document);
    if (!iframe) return null;
    var r = iframe.getBoundingClientRect();
    var vw = window.innerWidth || document.documentElement.clientWidth;
    var vh = window.innerHeight || document.documentElement.clientHeight;
    var fullyVisible = r.top >= 0 && r.left >= 0 && r.bottom <= vh && r.right <= vw;
    if (!fullyVisible && iframe.scrollIntoView) {
        iframe.scrollIntoView({ block: "center", inline: "center", behavior: "instant" });
    }
    return clickableRect(iframe);
"#,
    )
}

/// The presence detector. Returns the challenge iframe's raw rect if one is
/// mounted at all, else null.
///
/// Deliberately a different question from [`poll_expr`]'s `bbox`: this
/// reports a *mounted* iframe, that one reports a *clickable* one. A 0×0
/// invisible-Turnstile iframe is mounted but must never be clicked.
pub(crate) fn detect_expr(selectors: &TurnstileSelectors) -> String {
    main_world_expr(
        selectors,
        r#"
    var iframe = findChallengeIframe(document);
    if (!iframe) return null;
    var r = iframe.getBoundingClientRect();
    return { x: r.left, y: r.top, width: r.width, height: r.height };
"#,
    )
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    use crate::options::TurnstileSelectors;

    /// Selectors sharing no substring with any Cloudflare default, so an
    /// assertion that the defaults are absent cannot be satisfied by a
    /// source that ignored the overrides.
    fn alien() -> TurnstileSelectors {
        TurnstileSelectors {
            iframe_src_contains: "gate.alien.invalid".into(),
            container: ".alien-widget".into(),
            token_inputs: vec!["[name=\"alien-token\"]".into()],
        }
    }

    /// Every injected source must be built from the caller's markers. The
    /// negative half is the load-bearing one: a source still carrying
    /// `challenges.cloudflare.com` is a source with the marker baked in.
    #[test]
    fn custom_markers_template_into_every_injected_script() {
        let sel = alien();
        for (name, src) in [
            ("poll", poll_expr(&sel)),
            ("scroll", scroll_expr(&sel)),
            ("detect", detect_expr(&sel)),
        ] {
            assert!(
                src.contains("gate.alien.invalid"),
                "{name} evaluator ignored iframe_src_contains"
            );
            for default in [
                "challenges.cloudflare.com",
                ".cf-turnstile",
                "cf-turnstile-response",
            ] {
                assert!(
                    !src.contains(default),
                    "{name} evaluator still carries the hardcoded {default:?}"
                );
            }
        }

        // Only the poll evaluator reads the container / token markers, so
        // assert those where they belong rather than everywhere.
        let poll = poll_expr(&sel);
        assert!(poll.contains(".alien-widget"), "container marker missing");
        assert!(poll.contains("alien-token"), "token marker missing");
    }

    /// An empty `iframe_src_contains` must not mean "match every iframe".
    /// `String.prototype.includes("")` is true for every string, so an
    /// unguarded walk hands back the page's first `src`-carrying iframe —
    /// whatever it is — and the driver then scrolls to it and clicks it.
    /// The other two markers already read empty as "signal off"
    /// (`container` is guarded in [`poll_expr`], empty `token_inputs`
    /// entries are skipped), so this one does too.
    ///
    /// Asserted against the source rather than by running it: these tests
    /// have no JS engine. Presence alone would not be enough — a guard
    /// placed after the walk guards nothing — so the assertion is on where
    /// it sits.
    #[test]
    fn an_empty_iframe_marker_matches_no_iframe_rather_than_every_one() {
        const GUARD: &str = "if (!IFRAME_MARKER) return null;";
        let src = poll_expr(&TurnstileSelectors {
            iframe_src_contains: String::new(),
            ..TurnstileSelectors::default()
        });
        let guard = src
            .find(GUARD)
            .expect("an empty marker must short-circuit the walk");
        let walk = src
            .find(r#"querySelectorAll("iframe")"#)
            .expect("the walk itself went missing");
        assert!(
            guard < walk,
            "the empty-marker guard must precede the walk it guards"
        );
    }

    /// An empty `container` must switch the container signal off, not ask the
    /// page for `document.querySelector("")` — which is a `SyntaxError`, and
    /// throwing takes the whole poll evaluator down rather than just this one
    /// marker. The ternary is the guard; without it the empty string reaches
    /// `querySelector` directly.
    ///
    /// Asserted on where the guard sits, not merely that it exists: a check
    /// placed after the call guards nothing.
    #[test]
    fn an_empty_container_marker_disables_the_signal_rather_than_raising() {
        let src = poll_expr(&TurnstileSelectors {
            container: String::new(),
            ..TurnstileSelectors::default()
        });
        let guard = src
            .find("containerSelector ?")
            .expect("an empty container must short-circuit before querySelector");
        let call = src
            .find("querySelector(containerSelector)")
            .expect("the container lookup itself went missing");
        assert!(
            guard < call,
            "the empty-container guard must precede the lookup it guards"
        );
    }

    /// Selectors are caller data, and the default token selectors already
    /// contain double quotes. Interpolating them raw would terminate the JS
    /// string literal early and inject whatever followed.
    ///
    /// `token_inputs` carries two entries deliberately. Both `options.rs` and
    /// the book promise the modern input is preferred over the legacy one, and
    /// a single-element fixture cannot see order at all — reversing the emitted
    /// array would satisfy it just as well as preserving it.
    #[test]
    fn selector_strings_are_escaped_into_js_literals() {
        let sel = TurnstileSelectors {
            iframe_src_contains: "a\"b\\c".into(),
            container: "[data-x=\"quote\"]".into(),
            token_inputs: vec!["[name=\"tok\"]".into(), "[name=\"legacy-tok\"]".into()],
        };
        let src = poll_expr(&sel);

        assert!(
            src.contains(&serde_json::to_string(&sel.token_inputs).unwrap()),
            "token selectors must reach the page as one escaped array, in order"
        );
        for raw in [&sel.iframe_src_contains, &sel.container] {
            let escaped = serde_json::to_string(raw).unwrap();
            assert!(
                src.contains(&escaped),
                "{raw:?} was not emitted as the JS literal {escaped}"
            );
            assert!(
                !src.contains(&format!("\"{raw}\"")),
                "{raw:?} was interpolated raw — the literal terminates early"
            );
        }
    }

    /// Names every declaration in `src` at `{}` depth 0 that a classic
    /// script publishes on the page's global object: `var` and *named*
    /// `function`. Both become enumerable `window` properties (verified on
    /// Chrome 151); `let` / `const` / `class` are script-scoped and never
    /// do, so they are not leaks and are not counted. An anonymous
    /// `function (` — the IIFE wrapper itself — is an expression and binds
    /// nothing.
    ///
    /// Brace counting is only sound because no injected source puts a brace
    /// inside a string literal. Keep it that way; the alternative is a JS
    /// parser for a cookie-name-sized job. That now includes caller-supplied
    /// selectors, which is why the fixtures here use quotes and backslashes
    /// rather than braces. Comment text is scanned like any other, which can
    /// only ever produce a loud false positive, never a silent pass.
    fn top_level_page_globals(src: &str) -> Vec<String> {
        let is_ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
        let mut found = Vec::new();
        let mut depth = 0i32;
        for (i, ch) in src.char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
            if depth != 0 {
                continue;
            }
            // A keyword that is really the tail of a longer identifier
            // declares nothing: `myvar`, `refunction`.
            if src[..i].chars().next_back().is_some_and(is_ident) {
                continue;
            }
            for keyword in ["function", "var"] {
                let Some(rest) = src[i..].strip_prefix(keyword) else {
                    continue;
                };
                // `var` needs a separator to be a declaration. `function`
                // does not: `function(` is an expression, and the empty-name
                // check below is what rejects it.
                if keyword == "var" && !rest.starts_with(char::is_whitespace) {
                    break;
                }
                let name: String = rest
                    .trim_start()
                    .chars()
                    .take_while(|c| is_ident(*c))
                    .collect();
                if !name.is_empty() {
                    found.push(name);
                }
                break;
            }
        }
        found
    }

    /// The leak guard has to be able to fail. Fed the pre-fix shape — the
    /// walker evaluated beside the IIFE rather than inside it — it must name
    /// the declaration; fed the shipped shape it must stay quiet. The `var`
    /// case is here because the scan is named for page globals, and a
    /// top-level `var` is one just as much as a `function` is.
    #[test]
    fn top_level_scan_catches_every_leaked_page_global() {
        assert_eq!(
            top_level_page_globals(
                "function findChallengeIframe(root) { return null; }\n(function(){ return 1; })()"
            ),
            vec!["findChallengeIframe".to_string()],
        );
        assert_eq!(
            top_level_page_globals("var iframes = document.querySelectorAll('iframe');"),
            vec!["iframes".to_string()],
            "a top-level `var` is a `window` property too"
        );
        assert!(
            top_level_page_globals("(function(){ function nested(){} var local = 1; })()")
                .is_empty(),
            "a declaration inside the wrapper is a local, not a global"
        );
        // Script-scoped forms never reach `window`, so flagging them would
        // be a false positive, and a keyword inside a longer identifier is
        // not a keyword at all.
        assert!(
            top_level_page_globals("let a = 1; const b = 2; class C {}\nvarnish.myvar = 3;")
                .is_empty(),
        );
    }

    /// `Runtime.evaluate` with no `contextId` runs a classic script in the
    /// page's main world, where a top-level `function foo() {}` or `var bar`
    /// becomes `window.foo` / `window.bar`. Publishing `findChallengeIframe`
    /// / `clickableRect` there hands the challenge script — same realm,
    /// actively looking — a pair of names belonging to an automation
    /// library, on every poll tick. Every source this crate injects keeps
    /// its helpers and its locals inside a function scope.
    #[test]
    fn injected_sources_declare_no_page_globals() {
        let sel = TurnstileSelectors::default();
        for (name, src) in [
            ("poll evaluator", poll_expr(&sel)),
            ("scroll evaluator", scroll_expr(&sel)),
            ("detect evaluator", detect_expr(&sel)),
        ] {
            let leaked = top_level_page_globals(&src);
            assert!(
                leaked.is_empty(),
                "{name} publishes {leaked:?} onto the page's global object"
            );
            // Cannot pass by the walker having quietly gone missing.
            assert!(src.contains("function findChallengeIframe("));
            assert!(src.contains("function clickableRect("));
            assert!(
                src.starts_with("(function(){") && src.ends_with("})()"),
                "{name} must be exactly one IIFE"
            );
        }
    }

    /// `scrollIntoView`'s default `behavior: "auto"` resolves to the
    /// element's computed `scroll-behavior`, so on a page setting
    /// `html { scroll-behavior: smooth }` the scroll animates and the
    /// `clickableRect` call on the next synchronous line reads the
    /// *pre*-scroll rect — the one thing this extra round-trip exists to
    /// avoid, leaving the click aimed outside the viewport.
    #[test]
    fn scroll_evaluator_pins_instant_scroll_behavior() {
        const CALL: &str = "scrollIntoView(";
        let src = scroll_expr(&TurnstileSelectors::default());
        let mut calls = 0;
        for (idx, _) in src.match_indices(CALL) {
            let after = &src[idx + CALL.len()..];
            let end = after.find(')').expect("unterminated scrollIntoView call");
            let args = &after[..end];
            assert!(
                args.contains(r#"behavior: "instant""#),
                "scrollIntoView({args}) leaves behavior to the page's computed scroll-behavior"
            );
            calls += 1;
        }
        assert_eq!(calls, 1, "expected one scrollIntoView call to guard");
    }
}
