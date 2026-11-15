//! Request keys and the redemption ID (§3.4, §6.2).
//!
//! Every "ask the developer for a skin" makes a new X25519 key pair; the
//! private key stays on the device (Android: wrapped by a Keystore AES-GCM
//! key), the public key travels as the redemption ID:
//!
//! ```text
//! bytes:   0x01 (version) ‖ X25519 public key (32) ‖ checksum (2)
//! checksum = SHA-256("meowpuut-request/v1\0" ‖ 0x01 ‖ public key)[0..2]
//! text:    "MPR2-" + Crockford Base32 of the 35 bytes (56 symbols) in groups of 4 joined by "-"
//! ```
//!
//! Parsing ignores case, `-` and white space, reads `O` as 0 and `I`/`L` as 1,
//! and refuses a wrong version or checksum and any small-order key (those
//! would make the key agreement non-contributory).
//!
//! # Examples
//!
//! ```
//! use meowpuut_skin::request::*;
//! let secret = [7u8; 32];
//! let public = public_key(&secret);
//! let id = redemption_id(&public);
//! assert!(id.starts_with("MPR2-"));
//! assert_eq!(parse_redemption_id(&id.to_lowercase()).unwrap(), public);
//! assert!(parse_redemption_id(&redemption_id(&[0u8; 32])).is_err());
//! ```

use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

use crate::codec::{crockford_decode, crockford_encode, sha256, sha256_parts};
use crate::{fail, limits, Code, Error, Result};

/// Version byte of a redemption ID.
pub const VERSION: u8 = 1;
/// Display prefix of a redemption ID.
pub const PREFIX: &str = "MPR2-";
const CHECK_DOMAIN: &[u8] = b"meowpuut-request/v1\0";

/// A new request key pair from the operating system's random source:
/// `(secret, public)`. The secret is returned unclamped (X25519 clamps).
pub fn generate() -> Result<(Zeroizing<[u8; 32]>, [u8; 32])> {
    let mut secret = Zeroizing::new([0u8; 32]);
    getrandom::fill(secret.as_mut()).map_err(|_| Error::from(Code::Random))?;
    let public = public_key(&secret);
    if is_small_order(&public) {
        // Impossible for a clamped random scalar; refuse rather than hand it out.
        return fail(Code::Random, "degenerate key");
    }
    Ok((secret, public))
}

/// The X25519 public key of a request secret.
pub fn public_key(secret: &[u8; 32]) -> [u8; 32] {
    PublicKey::from(&StaticSecret::from(*secret)).to_bytes()
}

/// Whether `public` is a small-order point (or an encoding of one): X25519 with
/// any clamped scalar, a multiple of the cofactor, then yields all zeros.
pub fn is_small_order(public: &[u8; 32]) -> bool {
    let mut probe = [0u8; 32];
    probe[0] = 0x08;
    probe[31] = 0x40;
    x25519_dalek::x25519(probe, *public) == [0u8; 32]
}

/// `to`: the first 16 bytes of SHA-256 of the request public key.
pub fn recipient_tag(public: &[u8; 32]) -> [u8; 16] {
    let digest = sha256(public);
    let mut out = [0u8; 16];
    out.copy_from_slice(&digest[..16]);
    out
}

fn checksum(public: &[u8; 32]) -> [u8; 2] {
    let digest = sha256_parts(&[CHECK_DOMAIN, &[VERSION], public]);
    [digest[0], digest[1]]
}

/// The redemption ID text of a request public key.
pub fn redemption_id(public: &[u8; 32]) -> String {
    let mut bytes = Vec::with_capacity(35);
    bytes.push(VERSION);
    bytes.extend_from_slice(public);
    bytes.extend_from_slice(&checksum(public));
    let symbols = crockford_encode(&bytes);
    let groups: Vec<&str> = symbols.as_bytes().chunks(4).map(|c| std::str::from_utf8(c).unwrap_or("")).collect();
    format!("{PREFIX}{}", groups.join("-"))
}

/// Parses a redemption ID (the developer's `issue --to`, or a paste).
pub fn parse_redemption_id(text: &str) -> Result<[u8; 32]> {
    if text.len() > limits::REQUEST_TEXT {
        return fail(Code::TooLarge, "redemption ID");
    }
    let trimmed = text.trim();
    let body = trimmed
        .get(..PREFIX.len())
        .filter(|p| p.eq_ignore_ascii_case(PREFIX))
        .map(|_| &trimmed[PREFIX.len()..])
        .ok_or_else(|| Error::new(Code::RequestId, "prefix"))?;
    let bytes = crockford_decode(body).ok_or_else(|| Error::new(Code::RequestId, "symbols"))?;
    if bytes.len() != 35 || bytes[0] != VERSION {
        return fail(Code::RequestId, "length or version");
    }
    let mut public = [0u8; 32];
    public.copy_from_slice(&bytes[1..33]);
    if bytes[33..] != checksum(&public) {
        return fail(Code::RequestId, "checksum");
    }
    if is_small_order(&public) {
        return fail(Code::RequestId, "small-order key");
    }
    Ok(public)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_keys_round_trip() {
        let (secret, public) = generate().unwrap();
        assert_eq!(public_key(&secret), public);
        let id = redemption_id(&public);
        assert_eq!(id.len(), PREFIX.len() + 56 + 13);
        let spaced = format!("  {PREFIX}{}\n", id[PREFIX.len()..].replace('-', " - "));
        assert_eq!(parse_redemption_id(&spaced).unwrap(), public);
    }

    #[test]
    fn small_order_points_are_refused() {
        // The order-1, -2, -4 and -8 points and non-canonical encodings of 0 and 1.
        let mut points = vec![[0u8; 32], {
            let mut p = [0u8; 32];
            p[0] = 1;
            p
        }];
        points.push(hex32("e0eb7a7c3b41b8ae1656e3faf19fc46ada098deb9c32b1fd866205165f49b800"));
        points.push(hex32("5f9c95bca3508c24b1d0b1559c83ef5b04445cc4581c8e86d8224eddd09f1157"));
        points.push(hex32("ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f"));
        points.push(hex32("edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f"));
        points.push(hex32("eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f"));
        for p in points {
            assert!(is_small_order(&p), "{p:?}");
            assert_eq!(parse_redemption_id(&redemption_id(&p)).unwrap_err().code, Code::RequestId);
        }
        assert!(!is_small_order(&public_key(&[9u8; 32])));
    }

    #[test]
    fn checksum_catches_a_typo() {
        let id = redemption_id(&public_key(&[3u8; 32]));
        let mut chars: Vec<char> = id.chars().collect();
        let i = PREFIX.len() + 10;
        chars[i] = if chars[i] == 'A' { 'B' } else { 'A' };
        let typo: String = chars.into_iter().collect();
        assert_eq!(parse_redemption_id(&typo).unwrap_err().code, Code::RequestId);
        assert_eq!(parse_redemption_id("MPR2-").unwrap_err().code, Code::RequestId);
        assert_eq!(parse_redemption_id("XYZ").unwrap_err().code, Code::RequestId);
        assert_eq!(parse_redemption_id(&"A".repeat(3000)).unwrap_err().code, Code::TooLarge);
    }

    fn hex32(text: &str) -> [u8; 32] {
        crate::codec::hex_array(text).unwrap()
    }
}
