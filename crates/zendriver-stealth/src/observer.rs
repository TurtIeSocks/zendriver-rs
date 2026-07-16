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
use crate::persona::GeoPos;
use crate::{Fingerprint, Persona, ProfileKind, StealthProfile};

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
    /// Mock geolocation coordinates from the resolved [`Persona`], sent via
    /// `Emulation.setGeolocationOverride`. Unlike `timezone`/`locale` (carried
    /// on [`Fingerprint`]), geolocation has no `Fingerprint` counterpart, so
    /// it's captured here straight off the persona at construction time.
    geolocation: Option<GeoPos>,
}

impl StealthObserver {
    /// Build a new observer. Bootstrap source is composed eagerly so the
    /// per-target hot path only pays a `clone`/borrow.
    ///
    /// The bootstrap is driven by a [`Persona`] (surface-spoofing config) plus
    /// the [`Fingerprint`] (coherent UA / Chrome identity). This constructor
    /// uses [`Persona::default`] — no surface overrides, identity-only patches
    /// keep their current behavior. The launch path (a later task) will thread
    /// a caller-supplied persona via [`StealthObserver::with_persona`].
    #[must_use]
    pub fn new(profile: StealthProfile, fingerprint: Fingerprint) -> Self {
        Self::with_persona(profile, fingerprint, Persona::default())
    }

    /// Build a new observer with an explicit [`Persona`] driving the surface
    /// patches. `identity` still supplies the coherent UA / Chrome version.
    #[must_use]
    pub fn with_persona(
        profile: StealthProfile,
        fingerprint: Fingerprint,
        persona: Persona,
    ) -> Self {
        let bootstrap = if profile.kind() == ProfileKind::Spoofed {
            bootstrap_script(&persona, &fingerprint)
        } else {
            String::new()
        };
        let geolocation = persona.geolocation;
        Self {
            profile,
            fingerprint,
            bootstrap,
            geolocation,
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
        let accept_language = {
            let langs = crate::lang::resolve_languages(&Persona::default(), &self.fingerprint);
            // `Emulation.setUserAgentOverride.acceptLanguage` wants a PLAIN
            // comma-separated locale list (e.g. `en-US,en`) — Chrome appends the
            // `;q=` weights itself. Passing an already-weighted string (the
            // `accept_language()` header form) makes Chrome double them, yielding
            // a malformed `Accept-Language: en-US,en;q=0.9;q=0.9`. Send the bare
            // list so the emitted header is a clean `en-US,en;q=0.9`.
            langs.join(",")
        };
        session
            .call(
                "Emulation.setUserAgentOverride",
                json!({
                    "userAgent": &self.fingerprint.ua_string,
                    "acceptLanguage": accept_language,
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
        if let Some(ref geo) = self.geolocation {
            // Sets the value the Geolocation API *would* return — it does
            // NOT grant the `geolocation` permission. Chrome still gates the
            // API behind a permission prompt/grant; auto-granting it here
            // would be a separate (and itself suspicious) signal, so we
            // deliberately leave permissioning to the caller.
            let mut params = json!({
                "latitude": geo.latitude,
                "longitude": geo.longitude,
            });
            if let Some(accuracy) = geo.accuracy {
                params["accuracy"] = json!(accuracy);
            }
            session
                .call("Emulation.setGeolocationOverride", params)
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
            languages: None,
            screen: None,
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
            // `acceptLanguage` must be a PLAIN locale list — Chrome adds the
            // `;q=` weights. A weighted value here makes Chrome double them into
            // a malformed `en-US,en;q=0.9;q=0.9` header.
            if expected == "Emulation.setUserAgentOverride" {
                let al = mock.last_sent()["params"]["acceptLanguage"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                assert!(!al.is_empty(), "acceptLanguage must be set");
                assert!(
                    !al.contains(";q="),
                    "acceptLanguage must be a bare locale list (no q-weights); got: {al}"
                );
            }
            mock.reply(id, json!({})).await;
        }

        conn.shutdown();
    }

    #[tokio::test]
    async fn spoofed_observer_emits_geolocation_override_when_persona_has_geo() {
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
            languages: None,
            screen: None,
        };
        let persona = crate::Persona {
            geolocation: Some(crate::persona::GeoPos {
                latitude: 21.0285,
                longitude: 105.8542,
                accuracy: Some(50.0),
            }),
            ..crate::Persona::default()
        };
        let profile = StealthProfile::spoofed();
        let observer = std::sync::Arc::new(StealthObserver::with_persona(profile, fp, persona));

        let (mut mock, conn) = MockConnection::pair_with_observers(vec![observer.clone()]);

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

        for expected in [
            "Page.enable",
            "Emulation.setUserAgentOverride",
            "Emulation.setDeviceMetricsOverride",
            "Emulation.setFocusEmulationEnabled",
            "Emulation.setGeolocationOverride",
            "Page.setBypassCSP",
            "Page.addScriptToEvaluateOnNewDocument",
            "Runtime.runIfWaitingForDebugger",
        ] {
            let id =
                tokio::time::timeout(std::time::Duration::from_secs(2), mock.expect_cmd(expected))
                    .await
                    .unwrap_or_else(|_| panic!("did not see {expected} within 2s"));
            if expected == "Emulation.setGeolocationOverride" {
                let params = mock.last_sent()["params"].clone();
                assert_eq!(params["latitude"].as_f64(), Some(21.0285));
                assert_eq!(params["longitude"].as_f64(), Some(105.8542));
                assert_eq!(params["accuracy"].as_f64(), Some(50.0));
            }
            mock.reply(id, json!({})).await;
        }

        conn.shutdown();
    }

    #[tokio::test]
    async fn geolocation_override_omits_accuracy_when_unset() {
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
            languages: None,
            screen: None,
        };
        let persona = crate::Persona {
            geolocation: Some(crate::persona::GeoPos {
                latitude: 1.0,
                longitude: 2.0,
                accuracy: None,
            }),
            ..crate::Persona::default()
        };
        let profile = StealthProfile::native();
        let observer = std::sync::Arc::new(StealthObserver::with_persona(profile, fp, persona));

        let (mut mock, conn) = MockConnection::pair_with_observers(vec![observer.clone()]);

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

        for expected in [
            "Page.enable",
            "Emulation.setUserAgentOverride",
            "Emulation.setDeviceMetricsOverride",
            "Emulation.setFocusEmulationEnabled",
            "Emulation.setGeolocationOverride",
            "Runtime.runIfWaitingForDebugger",
        ] {
            let id =
                tokio::time::timeout(std::time::Duration::from_secs(2), mock.expect_cmd(expected))
                    .await
                    .unwrap_or_else(|_| panic!("did not see {expected} within 2s"));
            if expected == "Emulation.setGeolocationOverride" {
                let params = mock.last_sent()["params"].clone();
                assert_eq!(params["latitude"].as_f64(), Some(1.0));
                assert_eq!(params["longitude"].as_f64(), Some(2.0));
                assert!(
                    params.get("accuracy").is_none(),
                    "accuracy must be omitted when unset, got: {params}"
                );
            }
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
            languages: None,
            screen: None,
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
