//! Bundles individual patches into a bootstrap script driven by a [`Persona`]
//! (the surface-spoofing config) plus a [`Fingerprint`] (coherent UA / Chrome
//! identity probed at launch).
//!
//! The 9 identity patches run inside a single IIFE called with a serialized
//! fingerprint object — one `Page.addScriptToEvaluateOnNewDocument` round-trip
//! covers them. The persona-driven surface patches (canvas/audio/clientRects
//! noise, webgl/fonts/hardware value substitution, webrtc policy) are appended
//! after, each emitted only when its resolved [`Strategy`] is not `Native`.

use serde_json::json;

use crate::persona::surface::{Strategy, Surface};
use crate::persona::{FontSpec, HardwareSpec, SurfaceCfg, WebglSpec, WebgpuSpec, WebrtcSpec};
use crate::{Fingerprint, Persona, Seed, UserAgentMetadata};

// --- Native-function masking prelude (runs first, wraps everything) ------
const NATIVE: &str = include_str!("patches/_native.js");

// --- Identity patches (run inside the fp IIFE) ---------------------------
const WEBDRIVER: &str = include_str!("patches/webdriver.js");
const PLUGINS: &str = include_str!("patches/plugins.js");
const CHROME: &str = include_str!("patches/chrome.js");
const PERMISSIONS: &str = include_str!("patches/permissions.js");
const CODECS: &str = include_str!("patches/codecs.js");
const NAVIGATOR_PROPS: &str = include_str!("patches/navigator_props.js");
const USER_AGENT_DATA: &str = include_str!("patches/user_agent_data.js");
const BROKEN_IMAGE: &str = include_str!("patches/broken_image.js");

// --- Persona surface patches (appended after the identity IIFE) ----------
//
// `webgl.js` carries BOTH the hardcoded vendor/renderer fallback block and a
// persona-driven value-substitution IIFE, so it is emitted exactly ONCE here
// (not in the identity IIFE) to avoid a duplicate block + unsubstituted
// `WEBGL_VENDOR`/`WEBGL_RENDERER` tokens. It references no `fp` fields, so
// running it at the bootstrap's top level (rather than inside the fp IIFE) is
// behavior-preserving; its top-level `const VENDOR`/`RENDERER` are script-
// scoped and the nested IIFE's `const VENDOR` is function-scoped — no
// redeclaration.
const PRNG: &str = include_str!("patches/_prng.js");
const WEBGL: &str = include_str!("patches/webgl.js");
const CANVAS: &str = include_str!("patches/canvas.js");
const AUDIO: &str = include_str!("patches/audio.js");
const CLIENT_RECTS: &str = include_str!("patches/client_rects.js");
const FONTS: &str = include_str!("patches/fonts.js");
const WEBRTC: &str = include_str!("patches/webrtc.js");
const HARDWARE: &str = include_str!("patches/hardware.js");
const WEBGPU: &str = include_str!("patches/webgpu.js");
// Coherent window/screen geometry — fixes the impossible innerWidth>outerWidth
// and availHeight===height that setDeviceMetricsOverride leaves behind (a hard
// reese84/Imperva bot tell). References no `fp` fields; emitted unconditionally.
const SCREEN: &str = include_str!("patches/screen.js");
// Synthetic pointer entropy — a zero-mouse-motion session is a top reese84
// behavioral bot tell. Feeds a human-looking trajectory to the challenge's own
// mouse listeners. References no `fp` fields; emitted unconditionally.
const MOUSE: &str = include_str!("patches/mouse.js");

/// Build the bootstrap script for the spoofed profile.
///
/// `identity` supplies coherent UA-CH metadata and the *real* probed Chrome
/// version; `persona` overrides simple identity fields (platform / hardware /
/// locale) and drives the new fingerprint surfaces. Splitting the two keeps
/// the UA coherent: when the persona changes platform we rebuild the UA-CH
/// metadata against the real Chrome version instead of a stale fallback.
///
/// Order: identity IIFE first (webdriver-most-probed-first, navigator_props
/// last), then the PRNG definition, then each persona surface.
#[must_use]
pub fn bootstrap_script(persona: &Persona, identity: &Fingerprint) -> String {
    bootstrap_script_impl(persona, identity, true)
}

/// Like [`bootstrap_script`], but omits the WebGL vendor/renderer
/// value-substitution patch (`patches/webgl.js`) entirely — the host's real
/// `WebGLRenderingContext.getParameter`/`getSupportedExtensions` values pass
/// through unpatched instead of the coherent ANGLE/Direct3D11 Intel identity
/// the patch spoofs by default.
///
/// To keep WebGL and WebGPU reporting the *same* (real) GPU, the WebGPU
/// **value** adapter spoof is omitted here too — otherwise a spoofed
/// `navigator.gpu` adapter (derived from the WebGL renderer this variant does
/// not apply) would disagree with the real WebGL renderer, a cross-API
/// coherence tell. An explicit WebGPU `Block` (hiding `navigator.gpu`) is
/// renderer-neutral and is still honored. Every other patch (identity IIFE,
/// noise surfaces, fonts, hardware, webrtc, screen/mouse coherence) is
/// unaffected. See
/// [`StealthProfile::native_isolation`](crate::StealthProfile::native_isolation)
/// for the caller-facing trade-off this backs.
#[must_use]
pub fn bootstrap_script_native_webgl(persona: &Persona, identity: &Fingerprint) -> String {
    bootstrap_script_impl(persona, identity, false)
}

