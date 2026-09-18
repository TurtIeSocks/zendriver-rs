//! Persona: the unified fingerprint configuration.

pub mod seed;
pub mod specs;
pub mod surface;

pub use seed::Seed;
pub use specs::{
    FontSpec, HardwareSpec, ScreenSpec, SurfaceCfg, UaMetadata, UaSpec, WebglSpec, WebgpuSpec,
    WebrtcSpec,
};

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::{GpuProfile, Platform};

static SYSTEM: OnceLock<Persona> = OnceLock::new();

/// Mock geolocation coordinates, sent verbatim as
/// `Emulation.setGeolocationOverride` params. Field names match the CDP
/// params exactly (`latitude`, `longitude`, `accuracy`) so `GeoPos` can be
/// deserialized straight out of `persona_overlay` JSON, e.g.
/// `{"geolocation":{"latitude":21.0285,"longitude":105.8542}}`.
///
/// `accuracy` is optional — CDP accepts the override without it — and is
/// omitted from the CDP call entirely (not sent as `null`) when unset.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GeoPos {
    pub latitude: f64,
    pub longitude: f64,
    pub accuracy: Option<f64>,
}

/// Unified fingerprint configuration. Every field optional → overlay semantics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Persona {
    pub platform: Option<Platform>,
    pub ua: Option<UaSpec>,
    pub hardware_concurrency: Option<u32>,
    pub device_memory_gb: Option<u32>,
    /// IANA timezone name (e.g. `"Europe/Berlin"`). Drives
    /// `Emulation.setTimezoneOverride`, which is what `Intl` and `Date` read.
    /// When unset, the [`Fingerprint`](crate::Fingerprint)'s own timezone
    /// applies — from
    /// [`StealthProfile::timezone`](crate::StealthProfile::timezone) — and
    /// when neither is set no override is sent and Chrome keeps the host's.
    pub timezone: Option<String>,
    /// Locale tag (e.g. `"fr-FR"`). When [`languages`](Self::languages) is
    /// unset, drives both `Emulation.setLocaleOverride` and the
    /// `Accept-Language` header derived from it (`"fr-FR"` → `fr-FR,fr`);
    /// when the list is set, the list drives both instead. Same fallback as
    /// [`timezone`](Self::timezone): the fingerprint's locale, then Chrome's
    /// own.
    pub locale: Option<String>,
    /// Ordered language list (e.g. `["de-DE", "de"]`). Drives
    /// `navigator.languages`, the q-weighted `Accept-Language`, and
    /// `Emulation.setLocaleOverride` from its first entry. When unset, all
    /// three derive from [`locale`](Self::locale).
    ///
    /// The list outranks [`locale`](Self::locale) because a real Chrome
    /// always reports `navigator.language` as `navigator.languages[0]` — a
    /// locale outside the advertised list is a browser that cannot exist. An
    /// empty list counts as unset and inherits the fingerprint's, rather than
    /// erasing it.
    pub languages: Option<Vec<String>>,
    /// Mock coordinates for the Geolocation API (`Emulation.setGeolocationOverride`).
    /// Coherence axis alongside [`timezone`](Self::timezone) /
    /// [`locale`](Self::locale) — keeps `navigator.geolocation` in step with
    /// the exit IP's real location instead of leaking the host's.
    pub geolocation: Option<GeoPos>,
    /// Screen / device-metrics override (`Emulation.setDeviceMetricsOverride`
    /// plus the bootstrap's `outer*`/`avail*` geometry repair, which CDP
    /// cannot reach). Wins over
    /// [`StealthProfile::screen`](crate::StealthProfile::screen); `None` falls
    /// back to it, and to the fixed 1920x1080 default when neither is set.
    pub screen: Option<ScreenSpec>,
    pub webgl: Option<WebglSpec>,
    /// WebGPU adapter override — decorate a real adapter's `.info` (deriving
    /// vendor/architecture from [`webgl`](Self::webgl) when unset, the
    /// default), and optionally fabricate a synthetic adapter on a GPU-less
    /// host. **Opt-in only** — see [`WebgpuSpec`] for the full accuracy
    /// warning and v1 limitations before setting `limits`/`features`.
    pub webgpu: Option<WebgpuSpec>,
    /// One coherent GPU identity: every readable WebGL value plus the WebGPU
    /// adapter, resolved from the tier tables.
    ///
    /// `None` resolves from the persona's WebGL renderer via the device table.
    /// The finer-grained [`WebglSpec`](specs::WebglSpec) and
    /// [`WebgpuSpec`](specs::WebgpuSpec) still overlay on top of whatever this
    /// produces, so a caller can pin one value without restating a whole device.
    pub gpu: Option<GpuProfile>,
    pub canvas: Option<SurfaceCfg>,
    pub audio: Option<SurfaceCfg>,
    pub fonts: Option<FontSpec>,
    pub client_rects: Option<SurfaceCfg>,
    pub webrtc: Option<WebrtcSpec>,
    pub hardware: Option<HardwareSpec>,
    pub seed: Option<Seed>,
}

