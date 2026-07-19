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
    Equals(String, bool),
    Matches(String), // regex pattern
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
                TextPred::Equals(s, false) => checks.push(format!("{TXT}.trim()==={}", q(s))),
                TextPred::Equals(s, true) => checks.push(format!(
                    "{TXT}.trim().toLowerCase()==={}",
                    q(&s.to_lowercase())
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
            f.contains(r#"(el.innerText||el.textContent||"").trim()==="OK""#),
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
            f.contains(r#"(el.innerText||el.textContent||"").trim().toLowerCase()==="foo""#),
            "{f}"
        );
    }

    #[test]
    fn non_ci_text_variants_stay_byte_identical() {
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
            f.contains(r#"(el.innerText||el.textContent||"").trim()==="OK""#),
            "{f}"
        );
        // Never lowercased when the flag is off.
        assert!(!f.contains("toLowerCase"), "{f}");
    }
}