/// Shared implementation for [`bootstrap_script`] /
/// [`bootstrap_script_native_webgl`]. `spoof_webgl` gates the `push_webgl`
/// call and the WebGPU *value* spoof (which is skipped when WebGL is left
/// real, so the two APIs stay coherent — see [`push_webgpu`]); every other
/// patch is identical between the two public entry points.
fn bootstrap_script_impl(persona: &Persona, identity: &Fingerprint, spoof_webgl: bool) -> String {
    // Prelude first: installs the toString override + closure-local helpers
    // that every patch below routes through.
    let mut body = String::from(NATIVE);
    body.push('\n');
    body.push_str(&identity_iife(persona, identity));

    let seed = persona.seed.unwrap_or_else(Seed::random).value();

    // PRNG must be defined once, before any noise/font patch that references
    // `__zdRng`. Emit it unconditionally — cheap, and harmless if every
    // surface is Native (it just defines an unused function).
    body.push('\n');
    body.push_str(PRNG);

    // Geometry coherence runs unconditionally (no persona spec) — it repairs the
    // outer*/avail* props that the CDP metrics override cannot reach.
    body.push('\n');
    body.push_str(SCREEN);
    // Synthetic pointer entropy, also unconditional.
    body.push('\n');
    body.push_str(MOUSE);

    push_noise(
        &mut body,
        Surface::Canvas,
        persona.canvas.as_ref(),
        CANVAS,
        seed,
    );
    push_noise(
        &mut body,
        Surface::Audio,
        persona.audio.as_ref(),
        AUDIO,
        seed,
    );
    push_noise(
        &mut body,
        Surface::ClientRects,
        persona.client_rects.as_ref(),
        CLIENT_RECTS,
        seed,
    );

    if spoof_webgl {
        push_webgl(&mut body, persona.webgl.as_ref());
    }
    push_webgpu(
        &mut body,
        persona.webgpu.as_ref(),
        persona.webgl.as_ref(),
        spoof_webgl,
    );
    push_fonts(&mut body, persona.fonts.as_ref(), seed);
    push_hardware(&mut body, persona.hardware.as_ref());
    push_webrtc(&mut body, persona.webrtc.as_ref());

    // Single outer IIFE: helpers stay closure-local (no globalThis leak); the
    // Function.prototype.toString override inside still persists globally.
    format!("(function(){{\n{body}\n}})();")
}

/// Emit the 9 identity patches wrapped in `(function(fp){ ... })(fpJson)`.
/// Identity is resolved from `identity` with `persona` overrides applied,
/// preserving UA coherence (see [`bootstrap_script`]).
fn identity_iife(persona: &Persona, identity: &Fingerprint) -> String {
    let platform = persona.platform.unwrap_or(identity.platform);

    // If the persona changes the platform, rebuild UA-CH metadata coherently
    // against the REAL probed Chrome version (never a fallback) so platform +
    // platformVersion + brands all agree. Otherwise reuse the probed metadata.
    let uam = if persona.platform.is_some_and(|p| p != identity.platform) {
        UserAgentMetadata::realistic(platform, identity.chrome_major, &identity.chrome_full)
    } else {
        identity.ua_metadata.clone()
    };

    let cpu = persona.hardware_concurrency.unwrap_or(identity.cpu_count);
    let mem = persona.device_memory_gb.unwrap_or(identity.memory_gb);
    let languages = crate::lang::resolve_languages(persona, identity);

    let fp_json = json!({
        "platformJs":      platform.js_string(),
        "chPlatform":      platform.ch_platform(),
        "platformVersion": uam.platform_version,
        "cpuCount":        cpu,
        "memoryGb":        mem,
        "languages":       languages,
        "architecture":    uam.architecture,
        "bitness":         uam.bitness,
        "brands":          uam.brands,
        "fullVersionList": uam.full_version_list,
    });

    format!(
        "(function(fp){{\n{WEBDRIVER}\n{PLUGINS}\n{CHROME}\n{PERMISSIONS}\n{CODECS}\n{NAVIGATOR_PROPS}\n{USER_AGENT_DATA}\n{BROKEN_IMAGE}\n}})({fp_json});",
    )
}

/// JS source for the resolved seed token of a noise surface, or `None` when
/// the surface should be omitted entirely (`Native`).
fn seed_token(strat: Strategy, seed: u64) -> Option<String> {
    match strat {
        Strategy::Native => None,
        // Constant zero seed → every farble step deterministic & uniform; the
        // `/*BLOCK*/` marker documents intent and lets tests assert on it.
        Strategy::Block => Some("0/*BLOCK*/".to_string()),
        Strategy::Seeded | Strategy::Value => Some(seed.to_string()),
        Strategy::Random => Some("(Math.random()*4294967296)>>>0".to_string()),
    }
}

/// Append a noise-surface patch (`SEED` token), respecting the resolved
/// strategy. No-op under `Native`.
fn push_noise(out: &mut String, surface: Surface, cfg: Option<&SurfaceCfg>, js: &str, seed: u64) {
    let strat = surface.resolve_strategy(cfg.and_then(|c| c.strategy));
    if let Some(tok) = seed_token(strat, seed) {
        out.push('\n');
        out.push_str(&js.replace("SEED", &tok));
    }
}

/// JSON literal for an optional string value, or JS `null` when absent.
fn json_or_null(v: Option<&String>) -> String {
    v.map_or_else(
        || "null".to_string(),
        |s| serde_json::to_string(s).unwrap_or_else(|_| "null".to_string()),
    )
}

/// Append the webgl value-substitution patch. The existing hardcoded webgl
/// block (the first half of `webgl.js`) always runs; the appended IIFE's
/// `WEBGL_VENDOR` / `WEBGL_RENDERER` args carry the persona values (or JS
/// `null` when absent / `Native`, leaving the hardcoded block in charge).
fn push_webgl(out: &mut String, spec: Option<&WebglSpec>) {
    let strat = Surface::Webgl.resolve_strategy(spec.and_then(|s| s.strategy));
    let (vendor, renderer) = match strat {
        // Under Native the persona contributes nothing — pass null so the new
        // IIFE delegates entirely to the hardcoded block (which still runs).
        Strategy::Native => ("null".to_string(), "null".to_string()),
        _ => (
            json_or_null(spec.and_then(|s| s.unmasked_vendor.as_ref())),
            json_or_null(spec.and_then(|s| s.unmasked_renderer.as_ref())),
        ),
    };
    out.push('\n');
    out.push_str(
        &WEBGL
            .replace("WEBGL_VENDOR", &vendor)
            .replace("WEBGL_RENDERER", &renderer),
    );
}