/// Fluent builder for [`Persona`]. Every setter is optional.
#[derive(Debug, Clone, Default)]
pub struct PersonaBuilder(Persona);

impl Persona {
    pub fn try_from_json(s: &str) -> Result<Persona, serde_json::Error> {
        serde_json::from_str(s)
    }

    pub fn builder() -> PersonaBuilder {
        PersonaBuilder(Persona::default())
    }
}

impl PersonaBuilder {
    pub fn seed(mut self, s: Seed) -> Self {
        self.0.seed = Some(s);
        self
    }
    pub fn timezone(mut self, tz: impl Into<String>) -> Self {
        self.0.timezone = Some(tz.into());
        self
    }
    pub fn locale(mut self, l: impl Into<String>) -> Self {
        self.0.locale = Some(l.into());
        self
    }
    pub fn languages(mut self, langs: impl IntoIterator<Item = String>) -> Self {
        self.0.languages = Some(langs.into_iter().collect());
        self
    }
    pub fn device_memory_gb(mut self, gb: u32) -> Self {
        self.0.device_memory_gb = Some(gb);
        self
    }
    pub fn hardware_concurrency(mut self, n: u32) -> Self {
        self.0.hardware_concurrency = Some(n);
        self
    }
    pub fn webgl(mut self, w: WebglSpec) -> Self {
        self.0.webgl = Some(w);
        self
    }
    /// Claim a catalogued GPU.
    ///
    /// Deliberately **not** a new [`Persona`] field. A device's whole
    /// contribution is its renderer string, and pinning that already selects
    /// the capability tier, the WebGPU adapter, and the vendor through
    /// machinery that exists — so this sets [`WebglSpec::unmasked_renderer`]
    /// and nothing else. A dedicated field would add a fourth precedence layer
    /// and a serde problem (a [`GpuDevice`] is a pointer into a generated
    /// table) to buy nothing.
    ///
    /// Overwrites only the renderer, so a caller can still pin the vendor or a
    /// strategy alongside it:
    ///
    /// ```no_run
    /// use zendriver_stealth::{GpuDevice, Persona, Platform};
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let persona = Persona {
    ///     platform: Some(Platform::Win32),
    ///     ..Persona::builder()
    ///         .gpu_device(GpuDevice::by_name("NVIDIA GeForce RTX 4090")?)
    ///         .build()
    /// };
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn gpu_device(mut self, device: crate::GpuDevice) -> Self {
        let mut spec = self.0.webgl.unwrap_or_default();
        spec.unmasked_renderer = Some(device.renderer());
        self.0.webgl = Some(spec);
        self
    }
    pub fn build(self) -> Persona {
        self.0
    }
}

impl std::str::FromStr for Persona {
    type Err = serde_json::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(s)
    }
}

