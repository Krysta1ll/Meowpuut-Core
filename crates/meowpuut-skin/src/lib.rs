#![forbid(unsafe_code)]
//! Custom skins: the platform-neutral core of `plans/custom-skins.md` (v2).
//!
//! A skin is made and signed only on the developer's machine. This crate holds
//! the client side: it never signs or encrypts anything.
//!
//! * [`container`]: the two package kinds (recipient `.mpskin`, bundled
//!   `.mpskinb`) and their signed headers.
//! * [`trust`]: root keys, purpose-tagged subkey certificates, and the build
//!   policy that refuses `test-` kids outside Debug/Lab builds.
//! * [`revoke`]: the root-signed, add-only revocation set and the device's
//!   monotonic state (highest revision per skin and revocations).
//! * [`request`]: per-request X25519 device keys and the redemption ID.
//! * [`crypto`]: HKDF key wrapping and XChaCha20-Poly1305 per recipient.
//! * [`archive`]: the MPAR plaintext archive.
//! * [`image`], [`sound`]: PNG and WAV parsing, measuring and re-encoding.
//! * [`manifest`]: the validator and the canonical manifest rewrite.
//! * [`install`]: redeem, install-bundled, verify-on-load, list and remove.
//!
//! Two package kinds share the validator, the rewrite, the installed layout,
//! verify-on-load, the rollback guard and revocation: a **recipient**
//! package (signed and encrypted for one request on one device, redeemed on
//! Android) and a **bundled** package (signed by the same `skin` subkeys,
//! plaintext, not device-bound, shipped inside an app update; the only kind
//! iOS installs). `SEAL.md` specifies `seal.bin` and verify-on-load for a
//! verifier that does not link this crate (Swift/CryptoKit).
//!
//! Verification order of a recipient package (§3.2): size caps → certificate (root
//! signature, `skin` use, not revoked) → header signature (`verify_strict`)
//! → ciphertext length and hash → recipient → decryption → MPAR → validator →
//! rewrite → comparison with the header's `files`. Nothing is written before
//! every step has passed. A bundled package has no recipient and no decryption
//! step: the signed `payloadSha256` is the hash of the plaintext archive.

pub mod archive;
pub mod codec;
pub mod container;
pub mod crypto;
pub mod image;
pub mod install;
pub mod manifest;
pub mod names;
pub mod request;
pub mod revoke;
pub mod sound;
pub mod trust;

use std::fmt;

pub use zeroize::Zeroizing;

/// Size limits. Every one is checked before the bytes it guards are read.
pub mod limits {
    /// Largest header JSON (`header_len`).
    pub const HEADER: usize = 8192;
    /// Largest subkey certificate JSON.
    pub const CERT_JSON: usize = 1024;
    /// Largest file in a skin.
    pub const FILE: usize = 1024 * 1024;
    /// Largest sum of the files of a skin (archived and installed).
    pub const FILES_TOTAL: usize = 4 * 1024 * 1024;
    /// Most files in a skin, `manifest.json` included.
    pub const FILE_COUNT: usize = 48;
    /// Largest MPAR archive: the files plus every entry's own fields.
    pub const ARCHIVE: usize = FILES_TOTAL + 8 + FILE_COUNT * (1 + 64 + 4 + 32);
    /// Largest ciphertext (archive plus the Poly1305 tag).
    pub const PAYLOAD: usize = ARCHIVE + 16;
    /// Largest `.mpskin` file. The platform layer checks this before reading.
    pub const PACKAGE: usize = 12 + HEADER + 2 + CERT_JSON + 64 + 64 + PAYLOAD;
    /// Largest `seal.bin` (the package without its payload).
    pub const SEAL: usize = 12 + HEADER + 2 + CERT_JSON + 64 + 64;
    /// Largest `manifest.json`.
    pub const MANIFEST: usize = 64 * 1024;
    /// Largest `.mprevoke` file.
    pub const REVOCATION_FILE: usize = 128 * 1024;
    /// Most entries in the device's revocation set.
    pub const REVOCATIONS: usize = 2048;
    /// Most skin IDs remembered in the device's highest-revision table.
    pub const HIGHEST: usize = 1024;
    /// Largest monotonic state JSON accepted from the platform.
    pub const STATE_JSON: usize = 192 * 1024;
    /// Largest pasted redemption ID text.
    pub const REQUEST_TEXT: usize = 2048;
    /// Most skins installed on one device.
    pub const INSTALLED: usize = 12;
    /// Decoded RGBA of all images of one skin.
    pub const DECODED_IMAGES: usize = 4 * 1024 * 1024;
    /// Allocation limit handed to the PNG decoder.
    pub const PNG_DECODER: usize = 8 * 1024 * 1024;
    /// PCM bytes of all sounds of one skin.
    pub const PCM_TOTAL: usize = 512 * 1024;
    /// Most sound files of one skin.
    pub const SOUND_FILES: usize = 20;
}

