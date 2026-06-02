//! Persona: the unified fingerprint configuration.

pub mod seed;
pub mod specs;
pub mod surface;
pub(crate) mod webgpu_adapter;

pub use seed::Seed;
pub use specs::{FontSpec, HardwareSpec, SurfaceCfg, UaSpec, WebglSpec, WebrtcSpec};

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::Platform;

static SYSTEM: OnceLock<Persona> = OnceLock::new();

/// Unified fingerprint configuration. Every field optional → overlay semantics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Persona {
    pub platform: Option<Platform>,
    pub ua: Option<UaSpec>,
    pub hardware_concurrency: Option<u32>,
    pub device_memory_gb: Option<u32>,
    pub timezone: Option<String>,
    pub locale: Option<String>,
    pub webgl: Option<WebglSpec>,
    pub webgpu: Option<SurfaceCfg>,
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
            ua: over.ua.or(self.ua),
            hardware_concurrency: over.hardware_concurrency.or(self.hardware_concurrency),
            device_memory_gb: over.device_memory_gb.or(self.device_memory_gb),
            timezone: over.timezone.or(self.timezone),
            locale: over.locale.or(self.locale),
            webgl: over.webgl.or(self.webgl),
            webgpu: over.webgpu.or(self.webgpu),
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
    pub fn system() -> Persona {
        SYSTEM.get_or_init(Persona::probe_system).clone()
    }

    fn probe_system() -> Persona {
        let platform = crate::fingerprint::detect_platform();
        let cpu = crate::fingerprint::clamp_cpu_count(num_cpus::get() as u32);
        let mem = crate::fingerprint::detect_memory_gb().unwrap_or(8);
        Persona {
            platform: Some(platform),
            hardware_concurrency: Some(cpu),
            device_memory_gb: Some(mem),
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
}
