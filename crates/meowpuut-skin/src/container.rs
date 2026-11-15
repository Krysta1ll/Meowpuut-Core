//! The two package kinds and their seals (§3.2, §5.3).
//!
//! Both kinds share one framing (little endian):
//!
//! ```text
//! offset   size  field
//! 0        8     magic: "MPSKIN" 0x00 0x02 (recipient) or "MPSKBN" 0x00 0x01 (bundled)
//! 8        4     header_len n (1 ..= 8192)
//! 12       n     header: canonical JSON (sorted keys, no white space)
//! 12+n     2     cert_len c (1 ..= 1024)
//! 14+n     c     subkey certificate JSON (see `trust`)
//! 14+n+c   64    root signature of the certificate
//! 78+n+c   64    signature: Ed25519(subkey, DOMAIN(kind) ‖ header), verify_strict
//! 142+n+c  m     payload; m = header.payloadLen
//! ```
//!
//! The file ends exactly after the payload. `seal.bin` is the same bytes up to
//! and including the signature (no payload), kept unchanged in the installed
//! folder (see `SEAL.md` for a verifier that does not link this crate).
//!
//! The kind is explicit three times, and every one is bound to the others:
//! the magic picks the signature domain (`"meowpuut-skin/v2\0"` or
//! `"meowpuut-skin-bundled/v1\0"`), the header carries a kind-specific key
//! set (`"kind":"bundled"` exists only in a bundled header; a recipient
//! header has `recipient`, `nonce` and `aead`, which a bundled header must
//! not have), and every entry point names the kind it accepts. A
//! recipient-signed header therefore never verifies as a bundled one (and
//! the reverse), whatever its magic says.
//!
//! # Recipient package `.mpskin` (format 2): signed and encrypted for one request
//!
//! Header (every key required, no other key):
//!
//! ```text
//! {"aead":"xchacha20poly1305","created":<u32>,
//!  "files":[{"len":<u32>,"n":"<file name>","sha256":"<hex>"},…],   sorted by n, manifest.json included
//!  "format":2,"id":"<skin id>","kid":"<subkey kid>",
//!  "minClient":{"android":"<major>.<minor>",…},                      keys among android, ios, macos, windows
//!  "nonce":"<b64 24 bytes>","payloadLen":<u32>,"payloadSha256":"<hex of the ciphertext>",
//!  "recipient":{"code":"<b64 16>","eph":"<b64 32>","to":"<b64 16>","wrapped":"<b64 48>"},
//!  "rev":<u32 ≥ 1>}
//! ```
//!
//! payload: XChaCha20-Poly1305(CEK, header.nonce, MPAR, AAD).
//!
//! The header holds the ciphertext's hash, so it cannot be the AEAD's
//! associated data verbatim (the spec's "AAD = header" is circular). The AAD
//! is the canonical header **without `payloadSha256`**; every other header
//! field, `recipient` included, is bound to the plaintext, and the signature
//! binds `payloadSha256` to the ciphertext. The redemption code is bound to
//! the content by `content_hash`: SHA-256 of the canonical header without
//! `payloadSha256` and `recipient` (see `crypto`).
//!
//! # Bundled package `.mpskinb` (format 1): signed, not encrypted, not device-bound
//!
//! Shipped inside an app update (iOS and Android app bundles). Confidentiality
//! is pointless there, so the payload is the plaintext MPAR archive.
//!
//! ```text
//! {"created":<u32>,"files":[…as above…],"format":1,"id":"<skin id>","kind":"bundled",
//!  "kid":"<subkey kid>","minClient":{…},"payloadLen":<u32>,"payloadSha256":"<hex of the MPAR>",
//!  "rev":<u32 ≥ 1>}
//! ```
//!
//! In both kinds `files` lists every file **after** the install-time rewrite
//! (§5.2): the signature covers every byte the keyboard reads.

use std::collections::BTreeMap;

use ed25519_dalek::Signature;
use serde_json::{Map, Value};

use crate::codec::{b64_array, canonical_json, hex_array, parse_canonical, sha256};
use crate::names::{file_name, is_name, skin_ref, FileKind};
use crate::revoke::Revocations;
use crate::trust::{exact_keys, u32_field, verify_cert, Cert, Policy};
use crate::{fail, limits, Code, Error, Result};

