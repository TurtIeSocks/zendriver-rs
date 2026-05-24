//! CDP-style URL pattern matching.
//!
//! Compiles a CDP wildcard pattern (`*` matches any sequence, `?` matches a
//! single character) into a [`regex::Regex`]. All other regex metacharacters
//! in the input are escaped so the user-facing surface stays the simple CDP
//! syntax used by `Fetch.RequestPattern.urlPattern`.

use regex::Regex;

use crate::error::InterceptionError;

/// A compiled CDP URL pattern.
///
/// Constructed via [`UrlPattern::new`] from a string using CDP wildcard
/// syntax. Use [`UrlPattern::matches`] to test URLs against it, and
/// [`UrlPattern::pattern_str`] to recover the original pattern (e.g. when
/// forwarding to `Fetch.enable { patterns }`).
#[derive(Debug, Clone)]
pub struct UrlPattern {
    pattern: String,
    regex: Regex,
}

impl UrlPattern {
    /// Compile a CDP-style URL pattern.
    ///
    /// `*` matches any sequence of characters (including empty), `?` matches
    /// exactly one character, and every other regex metacharacter is escaped
    /// so the user sees the simple wildcard surface.
    ///
    /// Returns [`InterceptionError::InvalidPattern`] if the resulting regex
    /// fails to compile.
    #[allow(clippy::result_large_err)] // InterceptionError shape fixed by error.rs; boxing is a cross-cutting decision tracked separately.
    pub fn new(pattern: impl Into<String>) -> Result<Self, InterceptionError> {
        let pattern = pattern.into();
        let regex_src = compile_to_regex(&pattern);
        let regex = Regex::new(&regex_src)
            .map_err(|e| InterceptionError::InvalidPattern(format!("{pattern}: {e}")))?;
        Ok(Self { pattern, regex })
    }

    /// Test whether `url` matches this pattern.
    pub fn matches(&self, url: &str) -> bool {
        self.regex.is_match(url)
    }

    /// The original pattern string passed to [`UrlPattern::new`].
    ///
    /// Useful when forwarding the pattern to CDP via
    /// `Fetch.enable { patterns: [{ urlPattern: ... }] }`.
    pub fn pattern_str(&self) -> &str {
        &self.pattern
    }
}

/// Convert a CDP wildcard pattern into an anchored regex source.
///
/// `*` → `.*`, `?` → `.`, all other regex metacharacters are escaped via
/// [`regex::escape`]. The result is anchored at both ends so `matches`
/// behaves as a full-URL match.
fn compile_to_regex(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len() + 4);
    out.push('^');
    for ch in pattern.chars() {
        match ch {
            '*' => out.push_str(".*"),
            '?' => out.push('.'),
            other => out.push_str(&regex::escape(&other.to_string())),
        }
    }
    out.push('$');
    out
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_star_matches_all() {
        let p = UrlPattern::new("*").unwrap();
        assert!(p.matches("https://example.com/"));
        assert!(p.matches("http://foo.bar/baz?qux=1"));
        assert!(p.matches(""));
        assert_eq!(p.pattern_str(), "*");
    }

    #[test]
    fn subdomain_wildcard_matches() {
        let p = UrlPattern::new("*.example.com/*").unwrap();
        assert!(p.matches("https://api.example.com/foo"));
        assert!(p.matches("ws://cdn.example.com/socket"));
        assert!(!p.matches("https://example.org/foo"));
    }

    #[test]
    fn invalid_pattern_errors() {
        // Our wildcard expansion escapes every regex metachar in the input
        // so syntax-level failures (`[`, `\`, etc.) cannot happen via user
        // input. The deterministic failure path is `regex`'s default
        // 10 MiB compiled-size limit: enough `*` wildcards expand into
        // enough `.*` repetition to trip it.
        let huge = "*".repeat(50_000);
        let err = UrlPattern::new(huge).expect_err("expected size-limit failure");
        match err {
            InterceptionError::InvalidPattern(msg) => {
                assert!(
                    msg.contains("exceeds size limit") || msg.contains("size"),
                    "expected size-limit message, got: {msg}",
                );
            }
            other => panic!("unexpected error variant: {other:?}"),
        }
    }
}
