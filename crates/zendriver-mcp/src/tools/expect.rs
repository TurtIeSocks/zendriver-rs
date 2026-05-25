//! Expectation tools — `browser_expect_register / _await / _cancel`. Gated
//! behind the `expect` feature.
//!
//! ## Register-then-await split
//!
//! The lib's API is one-call:
//! `tab.expect_request(matcher).timeout(d).matched().await`. MCP's
//! request/response cycle can't model an in-flight `.matched()` future across
//! tool calls, so v0 splits the surface into three:
//!
//! - `browser_expect_register` constructs the expectation, applies an inner
//!   `pre_await_timeout_ms` (default 60s — generous so the user has time to
//!   call the trigger action tool in between), spawns a tokio task that
//!   awaits `.matched()`, and pipes the result through a
//!   `tokio::sync::oneshot::Sender`. The id is returned synchronously.
//! - `browser_expect_await` takes ownership of the receiver from the
//!   session's registry and `tokio::time::timeout`s on it.
//! - `browser_expect_cancel` drops the registry entry + `.abort()`s the
//!   spawned task so the lib-level `.matched()` future is killed promptly
//!   rather than left to time out on its own.
//!
//! ## Wire-shape mappers (`event_to_json`)
//!
//! Each kind has a small free fn that flattens its lib-side struct
//! (`MatchedRequest` / `MatchedResponse` / `MatchedDialog` / `MatchedDownload`)
//! into a `serde_json::Value`. Bodies (`MatchedResponse::body()`,
//! `MatchedDownload::save_to(...)`) are NOT fetched in v0 — they require an
//! extra round-trip and aren't part of the event itself. Dialog
//! accept/dismiss is also not exposed; Chrome's default handling fires
//! when the `MatchedDialog` is dropped at task end.

#![cfg(feature = "expect")]

use std::sync::Arc;
use std::time::Duration;

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Mutex, oneshot};
use zendriver::{
    MatchedDialog, MatchedDownload, MatchedRequest, MatchedResponse, UrlMatcher, ZendriverError,
};

use crate::errors::{McpServerError, map_error};
use crate::state::{ExpectationHandle, ExpectationId, SessionState};
use crate::tools::common::current_tab;

// ---------- shared types --------------------------------------------------

/// Which `Page.*` / `Network.*` family the expectation listens on.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExpectKind {
    /// `Network.requestWillBeSent` — outgoing request, no body fetch.
    Request,
    /// `Network.responseReceived` — incoming response headers (no body in v0).
    Response,
    /// `Page.javascriptDialogOpened` — alert/confirm/prompt/beforeunload.
    Dialog,
    /// `Page.downloadWillBegin` — start of a file download.
    Download,
}

impl ExpectKind {
    /// Static label used by [`ExpectationHandle::kind`] and surfaced
    /// indirectly through the matched-event `kind` field for traceability.
    fn label(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Response => "response",
            Self::Dialog => "dialog",
            Self::Download => "download",
        }
    }
}

/// URL match predicate for request/response expectations. Dialog and
/// Download kinds ignore matcher fields entirely (the lib has no URL filter
/// for them in v0).
///
/// Exactly zero or one of `url_substr` / `url_regex` may be set. Setting
/// neither matches every URL (substring matcher with the empty string).
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExpectMatcher {
    /// Substring matcher — match if request URL contains this needle.
    #[serde(default)]
    pub url_substr: Option<String>,
    /// Regex matcher — match if `regex::Regex::is_match(url)` returns true.
    /// Compiled inside `_register`; an invalid regex surfaces as an
    /// invalid-request error.
    #[serde(default)]
    pub url_regex: Option<String>,
}

impl ExpectMatcher {
    /// Build a [`UrlMatcher`] from the wire-level matcher.
    ///
    /// Priority: `url_regex` wins over `url_substr` when both are set —
    /// flagging that on the schema would be nicer, but a serde validator
    /// would be intrusive; document the precedence here and let agents pick
    /// one.
    fn into_url_matcher(self) -> Result<UrlMatcher, ErrorData> {
        if let Some(re) = self.url_regex {
            let compiled = regex::Regex::new(&re).map_err(|e| {
                ErrorData::invalid_request(format!("invalid url_regex `{re}`: {e}"), None)
            })?;
            return Ok(UrlMatcher::Regex(compiled));
        }
        if let Some(sub) = self.url_substr {
            return Ok(UrlMatcher::Substring(sub));
        }
        // Empty-string substring matcher matches every URL — easiest "any"
        // sentinel that doesn't require a separate variant.
        Ok(UrlMatcher::Substring(String::new()))
    }
}

