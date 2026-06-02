//! Shared URL matcher used by `expect_*` and the network `monitor`.

/// URL match predicate used by request/response expectations.
///
/// `From<&str>` / `From<String>` build a [`UrlMatcher::Substring`], while
/// `From<regex::Regex>` builds a [`UrlMatcher::Regex`] — so callers can pass
/// any of the three to [`crate::Tab::expect_request`] /
/// [`crate::Tab::expect_response`].
///
/// # Examples
///
/// ```
/// use zendriver::UrlMatcher;
/// let m: UrlMatcher = "/api/".into();
/// assert!(m.matches("https://example.com/api/users"));
/// assert!(!m.matches("https://example.com/static/app.js"));
/// ```
#[derive(Debug, Clone)]
pub enum UrlMatcher {
    /// Matches if the URL contains the needle anywhere.
    Substring(String),
    /// Matches via `regex::Regex::is_match`.
    Regex(regex::Regex),
}

impl UrlMatcher {
    /// Returns `true` if `url` matches this matcher.
    ///
    /// # Examples
    ///
    /// ```
    /// use zendriver::UrlMatcher;
    /// let re = regex::Regex::new(r"^https://example\.com/api/").unwrap();
    /// let m: UrlMatcher = re.into();
    /// assert!(m.matches("https://example.com/api/v1/users"));
    /// assert!(!m.matches("https://example.com/static/app.js"));
    /// ```
    pub fn matches(&self, url: &str) -> bool {
        match self {
            Self::Substring(s) => url.contains(s.as_str()),
            Self::Regex(re) => re.is_match(url),
        }
    }
}

impl From<&str> for UrlMatcher {
    fn from(s: &str) -> Self {
        Self::Substring(s.to_string())
    }
}

impl From<String> for UrlMatcher {
    fn from(s: String) -> Self {
        Self::Substring(s)
    }
}

impl From<regex::Regex> for UrlMatcher {
    fn from(re: regex::Regex) -> Self {
        Self::Regex(re)
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn substring_matches_when_url_contains_needle() {
        let m = UrlMatcher::Substring("/api/".to_string());
        assert!(m.matches("https://example.com/api/users"));
        assert!(!m.matches("https://example.com/static/app.js"));
    }

    #[test]
    fn regex_matches_pattern() {
        let re = regex::Regex::new(r"^https://example\.com/api/v\d+/").unwrap();
        let m = UrlMatcher::Regex(re);
        assert!(m.matches("https://example.com/api/v1/users"));
        assert!(m.matches("https://example.com/api/v42/orders"));
        assert!(!m.matches("https://example.com/api/users"));
    }

    #[test]
    fn from_str_builds_substring_variant() {
        let m: UrlMatcher = "/api/".into();
        match m {
            UrlMatcher::Substring(s) => assert_eq!(s, "/api/"),
            UrlMatcher::Regex(_) => panic!("expected Substring variant"),
        }
    }
}
