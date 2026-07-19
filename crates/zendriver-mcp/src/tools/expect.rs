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
//! ## Wire-shape mappers + drive
//!
//! Each kind has a small async free fn that flattens its lib-side struct
//! (`MatchedRequest` / `MatchedResponse` / `MatchedDialog` / `MatchedDownload`
//! / `MatchedFileChooser`) into a `serde_json::Value` and, when asked,
//! *drives* the matched object before it is dropped. The drive parameters
//! live on `browser_expect_register` (not `_await`) because the spawned
//! task owns the matched handle and several drive methods consume it
//! (`accept` / `dismiss` / `save_to`); the decision must therefore be made
//! at register time, before the triggering action runs:
//!
//! - **Dialog** — `dialog_action: accept|dismiss` (+ `dialog_prompt_text`)
//!   drives the dialog so the page's blocking `alert/confirm/prompt` returns.
//! - **Response** — `fetch_body: true` fetches the body and inlines it as
//!   `body_base64` + `body_len`.
//! - **Download** — `save_to: <path>` waits for completion and copies the file
//!   to the MCP host, reporting `saved_path`.
//! - **FileChooser** — `file_chooser_paths: [<path>, ...]` (required, must be
//!   non-empty) is applied automatically the instant the chooser opens, via
//!   `DOM.setFileInputFiles`; there is nothing left to drive by the time the
//!   matched event is reported.

#![cfg(feature = "expect")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Mutex, oneshot};
use zendriver::{
    DialogType, FileChooserMode, MatchedDialog, MatchedDownload, MatchedFileChooser,
    MatchedRequest, MatchedResponse, UrlMatcher, ZendriverError,
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
    /// `Page.fileChooserOpened` — a button/label-triggered (or direct)
    /// file picker, answered with `file_chooser_paths`.
    FileChooser,
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
            Self::FileChooser => "file_chooser",
        }
    }
}

