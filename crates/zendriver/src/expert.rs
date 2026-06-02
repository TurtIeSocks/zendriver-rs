//! Expert-mode runtime hooks that live outside the stealth bundle.
//!
//! Currently this is just the **force-open-shadow-roots** patch, opted into via
//! [`BrowserBuilder::force_open_shadow_roots`](crate::BrowserBuilder::force_open_shadow_roots).
//! It is implemented as a standalone [`TargetObserver`] (rather than folded
//! into `zendriver-stealth`'s spoofed bundle) so it works with stealth fully
//! off and never contaminates the spoofed fingerprint.
//!
//! # Detectability
//!
//! The patch makes `Element.prototype.attachShadow` always create an **open**
//! root, so a page can detect it (call `attachShadow({ mode: "closed" })` and
//! observe a non-null `.shadowRoot`). Keep it off for stealth-sensitive work.

use serde_json::json;
use zendriver_transport::{ObserverError, PausedSession, TargetObserver};

/// JS injected on every new document when force-open-shadow-roots is enabled.
///
/// Replaces `Element.prototype.attachShadow` with a wrapper that coerces the
/// init dict's `mode` to `"open"` while preserving every other option
/// (`delegatesFocus`, `slotAssignment`, …) and tolerating a missing/loosely
/// typed argument. `attachShadow` returns the (now open) `ShadowRoot`, so the
/// element's `.shadowRoot` becomes reachable from automation even when the page
/// asked for a closed root.
pub(crate) const FORCE_OPEN_SHADOW_ROOT_SCRIPT: &str = r#"(function () {
  try {
    var proto = Element.prototype;
    var original = proto.attachShadow;
    if (typeof original !== "function" || original.__zendriverForcedOpen) {
      return;
    }
    var patched = function attachShadow(init) {
      var opts = (init && typeof init === "object") ? init : {};
      var forced = Object.assign({}, opts, { mode: "open" });
      return original.call(this, forced);
    };
    patched.__zendriverForcedOpen = true;
    Object.defineProperty(proto, "attachShadow", {
      value: patched,
      writable: true,
      configurable: true,
    });
  } catch (e) {
    /* never break page load if the patch can't apply */
  }
})();"#;

/// [`TargetObserver`] that installs [`FORCE_OPEN_SHADOW_ROOT_SCRIPT`] on every
/// new page target via `Page.addScriptToEvaluateOnNewDocument`.
///
/// Added to the observer chain by
/// [`BrowserBuilder::launch`](crate::BrowserBuilder::launch) /
/// [`connect`](crate::BrowserBuilder::connect) only when
/// [`force_open_shadow_roots(true)`](crate::BrowserBuilder::force_open_shadow_roots)
/// is set. Independent of the stealth observer — runs regardless of the active
/// [`StealthProfile`](zendriver_stealth::StealthProfile).
#[derive(Debug, Default)]
pub(crate) struct ShadowRootObserver;

#[async_trait::async_trait]
impl TargetObserver for ShadowRootObserver {
    fn name(&self) -> &'static str {
        "force-open-shadow-roots"
    }

    async fn on_target_attached(&self, session: PausedSession<'_>) -> Result<(), ObserverError> {
        // Only page targets have a DOM to patch; iframes inherit the patch via
        // the parent in flat session mode, and workers have no `Element`.
        if session.target_info.kind != "page" {
            return Ok(());
        }
        // `Page.enable` is idempotent; the stealth observer may also enable it
        // earlier in the chain, which is fine.
        session.call("Page.enable", json!({})).await?;
        session
            .call(
                "Page.addScriptToEvaluateOnNewDocument",
                json!({
                    "source": FORCE_OPEN_SHADOW_ROOT_SCRIPT,
                    "includeCommandLineAPI": false,
                    "runImmediately": true,
                }),
            )
            .await?;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn script_overrides_attach_shadow_to_open() {
        // The injected payload must patch attachShadow and force open mode.
        assert!(FORCE_OPEN_SHADOW_ROOT_SCRIPT.contains("attachShadow"));
        assert!(FORCE_OPEN_SHADOW_ROOT_SCRIPT.contains("\"open\""));
        // It must preserve other init keys rather than replacing the dict.
        assert!(FORCE_OPEN_SHADOW_ROOT_SCRIPT.contains("Object.assign"));
    }

    #[tokio::test]
    async fn observer_injects_script_on_page_attach() {
        use serde_json::json;
        use zendriver_transport::testing::MockConnection;

        let observer = std::sync::Arc::new(ShadowRootObserver);
        let (mut mock, conn) = MockConnection::pair_with_observers(vec![observer]);

        mock.emit_event(
            "Target.attachedToTarget",
            json!({
                "sessionId": "S1",
                "targetInfo": {
                    "targetId": "T1",
                    "type": "page",
                    "url": "about:blank",
                    "attached": true,
                },
                "waitingForDebugger": true,
            }),
        )
        .await;

        // Page.enable, then the addScriptToEvaluateOnNewDocument carrying the
        // attachShadow override, then the actor's debugger release.
        for expected in [
            "Page.enable",
            "Page.addScriptToEvaluateOnNewDocument",
            "Runtime.runIfWaitingForDebugger",
        ] {
            let id =
                tokio::time::timeout(std::time::Duration::from_secs(2), mock.expect_cmd(expected))
                    .await
                    .unwrap_or_else(|_| panic!("did not see {expected} within 2s"));
            // Assert the injected source on the script call.
            if expected == "Page.addScriptToEvaluateOnNewDocument" {
                let frame = mock.last_sent();
                let source = frame["params"]["source"].as_str().unwrap_or_default();
                assert!(
                    source.contains("attachShadow") && source.contains("\"open\""),
                    "injected script must force attachShadow open: {source}"
                );
            }
            mock.reply(id, json!({})).await;
        }

        conn.shutdown();
    }
}
