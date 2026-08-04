//! bs4-like combinable predicate matchers. A `PredicateSet` compiles to a
//! CSS selector (structural parts) + a JS boolean post-filter (regex/text).
//! Pure — no CDP, fully unit-testable.

use serde_json::json;

#[derive(Debug, Clone, Default)]
pub(crate) struct PredicateSet {
    pub(crate) tag: Option<String>,
    pub(crate) attrs: Vec<AttrPred>,
    pub(crate) texts: Vec<TextPred>,
}

/// Trailing `bool` on every value-bearing variant below is `case_insensitive`
/// — `true` requests the CSS `i` flag (`to_css_selector`) / a lower-cased
/// compare (`to_js_filter`). `Has` (no value) and `Regex` (already
/// case-insensitive via an inline `(?i)` pattern flag) don't carry one.
#[derive(Debug, Clone)]
pub(crate) enum AttrPred {
    Exact(String, String, bool),
    Contains(String, String, bool),
    StartsWith(String, String, bool),
    EndsWith(String, String, bool),
    Has(String),
    Regex(String, String), // (name, pattern) — JS post-filter, not CSS
}

#[derive(Debug, Clone)]
pub(crate) enum TextPred {
    Contains(String, bool),
    /// Exact text match under XPath `normalize-space()` semantics — see
    /// [`normalize_space`]. Both sides are normalized, so this agrees with
    /// the `text_exact` selector kind, which compiles to XPath
    /// `//*[normalize-space(.)=<needle>]`.
    Equals(String, bool),
    Matches(String), // regex pattern
}

/// JS tail that applies [`normalize_space`]'s rule to the page-side text —
/// appended to `TXT` in [`PredicateSet::to_js_filter`].
const JS_NORMALIZE_SPACE: &str = r#".replace(/[ \t\r\n]+/g," ").trim()"#;

/// XPath `normalize-space()` semantics: collapse every run of XML
/// whitespace (space, tab, CR, LF) to a single space, then trim the ends.
///
/// Applied to the *needle* at compile time and mirrored on the page-side
/// text by [`JS_NORMALIZE_SPACE`], so both operands of a `text_equals`
/// compare are normalized the same way. Before this, the JS post-filter
/// only `trim()`ed while the `text_exact` selector compiled to XPath
/// `normalize-space(.)`, so `<b>Hello   world</b>` matched one and not the
/// other.
///
/// Deliberately limited to the four XML whitespace characters rather than
/// the broader JS `\s` / Rust `char::is_whitespace` class: `normalize-space()`
/// leaves `&nbsp;` (U+00A0) and other Unicode spaces alone, and widening the
/// class here would re-introduce on `&nbsp;` exactly the divergence this
/// normalization exists to remove.
pub(crate) fn normalize_space(s: &str) -> String {
    s.split([' ', '\t', '\r', '\n'])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Quote a value as a JSON string (`"v"`) — safe for both CSS attribute
/// values and JS string literals (the selector is later JSON-embedded into
/// the JS source, double-escaping correctly).
fn q(v: &str) -> String {
    json!(v).to_string()
}

/// CSS case-insensitivity flag suffix — `" i"` inside the bracket right
/// before `]` when `ci` is set, empty string (byte-identical to the
/// pre-case-insensitive output) otherwise. Valid for `=`/`*=`/`^=`/`$=`
/// attribute selectors (not `[name]` presence, which has no value to flag).
fn ci_flag(ci: bool) -> &'static str {
    if ci { " i" } else { "" }
}

impl PredicateSet {
    pub(crate) fn is_empty(&self) -> bool {
        self.tag.is_none() && self.attrs.is_empty() && self.texts.is_empty()
    }

