//! Byte-decoding helpers for binary and hex escape payloads.
//!
//! Two small `no_std`-clean routines: PG `\xHEX` decode for text-mode
//! BYTEA, and standard base64 (RFC 4648) decode for MySQL wire values
//! delivered as JSON strings. Both are vendored to avoid pulling
//! external deps for a hundred lines of parsing.

use alloc::vec::Vec;

#[cfg(any(feature = "wal2json", feature = "pg-walstream"))]
use super::uuid_helpers::hex_nibble;

/// Decode a PG `\xHEX` escape (produced by `\x` prefix + even-length
/// lowercase or uppercase hex) into raw bytes.
///
/// # Errors
///
/// Returns the zero-based byte offset (into `s`) of the first invalid
/// hex character. When the input does not begin with `\x` or has an
/// odd hex length, the offset points at that anomaly.
#[cfg(feature = "pg-walstream")]
pub(crate) fn decode_pg_hex_escape(s: &str) -> Result<Vec<u8>, usize> {
    let bytes = s.as_bytes();
    if bytes.len() < 2 || bytes[0] != b'\\' || bytes[1] != b'x' {
        return Err(0);
    }
    // Offsets stay relative to `s`, so shift past the `\x` prefix.
    decode_hex(&bytes[2..]).map_err(|at| at + 2)
}

/// Decode a wal2json BYTEA hex string into raw bytes.
///
/// wal2json emits BYTEA as bare lowercase hex with no `\x` prefix, so
/// accept both the bare form and a Postgres-style `\x`-prefixed form.
///
/// # Errors
///
/// Returns the zero-based byte offset (into the hex payload, after any
/// `\x` prefix) of the first invalid hex character, or of the odd
/// trailing nibble.
#[cfg(feature = "wal2json")]
pub(crate) fn decode_wal2json_bytea_hex(s: &str) -> Result<Vec<u8>, usize> {
    let hex = s.strip_prefix("\\x").unwrap_or(s);
    decode_hex(hex.as_bytes())
}