/// Why something was refused. The numbers are the C ABI's codes and never change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum Code {
    /// An input is over its size limit.
    TooLarge = 1,
    /// The container or seal structure is broken (magic, lengths, truncation).
    Malformed = 2,
    /// The header JSON is not canonical or a field is missing, unknown or out of range.
    Header = 3,
    /// The subkey certificate is malformed, from an unknown root, or its root signature fails.
    Certificate = 4,
    /// The subkey is not certified for this use.
    KeyUse = 5,
    /// A `test-` kid was met by a build that does not accept test keys.
    TestKey = 6,
    /// The subkey, skin revision or serial is revoked.
    Revoked = 7,
    /// The header signature does not verify (`verify_strict`).
    Signature = 8,
    /// The ciphertext length or SHA-256 differs from the header.
    Payload = 9,
    /// The package was issued for another request (another device).
    NotForThisDevice = 10,
    /// The key agreement was not contributory (small-order key).
    KeyAgreement = 11,
    /// The content key or payload did not decrypt: the code does not match the package.
    Decrypt = 12,
    /// The MPAR archive is malformed or a file name is not allowed.
    Archive = 13,
    /// The manifest failed validation.
    Manifest = 14,
    /// An image failed validation.
    Image = 15,
    /// A sound failed validation.
    Sound = 16,
    /// The rewritten files differ from the signed list.
    Files = 17,
    /// The revision is older than one this device has seen.
    Downgrade = 18,
    /// The device already holds the most skins allowed.
    TooManySkins = 19,
    /// The skin is not made for this platform or needs a newer client.
    Client = 20,
    /// The monotonic state JSON is invalid.
    State = 21,
    /// Reading or writing the skins folder failed.
    Storage = 22,
    /// No such installed skin.
    NotInstalled = 23,
    /// An installed skin's files differ from its seal (changed, missing or extra).
    Tampered = 24,
    /// The installed skin was issued for another device.
    Binding = 25,
    /// An argument is NULL, not UTF-8 or malformed.
    Argument = 26,
    /// The output buffer is too small; nothing was changed.
    Buffer = 27,
    /// The revocation file is malformed or not signed by a root key.
    Revocation = 28,
    /// An unexpected internal failure.
    Internal = 29,
    /// The redemption ID is malformed, has a wrong checksum or a weak key.
    RequestId = 30,
    /// The random number generator failed.
    Random = 31,
    /// The package or seal is of the other kind (bundled where a recipient
    /// package is expected, or the reverse).
    Kind = 32,
}

impl Code {
    /// Every code, in numeric order.
    pub const ALL: [Code; 32] = [
        Code::TooLarge,
        Code::Malformed,
        Code::Header,
        Code::Certificate,
        Code::KeyUse,
        Code::TestKey,
        Code::Revoked,
        Code::Signature,
        Code::Payload,
        Code::NotForThisDevice,
        Code::KeyAgreement,
        Code::Decrypt,
        Code::Archive,
        Code::Manifest,
        Code::Image,
        Code::Sound,
        Code::Files,
        Code::Downgrade,
        Code::TooManySkins,
        Code::Client,
        Code::State,
        Code::Storage,
        Code::NotInstalled,
        Code::Tampered,
        Code::Binding,
        Code::Argument,
        Code::Buffer,
        Code::Revocation,
        Code::Internal,
        Code::RequestId,
        Code::Random,
        Code::Kind,
    ];