/// Append the WebGPU coherence patch. Adapter info defaults to values derived
/// from the persona's WebGL renderer (or the hardcoded Intel default the
/// webgl block falls back to), so navigator.gpu agrees with WebGL — unless
/// the caller's [`WebgpuSpec`] explicitly overrides `vendor`/`architecture`.
/// Omitted under `Native`.
///
/// `spoof_webgl` reflects whether the WebGL value patch ran. When it is
/// `false` (the native-WebGL opt-in — [`bootstrap_script_native_webgl`]) the
/// real WebGL renderer passes through unpatched, so a spoofed WebGPU *value*
/// adapter — derived from the WebGL renderer we did NOT apply, or the
/// hardcoded default below — would disagree with the real GPU. That cross-API
/// mismatch is the exact coherence tell the opt-in exists to avoid, so the
/// value spoof (and any fabrication) is skipped too and the real
/// `navigator.gpu` adapter passes through. An explicit `Block` (hide
/// `navigator.gpu`) is renderer-neutral and stays honored regardless.
fn push_webgpu(
    out: &mut String,
    spec: Option<&WebgpuSpec>,
    webgl: Option<&WebglSpec>,
    spoof_webgl: bool,
) {
    use crate::persona::webgpu_adapter::adapter_for_renderer;
    let strat = Surface::Webgpu.resolve_strategy(spec.and_then(|s| s.strategy));
    if strat == Strategy::Native {
        return;
    }
    // Native-WebGL renderer coherence: skip the value adapter spoof (and any
    // fabrication) when the real WebGL renderer is left unpatched (see the
    // doc comment above). A `Block` is renderer-neutral, so it is still
    // emitted.
    if !spoof_webgl && strat != Strategy::Block {
        return;
    }

    if strat == Strategy::Block {
        out.push('\n');
        out.push_str(
            &WEBGPU
                .replace("WEBGPU_VENDOR", "null")
                .replace("WEBGPU_ARCHITECTURE", "null")
                .replace("WEBGPU_DEVICE", "null")
                .replace("WEBGPU_DESCRIPTION", "null")
                .replace("WEBGPU_LIMITS", "null")
                .replace("WEBGPU_FEATURES", "null")
                .replace("WEBGPU_MODE", "\"block\"")
                .replace("WEBGPU_FABRICATE", "false"),
        );
        return;
    }

    const DEFAULT_RENDERER: &str =
        "ANGLE (Intel, Intel(R) UHD Graphics 630 Direct3D11 vs_5_0 ps_5_0, D3D11)";
    let renderer = webgl
        .and_then(|w| w.unmasked_renderer.as_deref())
        .unwrap_or(DEFAULT_RENDERER);
    let derived = adapter_for_renderer(renderer);

    let vendor = spec
        .and_then(|s| s.vendor.clone())
        .unwrap_or(derived.vendor);
    let architecture = spec
        .and_then(|s| s.architecture.clone())
        .unwrap_or(derived.architecture);
    let device = spec.and_then(|s| s.device.clone()).unwrap_or_default();
    let description = spec.and_then(|s| s.description.clone()).unwrap_or_default();
    let limits = spec.and_then(|s| s.limits.as_ref());
    let features = spec.and_then(|s| s.features.as_ref());
    // Fabrication is only wired up when the caller opted in AND supplied
    // enough to fabricate coherently (vendor + limits) — a bare
    // `fabricate_when_absent: true` with nothing else is refused (no-op):
    // this project never auto-invents fingerprint values (see `WebgpuSpec`
    // rustdoc).
    let fabricate = spec.is_some_and(|s| {
        s.fabricate_when_absent == Some(true) && s.vendor.is_some() && s.limits.is_some()
    });

    out.push('\n');
    out.push_str(
        &WEBGPU
            .replace(
                "WEBGPU_VENDOR",
                &serde_json::to_string(&vendor).unwrap_or_else(|_| "null".into()),
            )
            .replace(
                "WEBGPU_ARCHITECTURE",
                &serde_json::to_string(&architecture).unwrap_or_else(|_| "null".into()),
            )
            .replace(
                "WEBGPU_DEVICE",
                &serde_json::to_string(&device).unwrap_or_else(|_| "\"\"".into()),
            )
            .replace(
                "WEBGPU_DESCRIPTION",
                &serde_json::to_string(&description).unwrap_or_else(|_| "\"\"".into()),
            )
            .replace(
                "WEBGPU_LIMITS",
                &limits.map_or_else(
                    || "null".to_string(),
                    |l| serde_json::to_string(l).unwrap_or_else(|_| "null".into()),
                ),
            )
            .replace(
                "WEBGPU_FEATURES",
                &features.map_or_else(
                    || "null".to_string(),
                    |f| serde_json::to_string(f).unwrap_or_else(|_| "null".into()),
                ),
            )
            .replace("WEBGPU_MODE", "\"value\"")
            .replace("WEBGPU_FABRICATE", if fabricate { "true" } else { "false" }),
    );
}

/// Append the fonts patch (`FONT_ALLOW` + `SEED`). Omitted under `Native`.
fn push_fonts(out: &mut String, spec: Option<&FontSpec>, seed: u64) {
    let strat = Surface::Fonts.resolve_strategy(spec.and_then(|s| s.strategy));
    let seed_tok = match seed_token(strat, seed) {
        None => return, // Native → omit entirely.
        Some(t) => t,
    };
    // Allow-list only meaningful when the persona supplies one AND we're not
    // blocking; under Block, hide every font (empty allow-list).
    let allow = match strat {
        Strategy::Block => "[]".to_string(),
        _ => spec.and_then(|s| s.available.as_ref()).map_or_else(
            || "null".to_string(),
            |v| serde_json::to_string(v).unwrap_or_else(|_| "null".to_string()),
        ),
    };
    out.push('\n');
    out.push_str(
        &FONTS
            .replace("FONT_ALLOW", &allow)
            .replace("SEED", &seed_tok),
    );
}

/// Append the hardware patch (`HW_BATTERY` / `HW_MEDIA_DEVICES` / `HW_VOICES`).
/// Omitted under `Native`. Each field independently `null` when unset.
fn push_hardware(out: &mut String, spec: Option<&HardwareSpec>) {
    let strat = Surface::Hardware.resolve_strategy(spec.and_then(|s| s.strategy));
    if strat == Strategy::Native {
        return;
    }
    let (battery, media, voices) = if strat == Strategy::Block {
        // Block → present a uniform, minimal hardware surface.
        ("1".to_string(), "0".to_string(), "[]".to_string())
    } else {
        let battery = spec
            .and_then(|s| s.battery_level)
            .map_or_else(|| "null".to_string(), |b| b.to_string());
        let media = spec
            .and_then(|s| s.media_devices)
            .map_or_else(|| "null".to_string(), |m| m.to_string());
        let voices = spec.and_then(|s| s.speech_voices.as_ref()).map_or_else(
            || "null".to_string(),
            |v| serde_json::to_string(v).unwrap_or_else(|_| "null".to_string()),
        );
        (battery, media, voices)
    };
    out.push('\n');
    out.push_str(
        &HARDWARE
            .replace("HW_BATTERY", &battery)
            .replace("HW_MEDIA_DEVICES", &media)
            .replace("HW_VOICES", &voices),
    );
}

