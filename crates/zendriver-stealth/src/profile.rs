//! Profile types: ProfileKind enum, Platform enum, PerFieldOverride struct,
//! plus the StealthProfile builder.

use std::path::{Path, PathBuf};

use crate::error::StealthError;
use crate::fingerprint::{Fingerprint, UserAgentMetadata};
use crate::persona::specs::ScreenSpec;

/// Stealth modes shipped by the library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileKind {
    /// No stealth applied. Browser is launched stock; no JS patches, no UA scrub.
    Off,
    /// Launch flags + UA scrub (HeadlessChrome → Chrome). No JS bootstrap.
    /// Safe against `Function.prototype.toString` detection. Default.
    Native,
    /// Native + Navigator-prototype JS patches. Passes sannysoft. Detectable
    /// by sophisticated bots that probe `toString` on Navigator getters.
    Spoofed,
}

/// JS `navigator.platform` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Platform {
    Win32,
    MacIntel,
    LinuxX86_64,
}

impl Platform {
    /// Map to the `navigator.platform` string Chrome reports for that OS.
    #[must_use]
    pub fn js_string(self) -> &'static str {
        match self {
            Platform::Win32 => "Win32",
            Platform::MacIntel => "MacIntel",
            Platform::LinuxX86_64 => "Linux x86_64",
        }
    }

    /// CDP `userAgentMetadata.platform` value (no version).
    #[must_use]
    pub fn ch_platform(self) -> &'static str {
        match self {
            Platform::Win32 => "Windows",
            Platform::MacIntel => "macOS",
            Platform::LinuxX86_64 => "Linux",
        }
    }

    /// UA-string OS token (the bit inside parentheses).
    #[must_use]
    pub fn ua_token(self) -> &'static str {
        match self {
            Platform::Win32 => "Windows NT 10.0; Win64; x64",
            Platform::MacIntel => "Macintosh; Intel Mac OS X 10_15_7",
            Platform::LinuxX86_64 => "X11; Linux x86_64",
        }
    }
}

#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub(crate) struct PerFieldOverride {
    pub memory_gb: Option<u32>,
    pub cpu_count: Option<u32>,
    pub chrome_major: Option<u32>,
    pub platform: Option<Platform>,
    pub timezone: Option<String>,
    pub locale: Option<String>,
    pub languages: Option<Vec<String>>,
    pub ua_string: Option<String>,
    pub ua_metadata: Option<UserAgentMetadata>,
    pub screen: Option<ScreenSpec>,
}

/// Stealth configuration passed to `BrowserBuilder::stealth(...)`.
#[derive(Debug, Clone)]
pub struct StealthProfile {
    pub(crate) kind: ProfileKind,
    pub(crate) extra_flags: Vec<String>,
    pub(crate) fingerprint_override: Option<Fingerprint>,
    pub(crate) per_field: PerFieldOverride,
    pub(crate) bypass_csp: bool,
    pub(crate) native_isolation: bool,
    /// Skip the WebGL vendor/renderer identity patch (and the coupled WebGPU
    /// value/fabrication spoof) for `spoofed`, independent of the real
    /// render-process site-isolation launch flags. See
    /// [`native_webgl`](Self::native_webgl) for the caller-facing setter.
    pub(crate) native_webgl: bool,
    // Wired by `BrowserBuilder::stealth` in Task 17.
    #[allow(dead_code)]
    pub(crate) user_data_dir: Option<PathBuf>,
}

impl StealthProfile {
    /// No stealth: stock browser launch.
    ///
    /// Use this when you want a bare-bones Chrome with none of the launch
    /// flags or CDP overrides applied — e.g. when verifying that a problem
    /// reproduces in vanilla Chrome.
    ///
    /// ```
    /// use zendriver_stealth::StealthProfile;
    /// let p = StealthProfile::off();
    /// assert!(p.build_flags().is_empty());
    /// ```
    #[must_use]
    pub fn off() -> Self {
        Self {
            kind: ProfileKind::Off,
            extra_flags: Vec::new(),
            fingerprint_override: None,
            per_field: PerFieldOverride::default(),
            bypass_csp: false,
            native_isolation: false,
            native_webgl: false,
            user_data_dir: None,
        }
    }

