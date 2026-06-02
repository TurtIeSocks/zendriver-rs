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

#[derive(Debug, Clone)]
pub(crate) enum AttrPred {
    Exact(String, String),
    Contains(String, String),
    StartsWith(String, String),
    EndsWith(String, String),
    Has(String),
    Regex(String, String), // (name, pattern) — JS post-filter, not CSS
}

#[derive(Debug, Clone)]
pub(crate) enum TextPred {
    Contains(String),
    Equals(String),
    Matches(String), // regex pattern
}

/// Quote a value as a JSON string (`"v"`) — safe for both CSS attribute
/// values and JS string literals (the selector is later JSON-embedded into
/// the JS source, double-escaping correctly).
#[allow(dead_code)] // consumed in T6 (predicate terminal wiring)
fn q(v: &str) -> String {
    json!(v).to_string()
}

impl PredicateSet {
    #[allow(dead_code)] // consumed in T6 (conflict guard at terminal)
    pub(crate) fn is_empty(&self) -> bool {
        self.tag.is_none() && self.attrs.is_empty() && self.texts.is_empty()
    }

    /// Structural predicates → a CSS selector. `attr_regex` + text predicates
    /// are post-filters and are NOT emitted here. Empty set → `"*"`.
    #[allow(dead_code)] // consumed in T6 (resolve_predicate_many)
    pub(crate) fn to_css_selector(&self) -> String {
        let mut s = self.tag.clone().unwrap_or_default();
        for a in &self.attrs {
            match a {
                AttrPred::Exact(n, v) => s.push_str(&format!("[{n}={}]", q(v))),
                AttrPred::Contains(n, v) => s.push_str(&format!("[{n}*={}]", q(v))),
                AttrPred::StartsWith(n, v) => s.push_str(&format!("[{n}^={}]", q(v))),
                AttrPred::EndsWith(n, v) => s.push_str(&format!("[{n}$={}]", q(v))),
                AttrPred::Has(n) => s.push_str(&format!("[{n}]")),
                AttrPred::Regex(..) => {}
            }
        }
        if s.is_empty() { "*".to_string() } else { s }
    }

    /// Post-filter predicates (`attr_regex` + all text predicates) → a JS
    /// boolean expression over a bound `el`. Returns `"true"` when there are
    /// no post-filters (so the caller can always `.filter(el => <expr>)`).
    #[allow(dead_code)] // consumed in T6 (resolve_predicate_many)
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
                TextPred::Contains(s) => checks.push(format!("{TXT}.includes({})", q(s))),
                TextPred::Equals(s) => checks.push(format!("{TXT}.trim()==={}", q(s))),
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
                AttrPred::Exact("data-role".into(), "card".into()),
                AttrPred::Contains("class".into(), "active".into()),
                AttrPred::StartsWith("id".into(), "item-".into()),
                AttrPred::EndsWith("data-x".into(), "-end".into()),
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
                TextPred::Contains("Buy".into()),
                TextPred::Equals("OK".into()),
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
}
