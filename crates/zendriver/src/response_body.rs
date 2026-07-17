//! Bounded response-body capture.
//!
//! Unbounded body capture ([`crate::monitor::NetworkExchange::body`] and
//! friends) risks OOM on a large response, or blowing an agent's context
//! window when the body is forwarded verbatim. [`BoundedBody`] gives callers
//! an explicit cap: capture at most `max_bytes` of a body and get back a
//! `truncated` flag that says whether anything was cut, so a short body is
//! never silently mistaken for a truncated one (and vice versa).
//!
//! This module is a standalone primitive — it does not itself fetch or
//! decode bodies. A caller (the network monitor, in a later change) is
//! expected to decode the raw body bytes (e.g. base64-decode a CDP
//! `getResponseBody` result) and pass the decoded bytes to
//! [`BoundedBody::capture`].

/// A body captured up to a byte cap, with explicit truncation bookkeeping.
///
/// Construct via [`BoundedBody::capture`]. All bounding happens against the
/// **raw decoded byte length** of the input — never a base64 or other
/// encoded representation — so a fully-captured small body is never
/// misreported as truncated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedBody {
    /// The captured bytes: either the full body, or the first `max_bytes` of
    /// it if the body exceeded the cap.
    pub bytes: Vec<u8>,
    /// `true` if `bytes` is a prefix of a larger body (i.e. the body's raw
    /// length exceeded `max_bytes`); `false` if `bytes` holds the entire
    /// body.
    pub truncated: bool,
    /// The full (pre-truncation) raw byte length of the body, regardless of
    /// how much of it was kept in `bytes`. Always equal to `bytes.len()` as
    /// a `u64` when `truncated` is `false`.
    pub encoded_len: u64,
}

impl BoundedBody {
    /// Capture up to `max_bytes` of `full`, a raw (already-decoded) byte
    /// slice.
    ///
    /// - If `full.len() <= max_bytes`, every byte is kept and `truncated` is
    ///   `false`.
    /// - Otherwise only the first `max_bytes` bytes are kept and `truncated`
    ///   is `true`.
    /// - `encoded_len` is always `full.len()` as a `u64`, i.e. the full
    ///   pre-truncation length — it does not shrink when the body is
    ///   truncated, so callers can report "captured N of M bytes".
    ///
    /// `max_bytes == 0` means **unbounded**: the entire body is kept and
    /// `truncated` is always `false`. This lets a caller opt out of bounding
    /// (e.g. a config default of `0`) without a separate code path — there
    /// is no "reject" behavior to special-case.
    ///
    /// `full` must already be the raw decoded bytes of the body (e.g. after
    /// base64-decoding a CDP `getResponseBody` result). Bounding is always
    /// computed against this raw length, never against an encoded
    /// representation's length — a body whose base64 form is longer than
    /// `max_bytes` but whose raw form is not must never be reported as
    /// truncated.
    ///
    /// ```
    /// use zendriver::BoundedBody;
    ///
    /// let body = BoundedBody::capture(b"hello world", 5);
    /// assert!(body.truncated);
    /// assert_eq!(body.bytes, b"hello");
    /// assert_eq!(body.encoded_len, 11);
    /// ```
    #[must_use]
    pub fn capture(full: &[u8], max_bytes: usize) -> Self {
        let encoded_len = full.len() as u64;

        if max_bytes == 0 || full.len() <= max_bytes {
            return Self {
                bytes: full.to_vec(),
                truncated: false,
                encoded_len,
            };
        }

        Self {
            bytes: full[..max_bytes].to_vec(),
            truncated: true,
            encoded_len,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_under_bound_is_not_truncated() {
        let full = b"hello";
        let result = BoundedBody::capture(full, 10);

        assert!(!result.truncated);
        assert_eq!(result.bytes, full.to_vec());
        assert_eq!(result.encoded_len, full.len() as u64);
    }

    #[test]
    fn body_over_bound_is_truncated_to_prefix() {
        let full = b"hello world";
        let max_bytes = 5;
        let result = BoundedBody::capture(full, max_bytes);

        assert!(result.truncated);
        assert_eq!(result.bytes.len(), max_bytes);
        assert_eq!(result.bytes, full[..max_bytes].to_vec());
        assert_eq!(result.encoded_len, full.len() as u64);
    }

    #[test]
    fn body_exactly_at_bound_is_not_truncated() {
        let full = b"hello";
        let result = BoundedBody::capture(full, full.len());

        assert!(!result.truncated);
        assert_eq!(result.bytes, full.to_vec());
        assert_eq!(result.encoded_len, full.len() as u64);
    }

    #[test]
    fn max_bytes_zero_is_unbounded() {
        let full = vec![0xAB; 10_000];
        let result = BoundedBody::capture(&full, 0);

        assert!(!result.truncated);
        assert_eq!(result.bytes, full);
        assert_eq!(result.encoded_len, full.len() as u64);
    }

    /// Bounding must be computed against the RAW decoded byte length, never
    /// a base64 (or other encoded) length. Construct a raw body small enough
    /// to fit under `max_bytes`, but whose base64 encoding (~4/3 larger,
    /// plus padding) would exceed `max_bytes` — and prove `capture` still
    /// reports it as fully captured.
    #[test]
    fn bounding_uses_raw_length_not_base64_encoded_length() {
        use base64::Engine as _;
        use base64::engine::general_purpose::STANDARD as BASE64;

        // 40 raw bytes.
        let full = vec![0x42; 40];
        let encoded = BASE64.encode(&full);

        // The base64 form is longer than the raw form (encoding overhead),
        // and specifically longer than a bound that comfortably fits the
        // raw bytes.
        let max_bytes = 40;
        assert!(
            encoded.len() > max_bytes,
            "test setup: base64 form ({}) must exceed max_bytes ({max_bytes}) \
             to exercise the raw-vs-encoded distinction",
            encoded.len()
        );

        // capture() is called with the RAW bytes (as a caller would after
        // base64-decoding), so it must NOT report truncation even though the
        // as-yet-undecoded encoded form would have exceeded the bound.
        let result = BoundedBody::capture(&full, max_bytes);

        assert!(
            !result.truncated,
            "raw body fits under max_bytes; must not be truncated even though \
             its base64 encoding would have exceeded max_bytes"
        );
        assert_eq!(result.bytes, full);
        assert_eq!(result.encoded_len, full.len() as u64);
    }

    #[test]
    fn empty_body_is_not_truncated() {
        let result = BoundedBody::capture(b"", 10);

        assert!(!result.truncated);
        assert!(result.bytes.is_empty());
        assert_eq!(result.encoded_len, 0);
    }
}
