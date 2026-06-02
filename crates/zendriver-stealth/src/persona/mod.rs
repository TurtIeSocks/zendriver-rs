//! Persona: the unified fingerprint configuration.

pub mod seed;
pub mod specs;
pub mod surface;

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
            canvas: over.canvas.or(self.canvas),
            audio: over.audio.or(self.audio),
            fonts: over.fonts.or(self.fonts),
            client_rects: over.client_rects.or(self.client_rects),
            webrtc: over.webrtc.or(self.webrtc),
            hardware: over.hardware.or(self.hardware),
            seed: over.seed.or(self.seed),
        }
    }

    /// Effective `navigator.platform` JS string for patch templating.
    /// Falls back to host platform when unset.
    pub fn resolved_platform_js(&self) -> String {
        let plat = self.platform.unwrap_or_else(|| {
            Persona::system().platform.unwrap_or(crate::Platform::LinuxX86_64)
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
        let mut p = Persona::default();
        p.seed = Some(Seed::from_u64(5));
        p.timezone = Some("America/New_York".into());
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
