//! Decode sampled Bayesian-network attribute strings onto a [`Persona`] subset.
//!
//! browserforge encodes complex/non-string leaf values with a literal
//! `*STRINGIFIED*` prefix; strip it and JSON-parse the remainder.

use std::collections::HashMap;

use serde_json::Value;
use zendriver_stealth::{FontSpec, Persona, Platform, WebglSpec};

const STRINGIFIED: &str = "*STRINGIFIED*";

/// Strip the `*STRINGIFIED*` prefix for a scalar (e.g. `*STRINGIFIED*16` -> `16`).
pub(super) fn destringify_scalar(raw: &str) -> &str {
    raw.strip_prefix(STRINGIFIED).unwrap_or(raw)
}

/// Strip `*STRINGIFIED*` and JSON-parse the remainder (objects / arrays).
pub(super) fn destringify_json(raw: &str) -> Option<Value> {
    serde_json::from_str(raw.strip_prefix(STRINGIFIED)?).ok()
}

/// A UA is desktop if it carries no mobile marker.
pub(super) fn is_desktop_ua(ua: &str) -> bool {
    !["Mobile", "Android", "iPhone", "iPad"]
        .iter()
        .any(|m| ua.contains(m))
}

/// Map a sampled `platform` value onto the desktop-only [`Platform`] enum.
pub(super) fn map_platform(v: &str) -> Option<Platform> {
    match v {
        "Win32" => Some(Platform::Win32),
        "MacIntel" => Some(Platform::MacIntel),
        _ if v.starts_with("Linux") => Some(Platform::LinuxX86_64),
        _ => None,
    }
}

/// Build a [`Persona`] from a sampled `node name -> value` assignment.
pub(super) fn persona_from_assignment(a: &HashMap<String, String>) -> Persona {
    let mut p = Persona::default();
    if let Some(v) = a.get("platform") {
        p.platform = map_platform(v);
    }
    if let Some(v) = a.get("deviceMemory") {
        p.device_memory_gb = destringify_scalar(v).parse().ok();
    }
    if let Some(v) = a.get("hardwareConcurrency") {
        p.hardware_concurrency = destringify_scalar(v).parse().ok();
    }
    if let Some(v) = a.get("videoCard") {
        if let Some(obj) = destringify_json(v) {
            let vendor = obj.get("vendor").and_then(Value::as_str).map(String::from);
            let renderer = obj
                .get("renderer")
                .and_then(Value::as_str)
                .map(String::from);
            if vendor.is_some() || renderer.is_some() {
                p.webgl = Some(WebglSpec {
                    unmasked_vendor: vendor,
                    unmasked_renderer: renderer,
                    ..Default::default()
                });
            }
        }
    }
    if let Some(v) = a.get("fonts") {
        if let Some(Value::Array(arr)) = destringify_json(v) {
            let list: Vec<String> = arr
                .iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect();
            if !list.is_empty() {
                p.fonts = Some(FontSpec {
                    available: Some(list),
                    ..Default::default()
                });
            }
        }
    }
    p
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn destringify_scalar_strips_prefix() {
        assert_eq!(destringify_scalar("*STRINGIFIED*16"), "16");
        assert_eq!(destringify_scalar("MacIntel"), "MacIntel");
    }

    #[test]
    fn destringify_json_parses_object_and_array() {
        let obj = destringify_json("*STRINGIFIED*{\"vendor\":\"V\",\"renderer\":\"R\"}").unwrap();
        assert_eq!(obj.get("vendor").unwrap(), "V");
        let arr = destringify_json("*STRINGIFIED*[\"a\",\"b\"]").unwrap();
        assert!(arr.is_array());
        assert!(destringify_json("not-stringified").is_none());
    }

    #[test]
    fn is_desktop_ua_filters_mobile() {
        assert!(is_desktop_ua(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/143"
        ));
        assert!(!is_desktop_ua(
            "Mozilla/5.0 (Linux; Android 13) Mobile Safari"
        ));
        assert!(!is_desktop_ua("Mozilla/5.0 (iPhone; CPU iPhone OS 17_0)"));
    }

    #[test]
    fn map_platform_table() {
        assert_eq!(map_platform("Win32"), Some(Platform::Win32));
        assert_eq!(map_platform("MacIntel"), Some(Platform::MacIntel));
        assert_eq!(map_platform("Linux x86_64"), Some(Platform::LinuxX86_64));
        assert_eq!(map_platform("Linux aarch64"), Some(Platform::LinuxX86_64));
        assert_eq!(map_platform("iPhone"), None);
    }

    #[test]
    fn persona_from_assignment_populates_subset() {
        let mut a = HashMap::new();
        a.insert("platform".into(), "MacIntel".into());
        a.insert("deviceMemory".into(), "*STRINGIFIED*16".into());
        a.insert("hardwareConcurrency".into(), "*STRINGIFIED*10".into());
        a.insert(
            "videoCard".into(),
            "*STRINGIFIED*{\"renderer\":\"ANGLE (Apple, Apple M2)\",\"vendor\":\"Google Inc. (Apple)\"}".into(),
        );
        a.insert(
            "fonts".into(),
            "*STRINGIFIED*[\"Menlo\",\"Gill Sans\"]".into(),
        );
        let p = persona_from_assignment(&a);
        assert_eq!(p.platform, Some(Platform::MacIntel));
        assert_eq!(p.device_memory_gb, Some(16));
        assert_eq!(p.hardware_concurrency, Some(10));
        let w = p.webgl.unwrap();
        assert_eq!(w.unmasked_vendor.as_deref(), Some("Google Inc. (Apple)"));
        assert!(w.unmasked_renderer.unwrap().contains("Apple M2"));
        assert_eq!(p.fonts.unwrap().available.unwrap().len(), 2);
    }
}
