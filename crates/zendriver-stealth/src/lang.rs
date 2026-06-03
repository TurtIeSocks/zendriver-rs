//! Locale list -> header/JS derivations. Always available (no feature gate);
//! the observer formats `Accept-Language` from these.

use crate::{Fingerprint, Persona};

/// Format an ordered language list as an `Accept-Language` header value.
///
/// Index 0 carries implicit `q=1.0`; each later entry gets
/// `q = (1.0 - 0.1*i)` clamped to `>= 0.1`, one decimal. Empty/duplicate
/// entries are dropped (order preserved). Empty input -> `""`.
pub fn accept_language(langs: &[String]) -> String {
    let mut seen = std::collections::HashSet::new();
    langs
        .iter()
        .filter(|l| !l.is_empty() && seen.insert(l.as_str()))
        .enumerate()
        .map(|(i, l)| {
            if i == 0 {
                l.clone()
            } else {
                let q = (1.0 - 0.1 * i as f64).max(0.1);
                format!("{l};q={q:.1}")
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Resolve the effective language list at apply time.
///
/// Precedence: `persona.languages` -> `fingerprint.languages` ->
/// derive from the primary locale -> `["en-US", "en"]`. Deriving from a
/// region locale yields `[locale, base_lang]` where `base_lang` is the
/// subtag before `-` (e.g. `"fr-FR"` -> `["fr-FR", "fr"]`); a bare locale
/// (no `-`) yields a single entry.
pub(crate) fn resolve_languages(persona: &Persona, fp: &Fingerprint) -> Vec<String> {
    if let Some(langs) = persona.languages.as_ref().filter(|v| !v.is_empty()) {
        return langs.clone();
    }
    if let Some(langs) = fp.languages.as_ref().filter(|v| !v.is_empty()) {
        return langs.clone();
    }
    let locale = persona.locale.clone().or_else(|| fp.locale.clone());
    match locale {
        Some(loc) => {
            let base = loc.split('-').next().unwrap_or(&loc).to_string();
            if base != loc {
                vec![loc, base]
            } else {
                vec![loc]
            }
        }
        None => vec!["en-US".to_string(), "en".to_string()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    pub(crate) fn bare_fingerprint() -> crate::Fingerprint {
        use crate::{Platform, UserAgentMetadata};
        crate::Fingerprint {
            platform: Platform::MacIntel,
            chrome_major: 120,
            chrome_full: "120.0.6099.234".into(),
            cpu_count: 8,
            memory_gb: 8,
            ua_string: String::new(),
            ua_metadata: UserAgentMetadata::realistic(Platform::MacIntel, 120, "120.0.6099.234"),
            timezone: None,
            locale: None,
            languages: None,
        }
    }

    #[test]
    fn resolve_precedence() {
        let fp = bare_fingerprint();

        // persona.languages wins
        let p = crate::Persona {
            languages: Some(vec!["es-ES".into(), "es".into()]),
            ..Default::default()
        };
        assert_eq!(super::resolve_languages(&p, &fp), vec!["es-ES", "es"]);

        // derive from locale: the fr-FR regression (was hardcoded "en")
        let p = crate::Persona {
            locale: Some("fr-FR".into()),
            ..Default::default()
        };
        assert_eq!(super::resolve_languages(&p, &fp), vec!["fr-FR", "fr"]);

        // bare locale -> single entry
        let p = crate::Persona {
            locale: Some("en".into()),
            ..Default::default()
        };
        assert_eq!(super::resolve_languages(&p, &fp), vec!["en"]);

        // nothing set -> default
        let p = crate::Persona::default();
        assert_eq!(super::resolve_languages(&p, &fp), vec!["en-US", "en"]);
    }

    #[test]
    fn formats_q_weights() {
        assert_eq!(accept_language(&v(&["en-US", "en"])), "en-US,en;q=0.9");
        assert_eq!(
            accept_language(&v(&["de-DE", "de", "en"])),
            "de-DE,de;q=0.9,en;q=0.8"
        );
    }

    #[test]
    fn single_and_empty() {
        assert_eq!(accept_language(&v(&["fr-FR"])), "fr-FR");
        assert_eq!(accept_language(&[]), "");
    }

    #[test]
    fn dedups_and_floors_q() {
        assert_eq!(accept_language(&v(&["en", "en", "fr"])), "en,fr;q=0.9");
        let many: Vec<String> = (0..12).map(|i| format!("l{i}")).collect();
        let out = accept_language(&many);
        assert!(out.ends_with("l11;q=0.1"), "q must floor at 0.1: {out}");
    }
}
