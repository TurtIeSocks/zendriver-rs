//! Interception tools — `browser_intercept_add_rule / _remove_rule /
//! _list_rules / _clear_rules`. Gated behind the `interception` feature.
//!
//! ## One MCP rule = one `InterceptHandle`
//!
//! The underlying `zendriver-interception` builder supports chaining many
//! actions onto a single handle, but that shape doesn't map cleanly to a
//! per-call MCP tool surface (each `add_rule` call must produce its own
//! removable id). v0 simplification: every `browser_intercept_add_rule`
//! invocation spawns a fresh `tab.intercept().pattern(pat).<action>.start()`
//! chain and keeps the returned [`InterceptHandle`] in
//! [`SessionState::rules`]. Removing the entry drops the handle, which the
//! interception actor treats as a cancellation signal — so
//! `browser_intercept_remove_rule` is the user-visible inverse of
//! `_add_rule`.
//!
//! ## Action surface (`InterceptAction`)
//!
//! Four tagged variants mirroring the four `InterceptBuilder` rule kinds:
//! `block`, `redirect`, `respond`, `modify_request`. `respond` carries a
//! UTF-8 `body` plus an optional `content_type` (collapsed into the headers
//! map when set) — binary bodies are out of scope for v0; agents that need
//! them can fall back to `browser_intercept_add_rule` with a `redirect` to
//! a server they control.
//!
//! `modify_request` accepts a headers map of replacements; CDP's
//! `continueRequest.headers` is a *full replacement*, so the closure
//! returns a [`RequestOverrides`] that preserves the original headers and
//! overlays the caller-supplied entries. Other override fields (URL,
//! method, post-data) are reserved for a follow-up.
//!
//! [`InterceptHandle`]: zendriver::InterceptHandle
//! [`RequestOverrides`]: zendriver::RequestOverrides

#![cfg(feature = "interception")]

use std::collections::BTreeMap;
use std::sync::Arc;

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use zendriver::{RequestOverrides, ResponseInfo, ResponseOverrides, ZendriverError};

use crate::errors::{McpServerError, map_error};
use crate::state::{InterceptRuleHandle, RuleId, SessionState};
use crate::tools::common::current_tab;

// ---------- shared types --------------------------------------------------

