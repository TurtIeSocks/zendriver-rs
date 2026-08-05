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
    /// Work-area width, i.e. `screen.availWidth`. `None` keeps the derived
    /// default (the full width — no vertical dock or side panel).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avail_width: Option<u32>,
    /// Work-area height, i.e. `screen.availHeight` — `height` minus whatever
    /// the OS reserves (Windows taskbar, macOS menu bar plus dock).
    ///
    /// `None` keeps the derived default of `height - 48`. That constant is a
    /// plausible Windows taskbar and nothing more: it exists so a caller who
    /// supplies no capture still gets `availHeight < height`, which is the
    /// relationship the tell lives in. A caller REPLAYING a real device should
    /// pass the value it measured — a real macOS reports `height - 25` minus
    /// the dock, a real Windows `- 40/48/72` depending on DPI scaling, or
    /// `- 0` with the taskbar auto-hidden, and none of those can be expressed
    /// by a constant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avail_height: Option<u32>,
    /// Viewport height, i.e. `window.innerHeight`. `None` keeps the derived
    /// default of `height - 86` (a plausible tab strip + omnibox + bookmarks
    /// bar). Same reasoning as [`Self::avail_height`]: the browser chrome's
    /// real height varies with the user's toolbars and zoom.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inner_height: Option<u32>,
}

impl ScreenSpec {
    /// A screen of `width x height` at `device_pixel_ratio`, with every inset
    /// left to the patch's own derivation.
    ///
    /// Use the `with_*` setters to replay a MEASURED device instead. They are
    /// separate because the derived defaults are a plausible fiction and a
    /// measurement is not: a caller should have to say which one it means.
    #[must_use]
    pub fn new(width: u32, height: u32, device_pixel_ratio: f64) -> Self {
        Self {
            width,
            height,
            device_pixel_ratio,
            avail_width: None,
            avail_height: None,
            inner_height: None,
        }
    }

    /// Replay a measured `screen.availWidth` / `screen.availHeight`.
    #[must_use]
    pub fn with_avail(mut self, width: u32, height: u32) -> Self {
        self.avail_width = Some(width);
        self.avail_height = Some(height);
        self
    }

    /// Replay a measured `window.innerHeight`.
    #[must_use]
    pub fn with_inner_height(mut self, height: u32) -> Self {
        self.inner_height = Some(height);
        self
    }
}

/// WebGL value substitution.
///
/// Finer-grained than [`Persona::gpu`](super::Persona::gpu): this patches the
/// two masked identity strings, while `gpu` carries a whole coherent device
/// (every readable WebGL parameter plus the WebGPU adapter) resolved from the
/// tier tables. Both can be set together — `gpu` supplies the full device,
/// and this still overlays on top to pin just the vendor/renderer strings
/// without restating the rest of the device.
///
/// A [`Strategy::Native`] strategy here emits no WebGL patch at all — the
/// host's real renderer passes through — and therefore also suppresses the
/// WebGPU **value** spoof, so `navigator.gpu` cannot name a GPU that
/// `getParameter(UNMASKED_RENDERER_WEBGL)` never claimed. An explicit
/// [`WebgpuSpec`] `Block` names no GPU at all and stays honored regardless.
/// [`StealthProfile::native_isolation`](crate::StealthProfile::native_isolation)
/// applies the same coupling profile-wide.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct WebglSpec {
    pub strategy: Option<Strategy>,
    pub unmasked_vendor: Option<String>,
    pub unmasked_renderer: Option<String>,
}