impl Persona {
    /// Field-wise merge: `Some` in `over` wins, `None` inherits from `self`.
    pub fn overlay(self, over: Persona) -> Persona {
        Persona {
            platform: over.platform.or(self.platform),
            // UA-CH composes field-wise (via `UaSpec::overlay`) rather than
            // one side wholesale-replacing the other, so layering two
            // personas can patch e.g. just `platform_version` without
            // clobbering a base persona's `brands`.
            ua: match (self.ua, over.ua) {
                (Some(base), Some(add)) => Some(base.overlay(add)),
                (base, add) => add.or(base),
            },
            hardware_concurrency: over.hardware_concurrency.or(self.hardware_concurrency),
            device_memory_gb: over.device_memory_gb.or(self.device_memory_gb),
            timezone: over.timezone.or(self.timezone),
            locale: over.locale.or(self.locale),
            languages: over.languages.or(self.languages),
            geolocation: over.geolocation.or(self.geolocation),
            screen: over.screen.or(self.screen),
            webgl: over.webgl.or(self.webgl),
            webgpu: over.webgpu.or(self.webgpu),
            // Whole-value, not field-wise: a GPU is one coherent artifact
            // (same rule as `screen`). Merging two personas' GPUs field-wise
            // could compose a device that exists nowhere — Metal's texture
            // limits beside D3D11's viewport bound is exactly the
            // incoherence this branch exists to eliminate.
            gpu: over.gpu.or(self.gpu),
            canvas: over.canvas.or(self.canvas),
            audio: over.audio.or(self.audio),
            fonts: over.fonts.or(self.fonts),
            client_rects: over.client_rects.or(self.client_rects),
            webrtc: over.webrtc.or(self.webrtc),
            hardware: over.hardware.or(self.hardware),
            seed: over.seed.or(self.seed),
        }
    }

    /// Force a single surface's render [`Strategy`], creating the surface's
    /// spec with default values if it was unset.
    ///
    /// Used by the public browser builder's `.surface(surface, strategy)` to
    /// layer per-surface overrides on top of the resolved persona. Each surface
    /// writes the `strategy` field of its corresponding spec; the [`UaSpec`]
    /// surface family (identity, not a render strategy) has no strategy field,
    /// so this is a no-op there.
    pub fn apply_surface_override(
        &mut self,
        surface: surface::Surface,
        strategy: surface::Strategy,
    ) {
        use surface::Surface;
        match surface {
            Surface::Canvas => {
                self.canvas.get_or_insert_with(Default::default).strategy = Some(strategy)
            }
            Surface::Audio => {
                self.audio.get_or_insert_with(Default::default).strategy = Some(strategy)
            }
            Surface::ClientRects => {
                self.client_rects
                    .get_or_insert_with(Default::default)
                    .strategy = Some(strategy)
            }
            Surface::Webgl => {
                self.webgl.get_or_insert_with(Default::default).strategy = Some(strategy)
            }
            Surface::Fonts => {
                self.fonts.get_or_insert_with(Default::default).strategy = Some(strategy)
            }
            Surface::Webrtc => {
                self.webrtc.get_or_insert_with(Default::default).strategy = Some(strategy)
            }
            Surface::Hardware => {
                self.hardware.get_or_insert_with(Default::default).strategy = Some(strategy)
            }
            Surface::Webgpu => {
                self.webgpu.get_or_insert_with(Default::default).strategy = Some(strategy)
            }
        }
    }

    /// Effective `navigator.platform` JS string for patch templating.
    /// Falls back to host platform when unset.
    pub fn resolved_platform_js(&self) -> String {
        let plat = self.platform.unwrap_or_else(|| {
            Persona::system()
                .platform
                .unwrap_or(crate::Platform::LinuxX86_64)
        });
        plat.js_string().to_string()
    }

    /// Host-probed persona (sysinfo). Cached: first call probes, rest clone.
    /// Runtime — NOT a build-script const (build host != run host).
    ///
    /// Reports only what it could actually read. An attribute whose probe fails
    /// is left `None` (with a `WARN` naming it) rather than filled with a
    /// stand-in value, so a caller never mistakes a library default for this
    /// host. Downstream resolution supplies the fallback explicitly.
    pub fn system() -> Persona {
        SYSTEM.get_or_init(Persona::probe_system).clone()
    }

    fn probe_system() -> Persona {
        Persona::from_host_probe(
            crate::fingerprint::detect_platform(),
            crate::fingerprint::clamp_cpu_count(num_cpus::get() as u32),
            crate::fingerprint::detect_memory_gb(),
        )
    }