// ---------- browser_expect_register ---------------------------------------

/// Input for `browser_expect_register`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RegisterInput {
    /// Which event family to watch.
    pub kind: ExpectKind,
    /// URL matcher (request/response only — ignored for dialog/download).
    #[serde(default)]
    pub matcher: Option<ExpectMatcher>,
    /// Inner timeout applied to the lib's `.timeout(d)` call. Generous by
    /// default (60s) so the user has time to call the action tool that
    /// triggers the event between `register` and `await`.
    #[serde(default = "default_pre_timeout")]
    pub pre_await_timeout_ms: u64,
}

fn default_pre_timeout() -> u64 {
    60_000
}

/// Output of `browser_expect_register`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RegisterOutput {
    /// Opaque id for the session's expectation registry. Pass to
    /// `browser_expect_await` or `browser_expect_cancel`.
    pub expectation_id: ExpectationId,
}

/// Register a one-shot expectation against the current tab.
///
/// Spawns a tokio task that constructs the appropriate `tab.expect_*` call,
/// applies the `pre_await_timeout_ms`, awaits `.matched()`, and pipes the
/// result (JSON-encoded matched-event, or a textual error) through a
/// `oneshot::Sender`. The task's `JoinHandle` is stored alongside the
/// receiver so `_cancel` can `.abort()` it promptly.
pub async fn register(
    state: Arc<Mutex<SessionState>>,
    input: RegisterInput,
) -> Result<RegisterOutput, ErrorData> {
    let pre_await = Duration::from_millis(input.pre_await_timeout_ms);
    let matcher = input.matcher.unwrap_or_default().into_url_matcher()?;
    let kind = input.kind;

    let tab = {
        let s = state.lock().await;
        current_tab(&s).await?
    };

    let (tx, rx) = oneshot::channel::<Result<Value, String>>();

    // One spawn per kind: each branch constructs its expectation against the
    // tab handle (cloneable Arc-backed), applies the pre-await timeout, and
    // forwards the matched event (or error string) over the oneshot. We
    // can't share the spawned body across kinds because each kind's
    // expectation type is different and `.matched()` returns a different
    // matched-event struct.
    let task: tokio::task::JoinHandle<()> = match kind {
        ExpectKind::Request => {
            let exp = tab.expect_request(matcher).timeout(pre_await);
            tokio::spawn(async move {
                let msg = exp.matched().await.map(request_to_json).map_err(err_to_str);
                let _ = tx.send(msg);
            })
        }
        ExpectKind::Response => {
            let exp = tab.expect_response(matcher).timeout(pre_await);
            tokio::spawn(async move {
                let msg = exp
                    .matched()
                    .await
                    .map(response_to_json)
                    .map_err(err_to_str);
                let _ = tx.send(msg);
            })
        }
        ExpectKind::Dialog => {
            let exp = tab.expect_dialog().timeout(pre_await);
            tokio::spawn(async move {
                let msg = exp.matched().await.map(dialog_to_json).map_err(err_to_str);
                let _ = tx.send(msg);
            })
        }
        ExpectKind::Download => {
            // expect_download is async (lazy per-tab setup of the download
            // coordinator + Browser.setDownloadBehavior dispatch). Surface
            // its setup error inline instead of stashing the expectation —
            // a registration we can never await isn't worth recording.
            let exp = tab
                .expect_download()
                .await
                .map_err(|e| map_error(McpServerError::from(e)))?
                .timeout(pre_await);
            tokio::spawn(async move {
                let msg = exp
                    .matched()
                    .await
                    .map(download_to_json)
                    .map_err(err_to_str);
                let _ = tx.send(msg);
            })
        }
    };

    let id: ExpectationId = uuid::Uuid::new_v4().to_string();
    {
        let mut s = state.lock().await;
        s.expectations.insert(
            id.clone(),
            ExpectationHandle {
                kind: kind.label(),
                task,
                rx,
            },
        );
    }
    Ok(RegisterOutput { expectation_id: id })
}

/// Collapse a [`ZendriverError`] surfaced by the spawned task to a string
/// suitable for the `Err` branch of the registry's `oneshot` channel. We
/// flatten to a string because `serde_json::Value` cannot itself round-trip
/// a `ZendriverError` and the error eventually surfaces through a plain
/// invalid-request anyway.
fn err_to_str(e: ZendriverError) -> String {
    e.to_string()
}

// ---------- browser_expect_await ------------------------------------------

/// Input for `browser_expect_await`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AwaitInput {
    /// Id returned by an earlier `browser_expect_register` call.
    pub expectation_id: ExpectationId,
    /// Outer timeout for waiting on the spawned task to send the matched
    /// event. Defaults to 30s.
    #[serde(default = "default_await_timeout")]
    pub timeout_ms: u64,
}