/// Decode an even-length hex slice into raw bytes.
///
/// # Errors
///
/// Returns the byte offset within `hex` of the first invalid character,
/// or `hex.len() - 1` when the length is odd.
#[cfg(any(feature = "wal2json", feature = "pg-walstream"))]
fn decode_hex(hex: &[u8]) -> Result<Vec<u8>, usize> {
    if !hex.len().is_multiple_of(2) {
        return Err(hex.len().saturating_sub(1));
    }
    let mut out = Vec::with_capacity(hex.len() / 2);
    for (i, chunk) in hex.chunks_exact(2).enumerate() {
        let hi = hex_nibble(chunk[0]).ok_or(i * 2)?;
        let lo = hex_nibble(chunk[1]).ok_or(i * 2 + 1)?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

/// Decode a standard base64 (RFC 4648) string into raw bytes.
///
/// Accepts padded standard base64 (the `+`, `/` alphabet with `=`
/// padding). Returns `Err(())` on: a padded length not divisible by
/// four, an effective (un-padded) length whose remainder mod four is
/// one, an all-padding input, any character outside the base64
/// alphabet, and overloaded trailing bits (RFC 4648 section 3.5).
#[cfg(feature = "maxwell")]
pub(crate) fn decode_base64(s: &str) -> Result<Vec<u8>, ()> {
    let src = s.as_bytes();
    // Strip trailing '=' padding to find the effective length.
    let mut effective_len = src.len();
    while effective_len > 0 && src[effective_len - 1] == b'=' {
        effective_len -= 1;
    }
    // Padded length must be a multiple of four.
    if !src.len().is_multiple_of(4) {
        return Err(());
    }
    // An all-padding input like "====" carries no data and is malformed.
    if effective_len == 0 && !src.is_empty() {
        return Err(());
    }
    // An effective length with remainder 1 mod 4 cannot represent a
    // complete byte (six bits cannot fill eight).
    if effective_len % 4 == 1 {
        return Err(());
    }
    let mut out = Vec::with_capacity((effective_len * 3) / 4);
    let mut buf = 0u32;
    let mut collected = 0u32;
    for &c in &src[..effective_len] {
        let v = base64_char(c).ok_or(())?;
        buf = (buf << 6) | u32::from(v);
        collected += 6;
        if collected >= 8 {
            collected -= 8;
            out.push(((buf >> collected) & 0xFF) as u8);
        }
    }
    // RFC 4648 section 3.5: leftover bits after the last complete byte
    // must all be zero.
    if collected > 0 && (buf & ((1u32 << collected) - 1)) != 0 {
        return Err(());
    }
    Ok(out)
}

#[cfg(feature = "maxwell")]
#[inline]
fn base64_char(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[cfg(feature = "pg-walstream")]
    #[test]
    fn hex_decode_deadbeef() {
        assert_eq!(
            decode_pg_hex_escape("\\xdeadbeef").unwrap(),
            vec![0xDE, 0xAD, 0xBE, 0xEF]
        );
    }

    #[cfg(feature = "pg-walstream")]
    #[test]
    fn hex_decode_uppercase() {
        assert_eq!(
            decode_pg_hex_escape("\\xDEADBEEF").unwrap(),
            vec![0xDE, 0xAD, 0xBE, 0xEF]
        );
    }

    #[cfg(feature = "pg-walstream")]
    #[test]
    fn hex_decode_empty() {
        assert_eq!(decode_pg_hex_escape("\\x").unwrap(), Vec::<u8>::new());
    }

    #[cfg(feature = "pg-walstream")]
    #[test]
    fn hex_decode_missing_prefix() {
        assert!(decode_pg_hex_escape("deadbeef").is_err());
    }

    #[cfg(feature = "pg-walstream")]
    #[test]
    fn hex_decode_odd_length() {
        assert!(decode_pg_hex_escape("\\xdea").is_err());
    }

    #[cfg(feature = "wal2json")]
    #[test]
    fn wal2json_bytea_bare_hex() {
        assert_eq!(
            decode_wal2json_bytea_hex("0001deadff").unwrap(),
            vec![0x00, 0x01, 0xDE, 0xAD, 0xFF]
        );
    }

    #[cfg(feature = "wal2json")]
    #[test]
    fn wal2json_bytea_prefixed_hex() {
        assert_eq!(
            decode_wal2json_bytea_hex("\\xdeadbeef").unwrap(),
            vec![0xDE, 0xAD, 0xBE, 0xEF]
        );
    }

    #[cfg(feature = "wal2json")]
    #[test]
    fn wal2json_bytea_odd_length() {
        assert!(decode_wal2json_bytea_hex("0001d").is_err());
    }

    #[cfg(feature = "wal2json")]
    #[test]
    fn wal2json_bytea_invalid_char() {
        assert!(decode_wal2json_bytea_hex("00zz").is_err());
    }

    #[cfg(feature = "pg-walstream")]
    #[test]
    fn hex_decode_invalid_char() {
        assert!(decode_pg_hex_escape("\\xzz").is_err());
    }

    #[cfg(feature = "maxwell")]
    #[test]
    fn base64_roundtrip() {
        assert_eq!(
            decode_base64("3q2+7w==").unwrap(),
            vec![0xDE, 0xAD, 0xBE, 0xEF]
        );
        assert_eq!(decode_base64("AQID").unwrap(), vec![0x01, 0x02, 0x03]);
        assert_eq!(decode_base64("").unwrap(), Vec::<u8>::new());
    }

    #[cfg(feature = "maxwell")]
    #[test]
    fn base64_invalid_char() {
        assert!(decode_base64("!!!!").is_err());
    }

    #[cfg(feature = "maxwell")]
    #[test]
    fn base64_invalid_length() {
        assert!(decode_base64("abc").is_err());
    }

    #[cfg(feature = "maxwell")]
    #[test]
    fn base64_effective_length_remainder_one() {
        // One effective char carries six bits, which cannot fill a byte.
        assert!(decode_base64("A===").is_err());
        // Longer strings with effective_len % 4 == 1 are equally invalid.
        assert!(decode_base64("AAAAA===").is_err());
    }

    #[cfg(feature = "maxwell")]
    #[test]
    fn base64_all_padding() {
        // A string of only '=' pads carries no data and is malformed.
        assert!(decode_base64("====").is_err());
        assert!(decode_base64("========").is_err());
    }

    #[cfg(feature = "maxwell")]
    #[test]
    fn base64_overloaded_trailing_bits() {
        // "AB==" has effective length 2. 'B' (value 1) leaves the lower
        // four bits of the 12-bit combined group as 0b0001, which is
        // non-zero, so this is rejected per RFC 4648 section 3.5.
        assert!(decode_base64("AB==").is_err());
        // "AQF=" has effective length 3. 'F' (value 5 = 0b000101) leaves
        // the lower two bits of the 18-bit group as 0b01, non-zero.
        assert!(decode_base64("AQF=").is_err());
        // Valid counterparts: "AA==" and "AQE=" have zero trailing bits.
        assert_eq!(decode_base64("AA==").unwrap(), vec![0x00]);
        assert_eq!(decode_base64("AQE=").unwrap(), vec![0x01, 0x01]);
    }
}
