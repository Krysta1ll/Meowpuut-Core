//! Shared golden vectors in `Tests/skin-vectors/` (test keys only, kids `test-…`).
//!
//! `golden_vectors` checks the committed files; it fails when the format or
//! the PNG/WAV rewrite changes (the signed `files` hashes would no longer
//! match). To write them again after a deliberate change:
//!
//! ```text
//! MEOWPUUT_WRITE_SKIN_VECTORS=1 cargo test --release -p meowpuut-skin --test vectors -- --ignored regenerate
//! ```

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use meowpuut_skin::codec::{b64_encode, canonical_json, hex_encode};
use meowpuut_skin::container::{open_seal, parse_seal, split};
use meowpuut_skin::install::{open_package, Client};
use meowpuut_skin::request::redemption_id;
use meowpuut_skin::revoke::{parse_file, State};
use meowpuut_skin::trust::{Policy, TEST_ROOT};
use meowpuut_skin::Code;
use serde_json::{json, Value};
use support::*;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../Tests/skin-vectors")
}

/// Every vector file and the case list.
fn build() -> (BTreeMap<String, Vec<u8>>, Value) {
    let mut files = BTreeMap::new();
    let good = Issue::sample("a").build();
    files.insert("sakura_tea-3.mpskin".to_string(), good.clone());
    let (seal, _) = split(&good).unwrap();
    files.insert("sakura_tea-3.seal.bin".to_string(), good[..seal.len].to_vec());

    // One header byte changed after signing: "rev":3 → "rev":4.
    let n = u32::from_le_bytes(good[8..12].try_into().unwrap()) as usize;
    let header = std::str::from_utf8(&good[12..12 + n]).unwrap();
    let at = 12 + header.find("\"rev\":3").unwrap() + 6;
    let mut tampered = good.clone();
    tampered[at] = b'4';
    files.insert("tampered-header.mpskin".to_string(), tampered);

    // S + ℓ: accepted by a lax verifier, refused by verify_strict and CryptoKit alike.
    let c = u16::from_le_bytes(good[12 + n..14 + n].try_into().unwrap()) as usize;
    let s_at = 14 + n + c + 64 + 32;
    let ell: [u8; 32] = [
        0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7, 0xa2, 0xde, 0xf9, 0xde, 0x14, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0x10,
    ];
    let mut s_plus = good.clone();
    let mut carry = 0u16;
    for i in 0..32 {
        let sum = u16::from(s_plus[s_at + i]) + u16::from(ell[i]) + carry;
        s_plus[s_at + i] = sum as u8;
        carry = sum >> 8;
    }
    files.insert("noncanonical-s.mpskin".to_string(), s_plus);

    let mut small = Issue::sample("a");
    small.eph_pub_override = Some([0u8; 32]);
    files.insert("small-order-eph.mpskin".to_string(), small.build());

    let mut code_only = Issue::sample("a");
    code_only.cert = cert(&root(), ROOT_KID, SKIN_KID, &signing(SKIN_KID), &["code"]);
    files.insert("code-only-subkey.mpskin".to_string(), code_only.build());

    files.insert("revoke-kid.mprevoke".to_string(), revocation(&root(), ROOT_KID, 1, &[("kid", SKIN_KID)]));
    files.insert("revoke-by-subkey.mprevoke".to_string(), revocation(&signing(SKIN_KID), ROOT_KID, 2, &[("kid", "test-x")]));

    let opened = open_package(&good, &[request_secret("a")], &State::default(), &Policy::with_test_keys(), &android()).unwrap();
    let installed: Vec<Value> = opened
        .skin
        .files
        .iter()
        .map(|(n, d)| json!({"n": n, "len": d.len(), "sha256": hex_encode(&meowpuut_skin::codec::sha256(d))}))
        .collect();
    let highest4 = json!({"highest": {"sakura_tea": 4}, "revoked": {"kid": [], "serial": [], "skin": []}, "seq": 0, "v": 1});
    let cases = json!([
        {"name": "valid", "file": "sakura_tea-3.mpskin", "request": "a", "policy": "test", "expect": 0, "installed": installed},
        {"name": "another device", "file": "sakura_tea-3.mpskin", "request": "b", "policy": "test", "expect": Code::NotForThisDevice.number()},
        {"name": "release build", "file": "sakura_tea-3.mpskin", "request": "a", "policy": "release", "expect": Code::TestKey.number()},
        {"name": "header changed after signing", "file": "tampered-header.mpskin", "request": "a", "policy": "test", "expect": Code::Signature.number()},
        {"name": "non-canonical S", "file": "noncanonical-s.mpskin", "request": "a", "policy": "test", "expect": Code::Signature.number()},
        {"name": "small-order ephemeral key", "file": "small-order-eph.mpskin", "request": "a", "policy": "test", "expect": Code::KeyAgreement.number()},
        {"name": "subkey without the skin use", "file": "code-only-subkey.mpskin", "request": "a", "policy": "test", "expect": Code::KeyUse.number()},
        {"name": "revoked subkey", "file": "sakura_tea-3.mpskin", "request": "a", "policy": "test", "revocations": ["revoke-kid.mprevoke"], "expect": Code::Revoked.number()},
        {"name": "downgrade", "file": "sakura_tea-3.mpskin", "request": "a", "policy": "test", "state": highest4, "expect": Code::Downgrade.number()},
        {"name": "revocation signed by a subkey", "revocationFile": "revoke-by-subkey.mprevoke", "policy": "test", "expect": Code::Revocation.number()},
        {"name": "revocation in a release build", "revocationFile": "revoke-kid.mprevoke", "policy": "release", "expect": Code::TestKey.number()}
    ]);
    let mut keys = Map::new();
    keys.insert(
        "root".into(),
        json!({"kid": ROOT_KID, "seed": "SHA-256(\"meowpuut-skin test key test-root-1\")", "public": b64_encode(&root().verifying_key().to_bytes())}),
    );
    let (cert_json, cert_sig) = cert(&root(), ROOT_KID, SKIN_KID, &signing(SKIN_KID), &["code", "skin"]);
    keys.insert(
        "subkey".into(),
        json!({"kid": SKIN_KID, "seed": "SHA-256(\"meowpuut-skin test key test-2026-skin\")",
               "public": b64_encode(&signing(SKIN_KID).verifying_key().to_bytes()),
               "cert": String::from_utf8(cert_json).unwrap(), "certSignature": b64_encode(&cert_sig)}),
    );
    for label in ["a", "b"] {
        keys.insert(
            format!("request-{label}"),
            json!({"seed": format!("SHA-256(\"meowpuut-skin test key request {label}\")"),
                   "secret": b64_encode(&request_secret(label)), "public": b64_encode(&request_public(label)),
                   "redemptionId": redemption_id(&request_public(label))}),
        );
    }
    let doc = json!({"client": "android/1.2", "keys": keys, "cases": cases});
    (files, doc)
}