/// Magic of a recipient `.mpskin` (and its `seal.bin`).
pub const MAGIC: [u8; 8] = *b"MPSKIN\x00\x02";
/// Domain separation of a recipient header signature.
pub const DOMAIN: &[u8] = b"meowpuut-skin/v2\0";
/// Magic of a bundled `.mpskinb` (and its `seal.bin`).
pub const BUNDLED_MAGIC: [u8; 8] = *b"MPSKBN\x00\x01";
/// Domain separation of a bundled header signature.
pub const BUNDLED_DOMAIN: &[u8] = b"meowpuut-skin-bundled/v1\0";
/// The value of a bundled header's `kind`.
pub const BUNDLED_KIND: &str = "bundled";
/// `format` of a bundled header.
pub const BUNDLED_FORMAT: u64 = 1;

/// The two package kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Signed and encrypted for one request on one device (`.mpskin`, redemption; Android).
    Recipient,
    /// Signed, plaintext, not device-bound (`.mpskinb`, shipped in an app update).
    Bundled,
}

impl Kind {
    /// The magic of this kind.
    pub fn magic(self) -> [u8; 8] {
        match self {
            Kind::Recipient => MAGIC,
            Kind::Bundled => BUNDLED_MAGIC,
        }
    }

    /// The signature domain of this kind.
    pub fn domain(self) -> &'static [u8] {
        match self {
            Kind::Recipient => DOMAIN,
            Kind::Bundled => BUNDLED_DOMAIN,
        }
    }

    /// The kind with this magic.
    pub fn from_magic(magic: &[u8]) -> Option<Kind> {
        if magic == MAGIC {
            Some(Kind::Recipient)
        } else if magic == BUNDLED_MAGIC {
            Some(Kind::Bundled)
        } else {
            None
        }
    }

    /// `"recipient"` or `"bundled"`.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Recipient => "recipient",
            Kind::Bundled => BUNDLED_KIND,
        }
    }
}
/// The only AEAD.
pub const AEAD: &str = "xchacha20poly1305";
/// Platforms a header may name in `minClient`.
pub const PLATFORMS: [&str; 4] = ["android", "ios", "macos", "windows"];

/// One installed file as signed in the header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    /// File name (see `names::file_name`).
    pub name: String,
    /// Length in bytes.
    pub len: u32,
    /// SHA-256.
    pub sha256: [u8; 32],
}

/// The `recipient` field: one request on one device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipient {
    /// SHA-256 of the request public key, first 16 bytes.
    pub to: [u8; 16],
    /// The issuer's ephemeral X25519 public key.
    pub eph: [u8; 32],
    /// The content key wrapped with the KEK (32 bytes plus the Poly1305 tag).
    pub wrapped: [u8; 48],
    /// The redemption code: 16 random bytes, the HKDF salt.
    pub code: [u8; 16],
}

/// The encryption fields of a recipient header.
#[derive(Debug, Clone)]
pub struct Encryption {
    /// Payload nonce.
    pub nonce: [u8; 24],
    /// The recipient.
    pub recipient: Recipient,
    /// Canonical JSON of the `recipient` object (the content of `binding.bin`).
    pub recipient_json: Vec<u8>,
    /// SHA-256 of the canonical header without `payloadSha256` and `recipient`:
    /// binds the code to this content (the files' hashes, id, rev, nonce, …).
    pub content_hash: [u8; 32],
    /// The AEAD's associated data: the canonical header without `payloadSha256`.
    pub aad: Vec<u8>,
}

/// A parsed, not yet verified header.
#[derive(Debug, Clone)]
pub struct Header {
    /// Which kind of package.
    pub kind: Kind,
    /// Skin ID.
    pub id: String,
    /// Revision.
    pub rev: u32,
    /// Signing subkey's kid.
    pub kid: String,
    /// Creation time (informational; never used for trust).
    pub created: u32,
    /// Lowest client version per platform.
    pub min_client: BTreeMap<String, (u32, u32)>,
    /// Payload length (ciphertext of a recipient package, MPAR of a bundled one).
    pub payload_len: u32,
    /// Payload SHA-256.
    pub payload_sha256: [u8; 32],
    /// Installed files, sorted by name.
    pub files: Vec<FileEntry>,
    /// The encryption fields: `Some` exactly for [`Kind::Recipient`].
    pub encryption: Option<Encryption>,
}

impl Header {
    /// `"<id>@<rev>"`.
    pub fn skin_ref(&self) -> String {
        skin_ref(&self.id, self.rev)
    }