fn default_await_timeout() -> u64 {
    30_000
}

/// Output of `browser_expect_await`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct AwaitOutput {
    /// Echoed id of the awaited expectation (matches input).
    pub expectation_id: ExpectationId,
    /// JSON-encoded matched event. Shape depends on the expectation's
    /// `kind` — see the per-kind `*_to_json` helpers in this module.
    pub event: Value,
}

/// Wait for a previously-registered expectation to resolve.
///
/// Removes the entry from `s.expectations` (taking ownership of the
/// `oneshot::Receiver` + `JoinHandle`), then applies the caller's outer
/// timeout to the channel. The spawned task's `pre_await_timeout_ms` is
/// independent — if it fires first, the channel resolves with `Ok(Err(_))`
/// carrying the lib's `ZendriverError::Timeout` as a string.
pub async fn await_expectation(
    state: Arc<Mutex<SessionState>>,
    input: AwaitInput,
) -> Result<AwaitOutput, ErrorData> {
    let handle = {
        let mut s = state.lock().await;
        s.expectations
            .remove(&input.expectation_id)
            .ok_or_else(|| {
                map_error(McpServerError::ExpectationNotFound(
                    input.expectation_id.clone(),
                ))
            })?
    };
    let ExpectationHandle { rx, task, .. } = handle;

    let outer = Duration::from_millis(input.timeout_ms);
    match tokio::time::timeout(outer, rx).await {
        Err(_elapsed) => {
            // Outer timeout fired — abort the spawned task so its
            // `.matched()` future stops holding a subscription stream open.
            task.abort();
            Err(ErrorData::invalid_request(
                format!(
                    "expectation `{}` await_timed_out after {:?}",
                    input.expectation_id, outer
                ),
                None,
            ))
        }
        Ok(Err(_recv)) => {
            // Sender dropped without sending — spawned task panicked or was
            // aborted externally. The task may still be in a transient
            // state; abort defensively to be sure.
            task.abort();
            Err(ErrorData::invalid_request(
                format!(
                    "expectation `{}` channel_closed (spawned task ended without sending)",
                    input.expectation_id
                ),
                None,
            ))
        }
        Ok(Ok(Err(err_str))) => {
            // Spawned task produced an error — lib-level failure or inner
            // pre_await_timeout fired. Surface it on the wire.
            Err(ErrorData::invalid_request(
                format!("expectation `{}` failed: {err_str}", input.expectation_id),
                None,
            ))
        }
        Ok(Ok(Ok(event))) => Ok(AwaitOutput {
            expectation_id: input.expectation_id,
            event,
        }),
    }
}

// ---------- browser_expect_cancel -----------------------------------------

/// Input for `browser_expect_cancel`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CancelInput {
    /// Id returned by an earlier `browser_expect_register` call.
    pub expectation_id: ExpectationId,
}

/// Output of `browser_expect_cancel`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CancelOutput {
    /// Always `true` on success. Cancellation of an unknown id returns
    /// [`McpServerError::ExpectationNotFound`] rather than `false` here.
    pub cancelled: bool,
}

/// Cancel a pending expectation, aborting its spawned task.
///
/// Drops the registry entry (which drops the `oneshot::Receiver`) and
/// `.abort()`s the spawned task. Any in-flight `.matched()` future inside
/// the task is dropped via abort, which tears down its lib-side subscriber.
pub async fn cancel(
    state: Arc<Mutex<SessionState>>,
    input: CancelInput,
) -> Result<CancelOutput, ErrorData> {
    let mut s = state.lock().await;
    let handle = s
        .expectations
        .remove(&input.expectation_id)
        .ok_or_else(|| {
            map_error(McpServerError::ExpectationNotFound(
                input.expectation_id.clone(),
            ))
        })?;
    handle.task.abort();
    Ok(CancelOutput { cancelled: true })
}

// ---------- wire-shape mappers -------------------------------------------

/// Flatten a [`MatchedRequest`] to a wire JSON object.
fn request_to_json(m: MatchedRequest) -> Value {
    json!({
        "kind": "request",
        "url": m.url,
        "method": m.method,
        "headers": m.headers,
        "request_id": m.request_id,
        // post_data: emit only the byte length to avoid blowing up the
        // wire for large POST bodies. Callers needing the bytes can use a
        // future tool that fetches them explicitly.
        "post_data_len": m.post_data.as_ref().map(Vec::len),
    })
}

