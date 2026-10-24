//! Inline UUID parser vendored to avoid a `uuid` crate dep.
//!
//! Accepts the 36-character hyphenated form
//! (`xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`) and the same form wrapped
//! in `{...}` braces. Case-insensitive on the hex characters.

use alloc::string::{String, ToString};

/// Parse a UUID string into 16 raw bytes.
///
/// Returns `Err(source_len)` when the input is not a valid UUID
/// representation.
pub(crate) fn parse_uuid(s: &str) -> Result<[u8; 16], usize> {
    let raw = strip_uuid_braces(s);
    let bytes = raw.as_bytes();
    if bytes.len() != 36 {
        return Err(s.len());
    }
    // Expect hyphens at positions 8, 13, 18, 23.
    for pos in [8usize, 13, 18, 23] {
        if bytes[pos] != b'-' {
            return Err(s.len());
        }
    }
    let mut out = [0u8; 16];
    let hex_positions: [usize; 16] = [0, 2, 4, 6, 9, 11, 14, 16, 19, 21, 24, 26, 28, 30, 32, 34];
    for (i, pos) in hex_positions.into_iter().enumerate() {
        let hi = hex_nibble(bytes[pos]).ok_or(s.len())?;
        let lo = hex_nibble(bytes[pos + 1]).ok_or(s.len())?;
        out[i] = (hi << 4) | lo;
    }
    Ok(out)
}

/// Verify a UUID string and return its canonical 36-character
/// hyphenated form (lowercase).
///
/// Returns `Err(source_len)` on malformed input.
pub(crate) fn canonicalize_uuid_text(s: &str) -> Result<String, usize> {
    let bytes = parse_uuid(s)?;
    Ok(format_uuid(&bytes))
}

fn strip_uuid_braces(s: &str) -> &str {
    let trimmed = s.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') && trimmed.len() >= 2 {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    }
}

/// The value of one ASCII hex digit, either case.
#[inline]
pub(crate) fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

const HEX: &[u8; 16] = b"0123456789abcdef";

fn format_uuid(bytes: &[u8; 16]) -> String {
    let mut out = String::with_capacity(36);
    // Hex table is a module-level const above.
    let write_pair = |out: &mut String, byte: u8| {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0F) as usize] as char);
    };
    // Byte ranges: [0..4]-[4..6]-[6..8]-[8..10]-[10..16].
    for &b in &bytes[0..4] {
        write_pair(&mut out, b);
    }
    out.push('-');
    for &b in &bytes[4..6] {
        write_pair(&mut out, b);
    }
    out.push('-');
    for &b in &bytes[6..8] {
        write_pair(&mut out, b);
    }
    out.push('-');
    for &b in &bytes[8..10] {
        write_pair(&mut out, b);
    }
    out.push('-');
    for &b in &bytes[10..16] {
        write_pair(&mut out, b);
    }
    out
}

/// Same as [`canonicalize_uuid_text`] but always returns
/// `s.to_string()` verbatim when the input is already the canonical
/// 36-character lowercase form. This preserves the wire text exactly
/// for round-trip parity when nothing needs normalizing.
pub(crate) fn preserve_or_canonicalize_uuid_text(s: &str) -> Result<String, usize> {
    let raw = s.trim();
    if raw.len() == 36
        && raw
            .as_bytes()
            .iter()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F' | b'-'))
    {
        // Validate structure.
        let _bytes = parse_uuid(raw)?;
        return Ok(raw.to_string());
    }
    canonicalize_uuid_text(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HYPHEN: &str = "550e8400-e29b-41d4-a716-446655440000";
    const BRACED: &str = "{550e8400-e29b-41d4-a716-446655440000}";
    const UPPER: &str = "550E8400-E29B-41D4-A716-446655440000";
    const BYTES: [u8; 16] = [
        0x55, 0x0e, 0x84, 0x00, 0xe2, 0x9b, 0x41, 0xd4, 0xa7, 0x16, 0x44, 0x66, 0x55, 0x44, 0x00,
        0x00,
    ];

    #[test]
    fn parse_hyphenated() {
        assert_eq!(parse_uuid(HYPHEN).unwrap(), BYTES);
    }

    #[test]
    fn parse_braced() {
        assert_eq!(parse_uuid(BRACED).unwrap(), BYTES);
    }

    #[test]
    fn parse_uppercase() {
        assert_eq!(parse_uuid(UPPER).unwrap(), BYTES);
    }

    #[test]
    fn parse_invalid_length() {
        assert!(parse_uuid("too short").is_err());
    }

    #[test]
    fn parse_invalid_hex() {
        assert!(parse_uuid("550e8400-e29b-41d4-a716-zzzzzzzzzzzz").is_err());
    }

    #[test]
    fn canonicalize_returns_lowercase_36() {
        assert_eq!(canonicalize_uuid_text(UPPER).unwrap(), HYPHEN);
        assert_eq!(canonicalize_uuid_text(BRACED).unwrap(), HYPHEN);
    }

    #[test]
    fn preserve_verbatim_when_canonical() {
        assert_eq!(preserve_or_canonicalize_uuid_text(HYPHEN).unwrap(), HYPHEN);
        assert_eq!(preserve_or_canonicalize_uuid_text(BRACED).unwrap(), HYPHEN);
    }
}