    /// Launch flags + UA scrub + Emulation overrides. No JS bootstrap.
    ///
    /// Safe against `Function.prototype.toString` detection (it doesn't
    /// patch any prototype getter). The default when stealth is requested.
    ///
    /// ```
    /// use zendriver_stealth::StealthProfile;
    /// let p = StealthProfile::native();
    /// assert!(!p.build_flags().is_empty());
    /// ```
    #[must_use]
    pub fn native() -> Self {
        Self {
            kind: ProfileKind::Native,
            extra_flags: Vec::new(),
            fingerprint_override: None,
            per_field: PerFieldOverride::default(),
            bypass_csp: false,
            native_isolation: false,
            native_webgl: false,
            user_data_dir: None,
        }
    }

    /// `native` + Navigator-prototype JS patches. Passes the sannysoft
    /// detection battery.
    ///
    /// Sets [`bypass_csp`](Self::bypass_csp) on by default so the bootstrap
    /// script can install on pages with strict CSP headers.
    ///
    /// ```
    /// use zendriver_stealth::StealthProfile;
    /// let p = StealthProfile::spoofed();
    /// assert!(p.bypass_csp_enabled());
    /// ```
    #[must_use]
    pub fn spoofed() -> Self {
        Self {
            kind: ProfileKind::Spoofed,
            extra_flags: Vec::new(),
            fingerprint_override: None,
            per_field: PerFieldOverride::default(),
            bypass_csp: true, // default ON for spoofed; see spec assumption #2
            native_isolation: false,
            native_webgl: false,
            user_data_dir: None,
        }
    }