/// Flatten a [`MatchedResponse`] to a wire JSON object.
///
/// Body is NOT fetched in v0 — `body()` is an async CDP round-trip and
/// not part of the event itself.
fn response_to_json(m: MatchedResponse) -> Value {
    json!({
        "kind": "response",
        "url": m.url,
        "status": m.status,
        "status_text": m.status_text,
        "headers": m.headers,
        "request_id": m.request_id,
    })
}

/// Flatten a [`MatchedDialog`] to a wire JSON object.
///
/// `dialog_type` is rendered as a lowercase string for stable wire output.
/// `accept`/`dismiss` are NOT exposed in v0 — Chrome's default handling
/// applies when the matched-dialog handle is dropped at task end.
fn dialog_to_json(m: MatchedDialog) -> Value {
    let kind = match m.dialog_type {
        zendriver::DialogType::Alert => "alert",
        zendriver::DialogType::Beforeunload => "beforeunload",
        zendriver::DialogType::Confirm => "confirm",
        zendriver::DialogType::Prompt => "prompt",
    };
    json!({
        "kind": "dialog",
        "dialog_type": kind,
        "message": m.message,
        "default_prompt": m.default_prompt,
        "url": m.url,
    })
}

/// Flatten a [`MatchedDownload`] to a wire JSON object.
///
/// `path()` is not awaited here — the download is only `InProgress` at
/// `Page.downloadWillBegin` time. Agents needing the on-disk path can call
/// a future tool that polls completion + reads the file.
fn download_to_json(m: MatchedDownload) -> Value {
    json!({
        "kind": "download",
        "url": m.url,
        "suggested_filename": m.suggested_filename,
        "guid": m.guid,
        "download_dir": m.download_dir.to_string_lossy(),
    })
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    //! No-browser unit coverage.
    //!
    //! Browser-touching `register` paths need a live Chrome (they construct
    //! a real `tab.expect_*` and spawn a real subscriber actor) — that path
    //! is exercised in `tests/integration_expect.rs`. Here we cover the
    //! bookkeeping: register / await / cancel of unknown ids surface
    //! `ExpectationNotFound`, register without a browser surfaces
    //! `BrowserNotOpen`, and the matcher conversion roundtrips.
    use super::*;

    #[test]
    fn matcher_default_is_match_any_substring() {
        let m = ExpectMatcher::default().into_url_matcher().unwrap();
        match m {
            UrlMatcher::Substring(s) => assert!(
                s.is_empty(),
                "default matcher must be empty substring (matches any url)",
            ),
            UrlMatcher::Regex(_) => panic!("expected Substring(\"\")"),
        }
    }

    #[test]
    fn matcher_url_regex_wins_over_url_substr() {
        // Documenting precedence: regex is checked first in
        // `into_url_matcher`. Confirms agents picking both get the regex.
        let m = ExpectMatcher {
            url_substr: Some("/foo/".into()),
            url_regex: Some(r"^https://api\.".into()),
        }
        .into_url_matcher()
        .unwrap();
        match m {
            UrlMatcher::Regex(re) => assert!(re.is_match("https://api.example.com/v1")),
            UrlMatcher::Substring(_) => panic!("expected Regex variant"),
        }
    }

    #[test]
    fn matcher_invalid_regex_errors() {
        let err = ExpectMatcher {
            url_regex: Some("[".into()),
            url_substr: None,
        }
        .into_url_matcher()
        .expect_err("expected invalid regex");
        assert!(err.message.contains("invalid url_regex"));
    }

    #[tokio::test]
    async fn await_unknown_expectation_id_surfaces_not_found() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let err = await_expectation(
            state,
            AwaitInput {
                expectation_id: "nope".into(),
                timeout_ms: 100,
            },
        )
        .await
        .expect_err("expected ExpectationNotFound");
        let data = err.data.as_ref().expect("data populated");
        assert_eq!(data["suggested_next"], "browser_expect_register");
    }

    #[tokio::test]
    async fn cancel_unknown_expectation_id_surfaces_not_found() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let err = cancel(
            state,
            CancelInput {
                expectation_id: "nope".into(),
            },
        )
        .await
        .expect_err("expected ExpectationNotFound");
        let data = err.data.as_ref().expect("data populated");
        assert_eq!(data["suggested_next"], "browser_expect_register");
    }

    #[tokio::test]
    async fn register_with_no_browser_errors() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let err = register(
            state,
            RegisterInput {
                kind: ExpectKind::Request,
                matcher: None,
                pre_await_timeout_ms: 1_000,
            },
        )
        .await
        .expect_err("expected BrowserNotOpen");
        assert!(err.message.contains("Browser not open"));
    }
}
