//! Event expectation helpers (`expect_request`/`expect_response`/
//! `expect_dialog`/`expect_download`).
//!
//! Each helper registers a one-shot subscription on a Tab's CDP event stream
//! and resolves with the first matching event. `UrlMatcher` is the shared
//! pattern type used by request/response expectations.

pub mod dialog;
pub mod download;
pub mod request;
pub mod response;

/// URL match predicate used by request/response expectations.
///
/// `Substring` matches if the URL contains the needle anywhere. `Regex` runs
/// `regex::Regex::is_match`. `From<&str>` / `From<String>` build a
/// `Substring`, while `From<regex::Regex>` builds a `Regex` variant — so
/// callers can pass any of the three to `Tab::expect_request` /
/// `Tab::expect_response`.
#[derive(Debug, Clone)]
pub enum UrlMatcher {
    Substring(String),
    Regex(regex::Regex),
}

impl UrlMatcher {
    /// Returns `true` if `url` matches this matcher.
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
