//! Country -> locale/languages derivation from a vendored Unicode CLDR table.
//!
//! Opt-in (`--features geo`). The single user-facing entry point is
//! [`persona`], which returns a [`Persona`](crate::Persona) overlay carrying a
//! coherent `locale` + `languages` for the given [`Country`]. Everything stays
//! overridable; an unknown country leaves the persona untouched.

use std::fmt;

/// An ISO 3166-1 alpha-2 country code (uppercase, validated).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Country([u8; 2]);

/// Error returned when a string is not a 2-letter ASCII country code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidCountry(pub String);

impl fmt::Display for InvalidCountry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid country code: {:?} (want 2 ASCII letters)", self.0)
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
            Ok(Country([b[0].to_ascii_uppercase(), b[1].to_ascii_uppercase()]))
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
    fn country_parse() {
        assert_eq!(Country::try_from("us").unwrap().as_str(), "US");
        assert_eq!(Country::try_from("DE").unwrap().as_str(), "DE");
        assert!(Country::try_from("USA").is_err());
        assert!(Country::try_from("u").is_err());
        assert!(Country::try_from("u1").is_err());
        assert!("ch".parse::<Country>().is_ok());
    }
}
