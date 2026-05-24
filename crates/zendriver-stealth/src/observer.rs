//! StealthObserver: applies a [`StealthProfile`] to each new attached target.
//!
//! Installed in the [`zendriver_transport`] actor's observer chain. On
//! `Target.attachedToTarget`, the actor pauses the new target, walks every
//! observer serially, then releases the debugger via
//! `Runtime.runIfWaitingForDebugger`. The observer's job is to push every
//! UA/screen/timezone/locale override and (for spoofed) the bootstrap script
//! _before_ the debugger releases, so the first script the page runs sees the
//! patched globals.

use serde_json::json;
use zendriver_transport::{ObserverError, PausedSession, TargetObserver};

use crate::patches::bootstrap_script;
use crate::{Fingerprint, ProfileKind, StealthProfile};

/// Observer that applies a [`StealthProfile`] + [`Fingerprint`] to every page
/// target. Workers and iframes are skipped — workers have no DOM and iframes
/// inherit patches from the parent target in flat session mode.
#[derive(Debug)]
pub struct StealthObserver {
    profile: StealthProfile,
    fingerprint: Fingerprint,
    /// Pre-rendered bootstrap source. Empty for `Off`/`Native` — we never send
    /// `Page.addScriptToEvaluateOnNewDocument` in those modes, so there is no
    /// need to pay the patches-bundle cost for them.
    bootstrap: String,
}

impl StealthObserver {
    /// Build a new observer. Bootstrap source is composed eagerly so the
    /// per-target hot path only pays a `clone`/borrow.
    #[must_use]
    pub fn new(profile: StealthProfile, fingerprint: Fingerprint) -> Self {
        let bootstrap = if profile.kind() == ProfileKind::Spoofed {
            bootstrap_script(&fingerprint)
        } else {
            String::new()
        };
        Self {
            profile,
            fingerprint,
            bootstrap,
        }
    }
}

#[async_trait::async_trait]
impl TargetObserver for StealthObserver {
    fn name(&self) -> &'static str {
        "stealth"
    }

    async fn on_target_attached(&self, session: PausedSession<'_>) -> Result<(), ObserverError> {
        // Workers + iframes are skipped — workers have no DOM; iframes inherit
        // patches via the parent in flat mode.
        if session.target_info.kind != "page" {
            return Ok(());
        }
        if self.profile.kind() == ProfileKind::Off {
            return Ok(());
        }

        session.call("Page.enable", json!({})).await?;

        // UA override — Emulation.setUserAgentOverride carries the Client-Hints
        // metadata too, so we don't have to send Network.setUserAgentOverride
        // separately.
        session
            .call(
                "Emulation.setUserAgentOverride",
                json!({
                    "userAgent": &self.fingerprint.ua_string,
                    "acceptLanguage": self.fingerprint
                        .locale
                        .as_deref()
                        .unwrap_or("en-US,en;q=0.9"),
                    "platform": self.fingerprint.platform.ch_platform(),
                    "userAgentMetadata": &self.fingerprint.ua_metadata,
                }),
            )
            .await?;

        // Screen-size override + focus emulation: keeps headless from leaking
        // an oddly-shaped viewport and from reporting `document.hasFocus()`
        // false for the (always-backgrounded) headless tab.
        session
            .call(
                "Emulation.setDeviceMetricsOverride",
                json!({
                    "width": 1920,
                    "height": 1080,
                    "deviceScaleFactor": 1.0,
                    "mobile": false,
                    "screenWidth": 1920,
                    "screenHeight": 1080,
                }),
            )
            .await?;

        session
            .call(
                "Emulation.setFocusEmulationEnabled",
                json!({ "enabled": true }),
            )
            .await?;

        if let Some(ref tz) = self.fingerprint.timezone {
            session
                .call("Emulation.setTimezoneOverride", json!({ "timezoneId": tz }))
                .await?;
        }
        if let Some(ref locale) = self.fingerprint.locale {
            session
                .call("Emulation.setLocaleOverride", json!({ "locale": locale }))
                .await?;
        }

        if self.profile.kind() == ProfileKind::Spoofed {
            if self.profile.bypass_csp_enabled() {
                session
                    .call("Page.setBypassCSP", json!({ "enabled": true }))
                    .await?;
            }
            // Inject into the MAIN world (no `worldName`). The bootstrap's
            // patches mutate `Navigator.prototype`, `window.chrome`,
            // `WebGLRenderingContext.prototype`, etc. — every isolated
            // world gets its own copy of these prototypes, so a patch
            // applied in a named/isolated world is invisible to the
            // page's own scripts (and to `evaluate_main`, the surface
            // detection sites probe). Running the bootstrap in the main
            // world is the only way these prototype mutations actually
            // affect the document under test.
            session
                .call(
                    "Page.addScriptToEvaluateOnNewDocument",
                    json!({
                        "source": &self.bootstrap,
                        "includeCommandLineAPI": false,
                        "runImmediately": true,
                    }),
                )
                .await?;
        }

        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::Platform;
    use serde_json::json;
    use zendriver_transport::testing::MockConnection;

    #[tokio::test]
    async fn spoofed_observer_sends_expected_sequence_for_page_target() {
        let fp = Fingerprint {
            platform: Platform::MacIntel,
            chrome_major: 120,
            chrome_full: "120.0.6099.234".into(),
            cpu_count: 10,
            memory_gb: 8,
            ua_string: crate::ua::compose_ua_string(Platform::MacIntel, "120.0.6099.234"),
            ua_metadata: crate::UserAgentMetadata::realistic(
                Platform::MacIntel,
                120,
                "120.0.6099.234",
            ),
            timezone: None,
            locale: None,
        };
        let profile = StealthProfile::spoofed();
        let observer = std::sync::Arc::new(StealthObserver::new(profile, fp));

        let (mut mock, conn) = MockConnection::pair_with_observers(vec![observer.clone()]);

        // Emit a Target.attachedToTarget event.
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

        // Expected sequence (each followed by a reply so the observer
        // continues). The closing Runtime.runIfWaitingForDebugger is the
        // actor's debugger-release after every observer succeeds.
        for expected in [
            "Page.enable",
            "Emulation.setUserAgentOverride",
            "Emulation.setDeviceMetricsOverride",
            "Emulation.setFocusEmulationEnabled",
            "Page.setBypassCSP",
            "Page.addScriptToEvaluateOnNewDocument",
            "Runtime.runIfWaitingForDebugger",
        ] {
            let id =
                tokio::time::timeout(std::time::Duration::from_secs(2), mock.expect_cmd(expected))
                    .await
                    .unwrap_or_else(|_| panic!("did not see {expected} within 2s"));
            mock.reply(id, json!({})).await;
        }

        conn.shutdown();
    }

    #[tokio::test]
    async fn off_observer_skips_all_commands_just_releases_debugger() {
        let fp = Fingerprint {
            platform: Platform::MacIntel,
            chrome_major: 120,
            chrome_full: "120.0.6099.234".into(),
            cpu_count: 10,
            memory_gb: 8,
            ua_string: String::new(),
            ua_metadata: crate::UserAgentMetadata::realistic(
                Platform::MacIntel,
                120,
                "120.0.6099.234",
            ),
            timezone: None,
            locale: None,
        };
        let observer = std::sync::Arc::new(StealthObserver::new(StealthProfile::off(), fp));
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

        // Off profile: only the actor's release-debugger call.
        let id = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            mock.expect_cmd("Runtime.runIfWaitingForDebugger"),
        )
        .await
        .unwrap();
        mock.reply(id, json!({})).await;
        conn.shutdown();
    }
}
