//! Coherent request-header values derived from the claimed stealth identity.
//!
//! A real Chrome over CDP already emits coherent request headers; the one value
//! that can silently skew from the *claimed* identity is `Accept-Encoding`,
//! because Chrome's network stack advertises the *real binary's* supported
//! encodings regardless of the UA / UA-CH overrides we apply. `zstd` shipped
//! enabled-by-default in Chrome 123 (2024-03); `br` (brotli) since Chrome 50.
//! Two branches therefore suffice — zendriver does not drive pre-`br` Chrome.
//!
//! See `docs/superpowers/specs/2026-06-02-header-coherence-design.md`.

/// The `Accept-Encoding` a real Chrome of `major` sends for a top-level HTTPS
/// navigation.
pub(crate) fn accept_encoding_for(major: u32) -> &'static str {
    if major >= 123 {
        "gzip, deflate, br, zstd"
    } else {
        "gzip, deflate, br"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_encoding_straddles_the_zstd_boundary() {
        assert_eq!(accept_encoding_for(122), "gzip, deflate, br");
        assert_eq!(accept_encoding_for(123), "gzip, deflate, br, zstd");
        assert_eq!(accept_encoding_for(148), "gzip, deflate, br, zstd");
    }
}