/// How to drive a matched JavaScript dialog (`kind: dialog` only).
///
/// Applied by the spawned task the instant the dialog opens — so the page's
/// blocking `alert()` / `confirm()` / `prompt()` call returns promptly and
/// the matched event carries the chosen action. Omitting `dialog_action`
/// leaves the dialog untouched: the matched handle is dropped without
/// dispatching `Page.handleJavaScriptDialog` (`MatchedDialog` has no `Drop`
/// impl, so nothing is sent to Chrome), so the dialog stays open and the
/// page's blocking JS call does not return until something else resolves
/// it. Use this to merely observe that a dialog opened without acting on
/// it.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DialogAction {
    /// Accept the dialog. For a `prompt`, submit `dialog_prompt_text` (or the
    /// dialog's default value when that is unset).
    Accept,
    /// Dismiss / cancel the dialog.
    Dismiss,
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
    /// Dialog only: drive the matched dialog (`accept` / `dismiss`) instead
    /// of leaving it open for observation only. Decided here, at register
    /// time, because the matched dialog handle is consumed by the spawned
    /// task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dialog_action: Option<DialogAction>,
    /// Dialog only: prompt answer for `dialog_action: accept` on a `prompt`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dialog_prompt_text: Option<String>,
    /// Response only: fetch the response body and include it (base64) in the
    /// matched event as `body_base64` + `body_len`. One extra CDP round-trip.
    #[serde(default)]
    pub fetch_body: bool,
    /// Download only: copy the completed download to this path on the MCP
    /// host; the matched event reports it as `saved_path`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub save_to: Option<String>,
    /// File-chooser only: absolute paths (on the MCP host) to feed into the
    /// chooser the instant it opens, via `DOM.setFileInputFiles`. Required
    /// for `kind: file_chooser` — an empty list clears the input instead of
    /// attaching anything, which is rarely what's wanted, so an empty/absent
    /// list surfaces an `invalid_request` rather than silently no-op'ing.
    #[serde(default)]
    pub file_chooser_paths: Vec<String>,
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
    // Drive params, captured before the per-kind spawn moves them in. Each is
    // meaningful for exactly one kind (see field docs); the others are unused.
    let dialog_action = input.dialog_action;
    let dialog_prompt = input.dialog_prompt_text;
    let fetch_body = input.fetch_body;
    let save_to = input.save_to;
    let file_chooser_paths = input.file_chooser_paths;
    if matches!(kind, ExpectKind::FileChooser) && file_chooser_paths.is_empty() {
        return Err(ErrorData::invalid_request(
            "kind: file_chooser requires a non-empty file_chooser_paths",
            None,
        ));
    }

    let tab = {
        let s = state.lock().await;
        current_tab(&s).await?
    };
    // Captured before the per-kind spawn below (which may move/consume
    // `tab`) so the registered handle can be reaped by `browser_tab_close`
    // when this tab closes — see `state::ExpectationHandle::tab_id`.
    let tab_id = tab.target_id().to_string();

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
                let msg = match exp.matched().await {
                    Ok(m) => response_to_json(m, fetch_body).await,
                    Err(e) => Err(err_to_str(e)),
                };
                let _ = tx.send(msg);
            })
        }
        ExpectKind::Dialog => {
            let exp = tab.expect_dialog().timeout(pre_await);
            tokio::spawn(async move {
                let msg = match exp.matched().await {
                    Ok(m) => dialog_to_json(m, dialog_action, dialog_prompt).await,
                    Err(e) => Err(err_to_str(e)),
                };
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
                let msg = match exp.matched().await {
                    Ok(m) => download_to_json(m, save_to).await,
                    Err(e) => Err(err_to_str(e)),
                };
                let _ = tx.send(msg);
            })
        }
        ExpectKind::FileChooser => {
            // expect_file_chooser is async (it must await
            // Page.setInterceptFileChooserDialog{enabled:true} reaching
            // Chrome before returning). Surface its setup error inline —
            // same reasoning as the Download arm above.
            let exp = tab
                .expect_file_chooser(&file_chooser_paths)
                .await
                .map_err(|e| map_error(McpServerError::from(e)))?
                .timeout(pre_await);
            tokio::spawn(async move {
                let msg = exp
                    .matched()
                    .await
                    .map(file_chooser_to_json)
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
                tab_id,
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

/// Flatten a [`MatchedResponse`] to a wire JSON object, optionally fetching
/// the response body (base64) when `fetch_body` is set.
///
/// `body()` is an async CDP round-trip, so it is opt-in: agents that only
/// need status/headers skip it.
async fn response_to_json(m: MatchedResponse, fetch_body: bool) -> Result<Value, String> {
    let mut v = json!({
        "kind": "response",
        "url": m.url,
        "status": m.status,
        "status_text": m.status_text,
        "headers": m.headers,
        "request_id": m.request_id,
    });
    if fetch_body {
        let bytes = m.body().await.map_err(err_to_str)?;
        v["body_len"] = json!(bytes.len());
        v["body_base64"] = json!(BASE64.encode(&bytes));
    }
    Ok(v)
}

/// Render a [`DialogType`] as a stable lowercase wire string.
fn dialog_type_str(d: &DialogType) -> &'static str {
    match d {
        DialogType::Alert => "alert",
        DialogType::Beforeunload => "beforeunload",
        DialogType::Confirm => "confirm",
        DialogType::Prompt => "prompt",
    }
}

/// Flatten a [`MatchedDialog`] to a wire JSON object, driving the dialog
/// (`accept` / `dismiss`) when `action` is set.
///
/// Fields are captured by reference first so the handle stays intact for the
/// consuming `accept`/`dismiss` call. With no `action`, the handle is merely
/// dropped here — `MatchedDialog` has no `Drop` impl, so no CDP call is
/// made and the dialog is left open (observe-only).
async fn dialog_to_json(
    m: MatchedDialog,
    action: Option<DialogAction>,
    prompt: Option<String>,
) -> Result<Value, String> {
    let dialog_type = dialog_type_str(&m.dialog_type);
    let message = m.message.clone();
    let default_prompt = m.default_prompt.clone();
    let url = m.url.clone();
    let driven = match action {
        Some(DialogAction::Accept) => {
            m.accept(prompt).await.map_err(err_to_str)?;
            "accept"
        }
        Some(DialogAction::Dismiss) => {
            m.dismiss().await.map_err(err_to_str)?;
            "dismiss"
        }
        None => {
            drop(m);
            "default"
        }
    };
    Ok(json!({
        "kind": "dialog",
        "dialog_type": dialog_type,
        "message": message,
        "default_prompt": default_prompt,
        "url": url,
        "driven": driven,
    }))
}

/// Flatten a [`MatchedDownload`] to a wire JSON object, optionally copying the
/// completed download to `save_to` on the MCP host.
///
/// `MatchedDownload::save_to` waits for completion before copying, so the
/// reported `saved_path` is a fully-written file.
async fn download_to_json(m: MatchedDownload, save_to: Option<String>) -> Result<Value, String> {
    let url = m.url.clone();
    let suggested_filename = m.suggested_filename.clone();
    let guid = m.guid.clone();
    let download_dir = m.download_dir.to_string_lossy().into_owned();
    let saved_path = if let Some(dest) = save_to {
        m.save_to(PathBuf::from(&dest)).await.map_err(err_to_str)?;
        Some(dest)
    } else {
        None
    };
    Ok(json!({
        "kind": "download",
        "url": url,
        "suggested_filename": suggested_filename,
        "guid": guid,
        "download_dir": download_dir,
        "saved_path": saved_path,
    }))
}

/// Render a [`FileChooserMode`] as a stable lowercase wire string.
fn file_chooser_mode_str(m: FileChooserMode) -> &'static str {
    match m {
        FileChooserMode::SelectSingle => "select_single",
        FileChooserMode::SelectMultiple => "select_multiple",
    }
}

/// Flatten a [`MatchedFileChooser`] to a wire JSON object. By the time this
/// is called the lib has already dispatched `DOM.setFileInputFiles` with
/// `file_chooser_paths` and disabled the intercept — nothing left to drive.
fn file_chooser_to_json(m: MatchedFileChooser) -> Value {
    json!({
        "kind": "file_chooser",
        "mode": file_chooser_mode_str(m.mode),
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
                dialog_action: None,
                dialog_prompt_text: None,
                fetch_body: false,
                save_to: None,
                file_chooser_paths: Vec::new(),
            },
        )
        .await
        .expect_err("expected BrowserNotOpen");
        assert!(err.message.contains("Browser not open"));
    }

    /// `kind: file_chooser` with no `file_chooser_paths` must reject before
    /// even checking for a browser — an empty list would silently clear the
    /// input rather than attach anything, which is never what's wanted.
    #[tokio::test]
    async fn register_file_chooser_without_paths_errors() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let err = register(
            state,
            RegisterInput {
                kind: ExpectKind::FileChooser,
                matcher: None,
                pre_await_timeout_ms: 1_000,
                dialog_action: None,
                dialog_prompt_text: None,
                fetch_body: false,
                save_to: None,
                file_chooser_paths: Vec::new(),
            },
        )
        .await
        .expect_err("expected invalid_request");
        assert!(err.message.contains("file_chooser_paths"));
    }
}
