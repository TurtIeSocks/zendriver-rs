//! Host-set matcher for the tracker/fingerprinter blocklist.
//!
//! [`HostMatcher`] holds a set of domain names and answers [`is_blocked`]
//! queries with a suffix-on-dot walk: `a.b.evil.com` is blocked if
//! `evil.com` (or any ancestor) is in the set.
//!
//! [`host_of`] extracts the host component from a raw URL string without
//! requiring a URL-parsing dependency — it handles the common `scheme://host`
//! prefix and strips any trailing port + path.
//!
//! [`is_blocked`]: HostMatcher::is_blocked

use std::collections::HashSet;

/// A compiled set of blocked host names with subdomain-walk semantics.
///
/// Constructed once from a domain list, then shared (via [`Arc`]) across
/// rule evaluations per request.
///
/// [`Arc`]: std::sync::Arc
#[derive(Debug, Clone)]
pub struct HostMatcher {
    /// Canonicalized (ASCII-lowercased) host names.
    blocked: HashSet<String>,
}

impl HostMatcher {
    /// Build a matcher from an iterator of domain strings.
    ///
    /// Each entry is lowercased and leading/trailing whitespace stripped.
    /// Blank entries and those starting with `#` (comments) are silently
    /// ignored so the caller can pass lines from a plain-text host list
    /// directly.
    pub fn new(domains: impl IntoIterator<Item = String>) -> Self {
        let blocked = domains
            .into_iter()
            .map(|s| s.trim().trim_end_matches('.').to_ascii_lowercase())
            .filter(|s| !s.is_empty() && !s.starts_with('#'))
            .collect();
        Self { blocked }
    }

    /// Number of distinct blocked host names.
    pub fn len(&self) -> usize {
        self.blocked.len()
    }

    /// Returns `true` if no hosts are blocked.
    pub fn is_empty(&self) -> bool {
        self.blocked.is_empty()
    }

    /// Returns `true` if `host` is blocked.
    ///
    /// Matching is suffix-on-dot: `a.b.evil.com` is blocked when `evil.com`
    /// (or any ancestor up to the bare root) is in the set.
    ///
    /// - Exact match first.
    /// - Then strips the leftmost label repeatedly until a match or exhausted.
    ///
    /// `host` is compared case-insensitively.
    pub fn is_blocked(&self, host: &str) -> bool {
        let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
        let mut cursor: &str = &host;
        loop {
            if self.blocked.contains(cursor) {
                return true;
            }
            // Strip the leftmost label (up to and including the first dot).
            match cursor.find('.') {
                Some(pos) => cursor = &cursor[pos + 1..],
                None => return false,
            }
        }
    }
}

