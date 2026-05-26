//! CAPTCHA escalation path: types passed to / returned from the
//! caller-supplied solver, plus the small CDP helpers that probe the page
//! for the embed's site key and inject the solver's response back into the
//! form field. Kept out of `bypass.rs` so that file stays focused on the
//! poll loop.

use std::sync::Arc;

use futures::future::BoxFuture;
use serde_json::json;
use zendriver_transport::SessionHandle;

use crate::detection::CaptchaKind;
use crate::error::ImpervaError;

/// CAPTCHA escalation handed to a user-supplied solver.
#[derive(Debug, Clone)]
pub struct CaptchaChallenge {
    pub kind: CaptchaKind,
    /// Site key extracted from the embed (hCaptcha / reCAPTCHA). `None`
    /// if the kind is `ImpervaNative` or `Unknown`, or if the probe found
    /// no matching `data-sitekey` attribute on the page.
    pub site_key: Option<String>,
    /// URL of the page presenting the CAPTCHA.
    pub url: String,
}

/// Token returned by a user-supplied CAPTCHA solver.
#[derive(Debug, Clone)]
pub struct CaptchaSolution {
    /// Verification token issued by the solver service.
    pub token: String,
    /// DOM field where the token must be injected for the page to accept it
    /// (e.g. `"h-captcha-response"`, `"g-recaptcha-response"`).
    pub form_field: String,
}

/// Erased solver closure: `Fn(CaptchaChallenge) -> Future<Result<...>>` with
/// `Send + Sync + 'static`, type-erased so [`ImpervaBypass`] can store it
/// behind an `Arc<dyn ...>` without leaking the closure's generic parameters
/// through the struct. The type alias spans four lines because dyn-trait
/// aliases over async closures are simply verbose in Rust 2024 — there's
/// no shorter form short of introducing a sealed marker trait.
///
/// [`ImpervaBypass`]: crate::bypass::ImpervaBypass
pub(crate) type CaptchaSolver = dyn Fn(
        CaptchaChallenge,
    ) -> BoxFuture<'static, Result<CaptchaSolution, Box<dyn std::error::Error + Send + Sync>>>
    + Send
    + Sync;

/// Convenience: wrap a typed closure into the `Arc<dyn CaptchaSolver>`
/// shape stored on [`ImpervaBypass`].
///
/// [`ImpervaBypass`]: crate::bypass::ImpervaBypass
pub(crate) fn arc_solver<F, Fut>(f: F) -> Arc<CaptchaSolver>
where
    F: Fn(CaptchaChallenge) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<
            Output = Result<CaptchaSolution, Box<dyn std::error::Error + Send + Sync>>,
        > + Send
        + 'static,
{
    Arc::new(move |challenge| Box::pin(f(challenge)))
}

/// Extract a CAPTCHA site key from the current page via a small inline JS
/// probe. Returns `(None, location.href)` if no recognizable embed is
/// present for the given `kind`.
pub(crate) async fn extract_captcha_site_key(
    session: &SessionHandle,
    kind: CaptchaKind,
) -> Result<(Option<String>, String), ImpervaError> {
    const PROBE_JS: &str = r#"
    (function () {
        function findKey(selector, attr) {
            var el = document.querySelector(selector);
            return el ? el.getAttribute(attr) : null;
        }
        var hcap =
            findKey(".h-captcha", "data-sitekey") ||
            findKey("[data-hcaptcha-sitekey]", "data-hcaptcha-sitekey");
        var rcap =
            findKey(".g-recaptcha", "data-sitekey") ||
            findKey("[data-recaptcha-sitekey]", "data-recaptcha-sitekey");
        return { hcap: hcap, rcap: rcap, url: location.href };
    })()
    "#;

    let res = session
        .call(
            "Runtime.evaluate",
            json!({
                "expression": PROBE_JS,
                "returnByValue": true,
            }),
        )
        .await?;
    let value = res
        .get("result")
        .and_then(|r| r.get("value"))
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    #[derive(serde::Deserialize)]
    struct Probe {
        hcap: Option<String>,
        rcap: Option<String>,
        url: String,
    }
    let probe: Probe = serde_json::from_value(value)
        .map_err(|e| ImpervaError::JsError(format!("invalid captcha probe payload: {e}")))?;

    let site_key = match kind {
        CaptchaKind::HCaptcha => probe.hcap,
        CaptchaKind::Recaptcha => probe.rcap,
        CaptchaKind::ImpervaNative | CaptchaKind::Unknown => None,
    };
    Ok((site_key, probe.url))
}

