# Changelog

All notable changes to this crate documented here. Format: [Keep a
Changelog](https://keepachangelog.com/en/1.1.0/). Adheres to [SemVer](https://semver.org/).

## [0.1.3] - 2026-06-02


## [0.1.2] - 2026-06-01


## [0.1.1] - 2026-05-26


## [0.1.0] - Unreleased

First release.

### Added

- Passive bypass driver for Imperva WAF / Incapsula. `Tab::imperva()`
  convenience constructs an `ImpervaBypass` bound to the tab's session;
  `wait_for_clearance()` runs a surface-aware poll loop returning
  `TokenAcquired { reese84, sessions } | ChallengeGone | AlreadyClear`.
- Three Imperva surfaces detected: modern reese84 bot management,
  legacy Incapsula (`___utmvc` / `incap_ses_*` / `visid_incap_*`), and
  CAPTCHA escalation (hCaptcha / reCAPTCHA / Imperva native).
- S3 hybrid AND clearance signal: `TokenAcquired` requires both a
  non-empty `reese84` cookie AND body markers cleared; `ChallengeGone`
  additionally requires the JS surface detector to report `None`.
- Opt-in Fetch-domain fast-path via `.with_interception()` —
  subscribes to `/_Incapsula_Resource*` and `Reese.js` responses,
  signals clearance on first 2xx, races against polling loop via
  `tokio::select!`. Subscription cancels cooperatively via
  `CancellationToken` on guard drop (with `abort()` as backstop).
- Opt-in CAPTCHA solver callback via `.on_captcha(...)` — extracts
  site key from the page (`data-sitekey` attrs, falls back to
  `___grecaptcha_cfg` walk, iframe `&k=` param), hands a
  `CaptchaChallenge { kind, site_key, url }` to the caller's async
  solver, injects the returned `CaptchaSolution { token, form_field }`
  into the named form field (escaping both `\` and `"`), resumes
  polling. Without a callback, CAPTCHA surfaces return
  `ImpervaError::CaptchaRequired` immediately.
- Stalled-poll telemetry: `wait_for_clearance` emits one
  `tracing::warn!` after ~2.5s of stalled clearance, suggesting
  `BrowserBuilder::stealth`.
- 25 unit tests + 5 doctests. Nightly integration smoke test gated by
  `imperva-tests` feature (env-var-driven; `IMPERVA_TEST_URL`).
