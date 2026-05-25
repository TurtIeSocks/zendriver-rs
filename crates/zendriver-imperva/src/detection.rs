//! Imperva surface detection.
//!
//! One round-trip per probe: bundles cookie reads, body marker scan, and
//! CAPTCHA iframe pattern checks into a single `Runtime.evaluate` carrying
//! [`detect.js`](./detect.js).

use serde::Deserialize;
use serde_json::{Value, json};
use zendriver_transport::SessionHandle;

use crate::error::ImpervaError;

/// Which Imperva surface a tab is currently showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImpervaSurface {
    /// Modern reese84-based bot management.
    Reese84,
    /// Legacy Incapsula (`___utmvc` / `incap_ses_*` / `visid_incap_*`).
    Legacy,
    /// Visual or invisible CAPTCHA challenge.
    Captcha(CaptchaKind),
    /// No Imperva surface detected.
    None,
}

/// Kind of CAPTCHA escalation Imperva is presenting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptchaKind {
    HCaptcha,
    Recaptcha,
    ImpervaNative,
    Unknown,
}

/// Snapshot of one `detect.js` round-trip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectionSnapshot {
    pub surface: ImpervaSurface,
    pub reese84: Option<String>,
    pub body_clean: bool,
    pub sessions: Vec<CookieSnapshot>,
}

