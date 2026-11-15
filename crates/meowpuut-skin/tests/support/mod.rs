//! Test-only issuing side: the steps the private signer performs, with TEST
//! keys derived from public seeds (`SHA-256("meowpuut-skin test key " ‖ label)`).
//! The crate itself has no signing or encrypting code; this module exists so
//! the tests can build packages, revocation files and tampered variants.
#![allow(dead_code)]

use std::path::PathBuf;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use ed25519_dalek::{Signer, SigningKey};
use meowpuut_skin::archive::Entry;
use meowpuut_skin::codec::{b64_encode, canonical_json, hex_encode, sha256};
use meowpuut_skin::container::{BUNDLED_DOMAIN, BUNDLED_MAGIC, DOMAIN, MAGIC};
use meowpuut_skin::crypto::{derive_kek, kek_info, CEK_AAD};
use meowpuut_skin::image::{encode, Rgba};
use meowpuut_skin::manifest;
use meowpuut_skin::request::{public_key, recipient_tag};
use meowpuut_skin::trust::CERT_DOMAIN;
use serde_json::{json, Value};

/// A test seed.
pub fn seed(label: &str) -> [u8; 32] {
    sha256(format!("meowpuut-skin test key {label}").as_bytes())
}

pub fn signing(label: &str) -> SigningKey {
    SigningKey::from_bytes(&seed(label))
}

pub const ROOT_KID: &str = "test-root-1";
pub const SKIN_KID: &str = "test-2026-skin";
pub const CODE_KID: &str = "test-2026-code";

pub fn root() -> SigningKey {
    signing(ROOT_KID)
}

/// A subkey certificate: (JSON, root signature).
pub fn cert(root: &SigningKey, root_kid: &str, kid: &str, subkey: &SigningKey, uses: &[&str]) -> (Vec<u8>, [u8; 64]) {
    let json = canonical_json(&json!({
        "issued": 1_763_078_400u32,
        "kid": kid,
        "pub": b64_encode(&subkey.verifying_key().to_bytes()),
        "root": root_kid,
        "uses": uses,
    }));
    let sig = root.sign(&[CERT_DOMAIN, &json].concat()).to_bytes();
    (json, sig)
}

/// A `.mprevoke` signed by `key`.
pub fn revocation(key: &SigningKey, root_kid: &str, seq: u64, entries: &[(&str, &str)]) -> Vec<u8> {
    let list: Vec<Value> = entries.iter().map(|(k, v)| json!({"kind": k, "value": v})).collect();
    let body = canonical_json(&json!({"entries": list, "root": root_kid, "seq": seq}));
    let sig = key.sign(&[meowpuut_skin::revoke::DOMAIN, &body].concat()).to_bytes();
    let mut out = meowpuut_skin::revoke::MAGIC.to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out.extend_from_slice(&sig);
    out
}

pub fn request_secret(label: &str) -> [u8; 32] {
    seed(&format!("request {label}"))
}

pub fn request_public(label: &str) -> [u8; 32] {
    public_key(&request_secret(label))
}

// ---------- assets ----------

pub fn blank(w: u32, h: u32) -> Rgba {
    Rgba { width: w, height: h, pixels: vec![0; (w * h * 4) as usize] }
}

pub fn fill(img: &mut Rgba, x0: u32, y0: u32, x1: u32, y1: u32, rgba: [u8; 4]) {
    for y in y0..=y1 {
        for x in x0..=x1 {
            let i = ((y * img.width + x) * 4) as usize;
            img.pixels[i..i + 4].copy_from_slice(&rgba);
        }
    }
}

/// A top-pose cat at 32×40 pt, anchor (0.5, 0.75): body cut on row 90, two paws below.
pub fn top_cat() -> Rgba {
    let mut img = blank(96, 120);
    fill(&mut img, 20, 20, 75, 89, [240, 200, 180, 255]);
    fill(&mut img, 28, 90, 35, 99, [250, 220, 200, 255]);
    fill(&mut img, 60, 90, 67, 99, [250, 220, 200, 255]);
    img
}

