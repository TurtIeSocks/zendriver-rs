//! Profile types: ProfileKind enum, Platform enum, PerFieldOverride struct,
//! plus the StealthProfile builder.

use std::path::{Path, PathBuf};

use crate::error::StealthError;
use crate::fingerprint::Fingerprint;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
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
    pub ua_string: Option<String>,
}

/// Stealth configuration passed to `BrowserBuilder::stealth(...)`.
#[derive(Debug, Clone)]
pub struct StealthProfile {
    pub(crate) kind: ProfileKind,
    pub(crate) extra_flags: Vec<String>,
    pub(crate) fingerprint_override: Option<Fingerprint>,
    pub(crate) per_field: PerFieldOverride,
    pub(crate) bypass_csp: bool,
    // Wired by `BrowserBuilder::stealth` in Task 17.
    #[allow(dead_code)]
    pub(crate) user_data_dir: Option<PathBuf>,
}

impl StealthProfile {
    /// No stealth: stock browser launch.
    #[must_use]
    pub fn off() -> Self {
        Self {
            kind: ProfileKind::Off,
            extra_flags: Vec::new(),
            fingerprint_override: None,
            per_field: PerFieldOverride::default(),
            bypass_csp: false,
            user_data_dir: None,
        }
    }

    /// Launch flags + UA scrub + Emulation overrides. No JS bootstrap.
    /// Safe against `Function.prototype.toString` detection. Default.
    #[must_use]
    pub fn native() -> Self {
        Self {
            kind: ProfileKind::Native,
            extra_flags: Vec::new(),
            fingerprint_override: None,
            per_field: PerFieldOverride::default(),
            bypass_csp: false,
            user_data_dir: None,
        }
    }

    /// Native + Navigator-prototype JS patches. Passes sannysoft.
    #[must_use]
    pub fn spoofed() -> Self {
        Self {
            kind: ProfileKind::Spoofed,
            extra_flags: Vec::new(),
            fingerprint_override: None,
            per_field: PerFieldOverride::default(),
            bypass_csp: true, // default ON for spoofed; see spec assumption #2
            user_data_dir: None,
        }
    }

    #[must_use]
    pub fn fingerprint(mut self, f: Fingerprint) -> Self {
        self.fingerprint_override = Some(f);
        self
    }
    #[must_use]
    pub fn memory_gb(mut self, gb: u32) -> Self {
        self.per_field.memory_gb = Some(gb);
        self
    }
    #[must_use]
    pub fn cpu_count(mut self, n: u32) -> Self {
        self.per_field.cpu_count = Some(n);
        self
    }
    #[must_use]
    pub fn chrome_version(mut self, major: u32) -> Self {
        self.per_field.chrome_major = Some(major);
        self
    }
    #[must_use]
    pub fn platform(mut self, p: Platform) -> Self {
        self.per_field.platform = Some(p);
        self
    }
    #[must_use]
    pub fn locale(mut self, l: impl Into<String>) -> Self {
        self.per_field.locale = Some(l.into());
        self
    }
    #[must_use]
    pub fn timezone(mut self, tz: impl Into<String>) -> Self {
        self.per_field.timezone = Some(tz.into());
        self
    }
    #[must_use]
    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.per_field.ua_string = Some(ua.into());
        self
    }
    #[must_use]
    pub fn bypass_csp(mut self, on: bool) -> Self {
        self.bypass_csp = on;
        self
    }
    #[must_use]
    pub fn arg(mut self, flag: impl Into<String>) -> Self {
        self.extra_flags.push(flag.into());
        self
    }
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

    /// Resolve final Fingerprint: explicit override or auto-detect, with
    /// per-field tweaks applied on top.
    // `StealthError` is large because `PatchFailed` wraps `CallError` (~152B).
    // Boxing it would cross the Task 5 file scope; bypass per-fn instead.
    #[allow(clippy::result_large_err)]
    pub fn resolve_fingerprint(&self, chrome_exe: &Path) -> Result<Fingerprint, StealthError> {
        let mut fp = match &self.fingerprint_override {
            Some(fp) => fp.clone(),
            None => Fingerprint::auto_detect(chrome_exe)?,
        };
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
            fp.memory_gb = if g >= 8 { 8 } else { 4 };
        }
        if let Some(ref ua) = self.per_field.ua_string {
            fp.ua_string = ua.clone();
        } else {
            fp.recompose();
        }
        if let Some(ref tz) = self.per_field.timezone {
            fp.timezone = Some(tz.clone());
        }
        if let Some(ref locale) = self.per_field.locale {
            fp.locale = Some(locale.clone());
        }
        Ok(fp)
    }

    /// Composed launch flag list: per-profile defaults + extras.
    pub fn build_flags(&self) -> Vec<String> {
        let mut flags = crate::flags::flags_for_profile(self.kind);
        if let Some(ref locale) = self.per_field.locale {
            flags.push(format!("--lang={locale}"));
        }
        flags.extend(self.extra_flags.iter().cloned());
        flags
    }

    pub fn bypass_csp_enabled(&self) -> bool {
        self.bypass_csp
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
        assert!(p.extra_flags.contains(&"--proxy-server=http://x".to_string()));
    }

    #[test]
    fn build_flags_includes_locale_arg_when_set() {
        let flags = StealthProfile::native().locale("fr-FR").build_flags();
        assert!(flags.iter().any(|f| f == "--lang=fr-FR"));
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
}
