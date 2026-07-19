//! Selector argument type shared by find / action tools.
//!
//! Validates exactly-one-of selector kinds and exposes (in a later
//! dispatch) an `apply` helper that configures a zendriver `FindBuilder`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Attribute-value comparison operator used in [`AttrPredicate`].
///
/// Determines how the actual element attribute value is compared against
/// the `value` field. When `op` is [`AttrOp::Has`], `value` is ignored.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum AttrOp {
    /// `[name="value"]` — exact match.
    Eq,
    /// `[name*="value"]` — contains substring.
    Contains,
    /// `[name^="value"]` — starts with.
    StartsWith,
    /// `[name$="value"]` — ends with.
    EndsWith,
    /// `[name]` — attribute is present regardless of value.
    Has,
    /// JS regex test on the attribute value (post-filter).
    Regex,
}

/// One attribute predicate in a predicate-mode [`Selector`].
///
/// Combine multiple predicates via `attrs: [...]`; they are AND-ed.
/// `value` is required for every `op` except [`AttrOp::Has`].
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttrPredicate {
    /// Attribute name (e.g. `"data-role"`, `"class"`, `"href"`).
    pub name: String,
    /// Comparison operator.
    pub op: AttrOp,
    /// Comparison value. Required for all operators except `has`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Compare case-insensitively (CSS `[name=value i]`). Valid only for
    /// `eq`/`contains`/`starts_with`/`ends_with` — `has` has no value to
    /// compare and `regex` is already case-insensitive via an inline
    /// `(?i)` pattern flag, so setting this with either is a validation
    /// error.
    #[serde(default)]
    pub case_insensitive: bool,
}

/// Selector arg struct on every find / action tool.
///
/// Caller supplies **exactly one** of the following selector kinds:
/// - single-selector mode: `css`, `xpath`, `text`, `text_exact`, `text_regex`, or `role`
/// - predicate mode: any combination of `tag`, `attrs`, `text`, `text_exact`, `text_regex`
///
/// In predicate mode, `tag` and `attrs` are combinable with each other and
/// with `text`/`text_exact`/`text_regex` (AND-ed). They are mutually exclusive
/// with `css`, `xpath`, and `role`.
///
/// `role_name` is an optional modifier on `role`.
/// Other fields are tuning knobs with defaults that match the lib's
/// `FindBuilder` defaults.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Selector {
    // --- single-selector kinds ---
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub css: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xpath: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_exact: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_regex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_name: Option<String>,
    // --- predicate mode ---
    /// HTML tag name filter (e.g. `"button"`, `"a"`). Predicate mode only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// Attribute predicates (AND-ed). Predicate mode only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attrs: Vec<AttrPredicate>,
    /// Compare the predicate-mode text post-filter case-insensitively
    /// (`text` → `containing_text_i`, `text_exact` → `text_equals_i`).
    /// Predicate mode only; ignored in single-selector mode, where `text`
    /// is already a case-insensitive substring match and `text_exact` has
    /// no case-insensitive single-selector form.
    #[serde(default)]
    pub text_case_insensitive: bool,
    // --- modifiers ---
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nth: Option<usize>,
    #[serde(default = "default_visible_only")]
    pub visible_only: bool,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_id: Option<String>,
}

fn default_visible_only() -> bool {
    true
}
fn default_timeout_ms() -> u64 {
    5000
}

/// Validation failure for [`Selector`].
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SelectorError {
    /// Caller supplied zero selector-kind signals.
    #[error(
        "Selector requires exactly one of: css, xpath, text, text_exact, text_regex, role, or a predicate group (tag/attrs)"
    )]
    NoneOrMultiple,
    /// Caller set `role_name` without setting `role`.
    #[error("`role_name` requires `role` to also be set")]
    OrphanRoleName,
    /// Caller combined predicate fields (`tag`/`attrs`) with an incompatible
    /// single-selector kind (`css`, `xpath`, or `role`).
    #[error(
        "predicate fields (`tag`/`attrs`) cannot be combined with `css`, `xpath`, or `role`; use one selector style per query"
    )]
    PredicateConflict,
    /// An [`AttrPredicate`] with an operator that requires a value was given
    /// without one.
    #[error("attribute predicate `{name}` with op `{op:?}` requires a `value` field")]
    AttrValueRequired { name: String, op: AttrOp },
    /// An [`AttrPredicate`] set `case_insensitive` on an operator where it
    /// has no effect.
    #[error(
        "attribute predicate `{name}` with op `{op:?}` does not support `case_insensitive` \
         (`has` has no value to compare; `regex` is already case-insensitive via an inline \
         `(?i)` pattern flag)"
    )]
    CaseInsensitiveNotApplicable { name: String, op: AttrOp },
}

impl Selector {
    /// Returns `true` when any predicate-mode field is set.
    fn has_predicates(&self) -> bool {
        self.tag.is_some() || !self.attrs.is_empty()
    }