/// A side-pose cat at 32×40 pt, anchor (0.25, 0.5).
pub fn side_cat() -> Rgba {
    let mut img = blank(96, 120);
    fill(&mut img, 20, 30, 70, 90, [120, 110, 100, 255]);
    img
}

/// A trim at 120×32 pt: a rim stroke 3 pt in from every edge, clear inside.
pub fn trim() -> Rgba {
    let mut img = blank(360, 96);
    let c = [255, 180, 190, 230];
    fill(&mut img, 6, 6, 353, 11, c);
    fill(&mut img, 6, 84, 353, 89, c);
    fill(&mut img, 6, 6, 11, 89, c);
    fill(&mut img, 348, 6, 353, 89, c);
    img
}

/// An end ornament at 20×20 pt.
pub fn end_piece() -> Rgba {
    let mut img = blank(60, 60);
    for y in 0..60u32 {
        for x in 0..60u32 {
            let (dx, dy) = (x as i32 - 30, y as i32 - 30);
            if dx * dx + dy * dy <= 25 * 25 {
                fill(&mut img, x, y, x, y, [200, 80, 100, 255]);
            }
        }
    }
    img
}

/// Inserts an ancillary chunk after IHDR and junk after IEND (the rewrite drops both).
pub fn with_extras(png: &[u8]) -> Vec<u8> {
    let mut out = png[..33].to_vec();
    let data = b"Comment\0drawn by a test";
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut chunk = b"tEXt".to_vec();
    chunk.extend_from_slice(data);
    out.extend_from_slice(&chunk);
    out.extend_from_slice(&crc32(&chunk).to_be_bytes());
    out.extend_from_slice(&png[33..]);
    out.extend_from_slice(b"trailing bytes after IEND");
    out
}

pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// A PNG chunk (length, type, data, CRC).
pub fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = (data.len() as u32).to_be_bytes().to_vec();
    let mut body = kind.to_vec();
    body.extend_from_slice(data);
    out.extend_from_slice(&body);
    out.extend_from_slice(&crc32(&body).to_be_bytes());
    out
}

/// sin by range reduction and a Taylor series: only IEEE basic operations,
/// so the golden vectors come out the same on every platform's libm.
pub fn dsin(x: f64) -> f64 {
    let tau = 2.0 * std::f64::consts::PI;
    let mut r = x - (x / tau).round() * tau;
    if r > std::f64::consts::PI {
        r -= tau;
    }
    let (mut term, mut sum) = (r, r);
    for k in 1..15 {
        term *= -r * r / ((2 * k) as f64 * (2 * k + 1) as f64);
        sum += term;
    }
    sum
}

pub fn dcos(x: f64) -> f64 {
    dsin(x + std::f64::consts::FRAC_PI_2)
}

/// A Hann-windowed 1 kHz tone, `amplitude` of full scale.
pub fn tone(amplitude: f64, n: usize) -> Vec<i16> {
    (0..n)
        .map(|i| {
            let t = i as f64 / 44_100.0;
            let w = 0.5 - 0.5 * dcos(2.0 * std::f64::consts::PI * i as f64 / (n - 1) as f64);
            let edge = i == 0 || i == n - 1;
            if edge { 0 } else { (amplitude * w * dsin(2.0 * std::f64::consts::PI * 1000.0 * t) * 32767.0).round() as i16 }
        })
        .collect()
}

/// A WAV with an extra LIST chunk before `fmt ` (the rewrite drops it).
pub fn wav_with_extras(samples: &[i16]) -> Vec<u8> {
    let clean = meowpuut_skin::sound::encode(samples);
    let mut out = b"RIFF\0\0\0\0WAVE".to_vec();
    out.extend_from_slice(b"LIST\x06\0\0\0INFOab");
    out.extend_from_slice(&clean[12..]);
    let riff = (out.len() - 8) as u32;
    out[4..8].copy_from_slice(&riff.to_le_bytes());
    out
}