/// Inject a CAPTCHA solver token into the named form field via
/// `Runtime.evaluate`. Creates a hidden `<textarea>` with the right
/// name+id if no matching field exists.
pub(crate) async fn inject_captcha_solution(
    session: &SessionHandle,
    solution: &CaptchaSolution,
) -> Result<(), ImpervaError> {
    // Escape `\` first, then `"` — order matters; reversing it would
    // double-escape the backslashes inserted by the quote pass.
    let name = solution
        .form_field
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let script = format!(
        r#"
        (function () {{
            var field = document.querySelector('[name="{name}"]')
                || document.getElementById("{name}");
            if (!field) {{
                var t = document.createElement("textarea");
                t.name = "{name}";
                t.id = "{name}";
                t.style.display = "none";
                document.body.appendChild(t);
                field = t;
            }}
            field.value = {token};
            field.dispatchEvent(new Event("change", {{ bubbles: true }}));
            return true;
        }})()
        "#,
        name = name,
        token = serde_json::Value::String(solution.token.clone()),
    );

    let res = session
        .call(
            "Runtime.evaluate",
            json!({
                "expression": script,
                "returnByValue": true,
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
    Ok(())
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use zendriver_transport::testing::MockConnection;

    #[tokio::test]
    async fn extract_returns_none_when_neither_sitekey_attr_present() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move { extract_captcha_site_key(&s, CaptchaKind::HCaptcha).await }
        });

        let id = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id,
            json!({
                "result": {
                    "type": "object",
                    "value": {
                        "hcap": null,
                        "rcap": null,
                        "url": "https://example.com/x",
                    }
                }
            }),
        )
        .await;

        let (site_key, url) = fut.await.unwrap().unwrap();
        assert!(site_key.is_none(), "no .h-captcha[data-sitekey] → None");
        assert_eq!(url, "https://example.com/x");
        conn.shutdown();
    }

    #[tokio::test]
    async fn extract_uses_rcap_for_recaptcha_kind() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move { extract_captcha_site_key(&s, CaptchaKind::Recaptcha).await }
        });

        let id = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id,
            json!({
                "result": {
                    "type": "object",
                    "value": {
                        "hcap": "IGNORED",
                        "rcap": "RKEY",
                        "url": "https://x.com/",
                    }
                }
            }),
        )
        .await;

        let (site_key, _) = fut.await.unwrap().unwrap();
        assert_eq!(site_key.as_deref(), Some("RKEY"));
        conn.shutdown();
    }

    #[tokio::test]
    async fn extract_returns_none_for_imperva_native_kind() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move { extract_captcha_site_key(&s, CaptchaKind::ImpervaNative).await }
        });

        let id = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id,
            json!({
                "result": {
                    "type": "object",
                    "value": {
                        "hcap": "WOULD_IGNORE",
                        "rcap": "WOULD_IGNORE",
                        "url": "https://x.com/",
                    }
                }
            }),
        )
        .await;

        let (site_key, _) = fut.await.unwrap().unwrap();
        assert!(
            site_key.is_none(),
            "ImpervaNative kind never extracts a site_key"
        );
        conn.shutdown();
    }

    #[tokio::test]
    async fn inject_escapes_backslash_and_quote_in_form_field() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let solution = CaptchaSolution {
            token: "TOK".into(),
            form_field: r#"a\b"c"#.into(),
        };

        let fut = tokio::spawn({
            let s = sess.clone();
            async move { inject_captcha_solution(&s, &solution).await }
        });

        let id = mock.expect_cmd("Runtime.evaluate").await;
        let sent = mock.last_sent();
        let script = sent["params"]["expression"].as_str().unwrap();
        assert!(
            script.contains(r#"a\\b\"c"#),
            "form_field must have \\ and \" both escaped; script was: {script}"
        );
        mock.reply(
            id,
            json!({ "result": { "type": "boolean", "value": true } }),
        )
        .await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    #[tokio::test]
    async fn inject_returns_jserror_when_evaluator_raises() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let solution = CaptchaSolution {
            token: "TOK".into(),
            form_field: "h-captcha-response".into(),
        };

        let fut = tokio::spawn({
            let s = sess.clone();
            async move { inject_captcha_solution(&s, &solution).await }
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
