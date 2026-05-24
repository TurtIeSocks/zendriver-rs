//! [`InterceptBuilder`] — fluent rule + pattern registration.
//!
//! Two-phase API:
//! - **Configure**: chain [`block`], [`redirect`], [`respond`],
//!   [`modify_request`] for declarative rules, plus [`pattern`] /
//!   [`at_request`] / [`at_response`] / [`resource`] to control which CDP
//!   `Fetch.RequestPattern` entries are sent on `Fetch.enable`.
//! - **Activate**: `start()` spawns the actor task (Task 7), or
//!   `subscribe()` returns a `Stream<Item = PausedRequest>` (Task 7) — both
//!   deferred to a later task. This task only ships the type + builder
//!   methods.
//!
//! The `tab` field is a borrow of [`SessionHandle`] (not the full `Tab` from
//! `zendriver` core) — this crate must not depend on `zendriver` (cycle).
//! `Tab::intercept()` in Task 7 constructs the builder via
//! `InterceptBuilder::new(self.session())`.
//!
//! [`block`]: InterceptBuilder::block
//! [`redirect`]: InterceptBuilder::redirect
//! [`respond`]: InterceptBuilder::respond
//! [`modify_request`]: InterceptBuilder::modify_request
//! [`pattern`]: InterceptBuilder::pattern
//! [`at_request`]: InterceptBuilder::at_request
//! [`at_response`]: InterceptBuilder::at_response
//! [`resource`]: InterceptBuilder::resource

use std::sync::Arc;

use zendriver_transport::SessionHandle;

use crate::error::InterceptionError;
use crate::rule::Rule;
use crate::types::{RequestInfo, RequestOverrides, RequestStage, ResourceType};
use crate::url_pattern::UrlPattern;

/// A pending `Fetch.RequestPattern` entry to send on `Fetch.enable`.
///
/// CDP's [`Fetch.RequestPattern`] takes an optional `urlPattern`,
/// `resourceType`, and `requestStage`. We mirror it 1:1 here. The builder
/// accumulates these via [`InterceptBuilder::pattern`] / `at_request` /
/// `at_response` / `resource`, mutating the last-pushed entry per chain — so
/// `builder.pattern("*").at_response().resource(Image)` produces a single
/// `RequestPattern` with all three fields set.
///
/// [`Fetch.RequestPattern`]: https://chromedevtools.github.io/devtools-protocol/tot/Fetch/#type-RequestPattern
#[derive(Debug, Clone, Default)]
pub struct RequestPattern {
    /// URL pattern in CDP wildcard syntax. `None` means "match any URL"
    /// (CDP default).
    pub url_pattern: Option<String>,
    /// Resource type filter (e.g. `Image`, `XHR`). `None` means "all types".
    pub resource_type: Option<ResourceType>,
    /// Lifecycle stage at which to pause. `None` means CDP's default
    /// (`Request`).
    pub request_stage: Option<RequestStage>,
}

/// Fluent builder for rule-based interception against a single tab session.
///
/// Construct via `Tab::intercept()` (gated `feature = "interception"`, wired
/// in Task 7). Chain configuration methods to register rules and declare CDP
/// `Fetch.enable` patterns, then call [`start`](Self::start) (Task 7) to
/// activate the background actor or [`subscribe`](Self::subscribe) (Task 7)
/// for the stream-driven escape hatch.
///
/// `'tab` ties the builder's lifetime to the tab's session — the borrow lasts
/// only until `start()` / `subscribe()` consumes the builder.
//
// No `Debug`: `Rule::Modify` carries an `Arc<dyn Fn ...>` (not `Debug`); the
// inner `Vec<Rule>` therefore can't auto-derive. If callers want diagnostics,
// `rules_count()` is exposed in tests via `pub(crate) fn rules_count`.
pub struct InterceptBuilder<'tab> {
    #[allow(dead_code)] // Wired by T6/T7 (actor spawn + Fetch.enable dispatch).
    tab: &'tab SessionHandle,
    patterns: Vec<RequestPattern>,
    rules: Vec<Rule>,
}

impl<'tab> InterceptBuilder<'tab> {
    /// Construct a fresh builder bound to `tab`'s session.
    ///
    /// `pub(crate)` so the only public entry point is the future
    /// `Tab::intercept()` shim in Task 7 — users never invoke this directly.
    //
    // `dead_code` until Task 7 wires `Tab::intercept()` to call this.
    // Currently only exercised via the in-crate builder tests.
    #[allow(dead_code)]
    pub(crate) fn new(tab: &'tab SessionHandle) -> Self {
        Self {
            tab,
            patterns: Vec::new(),
            rules: Vec::new(),
        }
    }