/// The sample skin's files (before rewriting), without the manifest.
pub fn sample_files() -> Vec<(String, Vec<u8>)> {
    vec![
        ("frame.png".into(), with_extras(&encode(&trim()).unwrap())),
        ("key-1.wav".into(), wav_with_extras(&tone(0.0731, 2205))),
        ("mimimo_peek.png".into(), with_extras(&encode(&top_cat()).unwrap())),
        ("momo_hold.png".into(), encode(&side_cat()).unwrap()),
        ("return-1.wav".into(), meowpuut_skin::sound::encode(&tone(0.06, 4410))),
        ("right.png".into(), encode(&end_piece()).unwrap()),
        ("space-1.wav".into(), meowpuut_skin::sound::encode(&tone(0.07, 2646))),
    ]
}

/// The sample manifest (input form).
pub fn sample_manifest(id: &str, rev: u32, serial: &str) -> Value {
    json!({
        "schema": 1, "id": id, "rev": rev, "serial": serial,
        "names": {"hant": "櫻花奶茶", "hans": "樱花奶茶", "ja": "桜ミルクティー", "en": "Sakura Tea"},
        "base": "current", "festivals": "suppress",
        "palette": {
            "light": {"paper": "#FFF8F0", "paper2": "#F7EDE4", "ink": "#3A2A2A", "inkSecondary": "#6B5555",
                      "pill": ["#F4D6D9", "#EBC3C8"], "pillText": "#3A2A2A", "stamp": "#B4475A", "accent": "#A23B4F"},
            "dark": {"paper": "#2A2224", "paper2": "#352B2E", "ink": "#F5ECEC", "inkSecondary": "#CDBFC0",
                     "pill": ["#5A3D43", "#4A3237"], "pillText": "#F5ECEC", "stamp": "#E9A3B0", "accent": "#A23B4F"},
            "contrastPill": "#7A4E52"
        },
        "glass": {"shape": "cloud", "lockShape": false, "strength": "balanced", "deckTint": 0.06},
        "keys": {"tint": 0.12, "rim": true,
                 "light": {"character": "#FFFFFF", "utility": "#E8DADA", "text": "#3A2A2A"},
                 "dark": {"character": "#4A3A3D", "utility": "#3A2D30", "text": "#F5ECEC"}},
        "cats": {"seating": "perched",
                 "sprites": {"mimimo_edge_peek": {"image": "mimimo_peek", "size": [32, 40], "anchor": [0.5, 0.75]},
                             "momo_side_hold": {"image": "momo_hold", "size": [32, 40], "anchor": [0.25, 0.5]}},
                 "trim": {"image": "frame", "size": [120, 32], "caps": [0, 16, 0, 16], "safeText": [12, 12, 12, 12]},
                 "ends": {"right": {"image": "right", "size": [20, 20]}}},
        "sounds": {"files": {"key": ["key-1"], "space": ["space-1"], "return": ["return-1"]}, "gainDb": 0}
    })
}

/// MPAR bytes of sorted entries.
pub fn mpar(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = b"MPAR".to_vec();
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for (name, data) in entries {
        out.push(name.len() as u8);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&sha256(data));
        out.extend_from_slice(data);
    }
    out
}

type HeaderEdit = Box<dyn Fn(&mut Value)>;