/// Wire-level interception action. One variant per supported rule kind.
///
/// Tagged with `kind` so a JSON payload looks like
/// `{ "kind": "block" }` or `{ "kind": "redirect", "to": "..." }`, which
/// is easier for an agent to construct than a flat field set with mutually
/// exclusive shapes.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
pub enum InterceptAction {
    /// Fail every matching request with `BlockedByClient`.
    Block,
    /// Redirect every match to `to` (verbatim URL replacement).
    Redirect {
        /// Target URL. Sent to the actor as a string — no expansion of the
        /// matched URL is performed; if you need per-request substitution
        /// use `modify_request` or the manual stream API.
        to: String,
    },
    /// Synthesize a response with the given status / body / headers.
    Respond {
        /// HTTP status code (e.g. `200`, `404`).
        status: u16,
        /// Response body as UTF-8 text.
        body: String,
        /// Convenience for the most common header: when set, a
        /// `Content-Type: <content_type>` header is added (and overrides
        /// any same-named entry in `headers`).
        #[serde(default)]
        content_type: Option<String>,
        /// Additional response headers. Lexicographically sorted (`BTreeMap`)
        /// for stable wire output.
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
    /// Continue the request with extra / replaced headers. Other fields
    /// (URL, method, post-data) are left at Chrome's originals.
    ModifyRequest {
        /// Headers to merge over the request's originals. CDP's
        /// `continueRequest.headers` is a full replacement, so the closure
        /// rebuilds the header list from the intercepted request and
        /// overlays these entries (case-insensitive on names).
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
    /// Continue the *response* with an overridden status and/or headers
    /// (pauses at the response stage). Other fields are left at Chrome's
    /// originals. The body is not rewritten — use `respond` to synthesize a
    /// whole response instead.
    ModifyResponse {
        /// Override the HTTP status code. Omit to keep Chrome's original.
        #[serde(default)]
        status: Option<u16>,
        /// Headers to merge over the response's originals (full-replacement
        /// semantics, same as `modify_request`; case-insensitive names).
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
}

// ---------- browser_intercept_add_rule -----------------------------------

/// Input for `browser_intercept_add_rule`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddRuleInput {
    /// URL pattern (CDP wildcard syntax — `*` and `?` glob metacharacters).
    /// Matched against the full request URL, not just the path.
    pub pattern: String,
    /// Action to take on matches.
    pub action: InterceptAction,
}

/// Output of `browser_intercept_add_rule`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct AddRuleOutput {
    /// Opaque id for [`SessionState::rules`]. Pass to
    /// `browser_intercept_remove_rule` to take the rule down.
    pub rule_id: RuleId,
}

/// Register one interception rule. Spawns a fresh
/// `tab.intercept().pattern(...).<action>.start()` chain on the current
/// tab and stashes the resulting handle.
pub async fn add_rule(
    state: Arc<Mutex<SessionState>>,
    input: AddRuleInput,
) -> Result<AddRuleOutput, ErrorData> {
    let mut s = state.lock().await;
    let tab = current_tab(&s).await?;
    // Captured before the per-action match below so the registered handle
    // can be reaped by `browser_tab_close` when this tab closes — see
    // `state::InterceptRuleHandle::tab_id`.
    let tab_id = tab.target_id().to_string();

    // Each match-arm builds + starts its own InterceptBuilder. The action
    // method (`block` / `redirect` / `respond` / `modify_request`) takes the
    // URL pattern as its first arg and registers it as the rule's matcher;
    // `InterceptBuilder::start` auto-injects a match-all `"*"` CDP
    // `Fetch.RequestPattern` when none is added via `.pattern()`, so we
    // don't need to add one ourselves. (Conflating the two would just
    // register the same string twice as conceptually different things —
    // the CDP server-side filter and the rule's per-request matcher.)
    let (handle, action_kind): (zendriver::InterceptHandle, &'static str) = match input.action {
        InterceptAction::Block => {
            let h = tab
                .intercept()
                .block(input.pattern.clone())
                .map_err(zendriver_err)?
                .start();
            (h, "block")
        }
        InterceptAction::Redirect { to } => {
            let h = tab
                .intercept()
                .redirect(input.pattern.clone(), to)
                .map_err(zendriver_err)?
                .start();
            (h, "redirect")
        }
        InterceptAction::Respond {
            status,
            body,
            content_type,
            mut headers,
        } => {
            if let Some(ct) = content_type {
                // `Content-Type` is the common case; honor the convenience
                // field by overlaying it into the explicit map.
                headers.insert("content-type".into(), ct);
            }
            let header_vec: Vec<(String, String)> = headers.into_iter().collect();
            let h = tab
                .intercept()
                .respond(input.pattern.clone(), status, header_vec, body.into_bytes())
                .map_err(zendriver_err)?
                .start();
            (h, "respond")
        }
        InterceptAction::ModifyRequest { headers } => {
            // Snapshot the overlay into an `Arc` so the per-request closure
            // can be `Fn + Send + Sync + 'static` (the builder requires it
            // — see InterceptBuilder::modify_request).
            let overlay = Arc::new(headers);
            let h = tab
                .intercept()
                .modify_request(input.pattern.clone(), move |req| {
                    merge_headers(&req.headers, &overlay)
                })
                .map_err(zendriver_err)?
                .start();
            (h, "modify_request")
        }
        InterceptAction::ModifyResponse { status, headers } => {
            // Same `Arc`-capture pattern as `modify_request`; the closure runs
            // on the actor task per matching response.
            let overlay = Arc::new(headers);
            let h = tab
                .intercept()
                .modify_response(input.pattern.clone(), move |resp: &ResponseInfo| {
                    let headers = if overlay.is_empty() {
                        None
                    } else {
                        Some(merge_header_list(&resp.headers, &overlay))
                    };
                    ResponseOverrides {
                        status,
                        headers,
                        ..ResponseOverrides::default()
                    }
                })
                .map_err(zendriver_err)?
                .start();
            (h, "modify_response")
        }
    };

    let id: RuleId = uuid::Uuid::new_v4().to_string();
    s.rules.insert(
        id.clone(),
        InterceptRuleHandle {
            pattern: input.pattern,
            action_kind,
            _handle: handle,
            tab_id,
        },
    );
    Ok(AddRuleOutput { rule_id: id })
}

/// Build the [`RequestOverrides`] payload for a `modify_request` rule.
///
/// CDP's `Fetch.continueRequest.headers` is a *full replacement* — there is
/// no merge mode — so we rebuild the header list from the request as Chrome
/// reported it and overlay the caller's entries by case-insensitive name
/// match. Case-insensitive: HTTP header names are case-insensitive per RFC
/// 7230, and Chrome's emission casing isn't stable across versions.
fn merge_headers(
    original: &[(String, String)],
    overlay: &BTreeMap<String, String>,
) -> RequestOverrides {
    RequestOverrides {
        headers: Some(merge_header_list(original, overlay)),
        ..RequestOverrides::default()
    }
}

/// Rebuild a full header list from `original`, dropping entries the `overlay`
/// replaces (case-insensitive) and appending the overlay entries. Shared by
/// `modify_request` (→ [`RequestOverrides`]) and `modify_response`
/// (→ [`ResponseOverrides`]) because CDP's continue-* header fields are both
/// full replacements with no merge mode.
fn merge_header_list(
    original: &[(String, String)],
    overlay: &BTreeMap<String, String>,
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::with_capacity(original.len() + overlay.len());
    let overlay_keys_lower: std::collections::HashSet<String> =
        overlay.keys().map(|k| k.to_ascii_lowercase()).collect();
    for (k, v) in original {
        if !overlay_keys_lower.contains(&k.to_ascii_lowercase()) {
            out.push((k.clone(), v.clone()));
        }
    }
    for (k, v) in overlay {
        out.push((k.clone(), v.clone()));
    }
    out
}

/// Wrap a `zendriver_interception::InterceptionError` (re-exported as
/// `zendriver::InterceptionError`) into the MCP error wire format.
fn zendriver_err(e: zendriver::InterceptionError) -> ErrorData {
    map_error(McpServerError::from(ZendriverError::from(e)))
}

// ---------- browser_intercept_remove_rule --------------------------------

/// Input for `browser_intercept_remove_rule`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveRuleInput {
    /// Id returned by an earlier `browser_intercept_add_rule` call.
    pub rule_id: RuleId,
}