    /// The recipient (recipient packages only).
    pub fn recipient(&self) -> Option<&Recipient> {
        self.encryption.as_ref().map(|e| &e.recipient)
    }
}

/// Parses `"<major>.<minor>"`.
pub fn parse_version(text: &str) -> Option<(u32, u32)> {
    let (a, b) = text.split_once('.')?;
    let num = |t: &str| {
        (!t.is_empty() && t.len() <= 6 && t.bytes().all(|c| c.is_ascii_digit()) && (t == "0" || !t.starts_with('0')))
            .then(|| t.parse::<u32>().ok())
            .flatten()
    };
    Some((num(a)?, num(b)?))
}

fn header_error(what: &str) -> Error {
    Error::new(Code::Header, what.to_string())
}

/// Parses a recipient header JSON: canonical form, exact fields, every value in range.
pub fn parse_header(bytes: &[u8]) -> Result<Header> {
    parse_header_of(Kind::Recipient, bytes)
}

/// Parses a header of `kind`: canonical form, exactly that kind's fields,
/// every value in range.
pub fn parse_header_of(kind: Kind, bytes: &[u8]) -> Result<Header> {
    let value = parse_canonical(bytes).ok_or_else(|| header_error("not canonical JSON"))?;
    let map = value.as_object().ok_or_else(|| header_error("not an object"))?;
    let recipient_keys = [
        "aead",
        "created",
        "files",
        "format",
        "id",
        "kid",
        "minClient",
        "nonce",
        "payloadLen",
        "payloadSha256",
        "recipient",
        "rev",
    ];
    let bundled_keys = ["created", "files", "format", "id", "kid", "kind", "minClient", "payloadLen", "payloadSha256", "rev"];
    match kind {
        Kind::Recipient => {
            if !exact_keys(map, &recipient_keys) {
                return Err(header_error("fields"));
            }
            if map["format"].as_u64() != Some(2) {
                return Err(header_error("format"));
            }
            if map["aead"].as_str() != Some(AEAD) {
                return Err(header_error("aead"));
            }
        }
        Kind::Bundled => {
            if !exact_keys(map, &bundled_keys) {
                return Err(header_error("fields"));
            }
            if map["format"].as_u64() != Some(BUNDLED_FORMAT) {
                return Err(header_error("format"));
            }
            if map["kind"].as_str() != Some(BUNDLED_KIND) {
                return Err(header_error("kind"));
            }
        }
    }
    let id = map["id"].as_str().filter(|s| is_name(s)).ok_or_else(|| header_error("id"))?;
    let rev = u32_field(map, "rev").filter(|r| *r >= 1).ok_or_else(|| header_error("rev"))?;
    let kid = map["kid"].as_str().filter(|s| is_name(s)).ok_or_else(|| header_error("kid"))?;
    let created = u32_field(map, "created").ok_or_else(|| header_error("created"))?;
    let mut min_client = BTreeMap::new();
    let mc = map["minClient"].as_object().ok_or_else(|| header_error("minClient"))?;
    for (platform, version) in mc {
        let version = version.as_str().and_then(parse_version);
        match version {
            Some(v) if PLATFORMS.contains(&platform.as_str()) => min_client.insert(platform.clone(), v),
            _ => return Err(header_error("minClient entry")),
        };
    }
    let payload_len = u32_field(map, "payloadLen").ok_or_else(|| header_error("payloadLen"))?;
    let (low, high) = match kind {
        Kind::Recipient => (16, limits::PAYLOAD),
        Kind::Bundled => (8, limits::ARCHIVE),
    };
    if payload_len as usize > high || (payload_len as usize) < low {
        return fail(Code::TooLarge, "payloadLen");
    }
    let payload_sha256 =
        map["payloadSha256"].as_str().and_then(hex_array).ok_or_else(|| header_error("payloadSha256"))?;
    let files = parse_files(&map["files"])?;
    let encryption = match kind {
        Kind::Bundled => None,
        Kind::Recipient => {
            let nonce = map["nonce"].as_str().and_then(b64_array).ok_or_else(|| header_error("nonce"))?;
            let r = map["recipient"].as_object().ok_or_else(|| header_error("recipient"))?;
            if !exact_keys(r, &["code", "eph", "to", "wrapped"]) {
                return Err(header_error("recipient fields"));
            }
            let field = |k: &str| r[k].as_str().ok_or_else(|| header_error(k));
            let recipient = Recipient {
                to: b64_array(field("to")?).ok_or_else(|| header_error("to"))?,
                eph: b64_array(field("eph")?).ok_or_else(|| header_error("eph"))?,
                wrapped: b64_array(field("wrapped")?).ok_or_else(|| header_error("wrapped"))?,
                code: b64_array(field("code")?).ok_or_else(|| header_error("code"))?,
            };
            let recipient_json = canonical_json(&map["recipient"]);
            let mut without: Map<String, Value> = map.clone();
            without.remove("payloadSha256");
            let aad = canonical_json(&Value::Object(without.clone()));
            without.remove("recipient");
            let content_hash = sha256(&canonical_json(&Value::Object(without)));
            Some(Encryption { nonce, recipient, recipient_json, content_hash, aad })
        }
    };
    Ok(Header {
        kind,
        id: id.to_string(),
        rev,
        kid: kid.to_string(),
        created,
        min_client,
        payload_len,
        payload_sha256,
        files,
        encryption,
    })
}