/// Append the webrtc policy patch (`WEBRTC_POLICY` + `WEBRTC_FAKE_IP`).
/// The patch self-no-ops on the `"native"` policy, so we always emit it but
/// pass the strategy-derived policy string.
fn push_webrtc(out: &mut String, spec: Option<&WebrtcSpec>) {
    let strat = Surface::Webrtc.resolve_strategy(spec.and_then(|s| s.strategy));
    let policy = match strat {
        Strategy::Native => "native",
        Strategy::Value => "value",
        // Block (the policy default) + any non-meaningful fallback.
        _ => "block",
    };
    let fake_ip = json_or_null(spec.and_then(|s| s.fake_ip.as_ref()));
    out.push('\n');
    out.push_str(
        &WEBRTC
            .replace("WEBRTC_POLICY", &format!("\"{policy}\""))
            .replace("WEBRTC_FAKE_IP", &fake_ip),
    );
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::persona::surface::Strategy;
    use crate::{Platform, UserAgentMetadata};

    fn mock_identity() -> Fingerprint {
        Fingerprint {
            platform: Platform::MacIntel,
            chrome_major: 120,
            chrome_full: "120.0.6099.234".into(),
            cpu_count: 10,
            memory_gb: 8,
            ua_string: String::new(),
            ua_metadata: UserAgentMetadata::realistic(Platform::MacIntel, 120, "120.0.6099.234"),
            timezone: None,
            locale: Some("en-US".into()),
            languages: None,
            screen: None,
        }
    }

    // --- Migrated identity tests (the original three) --------------------

    #[test]
    fn bootstrap_includes_all_nine_patches() {
        let s = bootstrap_script(&Persona::default(), &mock_identity());
        assert!(s.contains("webdriver"), "webdriver patch missing");
        assert!(s.contains("PluginArray"), "plugins patch missing");
        assert!(s.contains("window.chrome"), "chrome patch missing");
        assert!(
            s.contains("UNMASKED_VENDOR_WEBGL") || s.contains("37445"),
            "webgl patch missing"
        );
        assert!(
            s.contains("Notification.permission"),
            "permissions patch missing"
        );
        assert!(s.contains("canPlayType"), "codecs patch missing");
        assert!(
            s.contains("hardwareConcurrency"),
            "navigator_props patch missing"
        );
        assert!(s.contains("userAgentData"), "user_agent_data patch missing");
        assert!(s.contains("naturalWidth"), "broken_image patch missing");
    }

    #[test]
    fn bootstrap_is_an_iife_taking_fp() {
        let s = bootstrap_script(&Persona::default(), &mock_identity());
        // The identity patches still run inside `(function(fp){…})({…})`, now
        // nested within the outer masking IIFE.
        assert!(s.contains("(function(fp){"));
        assert!(s.contains("})({"), "fp arg JSON should follow");
    }

    #[test]
    fn bootstrap_installs_native_masking_prelude() {
        let s = bootstrap_script(&Persona::default(), &mock_identity());
        assert!(s.contains("__zdReplace"), "replace helper missing");
        assert!(s.contains("__zdGetter"), "getter helper missing");
        assert!(s.contains("__zdMark"), "mark helper missing");
        assert!(
            s.contains("Function.prototype, \"toString\""),
            "toString override missing"
        );
    }

    #[test]
    fn bootstrap_wraps_everything_in_outer_masking_iife() {
        let s = bootstrap_script(&Persona::default(), &mock_identity());
        assert!(
            s.starts_with("(function(){"),
            "outer masking IIFE must be first"
        );
        // identity IIFE is now nested inside the outer one.
        assert!(
            s.contains("(function(fp){"),
            "identity IIFE still present (nested)"
        );
        assert!(s.trim_end().ends_with("})();"), "outer IIFE is invoked");
    }

    #[test]
    fn bootstrap_substitutes_platform_js_string() {
        let s = bootstrap_script(&Persona::default(), &mock_identity());
        assert!(s.contains("\"MacIntel\""));
    }

    // --- Identity coherence ---------------------------------------------

    #[test]
    fn persona_platform_override_rebuilds_uam_with_real_chrome_version() {
        // identity is mac/chrome120; persona overrides platform to Win32.
        // The rebuilt UA-CH must use the REAL chrome version (120) and the
        // Win32 platformVersion (15.0.0), not a fallback.
        let persona = Persona {
            platform: Some(Platform::Win32),
            ..Persona::default()
        };
        let s = bootstrap_script(&persona, &mock_identity());
        assert!(s.contains("\"Win32\""), "platformJs should be Win32");
        assert!(
            s.contains("15.0.0"),
            "Win32 platformVersion should be present"
        );
        // chrome 120 brands carried through from the real identity.
        assert!(s.contains("\"120\""), "real chrome major should survive");
    }

    #[test]
    fn persona_overrides_simple_identity_fields() {
        let persona = Persona {
            hardware_concurrency: Some(4),
            device_memory_gb: Some(16),
            locale: Some("fr-FR".into()),
            ..Persona::default()
        };
        let s = bootstrap_script(&persona, &mock_identity());
        assert!(s.contains("\"cpuCount\":4"), "cpu override missing");
        // deviceMemory is rendered into the fp json untouched here (the JS
        // clamps); persona value should appear.
        assert!(s.contains("\"memoryGb\":16"), "memory override missing");
        assert!(s.contains("fr-FR"), "locale override missing");
    }

    // --- Surface strategy emission (new) --------------------------------

    #[test]
    fn native_strategy_omits_surface_patch() {
        let p = Persona {
            canvas: Some(SurfaceCfg {
                strategy: Some(Strategy::Native),
            }),
            seed: Some(Seed::from_u64(1)),
            ..Persona::default()
        };
        let script = bootstrap_script(&p, &mock_identity());
        assert!(
            !script.contains("origGetImageData"),
            "Native canvas → no farble hook"
        );
    }

    #[test]
    fn block_strategy_emits_constant_canvas() {
        let p = Persona {
            canvas: Some(SurfaceCfg {
                strategy: Some(Strategy::Block),
            }),
            ..Persona::default()
        };
        let script = bootstrap_script(&p, &mock_identity());
        // canvas patch IS emitted (hook present) but seeded with the BLOCK
        // marker rather than a live seed.
        assert!(
            script.contains("origGetImageData"),
            "Block canvas still hooks getImageData"
        );
        assert!(script.contains("BLOCK"), "Block canvas marker present");
    }

    #[test]
    fn seeded_canvas_templates_seed() {
        let p = Persona {
            canvas: Some(SurfaceCfg {
                strategy: Some(Strategy::Seeded),
            }),
            seed: Some(Seed::from_u64(123)),
            ..Persona::default()
        };
        let script = bootstrap_script(&p, &mock_identity());
        assert!(
            script.contains("origGetImageData"),
            "canvas farble must hook getImageData"
        );
        // The seed `123` is substituted as the IIFE arg: `})(123);`
        assert!(
            script.contains("})(123)"),
            "seed value must be templated in"
        );
    }

    #[test]
    fn random_canvas_uses_math_random_seed() {
        let p = Persona {
            canvas: Some(SurfaceCfg {
                strategy: Some(Strategy::Random),
            }),
            ..Persona::default()
        };
        let script = bootstrap_script(&p, &mock_identity());
        assert!(
            script.contains("Math.random()*4294967296"),
            "Random canvas re-seeds per page"
        );
    }

    #[test]
    fn webgl_value_substitutes_persona_renderer() {
        let p = Persona {
            webgl: Some(WebglSpec {
                strategy: Some(Strategy::Value),
                unmasked_vendor: Some("Google Inc. (NVIDIA)".into()),
                unmasked_renderer: Some("ANGLE (NVIDIA GeForce RTX 4090)".into()),
            }),
            ..Persona::default()
        };
        let script = bootstrap_script(&p, &mock_identity());
        assert!(
            script.contains("ANGLE (NVIDIA GeForce RTX 4090)"),
            "persona renderer must be substituted into the webgl IIFE"
        );
        assert!(
            script.contains("Google Inc. (NVIDIA)"),
            "persona vendor must be substituted into the webgl IIFE"
        );
    }

    #[test]
    fn webgl_native_passes_null_and_keeps_hardcoded_block() {
        // No webgl spec → Native resolution → IIFE args null, but the
        // hardcoded fallback block (37445/37446) still present.
        let script = bootstrap_script(&Persona::default(), &mock_identity());
        assert!(
            s_has_webgl_iife_null(&script),
            "webgl IIFE args should be null"
        );
        assert!(
            script.contains("37445"),
            "hardcoded webgl fallback block must remain"
        );
    }

    fn s_has_webgl_iife_null(s: &str) -> bool {
        // The appended webgl IIFE ends with its args; null both => `})(null, null);`
        s.contains("})(null, null);")
    }

    // --- opt-in native-WebGL bootstrap (Task 10) ----------------------------

    #[test]
    fn native_webgl_bootstrap_omits_webgl_patch_entirely() {
        // Unlike `Strategy::Native` (which still keeps the hardcoded
        // 37445/37446 fallback block, just with null persona args — see
        // `webgl_native_passes_null_and_keeps_hardcoded_block` above), the
        // profile-level opt-in must drop the whole webgl.js block — no
        // getParameter/getSupportedExtensions override at all.
        let s = bootstrap_script_native_webgl(&Persona::default(), &mock_identity());
        assert!(
            !s.contains("UNMASKED_VENDOR_WEBGL") && !s.contains("37445"),
            "native-webgl bootstrap must omit the hardcoded webgl fallback block entirely, got: {s}"
        );
        assert!(
            !s.contains("getSupportedExtensions"),
            "native-webgl bootstrap must not override getSupportedExtensions"
        );
    }

    #[test]
    fn native_webgl_bootstrap_still_spoofs_persona_webgl_vendor_when_supplied() {
        // Profile-level opt-in wins over ANY persona.webgl spec — even an
        // explicit `Value` strategy with vendor/renderer set must not leak
        // into the script when the caller asked for the real WebGL renderer.
        let p = Persona {
            webgl: Some(WebglSpec {
                strategy: Some(Strategy::Value),
                unmasked_vendor: Some("Google Inc. (NVIDIA)".into()),
                unmasked_renderer: Some("ANGLE (NVIDIA GeForce RTX 4090)".into()),
            }),
            ..Persona::default()
        };
        let s = bootstrap_script_native_webgl(&p, &mock_identity());
        assert!(
            !s.contains("ANGLE (NVIDIA GeForce RTX 4090)"),
            "native-webgl bootstrap must not substitute persona webgl values"
        );
        assert!(!s.contains("UNMASKED_VENDOR_WEBGL"));
    }

    #[test]
    fn native_webgl_bootstrap_keeps_every_other_patch() {
        // Only the webgl block is dropped — identity IIFE + other surfaces
        // are byte-for-byte what `bootstrap_script` would emit.
        let s = bootstrap_script_native_webgl(&Persona::default(), &mock_identity());
        assert!(s.contains("webdriver"), "webdriver patch missing");
        assert!(s.contains("PluginArray"), "plugins patch missing");
        assert!(s.contains("window.chrome"), "chrome patch missing");
        assert!(
            s.contains("Notification.permission"),
            "permissions patch missing"
        );
        assert!(s.contains("canPlayType"), "codecs patch missing");
        assert!(
            s.contains("hardwareConcurrency"),
            "navigator_props patch missing"
        );
        assert!(s.contains("userAgentData"), "user_agent_data patch missing");
        assert!(s.contains("naturalWidth"), "broken_image patch missing");
    }

    #[test]
    fn native_webgl_bootstrap_omits_webgpu_value_spoof_for_renderer_coherence() {
        // native_isolation leaves the real WebGL renderer unpatched. A spoofed
        // WebGPU *value* adapter — derived from the WebGL renderer we did NOT
        // apply, or the hardcoded Intel default — would disagree with the real
        // GPU. That cross-API mismatch is exactly the coherence tell this
        // opt-in exists to avoid, so the default (`Value`) WebGPU spoof must
        // ALSO be omitted, leaving the real `navigator.gpu` adapter through.
        let s = bootstrap_script_native_webgl(&Persona::default(), &mock_identity());
        assert!(
            !s.contains("GPUAdapter.prototype"),
            "native-webgl must omit the WebGPU value spoof for WebGL/WebGPU renderer coherence"
        );
    }

    #[test]
    fn native_webgl_bootstrap_still_honors_explicit_webgpu_block() {
        // A `Block` (hide `navigator.gpu`) is renderer-neutral — it removes the
        // API rather than reporting a mismatched adapter — so an explicit Block
        // stays honored under native-webgl.
        let p = Persona {
            webgpu: Some(WebgpuSpec {
                strategy: Some(Strategy::Block),
                ..Default::default()
            }),
            ..Persona::default()
        };
        let s = bootstrap_script_native_webgl(&p, &mock_identity());
        assert!(
            s.contains("navigator, 'gpu'"),
            "explicit webgpu Block should still shadow navigator.gpu under native-webgl"
        );
    }

    #[test]
    fn default_bootstrap_script_still_spoofs_webgl_regression_guard() {
        // Regression guard: the existing (unmodified) public entry point's
        // output is unaffected by adding `bootstrap_script_native_webgl`.
        let s = bootstrap_script(&Persona::default(), &mock_identity());
        assert!(s.contains("UNMASKED_VENDOR_WEBGL") || s.contains("37445"));
        assert!(s.contains("getSupportedExtensions"));
    }

    #[test]
    fn fonts_native_omits_measuretext_hook() {
        let p = Persona {
            fonts: Some(FontSpec {
                strategy: Some(Strategy::Native),
                available: None,
            }),
            ..Persona::default()
        };
        let script = bootstrap_script(&p, &mock_identity());
        assert!(
            !script.contains("measureText"),
            "Native fonts → no measureText hook"
        );
    }

    #[test]
    fn fonts_value_substitutes_allow_list() {
        let p = Persona {
            fonts: Some(FontSpec {
                strategy: Some(Strategy::Value),
                available: Some(vec!["Arial".into(), "Helvetica".into()]),
            }),
            seed: Some(Seed::from_u64(7)),
            ..Persona::default()
        };
        let script = bootstrap_script(&p, &mock_identity());
        assert!(script.contains("measureText"), "fonts patch emitted");
        assert!(
            script.contains("[\"Arial\",\"Helvetica\"]"),
            "allow-list substituted"
        );
    }

    #[test]
    fn webrtc_value_substitutes_policy_and_ip() {
        let p = Persona {
            webrtc: Some(WebrtcSpec {
                strategy: Some(Strategy::Value),
                fake_ip: Some("203.0.113.7".into()),
            }),
            ..Persona::default()
        };
        let script = bootstrap_script(&p, &mock_identity());
        assert!(
            script.contains("})(\"value\", \"203.0.113.7\");"),
            "webrtc value policy + fake ip substituted"
        );
    }

    #[test]
    fn webrtc_default_is_block_policy() {
        let script = bootstrap_script(&Persona::default(), &mock_identity());
        assert!(
            script.contains("})(\"block\", null);"),
            "default webrtc policy is block with no fake ip"
        );
    }

    #[test]
    fn hardware_native_omits_patch() {
        let p = Persona {
            hardware: Some(HardwareSpec {
                strategy: Some(Strategy::Native),
                ..HardwareSpec::default()
            }),
            ..Persona::default()
        };
        let script = bootstrap_script(&p, &mock_identity());
        assert!(
            !script.contains("getBattery"),
            "Native hardware → no getBattery hook"
        );
    }

    #[test]
    fn hardware_value_substitutes_specs() {
        let p = Persona {
            hardware: Some(HardwareSpec {
                strategy: Some(Strategy::Value),
                battery_level: Some(0.5),
                media_devices: Some(3),
                speech_voices: Some(vec!["Daniel".into()]),
            }),
            ..Persona::default()
        };
        let script = bootstrap_script(&p, &mock_identity());
        assert!(script.contains("getBattery"), "hardware patch emitted");
        assert!(
            script.contains("})(0.5, 3, [\"Daniel\"]);"),
            "hardware specs substituted"
        );
    }

    #[test]
    fn prng_defined_once() {
        let p = Persona {
            canvas: Some(SurfaceCfg {
                strategy: Some(Strategy::Seeded),
            }),
            audio: Some(SurfaceCfg {
                strategy: Some(Strategy::Seeded),
            }),
            ..Persona::default()
        };
        let script = bootstrap_script(&p, &mock_identity());
        assert_eq!(
            script.matches("function __zdRng(").count(),
            1,
            "PRNG defined exactly once even with multiple noise surfaces"
        );
    }

    #[test]
    fn webgpu_value_substitutes_coherent_adapter_from_renderer() {
        let p = Persona {
            webgl: Some(WebglSpec {
                strategy: Some(Strategy::Value),
                unmasked_vendor: Some("Google Inc. (Apple)".into()),
                unmasked_renderer: Some(
                    "ANGLE (Apple, ANGLE Metal Renderer: Apple M4 Pro, Unspecified Version)".into(),
                ),
            }),
            ..Persona::default()
        };
        let s = bootstrap_script(&p, &mock_identity());
        assert!(
            s.contains("GPUAdapter.prototype"),
            "webgpu patch emitted by default (Value)"
        );
        assert!(
            s.contains("\"apple\""),
            "coherent vendor derived from renderer"
        );
        assert!(
            s.contains("\"metal-3\""),
            "validated architecture for Apple Metal (real Chrome probe)"
        );
    }

    #[test]
    fn webgpu_objects_built_via_prototype_helper_for_instanceof() {
        // GPU objects must be built through the __zdGpuProto prototype helper so
        // `instanceof` holds — not the old bare `var info = { vendor: ... }`
        // object literal. All three helper calls are static template text
        // (present regardless of the runtime fabricate flag).
        let s = bootstrap_script(&Persona::default(), &mock_identity());
        assert!(
            s.contains("__zdGpuProto('GPUAdapterInfo')"),
            "info must inherit GPUAdapterInfo.prototype: {s}"
        );
        assert!(
            s.contains("__zdGpuProto('GPUAdapter')"),
            "fabricated adapter must inherit GPUAdapter.prototype"
        );
        assert!(
            s.contains("__zdGpuProto('GPU')"),
            "synthetic navigator.gpu must inherit GPU.prototype"
        );
        assert!(
            !s.contains("var info = { vendor:"),
            "old plain-object info literal must be gone"
        );
    }

    #[test]
    fn webgpu_none_everywhere_is_byte_for_byte_regression_guard() {
        // A `WebgpuSpec::default()` (all-None) must produce the exact same
        // bootstrap output as no spec at all (`persona.webgpu = None`) — the
        // regression guard the promotion from `SurfaceCfg` must preserve.
        // Pin the seed explicitly — `Persona::default()` draws a fresh random
        // seed per call, which would make two independent bootstraps differ
        // in their (unrelated) canvas/audio/clientRects noise regardless of
        // the webgpu change under test.
        let base_persona = Persona {
            seed: Some(Seed::from_u64(42)),
            ..Persona::default()
        };
        let baseline = bootstrap_script(&base_persona, &mock_identity());
        let p = Persona {
            webgpu: Some(WebgpuSpec::default()),
            ..base_persona
        };
        let with_default_spec = bootstrap_script(&p, &mock_identity());
        assert_eq!(
            baseline, with_default_spec,
            "WebgpuSpec::default() must be byte-for-byte identical to no spec"
        );
    }

    #[test]
    fn webgpu_caller_vendor_overrides_derived_value() {
        // Explicit `vendor`/`architecture` win over the WebGL-derived default,
        // even though a WebGL renderer that would derive something else is
        // also present.
        let p = Persona {
            webgl: Some(WebglSpec {
                strategy: Some(Strategy::Value),
                unmasked_renderer: Some("NVIDIA GeForce RTX 4090".into()),
                ..Default::default()
            }),
            webgpu: Some(WebgpuSpec {
                vendor: Some("caller-vendor".into()),
                architecture: Some("caller-arch".into()),
                device: Some("caller-device".into()),
                description: Some("caller-description".into()),
                ..Default::default()
            }),
            ..Persona::default()
        };
        let s = bootstrap_script(&p, &mock_identity());
        assert!(
            s.contains("\"caller-vendor\""),
            "explicit vendor overrides derived nvidia"
        );
        assert!(s.contains("\"caller-arch\""), "explicit architecture wins");
        assert!(s.contains("\"caller-device\""), "explicit device emitted");
        assert!(
            s.contains("\"caller-description\""),
            "explicit description emitted"
        );
        assert!(
            !s.contains("\"nvidia\""),
            "derived vendor must not leak through when overridden"
        );
    }

    #[test]
    fn webgpu_limits_and_features_decorate_when_supplied() {
        let mut limits = std::collections::BTreeMap::new();
        limits.insert("maxTextureDimension2D".to_string(), 16384u64);
        let p = Persona {
            webgpu: Some(WebgpuSpec {
                limits: Some(limits),
                features: Some(vec!["texture-compression-bc".into()]),
                ..Default::default()
            }),
            ..Persona::default()
        };
        let s = bootstrap_script(&p, &mock_identity());
        // The `__zdGetter(GPUAdapter.prototype, 'limits'/'features' ...)`
        // JS SOURCE lines are static template text emitted whenever the
        // webgpu patch runs at all (the `if (limits) {...}` gating is a
        // RUNTIME check inside that source, not a Rust-side omission) — so
        // asserting their presence wouldn't distinguish "supplied" from
        // "absent". What Rust actually varies is the substituted argument
        // values passed into the IIFE; assert those instead.
        assert!(
            s.contains(
                r#"{"maxTextureDimension2D":16384}, ["texture-compression-bc"], "value", false);"#
            ),
            "limits + features substituted into the invocation args: {s}"
        );
    }

    #[test]
    fn webgpu_limits_and_features_absent_pass_null() {
        let s = bootstrap_script(&Persona::default(), &mock_identity());
        assert!(
            s.contains(r#", null, null, "value", false);"#),
            "limits/features args are JS null when unset: {s}"
        );
    }

    #[test]
    fn webgpu_fabricate_requires_vendor_and_limits_else_noop() {
        // `fabricate_when_absent: true` alone (no vendor, no limits) is
        // refused — no auto-invented fingerprint values. The JS fabrication
        // branch is static source text either way (guarded by a runtime `if
        // (!fabricate) return;`), so what must be checked is the substituted
        // trailing `fabricate` argument itself, not source-line presence.
        let p = Persona {
            webgpu: Some(WebgpuSpec {
                fabricate_when_absent: Some(true),
                ..Default::default()
            }),
            ..Persona::default()
        };
        let s = bootstrap_script(&p, &mock_identity());
        assert!(
            s.contains("\"value\", false);"),
            "fabricate arg must be false (no-op) without vendor+limits: {s}"
        );
        assert!(!s.contains("\"value\", true);"));

        // vendor alone (no limits) is still refused.
        let p2 = Persona {
            webgpu: Some(WebgpuSpec {
                vendor: Some("nvidia".into()),
                fabricate_when_absent: Some(true),
                ..Default::default()
            }),
            ..Persona::default()
        };
        let s2 = bootstrap_script(&p2, &mock_identity());
        assert!(
            s2.contains("\"value\", false);"),
            "fabricate arg must be false (no-op) with vendor but no limits: {s2}"
        );
        assert!(!s2.contains("\"value\", true);"));
    }

    #[test]
    fn webgpu_fabricate_wires_requestadapter_when_vendor_and_limits_set() {
        let mut limits = std::collections::BTreeMap::new();
        limits.insert("maxBufferSize".to_string(), 1_073_741_824u64);
        let p = Persona {
            webgpu: Some(WebgpuSpec {
                vendor: Some("apple".into()),
                architecture: Some("metal-3".into()),
                limits: Some(limits),
                fabricate_when_absent: Some(true),
                ..Default::default()
            }),
            ..Persona::default()
        };
        let s = bootstrap_script(&p, &mock_identity());
        assert!(
            s.contains(r#"{"maxBufferSize":1073741824}, null, "value", true);"#),
            "fabricate arg substituted true when vendor+limits both set: {s}"
        );
        assert!(
            s.contains("NotSupportedError"),
            "synthetic adapter's requestDevice rejects (v1 limitation)"
        );
        // The fabricate patch must carry BOTH runtime branches: (a) wrap
        // GPU.prototype.requestAdapter when navigator.gpu exists, and (b)
        // DEFINE a synthetic navigator.gpu on Navigator.prototype when it's
        // entirely absent (the `--disable-gpu` GPU-less case this feature
        // exists for). Guards against either branch being dropped.
        assert!(
            s.contains("GPU.prototype, 'requestAdapter'"),
            "case (a): wraps requestAdapter when navigator.gpu is present: {s}"
        );
        assert!(
            s.contains("Navigator.prototype, 'gpu'"),
            "case (b): defines synthetic navigator.gpu when entirely absent: {s}"
        );
        assert!(
            s.contains("getPreferredCanvasFormat"),
            "synthetic navigator.gpu exposes getPreferredCanvasFormat for coherence: {s}"
        );
    }

    #[test]
    fn webgpu_block_deletes_navigator_gpu() {
        let p = Persona {
            webgpu: Some(WebgpuSpec {
                strategy: Some(Strategy::Block),
                ..Default::default()
            }),
            ..Persona::default()
        };
        let s = bootstrap_script(&p, &mock_identity());
        assert!(s.contains("\"block\""), "block mode token substituted");
        assert!(
            s.contains("navigator, 'gpu'"),
            "block mode shadows navigator.gpu"
        );
    }

    #[test]
    fn webgpu_native_passes_null_vendor() {
        let p = Persona {
            webgpu: Some(WebgpuSpec {
                strategy: Some(Strategy::Native),
                ..Default::default()
            }),
            ..Persona::default()
        };
        let s = bootstrap_script(&p, &mock_identity());
        // Native → no webgpu patch emitted at all.
        assert!(!s.contains("WEBGPU_VENDOR"), "no unsubstituted token");
        assert!(
            !s.contains("GPUAdapter.prototype"),
            "Native webgpu omits the patch"
        );
    }

    #[test]
    fn navigator_languages_derives_base_lang_not_en() {
        let persona = Persona {
            locale: Some("fr-FR".into()),
            ..Default::default()
        };
        let identity = mock_identity(); // existing helper in this test module
        let script = bootstrap_script(&persona, &identity);
        assert!(
            script.contains(r#"["fr-FR","fr"]"#) || script.contains(r#"["fr-FR", "fr"]"#),
            "languages should derive base lang, not hardcode en: {script}"
        );
        assert!(
            !script.contains(r#"["fr-FR","en"]"#),
            "must not hardcode en"
        );
    }

    #[test]
    fn identity_patches_route_through_masking_helpers() {
        let s = bootstrap_script(&Persona::default(), &mock_identity());
        assert!(
            s.contains("__zdGetter(Navigator.prototype, 'webdriver'"),
            "webdriver"
        );
        assert!(
            s.contains("__zdGetter(Navigator.prototype, 'plugins'"),
            "plugins getter"
        );
        assert!(
            s.contains("__zdReplace"),
            "permissions/codecs methods routed"
        );
        assert!(s.contains("__zdMark"), "value-fn members marked");
        // No raw defineProperty getter on Navigator.prototype.webdriver remains.
        assert!(
            !s.contains("Object.defineProperty(Navigator.prototype, 'webdriver'"),
            "webdriver should go through __zdGetter, not raw defineProperty"
        );
    }

    #[test]
    fn surface_patches_route_through_masking_helpers() {
        let p = Persona {
            webgl: Some(WebglSpec {
                strategy: Some(Strategy::Value),
                unmasked_vendor: Some("Google Inc. (NVIDIA)".into()),
                unmasked_renderer: Some("ANGLE (NVIDIA GeForce RTX 4090)".into()),
            }),
            canvas: Some(SurfaceCfg {
                strategy: Some(Strategy::Seeded),
            }),
            webgpu: Some(WebgpuSpec {
                strategy: Some(Strategy::Value),
                ..Default::default()
            }),
            seed: Some(Seed::from_u64(1)),
            ..Persona::default()
        };
        let s = bootstrap_script(&p, &mock_identity());
        assert!(
            s.contains("__zdReplace(WebGLRenderingContext.prototype, 'getParameter'")
                || s.contains("__zdReplace(proto, 'getParameter'"),
            "webgl routed"
        );
        assert!(s.contains("__zdReplace"), "canvas/getImageData routed");
        assert!(
            s.contains("__zdGetter(GPUAdapter.prototype, 'info'"),
            "webgpu info getter routed"
        );
        // tokens still substituted, not left raw:
        assert!(
            !s.contains("SEED") && !s.contains("WEBGL_VENDOR"),
            "tokens substituted"
        );
    }

    #[test]
    fn no_unsubstituted_tokens_remain() {
        // Exercise every surface so all token-bearing patches are emitted.
        let p = Persona {
            canvas: Some(SurfaceCfg {
                strategy: Some(Strategy::Seeded),
            }),
            audio: Some(SurfaceCfg {
                strategy: Some(Strategy::Random),
            }),
            client_rects: Some(SurfaceCfg {
                strategy: Some(Strategy::Block),
            }),
            webgl: Some(WebglSpec {
                strategy: Some(Strategy::Value),
                unmasked_vendor: Some("V".into()),
                unmasked_renderer: Some("R".into()),
            }),
            fonts: Some(FontSpec {
                strategy: Some(Strategy::Value),
                available: Some(vec!["Arial".into()]),
            }),
            webrtc: Some(WebrtcSpec {
                strategy: Some(Strategy::Value),
                fake_ip: Some("1.2.3.4".into()),
            }),
            hardware: Some(HardwareSpec {
                strategy: Some(Strategy::Value),
                battery_level: Some(0.9),
                media_devices: Some(2),
                speech_voices: Some(vec!["A".into()]),
            }),
            webgpu: Some(WebgpuSpec {
                strategy: Some(Strategy::Value),
                vendor: Some("nvidia".into()),
                architecture: Some("ada".into()),
                device: Some("RTX 4090".into()),
                description: Some("test adapter".into()),
                limits: Some(std::collections::BTreeMap::from([(
                    "maxTextureDimension2D".to_string(),
                    16384u64,
                )])),
                features: Some(vec!["texture-compression-bc".into()]),
                fabricate_when_absent: Some(true),
            }),
            seed: Some(Seed::from_u64(9)),
            ..Persona::default()
        };
        let script = bootstrap_script(&p, &mock_identity());
        for tok in [
            "SEED",
            "WEBGL_VENDOR",
            "WEBGL_RENDERER",
            "FONT_ALLOW",
            "WEBRTC_POLICY",
            "WEBRTC_FAKE_IP",
            "HW_BATTERY",
            "HW_MEDIA_DEVICES",
            "HW_VOICES",
            "WEBGPU_VENDOR",
            "WEBGPU_ARCHITECTURE",
            "WEBGPU_DEVICE",
            "WEBGPU_DESCRIPTION",
            "WEBGPU_LIMITS",
            "WEBGPU_FEATURES",
            "WEBGPU_MODE",
            "WEBGPU_FABRICATE",
        ] {
            assert!(
                !script.contains(tok),
                "unsubstituted token `{tok}` left in bootstrap"
            );
        }
    }
}
