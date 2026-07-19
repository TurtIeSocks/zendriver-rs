//! Per-surface value specs carried by a Persona. All fields optional → overlay.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::surface::Strategy;
use crate::fingerprint::{Brand, UserAgentMetadata};

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
    /// Full UA-CH (`Emulation.setUserAgentOverride.userAgentMetadata`)
    /// override, field-wise. Any sub-field left `None` falls back to the
    /// fingerprint-derived value at resolve (see [`UaMetadata::resolve`]).
    /// JSON key is `userAgentMetadata` to match the wire shape callers
    /// harvest it from (e.g. `navigator.userAgentData.getHighEntropyValues`).
    #[serde(rename = "userAgentMetadata")]
    pub ua_metadata: Option<UaMetadata>,
}

impl UaSpec {
    /// Field-wise merge: `Some` in `over` wins, `None` inherits from `self`.
    /// `ua_metadata` recurses into [`UaMetadata::overlay`] when both sides
    /// carry one, so layering two personas composes UA-CH sub-fields instead
    /// of one side wholesale-replacing the other.
    #[must_use]
    pub fn overlay(self, over: UaSpec) -> UaSpec {
        UaSpec {
            ua_string: over.ua_string.or(self.ua_string),
            platform: over.platform.or(self.platform),
            ua_metadata: match (self.ua_metadata, over.ua_metadata) {
                (Some(base), Some(add)) => Some(base.overlay(add)),
                (base, add) => add.or(base),
            },
        }
    }
}

/// Field-wise `userAgentMetadata` (UA-CH) override. Mirrors
/// [`UserAgentMetadata`] but every field is optional: unset fields fall back
/// to the fingerprint-derived value at resolve (see [`Self::resolve`]).
/// `wow64` is intentionally absent — it has no independent override surface
/// here and always comes from the derived base.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct UaMetadata {
    pub brands: Option<Vec<Brand>>,
    #[serde(rename = "fullVersionList")]
    pub full_version_list: Option<Vec<Brand>>,
    pub platform: Option<String>,
    #[serde(rename = "platformVersion")]
    pub platform_version: Option<String>,
    pub architecture: Option<String>,
    pub bitness: Option<String>,
    pub mobile: Option<bool>,
    pub model: Option<String>,
}

impl UaMetadata {
    /// Field-wise merge: `Some` in `over` wins, `None` inherits from `self`.
    #[must_use]
    pub fn overlay(self, over: UaMetadata) -> UaMetadata {
        UaMetadata {
            brands: over.brands.or(self.brands),
            full_version_list: over.full_version_list.or(self.full_version_list),
            platform: over.platform.or(self.platform),
            platform_version: over.platform_version.or(self.platform_version),
            architecture: over.architecture.or(self.architecture),
            bitness: over.bitness.or(self.bitness),
            mobile: over.mobile.or(self.mobile),
            model: over.model.or(self.model),
        }
    }

    /// Resolve into a complete [`UserAgentMetadata`], filling any unset
    /// sub-field from `base` (the fingerprint-derived value). `wow64` always
    /// comes from `base` — it has no override surface on [`UaMetadata`].
    #[must_use]
    pub fn resolve(&self, base: &UserAgentMetadata) -> UserAgentMetadata {
        UserAgentMetadata {
            brands: self.brands.clone().unwrap_or_else(|| base.brands.clone()),
            full_version_list: self
                .full_version_list
                .clone()
                .unwrap_or_else(|| base.full_version_list.clone()),
            platform: self
                .platform
                .clone()
                .unwrap_or_else(|| base.platform.clone()),
            platform_version: self
                .platform_version
                .clone()
                .unwrap_or_else(|| base.platform_version.clone()),
            architecture: self
                .architecture
                .clone()
                .unwrap_or_else(|| base.architecture.clone()),
            bitness: self.bitness.clone().unwrap_or_else(|| base.bitness.clone()),
            wow64: base.wow64,
            mobile: self.mobile.unwrap_or(base.mobile),
            model: self.model.clone().unwrap_or_else(|| base.model.clone()),
        }
    }
}