/// Everything one `issue` decides; every field can be changed to build a variant.
pub struct Issue {
    pub id: String,
    pub rev: u32,
    pub manifest: Value,
    pub files: Vec<(String, Vec<u8>)>,
    pub signer: SigningKey,
    pub kid: String,
    pub cert: (Vec<u8>, [u8; 64]),
    pub request_pub: [u8; 32],
    pub eph_secret: [u8; 32],
    pub eph_pub_override: Option<[u8; 32]>,
    pub code: [u8; 16],
    pub cek: [u8; 32],
    pub nonce: [u8; 24],
    pub min_client: Value,
    /// Replaces the archive (already sorted entries are not required).
    pub archive_override: Option<Vec<u8>>,
    /// Replaces the signed `files` list.
    pub files_override: Option<Value>,
    /// Edits the header just before it is signed (after encryption).
    pub edit_signed: Option<HeaderEdit>,
    /// Edits the header before the content hash and encryption.
    pub edit_early: Option<HeaderEdit>,
}

impl Issue {
    /// The sample skin for request `label`.
    pub fn sample(label: &str) -> Issue {
        let id = "sakura_tea".to_string();
        let rev = 3;
        let root = root();
        let signer = signing(SKIN_KID);
        let first = |s: [u8; 32], n: usize| s[..n].to_vec();
        Issue {
            manifest: sample_manifest(&id, rev, "0123456789abcdef"),
            id,
            rev,
            files: sample_files(),
            cert: cert(&root, ROOT_KID, SKIN_KID, &signer, &["code", "skin"]),
            signer,
            kid: SKIN_KID.into(),
            request_pub: request_public(label),
            eph_secret: seed(&format!("eph {label}")),
            eph_pub_override: None,
            code: first(seed(&format!("code {label}")), 16).try_into().unwrap(),
            cek: seed(&format!("cek {label}")),
            nonce: first(seed(&format!("nonce {label}")), 24).try_into().unwrap(),
            min_client: json!({"android": "1.2", "ios": "1.2", "macos": "1.2"}),
            archive_override: None,
            files_override: None,
            edit_signed: None,
            edit_early: None,
        }
    }

    /// The raw (pre-rewrite) archive entries, manifest included, sorted.
    pub fn raw_entries(&self) -> Vec<(String, Vec<u8>)> {
        let mut entries = self.files.clone();
        entries.push(("manifest.json".into(), serde_json::to_vec(&self.manifest).unwrap()));
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        entries
    }

    /// The signed `files` list: what the client's rewrite produces (as the CLI computes it).
    pub fn files_value(&self) -> Value {
        if let Some(v) = &self.files_override {
            return v.clone();
        }
        let raw = self.raw_entries();
        let entries: Vec<Entry<'_>> = raw.iter().map(|(n, d)| Entry { name: n, data: d }).collect();
        let rewritten = manifest::validate(&entries, &self.id, self.rev).expect("the sample validates");
        Value::Array(
            rewritten
                .files
                .iter()
                .map(|(n, d)| json!({"len": d.len(), "n": n, "sha256": hex_encode(&sha256(d))}))
                .collect(),
        )
    }

    pub fn build(&self) -> Vec<u8> {
        let archive = self.archive_override.clone().unwrap_or_else(|| mpar(&self.raw_entries()));
        let mut header = json!({
            "aead": "xchacha20poly1305",
            "created": 1_763_078_400u32,
            "files": self.files_value(),
            "format": 2,
            "id": self.id,
            "kid": self.kid,
            "minClient": self.min_client,
            "nonce": b64_encode(&self.nonce),
            "payloadLen": archive.len() + 16,
            "rev": self.rev,
        });
        if let Some(edit) = &self.edit_early {
            edit(&mut header);
        }
        let content_hash = sha256(&canonical_json(&header));
        let eph_pub = self.eph_pub_override.unwrap_or_else(|| public_key(&self.eph_secret));
        let shared = x25519_dalek::x25519(self.eph_secret, self.request_pub);
        let id = header["id"].as_str().unwrap_or("").to_string();
        let rev = header["rev"].as_u64().unwrap_or(0) as u32;
        let kek = derive_kek(&shared, &self.code, &kek_info(&id, rev, &self.request_pub, &eph_pub, &content_hash));
        let wrap = XChaCha20Poly1305::new_from_slice(kek.as_ref()).unwrap();
        let wrapped = wrap.encrypt(&XNonce::default(), Payload { msg: &self.cek, aad: CEK_AAD }).unwrap();
        header["recipient"] = json!({
            "code": b64_encode(&self.code),
            "eph": b64_encode(&eph_pub),
            "to": b64_encode(&recipient_tag(&self.request_pub)),
            "wrapped": b64_encode(&wrapped),
        });
        let aad = canonical_json(&header);
        let cipher = XChaCha20Poly1305::new_from_slice(&self.cek).unwrap();
        let ct = cipher.encrypt(&XNonce::try_from(&self.nonce[..]).unwrap(), Payload { msg: &archive, aad: &aad }).unwrap();
        header["payloadSha256"] = json!(hex_encode(&sha256(&ct)));
        if let Some(edit) = &self.edit_signed {
            edit(&mut header);
        }
        let header_bytes = canonical_json(&header);
        assemble(&header_bytes, &self.cert, &self.signer, &ct)
    }
}