    /// Assemble the host persona from already-probed values. Split out of
    /// [`Persona::probe_system`] so the failure path is reachable from tests
    /// without a host that actually fails to report its RAM (the same idiom as
    /// `fingerprint::parse_version_banner`).
    ///
    /// A probe that fails leaves its field `None` — never a stand-in constant.
    /// This persona's entire contract is "the real host", so substituting a
    /// plausible-looking value would be the worst available failure: the caller
    /// believes they hold a coherent host identity while every user of this
    /// crate whose probe failed ships the *same* value, which is exactly the
    /// cross-user correlation signal a persona exists to avoid. `None` is the
    /// persona's "unset, resolve downstream" state, and downstream resolution
    /// is already explicit — `patches::identity_iife` falls back to the
    /// [`Fingerprint`](crate::fingerprint::Fingerprint)'s own probed memory,
    /// which surfaces an error rather than inventing one if it, too, fails.
    fn from_host_probe(
        platform: Platform,
        cpu: u32,
        memory_gb: Result<u32, crate::StealthError>,
    ) -> Persona {
        let device_memory_gb = memory_gb
            .inspect_err(|e| {
                tracing::warn!(
                    "host memory probe failed: {e}; leaving device_memory_gb unset \
                     (resolved from the probed fingerprint downstream) rather than \
                     reporting a fabricated value"
                );
            })
            .ok();
        Persona {
            platform: Some(platform),
            hardware_concurrency: Some(cpu),
            device_memory_gb,
            timezone: None,
            locale: None,
            seed: Some(Seed::random()),
            ..Persona::default()
        }
    }
}

/// Minimal eval surface so stealth can probe a live page without depending on
/// the `zendriver` core crate (which would be a dependency cycle).
///
/// The `zendriver` crate implements this for its `Tab`, mapping its own
/// evaluation error into [`StealthError::Probe`].
#[async_trait::async_trait]
pub trait JsProbe {
    /// Evaluate `js` in the page and return the result as a JSON value.
    async fn eval_json(&self, js: &str) -> Result<serde_json::Value, crate::StealthError>;
}

/// JS run by [`Persona::from_browser`] to read the live browser's REAL
/// fingerprint-relevant values (platform, memory, timezone, locale, WebGL
/// vendor/renderer). Returns a single JSON object.
const PROBE_JS: &str = r#"(() => {
  const c = document.createElement('canvas').getContext('webgl');
  const dbg = c && c.getExtension('WEBGL_debug_renderer_info');
  return {
    platform: navigator.platform,
    deviceMemory: navigator.deviceMemory,
    hardwareConcurrency: navigator.hardwareConcurrency,
    timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
    locale: navigator.language,
    webglVendor: dbg ? c.getParameter(dbg.UNMASKED_VENDOR_WEBGL) : null,
    webglRenderer: dbg ? c.getParameter(dbg.UNMASKED_RENDERER_WEBGL) : null,
  };
})()"#;

impl Persona {
    /// Probe the live Chrome for its REAL webgl/platform/memory/timezone
    /// values, producing a maximally coherent host persona.
    ///
    /// Runs [`PROBE_JS`] through the supplied [`JsProbe`] (the `zendriver`
    /// `Tab` implements it) and maps the resulting JSON onto a [`Persona`].
    /// Fields the browser does not expose are left `None`.
    pub async fn from_browser<P: JsProbe + Sync>(
        probe: &P,
    ) -> Result<Persona, crate::StealthError> {
        let v = probe.eval_json(PROBE_JS).await?;
        Ok(persona_from_probe(&v))
    }
}

/// Map the [`PROBE_JS`] JSON result onto a [`Persona`].
fn persona_from_probe(v: &serde_json::Value) -> Persona {
    let mut p = Persona::default();

    // `navigator.platform` strings match `Platform::js_string()`:
    // "Win32" / "MacIntel" / "Linux x86_64" (any other → Linux fallback).
    if let Some(plat) = v.get("platform").and_then(|x| x.as_str()) {
        p.platform = Some(match plat {
            "Win32" => Platform::Win32,
            "MacIntel" => Platform::MacIntel,
            _ => Platform::LinuxX86_64,
        });
    }

    // `navigator.deviceMemory` is a JS number (gigabytes).
    if let Some(mem) = v.get("deviceMemory").and_then(|x| x.as_u64()) {
        p.device_memory_gb = Some(mem as u32);
    }

    if let Some(hc) = v.get("hardwareConcurrency").and_then(|x| x.as_u64()) {
        p.hardware_concurrency = Some(hc as u32);
    }

    if let Some(tz) = v.get("timezone").and_then(|x| x.as_str()) {
        p.timezone = Some(tz.to_string());
    }

    if let Some(loc) = v.get("locale").and_then(|x| x.as_str()) {
        p.locale = Some(loc.to_string());
    }

    // WebGL vendor/renderer become a value-substitution WebglSpec when present.
    let vendor = v
        .get("webglVendor")
        .and_then(|x| x.as_str())
        .map(str::to_string);
    let renderer = v
        .get("webglRenderer")
        .and_then(|x| x.as_str())
        .map(str::to_string);
    if vendor.is_some() || renderer.is_some() {
        p.webgl = Some(WebglSpec {
            strategy: None,
            unmasked_vendor: vendor,
            unmasked_renderer: renderer,
        });
    }

    p
}