/// Output of `browser_intercept_remove_rule`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RemoveRuleOutput {
    /// Always `true` on success. Removal of an unknown id returns
    /// [`McpServerError::RuleNotFound`] rather than a `false` here, so an
    /// agent can branch on success without re-checking the body.
    pub removed: bool,
}

/// Drop a previously-registered rule, tearing down its interception actor.
pub async fn remove_rule(
    state: Arc<Mutex<SessionState>>,
    input: RemoveRuleInput,
) -> Result<RemoveRuleOutput, ErrorData> {
    let mut s = state.lock().await;
    // `.remove(...)` returns the value (and thus its `_handle: InterceptHandle`),
    // dropping it here cancels the actor. We don't need to await `stop()` —
    // `Drop` fires-and-forgets the `Fetch.disable`.
    s.rules
        .remove(&input.rule_id)
        .ok_or_else(|| map_error(McpServerError::RuleNotFound(input.rule_id)))?;
    Ok(RemoveRuleOutput { removed: true })
}

// ---------- browser_intercept_list_rules ---------------------------------

/// Output of `browser_intercept_list_rules`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ListRulesOutput {
    /// One entry per live rule. Sorted by `rule_id` so the output is stable
    /// across runs (rather than HashMap-iteration-order chaos).
    pub rules: Vec<RuleSummary>,
}

/// Description of a single live interception rule.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RuleSummary {
    /// Same id `browser_intercept_add_rule` returned.
    pub rule_id: RuleId,
    /// URL pattern the rule was registered with.
    pub pattern: String,
    /// `"block"`, `"redirect"`, `"respond"`, or `"modify_request"`.
    pub action_kind: String,
}

/// Enumerate every live rule. Empty list when none are registered (never
/// an error — a session with no rules is a valid steady state).
pub async fn list_rules(
    state: Arc<Mutex<SessionState>>,
    _: crate::tools::common::EmptyInput,
) -> Result<ListRulesOutput, ErrorData> {
    let s = state.lock().await;
    let mut rules: Vec<RuleSummary> = s
        .rules
        .iter()
        .map(|(id, h)| RuleSummary {
            rule_id: id.clone(),
            pattern: h.pattern.clone(),
            action_kind: h.action_kind.to_string(),
        })
        .collect();
    rules.sort_by(|a, b| a.rule_id.cmp(&b.rule_id));
    Ok(ListRulesOutput { rules })
}