    /// Structural predicates → a CSS selector. `attr_regex` + text predicates
    /// are post-filters and are NOT emitted here. Empty set → `"*"`.
    ///
    /// Attribute *values* are JSON-quoted via [`q`] so a quote/backslash
    /// can't break the `[name="value"]` literal. Attribute *names* are
    /// caller-supplied identifiers and are emitted verbatim — a malformed
    /// name yields a malformed selector, which surfaces as a JS
    /// `SyntaxError` from `querySelectorAll` (a `JsException`), never an
    /// escape out of the JS string (the whole selector is JSON-embedded
    /// before evaluation by the resolver).
    pub(crate) fn to_css_selector(&self) -> String {
        let mut s = self.tag.clone().unwrap_or_default();
        for a in &self.attrs {
            match a {
                AttrPred::Exact(n, v, ci) => {
                    s.push_str(&format!("[{n}={}{}]", q(v), ci_flag(*ci)));
                }
                AttrPred::Contains(n, v, ci) => {
                    s.push_str(&format!("[{n}*={}{}]", q(v), ci_flag(*ci)));
                }
                AttrPred::StartsWith(n, v, ci) => {
                    s.push_str(&format!("[{n}^={}{}]", q(v), ci_flag(*ci)));
                }
                AttrPred::EndsWith(n, v, ci) => {
                    s.push_str(&format!("[{n}$={}{}]", q(v), ci_flag(*ci)));
                }
                AttrPred::Has(n) => s.push_str(&format!("[{n}]")),
                AttrPred::Regex(..) => {}
            }
        }
        if s.is_empty() { "*".to_string() } else { s }
    }