/// Container bytes from parts, signing the header with `signer`.
pub fn assemble(header: &[u8], cert: &(Vec<u8>, [u8; 64]), signer: &SigningKey, payload: &[u8]) -> Vec<u8> {
    let sig = signer.sign(&[DOMAIN, header].concat()).to_bytes();
    frame(&MAGIC, header, cert, &sig, payload)
}

/// Container bytes from parts with a given magic and an already made signature.
pub fn frame(magic: &[u8; 8], header: &[u8], cert: &(Vec<u8>, [u8; 64]), sig: &[u8; 64], payload: &[u8]) -> Vec<u8> {
    let mut out = magic.to_vec();
    out.extend_from_slice(&(header.len() as u32).to_le_bytes());
    out.extend_from_slice(header);
    out.extend_from_slice(&(cert.0.len() as u16).to_le_bytes());
    out.extend_from_slice(&cert.0);
    out.extend_from_slice(&cert.1);
    out.extend_from_slice(sig);
    out.extend_from_slice(payload);
    out
}

/// The signed `files` list of raw entries: what the client's rewrite produces.
pub fn signed_files(raw: &[(String, Vec<u8>)], id: &str, rev: u32) -> Value {
    let entries: Vec<Entry<'_>> = raw.iter().map(|(n, d)| Entry { name: n, data: d }).collect();
    let rewritten = manifest::validate(&entries, id, rev).expect("the sample validates");
    Value::Array(
        rewritten.files.iter().map(|(n, d)| json!({"len": d.len(), "n": n, "sha256": hex_encode(&sha256(d))})).collect(),
    )
}

/// Everything a bundled package decides; every field can be changed to build a variant.
pub struct Bundle {
    pub id: String,
    pub rev: u32,
    pub manifest: Value,
    pub files: Vec<(String, Vec<u8>)>,
    pub signer: SigningKey,
    pub kid: String,
    pub cert: (Vec<u8>, [u8; 64]),
    pub min_client: Value,
    pub magic: [u8; 8],
    pub domain: Vec<u8>,
    /// Replaces the archive.
    pub archive_override: Option<Vec<u8>>,
    /// Replaces the signed `files` list.
    pub files_override: Option<Value>,
    /// Edits the header just before it is signed.
    pub edit_signed: Option<HeaderEdit>,
}

/// The serial of the sample bundled skin.
pub const BUNDLED_SERIAL: &str = "b0b0b0b0c0ffee01";

