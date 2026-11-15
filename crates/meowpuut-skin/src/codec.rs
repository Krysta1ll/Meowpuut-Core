//! Encodings shared by every format: SHA-256, Base64 (RFC 4648, padded),
//! lowercase hex, Crockford Base32 and canonical JSON.
//!
//! Every decoder is strict: it accepts exactly one spelling of a value, so a
//! signed text has one meaning on every client.
//!
//! # Examples
//!
//! ```
//! use meowpuut_skin::codec::*;
//! assert_eq!(b64_encode(b"meow"), "bWVvdw==");
//! assert_eq!(b64_decode("bWVvdw==").as_deref(), Some(&b"meow"[..]));
//! assert_eq!(b64_decode("bWVvdx=="), None); // non-zero padding bits
//! assert_eq!(hex_decode("00ff"), Some(vec![0, 255]));
//! assert_eq!(hex_decode("00FF"), None);
//! let v: serde_json::Value = serde_json::from_str(r#"{"b":1,"a":[true,"x"]}"#).unwrap();
//! assert_eq!(canonical_json(&v), br#"{"a":[true,"x"],"b":1}"#);
//! ```

use serde_json::Value;
use sha2::{Digest, Sha256};

/// SHA-256 of `data`.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// SHA-256 over several pieces, as if concatenated.
pub fn sha256_parts(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard padded Base64.
pub fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(B64[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Strict Base64: only the spelling [`b64_encode`] produces is accepted.
pub fn b64_decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        let mut n = 0u32;
        let mut pad = 0;
        for (i, &c) in chunk.iter().enumerate() {
            let v = match c {
                b'A'..=b'Z' => c - b'A',
                b'a'..=b'z' => c - b'a' + 26,
                b'0'..=b'9' => c - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                b'=' if i >= 2 => {
                    pad += 1;
                    0
                }
                _ => return None,
            };
            if pad > 0 && c != b'=' {
                return None;
            }
            n = (n << 6) | u32::from(v);
        }
        let take = 3 - pad;
        for i in 0..take {
            out.push((n >> (16 - 8 * i)) as u8);
        }
    }
    // Rejects data after padding and non-zero unused bits in one go.
    (b64_encode(&out) == text).then_some(out)
}

/// Base64 that must decode to exactly `N` bytes.
pub fn b64_array<const N: usize>(text: &str) -> Option<[u8; N]> {
    b64_decode(text)?.try_into().ok()
}

/// Lowercase hex.
pub fn hex_encode(data: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(data.len() * 2);
    for &b in data {
        out.push(char::from(HEX[usize::from(b >> 4)]));
        out.push(char::from(HEX[usize::from(b & 15)]));
    }
    out
}

/// Strict lowercase hex.
pub fn hex_decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let digit = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    bytes.chunks(2).map(|p| Some((digit(p[0])? << 4) | digit(p[1])?)).collect()
}

/// Lowercase hex that must decode to exactly `N` bytes.
pub fn hex_array<const N: usize>(text: &str) -> Option<[u8; N]> {
    hex_decode(text)?.try_into().ok()
}

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Crockford Base32 (upper case, no check symbol); the bit length must be a
/// multiple of 5 for an exact round trip.
pub fn crockford_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len() * 8 / 5 + 1);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &b in data {
        acc = (acc << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(char::from(CROCKFORD[((acc >> bits) & 31) as usize]));
        }
        acc &= (1 << bits) - 1;
    }
    if bits > 0 {
        out.push(char::from(CROCKFORD[((acc << (5 - bits)) & 31) as usize]));
    }
    out
}

/// Crockford Base32 decoding: case-insensitive, `O` reads as 0, `I`/`L` as 1,
/// `-` and whitespace are ignored. Leftover bits must be zero.
pub fn crockford_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in text.chars() {
        if c == '-' || c.is_ascii_whitespace() {
            continue;
        }
        let c = c.to_ascii_uppercase();
        let c = match c {
            'O' => '0',
            'I' | 'L' => '1',
            _ => c,
        };
        let v = CROCKFORD.iter().position(|&x| char::from(x) == c)? as u32;
        acc = (acc << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    (acc == 0).then_some(out)
}

/// Canonical JSON: object keys sorted by their UTF-8 bytes, no whitespace,
/// strings escaped as `serde_json` does, numbers in `serde_json`'s shortest
/// form. Written here (not with `serde_json::to_vec`) so the key order does
/// not depend on whether some other crate enables `preserve_order`.
pub fn canonical_json(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Value::Number(n) => out.extend_from_slice(n.to_string().as_bytes()),
        Value::String(s) => write_string(s, out),
        Value::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_canonical(item, out);
            }
            out.push(b']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
            out.push(b'{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_string(key, out);
                out.push(b':');
                write_canonical(&map[key], out);
            }
            out.push(b'}');
        }
    }
}

fn write_string(s: &str, out: &mut Vec<u8>) {
    // serde_json's string escaping is fixed: `"`, `\` and C0 controls only.
    let escaped = serde_json::to_string(s).unwrap_or_else(|_| String::from("\"\""));
    out.extend_from_slice(escaped.as_bytes());
}

/// Parses JSON that must already be in canonical form (signed texts).
pub fn parse_canonical(bytes: &[u8]) -> Option<Value> {
    let value: Value = serde_json::from_slice(bytes).ok()?;
    (canonical_json(&value) == bytes).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips_every_length() {
        for n in 0..40u8 {
            let data: Vec<u8> = (0..n).map(|i| i.wrapping_mul(37).wrapping_add(n)).collect();
            assert_eq!(b64_decode(&b64_encode(&data)), Some(data));
        }
        assert_eq!(b64_decode("===="), None);
        assert_eq!(b64_decode("QQ==QQ=="), None);
        assert_eq!(b64_decode("QQ="), None);
        assert_eq!(b64_decode("Q==="), None);
    }

    #[test]
    fn crockford_round_trips_and_aliases() {
        let data: Vec<u8> = (0..35u8).collect();
        let text = crockford_encode(&data);
        assert_eq!(text.len(), 56);
        assert_eq!(crockford_decode(&text), Some(data.clone()));
        let lower = text.to_ascii_lowercase().replace('0', "o").replace('1', "l");
        assert_eq!(crockford_decode(&lower), Some(data));
        assert_eq!(crockford_decode("U"), None);
    }

    #[test]
    fn canonical_rejects_other_spellings() {
        assert!(parse_canonical(br#"{"a":1,"b":"x"}"#).is_some());
        assert!(parse_canonical(br#"{"b":"x","a":1}"#).is_none());
        assert!(parse_canonical(br#"{"a": 1}"#).is_none());
        assert!(parse_canonical(br#"{"a":1,"a":1}"#).is_none());
        assert!(parse_canonical(br#"{"a":"\u0041"}"#).is_none());
        assert!(parse_canonical(br#"{"a":1e0}"#).is_none());
        assert!(parse_canonical(br#"{"a":1.0}"#).is_some());
    }
}
