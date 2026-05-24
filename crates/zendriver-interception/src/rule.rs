//! Declarative interception rules.
//!
//! Each rule pairs a [`UrlPattern`] with one of four actions ([`Block`],
//! [`Redirect`], [`Respond`], [`Modify`]). The actor in T6 walks the rule list
//! in registration order on each `Fetch.requestPaused` event; the first rule
//! whose pattern matches the request URL wins.
//!
//! [`Block`]: Rule::Block
//! [`Redirect`]: Rule::Redirect
//! [`Respond`]: Rule::Respond
//! [`Modify`]: Rule::Modify

use std::fmt;
use std::sync::Arc;

use crate::types::{RequestInfo, RequestOverrides};
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
            | Self::Modify { pattern, .. } => pattern.matches(url),
            Self::Redirect { from, .. } => from.matches(url),
        }
    }
}

// Hand-written `Debug` because the `Modify` variant holds `Arc<dyn Fn ...>`,
// which is not `Debug`. We print the closure as a placeholder so the rest of
// the rule (the pattern) stays inspectable.
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
}
