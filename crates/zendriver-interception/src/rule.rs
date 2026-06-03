//! Declarative interception rules.
//!
//! Each rule pairs a matcher (a [`UrlPattern`], or a [`HostMatcher`] for
//! [`BlockHosts`]) with one of six actions ([`Block`], [`Redirect`],
//! [`Respond`], [`Modify`], [`ModifyResponse`], [`BlockHosts`]). The actor in
//! T6 walks the rule list in registration order on each `Fetch.requestPaused`
//! event; the first rule that matches the request URL wins.
//!
//! [`Block`]: Rule::Block
//! [`Redirect`]: Rule::Redirect
//! [`Respond`]: Rule::Respond
//! [`Modify`]: Rule::Modify
//! [`ModifyResponse`]: Rule::ModifyResponse
//! [`BlockHosts`]: Rule::BlockHosts
//! [`HostMatcher`]: crate::host_matcher::HostMatcher

use std::fmt;
use std::sync::Arc;

use crate::host_matcher::HostMatcher;
use crate::types::{RequestInfo, RequestOverrides, ResponseInfo, ResponseOverrides};
use crate::url_pattern::UrlPattern;

/// A single interception rule.
///
/// Each variant carries its own [`UrlPattern`], so different rules in the same
/// [`InterceptBuilder`](crate::builder::InterceptBuilder) can match disjoint
/// URL sets. Rules are evaluated in registration order — earlier rules
/// shadow later ones for overlapping patterns.
//
// Not `Clone`: `Modify` holds an `Arc<dyn Fn ...>` which is cheap to clone,
// but the other variants own `Vec`s/`String`s that we'd rather not silently
// duplicate. The actor consumes the rule list by reference (`&Rule`) so a
// `Clone` impl is unnecessary in practice.
pub enum Rule {
    /// Abort matching requests with `Fetch.failRequest`
    /// (`errorReason: "BlockedByClient"`).
    Block {
        /// URL pattern matched against `Fetch.requestPaused.request.url`.
        pattern: UrlPattern,
    },
    /// Redirect matching requests to `to` via `Fetch.continueRequest { url }`.
    Redirect {
        /// URL pattern matched against the incoming request URL.
        from: UrlPattern,
        /// Absolute target URL substituted into the continued request.
        to: String,
    },
    /// Serve a synthesized response with `Fetch.fulfillRequest`.
    Respond {
        /// URL pattern matched against the incoming request URL.
        pattern: UrlPattern,
        /// HTTP status code returned to the page (`responseCode`).
        status: u16,
        /// Response headers as `(name, value)` pairs.
        headers: Vec<(String, String)>,
        /// Raw response body bytes (base64-encoded on the wire by the actor).
        body: Vec<u8>,
    },
    /// Rewrite the outgoing request per-field via a user closure, then
    /// continue. The closure receives the live [`RequestInfo`] and returns
    /// the [`RequestOverrides`] to apply.
    Modify {
        /// URL pattern matched against the incoming request URL.
        pattern: UrlPattern,
        /// Closure invoked per matching request to produce overrides.
        ///
        /// Wrapped in [`Arc`] so the actor can cheaply share the closure
        /// across the rule list without forcing the rule itself to be `Clone`
        /// or `Send`-by-value.
        modify: Arc<dyn Fn(&RequestInfo) -> RequestOverrides + Send + Sync>,
    },
    /// Rewrite an upstream response's status/headers per a user closure, then
    /// continue with `Fetch.continueResponse` (keeping Chrome's body). Only
    /// fires at the `Response` stage — the closure receives the live
    /// [`ResponseInfo`] and returns the [`ResponseOverrides`] to apply. A rule
    /// of this kind that matches at the `Request` stage is a no-op (there is
    /// no response yet).
    ModifyResponse {
        /// URL pattern matched against the incoming request URL.
        pattern: UrlPattern,
        /// Closure invoked per matching response to produce overrides.
        ///
        /// Wrapped in [`Arc`] for the same cheap-share reason as [`Modify`].
        ///
        /// [`Modify`]: Rule::Modify
        modify: Arc<dyn Fn(&ResponseInfo) -> ResponseOverrides + Send + Sync>,
    },
    /// Abort requests whose **host** is in a [`HostMatcher`] with
    /// `Fetch.failRequest { errorReason: "BlockedByClient" }` — the same
    /// `net::ERR_BLOCKED_BY_CLIENT` a real adblocker / Brave raises.
    ///
    /// Unlike [`Block`](Rule::Block), which globs the full URL, this matches
    /// host-set membership (exact + parent-domain suffix on dot boundaries),
    /// so a curated list of thousands of hosts is one O(1) set lookup per
    /// request rather than N glob comparisons. Powers the tracker blocklist.
    BlockHosts {
        /// Shared host set; cheap to clone across tabs/rules.
        matcher: Arc<HostMatcher>,
    },
}

