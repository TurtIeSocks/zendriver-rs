//! Coherent request-header values derived from the claimed stealth identity.
//!
//! A real Chrome over CDP already emits coherent request headers; the one value
//! that can silently skew from the *claimed* identity is `Accept-Encoding`,
//! because Chrome's network stack advertises the *real binary's* supported
//! encodings regardless of the UA / UA-CH overrides we apply. `zstd` shipped
//! enabled-by-default in Chrome 123 (2024-03); `br` (brotli) since Chrome 50.
//! Two branches therefore suffice — zendriver does not drive pre-`br` Chrome.
//!
//! Chrome's network service **owns** this header: `Network.setExtraHTTPHeaders`
//! does *not* override it (verified against Chrome 148 — the override is
//! silently dropped), and the `--disable-features=ZstdContentEncoding` launch
//! flag is inert on current builds. The skew is therefore **observable but not
//! correctable over CDP** — the stealth layer only *warns* about it (see
//! [`accept_encoding_skew`]).
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

/// The `Accept-Encoding` a `claimed_major` *should* advertise, returned only
/// when it differs from what the launched `binary_major` actually sends — i.e.
/// the two straddle the `zstd`/Chrome-123 boundary. `None` means the binary's
/// native header already matches the claimed identity (no incoherence).
///
/// A `Some` result is a *warning signal*, not an override target: this header
/// cannot be corrected over CDP (see the module docs). Callers warn the user
/// that a pinned `chrome_version` will leak the binary's encodings.
pub(crate) fn accept_encoding_skew(binary_major: u32, claimed_major: u32) -> Option<&'static str> {
    let claimed = accept_encoding_for(claimed_major);
    (claimed != accept_encoding_for(binary_major)).then_some(claimed)
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

    #[test]
    fn accept_encoding_skew_flags_only_a_boundary_crossing() {
        // Claimed pre-zstd on a zstd-capable binary -> skew (the leak case).
        assert_eq!(accept_encoding_skew(148, 120), Some("gzip, deflate, br"));
        // Claimed zstd-capable on a pre-zstd binary -> skew (other direction).
        assert_eq!(
            accept_encoding_skew(120, 148),
            Some("gzip, deflate, br, zstd")
        );
        // Both sides of the boundary agree -> no skew.
        assert_eq!(accept_encoding_skew(148, 140), None);
        assert_eq!(accept_encoding_skew(120, 122), None);
        assert_eq!(accept_encoding_skew(130, 130), None);
    }
}
