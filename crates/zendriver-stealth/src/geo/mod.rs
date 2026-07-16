//! Country -> locale/languages derivation from a vendored Unicode CLDR table.
//!
//! Opt-in (`--features geo`). The single user-facing entry point is
//! [`persona`], which returns a [`Persona`](crate::Persona) overlay carrying a
//! coherent `locale` + `languages` for the given [`Country`]. Everything stays
//! overridable; an unknown country leaves the persona untouched.

mod table;

use std::fmt;

use crate::Persona;

/// Ranked language subtags for a country, or `None` if absent from the table.
pub(crate) fn languages_for(cc: &str) -> Option<&'static [&'static str]> {
    table::COUNTRIES
        .binary_search_by(|(c, _)| (*c).cmp(cc))
        .ok()
        .map(|i| table::COUNTRIES[i].1)
}

/// The Unicode CLDR release the vendored locale table was generated from
/// (e.g. `"46.0.0"`). Exposed for provenance/introspection; also keeps the
/// generated `CLDR_VERSION` a live (non-dead) symbol so the generated
/// `table.rs` needs no hand-added `#[allow]` that a resync would clobber.
pub fn cldr_version() -> &'static str {
    table::CLDR_VERSION
}

/// The IANA tz-database release the vendored `TIMEZONES` table was generated
/// from. Keeps `TZDATA_VERSION` a live symbol (no generated-file `#[allow]`).
pub fn tzdata_version() -> &'static str {
    table::TZDATA_VERSION
}

/// Representative IANA timezone for a country, or `None` if unmapped.
pub(crate) fn timezone_for(cc: &str) -> Option<&'static str> {
    table::TIMEZONES
        .binary_search_by(|(c, _)| (*c).cmp(cc))
        .ok()
        .map(|i| table::TIMEZONES[i].1)
}

/// Build a [`Persona`] overlay carrying a coherent `locale` + `languages` for
/// `country`. Default policy: the country's dominant language only, formed as
/// `lang-COUNTRY` with its base subtag (e.g. `CH` -> `de-CH` + `["de-CH","de"]`).
/// An unknown country yields an empty overlay and a warning — never locks a value.
pub fn persona(country: Country) -> Persona {
    let cc = country.as_str();
    match languages_for(cc) {
        Some([primary, ..]) => {
            let locale = format!("{primary}-{cc}");
            let mut p = Persona {
                locale: Some(locale.clone()),
                languages: Some(vec![locale, (*primary).to_string()]),
                ..Default::default()
            };
            p.timezone = timezone_for(cc).map(str::to_string);
            if p.locale.is_some() && p.timezone.is_none() {
                tracing::debug!("geo: no timezone mapping for country {cc}");
            }
            p
        }
        _ => {
            tracing::warn!("geo: no locale mapping for country {cc}; leaving locale unset");
            Persona::default()
        }
    }
}

/// Resolve a [`Country`] from ambient context (e.g. the exit IP behind a
/// proxy). The auto-resolving implementation (`IpApiResolver` + structured
/// proxy URL + outbound probe) lands in a follow-up PR; this seam exists now so
/// callers and the `BrowserBuilder` API are forward-compatible. Both the
/// caller-supplied path (`geo_locale("US")`) and a future resolver terminate in
/// the same [`persona`] mapping.
#[async_trait::async_trait]
pub trait GeoResolver: Send + Sync {
    /// Resolve the apparent country, or `None` if it cannot be determined.
    async fn country(&self) -> Option<Country>;
}

/// An ISO 3166-1 alpha-2 country code (uppercase, validated).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Country([u8; 2]);

/// Error returned when a string is not a 2-letter ASCII country code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidCountry(pub String);

impl fmt::Display for InvalidCountry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid country code: {:?} (want 2 ASCII letters)",
            self.0
        )
    }
}
impl std::error::Error for InvalidCountry {}

impl Country {
    /// The uppercased 2-letter code as a string slice.
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0).unwrap_or("")
    }
}

impl TryFrom<&str> for Country {
    type Error = InvalidCountry;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        let b = s.as_bytes();
        if b.len() == 2 && b.iter().all(u8::is_ascii_alphabetic) {
            Ok(Country([
                b[0].to_ascii_uppercase(),
                b[1].to_ascii_uppercase(),
            ]))
        } else {
            Err(InvalidCountry(s.to_string()))
        }
    }
}

impl std::str::FromStr for Country {
    type Err = InvalidCountry;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Country::try_from(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_for_known_and_unknown() {
        assert_eq!(languages_for("US"), Some(["en", "es", "haw"].as_slice()));
        assert!(languages_for("ZZ").is_none());
    }

    #[test]
    fn country_parse() {
        assert_eq!(Country::try_from("us").unwrap().as_str(), "US");
        assert_eq!(Country::try_from("DE").unwrap().as_str(), "DE");
        assert!(Country::try_from("USA").is_err());
        assert!(Country::try_from("u").is_err());
        assert!(Country::try_from("u1").is_err());
        assert!("ch".parse::<Country>().is_ok());
    }

    #[test]
    fn persona_sets_timezone_from_country() {
        assert_eq!(
            persona(Country::try_from("US").unwrap())
                .timezone
                .as_deref(),
            Some("America/New_York")
        );
    }

    #[test]
    fn persona_timezone_uses_override() {
        assert_eq!(
            persona(Country::try_from("RU").unwrap())
                .timezone
                .as_deref(),
            Some("Europe/Moscow")
        );
    }

    #[test]
    fn persona_locale_unchanged() {
        // existing locale/languages assertions still hold (regression)
        let p = persona(Country::try_from("US").unwrap());
        assert_eq!(p.locale.as_deref(), Some("en-US"));
    }

    #[test]
    fn persona_derives_locale_and_languages() {
        let p = persona(Country::try_from("US").unwrap());
        assert_eq!(p.locale.as_deref(), Some("en-US"));
        assert_eq!(p.languages.unwrap(), vec!["en-US", "en"]);

        let p = persona(Country::try_from("DE").unwrap());
        assert_eq!(p.locale.as_deref(), Some("de-DE"));

        // multilingual country -> single dominant language by default
        let ch = persona(Country::try_from("CH").unwrap());
        assert_eq!(ch.locale.as_deref(), Some("de-CH"));
        assert_eq!(ch.languages.unwrap(), vec!["de-CH", "de"]);

        // unknown country -> empty overlay, no panic
        let empty = persona(Country::try_from("ZZ").unwrap());
        assert!(empty.locale.is_none() && empty.languages.is_none());
    }

    #[test]
    fn cldr_table_invariants() {
        let rows = table::COUNTRIES;
        assert!(
            (100..=1000).contains(&rows.len()),
            "row count {} out of [100,1000]",
            rows.len()
        );
        // sorted + unique (binary_search correctness)
        assert!(
            rows.windows(2).all(|w| w[0].0 < w[1].0),
            "COUNTRIES must be strictly sorted by code"
        );
        for (cc, langs) in rows {
            assert_eq!(cc.len(), 2, "{cc} not 2 chars");
            assert!(
                cc.bytes().all(|b| b.is_ascii_uppercase()),
                "{cc} not uppercase"
            );
            assert!(!langs.is_empty(), "{cc} has no languages");
            for l in *langs {
                assert!(
                    l.len() >= 2 && l.bytes().all(|b| b.is_ascii_lowercase()),
                    "{cc}: bad lang subtag {l:?}"
                );
            }
            let _ = format!("{}-{}", langs[0], cc); // forms a BCP-47 primary locale
        }
        assert!(!table::CLDR_VERSION.is_empty());
    }
}