fn parse_files(value: &Value) -> Result<Vec<FileEntry>> {
    let list = value.as_array().ok_or_else(|| header_error("files"))?;
    if list.is_empty() || list.len() > limits::FILE_COUNT {
        return Err(header_error("file count"));
    }
    let mut out: Vec<FileEntry> = Vec::with_capacity(list.len());
    let mut total = 0usize;
    for item in list {
        let f = item.as_object().filter(|f| exact_keys(f, &["len", "n", "sha256"])).ok_or_else(|| header_error("file"))?;
        let name = f["n"].as_str().filter(|n| file_name(n).is_some()).ok_or_else(|| header_error("file name"))?;
        if file_name(name).is_some_and(|(_, kind)| kind == FileKind::Json) && name != "manifest.json" {
            return Err(header_error("only manifest.json may be JSON"));
        }
        let len = u32_field(f, "len").ok_or_else(|| header_error("file len"))?;
        if len as usize > limits::FILE {
            return fail(Code::TooLarge, format!("file {name}"));
        }
        total += len as usize;
        let sha = f["sha256"].as_str().and_then(hex_array).ok_or_else(|| header_error("file sha256"))?;
        if out.last().is_some_and(|last| last.name.as_str() >= name) {
            return Err(header_error("files must be sorted and unique"));
        }
        out.push(FileEntry { name: name.to_string(), len, sha256: sha });
    }
    if total > limits::FILES_TOTAL {
        return fail(Code::TooLarge, "files");
    }
    if !out.iter().any(|f| f.name == "manifest.json") {
        return Err(header_error("no manifest.json"));
    }
    Ok(out)
}

/// The signed part of a package (also the whole of `seal.bin`), split but
/// not yet verified or parsed.
#[derive(Debug, Clone, Copy)]
pub struct Seal<'a> {
    /// The kind named by the magic (it picks the signature domain).
    pub kind: Kind,
    /// Header JSON bytes.
    pub header_bytes: &'a [u8],
    /// Certificate JSON.
    pub cert_json: &'a [u8],
    /// Root signature of the certificate.
    pub cert_signature: [u8; 64],
    /// Subkey signature of the header.
    pub signature: [u8; 64],
    /// Length of the sealed prefix (magic through signature).
    pub len: usize,
}

/// Splits a package into its seal and payload. Structure only: nothing is
/// verified or parsed yet. Every length is checked against its limit before
/// anything is sliced.
pub fn split(bytes: &[u8]) -> Result<(Seal<'_>, &[u8])> {
    if bytes.len() > limits::PACKAGE {
        return fail(Code::TooLarge, "package");
    }
    let seal = split_seal(bytes)?;
    Ok((seal, &bytes[seal.len..]))
}

/// Splits a `seal.bin`: the sealed prefix must be the whole input.
pub fn parse_seal(bytes: &[u8]) -> Result<Seal<'_>> {
    if bytes.len() > limits::SEAL {
        return fail(Code::TooLarge, "seal");
    }
    let seal = split_seal(bytes)?;
    if seal.len != bytes.len() {
        return fail(Code::Malformed, "bytes after the signature");
    }
    Ok(seal)
}

