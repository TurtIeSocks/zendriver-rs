//! CAPTCHA escalation: types handed to / returned from the caller-supplied
//! solver, plus the CDP helpers that build the challenge descriptor and apply
//! the solved `datadome` cookie. DataDome's solution is a COOKIE (it
//! whitelists the browser), not a form-field token — the structural delta from
//! imperva.

use std::sync::Arc;

use futures::future::BoxFuture;
use serde_json::json;
use zendriver_transport::SessionHandle;

use crate::detection::DetectionSnapshot;
use crate::error::DataDomeError;

/// CAPTCHA escalation handed to a caller-supplied solver.
#[derive(Debug, Clone)]
pub struct DataDomeChallenge {
    /// captcha-delivery iframe URL, e.g.
    /// `https://geo.captcha-delivery.com/captcha/?initialCid=…&hash=…&cid=…&t=fe`.
    pub captcha_url: String,
    /// URL of the page presenting the CAPTCHA.
    pub site_url: String,
    /// Browser UA — MUST match the page's UA (solver-service requirement).
    pub user_agent: String,
    /// datadome cookie / `dd.cid`, when known.
    pub cid: Option<String>,
    /// `dd.hsh`, when known.
    pub hash: Option<String>,
}

/// Token returned by a caller-supplied solver. For DataDome this is the solved
/// `datadome` COOKIE value.
#[derive(Debug, Clone)]
pub struct DataDomeSolution {
    pub datadome_cookie: String,
}

/// Erased solver closure, stored behind `Arc<dyn ...>` on [`DataDomeBypass`].
///
/// [`DataDomeBypass`]: crate::bypass::DataDomeBypass
pub(crate) type CaptchaSolver = dyn Fn(
        DataDomeChallenge,
    )
        -> BoxFuture<'static, Result<DataDomeSolution, Box<dyn std::error::Error + Send + Sync>>>
    + Send
    + Sync;

/// Wrap a typed closure into the stored `Arc<dyn CaptchaSolver>` shape.
pub(crate) fn arc_solver<F, Fut>(f: F) -> Arc<CaptchaSolver>
where
    F: Fn(DataDomeChallenge) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<
            Output = Result<DataDomeSolution, Box<dyn std::error::Error + Send + Sync>>,
        > + Send
        + 'static,
{
    Arc::new(move |challenge| Box::pin(f(challenge)))
}

/// Build a [`DataDomeChallenge`] from a detection snapshot + the live page URL
/// + UA (read via CDP).
pub(crate) async fn build_challenge(
    session: &SessionHandle,
    snap: &DetectionSnapshot,
) -> Result<DataDomeChallenge, DataDomeError> {
    // Read location.href + navigator.userAgent in one eval.
    let res = session
        .call(
            "Runtime.evaluate",
            json!({
                "expression": "({ url: location.href, ua: navigator.userAgent })",
                "returnByValue": true,
            }),
        )
        .await?;
    let v = res
        .get("result")
        .and_then(|r| r.get("value"))
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let site_url = v
        .get("url")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let user_agent = v
        .get("ua")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();

    if site_url.is_empty() || user_agent.is_empty() {
        tracing::warn!(
            "datadome: build_challenge read empty url/ua — solver may receive an incomplete challenge"
        );
    }

    Ok(DataDomeChallenge {
        captcha_url: snap.captcha_url.clone().unwrap_or_default(),
        site_url,
        user_agent,
        cid: snap
            .dd
            .as_ref()
            .and_then(|d| d.cid.clone())
            .or_else(|| snap.datadome.clone()),
        hash: snap.dd.as_ref().and_then(|d| d.hsh.clone()),
    })
}

/// Apply the solved `datadome` cookie via `Network.setCookie`, scoped to the
/// registrable parent domain of `site_url` (DataDome sets the cookie on the
/// eTLD+1 so it covers subdomains), then reload the page.
pub(crate) async fn apply_solution(
    session: &SessionHandle,
    solution: &DataDomeSolution,
    site_url: &str,
) -> Result<(), DataDomeError> {
    let domain = cookie_domain(site_url);
    let res = session
        .call(
            "Network.setCookie",
            json!({
                "name": "datadome",
                "value": solution.datadome_cookie,
                "domain": domain,
                "path": "/",
                "secure": true,
                "sameSite": "Lax",
            }),
        )
        .await?;
    if res.get("success").and_then(|s| s.as_bool()) == Some(false) {
        tracing::warn!(
            domain = %domain,
            "datadome: Network.setCookie reported success=false — the solved cookie was rejected (bad domain?); clearance will likely time out"
        );
    }
    session.call("Page.reload", json!({})).await?;
    Ok(())
}