    /// Post-filter predicates (`attr_regex` + all text predicates) → a JS
    /// boolean expression over a bound `el`. Returns `"true"` when there are
    /// no post-filters (so the caller can always `.filter(el => <expr>)`).
    ///
    /// [`TextPred::Equals`] normalizes both operands per [`normalize_space`];
    /// [`TextPred::Contains`] and [`TextPred::Matches`] run against the raw
    /// text, since a substring or regex needle may legitimately carry its own
    /// whitespace.
    pub(crate) fn to_js_filter(&self) -> String {
        const TXT: &str = r#"(el.innerText||el.textContent||"")"#;
        let mut checks: Vec<String> = Vec::new();
        for a in &self.attrs {
            if let AttrPred::Regex(n, p) = a {
                checks.push(format!(
                    "new RegExp({}).test(el.getAttribute({})||\"\")",
                    q(p),
                    q(n)
                ));
            }
        }
        for t in &self.texts {
            match t {
                TextPred::Contains(s, false) => checks.push(format!("{TXT}.includes({})", q(s))),
                TextPred::Contains(s, true) => checks.push(format!(
                    "{TXT}.toLowerCase().includes({})",
                    q(&s.to_lowercase())
                )),
                TextPred::Equals(s, false) => checks.push(format!(
                    "{TXT}{JS_NORMALIZE_SPACE}==={}",
                    q(&normalize_space(s))
                )),
                TextPred::Equals(s, true) => checks.push(format!(
                    "{TXT}{JS_NORMALIZE_SPACE}.toLowerCase()==={}",
                    q(&normalize_space(&s.to_lowercase()))
                )),
                TextPred::Matches(p) => checks.push(format!("new RegExp({}).test({TXT})", q(p))),
            }
        }
        if checks.is_empty() {
            "true".to_string()
        } else {
            checks.join("&&")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_set_compiles_to_star() {
        assert_eq!(PredicateSet::default().to_css_selector(), "*");
    }

    #[test]
    fn tag_and_attrs_compile_to_css() {
        let p = PredicateSet {
            tag: Some("div".into()),
            attrs: vec![
                AttrPred::Exact("data-role".into(), "card".into(), false),
                AttrPred::Contains("class".into(), "active".into(), false),
                AttrPred::StartsWith("id".into(), "item-".into(), false),
                AttrPred::EndsWith("data-x".into(), "-end".into(), false),
                AttrPred::Has("data-ready".into()),
            ],
            texts: vec![],
        };
        assert_eq!(
            p.to_css_selector(),
            r#"div[data-role="card"][class*="active"][id^="item-"][data-x$="-end"][data-ready]"#
        );
    }

    #[test]
    fn attr_regex_is_not_in_css() {
        let p = PredicateSet {
            tag: Some("a".into()),
            attrs: vec![AttrPred::Regex("href".into(), r"\d+".into())],
            texts: vec![],
        };
        assert_eq!(p.to_css_selector(), "a");
    }

    #[test]
    fn empty_filter_is_true() {
        assert_eq!(PredicateSet::default().to_js_filter(), "true");
    }

    #[test]
    fn regex_and_text_compile_to_js_checks() {
        let p = PredicateSet {
            tag: None,
            attrs: vec![AttrPred::Regex("href".into(), r"\d+".into())],
            texts: vec![
                TextPred::Contains("Buy".into(), false),
                TextPred::Equals("OK".into(), false),
                TextPred::Matches(r"^\$".into()),
            ],
        };
        let f = p.to_js_filter();
        assert!(
            f.contains(r#"new RegExp("\\d+").test(el.getAttribute("href")||"")"#),
            "{f}"
        );
        assert!(
            f.contains(r#"(el.innerText||el.textContent||"").includes("Buy")"#),
            "{f}"
        );
        assert!(
            f.contains(
                r#"(el.innerText||el.textContent||"").replace(/[ \t\r\n]+/g," ").trim()==="OK""#
            ),
            "{f}"
        );
        assert!(
            f.contains(r#"new RegExp("^\\$").test((el.innerText||el.textContent||""))"#),
            "{f}"
        );
        assert!(f.contains("&&"), "checks are AND-joined: {f}");
    }

    // --- case-insensitive predicate matchers (Phase 3, item 1) ------------

    #[test]
    fn attr_i_variants_emit_css_case_insensitivity_flag() {
        let p = PredicateSet {
            tag: Some("div".into()),
            attrs: vec![
                AttrPred::Exact("class".into(), "Foo".into(), true),
                AttrPred::Contains("class".into(), "Foo".into(), true),
                AttrPred::StartsWith("class".into(), "Foo".into(), true),
                AttrPred::EndsWith("class".into(), "Foo".into(), true),
            ],
            texts: vec![],
        };
        let css = p.to_css_selector();
        assert!(css.contains(r#"[class="Foo" i]"#), "{css}");
        assert!(css.contains(r#"[class*="Foo" i]"#), "{css}");
        assert!(css.contains(r#"[class^="Foo" i]"#), "{css}");
        assert!(css.contains(r#"[class$="Foo" i]"#), "{css}");
    }

    #[test]
    fn non_ci_attr_variants_stay_byte_identical() {
        // Regression: the `false` (non-`_i`) arm must emit exactly what it
        // did before the case-insensitive flag was added — no trailing
        // " i" and no other formatting drift.
        let p = PredicateSet {
            tag: Some("div".into()),
            attrs: vec![
                AttrPred::Exact("data-role".into(), "card".into(), false),
                AttrPred::Contains("class".into(), "active".into(), false),
                AttrPred::StartsWith("id".into(), "item-".into(), false),
                AttrPred::EndsWith("data-x".into(), "-end".into(), false),
                AttrPred::Has("data-ready".into()),
            ],
            texts: vec![],
        };
        assert_eq!(
            p.to_css_selector(),
            r#"div[data-role="card"][class*="active"][id^="item-"][data-x$="-end"][data-ready]"#
        );
    }

    #[test]
    fn containing_text_i_lowercases_both_sides_in_js_filter() {
        let p = PredicateSet {
            tag: None,
            attrs: vec![],
            texts: vec![TextPred::Contains("Foo".into(), true)],
        };
        let f = p.to_js_filter();
        assert!(
            f.contains(r#"(el.innerText||el.textContent||"").toLowerCase().includes("foo")"#),
            "{f}"
        );
    }

    #[test]
    fn text_equals_i_lowercases_both_sides_in_js_filter() {
        let p = PredicateSet {
            tag: None,
            attrs: vec![],
            texts: vec![TextPred::Equals("Foo".into(), true)],
        };
        let f = p.to_js_filter();
        assert!(
            f.contains(
                r#"(el.innerText||el.textContent||"").replace(/[ \t\r\n]+/g," ").trim().toLowerCase()==="foo""#
            ),
            "{f}"
        );
    }

    #[test]
    fn non_ci_text_variants_never_lowercase() {
        let p = PredicateSet {
            tag: None,
            attrs: vec![],
            texts: vec![
                TextPred::Contains("Buy".into(), false),
                TextPred::Equals("OK".into(), false),
            ],
        };
        let f = p.to_js_filter();
        assert!(
            f.contains(r#"(el.innerText||el.textContent||"").includes("Buy")"#),
            "{f}"
        );
        assert!(
            f.contains(
                r#"(el.innerText||el.textContent||"").replace(/[ \t\r\n]+/g," ").trim()==="OK""#
            ),
            "{f}"
        );
        // Never lowercased when the flag is off.
        assert!(!f.contains("toLowerCase"), "{f}");
    }

    // --- normalize-space unification (text_equals ↔ text_exact) -----------

    #[test]
    fn normalize_space_collapses_interior_runs_and_trims_ends() {
        assert_eq!(normalize_space("  Hello \t\n  world  "), "Hello world");
        assert_eq!(normalize_space("Hello world"), "Hello world");
        assert_eq!(normalize_space("   "), "");
        assert_eq!(normalize_space(""), "");
    }

    #[test]
    fn normalize_space_leaves_nbsp_alone_like_xpath() {
        // XPath `normalize-space()` only folds space/tab/CR/LF. Folding
        // U+00A0 here would make `text_equals` disagree with `text_exact`
        // on `&nbsp;` markup — the exact bug this normalization fixes.
        assert_eq!(normalize_space("a\u{a0}\u{a0}b"), "a\u{a0}\u{a0}b");
    }

    #[test]
    fn text_equals_normalizes_the_needle_at_compile_time() {
        // `<b>Hello   world</b>` matches XPath `normalize-space(.)="Hello
        // world"`, so the JS post-filter must compare against the collapsed
        // needle too — otherwise a needle typed with interior runs could
        // never match anything.
        let p = PredicateSet {
            tag: None,
            attrs: vec![],
            texts: vec![TextPred::Equals("Hello   world".into(), false)],
        };
        let f = p.to_js_filter();
        assert!(f.contains(r#"==="Hello world""#), "{f}");
        assert!(!f.contains(r#"==="Hello   world""#), "{f}");
    }

    #[test]
    fn text_equals_normalizes_the_page_text_side_too() {
        // Needle-side normalization alone is not enough: `<b>Hello   world</b>`
        // must also collapse before the compare, or the two sides still
        // disagree on the same markup.
        let p = PredicateSet {
            tag: None,
            attrs: vec![],
            texts: vec![TextPred::Equals("Hello world".into(), false)],
        };
        let f = p.to_js_filter();
        assert!(f.contains(r#".replace(/[ \t\r\n]+/g," ").trim()"#), "{f}");
    }

    #[test]
    fn text_equals_i_normalizes_both_sides_before_lowercasing() {
        let p = PredicateSet {
            tag: None,
            attrs: vec![],
            texts: vec![TextPred::Equals("  Hello \n WORLD ".into(), true)],
        };
        let f = p.to_js_filter();
        assert!(
            f.contains(
                r#"(el.innerText||el.textContent||"").replace(/[ \t\r\n]+/g," ").trim().toLowerCase()==="hello world""#
            ),
            "{f}"
        );
    }

    #[test]
    fn contains_and_matches_keep_raw_text() {
        // Only `Equals` normalizes — a substring/regex needle may carry
        // meaningful whitespace of its own.
        let p = PredicateSet {
            tag: None,
            attrs: vec![],
            texts: vec![
                TextPred::Contains("Buy  now".into(), false),
                TextPred::Matches(r"a\s+b".into()),
            ],
        };
        let f = p.to_js_filter();
        assert!(
            f.contains(r#"(el.innerText||el.textContent||"").includes("Buy  now")"#),
            "{f}"
        );
        assert!(
            f.contains(r#"new RegExp("a\\s+b").test((el.innerText||el.textContent||""))"#),
            "{f}"
        );
    }
}