    /// The C ABI number.
    pub fn number(self) -> i32 {
        self as i32
    }

    /// The code for a C ABI number.
    pub fn from_number(number: i32) -> Option<Code> {
        Code::ALL.iter().copied().find(|c| c.number() == number)
    }

    /// A short English description (NUL-free, static).
    pub fn message(self) -> &'static str {
        match self {
            Code::TooLarge => "input is over its size limit",
            Code::Malformed => "the package structure is broken",
            Code::Header => "the package header is invalid",
            Code::Certificate => "the signing key certificate is invalid",
            Code::KeyUse => "the signing key is not certified for this use",
            Code::TestKey => "a test key is not accepted by this build",
            Code::Revoked => "the key, skin or serial is revoked",
            Code::Signature => "the signature does not verify",
            Code::Payload => "the payload differs from the header",
            Code::NotForThisDevice => "the package was issued for another request",
            Code::KeyAgreement => "the key agreement was not contributory",
            Code::Decrypt => "the code does not match the package",
            Code::Archive => "the archive is malformed",
            Code::Manifest => "the manifest is invalid",
            Code::Image => "an image is invalid",
            Code::Sound => "a sound is invalid",
            Code::Files => "the rewritten files differ from the signed list",
            Code::Downgrade => "an older revision than one already seen",
            Code::TooManySkins => "too many skins are installed",
            Code::Client => "the skin is not made for this client",
            Code::State => "the device state is invalid",
            Code::Storage => "reading or writing the skins folder failed",
            Code::NotInstalled => "the skin is not installed",
            Code::Tampered => "the installed files differ from the seal",
            Code::Binding => "the installed skin belongs to another device",
            Code::Argument => "an argument is invalid",
            Code::Buffer => "the output buffer is too small",
            Code::Revocation => "the revocation file is invalid",
            Code::Internal => "internal error",
            Code::RequestId => "the redemption ID is invalid",
            Code::Random => "the random number generator failed",
            Code::Kind => "the package is of the other kind (bundled or per-recipient)",
        }
    }
}

/// A refusal: a stable [`Code`] plus a detail for logs and the developer tools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    /// The stable reason.
    pub code: Code,
    /// What exactly failed (English, may name a field).
    pub detail: String,
}

impl Error {
    /// A refusal with a detail.
    pub fn new(code: Code, detail: impl Into<String>) -> Self {
        Self { code, detail: detail.into() }
    }
}

impl Error {
    /// `{"detail":"…","error":<code>}` (canonical JSON).
    pub fn to_json(&self) -> String {
        let value = serde_json::json!({"detail": self.detail, "error": self.code.number()});
        String::from_utf8(codec::canonical_json(&value)).unwrap_or_default()
    }
}

impl From<Code> for Error {
    fn from(code: Code) -> Self {
        Self { code, detail: String::new() }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.detail.is_empty() {
            write!(f, "{} ({})", self.code.message(), self.code.number())
        } else {
            write!(f, "{} ({}): {}", self.code.message(), self.code.number(), self.detail)
        }
    }
}

impl std::error::Error for Error {}

/// Result with [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Shorthand: `Err(Error::new(code, detail))`.
pub(crate) fn fail<T>(code: Code, detail: impl Into<String>) -> Result<T> {
    Err(Error::new(code, detail))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable_and_distinct() {
        for (i, code) in Code::ALL.iter().enumerate() {
            assert_eq!(code.number(), i as i32 + 1);
            assert_eq!(Code::from_number(code.number()), Some(*code));
            assert!(!code.message().contains('\0'));
        }
        assert_eq!(Code::from_number(0), None);
    }

    #[test]
    fn limits_hold_together() {
        const { assert!(limits::PACKAGE > limits::PAYLOAD + limits::HEADER) };
        assert_eq!(limits::SEAL + limits::PAYLOAD, limits::PACKAGE);
    }
}