/// Derive the `.eTLD+1` cookie domain from a URL: take the host, then look
/// up its registrable domain (public suffix + one label) against the
/// compiled-in Mozilla Public Suffix List via the [`psl`] crate, prefixing
/// the result with `.`.
///
/// (`shop.example.com` → `.example.com`; `example.com` → `.example.com`;
/// `shop.example.co.uk` → `.example.co.uk` — multi-label public suffixes
/// like `co.uk` resolve correctly instead of the old naive "drop one
/// label" heuristic, which mistook `co.uk` itself for the registrable
/// domain.)
///
/// Falls back to the bare host — no leading dot — when `psl` finds no
/// registrable domain: bare IP addresses (`psl` has no suffix entries for
/// numeric labels, so treating them as a domain tree would misparse
/// `127.0.0.1` as suffix `1` under the PSL's default "unlisted TLD" rule)
/// and single-label hosts like `localhost`.
fn cookie_domain(site_url: &str) -> String {
    let host = site_url
        .split("://")
        .nth(1)
        .unwrap_or(site_url)
        .split('/')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("");

    if host.parse::<std::net::IpAddr>().is_ok() {
        return host.to_string();
    }

    match psl::domain_str(host) {
        Some(registrable) => format!(".{registrable}"),
        None => host.to_string(),
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;
    use zendriver_transport::testing::MockConnection;

    #[test]
    fn cookie_domain_derives_etld_plus_one() {
        assert_eq!(cookie_domain("https://shop.example.com/x"), ".example.com");
        assert_eq!(cookie_domain("https://example.com/"), ".example.com");
        assert_eq!(
            cookie_domain("https://a.b.example.com/p?q=1"),
            ".example.com"
        );
        assert_eq!(cookie_domain("http://localhost:8080/"), "localhost");
    }

    /// Multi-label public suffixes (`co.uk`, `uk.com`, ...) resolve to the
    /// correct registrable domain via the compiled-in public suffix list,
    /// fixing the v1 naive "drop one label" heuristic that mistook `co.uk`
    /// itself for the registrable domain (previously asserted here as the
    /// documented-wrong `.co.uk`).
    #[test]
    fn cookie_domain_handles_multi_label_public_suffixes() {
        assert_eq!(
            cookie_domain("https://shop.example.co.uk/"),
            ".example.co.uk"
        );
        assert_eq!(
            cookie_domain("https://a.b.shop.example.co.uk/p?q=1"),
            ".example.co.uk"
        );
        // "uk.com" is a private-section PSL entry (not ICANN-delegated),
        // covered the same way as an ICANN multi-label suffix.
        assert_eq!(
            cookie_domain("https://a.example.uk.com/"),
            ".example.uk.com"
        );
    }

    /// Bare IP addresses have no registrable domain under the public
    /// suffix list (numeric labels never match a PSL rule, so the PSL's
    /// default "unlisted TLD" rule would otherwise misparse `127.0.0.1` as
    /// suffix `1` / domain `0.1`) — must fall back to the exact host, no
    /// leading dot.
    #[test]
    fn cookie_domain_falls_back_to_exact_host_for_ip_addresses() {
        assert_eq!(cookie_domain("http://127.0.0.1:8080/"), "127.0.0.1");
    }

    #[tokio::test]
    async fn build_challenge_handles_null_eval_without_panicking() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let snap = crate::detection::DetectionSnapshot {
            surface: crate::detection::DataDomeSurface::Captcha,
            datadome: Some("CID".into()),
            dd: None,
            captcha_url: Some("https://geo.captcha-delivery.com/captcha/?cid=CID".into()),
            body_clean: false,
        };
        let fut = tokio::spawn({
            let s = sess.clone();
            async move { build_challenge(&s, &snap).await }
        });
        let id = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(id, json!({"result":{"type":"undefined"}})).await; // no value field
        let ch = fut.await.unwrap().unwrap();
        assert_eq!(
            ch.captcha_url,
            "https://geo.captcha-delivery.com/captcha/?cid=CID"
        );
        assert_eq!(ch.cid.as_deref(), Some("CID")); // falls back to snap.datadome
        assert!(ch.site_url.is_empty() && ch.user_agent.is_empty());
        conn.shutdown();
    }

    #[tokio::test]
    async fn apply_solution_sets_cookie_then_reloads() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let sol = DataDomeSolution {
            datadome_cookie: "SOLVED_DD".into(),
        };
        let fut = tokio::spawn({
            let s = sess.clone();
            async move { apply_solution(&s, &sol, "https://shop.example.com/x").await }
        });

        let id_cookie = mock.expect_cmd("Network.setCookie").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["name"], "datadome");
        assert_eq!(sent["params"]["value"], "SOLVED_DD");
        assert_eq!(sent["params"]["domain"], ".example.com");
        mock.reply(id_cookie, json!({ "success": true })).await;

        let id_reload = mock.expect_cmd("Page.reload").await;
        mock.reply(id_reload, json!({})).await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }
}