#[cfg(test)]
mod persona_tests {
    use super::*;
    use crate::test_logs::captured_warnings;

    #[test]
    fn default_persona_is_all_none() {
        let p = Persona::default();
        assert!(p.platform.is_none() && p.seed.is_none() && p.webgl.is_none());
    }

    #[test]
    fn persona_round_trips_json() {
        let p = Persona {
            seed: Some(Seed::from_u64(5)),
            timezone: Some("America/New_York".into()),
            ..Persona::default()
        };
        let s = serde_json::to_string(&p).unwrap();
        let back: Persona = serde_json::from_str(&s).unwrap();
        assert_eq!(back.seed, Some(Seed::from_u64(5)));
        assert_eq!(back.timezone.as_deref(), Some("America/New_York"));
    }

    #[test]
    fn persona_exposes_resolved_platform_for_patches() {
        let p = Persona::system();
        assert!(!p.resolved_platform_js().is_empty());
    }

    #[test]
    fn system_persona_is_populated_and_cached() {
        let a = Persona::system();
        // Host probe fills platform + cpu + memory.
        assert!(a.platform.is_some());
        assert!(a.hardware_concurrency.is_some());
        assert!(a.device_memory_gb.is_some());
        let b = Persona::system();
        // Cached → same values.
        assert_eq!(a.device_memory_gb, b.device_memory_gb);
    }

    // `StealthError` is large because `PatchFailed` wraps `CallError` (~152B);
    // bypass per-fn, as the rest of the crate does.
    #[allow(clippy::result_large_err)]
    fn failed_memory_probe() -> Result<u32, crate::StealthError> {
        Err(crate::StealthError::SystemInfo(
            "total_memory returned 0".into(),
        ))
    }

    /// The shipped bug: an unreadable host reported a library-wide constant
    /// 8 GB, so every user whose probe failed shipped one correlatable value
    /// while believing they held their own host's identity.
    #[test]
    fn failed_memory_probe_leaves_the_field_unset_never_fabricated() {
        let p = Persona::from_host_probe(Platform::MacIntel, 8, failed_memory_probe());
        assert_eq!(
            p.device_memory_gb, None,
            "a failed probe must surface as unset, not as a plausible constant"
        );
        // Guard the exact regression, in case some other default creeps back.
        assert_ne!(p.device_memory_gb, Some(8));
    }

    /// Only the attribute that failed goes unset — a memory probe failure must
    /// not discard the platform/CPU the host *did* report.
    #[test]
    fn failed_memory_probe_keeps_the_attributes_that_succeeded() {
        let p = Persona::from_host_probe(Platform::Win32, 12, failed_memory_probe());
        assert_eq!(p.platform, Some(Platform::Win32));
        assert_eq!(p.hardware_concurrency, Some(12));
        assert!(p.seed.is_some());
    }

    /// Unset alone is a silent hole; the failure has to be observable.
    #[test]
    fn failed_memory_probe_warns_naming_the_attribute() {
        let logs = captured_warnings(|| {
            let p = Persona::from_host_probe(Platform::LinuxX86_64, 4, failed_memory_probe());
            assert!(p.device_memory_gb.is_none());
        });
        assert!(logs.contains("device_memory_gb"), "got: {logs}");
        // The underlying probe error is carried through, not swallowed.
        assert!(logs.contains("total_memory returned 0"), "got: {logs}");
    }

    /// The happy path is verbatim: a successful probe is reported as probed,
    /// with no second-guessing between the probe and the persona.
    #[test]
    fn successful_memory_probe_is_reported_verbatim() {
        let p = Persona::from_host_probe(Platform::MacIntel, 10, Ok(16));
        assert_eq!(p.device_memory_gb, Some(16));
        assert_eq!(p.hardware_concurrency, Some(10));
    }