/// Screen / device-metrics override (`Emulation.setDeviceMetricsOverride`).
/// Whole-value at [`Persona::overlay`](super::Persona::overlay) — composing
/// two personas has the higher-priority persona's screen win outright when
/// set, same as every other spec field (a screen is one coherent artifact,
/// not sub-field patchable the way [`UaMetadata`] is).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ScreenSpec {
    pub width: u32,
    pub height: u32,
    pub device_pixel_ratio: f64,
}

/// WebGL value substitution.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct WebglSpec {
    pub strategy: Option<Strategy>,
    pub unmasked_vendor: Option<String>,
    pub unmasked_renderer: Option<String>,
}

/// WebGPU adapter value substitution + opt-in synthetic-adapter fabrication.
///
/// `None`-everywhere ([`WebgpuSpec::default`]) is the regression guard: it
/// behaves **byte-for-byte** like the pre-`WebgpuSpec` `SurfaceCfg` did — a
/// real `navigator.gpu` adapter's `.info` is decorated with a vendor /
/// architecture DERIVED from the [`WebglSpec`] renderer (never fabricated),
/// `device` / `description` are emitted empty (Chrome masks them), `.limits`
/// / `.features` are left untouched, and a GPU-less host is left alone —
/// `navigator.gpu.requestAdapter()` still resolves `null`, same as native
/// Chrome.
///
/// # You own value accuracy
///
/// Every field below is **caller-supplied** — nothing here is probed or
/// invented from a real GPU. A `vendor` / `limits` / `features` combination
/// that does not correspond to any real device is **more detectable than
/// leaving the field `None`**: fingerprinting scripts cross-check
/// `GPUAdapterInfo` against `GPUSupportedLimits` / `GPUSupportedFeatures` and
/// against the WebGL renderer string, so an incoherent combination reads as a
/// bot faster than honest absence does. Only set these fields to values
/// you've verified against a real device (e.g. probed live from
/// `navigator.gpu` on that device) — never invent plausible-looking numbers.
///
/// # What fabrication does
///
/// [`fabricate_when_absent`](Self::fabricate_when_absent) covers **both**
/// GPU-less shapes:
/// - **`navigator.gpu` entirely absent** (`'gpu' in navigator === false` —
///   the common case, e.g. Chrome launched with `--disable-gpu`, which is
///   zendriver's default under headless): a synthetic `navigator.gpu` is
///   defined on `Navigator.prototype` whose `requestAdapter()` resolves the
///   synthetic adapter. This flips `'gpu' in navigator` to **true**, which is
///   coherent for a modern-Chrome persona — real modern Chrome always exposes
///   `navigator.gpu` even with no usable GPU (there `requestAdapter()` merely
///   resolves `null`). Restoring that presence is your explicit opt-in.
/// - **`navigator.gpu` present but `requestAdapter()` resolves `null`**: the
///   real `requestAdapter` is wrapped so a `null`/rejected result falls back
///   to the synthetic adapter (a real adapter passes through untouched).
///
/// # v1 limitations
///
/// - The decorated / fabricated `.info`, `.limits`, `.features` are **plain
///   objects**, not real `GPUAdapterInfo` / `GPUSupportedLimits` /
///   `GPUSupportedFeatures` instances — an `instanceof` check would tell.
///   Likewise a synthesized `navigator.gpu` is a plain object, so
///   `navigator.gpu instanceof GPU` is `false`.
/// - [`fabricate_when_absent`](Self::fabricate_when_absent)'s synthetic
///   adapter's `requestDevice()` **always rejects**. Faking a working
///   `GPUDevice` needs a real GPU behind it, which this patch cannot
///   provide — it only makes `navigator.gpu.requestAdapter()` resolve a
///   coherent adapter for detection scripts that stop at the adapter, it
///   does not unlock actual WebGPU rendering on a GPU-less host.
///
/// ```no_run
/// use std::collections::BTreeMap;
/// use zendriver_stealth::{Persona, WebgpuSpec};
///
/// // Decorate a REAL adapter with values probed from an actual device.
/// let persona = Persona {
///     webgpu: Some(WebgpuSpec {
///         vendor: Some("apple".into()),
///         architecture: Some("metal-3".into()),
///         ..Default::default()
///     }),
///     ..Persona::default()
/// };
///
/// // Opt-in fabrication on a GPU-less host: requires an explicit vendor AND
/// // limits (see `fabricate_when_absent` below) — anything less is refused.
/// let mut limits = BTreeMap::new();
/// limits.insert("maxTextureDimension2D".to_string(), 16384);
/// let fabricated = WebgpuSpec {
///     vendor: Some("apple".into()),
///     architecture: Some("metal-3".into()),
///     limits: Some(limits),
///     features: Some(vec!["texture-compression-bc".into()]),
///     fabricate_when_absent: Some(true),
///     ..Default::default()
/// };
/// let _ = fabricated;
/// ```
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct WebgpuSpec {
    /// Render strategy for this surface. `None` → kind default (`Value`).
    pub strategy: Option<Strategy>,
    /// `GPUAdapterInfo.vendor`. `None` → derived from the [`WebglSpec`]
    /// renderer (today's behavior, via the internal `adapter_for_renderer`
    /// dataset mapping).
    pub vendor: Option<String>,
    /// `GPUAdapterInfo.architecture`. `None` → derived from the WebGL
    /// renderer, same as [`vendor`](Self::vendor).
    pub architecture: Option<String>,
    /// `GPUAdapterInfo.device`. `None` → `""` (real Chrome masks this field).
    pub device: Option<String>,
    /// `GPUAdapterInfo.description`. `None` → `""` (real Chrome masks this
    /// field too).
    pub description: Option<String>,
    /// Caller-supplied `GPUSupportedLimits` caps (e.g.
    /// `"maxTextureDimension2D"`). Applied to a real (or fabricated) adapter's
    /// `.limits` getter. `None` → the adapter's own limits are left
    /// untouched.
    pub limits: Option<BTreeMap<String, u64>>,
    /// Caller-supplied `GPUSupportedFeatures` strings (e.g.
    /// `"texture-compression-bc"`). `None` → the adapter's own features are
    /// left untouched.
    pub features: Option<Vec<String>>,
    /// Explicit opt-in: synthesize a `navigator.gpu` adapter on a host with no
    /// real one, instead of leaving `requestAdapter()` at `null` (or
    /// `navigator.gpu` entirely absent). Covers both GPU-less shapes — a
    /// missing `navigator.gpu` is defined fresh (flipping `'gpu' in navigator`
    /// to true), and a present-but-null `requestAdapter` is wrapped; see the
    /// "What fabrication does" section above. Requires [`vendor`](Self::vendor)
    /// AND [`limits`](Self::limits) to BOTH be explicitly set — a bare `true`
    /// with nothing else is refused (silently, no-op) because there is nothing
    /// coherent to fabricate; this project never auto-invents fingerprint
    /// values. See the v1 limitations above for what fabrication does NOT
    /// cover (no working `GPUDevice`; plain-object `instanceof`).
    pub fabricate_when_absent: Option<bool>,
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
#[allow(clippy::panic, clippy::unwrap_used)]
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
    fn ua_metadata_round_trips_json_camelcase() {
        let m = UaMetadata {
            brands: Some(vec![Brand {
                brand: "Not_A Brand".into(),
                version: "8".into(),
            }]),
            full_version_list: Some(vec![Brand {
                brand: "Chromium".into(),
                version: "150.0.7500.0".into(),
            }]),
            platform: Some("Windows".into()),
            platform_version: Some("15.0.0".into()),
            architecture: Some("x86".into()),
            bitness: Some("64".into()),
            mobile: Some(false),
            model: Some(String::new()),
        };
        let s = serde_json::to_string(&m).unwrap();
        assert!(
            s.contains("\"fullVersionList\""),
            "expected camelCase key, got: {s}"
        );
        assert!(
            s.contains("\"platformVersion\""),
            "expected camelCase key, got: {s}"
        );
        let back: UaMetadata = serde_json::from_str(&s).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn ua_spec_round_trips_ua_metadata_under_camelcase_key() {
        let spec = UaSpec {
            ua_string: Some("Mozilla/5.0 (Windows NT 10.0; Win64; x64)".into()),
            platform: Some("Windows".into()),
            ua_metadata: Some(UaMetadata {
                platform_version: Some("15.0.0".into()),
                architecture: Some("arm".into()),
                ..Default::default()
            }),
        };
        let s = serde_json::to_string(&spec).unwrap();
        assert!(
            s.contains("\"userAgentMetadata\""),
            "expected camelCase field key, got: {s}"
        );
        let back: UaSpec = serde_json::from_str(&s).unwrap();
        assert_eq!(spec, back);
    }

    #[test]
    fn ua_spec_overlay_composes_ua_metadata_field_wise() {
        let base = UaSpec {
            platform: Some("Windows".into()),
            ua_metadata: Some(UaMetadata {
                brands: Some(vec![Brand {
                    brand: "X".into(),
                    version: "1".into(),
                }]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let over = UaSpec {
            ua_metadata: Some(UaMetadata {
                platform_version: Some("11.0.0".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let merged = base.overlay(over);
        // Non-`ua` top-level field inherited unchanged (`over` didn't set it).
        assert_eq!(merged.platform.as_deref(), Some("Windows"));
        let uam = merged.ua_metadata.expect("ua_metadata merged");
        assert!(uam.brands.is_some(), "brands inherited from base");
        assert_eq!(uam.platform_version.as_deref(), Some("11.0.0")); // over wins
    }

    #[test]
    fn ua_spec_overlay_one_sided_is_a_no_op_merge() {
        let base = UaSpec {
            ua_string: Some("base-ua".into()),
            ..Default::default()
        };
        let merged = base.clone().overlay(UaSpec::default());
        assert_eq!(merged, base);
    }

    #[test]
    fn ua_metadata_resolve_fills_unset_fields_from_base() {
        let base = UserAgentMetadata::realistic(crate::Platform::Win32, 150, "150.0.7500.0");
        let custom = UaMetadata {
            platform_version: Some("11.0.0".into()),
            ..Default::default()
        };
        let resolved = custom.resolve(&base);
        assert_eq!(resolved.platform_version, "11.0.0"); // custom wins
        assert_eq!(resolved.brands, base.brands); // fell back to base
        assert_eq!(resolved.wow64, base.wow64); // no override surface, always base
    }

    #[test]
    fn ua_metadata_resolve_empty_equals_base_exactly() {
        let base = UserAgentMetadata::realistic(crate::Platform::MacIntel, 148, "148.0.7778.181");
        let resolved = UaMetadata::default().resolve(&base);
        assert_eq!(resolved, base);
    }

    #[test]
    fn screen_spec_round_trips_json() {
        let s = ScreenSpec {
            width: 1536,
            height: 864,
            device_pixel_ratio: 1.25,
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: ScreenSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn empty_spec_omits_nothing_required() {
        let f = FontSpec::default();
        assert!(f.available.is_none());
    }

    #[test]
    fn webgpu_spec_default_is_all_none() {
        let w = WebgpuSpec::default();
        assert!(w.strategy.is_none());
        assert!(w.vendor.is_none());
        assert!(w.architecture.is_none());
        assert!(w.device.is_none());
        assert!(w.description.is_none());
        assert!(w.limits.is_none());
        assert!(w.features.is_none());
        assert!(w.fabricate_when_absent.is_none());
    }

    #[test]
    fn webgpu_spec_round_trips_json() {
        let mut limits = std::collections::BTreeMap::new();
        limits.insert("maxTextureDimension2D".to_string(), 16384u64);
        let w = WebgpuSpec {
            strategy: Some(Strategy::Value),
            vendor: Some("apple".into()),
            architecture: Some("metal-3".into()),
            device: Some("Apple M4 Pro".into()),
            description: Some("Metal 3".into()),
            limits: Some(limits),
            features: Some(vec!["texture-compression-bc".into()]),
            fabricate_when_absent: Some(true),
        };
        let s = serde_json::to_string(&w).unwrap();
        let back: WebgpuSpec = serde_json::from_str(&s).unwrap();
        assert_eq!(w, back);
    }
}
