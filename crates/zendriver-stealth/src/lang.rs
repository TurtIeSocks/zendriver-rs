//! Locale list -> header/JS derivations. Always available (no feature gate);
//! the observer formats `Accept-Language` from these.

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

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
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