fn split_seal(bytes: &[u8]) -> Result<Seal<'_>> {
    let short = || Error::new(Code::Malformed, "truncated");
    if bytes.len() < 12 {
        return Err(short());
    }
    let kind = Kind::from_magic(&bytes[..8]).ok_or_else(|| Error::new(Code::Malformed, "magic"))?;
    let n = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
    if n == 0 || n > limits::HEADER {
        return fail(Code::TooLarge, "header_len");
    }
    let header_bytes = bytes.get(12..12 + n).ok_or_else(short)?;
    let c_at = 12 + n;
    let c = u16::from_le_bytes([*bytes.get(c_at).ok_or_else(short)?, *bytes.get(c_at + 1).ok_or_else(short)?]) as usize;
    if c == 0 || c > limits::CERT_JSON {
        return fail(Code::TooLarge, "cert_len");
    }
    let cert_json = bytes.get(c_at + 2..c_at + 2 + c).ok_or_else(short)?;
    let at = c_at + 2 + c;
    let cert_signature: [u8; 64] = bytes.get(at..at + 64).ok_or_else(short)?.try_into().map_err(|_| short())?;
    let signature: [u8; 64] = bytes.get(at + 64..at + 128).ok_or_else(short)?.try_into().map_err(|_| short())?;
    Ok(Seal { kind, header_bytes, cert_json, cert_signature, signature, len: at + 128 })
}

/// Verifies a seal of either kind in the order of §3.2: the certificate
/// under the policy (root signature, `skin` use, kid not revoked), then the
/// strict header signature over the raw header bytes under the domain of the
/// seal's kind (its magic); only then is the header parsed as that kind, its
/// kid compared with the certificate and its `id@rev` checked against the
/// revocations. Returns the parsed header and the certificate.
pub fn open_seal(seal: &Seal<'_>, policy: &Policy, revoked: &Revocations) -> Result<(Header, Cert)> {
    let cert = verify_cert(seal.cert_json, &seal.cert_signature, policy, revoked)?;
    if !cert.allows("skin") {
        return fail(Code::KeyUse, format!("{} is not certified for skins", cert.kid));
    }
    let message = [seal.kind.domain(), seal.header_bytes].concat();
    cert.key
        .verify_strict(&message, &Signature::from_bytes(&seal.signature))
        .map_err(|_| Error::from(Code::Signature))?;
    let header = parse_header_of(seal.kind, seal.header_bytes)?;
    if cert.kid != header.kid {
        return fail(Code::Header, "kid differs from the certificate");
    }
    if revoked.skins.contains(&header.skin_ref()) {
        return fail(Code::Revoked, header.skin_ref());
    }
    Ok((header, cert))
}

/// [`open_seal`] for a seal that must be of `kind` (an entry point's own
/// kind): a seal of the other kind is refused with [`Code::Kind`] before
/// anything is verified.
pub fn open_seal_of(kind: Kind, seal: &Seal<'_>, policy: &Policy, revoked: &Revocations) -> Result<(Header, Cert)> {
    if seal.kind != kind {
        return fail(Code::Kind, format!("a {} package, expected {}", seal.kind.name(), kind.name()));
    }
    open_seal(seal, policy, revoked)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("1.2"), Some((1, 2)));
        assert_eq!(parse_version("0.10"), Some((0, 10)));
        for bad in ["1", "1.", ".1", "01.2", "1.02", "1.2.3", "-1.2", "1.x", "9999999.1"] {
            assert_eq!(parse_version(bad), None, "{bad}");
        }
    }

    #[test]
    fn split_checks_lengths_before_slicing() {
        assert_eq!(split(&[]).unwrap_err().code, Code::Malformed);
        let mut b = MAGIC.to_vec();
        b.extend_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(split(&b).unwrap_err().code, Code::TooLarge);
        let mut b = MAGIC.to_vec();
        b.extend_from_slice(&100u32.to_le_bytes());
        assert_eq!(split(&b).unwrap_err().code, Code::Malformed);
        let mut b = MAGIC.to_vec();
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(b"{");
        b.extend_from_slice(&u16::MAX.to_le_bytes());
        assert_eq!(split(&b).unwrap_err().code, Code::TooLarge);
        assert_eq!(split(&vec![0u8; limits::PACKAGE + 1]).unwrap_err().code, Code::TooLarge);
        assert_eq!(parse_seal(&vec![0u8; limits::SEAL + 1]).unwrap_err().code, Code::TooLarge);
    }
}
