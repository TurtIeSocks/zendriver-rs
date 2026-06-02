//! DataDome surface detection. One `Runtime.evaluate` round-trip bundles the
//! cookie read, `window.dd` parse, and captcha-delivery iframe walk.

use serde::Deserialize;
use serde_json::{Value, json};
use zendriver_transport::SessionHandle;

use crate::error::DataDomeError;

/// Which DataDome surface a tab is currently showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataDomeSurface {
    /// window.dd.t == 'fe' device-check / interstitial — invisible JS
    /// interrogation; also covers the auto-resolving "please wait" page.
    DeviceCheck,
    /// captcha-delivery.com iframe present (slider / puzzle / press-hold).
    Captcha,
    /// window.dd.t == 'bv' — IP banned; unsolvable in-browser.
    Block,
    /// No DataDome surface detected; the datadome cookie may already be valid.
    None,
}

/// Parsed `window.dd` challenge descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DdConfig {
    pub cid: Option<String>,
    pub hsh: Option<String>,
    pub t: Option<String>,
    pub host: Option<String>,
}

/// Snapshot of one `detect.js` round-trip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectionSnapshot {
    pub surface: DataDomeSurface,
    pub datadome: Option<String>,
    pub dd: Option<DdConfig>,
    pub captcha_url: Option<String>,
    pub body_clean: bool,
}

#[derive(Debug, Deserialize)]
struct RawSnapshot {
    surface: String,
    #[serde(default)]
    datadome: Option<String>,
    #[serde(default)]
    dd: Option<DdConfig>,
    #[serde(default)]
    captcha_url: Option<String>,
    body_clean: bool,
}

impl From<RawSnapshot> for DetectionSnapshot {
    fn from(r: RawSnapshot) -> Self {
        let surface = match r.surface.as_str() {
            "device_check" => DataDomeSurface::DeviceCheck,
            "captcha" => DataDomeSurface::Captcha,
            "block" => DataDomeSurface::Block,
            _ => DataDomeSurface::None,
        };
        Self {
            surface,
            datadome: r.datadome,
            dd: r.dd,
            captcha_url: r.captcha_url,
            body_clean: r.body_clean,
        }
    }
}

/// Run a single `detect.js` probe against `session`'s main world.
pub(crate) async fn detect_snapshot(
    session: &SessionHandle,
) -> Result<DetectionSnapshot, DataDomeError> {
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
        return Err(DataDomeError::JsError(msg));
    }

    let value = res
        .get("result")
        .and_then(|r| r.get("value"))
        .cloned()
        .unwrap_or(Value::Null);

    let raw: RawSnapshot = serde_json::from_value(value)
        .map_err(|e| DataDomeError::JsError(format!("invalid detect.js payload: {e}")))?;
    Ok(raw.into())
}

/// Surface-only probe. Convenience for "which surface am I on" without driving
/// a bypass.
pub async fn detect_surface(session: &SessionHandle) -> Result<DataDomeSurface, DataDomeError> {
    Ok(detect_snapshot(session).await?.surface)
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;
    use zendriver_transport::testing::MockConnection;

    fn reply(v: serde_json::Value) -> serde_json::Value {
        json!({ "result": { "type": "object", "value": v } })
    }

    #[tokio::test]
    async fn detect_surface_classifies_each_surface() {
        for (payload, expected) in [
            (
                json!({"surface":"device_check","datadome":null,"dd":{"cid":"C","hsh":"H","t":"fe","host":"geo.captcha-delivery.com"},"captcha_url":null,"body_clean":false}),
                DataDomeSurface::DeviceCheck,
            ),
            (
                json!({"surface":"captcha","datadome":"DD","dd":{"cid":"C","hsh":"H","t":"fe","host":"geo.captcha-delivery.com"},"captcha_url":"https://geo.captcha-delivery.com/captcha/?cid=C","body_clean":false}),
                DataDomeSurface::Captcha,
            ),
            (
                json!({"surface":"block","datadome":null,"dd":{"cid":"C","hsh":"H","t":"bv","host":"geo.captcha-delivery.com"},"captcha_url":null,"body_clean":false}),
                DataDomeSurface::Block,
            ),
            (
                json!({"surface":"none","datadome":"DD","dd":null,"captcha_url":null,"body_clean":true}),
                DataDomeSurface::None,
            ),
        ] {
            let (mut mock, conn) = MockConnection::pair();
            let sess = SessionHandle::new(conn.clone(), "S1");
            let fut = tokio::spawn({
                let s = sess.clone();
                async move { detect_surface(&s).await }
            });
            let id = mock.expect_cmd("Runtime.evaluate").await;
            assert!(
                mock.last_sent()["params"]["expression"]
                    .as_str()
                    .unwrap()
                    .contains("captcha-delivery.com")
            );
            mock.reply(id, reply(payload)).await;
            assert_eq!(fut.await.unwrap().unwrap(), expected);
            conn.shutdown();
        }
    }
}
