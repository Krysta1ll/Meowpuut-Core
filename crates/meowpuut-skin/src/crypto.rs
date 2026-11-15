//! Per-recipient content keys (§3.2, §6.2).
//!
//! ```text
//! shared = X25519(request_secret, recipient.eph)             must be contributory
//! info   = "meowpuut-skin-kek/v2" ‖ u8 len(id) ‖ id ‖ u32le rev ‖ request_pub ‖ eph_pub ‖ content_hash
//! KEK    = HKDF-SHA256(salt = recipient.code, ikm = shared, info) → 32 bytes
//! CEK    = XChaCha20-Poly1305-open(KEK, nonce = 24 zero bytes, recipient.wrapped, AAD = "meowpuut-skin-cek/v2")
//! MPAR   = XChaCha20-Poly1305-open(CEK, header.nonce, payload, AAD = canonical header without payloadSha256)
//! ```
//!
//! `content_hash` is the SHA-256 of the canonical header without
//! `payloadSha256` and `recipient` (it lists every installed file's SHA-256),
//! so a code opens only the package it was issued with (review L2). The KEK
//! is used once (a fresh ephemeral key and code per package), so the fixed
//! zero nonce of the wrap never repeats under one key. Every secret buffer is
//! zeroized on drop.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

use crate::container::Header;
use crate::{Code, Error, Result};

/// HKDF info prefix of the KEK.
pub const KEK_INFO: &[u8] = b"meowpuut-skin-kek/v2";
/// Associated data of the wrapped CEK.
pub const CEK_AAD: &[u8] = b"meowpuut-skin-cek/v2";

/// The HKDF info for one recipient of one package.
pub fn kek_info(id: &str, rev: u32, request_pub: &[u8; 32], eph_pub: &[u8; 32], content_hash: &[u8; 32]) -> Vec<u8> {
    let mut info = Vec::with_capacity(KEK_INFO.len() + 1 + id.len() + 4 + 96);
    info.extend_from_slice(KEK_INFO);
    info.push(id.len() as u8);
    info.extend_from_slice(id.as_bytes());
    info.extend_from_slice(&rev.to_le_bytes());
    info.extend_from_slice(request_pub);
    info.extend_from_slice(eph_pub);
    info.extend_from_slice(content_hash);
    info
}

/// The KEK from an X25519 shared secret.
pub fn derive_kek(shared: &[u8; 32], code: &[u8; 16], info: &[u8]) -> Zeroizing<[u8; 32]> {
    let mut kek = Zeroizing::new([0u8; 32]);
    let hk = Hkdf::<Sha256>::new(Some(code), shared);
    // 32 bytes is far below HKDF-SHA256's 8160-byte limit.
    let _ = hk.expand(info, kek.as_mut());
    kek
}

/// The request's X25519 agreement with the issuer's ephemeral key; refuses a
/// non-contributory result (small-order ephemeral key).
pub fn agree(request_secret: &[u8; 32], eph_pub: &[u8; 32]) -> Result<Zeroizing<[u8; 32]>> {
    let secret = StaticSecret::from(*request_secret);
    let shared = secret.diffie_hellman(&PublicKey::from(*eph_pub));
    if !shared.was_contributory() {
        return Err(Error::new(Code::KeyAgreement, "small-order ephemeral key"));
    }
    Ok(Zeroizing::new(shared.to_bytes()))
}

/// Unwraps this recipient's CEK with the request secret whose public key is `request_pub`.
/// A bundled header has no recipient: [`Code::Kind`].
pub fn unwrap_cek(header: &Header, request_secret: &[u8; 32], request_pub: &[u8; 32]) -> Result<Zeroizing<[u8; 32]>> {
    let enc = header.encryption.as_ref().ok_or_else(|| Error::new(Code::Kind, "a bundled package has no recipient"))?;
    let r = &enc.recipient;
    let shared = agree(request_secret, &r.eph)?;
    let info = kek_info(&header.id, header.rev, request_pub, &r.eph, &enc.content_hash);
    let kek = derive_kek(&shared, &r.code, &info);
    let cipher = XChaCha20Poly1305::new_from_slice(kek.as_ref()).map_err(|_| Error::from(Code::Internal))?;
    let nonce = XNonce::default();
    let opened = Zeroizing::new(
        cipher
            .decrypt(&nonce, Payload { msg: &r.wrapped, aad: CEK_AAD })
            .map_err(|_| Error::new(Code::Decrypt, "content key"))?,
    );
    let mut cek = Zeroizing::new([0u8; 32]);
    if opened.len() != 32 {
        return Err(Error::new(Code::Decrypt, "content key length"));
    }
    cek.copy_from_slice(&opened);
    Ok(cek)
}

/// Decrypts the payload into the MPAR plaintext (zeroized on drop). `aad` is `Header::aad`.
pub fn open_payload(cek: &[u8; 32], nonce: &[u8; 24], aad: &[u8], payload: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    let cipher = XChaCha20Poly1305::new_from_slice(cek).map_err(|_| Error::from(Code::Internal))?;
    let nonce = XNonce::try_from(&nonce[..]).map_err(|_| Error::from(Code::Internal))?;
    cipher
        .decrypt(&nonce, Payload { msg: payload, aad })
        .map(Zeroizing::new)
        .map_err(|_| Error::new(Code::Decrypt, "payload"))
}
