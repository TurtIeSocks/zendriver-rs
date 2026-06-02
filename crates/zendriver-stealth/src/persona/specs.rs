//! Per-surface value specs carried by a Persona. All fields optional → overlay.

use serde::{Deserialize, Serialize};

use super::surface::Strategy;

/// Noise-surface config (canvas, audio, clientRects).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SurfaceCfg {
    /// Render strategy for this surface. None → kind default.
    pub strategy: Option<Strategy>,
}

/// UA string + UA-CH metadata override.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct UaSpec {
    pub ua_string: Option<String>,
    /// Free-form UA-CH overrides; merged onto realistic() output at resolve.
    pub platform: Option<String>,
}

/// WebGL value substitution.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct WebglSpec {
    pub strategy: Option<Strategy>,
    pub unmasked_vendor: Option<String>,
    pub unmasked_renderer: Option<String>,
}

/// Font set + measureText noise.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct FontSpec {
    pub strategy: Option<Strategy>,
    /// Allow-list of font families the page may detect.
    pub available: Option<Vec<String>>,
}

/// WebRTC policy.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct WebrtcSpec {
    pub strategy: Option<Strategy>,
    /// Fake public IP used when strategy = Value.
    pub fake_ip: Option<String>,
}

/// Hardware bundle.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct HardwareSpec {
    pub strategy: Option<Strategy>,
    pub battery_level: Option<f64>,
    pub media_devices: Option<u32>,
    pub speech_voices: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_round_trip_json() {
        let w = WebglSpec {
            strategy: Some(Strategy::Value),
            unmasked_vendor: Some("Google Inc. (NVIDIA)".into()),
            unmasked_renderer: Some("ANGLE (NVIDIA, ...)".into()),
        };
        let s = serde_json::to_string(&w).unwrap();
        let back: WebglSpec = serde_json::from_str(&s).unwrap();
        assert_eq!(w, back);
    }

    #[test]
    fn empty_spec_omits_nothing_required() {
        let f = FontSpec::default();
        assert!(f.available.is_none());
    }
}
