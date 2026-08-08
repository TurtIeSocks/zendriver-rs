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

/// What both language surfaces fall back to when nothing at all is configured.
fn default_languages() -> Vec<String> {
    vec!["en-US".to_string(), "en".to_string()]
}

/// Derive a language list from a single locale.
///
/// A region locale yields `[locale, base_lang]` where `base_lang` is the
/// subtag before `-` (e.g. `"fr-FR"` -> `["fr-FR", "fr"]`); a bare locale (no
/// `-`) yields a single entry.
fn derive_from_locale(locale: &str) -> Vec<String> {
    let base = locale.split('-').next().unwrap_or(locale);
    if base == locale {
        vec![locale.to_string()]
    } else {
        vec![locale.to_string(), base.to_string()]
    }
}

/// The single resolution both locale-bearing CDP surfaces read: the language
/// list a [`Fingerprint`] actually configures, or `None` when it configures
/// neither `languages` nor `locale`.
///
/// Precedence is `languages` -> derived from `locale`, and the two callers
/// differ only in what they do with `None`: the `Accept-Language` header
/// substitutes `["en-US", "en"]` ([`fingerprint_languages`]), while
/// `Emulation.setLocaleOverride` sends nothing at all
/// ([`effective_locale`]) so Chrome keeps its own.
///
/// # Why `languages` outranks `locale`
///
/// In a real Chrome `navigator.language` is always `navigator.languages[0]`.
/// A caller who sets both and lets them disagree is asking for a browser that
/// cannot exist, so honoring both is not an option and the list has to win:
/// `navigator.languages` is observable in full, and a `language` that is not
/// its head is a mismatch no genuine browser produces. Resolving the header
/// and the locale override independently is what produced exactly that — the
/// header advertising one list while `Intl` reported a locale from outside it.
/// An explicit `locale` with no `languages` still drives both surfaces.
///
/// This is the post-merge view. Once
/// [`Fingerprint::overlay_persona`](crate::Fingerprint::overlay_persona) has
/// folded a persona in, the fingerprint already carries whatever the persona
/// pinned, so the observer reads that one value instead of re-deriving the
/// persona-vs-fingerprint precedence at each CDP call site.
fn configured_languages(fp: &Fingerprint) -> Option<Vec<String>> {
    if let Some(langs) = fp.languages.as_ref().filter(|v| !v.is_empty()) {
        return Some(langs.clone());
    }
    fp.locale.as_deref().map(derive_from_locale)
}

/// The language list to advertise, falling back to `["en-US", "en"]` when the
/// fingerprint configures none. See [`configured_languages`].
pub(crate) fn fingerprint_languages(fp: &Fingerprint) -> Vec<String> {
    configured_languages(fp).unwrap_or_else(default_languages)
}

/// The locale to pin via `Emulation.setLocaleOverride`, or `None` to send no
/// override at all.
///
/// By construction this is the head of [`fingerprint_languages`] whenever
/// anything is configured, which is the coherence the two surfaces need — see
/// [`configured_languages`] for why the list, not `locale`, decides.
pub(crate) fn effective_locale(fp: &Fingerprint) -> Option<String> {
    configured_languages(fp)?.into_iter().next()
}

/// Resolve the effective language list at apply time, persona first.
///
/// Precedence: `persona.languages` -> `fingerprint.languages` ->
/// derive from the primary locale (`persona.locale` before `fp.locale`) ->
/// `["en-US", "en"]`.
///
/// For a fingerprint the persona has already been folded into, this agrees
/// with [`fingerprint_languages`] by construction — the persona's values are
/// on `fp` by then. It stays persona-first for the public
/// [`bootstrap_script`](crate::patches::bootstrap_script) path, whose caller
/// may hand it an unmerged pair.
pub(crate) fn resolve_languages(persona: &Persona, fp: &Fingerprint) -> Vec<String> {
    if let Some(langs) = persona.languages.as_ref().filter(|v| !v.is_empty()) {
        return langs.clone();
    }
    if let Some(langs) = fp.languages.as_ref().filter(|v| !v.is_empty()) {
        return langs.clone();
    }
    match persona.locale.as_deref().or(fp.locale.as_deref()) {
        Some(locale) => derive_from_locale(locale),
        None => default_languages(),
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
            screen: None,
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