    /// Push a new pattern entry with the given URL pattern string.
    ///
    /// Subsequent [`at_request`](Self::at_request) /
    /// [`at_response`](Self::at_response) / [`resource`](Self::resource) calls
    /// mutate this newest entry, so a chain like
    /// `.pattern("*").at_response().resource(ResourceType::XHR)` produces one
    /// `RequestPattern` with all three fields populated.
    #[must_use]
    pub fn pattern(mut self, pattern: impl Into<String>) -> Self {
        self.patterns.push(RequestPattern {
            url_pattern: Some(pattern.into()),
            ..RequestPattern::default()
        });
        self
    }

    /// Pause matching requests at the `Request` stage on the most-recently
    /// pushed pattern.
    ///
    /// If no pattern has been pushed yet, this creates an empty one (matches
    /// every URL by CDP default) and sets the stage on it.
    #[must_use]
    pub fn at_request(mut self) -> Self {
        self.ensure_pattern().request_stage = Some(RequestStage::Request);
        self
    }

    /// Pause matching requests at the `Response` stage on the most-recently
    /// pushed pattern.
    #[must_use]
    pub fn at_response(mut self) -> Self {
        self.ensure_pattern().request_stage = Some(RequestStage::Response);
        self
    }

    /// Restrict the most-recently pushed pattern to a single resource type.
    #[must_use]
    pub fn resource(mut self, kind: ResourceType) -> Self {
        self.ensure_pattern().resource_type = Some(kind);
        self
    }

    /// Register a [`Rule::Block`] for `pattern`.
    ///
    /// Compiles `pattern` eagerly; an invalid pattern fails the builder chain
    /// with [`InterceptionError::InvalidPattern`] returned as `Err(Self)` via
    /// the `Result` wrapper.
    #[allow(clippy::result_large_err)] // Bubbles up InterceptionError shape from url_pattern.rs.
    pub fn block(mut self, pattern: impl Into<String>) -> Result<Self, InterceptionError> {
        self.rules.push(Rule::Block {
            pattern: UrlPattern::new(pattern)?,
        });
        Ok(self)
    }

    /// Register a [`Rule::Redirect`] that rewrites `from` → `to`.
    #[allow(clippy::result_large_err)]
    pub fn redirect(
        mut self,
        from: impl Into<String>,
        to: impl Into<String>,
    ) -> Result<Self, InterceptionError> {
        self.rules.push(Rule::Redirect {
            from: UrlPattern::new(from)?,
            to: to.into(),
        });
        Ok(self)
    }

    /// Register a [`Rule::Respond`] serving a synthesized response.
    #[allow(clippy::result_large_err)]
    pub fn respond(
        mut self,
        pattern: impl Into<String>,
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> Result<Self, InterceptionError> {
        self.rules.push(Rule::Respond {
            pattern: UrlPattern::new(pattern)?,
            status,
            headers,
            body,
        });
        Ok(self)
    }

    /// Register a [`Rule::Modify`] driven by a user closure.
    ///
    /// The closure runs on the actor task per matching request — it must be
    /// `Send + Sync` and `'static`. Wrap shared state in `Arc` if needed.
    #[allow(clippy::result_large_err)]
    pub fn modify_request<F>(
        mut self,
        pattern: impl Into<String>,
        modify: F,
    ) -> Result<Self, InterceptionError>
    where
        F: Fn(&RequestInfo) -> RequestOverrides + Send + Sync + 'static,
    {
        self.rules.push(Rule::Modify {
            pattern: UrlPattern::new(pattern)?,
            modify: Arc::new(modify),
        });
        Ok(self)
    }

    /// Lazily push an empty pattern if none exists, so the stage/resource
    /// setters always have a target. Mirrors CDP's "missing fields default to
    /// match-all" semantics.
    fn ensure_pattern(&mut self) -> &mut RequestPattern {
        if self.patterns.is_empty() {
            self.patterns.push(RequestPattern::default());
        }
        self.patterns
            .last_mut()
            .expect("ensure_pattern pushed if empty")
    }

    /// Test-only accessor: number of registered rules. Used by the Task 5
    /// builder test (and future actor tests) without exposing the rule list
    /// as public API.
    #[cfg(test)]
    pub(crate) fn rules_count(&self) -> usize {
        self.rules.len()
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use zendriver_transport::testing::MockConnection;

    /// Register three rules (block + redirect + respond) on a fresh builder
    /// and assert the rule list grew to length 3. Verifies the chain wiring
    /// without touching the actor (Task 6) or CDP dispatch (Task 7).
    #[tokio::test]
    async fn three_rules_register_and_count() {
        let (_mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let builder = InterceptBuilder::new(&sess)
            .block("*/ads/*")
            .unwrap()
            .redirect("*/old/*", "https://example.com/new/")
            .unwrap()
            .respond(
                "*/api/health",
                200,
                vec![("content-type".into(), "application/json".into())],
                br#"{"ok":true}"#.to_vec(),
            )
            .unwrap();

        assert_eq!(builder.rules_count(), 3);
        conn.shutdown();
    }
}