    /// Returns `Ok(())` iff the selector is valid:
    /// - exactly one selector kind (single-selector OR predicate group + optional text*)
    /// - predicate group not combined with css/xpath/role
    /// - `role_name` paired with `role`
    /// - every `AttrPredicate` that needs a value has one
    pub fn validate(&self) -> Result<(), SelectorError> {
        let has_pred = self.has_predicates();
        let single_count = [
            self.css.is_some(),
            self.xpath.is_some(),
            self.role.is_some(),
        ]
        .into_iter()
        .filter(|b| *b)
        .count();
        let text_count = [
            self.text.is_some(),
            self.text_exact.is_some(),
            self.text_regex.is_some(),
        ]
        .into_iter()
        .filter(|b| *b)
        .count();

        if has_pred {
            // Predicate mode: tag/attrs are the primary kind; text* fields are
            // optional AND-ed post-filters. css/xpath/role are incompatible.
            if single_count > 0 {
                return Err(SelectorError::PredicateConflict);
            }
            // text* still must be at most one
            if text_count > 1 {
                return Err(SelectorError::NoneOrMultiple);
            }
            // Validate individual attr predicates
            for ap in &self.attrs {
                if ap.value.is_none() && ap.op != AttrOp::Has {
                    return Err(SelectorError::AttrValueRequired {
                        name: ap.name.clone(),
                        op: ap.op.clone(),
                    });
                }
                if ap.case_insensitive && matches!(ap.op, AttrOp::Has | AttrOp::Regex) {
                    return Err(SelectorError::CaseInsensitiveNotApplicable {
                        name: ap.name.clone(),
                        op: ap.op.clone(),
                    });
                }
            }
        } else {
            // Single-selector mode: exactly one of css/xpath/text*/role required.
            let n = single_count + text_count;
            if n != 1 {
                return Err(SelectorError::NoneOrMultiple);
            }
        }

        if self.role_name.is_some() && self.role.is_none() {
            return Err(SelectorError::OrphanRoleName);
        }
        Ok(())
    }

    // The `Selector` → `FindBuilder` bridge lives in [`crate::tools::find`]
    // (specifically [`crate::tools::find::resolve`] and `resolve_all`). The
    // bridge depends on zendriver types (`FindBuilder`, `AriaRole`, `Regex`)
    // that `selectors.rs` deliberately doesn't pull in — keeping this wire
    // struct decoupled from CDP-shaped concerns.
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Selector {
        Selector {
            css: None,
            xpath: None,
            text: None,
            text_exact: None,
            text_regex: None,
            role: None,
            role_name: None,
            tag: None,
            attrs: vec![],
            text_case_insensitive: false,
            nth: None,
            visible_only: true,
            timeout_ms: 5000,
            frame_id: None,
        }
    }

    #[test]
    fn validate_rejects_zero_selectors() {
        let s = base();
        assert_eq!(s.validate(), Err(SelectorError::NoneOrMultiple));
    }

    #[test]
    fn validate_rejects_two_selectors() {
        let mut s = base();
        s.css = Some("#x".into());
        s.text = Some("hi".into());
        assert_eq!(s.validate(), Err(SelectorError::NoneOrMultiple));
    }

    #[test]
    fn validate_accepts_single_css() {
        let mut s = base();
        s.css = Some("#x".into());
        assert!(s.validate().is_ok());
    }

    #[test]
    fn validate_accepts_role_with_name() {
        let mut s = base();
        s.role = Some("button".into());
        s.role_name = Some("Submit".into());
        assert!(s.validate().is_ok());
    }

    #[test]
    fn validate_rejects_role_name_without_role() {
        let mut s = base();
        s.text = Some("hi".into());
        s.role_name = Some("Submit".into());
        assert_eq!(s.validate(), Err(SelectorError::OrphanRoleName));
    }

    // --- T1: predicate mode tests ---

    #[test]
    fn validate_accepts_tag_only() {
        let mut s = base();
        s.tag = Some("button".into());
        assert!(s.validate().is_ok());
    }

    #[test]
    fn validate_accepts_tag_with_text() {
        let mut s = base();
        s.tag = Some("a".into());
        s.text = Some("Buy".into());
        assert!(s.validate().is_ok());
    }

    #[test]
    fn validate_rejects_predicate_plus_css() {
        let mut s = base();
        s.tag = Some("div".into());
        s.css = Some(".foo".into());
        assert_eq!(s.validate(), Err(SelectorError::PredicateConflict));
    }

    #[test]
    fn validate_rejects_attr_predicate_missing_value() {
        let mut s = base();
        s.attrs = vec![AttrPredicate {
            name: "data-id".into(),
            op: AttrOp::Eq,
            value: None,
            case_insensitive: false,
        }];
        assert!(matches!(
            s.validate(),
            Err(SelectorError::AttrValueRequired { .. })
        ));
    }

    #[test]
    fn validate_accepts_has_attr_without_value() {
        let mut s = base();
        s.attrs = vec![AttrPredicate {
            name: "data-ready".into(),
            op: AttrOp::Has,
            value: None,
            case_insensitive: false,
        }];
        assert!(s.validate().is_ok());
    }

    // --- case-insensitive predicate matchers (Phase 3, item 1) ------------

    #[test]
    fn validate_accepts_case_insensitive_eq() {
        let mut s = base();
        s.attrs = vec![AttrPredicate {
            name: "class".into(),
            op: AttrOp::Eq,
            value: Some("Primary".into()),
            case_insensitive: true,
        }];
        assert!(s.validate().is_ok());
    }

    #[test]
    fn validate_rejects_case_insensitive_has() {
        let mut s = base();
        s.attrs = vec![AttrPredicate {
            name: "data-ready".into(),
            op: AttrOp::Has,
            value: None,
            case_insensitive: true,
        }];
        assert!(matches!(
            s.validate(),
            Err(SelectorError::CaseInsensitiveNotApplicable { .. })
        ));
    }

    #[test]
    fn validate_rejects_case_insensitive_regex() {
        let mut s = base();
        s.attrs = vec![AttrPredicate {
            name: "href".into(),
            op: AttrOp::Regex,
            value: Some(r"^\d+$".into()),
            case_insensitive: true,
        }];
        assert!(matches!(
            s.validate(),
            Err(SelectorError::CaseInsensitiveNotApplicable { .. })
        ));
    }

    #[test]
    fn validate_accepts_text_case_insensitive_in_predicate_mode() {
        let mut s = base();
        s.tag = Some("span".into());
        s.text = Some("hello".into());
        s.text_case_insensitive = true;
        assert!(s.validate().is_ok());
    }
}