impl Bundle {
    /// The sample skin as a bundled package.
    pub fn sample() -> Bundle {
        let id = "sakura_tea".to_string();
        let rev = 3;
        let signer = signing(SKIN_KID);
        Bundle {
            manifest: sample_manifest(&id, rev, BUNDLED_SERIAL),
            id,
            rev,
            files: sample_files(),
            cert: cert(&root(), ROOT_KID, SKIN_KID, &signer, &["code", "skin"]),
            signer,
            kid: SKIN_KID.into(),
            min_client: json!({"android": "1.2", "ios": "1.2"}),
            magic: BUNDLED_MAGIC,
            domain: BUNDLED_DOMAIN.to_vec(),
            archive_override: None,
            files_override: None,
            edit_signed: None,
        }
    }

    /// The same skin at another revision.
    pub fn at_rev(rev: u32) -> Bundle {
        let mut b = Bundle::sample();
        b.rev = rev;
        b.manifest["rev"] = json!(rev);
        b
    }

    pub fn raw_entries(&self) -> Vec<(String, Vec<u8>)> {
        let mut entries = self.files.clone();
        entries.push(("manifest.json".into(), serde_json::to_vec(&self.manifest).unwrap()));
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        entries
    }

    pub fn archive(&self) -> Vec<u8> {
        self.archive_override.clone().unwrap_or_else(|| mpar(&self.raw_entries()))
    }

    /// The canonical header bytes that get signed.
    pub fn header(&self) -> Vec<u8> {
        let archive = self.archive();
        let files = self.files_override.clone().unwrap_or_else(|| signed_files(&self.raw_entries(), &self.id, self.rev));
        let mut header = json!({
            "created": 1_763_078_400u32,
            "files": files,
            "format": 1,
            "id": self.id,
            "kid": self.kid,
            "kind": "bundled",
            "minClient": self.min_client,
            "payloadLen": archive.len(),
            "payloadSha256": hex_encode(&sha256(&archive)),
            "rev": self.rev,
        });
        if let Some(edit) = &self.edit_signed {
            edit(&mut header);
        }
        canonical_json(&header)
    }

    pub fn build(&self) -> Vec<u8> {
        let header = self.header();
        let sig = self.signer.sign(&[self.domain.as_slice(), &header].concat()).to_bytes();
        frame(&self.magic, &header, &self.cert, &sig, &self.archive())
    }
}

/// The length of the sealed prefix of a package (magic through signature).
pub fn seal_len(package: &[u8]) -> usize {
    let n = u32::from_le_bytes(package[8..12].try_into().unwrap()) as usize;
    let c = u16::from_le_bytes(package[12 + n..14 + n].try_into().unwrap()) as usize;
    14 + n + c + 128
}

/// Offset of the header signature in a package or seal.
pub fn signature_at(package: &[u8]) -> usize {
    seal_len(package) - 64
}

// ---------- scalars mod ℓ (only for crafting signatures a strict verifier must refuse) ----------

/// ℓ = 2^252 + 27742317777372353535851937790883648493, little-endian 64-bit limbs.
const ELL: [u64; 4] = [0x5812_631a_5cf5_d3ed, 0x14de_f9de_a2f7_9cd6, 0, 0x1000_0000_0000_0000];

fn ge(a: &[u64; 4], b: &[u64; 4]) -> bool {
    for i in (0..4).rev() {
        if a[i] != b[i] {
            return a[i] > b[i];
        }
    }
    true
}

fn sub(a: &[u64; 4], b: &[u64; 4]) -> [u64; 4] {
    let mut out = [0u64; 4];
    let mut borrow = 0u64;
    for i in 0..4 {
        let (d, b1) = a[i].overflowing_sub(b[i]);
        let (d, b2) = d.overflowing_sub(borrow);
        out[i] = d;
        borrow = u64::from(b1 || b2);
    }
    out
}

fn add_mod(a: &[u64; 4], b: &[u64; 4]) -> [u64; 4] {
    // a, b < ℓ < 2^253: the sum fits in 256 bits.
    let mut out = [0u64; 4];
    let mut carry = 0u64;
    for i in 0..4 {
        let (s, c1) = a[i].overflowing_add(b[i]);
        let (s, c2) = s.overflowing_add(carry);
        out[i] = s;
        carry = u64::from(c1 || c2);
    }
    if ge(&out, &ELL) {
        sub(&out, &ELL)
    } else {
        out
    }
}