/// WebGPU adapter value substitution + opt-in synthetic-adapter fabrication.
///
/// `None`-everywhere ([`WebgpuSpec::default`]) resolves a whole coherent
/// adapter rather than doing nothing: `.info`'s vendor / architecture are
/// DERIVED from the [`WebglSpec`] renderer (never fabricated), `device` /
/// `description` are emitted empty (Chrome masks them), and `.limits` /
/// `.features` are the **measured** values of the same capability tier that
/// renderer resolves — probed from a real `navigator.gpu` in the same run as
/// that tier's WebGL blocks, so both APIs describe one device. A GPU-less host
/// is still left alone: `requestAdapter()` resolves `null`, same as native
/// Chrome, unless [`fabricate_when_absent`](Self::fabricate_when_absent) says
/// otherwise.
///
/// A tier whose own machine had **no** adapter (SwiftShader) serves no limits
/// and no features, leaving the host adapter's untouched — that absence is the
/// measurement, and filling it from another tier would claim a GPU the persona
/// just told WebGL it does not have.
///
/// # You own the accuracy of what you set
///
/// The defaults above are measured. Every field below **replaces** them with a
/// value only you can vouch for — nothing here is probed. A `vendor` /
/// `limits` / `features` combination that does not correspond to any real
/// device is **more detectable than leaving the field `None`**: fingerprinting
/// scripts cross-check `GPUAdapterInfo` against `GPUSupportedLimits` /
/// `GPUSupportedFeatures` and against the WebGL renderer string, so an
/// incoherent combination reads as a bot faster than honest absence does. Only
/// set these fields to values you've verified against a real device (e.g.
/// probed live from `navigator.gpu` on that device) — never invent
/// plausible-looking numbers.
///
/// # What fabrication does
///
/// [`fabricate_when_absent`](Self::fabricate_when_absent) covers **both**
/// GPU-less shapes:
/// - **`navigator.gpu` entirely absent** (`'gpu' in navigator === false`):
///   `navigator.gpu` is `[SecureContext]`-gated, so this is what an
///   opaque-origin page (`about:blank`, `data:`) reports no matter which GPU
///   backend Chrome is running — not a consequence of `--disable-gpu` or any
///   other launch flag. On such a page, a synthetic `navigator.gpu` is
///   defined on `Navigator.prototype` whose `requestAdapter()` resolves the
///   synthetic adapter. This flips `'gpu' in navigator` to **true**, which is
///   coherent for a modern-Chrome persona — real modern Chrome always exposes
///   `navigator.gpu` even with no usable GPU (there `requestAdapter()` merely
///   resolves `null`). Restoring that presence is your explicit opt-in.
/// - **`navigator.gpu` present but `requestAdapter()` resolves `null`**: what
///   a secure-context page reports when Chrome has no usable GPU (e.g. under
///   zendriver's default `--disable-gpu` headless launch) — the real
///   `requestAdapter` is wrapped so a `null`/rejected result falls back to
///   the synthetic adapter (a real adapter passes through untouched).
///
/// # v1 limitations
///
/// - `navigator.gpu`, the fabricated adapter, and its `.info` inherit the real
///   `GPU` / `GPUAdapter` / `GPUAdapterInfo` prototypes (or a synthesized
///   same-named constructor when the WebGPU IDL is absent), so `instanceof`
///   holds for all three. `.limits` / `.features` are genuine
///   `GPUSupportedLimits` / `GPUSupportedFeatures` instances wherever those
///   classes exist — the patch overrides their prototypes' accessors and
///   setlike members rather than substituting a plain object and a `Set`, so
///   brand, `constructor.name` and own-property shape are a real adapter's.
///   Only a page with no WebGPU IDL at all (the opaque origin that
///   [`fabricate_when_absent`](Self::fabricate_when_absent) case (b) covers)
///   falls back to the substitutes. What remains either way: the iterators
///   `features.keys()` / `values()` / `entries()` hand back are ordinary
///   `Array Iterator`s rather than `GPUSupportedFeatures Iterator`s.
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
    /// `.limits` getter, merged **key-wise** over the resolved tier's measured
    /// limits — so pinning one limit overrides that one and keeps the tier's
    /// value for every other, the same merge [`Persona::gpu`](super::Persona::gpu)
    /// gives the WebGL parameter maps. `None` → the tier's measured limits
    /// alone (or the adapter's own, on a tier that measured none).
    pub limits: Option<BTreeMap<String, u64>>,
    /// Caller-supplied `GPUSupportedFeatures` strings (e.g.
    /// `"texture-compression-bc"`). Replaces the resolved tier's measured
    /// feature list **wholesale** — a feature list is a set you either state in
    /// full or leave alone, so a partial list would claim the absence of every
    /// feature you did not name. `None` → the tier's measured features alone
    /// (or the adapter's own, on a tier that measured none).
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
    /// cover (no working `GPUDevice`; `limits`/`features` `instanceof`).
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
        let s = ScreenSpec::new(1536, 864, 1.25);
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
