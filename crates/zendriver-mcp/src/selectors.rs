//! Selector argument type shared by find / action tools.
//!
//! Validates exactly-one-of selector kinds and exposes (in a later
//! dispatch) an `apply` helper that configures a zendriver `FindBuilder`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Selector arg struct on every find / action tool.
///
/// Caller supplies exactly one of `css`, `xpath`, `text`, `text_exact`,
/// `text_regex`, or `role`. `role_name` is an optional modifier on `role`.
/// Other fields are tuning knobs with defaults that match the lib's
/// `FindBuilder` defaults.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Selector {
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
    /// Caller supplied zero or 2+ selector kinds — exactly one is required.
    #[error("Selector requires exactly one of: css, xpath, text, text_exact, text_regex, role")]
    NoneOrMultiple,
    /// Caller set `role_name` without setting `role`.
    #[error("`role_name` requires `role` to also be set")]
    OrphanRoleName,
}

impl Selector {
    /// Returns `Ok(())` iff exactly one selector kind is present and
    /// `role_name` (if any) is paired with `role`.
    pub fn validate(&self) -> Result<(), SelectorError> {
        let n = [
            self.css.is_some(),
            self.xpath.is_some(),
            self.text.is_some(),
            self.text_exact.is_some(),
            self.text_regex.is_some(),
            self.role.is_some(),
        ]
        .into_iter()
        .filter(|b| *b)
        .count();
        if n != 1 {
            return Err(SelectorError::NoneOrMultiple);
        }
        if self.role_name.is_some() && self.role.is_none() {
            return Err(SelectorError::OrphanRoleName);
        }
        Ok(())
    }

    // TODO(find-tools): impl Selector::apply(&self, FindBuilder) -> FindBuilder
    // once tools/find.rs lands. The bridge lives there because it depends on
    // zendriver's FindBuilder + AriaRole + Regex types.
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
}