    #[test]
    fn builder_sets_fields() {
        let p = Persona::builder()
            .seed(Seed::from_u64(3))
            .timezone("UTC")
            .device_memory_gb(16)
            .build();
        assert_eq!(p.seed, Some(Seed::from_u64(3)));
        assert_eq!(p.device_memory_gb, Some(16));
        assert_eq!(p.timezone.as_deref(), Some("UTC"));
    }

    #[test]
    fn gpu_device_pins_the_renderer_and_keeps_the_rest_of_the_spec() {
        let device = crate::GpuDevice::by_name("NVIDIA GeForce RTX 4090").expect("catalogued");
        let p = Persona::builder()
            .webgl(crate::WebglSpec {
                unmasked_vendor: Some("Google Inc. (NVIDIA)".into()),
                ..Default::default()
            })
            .gpu_device(device)
            .build();
        let spec = p.webgl.expect("webgl spec");
        assert_eq!(
            spec.unmasked_renderer.as_deref(),
            Some(device.renderer().as_str())
        );
        assert!(spec.unmasked_renderer.unwrap().contains("(0x00002684)"));
        // Only the renderer is overwritten; a vendor pinned alongside survives.
        assert_eq!(
            spec.unmasked_vendor.as_deref(),
            Some("Google Inc. (NVIDIA)")
        );
    }

    #[test]
    fn from_json_and_fromstr_parse() {
        let json = r#"{"timezone":"Europe/Paris","seed":99}"#;
        let a = Persona::try_from_json(json).unwrap();
        assert_eq!(a.timezone.as_deref(), Some("Europe/Paris"));
        let b: Persona = json.parse().unwrap();
        assert_eq!(b.seed, Some(Seed::from_u64(99)));
    }

    struct FakeProbe(serde_json::Value);

    #[async_trait::async_trait]
    impl JsProbe for FakeProbe {
        async fn eval_json(&self, _js: &str) -> Result<serde_json::Value, crate::StealthError> {
            Ok(self.0.clone())
        }
    }

    #[tokio::test]
    async fn from_browser_maps_probe_fields() {
        let probe = FakeProbe(serde_json::json!({
            "platform": "MacIntel",
            "deviceMemory": 16,
            "hardwareConcurrency": 10,
            "timezone": "America/New_York",
            "locale": "en-US",
            "webglVendor": "Google Inc. (Apple)",
            "webglRenderer": "ANGLE (Apple, Apple M1, OpenGL 4.1)",
        }));
        let p = Persona::from_browser(&probe).await.unwrap();
        assert_eq!(p.platform, Some(Platform::MacIntel));
        assert_eq!(p.device_memory_gb, Some(16));
        assert_eq!(p.hardware_concurrency, Some(10));
        assert_eq!(p.timezone.as_deref(), Some("America/New_York"));
        assert_eq!(p.locale.as_deref(), Some("en-US"));
        let webgl = p.webgl.expect("webgl spec populated from probe");
        assert_eq!(
            webgl.unmasked_renderer.as_deref(),
            Some("ANGLE (Apple, Apple M1, OpenGL 4.1)")
        );
        assert_eq!(
            webgl.unmasked_vendor.as_deref(),
            Some("Google Inc. (Apple)")
        );
    }

    #[tokio::test]
    async fn from_browser_handles_missing_webgl_and_linux_platform() {
        // Null WebGL (debug-info ext unavailable) + a Linux platform string
        // that isn't a literal enum name → Linux fallback, no webgl spec.
        let probe = FakeProbe(serde_json::json!({
            "platform": "Linux x86_64",
            "deviceMemory": 8,
            "hardwareConcurrency": 4,
            "timezone": "UTC",
            "locale": "en-GB",
            "webglVendor": serde_json::Value::Null,
            "webglRenderer": serde_json::Value::Null,
        }));
        let p = Persona::from_browser(&probe).await.unwrap();
        assert_eq!(p.platform, Some(Platform::LinuxX86_64));
        assert_eq!(p.device_memory_gb, Some(8));
        assert!(p.webgl.is_none(), "null webgl → no spec");
    }