/// Cookie name + value as observed at probe time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CookieSnapshot {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Deserialize)]
struct RawSurface {
    kind: String,
    #[serde(default)]
    captcha: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawCookie {
    name: String,
    value: String,
}

#[derive(Debug, Deserialize)]
struct RawSnapshot {
    surface: RawSurface,
    #[serde(default)]
    reese84: Option<String>,
    body_clean: bool,
    #[serde(default)]
    sessions: Vec<RawCookie>,
}

impl From<RawSurface> for ImpervaSurface {
    fn from(r: RawSurface) -> Self {
        match r.kind.as_str() {
            "Reese84" => Self::Reese84,
            "Legacy" => Self::Legacy,
            "Captcha" => {
                let k = match r.captcha.as_deref() {
                    Some("HCaptcha") => CaptchaKind::HCaptcha,
                    Some("Recaptcha") => CaptchaKind::Recaptcha,
                    Some("ImpervaNative") => CaptchaKind::ImpervaNative,
                    _ => CaptchaKind::Unknown,
                };
                Self::Captcha(k)
            }
            _ => Self::None,
        }
    }
}

impl From<RawSnapshot> for DetectionSnapshot {
    fn from(r: RawSnapshot) -> Self {
        Self {
            surface: r.surface.into(),
            reese84: r.reese84,
            body_clean: r.body_clean,
            sessions: r
                .sessions
                .into_iter()
                .map(|c| CookieSnapshot {
                    name: c.name,
                    value: c.value,
                })
                .collect(),
        }
    }
}

/// Run a single `detect.js` probe against `session`'s main world.
pub(crate) async fn detect_snapshot(
    session: &SessionHandle,
) -> Result<DetectionSnapshot, ImpervaError> {
    let res = session
        .call(
            "Runtime.evaluate",
            json!({
                "expression": include_str!("detect.js"),
                "returnByValue": true,
                "awaitPromise": true,
            }),
        )
        .await?;

    if let Some(details) = res.get("exceptionDetails") {
        let msg = details
            .get("exception")
            .and_then(|e| e.get("description"))
            .and_then(|d| d.as_str())
            .unwrap_or("unknown")
            .to_string();
        return Err(ImpervaError::JsError(msg));
    }

    let value = res
        .get("result")
        .and_then(|r| r.get("value"))
        .cloned()
        .unwrap_or(Value::Null);

    let raw: RawSnapshot = serde_json::from_value(value)
        .map_err(|e| ImpervaError::JsError(format!("invalid detect.js payload: {e}")))?;
    Ok(raw.into())
}

/// Surface-only probe. Convenience for callers wanting a non-blocking
/// "which surface is showing" check without the full snapshot.
pub async fn detect_surface(session: &SessionHandle) -> Result<ImpervaSurface, ImpervaError> {
    Ok(detect_snapshot(session).await?.surface)
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use zendriver_transport::testing::MockConnection;

    fn reply_value(mock_reply: serde_json::Value) -> serde_json::Value {
        json!({
            "result": {
                "type": "object",
                "value": mock_reply,
            }
        })
    }

    #[tokio::test]
    async fn detect_surface_returns_reese84_when_cookie_present() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move { detect_surface(&s).await }
        });

        let id = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id,
            reply_value(json!({
                "surface": { "kind": "Reese84" },
                "reese84": "TOKEN_XYZ",
                "body_clean": false,
                "sessions": [{ "name": "reese84", "value": "TOKEN_XYZ" }],
                "has_imperva_signal": true,
            })),
        )
        .await;

        let surf = fut.await.unwrap().unwrap();
        assert_eq!(surf, ImpervaSurface::Reese84);
        conn.shutdown();
    }

    #[tokio::test]
    async fn detect_surface_returns_legacy_for_incap_ses_cookies() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move { detect_surface(&s).await }
        });

        let id = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id,
            reply_value(json!({
                "surface": { "kind": "Legacy" },
                "reese84": null,
                "body_clean": false,
                "sessions": [{ "name": "incap_ses_123", "value": "ABC" }],
                "has_imperva_signal": true,
            })),
        )
        .await;

        let surf = fut.await.unwrap().unwrap();
        assert_eq!(surf, ImpervaSurface::Legacy);
        conn.shutdown();
    }

    #[tokio::test]
    async fn detect_surface_distinguishes_hcaptcha_vs_recaptcha_vs_native() {
        for (kind_str, expected) in [
            ("HCaptcha", CaptchaKind::HCaptcha),
            ("Recaptcha", CaptchaKind::Recaptcha),
            ("ImpervaNative", CaptchaKind::ImpervaNative),
            ("Unknown", CaptchaKind::Unknown),
        ] {
            let (mut mock, conn) = MockConnection::pair();
            let sess = SessionHandle::new(conn.clone(), "S1");

            let fut = tokio::spawn({
                let s = sess.clone();
                async move { detect_surface(&s).await }
            });

            let id = mock.expect_cmd("Runtime.evaluate").await;
            mock.reply(
                id,
                reply_value(json!({
                    "surface": { "kind": "Captcha", "captcha": kind_str },
                    "reese84": null,
                    "body_clean": false,
                    "sessions": [],
                    "has_imperva_signal": true,
                })),
            )
            .await;

            let surf = fut.await.unwrap().unwrap();
            assert_eq!(surf, ImpervaSurface::Captcha(expected));
            conn.shutdown();
        }
    }

    #[tokio::test]
    async fn detect_surface_returns_none_on_clean_page() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move { detect_surface(&s).await }
        });

        let id = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id,
            reply_value(json!({
                "surface": { "kind": "None" },
                "reese84": null,
                "body_clean": true,
                "sessions": [],
                "has_imperva_signal": false,
            })),
        )
        .await;

        let surf = fut.await.unwrap().unwrap();
        assert_eq!(surf, ImpervaSurface::None);
        conn.shutdown();
    }

    #[tokio::test]
    async fn detect_snapshot_propagates_js_exception_as_jserror() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move { detect_snapshot(&s).await }
        });

        let id = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id,
            json!({
                "result": { "type": "undefined" },
                "exceptionDetails": {
                    "exception": { "description": "TypeError: nope" }
                }
            }),
        )
        .await;

        let err = fut.await.unwrap().unwrap_err();
        assert!(matches!(err, ImpervaError::JsError(s) if s.contains("TypeError")));
        conn.shutdown();
    }
}