// ---------- browser_intercept_clear_rules --------------------------------

/// Output of `browser_intercept_clear_rules`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ClearRulesOutput {
    /// Number of rules that were dropped. `0` is a successful no-op
    /// rather than an error — calling clear on an empty registry is the
    /// agent's idempotent "make sure nothing is intercepting" lever.
    pub cleared: usize,
}

/// Drop every live rule. Each handle's `Drop` cancels its actor; we don't
/// wait for any of them — same semantics as `remove_rule`.
pub async fn clear_rules(
    state: Arc<Mutex<SessionState>>,
    _: crate::tools::common::EmptyInput,
) -> Result<ClearRulesOutput, ErrorData> {
    let mut s = state.lock().await;
    let cleared = s.rules.len();
    s.rules.clear();
    Ok(ClearRulesOutput { cleared })
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    //! No-browser unit coverage.
    //!
    //! The browser-touching `add_rule` path needs a live Chrome (it goes
    //! through `current_tab` and spawns a real interception actor) — that
    //! path is exercised in `tests/integration_interception.rs`. Here we
    //! cover the bookkeeping: list / clear on an empty registry never
    //! errors, remove on an unknown id surfaces `RuleNotFound`, and
    //! `add_rule` without a browser surfaces `BrowserNotOpen`.

    use super::*;
    use crate::tools::common::EmptyInput;

    #[tokio::test]
    async fn list_rules_empty_returns_empty_vec() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let out = list_rules(state, EmptyInput {})
            .await
            .expect("list_rules ok");
        assert!(out.rules.is_empty());
    }

    #[tokio::test]
    async fn clear_rules_empty_returns_zero() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let out = clear_rules(state, EmptyInput {})
            .await
            .expect("clear_rules ok");
        assert_eq!(out.cleared, 0);
    }

    #[tokio::test]
    async fn remove_unknown_rule_surfaces_rule_not_found() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let err = remove_rule(
            state,
            RemoveRuleInput {
                rule_id: "nope".into(),
            },
        )
        .await
        .expect_err("expected RuleNotFound");
        // The hint should point at add_rule (per errors::map_error).
        let data = err.data.as_ref().expect("data populated");
        assert_eq!(data["suggested_next"], "browser_intercept_add_rule");
    }

    #[tokio::test]
    async fn add_rule_with_no_browser_errors() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let err = add_rule(
            state,
            AddRuleInput {
                pattern: "*".into(),
                action: InterceptAction::Block,
            },
        )
        .await
        .expect_err("expected BrowserNotOpen");
        assert!(err.message.contains("Browser not open"));
    }

    #[test]
    fn merge_headers_overlays_case_insensitively_and_drops_originals() {
        let original = vec![
            ("Host".to_string(), "example.com".to_string()),
            ("User-Agent".to_string(), "old".to_string()),
            ("Accept".to_string(), "*/*".to_string()),
        ];
        let mut overlay = BTreeMap::new();
        overlay.insert("user-agent".to_string(), "new".to_string());
        overlay.insert("X-Marker".to_string(), "yes".to_string());

        let ov = merge_headers(&original, &overlay);
        let headers = ov.headers.expect("headers populated");

        // Original "User-Agent" gone (case-insens), replaced by overlay's
        // "user-agent: new". Host + Accept survive. Overlay's X-Marker is
        // appended.
        let names_lower: Vec<String> = headers
            .iter()
            .map(|(k, _)| k.to_ascii_lowercase())
            .collect();
        assert!(names_lower.contains(&"host".into()));
        assert!(names_lower.contains(&"accept".into()));
        assert!(names_lower.contains(&"user-agent".into()));
        assert!(names_lower.contains(&"x-marker".into()));
        // Exactly one "user-agent" (no dupes).
        assert_eq!(names_lower.iter().filter(|n| *n == "user-agent").count(), 1);
        // Replaced value is the overlay's.
        let ua = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("user-agent"))
            .map(|(_, v)| v.as_str())
            .expect("user-agent present");
        assert_eq!(ua, "new");
    }
}