    #[test]
    fn apply_surface_override_sets_strategy_creating_spec() {
        use crate::{Strategy, Surface};
        let mut p = Persona::default();
        // Surface spec absent → created with the override strategy.
        p.apply_surface_override(Surface::Webrtc, Strategy::Native);
        assert_eq!(
            p.webrtc.as_ref().and_then(|w| w.strategy),
            Some(Strategy::Native)
        );
        // Existing spec → only strategy mutated, other fields preserved.
        p.webgl = Some(WebglSpec {
            unmasked_renderer: Some("ANGLE (x)".into()),
            ..Default::default()
        });
        p.apply_surface_override(Surface::Webgl, Strategy::Value);
        let webgl = p.webgl.as_ref().unwrap();
        assert_eq!(webgl.strategy, Some(Strategy::Value));
        assert_eq!(webgl.unmasked_renderer.as_deref(), Some("ANGLE (x)"));
    }

    #[test]
    fn apply_surface_override_webgpu() {
        use crate::{Strategy, Surface};
        let mut p = Persona::default();
        p.apply_surface_override(Surface::Webgpu, Strategy::Block);
        assert_eq!(
            p.webgpu.as_ref().and_then(|c| c.strategy),
            Some(Strategy::Block)
        );
    }

    #[test]
    fn languages_overlay_and_builder() {
        let base = Persona::builder()
            .languages(["en-US".to_string(), "en".to_string()])
            .build();
        assert_eq!(
            base.languages.as_deref(),
            Some(["en-US".to_string(), "en".to_string()].as_slice())
        );
        // `Some` in the overlay wins; `None` inherits.
        let over = Persona {
            languages: Some(vec!["de-DE".into(), "de".into()]),
            ..Default::default()
        };
        let merged = base.clone().overlay(over);
        assert_eq!(merged.languages.unwrap(), vec!["de-DE", "de"]);
        let merged2 = base.overlay(Persona::default());
        assert_eq!(merged2.languages.unwrap(), vec!["en-US", "en"]);
    }

    #[test]
    fn overlay_some_wins_none_inherits() {
        let base = Persona {
            timezone: Some("UTC".into()),
            device_memory_gb: Some(8),
            seed: Some(Seed::from_u64(1)),
            ..Persona::default()
        };
        let over = Persona {
            timezone: Some("Asia/Tokyo".into()),
            ..Persona::default()
        };
        let merged = base.overlay(over);
        assert_eq!(merged.timezone.as_deref(), Some("Asia/Tokyo")); // some wins
        assert_eq!(merged.device_memory_gb, Some(8)); // none inherits
        assert_eq!(merged.seed, Some(Seed::from_u64(1)));
    }

    #[test]
    fn persona_round_trips_geolocation_json() {
        let p = Persona {
            geolocation: Some(GeoPos {
                latitude: 21.0285,
                longitude: 105.8542,
                accuracy: Some(50.0),
            }),
            ..Persona::default()
        };
        let s = serde_json::to_string(&p).unwrap();
        let back: Persona = serde_json::from_str(&s).unwrap();
        assert_eq!(
            back.geolocation,
            Some(GeoPos {
                latitude: 21.0285,
                longitude: 105.8542,
                accuracy: Some(50.0),
            })
        );
    }

    #[test]
    fn geolocation_parses_without_accuracy() {
        // `persona_overlay` accepts parsed JSON (BrowserBuilder doc example);
        // accuracy is optional and must default to None when omitted.
        let json = r#"{"geolocation":{"latitude":10.0,"longitude":20.0}}"#;
        let p: Persona = json.parse().unwrap();
        let geo = p.geolocation.expect("geolocation parsed");
        assert_eq!(geo.latitude, 10.0);
        assert_eq!(geo.longitude, 20.0);
        assert_eq!(geo.accuracy, None);
    }