impl Rule {
    /// Test whether this rule's pattern matches `url`.
    ///
    /// Delegates to the embedded [`UrlPattern::matches`]; the field selector
    /// (`pattern`, `from`) varies per variant but the semantics are identical
    /// — a CDP-style wildcard match against the full request URL.
    pub fn matches(&self, url: &str) -> bool {
        match self {
            Self::Block { pattern }
            | Self::Respond { pattern, .. }
            | Self::Modify { pattern, .. }
            | Self::ModifyResponse { pattern, .. } => pattern.matches(url),
            Self::Redirect { from, .. } => from.matches(url),
            Self::BlockHosts { matcher } => {
                crate::host_matcher::host_of(url).is_some_and(|h| matcher.is_blocked(h))
            }
        }
    }
}

// Hand-written `Debug` because the `Modify` / `ModifyResponse` variants hold
// `Arc<dyn Fn ...>`, which is not `Debug`. We print the closure as a
// placeholder so the rest of the rule (the pattern) stays inspectable.
impl fmt::Debug for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Block { pattern } => f.debug_struct("Block").field("pattern", pattern).finish(),
            Self::Redirect { from, to } => f
                .debug_struct("Redirect")
                .field("from", from)
                .field("to", to)
                .finish(),
            Self::Respond {
                pattern,
                status,
                headers,
                body,
            } => f
                .debug_struct("Respond")
                .field("pattern", pattern)
                .field("status", status)
                .field("headers", headers)
                .field("body_len", &body.len())
                .finish(),
            Self::Modify { pattern, .. } => f
                .debug_struct("Modify")
                .field("pattern", pattern)
                .field("modify", &"<closure>")
                .finish(),
            Self::ModifyResponse { pattern, .. } => f
                .debug_struct("ModifyResponse")
                .field("pattern", pattern)
                .field("modify", &"<closure>")
                .finish(),
            Self::BlockHosts { matcher } => f
                .debug_struct("BlockHosts")
                .field("hosts", &matcher.len())
                .finish(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn block_matches_via_pattern() {
        let rule = Rule::Block {
            pattern: UrlPattern::new("*/ads/*").unwrap(),
        };
        assert!(rule.matches("https://example.com/ads/banner.png"));
        assert!(!rule.matches("https://example.com/content/main.css"));
    }

    #[test]
    fn redirect_matches_via_from_field() {
        let rule = Rule::Redirect {
            from: UrlPattern::new("*/old/*").unwrap(),
            to: "https://example.com/new/replacement".into(),
        };
        assert!(rule.matches("https://example.com/old/page.html"));
        assert!(!rule.matches("https://example.com/new/page.html"));
    }

    #[test]
    fn block_hosts_matches_on_host_and_subdomain() {
        let rule = Rule::BlockHosts {
            matcher: Arc::new(HostMatcher::new(["evil.com".to_string()])),
        };
        assert!(rule.matches("https://evil.com/track.js"));
        assert!(rule.matches("https://a.b.evil.com/x?y=1"));
        assert!(!rule.matches("https://good.com/app.js"));
        assert!(!rule.matches("https://notevil.com/app.js"));

        // Debug renders the variant name + the host count (the Arc<HostMatcher>
        // is summarized, not dumped).
        let dbg = format!("{rule:?}");
        assert!(dbg.contains("BlockHosts"), "got: {dbg}");
        assert!(dbg.contains("hosts"), "got: {dbg}");
    }

    #[test]
    fn rule_modify_response_matches_and_debug() {
        let rule = Rule::ModifyResponse {
            pattern: UrlPattern::new("*/api/*").unwrap(),
            modify: Arc::new(|_resp: &ResponseInfo| ResponseOverrides {
                status: Some(418),
                ..ResponseOverrides::default()
            }),
        };
        assert!(rule.matches("https://example.com/api/users"));
        assert!(!rule.matches("https://example.com/static/app.js"));

        // Debug renders the pattern + a closure placeholder (the Arc<dyn Fn>
        // isn't Debug).
        let dbg = format!("{rule:?}");
        assert!(dbg.contains("ModifyResponse"), "got: {dbg}");
        assert!(dbg.contains("*/api/*"), "got: {dbg}");
        assert!(dbg.contains("<closure>"), "got: {dbg}");
    }
}