    /// Override the auto-detected [`Fingerprint`] wholesale.
    ///
    /// Use this when you need to pin a specific Chrome version / platform /
    /// hardware combination across runs (e.g. to keep request fingerprints
    /// stable across CI invocations).
    #[must_use]
    pub fn fingerprint(mut self, f: Fingerprint) -> Self {
        self.fingerprint_override = Some(f);
        self
    }
    /// Override the reported `navigator.deviceMemory` (in GB).
    ///
    /// Per the [HTML Device Memory spec][spec] the value Chrome reports
    /// is one of `{1, 2, 4, 8}` (Chrome does not expose fractional values
    /// to JS). The resolver snaps `gb` to the nearest valid integer at
    /// or below it:
    ///
    /// | Input `gb` | Reported |
    /// |-----------:|---------:|
    /// | `0` or `1` | `1`      |
    /// | `2` or `3` | `2`      |
    /// | `4`–`7`    | `4`      |
    /// | `8`+       | `8`      |
    ///
    /// Snap-down (not nearest-round) matches Chrome's own behavior and
    /// avoids inflating the reported value above what the host plausibly
    /// has. Anything outside `{1, 2, 4, 8}` is itself a stealth tell
    /// because real browsers never report it.
    ///
    /// [spec]: https://www.w3.org/TR/device-memory/
    #[must_use]
    pub fn memory_gb(mut self, gb: u32) -> Self {
        self.per_field.memory_gb = Some(gb);
        self
    }
    /// Override the reported `navigator.hardwareConcurrency` (CPU count).
    ///
    /// Clamped to `2..=32` at resolve time — values outside that range are
    /// implausibly low/high and trip simple heuristics.
    #[must_use]
    pub fn cpu_count(mut self, n: u32) -> Self {
        self.per_field.cpu_count = Some(n);
        self
    }
    /// Override the reported Chrome major version (e.g. `125`).
    #[must_use]
    pub fn chrome_version(mut self, major: u32) -> Self {
        self.per_field.chrome_major = Some(major);
        self
    }
    /// Override the reported [`Platform`] (`navigator.platform` + UA OS
    /// token + UAM `platform`).
    #[must_use]
    pub fn platform(mut self, p: Platform) -> Self {
        self.per_field.platform = Some(p);
        self
    }
    /// Override the reported locale (e.g. `"en-US"`, `"fr-FR"`).
    ///
    /// Sends `Emulation.setLocaleOverride` and adds `--lang=...` to the
    /// launch flags.
    #[must_use]
    pub fn locale(mut self, l: impl Into<String>) -> Self {
        self.per_field.locale = Some(l.into());
        self
    }
    /// Override the reported language list (drives `navigator.languages` +
    /// q-weighted `Accept-Language`). When unset, derived from
    /// [`locale`](Self::locale).
    #[must_use]
    pub fn languages(mut self, langs: impl IntoIterator<Item = String>) -> Self {
        self.per_field.languages = Some(langs.into_iter().collect());
        self
    }
    /// Override the reported timezone via `Emulation.setTimezoneOverride`
    /// (IANA name, e.g. `"America/Los_Angeles"`).
    #[must_use]
    pub fn timezone(mut self, tz: impl Into<String>) -> Self {
        self.per_field.timezone = Some(tz.into());
        self
    }
    /// Override the reported User-Agent string verbatim.
    ///
    /// Skips the composed-from-fingerprint step — prefer
    /// [`platform`](Self::platform) + [`chrome_version`](Self::chrome_version)
    /// unless you need an exact UA string.
    #[must_use]
    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.per_field.ua_string = Some(ua.into());
        self
    }
    /// Override the reported UA-CH (`userAgentMetadata`) wholesale.
    ///
    /// Skips the composed-from-fingerprint step entirely — the resolved
    /// [`Fingerprint::ua_metadata`] equals exactly what's passed here.
    /// Prefer this over hand-composing individual UA-CH fields when you
    /// need an exact, pre-built [`UserAgentMetadata`] (e.g. captured from a
    /// real browser). For persona-driven, field-wise UA-CH overrides that
    /// fall back to the derived value per sub-field, use a
    /// [`Persona`](crate::Persona)'s `ua.ua_metadata`
    /// ([`UaMetadata`](crate::UaMetadata)) instead, threaded via
    /// [`StealthObserver::with_persona`](crate::StealthObserver::with_persona).
    #[must_use]
    pub fn user_agent_metadata(mut self, m: UserAgentMetadata) -> Self {
        self.per_field.ua_metadata = Some(m);
        self
    }
    /// Override the reported screen / device-metrics
    /// (`Emulation.setDeviceMetricsOverride`) wholesale.
    ///
    /// Replaces the observer's fixed 1920x1080 default. Like
    /// [`user_agent_metadata`](Self::user_agent_metadata), this is a
    /// profile-level pin; a [`Persona`](crate::Persona)'s `screen` field
    /// (threaded via
    /// [`StealthObserver::with_persona`](crate::StealthObserver::with_persona))
    /// takes precedence when both are set.
    #[must_use]
    pub fn screen(mut self, s: ScreenSpec) -> Self {
        self.per_field.screen = Some(s);
        self
    }
    /// Toggle `Page.setBypassCSP`. Default `true` for [`spoofed`](Self::spoofed),
    /// `false` for [`native`](Self::native) / [`off`](Self::off).
    #[must_use]
    pub fn bypass_csp(mut self, on: bool) -> Self {
        self.bypass_csp = on;
        self
    }
    /// Opt in to Chrome's **real** render-process site isolation:
    /// `IsolateOrigins`/`site-per-process` stay enabled — the launch flags
    /// omit the isolation-disabling `--disable-features=...` entries the
    /// library applies by default. Affects **only** the launch flags.
    ///
    /// # This is a trade-off, not a strict improvement
    ///
    /// The default this opts out of exists for a reason: disabling site
    /// isolation keeps every frame in one render process, which makes CDP
    /// target attachment simpler. Turning real isolation back on is useful
    /// when you want Chrome's stock process-isolation security boundary for a
    /// test harness. It is orthogonal to stealth — it changes no JS
    /// fingerprint surface.
    ///
    /// To skip the WebGL/WebGPU identity patch — a separate stealth axis this
    /// setter used to bundle in — use [`native_webgl`](Self::native_webgl).
    /// Set both to reproduce the pre-split `native_isolation(true)` behavior.
    ///
    /// Off by default. [`native`](Self::native) and [`spoofed`](Self::spoofed)
    /// are unaffected unless you call this explicitly, so existing callers
    /// see no behavior change.
    ///
    /// ```
    /// use zendriver_stealth::StealthProfile;
    /// let p = StealthProfile::spoofed().native_isolation(true);
    /// assert!(!p.build_flags().iter().any(|f| f.contains("IsolateOrigins")));
    /// assert!(p.native_isolation_enabled());
    /// ```
    #[must_use]
    pub fn native_isolation(mut self, on: bool) -> Self {
        self.native_isolation = on;
        self
    }

    /// Skip the WebGL vendor/renderer value-substitution patch (and the
    /// coupled WebGPU value/fabrication spoof, kept coherent with it) for the
    /// [`spoofed`](Self::spoofed) profile — the host's real
    /// `WebGLRenderingContext.getParameter`/`getSupportedExtensions` (and
    /// `navigator.gpu`) values pass through unpatched instead of reporting the
    /// coherent ANGLE/Direct3D11 Intel identity `patches/webgl.js` spoofs by
    /// default.
    ///
    /// # This is a trade-off, not a strict improvement
    ///
    /// The WebGL patch is itself an anti-WAF *coherence* defense — some WAFs
    /// (Imperva/Incapsula) cross-check the WebGL identity against other
    /// signals and flag a real, host-specific renderer string as a bot tell
    /// when it doesn't match the rest of the spoofed fingerprint. Turning this
    /// on trades that coherence-with-a-fake-identity for coherence-with-the-
    /// real-host: useful when you need the host's actual GPU behavior
    /// (WebGL-heavy rendering/testing, screenshot fidelity) and evasion is not
    /// the priority. It is **not** "more stealthy" than the default — it
    /// removes an anti-detection defense.
    ///
    /// WebGL/WebGPU stay coherent: when this drops the WebGL patch, the
    /// separate WebGPU **value** adapter spoof (driven by the
    /// [`Persona`](crate::Persona) `webgpu` surface, `Value` by default) is
    /// skipped too, so `navigator.gpu` reports the real host adapter rather
    /// than one derived from a renderer the WebGL patch no longer applies. An
    /// explicit WebGPU [`Strategy::Block`](crate::Strategy) (hiding
    /// `navigator.gpu`) is renderer-neutral and is still honored.
    ///
    /// Independent of [`native_isolation`](Self::native_isolation), which only
    /// changes Chrome's launch flags. Set both to reproduce the pre-split
    /// `native_isolation(true)` bundle.
    ///
    /// Off by default — existing callers see no behavior change.
    ///
    /// ```
    /// use zendriver_stealth::StealthProfile;
    /// let p = StealthProfile::spoofed().native_webgl(true);
    /// // Axis 2 only: the launch flags are unchanged from a plain spoofed
    /// // profile — real site isolation stays disabled by default.
    /// assert!(p.build_flags().iter().any(|f| f.contains("IsolateOrigins")));
    /// assert!(p.native_webgl_enabled());
    /// ```
    #[must_use]
    pub fn native_webgl(mut self, on: bool) -> Self {
        self.native_webgl = on;
        self
    }
    /// Add a single extra Chrome launch flag (e.g. `"--proxy-server=..."`).
    #[must_use]
    pub fn arg(mut self, flag: impl Into<String>) -> Self {
        self.extra_flags.push(flag.into());
        self
    }
    /// Add a batch of Chrome launch flags.
    #[must_use]
    pub fn args(mut self, flags: impl IntoIterator<Item = String>) -> Self {
        self.extra_flags.extend(flags);
        self
    }

    // Consumed by `StealthObserver` in Task 13.
    #[allow(dead_code)]
    pub(crate) fn kind(&self) -> ProfileKind {
        self.kind
    }

    /// Resolve the final [`Fingerprint`]: either the explicit override or an
    /// auto-detected baseline, with per-field tweaks (`platform`, `locale`,
    /// `memory_gb`, …) applied on top.
    ///
    /// `chrome_exe` is probed for its Chrome major: by reading the binary's PE
    /// version resource on Windows, and by running `--version` on Unix (Windows
    /// Chrome answers `--version` by launching a browser that never exits, so it
    /// must not be executed there). Either probe failing falls back to a
    /// baked-in default, so the resolver never errors solely on Chrome being
    /// unavailable.
    ///
    /// # Errors
    /// Returns [`StealthError::ChromeVersionDetect`] when the Chrome probe
    /// fails *and* no override is provided, and [`StealthError::SystemInfo`]
    /// when total-RAM detection fails.
    // `StealthError` is large because `PatchFailed` wraps `CallError` (~152B).
    // Boxing it would cross the Task 5 file scope; bypass per-fn instead.
    #[allow(clippy::result_large_err)]
    pub fn resolve_fingerprint(&self, chrome_exe: &Path) -> Result<Fingerprint, StealthError> {
        let mut fp = match &self.fingerprint_override {
            Some(fp) => fp.clone(),
            None => Fingerprint::auto_detect(chrome_exe)?,
        };
        // The Accept-Encoding the *real* binary advertises, captured before the
        // claimed-major override below (used only to warn on an uncorrectable skew).
        let binary_major = fp.chrome_major;
        if let Some(p) = self.per_field.platform {
            fp.platform = p;
        }
        if let Some(c) = self.per_field.chrome_major {
            fp.chrome_major = c;
            fp.chrome_full = format!("{c}.0.6099.234"); // synthesize a full version if user only set major
        }
        if let Some(n) = self.per_field.cpu_count {
            fp.cpu_count = n.clamp(2, 32);
        }
        if let Some(g) = self.per_field.memory_gb {
            // Snap-down to the nearest spec-valid value Chrome exposes
            // to JS — see `memory_gb` doc table.
            let snapped = match g {
                0..=1 => 1,
                2..=3 => 2,
                4..=7 => 4,
                _ => 8,
            };
            if snapped != g {
                tracing::debug!(
                    requested = g,
                    chosen = snapped,
                    "stealth memory_gb snapped to nearest navigator.deviceMemory spec value",
                );
            }
            fp.memory_gb = snapped;
        }
        // Always recompose so `ua_metadata.{platform_version, architecture,
        // bitness}` track any `platform` / `chrome_major` overrides applied
        // above. Then, if the user supplied an explicit UA string, replace
        // the freshly composed `ua_string` with it (UAM remains coherent
        // with the overridden platform).
        fp.recompose();
        if let Some(ref ua) = self.per_field.ua_string {
            fp.ua_string = ua.clone();
        }
        // Wholesale UA-CH pin: mirrors the `ua_string` override immediately
        // above — when set, it REPLACES the recompose()-derived UAM outright
        // (no field-wise fallback; that's what `Persona`'s `ua_metadata` is
        // for, resolved in the observer against this fingerprint instead).
        if let Some(ref uam) = self.per_field.ua_metadata {
            fp.ua_metadata = uam.clone();
        }
        // Screen has no "derived" baseline (unlike UA-CH) — `Fingerprint`
        // never had a screen concept before this, so leaving it `None` here
        // is exactly today's behavior (the observer's fixed 1920x1080).
        if let Some(s) = self.per_field.screen {
            fp.screen = Some(s);
        }
        if let Some(ref tz) = self.per_field.timezone {
            fp.timezone = Some(tz.clone());
        }
        if let Some(ref locale) = self.per_field.locale {
            fp.locale = Some(locale.clone());
        }
        if let Some(ref langs) = self.per_field.languages {
            fp.languages = Some(langs.clone());
        }
        // Accept-Encoding coherence is observable but NOT correctable over CDP:
        // Chrome's network service owns the header and ignores
        // `Network.setExtraHTTPHeaders` for it (verified, Chrome 148). When a
        // pinned `chrome_version` straddles the `zstd`/Chrome-123 boundary vs the
        // launched binary, the request advertises the binary's encodings — an
        // Accept-Encoding vs User-Agent mismatch we can only warn about.
        if let Some(coherent) = crate::headers::accept_encoding_skew(binary_major, fp.chrome_major)
        {
            tracing::warn!(
                claimed_major = fp.chrome_major,
                binary_major,
                coherent_accept_encoding = coherent,
                "stealth: claimed Chrome major advertises a different Accept-Encoding \
                 than the launched binary; Chrome controls this header and CDP cannot \
                 override it, so requests will leak the binary's encodings — pin \
                 chrome_version to the binary's major to stay coherent",
            );
        }
        Ok(fp)
    }

    /// Composed Chrome launch flag list: per-profile defaults plus any
    /// extras added via [`arg`](Self::arg) / [`args`](Self::args), with a
    /// `--lang=<locale>` flag injected when [`locale`](Self::locale) is set.
    ///
    /// ```
    /// use zendriver_stealth::StealthProfile;
    /// let flags = StealthProfile::native().locale("fr-FR").build_flags();
    /// assert!(flags.iter().any(|f| f == "--lang=fr-FR"));
    /// ```
    pub fn build_flags(&self) -> Vec<String> {
        let mut flags = crate::flags::flags_for_profile(self.kind, self.native_isolation);
        if let Some(ref locale) = self.per_field.locale {
            flags.push(format!("--lang={locale}"));
        }
        flags.extend(self.extra_flags.iter().cloned());
        flags
    }

    /// Whether `Page.setBypassCSP` will be sent for this profile. Defaults
    /// to `true` for [`spoofed`](Self::spoofed) and `false` otherwise; the
    /// [`bypass_csp`](Self::bypass_csp) setter toggles it explicitly.
    pub fn bypass_csp_enabled(&self) -> bool {
        self.bypass_csp
    }

    /// Whether the [`native_isolation`](Self::native_isolation) opt-in is
    /// active for this profile. `false` unless explicitly set.
    #[must_use]
    pub fn native_isolation_enabled(&self) -> bool {
        self.native_isolation
    }

    /// Whether the [`native_webgl`](Self::native_webgl) opt-in is active for
    /// this profile. `false` unless explicitly set.
    #[must_use]
    pub fn native_webgl_enabled(&self) -> bool {
        self.native_webgl
    }

    /// Returns the input-realism profile appropriate for this stealth profile.
    /// `spoofed` returns realistic timings; `native` and `off` return zero-overhead.
    #[must_use]
    pub fn input_profile(&self) -> crate::InputProfile {
        match self.kind {
            ProfileKind::Spoofed => crate::InputProfile::spoofed(),
            ProfileKind::Native | ProfileKind::Off => crate::InputProfile::native(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn platform_js_string_matches_chrome_output() {
        assert_eq!(Platform::Win32.js_string(), "Win32");
        assert_eq!(Platform::MacIntel.js_string(), "MacIntel");
        assert_eq!(Platform::LinuxX86_64.js_string(), "Linux x86_64");
    }

    #[test]
    fn platform_ch_platform_uses_no_version() {
        assert_eq!(Platform::MacIntel.ch_platform(), "macOS");
    }

    #[test]
    fn platform_ua_token_includes_arch() {
        assert!(Platform::Win32.ua_token().contains("Win64; x64"));
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod profile_tests {
    use super::*;
    use crate::fingerprint::UserAgentMetadata;

    #[test]
    fn off_profile_has_no_flags_no_patches() {
        let p = StealthProfile::off();
        assert_eq!(p.kind, ProfileKind::Off);
        assert!(p.build_flags().is_empty());
    }

    #[test]
    fn native_profile_has_flags_no_patches() {
        let p = StealthProfile::native();
        assert_eq!(p.kind, ProfileKind::Native);
        assert!(!p.build_flags().is_empty());
    }

    #[test]
    fn spoofed_profile_default_bypass_csp_on() {
        let p = StealthProfile::spoofed();
        assert!(p.bypass_csp_enabled());
    }

    #[test]
    fn native_isolation_off_by_default_on_every_profile() {
        assert!(!StealthProfile::off().native_isolation_enabled());
        assert!(!StealthProfile::native().native_isolation_enabled());
        assert!(!StealthProfile::spoofed().native_isolation_enabled());
    }

    #[test]
    fn native_isolation_toggle_sets_flag() {
        let p = StealthProfile::spoofed().native_isolation(true);
        assert!(p.native_isolation_enabled());
    }

    #[test]
    fn native_isolation_build_flags_omit_isolate_origins() {
        let flags = StealthProfile::native()
            .native_isolation(true)
            .build_flags();
        assert!(!flags.iter().any(|f| f.contains("IsolateOrigins")));
    }

    #[test]
    fn default_native_build_flags_still_disable_isolation_regression_guard() {
        // Existing default (no opt-in) must be unchanged.
        let flags = StealthProfile::native().build_flags();
        assert!(flags.iter().any(|f| f.contains("IsolateOrigins")));
    }

    #[test]
    fn native_webgl_off_by_default_on_every_profile() {
        assert!(!StealthProfile::off().native_webgl_enabled());
        assert!(!StealthProfile::native().native_webgl_enabled());
        assert!(!StealthProfile::spoofed().native_webgl_enabled());
    }

    #[test]
    fn native_webgl_toggle_sets_flag() {
        let p = StealthProfile::spoofed().native_webgl(true);
        assert!(p.native_webgl_enabled());
        // The WebGL axis is independent — it must not flip the isolation axis.
        assert!(!p.native_isolation_enabled());
    }

    #[test]
    fn native_webgl_true_native_isolation_false_build_flags_unaffected() {
        // The split is real, not cosmetic: toggling axis 2 (WebGL patch skip)
        // alone must leave the launch-flag list (axis 1) byte-for-byte equal to
        // a plain spoofed profile.
        let baseline = StealthProfile::spoofed().build_flags();
        let webgl_only = StealthProfile::spoofed().native_webgl(true).build_flags();
        assert_eq!(baseline, webgl_only);
    }

    #[test]
    fn native_isolation_true_native_webgl_false_build_flags_omit_isolate_origins() {
        // Axis 1 alone still drops the isolation-disabling features from the
        // launch flags, matching the pre-split behavior.
        let flags = StealthProfile::spoofed()
            .native_isolation(true)
            .build_flags();
        assert!(!flags.iter().any(|f| f.contains("IsolateOrigins")));
        assert!(!flags.iter().any(|f| f.contains("site-per-process")));
    }

    #[test]
    fn builder_chains_set_fields() {
        let p = StealthProfile::spoofed()
            .memory_gb(16)
            .cpu_count(10)
            .chrome_version(125)
            .platform(Platform::MacIntel)
            .locale("en-US")
            .timezone("America/Los_Angeles")
            .arg("--proxy-server=http://x");
        assert_eq!(p.per_field.memory_gb, Some(16));
        assert_eq!(p.per_field.cpu_count, Some(10));
        assert_eq!(p.per_field.chrome_major, Some(125));
        assert_eq!(p.per_field.platform, Some(Platform::MacIntel));
        assert_eq!(p.per_field.locale.as_deref(), Some("en-US"));
        assert_eq!(p.per_field.timezone.as_deref(), Some("America/Los_Angeles"));
        assert!(
            p.extra_flags
                .contains(&"--proxy-server=http://x".to_string())
        );
    }

    #[test]
    fn build_flags_includes_locale_arg_when_set() {
        let flags = StealthProfile::native().locale("fr-FR").build_flags();
        assert!(flags.iter().any(|f| f == "--lang=fr-FR"));
    }

    #[test]
    fn spoofed_profile_uses_spoofed_input_profile() {
        let ip = StealthProfile::spoofed().input_profile();
        assert!(ip.typo_rate > 0.0);
    }

    #[test]
    fn native_profile_uses_native_input_profile() {
        let ip = StealthProfile::native().input_profile();
        assert_eq!(ip.typo_rate, 0.0);
    }

    #[test]
    fn off_profile_uses_native_input_profile() {
        let ip = StealthProfile::off().input_profile();
        assert_eq!(ip.typo_rate, 0.0);
    }

    #[test]
    fn resolve_fingerprint_with_explicit_override_skips_autodetect() {
        let fp = Fingerprint {
            platform: Platform::Win32,
            chrome_major: 120,
            chrome_full: "120.0.6099.234".into(),
            cpu_count: 8,
            memory_gb: 8,
            ua_string: String::new(),
            ua_metadata: UserAgentMetadata::realistic(Platform::Win32, 120, "120.0.6099.234"),
            timezone: None,
            locale: None,
            languages: None,
            screen: None,
        };
        let p = StealthProfile::native()
            .fingerprint(fp.clone())
            .platform(Platform::MacIntel);
        // Pass a path that doesn't exist; if it tried to probe, it'd fail.
        let resolved = p
            .resolve_fingerprint(std::path::Path::new("/nonexistent"))
            .unwrap();
        assert_eq!(resolved.platform, Platform::MacIntel); // per-field override applied
    }

    #[test]
    fn profile_languages_resolve_into_fingerprint() {
        let profile = StealthProfile::native().languages(["fr-FR".into(), "fr".into()]);
        let fp = Fingerprint {
            platform: Platform::Win32,
            chrome_major: 120,
            chrome_full: "120.0.6099.234".into(),
            cpu_count: 8,
            memory_gb: 8,
            ua_string: String::new(),
            ua_metadata: UserAgentMetadata::realistic(Platform::Win32, 120, "120.0.6099.234"),
            timezone: None,
            locale: None,
            languages: None,
            screen: None,
        };
        let profile = profile.fingerprint(fp);
        let resolved = profile
            .resolve_fingerprint(std::path::Path::new("/nonexistent-chrome"))
            .expect("resolve ok");
        assert_eq!(resolved.languages.unwrap(), vec!["fr-FR", "fr"]);
    }

    fn bare_fp() -> Fingerprint {
        Fingerprint {
            platform: Platform::Win32,
            chrome_major: 120,
            chrome_full: "120.0.6099.234".into(),
            cpu_count: 8,
            memory_gb: 8,
            ua_string: String::new(),
            ua_metadata: UserAgentMetadata::realistic(Platform::Win32, 120, "120.0.6099.234"),
            timezone: None,
            locale: None,
            languages: None,
            screen: None,
        }
    }

    #[test]
    fn custom_ua_metadata_replaces_recompose_derived_when_supplied() {
        let custom = UserAgentMetadata::realistic(Platform::MacIntel, 150, "150.0.7500.0");
        let profile = StealthProfile::native()
            .fingerprint(bare_fp())
            .user_agent_metadata(custom.clone());
        let resolved = profile
            .resolve_fingerprint(std::path::Path::new("/nonexistent"))
            .unwrap();
        // Equals the SUPPLIED value exactly, not the recompose()-derived one
        // (which would be `realistic(Win32, 120, ...)` from `bare_fp()`).
        assert_eq!(resolved.ua_metadata, custom);
    }

    #[test]
    fn absent_ua_metadata_override_keeps_recompose_derived_behavior() {
        let profile = StealthProfile::native().fingerprint(bare_fp());
        let resolved = profile
            .resolve_fingerprint(std::path::Path::new("/nonexistent"))
            .unwrap();
        // No override → today's behavior: recompose()-derived from platform
        // + chrome_major, i.e. matches `UserAgentMetadata::realistic` for the
        // fingerprint's own (unoverridden) platform/version.
        assert_eq!(
            resolved.ua_metadata,
            UserAgentMetadata::realistic(Platform::Win32, 120, "120.0.6099.234")
        );
    }

    #[test]
    fn custom_screen_resolves_onto_fingerprint_when_supplied() {
        let screen = ScreenSpec {
            width: 1536,
            height: 864,
            device_pixel_ratio: 1.25,
        };
        let profile = StealthProfile::native()
            .fingerprint(bare_fp())
            .screen(screen);
        let resolved = profile
            .resolve_fingerprint(std::path::Path::new("/nonexistent"))
            .unwrap();
        assert_eq!(resolved.screen, Some(screen));
    }

    #[test]
    fn absent_screen_override_leaves_fingerprint_screen_none() {
        let profile = StealthProfile::native().fingerprint(bare_fp());
        let resolved = profile
            .resolve_fingerprint(std::path::Path::new("/nonexistent"))
            .unwrap();
        // No override → today's behavior: no screen field influence at all.
        assert_eq!(resolved.screen, None);
    }
}