    #[test]
    fn persona_round_trips_ua_metadata_and_screen_json() {
        let p = Persona {
            ua: Some(UaSpec {
                ua_string: Some("Mozilla/5.0 (Windows NT 10.0; Win64; x64)".into()),
                ua_metadata: Some(UaMetadata {
                    platform_version: Some("15.0.0".into()),
                    architecture: Some("arm".into()),
                    bitness: Some("64".into()),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            screen: Some(ScreenSpec::new(1536, 864, 1.25)),
            ..Persona::default()
        };
        let s = serde_json::to_string(&p).unwrap();
        let back: Persona = serde_json::from_str(&s).unwrap();
        assert_eq!(
            back.ua
                .as_ref()
                .and_then(|u| u.ua_metadata.as_ref())
                .and_then(|m| m.architecture.clone())
                .as_deref(),
            Some("arm")
        );
        assert_eq!(back.screen, Some(ScreenSpec::new(1536, 864, 1.25)));
    }

    #[test]
    fn overlay_screen_some_wins_none_inherits() {
        let base = Persona {
            screen: Some(ScreenSpec::new(1920, 1080, 1.0)),
            ..Persona::default()
        };
        let over = Persona {
            screen: Some(ScreenSpec::new(1536, 864, 1.25)),
            ..Persona::default()
        };
        let merged = base.clone().overlay(over);
        assert_eq!(merged.screen, Some(ScreenSpec::new(1536, 864, 1.25))); // some wins
        let merged2 = base.overlay(Persona::default());
        assert_eq!(merged2.screen, Some(ScreenSpec::new(1920, 1080, 1.0))); // none inherits
    }

    #[test]
    fn overlay_ua_composes_field_wise_not_whole_swap() {
        // A base persona's `platform` override must survive an overlay that
        // only sets `ua_string` — whole-swap semantics would have silently
        // dropped it.
        let base = Persona {
            ua: Some(UaSpec {
                platform: Some("Windows".into()),
                ..Default::default()
            }),
            ..Persona::default()
        };
        let over = Persona {
            ua: Some(UaSpec {
                ua_string: Some("custom-ua".into()),
                ..Default::default()
            }),
            ..Persona::default()
        };
        let merged = base.overlay(over);
        let ua = merged.ua.expect("ua merged");
        assert_eq!(ua.platform.as_deref(), Some("Windows")); // inherited
        assert_eq!(ua.ua_string.as_deref(), Some("custom-ua")); // over wins
    }

    #[test]
    fn overlay_geolocation_some_wins_none_inherits() {
        let base = Persona {
            geolocation: Some(GeoPos {
                latitude: 1.0,
                longitude: 2.0,
                accuracy: None,
            }),
            ..Persona::default()
        };
        let over = Persona {
            geolocation: Some(GeoPos {
                latitude: 9.0,
                longitude: 8.0,
                accuracy: Some(5.0),
            }),
            ..Persona::default()
        };
        let merged = base.clone().overlay(over);
        assert_eq!(
            merged.geolocation,
            Some(GeoPos {
                latitude: 9.0,
                longitude: 8.0,
                accuracy: Some(5.0),
            })
        ); // some wins
        let merged2 = base.overlay(Persona::default());
        assert_eq!(
            merged2.geolocation,
            Some(GeoPos {
                latitude: 1.0,
                longitude: 2.0,
                accuracy: None,
            })
        ); // none inherits
    }

    #[test]
    fn persona_gpu_defaults_to_none() {
        assert!(Persona::default().gpu.is_none());
    }

    #[test]
    fn persona_overlay_takes_the_higher_priority_gpu_whole() {
        // One device is one coherent artifact, like ScreenSpec: the winning
        // persona's GPU wins outright rather than merging field-wise, which
        // could compose two devices into one that exists nowhere.
        let base = Persona {
            gpu: Some(crate::GpuProfile::empty()),
            ..Persona::default()
        };
        let mut over = Persona::default();
        let mut p = crate::GpuProfile::empty();
        p.unmasked_renderer = "ANGLE (NVIDIA, ...)".into();
        over.gpu = Some(p);

        let merged = base.overlay(over);
        assert_eq!(
            merged.gpu.expect("gpu survives overlay").unmasked_renderer,
            "ANGLE (NVIDIA, ...)"
        );
    }

    #[test]
    fn persona_overlay_keeps_the_base_gpu_when_the_overlay_has_none() {
        let mut base = Persona::default();
        let mut p = crate::GpuProfile::empty();
        p.unmasked_renderer = "base-renderer".into();
        base.gpu = Some(p);

        let merged = base.overlay(Persona::default());
        assert_eq!(
            merged.gpu.expect("gpu survives").unmasked_renderer,
            "base-renderer"
        );
    }
}