/// Extract the host component from a raw URL string.
///
/// Handles the common `scheme://authority/path` form: strips everything up
/// to and including `://`, then takes the authority portion (up to the first
/// `/`, `?`, or `#`). Trims a trailing port (`:NNN`).
///
/// Returns `None` when no `://` separator is present (malformed or
/// non-standard URL).
pub fn host_of(url: &str) -> Option<&str> {
    // Find scheme-authority boundary.
    let after_scheme = url.split_once("://")?.1;
    // Authority ends at the first `/`, `?`, or `#`.
    let authority = match after_scheme.find(['/', '?', '#']) {
        Some(pos) => &after_scheme[..pos],
        None => after_scheme,
    };
    // Strip userinfo (`user:pass@host`).
    let host_and_port = match authority.rfind('@') {
        Some(pos) => &authority[pos + 1..],
        None => authority,
    };
    // Strip port, but be careful with IPv6 literals like `[::1]:8080`.
    let host = if host_and_port.starts_with('[') {
        // IPv6: authority is `[addr]` or `[addr]:port` — keep the brackets.
        match host_and_port.find(']') {
            Some(pos) => &host_and_port[..=pos],
            None => host_and_port,
        }
    } else {
        match host_and_port.rfind(':') {
            Some(pos) => &host_and_port[..pos],
            None => host_and_port,
        }
    };
    Some(host)
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    // --- HostMatcher ----------------------------------------------------------

    #[test]
    fn exact_match_blocked() {
        let m = HostMatcher::new(["evil.com".to_string()]);
        assert!(m.is_blocked("evil.com"));
    }

    #[test]
    fn subdomain_of_listed_domain_is_blocked() {
        let m = HostMatcher::new(["evil.com".to_string()]);
        assert!(m.is_blocked("tracker.evil.com"));
        assert!(m.is_blocked("a.b.tracker.evil.com"));
    }

    #[test]
    fn unrelated_host_not_blocked() {
        let m = HostMatcher::new(["evil.com".to_string()]);
        assert!(!m.is_blocked("good.com"));
        assert!(!m.is_blocked("notevil.com"));
        // suffix match must be on a dot boundary
        assert!(!m.is_blocked("totallyevil.com"));
    }

    #[test]
    fn bare_root_in_set_blocks_all_subdomains() {
        // if someone lists a TLD or bare root (unusual but valid)
        let m = HostMatcher::new(["example.com".to_string()]);
        assert!(m.is_blocked("example.com"));
        assert!(m.is_blocked("sub.example.com"));
    }

    #[test]
    fn case_insensitive_match() {
        let m = HostMatcher::new(["Evil.Com".to_string()]);
        assert!(m.is_blocked("EVIL.COM"));
        assert!(m.is_blocked("Tracker.Evil.Com"));
    }

    #[test]
    fn empty_matcher_blocks_nothing() {
        let m = HostMatcher::new(std::iter::empty());
        assert!(!m.is_blocked("evil.com"));
    }

    #[test]
    fn comment_and_blank_lines_ignored() {
        let lines = vec![
            "# this is a comment".to_string(),
            "".to_string(),
            "  ".to_string(),
            "tracker.example.com".to_string(),
            "# another comment".to_string(),
        ];
        let m = HostMatcher::new(lines);
        assert!(m.is_blocked("tracker.example.com"));
        assert!(!m.is_blocked("example.com")); // only the exact entry, not parent
    }

    #[test]
    fn single_label_host_no_infinite_loop() {
        // A host with no dot should not match and must not loop forever.
        let m = HostMatcher::new(["localhost".to_string()]);
        assert!(m.is_blocked("localhost"));
        // A different single-label host misses cleanly.
        assert!(!m.is_blocked("otherhost"));
    }

    // --- host_of --------------------------------------------------------------

    #[test]
    fn host_of_simple_url() {
        assert_eq!(host_of("https://example.com/path"), Some("example.com"));
    }

    #[test]
    fn host_of_with_port() {
        assert_eq!(host_of("http://example.com:8080/path"), Some("example.com"));
    }

    #[test]
    fn host_of_no_path() {
        assert_eq!(host_of("https://example.com"), Some("example.com"));
    }

    #[test]
    fn host_of_with_query() {
        assert_eq!(host_of("https://example.com?foo=bar"), Some("example.com"));
    }

    #[test]
    fn host_of_with_fragment() {
        assert_eq!(host_of("https://example.com#section"), Some("example.com"));
    }

    #[test]
    fn host_of_missing_scheme_separator() {
        assert_eq!(host_of("not-a-url"), None);
    }

    #[test]
    fn host_of_ipv6() {
        assert_eq!(host_of("https://[::1]:443/path"), Some("[::1]"));
    }

    #[test]
    fn host_of_with_userinfo() {
        assert_eq!(
            host_of("https://user:pass@example.com/path"),
            Some("example.com")
        );
    }

    // --- Integration: host_of + HostMatcher -----------------------------------

    #[test]
    fn is_blocked_using_host_of() {
        let m = HostMatcher::new(["fingerprinter.io".to_string()]);
        let url = "https://cdn.fingerprinter.io/track.js?v=1";
        let host = host_of(url).expect("host_of returned None");
        assert!(m.is_blocked(host));
    }
}