/// Little-endian bytes reduced mod ℓ.
fn mod_ell(bytes: &[u8]) -> [u64; 4] {
    let mut r = [0u64; 4];
    for byte in bytes.iter().rev() {
        for bit in (0..8).rev() {
            r = add_mod(&r, &r);
            if byte >> bit & 1 == 1 {
                r = add_mod(&r, &[1, 0, 0, 0]);
            }
        }
    }
    r
}

fn mul_mod(a: &[u64; 4], b: &[u64; 4]) -> [u64; 4] {
    let mut acc = [0u64; 4];
    for i in (0..256).rev() {
        acc = add_mod(&acc, &acc);
        if b[i / 64] >> (i % 64) & 1 == 1 {
            acc = add_mod(&acc, a);
        }
    }
    acc
}

fn limbs_to_bytes(l: &[u64; 4]) -> [u8; 32] {
    let mut out = [0u8; 32];
    for i in 0..4 {
        out[i * 8..i * 8 + 8].copy_from_slice(&l[i].to_le_bytes());
    }
    out
}

/// A signature with R = the identity point (encoding `01 00…00`, small order)
/// and S = k·a, where k = SHA-512(R ‖ A ‖ message) mod ℓ. The cofactorless
/// equation [S]B = R + [k]A holds, so a verifier that does not refuse
/// small-order R accepts it; `verify_strict` refuses it.
pub fn small_order_r_signature(key: &SigningKey, message: &[u8]) -> [u8; 64] {
    use sha2::{Digest, Sha512};
    let mut r = [0u8; 32];
    r[0] = 1;
    let a_pub = key.verifying_key().to_bytes();
    let k = mod_ell(&Sha512::new().chain_update(r).chain_update(a_pub).chain_update(message).finalize());
    let mut a = key.to_scalar_bytes();
    a[0] &= 248;
    a[31] &= 127;
    a[31] |= 64;
    let s = mul_mod(&k, &mod_ell(&a));
    let mut sig = [0u8; 64];
    sig[..32].copy_from_slice(&r);
    sig[32..].copy_from_slice(&limbs_to_bytes(&s));
    sig
}

/// `S + ℓ` in place of `S` (a non-canonical scalar, same group element).
pub fn s_plus_ell(sig: &[u8; 64]) -> [u8; 64] {
    let mut out = *sig;
    let ell = limbs_to_bytes(&ELL);
    let mut carry = 0u16;
    for i in 0..32 {
        let sum = u16::from(out[32 + i]) + u16::from(ell[i]) + carry;
        out[32 + i] = sum as u8;
        carry = sum >> 8;
    }
    out
}

/// The 14 encodings that ed25519-dalek decompresses to a point of small
/// order (the 8 torsion points; y = 0 and y = 1 also as y + p; x = 0 also
/// with the sign bit set).
pub fn small_order_encodings() -> Vec<[u8; 32]> {
    let hex = [
        "0000000000000000000000000000000000000000000000000000000000000000",
        "0000000000000000000000000000000000000000000000000000000000000080",
        "0100000000000000000000000000000000000000000000000000000000000000",
        "0100000000000000000000000000000000000000000000000000000000000080",
        "ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
        "ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        "edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
        "edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        "eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
        "eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        "26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc05",
        "26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc85",
        "c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac037a",
        "c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac03fa",
    ];
    hex.iter().map(|h| meowpuut_skin::codec::hex_array::<32>(h).unwrap()).collect()
}

/// A fresh empty directory under the target directory.
pub fn temp_dir(label: &str) -> PathBuf {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skin-tests");
    let dir = base.join(format!("{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A small deterministic PRNG (xorshift64*).
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    pub fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
}