type Map = serde_json::Map<String, Value>;

fn android() -> Client {
    Client::parse("android/1.2").unwrap()
}

#[test]
#[ignore]
fn regenerate() {
    if std::env::var("MEOWPUUT_WRITE_SKIN_VECTORS").as_deref() != Ok("1") {
        eprintln!("set MEOWPUUT_WRITE_SKIN_VECTORS=1 to write Tests/skin-vectors");
        return;
    }
    let (files, doc) = build();
    fs::create_dir_all(dir()).unwrap();
    for (name, data) in files {
        fs::write(dir().join(name), data).unwrap();
    }
    let mut text = serde_json::to_string_pretty(&doc).unwrap();
    text.push('\n');
    fs::write(dir().join("cases.json"), text).unwrap();
}

#[test]
fn golden_vectors() {
    let doc: Value = serde_json::from_slice(&fs::read(dir().join("cases.json")).unwrap()).unwrap();
    let client = Client::parse(doc["client"].as_str().unwrap()).unwrap();
    // Keys: the published seeds give the published keys; the test root is the built-in one.
    let keys = &doc["keys"];
    assert_eq!(keys["root"]["public"], b64_encode(&TEST_ROOT.public));
    assert_eq!(keys["root"]["kid"], TEST_ROOT.kid);
    assert!(keys["subkey"]["kid"].as_str().unwrap().starts_with("test-"));
    for label in ["a", "b"] {
        let k = &keys[format!("request-{label}")];
        assert_eq!(k["secret"], b64_encode(&request_secret(label)));
        assert_eq!(k["public"], b64_encode(&request_public(label)));
        assert_eq!(k["redemptionId"], redemption_id(&request_public(label)));
        assert_eq!(meowpuut_skin::request::parse_redemption_id(k["redemptionId"].as_str().unwrap()).unwrap(), request_public(label));
    }
    let read = |name: &str| fs::read(dir().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
    let mut checked = 0;
    for case in doc["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let policy = match case["policy"].as_str().unwrap() {
            "test" => Policy::with_test_keys(),
            _ => Policy::release(),
        };
        let expect = case["expect"].as_i64().unwrap() as i32;
        if let Some(file) = case["revocationFile"].as_str() {
            let got = parse_file(&read(file), &policy).map(|_| 0).unwrap_or_else(|e| e.code.number());
            assert_eq!(got, expect, "{name}");
            checked += 1;
            continue;
        }
        let mut state = match case.get("state") {
            Some(s) => State::parse(&s.to_string()).unwrap(),
            None => State::default(),
        };
        for file in case["revocations"].as_array().into_iter().flatten() {
            state.merge(&parse_file(&read(file.as_str().unwrap()), &policy).unwrap()).unwrap();
        }
        let secret = request_secret(case["request"].as_str().unwrap());
        let package = read(case["file"].as_str().unwrap());
        match open_package(&package, &[secret], &state, &policy, &client) {
            Ok(opened) => {
                assert_eq!(expect, 0, "{name}: opened");
                let listed: Vec<Value> = opened
                    .skin
                    .files
                    .iter()
                    .map(|(n, d)| json!({"n": n, "len": d.len(), "sha256": hex_encode(&meowpuut_skin::codec::sha256(d))}))
                    .collect();
                assert_eq!(Value::Array(listed), case["installed"], "{name}: rewritten files");
            }
            Err(e) => assert_eq!(e.code.number(), expect, "{name}: {e}"),
        }
        checked += 1;
    }
    assert_eq!(checked, 11);
    // The seal alone verifies (the load-time path; CryptoKit must agree).
    let seal_bytes = read("sakura_tea-3.seal.bin");
    let seal = parse_seal(&seal_bytes).unwrap();
    let (header, cert) = open_seal(&seal, &Policy::with_test_keys(), &Default::default()).unwrap();
    assert_eq!(header.skin_ref(), "sakura_tea@3");
    assert_eq!(cert.kid, keys["subkey"]["kid"].as_str().unwrap());
    assert_eq!(canonical_json(&serde_json::from_slice::<Value>(seal.header_bytes).unwrap()), seal.header_bytes);
}

/// The committed vectors are what the current code builds (same platform
/// maths); a mismatch means the vectors are stale.
#[test]
fn vectors_are_current() {
    let (files, doc) = build();
    for (name, data) in files {
        assert_eq!(fs::read(dir().join(&name)).unwrap(), data, "{name} is stale: regenerate the vectors");
    }
    let committed: Value = serde_json::from_slice(&fs::read(dir().join("cases.json")).unwrap()).unwrap();
    assert_eq!(committed, doc);
}
