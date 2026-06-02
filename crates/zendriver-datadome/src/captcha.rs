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
    session
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
    session.call("Page.reload", json!({})).await?;
    Ok(())
}

/// Derive the `.eTLD+1` cookie domain from a URL. Best-effort: take the host,
/// drop the leftmost label when there are ≥3 labels, prefix with `.`.
/// (`shop.example.com` → `.example.com`; `example.com` → `.example.com`.)
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
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() >= 3 {
        format!(".{}", labels[labels.len() - 2..].join("."))
    } else if labels.len() == 2 {
        format!(".{host}")
    } else {
        host.to_string()
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;
    use zendriver_transport::testing::MockConnection;

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
