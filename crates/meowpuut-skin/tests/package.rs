//! Package checks: the happy path, every header field, truncation, random
//! input, size bombs, test keys in a release policy.

mod support;

use meowpuut_skin::codec::{b64_encode, canonical_json};
use meowpuut_skin::install::{open_package, redeem, Client};
use meowpuut_skin::revoke::State;
use meowpuut_skin::trust::Policy;
use meowpuut_skin::{limits, Code};
use serde_json::{json, Value};
use support::*;

fn android() -> Client {
    Client::parse("android/1.2").unwrap()
}

fn open(package: &[u8], secrets: &[[u8; 32]]) -> Result<meowpuut_skin::install::Opened, meowpuut_skin::Error> {
    open_package(package, secrets, &State::default(), &Policy::with_test_keys(), &android())
}

fn code_of(package: &[u8]) -> Code {
    open(package, &[request_secret("a")]).map(|_| ()).unwrap_err().code
}

#[test]
fn sample_opens_and_rewrites() {
    let package = Issue::sample("a").build();
    let opened = open(&package, &[request_secret("b"), request_secret("a")]).unwrap();
    assert_eq!(opened.request_index, Some(1));
    assert_eq!(opened.header.skin_ref(), "sakura_tea@3");
    let names: Vec<&str> = opened.skin.files.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        ["frame.png", "key-1.wav", "manifest.json", "mimimo_peek.png", "momo_hold.png", "return-1.wav", "right.png", "space-1.wav"]
    );
    for (name, data) in &opened.skin.files {
        if name.ends_with(".png") {
            let types = meowpuut_skin::image::chunk_types(data);
            assert!(types.iter().all(|t| matches!(t, b"IHDR" | b"IDAT" | b"IEND")), "{name}: {types:?}");
            assert!(data.ends_with(&[0xAE, 0x42, 0x60, 0x82]), "{name}: nothing after IEND");
        }
        if name.ends_with(".wav") {
            assert_eq!(&data[36..40], b"data", "{name}: 44-byte header");
            assert_eq!(u32::from_le_bytes(data[40..44].try_into().unwrap()) as usize, data.len() - 44);
        }
    }
    let manifest: Value = serde_json::from_slice(&opened.skin.files[2].1).unwrap();
    assert_eq!(canonical_json(&manifest), opened.skin.files[2].1, "manifest is canonical");
    let peek = &manifest["cats"]["sprites"]["mimimo_edge_peek"];
    assert_eq!(peek["cut"], json!([0.2083, 0.7917]));
    assert_eq!(peek["paws"], json!([[0.3333, 0.0417, 0.0833], [0.6667, 0.0417, 0.0833]]));
    assert_eq!(peek["bounds"], json!([0.2083, 0.1667, 0.7917, 0.8333]));
    assert_eq!(manifest["cats"]["trim"]["rim"], json!([3.0, 3.0, 3.0, 3.0]));
    assert_eq!(manifest["palette"]["light"]["paper"], "#FFF8F0");
}

#[test]
fn package_for_another_request_is_refused() {
    let package = Issue::sample("a").build();
    assert_eq!(open(&package, &[request_secret("b")]).unwrap_err().code, Code::NotForThisDevice);
    assert_eq!(open(&package, &[]).unwrap_err().code, Code::NotForThisDevice);
}

#[test]
fn release_policy_refuses_test_kids() {
    let package = Issue::sample("a").build();
    for policy in [Policy::release(), Policy::custom(&[], false).unwrap()] {
        let e = open_package(&package, &[request_secret("a")], &State::default(), &policy, &android()).unwrap_err();
        assert_eq!(e.code, Code::TestKey, "{e}");
    }
}

/// The build flag of this test binary: without `test-keys` (the default and every release build),
/// the build policy refuses the test vectors' kids.
#[cfg(not(feature = "test-keys"))]
#[test]
fn non_test_build_refuses_test_kids() {
    let package = Issue::sample("a").build();
    let e = open_package(&package, &[request_secret("a")], &State::default(), &Policy::from_build(), &android()).unwrap_err();
    assert_eq!(e.code, Code::TestKey);
}

#[test]
fn every_flipped_bit_is_refused() {
    let package = Issue::sample("a").build();
    let secrets = [request_secret("a")];
    for i in 0..package.len() {
        for bit in [0x01u8, 0x80] {
            let mut p = package.clone();
            p[i] ^= bit;
            assert!(open(&p, &secrets).is_err(), "flipping byte {i} bit {bit:#x} went through");
        }
    }
}

#[test]
fn every_truncation_and_extension_is_refused() {
    let package = Issue::sample("a").build();
    for n in 0..package.len() {
        assert!(open(&package[..n], &[request_secret("a")]).is_err(), "prefix {n}");
    }
    let mut longer = package.clone();
    longer.push(0);
    assert_eq!(open(&longer, &[request_secret("a")]).unwrap_err().code, Code::Payload);
}

#[test]
fn random_input_and_random_mutations_are_refused() {
    let mut rng = Rng(0x5eed_1234_abcd_0001);
    for _ in 0..2000 {
        let n = rng.below(4096);
        let bytes = rng.bytes(n);
        assert!(open(&bytes, &[request_secret("a")]).is_err());
        let mut with_magic = meowpuut_skin::container::MAGIC.to_vec();
        with_magic.extend_from_slice(&bytes);
        assert!(open(&with_magic, &[request_secret("a")]).is_err());
    }
    let package = Issue::sample("a").build();
    for _ in 0..1500 {
        let mut p = package.clone();
        for _ in 0..1 + rng.below(8) {
            let i = rng.below(p.len());
            p[i] = rng.next() as u8;
        }
        if p != package {
            assert!(open(&p, &[request_secret("a")]).is_err());
        }
    }
}

#[test]
fn size_limits_are_checked_before_reading() {
    // A claimed 4 GiB header and payload: refused from the length fields alone.
    let mut p = meowpuut_skin::container::MAGIC.to_vec();
    p.extend_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(code_of(&p), Code::TooLarge);
    let mut p = meowpuut_skin::container::MAGIC.to_vec();
    p.extend_from_slice(&((limits::HEADER + 1) as u32).to_le_bytes());
    p.extend_from_slice(&vec![b' '; limits::HEADER + 1]);
    assert_eq!(code_of(&p), Code::TooLarge);
    assert_eq!(code_of(&vec![0u8; limits::PACKAGE + 1]), Code::TooLarge);
    // A signed header claiming a payload over the limit.
    let mut issue = Issue::sample("a");
    issue.edit_signed = Some(Box::new(|h: &mut Value| h["payloadLen"] = json!(limits::PAYLOAD + 1)));
    assert_eq!(code_of(&issue.build()), Code::TooLarge);
    issue.edit_signed = Some(Box::new(|h: &mut Value| h["payloadLen"] = json!(u32::MAX)));
    assert_eq!(code_of(&issue.build()), Code::TooLarge);
}

fn signed_edit(edit: impl Fn(&mut Value) + 'static) -> Code {
    let mut issue = Issue::sample("a");
    issue.edit_signed = Some(Box::new(edit));
    code_of(&issue.build())
}

fn files_edit(edit: impl Fn(&mut Value)) -> Code {
    let mut issue = Issue::sample("a");
    let mut files = issue.files_value();
    edit(&mut files);
    issue.files_override = Some(files);
    code_of(&issue.build())
}

fn early_edit(edit: impl Fn(&mut Value) + 'static) -> Code {
    let mut issue = Issue::sample("a");
    issue.edit_early = Some(Box::new(edit));
    code_of(&issue.build())
}

/// Headers signed with the real (test) subkey but wrong in one field: what a
/// leaked key or a broken tool could produce. Every one is refused.
#[test]
fn every_header_field_is_checked() {
    assert_eq!(signed_edit(|h| h["format"] = json!(3)), Code::Header);
    assert_eq!(signed_edit(|h| h["aead"] = json!("aes256gcm")), Code::Header);
    assert_eq!(signed_edit(|h| h["id"] = json!("Sakura_tea")), Code::Header);
    assert_eq!(signed_edit(|h| h["id"] = json!("con")), Code::Header);
    assert_eq!(signed_edit(|h| h["id"] = json!("../x")), Code::Header);
    assert_eq!(signed_edit(|h| h["rev"] = json!(0)), Code::Header);
    assert_eq!(signed_edit(|h| h["rev"] = json!(4294967296u64)), Code::Header);
    assert_eq!(signed_edit(|h| h["rev"] = json!(-1)), Code::Header);
    assert_eq!(signed_edit(|h| h["created"] = json!(1.5)), Code::Header);
    assert_eq!(signed_edit(|h| h["kid"] = json!(CODE_KID)), Code::Header);
    assert_eq!(signed_edit(|h| h["minClient"] = json!({"android": "1"})), Code::Header);
    assert_eq!(signed_edit(|h| h["minClient"] = json!({"symbian": "1.0"})), Code::Header);
    assert_eq!(signed_edit(|h| h["nonce"] = json!(b64_encode(&[0u8; 12]))), Code::Header);
    assert_eq!(signed_edit(|h| h["nonce"] = json!(b64_encode(&[0u8; 24]))), Code::Decrypt);
    assert_eq!(signed_edit(|h| h["payloadSha256"] = json!("00".repeat(32))), Code::Payload);
    assert_eq!(signed_edit(|h| h["payloadSha256"] = json!("AB".repeat(32))), Code::Header);
    assert_eq!(signed_edit(|h| h["payloadLen"] = json!(h["payloadLen"].as_u64().unwrap() - 1)), Code::Payload);
    assert_eq!(signed_edit(|h| h["extra"] = json!(1)), Code::Header);
    assert_eq!(signed_edit(|h| { h.as_object_mut().unwrap().remove("created"); }), Code::Header);
    // recipient
    assert_eq!(signed_edit(|h| h["recipient"]["to"] = json!(b64_encode(&[7u8; 16]))), Code::NotForThisDevice);
    assert_eq!(signed_edit(|h| h["recipient"]["code"] = json!(b64_encode(&[7u8; 16]))), Code::Decrypt);
    assert_eq!(signed_edit(|h| h["recipient"]["wrapped"] = json!(b64_encode(&[7u8; 48]))), Code::Decrypt);
    assert_eq!(signed_edit(|h| h["recipient"]["eph"] = json!(b64_encode(&request_public("z")))), Code::Decrypt);
    assert_eq!(signed_edit(|h| h["recipient"]["eph"] = json!(b64_encode(&[0u8; 32]))), Code::KeyAgreement);
    assert_eq!(signed_edit(|h| h["recipient"]["code"] = json!(b64_encode(&[7u8; 15]))), Code::Header);
    assert_eq!(signed_edit(|h| h["recipient"]["extra"] = json!("")), Code::Header);
    // files
    // Signed after the fact, a files edit changes the content hash: the code no longer opens it.
    assert_eq!(signed_edit(|h| h["files"][0]["len"] = json!(1)), Code::Decrypt);
    // Issued with a wrong list (the code derived from it): the rewrite comparison refuses it.
    assert_eq!(files_edit(|f| f[0]["len"] = json!(1)), Code::Files);
    assert_eq!(files_edit(|f| f[1]["sha256"] = json!("00".repeat(32))), Code::Files);
    assert_eq!(files_edit(|f| { f.as_array_mut().unwrap().pop(); }), Code::Files);
    assert_eq!(files_edit(|f| f[0]["n"] = json!("frame0.png")), Code::Files);
    assert_eq!(signed_edit(|h| h["files"][0]["n"] = json!("../frame.png")), Code::Header);
    assert_eq!(signed_edit(|h| h["files"][0]["n"] = json!("con.png")), Code::Header);
    assert_eq!(signed_edit(|h| h["files"][0]["n"] = json!("other.json")), Code::Header);
    assert_eq!(signed_edit(|h| { let f = h["files"][0].clone(); h["files"].as_array_mut().unwrap().insert(0, f); }), Code::Header);
    assert_eq!(signed_edit(|h| h["files"][0]["len"] = json!(limits::FILE + 1)), Code::TooLarge);
}

/// The code is bound to the content: changing a signed field before encryption
/// (so the recipient is derived from it) still fails when the rest disagrees.
#[test]
fn id_and_rev_must_match_the_manifest() {
    assert_eq!(early_edit(|h| h["rev"] = json!(4)), Code::Manifest);
    assert_eq!(early_edit(|h| h["id"] = json!("other")), Code::Manifest);
}

#[test]
fn header_must_be_canonical() {
    let package = Issue::sample("a").build();
    let n = u32::from_le_bytes(package[8..12].try_into().unwrap()) as usize;
    let header: Value = serde_json::from_slice(&package[12..12 + n]).unwrap();
    let pretty = serde_json::to_vec_pretty(&header).unwrap();
    let issue = Issue::sample("a");
    let ct = package[package.len() - header["payloadLen"].as_u64().unwrap() as usize..].to_vec();
    let rebuilt = assemble(&pretty, &issue.cert, &issue.signer, &ct);
    assert_eq!(code_of(&rebuilt), Code::Header);
}

#[test]
fn certificate_rules() {
    let root = root();
    let sub = signing(SKIN_KID);
    let mut issue = Issue::sample("a");
    // A code-only subkey cannot sign skins.
    issue.cert = cert(&root, ROOT_KID, SKIN_KID, &sub, &["code"]);
    assert_eq!(code_of(&issue.build()), Code::KeyUse);
    // Unsorted, unknown or duplicate uses.
    for uses in [&["skin", "code"][..], &["skin", "skin"], &["sign"], &[]] {
        issue.cert = cert(&root, ROOT_KID, SKIN_KID, &sub, uses);
        assert_eq!(code_of(&issue.build()), Code::Certificate, "{uses:?}");
    }
    // Certified by an unknown root, or self-signed.
    issue.cert = cert(&signing("test-root-2"), "test-root-2", SKIN_KID, &sub, &["skin"]);
    assert_eq!(code_of(&issue.build()), Code::Certificate);
    issue.cert = cert(&sub, ROOT_KID, SKIN_KID, &sub, &["skin"]);
    assert_eq!(code_of(&issue.build()), Code::Certificate);
    // A test root certifying a release kid.
    let release_kid = signing("2026a-skin");
    issue.cert = cert(&root, ROOT_KID, "2026a-skin", &release_kid, &["skin"]);
    issue.kid = "2026a-skin".into();
    issue.signer = release_kid;
    assert_eq!(code_of(&issue.build()), Code::Certificate);
    // A non-canonical certificate.
    let mut issue = Issue::sample("a");
    let (json, _) = cert(&root, ROOT_KID, SKIN_KID, &sub, &["skin"]);
    let v: Value = serde_json::from_slice(&json).unwrap();
    let pretty = serde_json::to_vec_pretty(&v).unwrap();
    let sig = ed25519_dalek::Signer::sign(&root, &[meowpuut_skin::trust::CERT_DOMAIN, &pretty].concat()).to_bytes();
    issue.cert = (pretty, sig);
    assert_eq!(code_of(&issue.build()), Code::Certificate);
}

/// verify_strict: a signature with S + ℓ (non-canonical) is refused.
#[test]
fn non_canonical_signature_is_refused() {
    let package = Issue::sample("a").build();
    let n = u32::from_le_bytes(package[8..12].try_into().unwrap()) as usize;
    let c = u16::from_le_bytes(package[12 + n..14 + n].try_into().unwrap()) as usize;
    let s_at = 14 + n + c + 64 + 32;
    let mut p = package.clone();
    // ℓ = 2^252 + 27742317777372353535851937790883648493, little endian.
    let ell: [u8; 32] = [
        0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7, 0xa2, 0xde, 0xf9, 0xde, 0x14, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0x10,
    ];
    let mut carry = 0u16;
    for i in 0..32 {
        let sum = u16::from(p[s_at + i]) + u16::from(ell[i]) + carry;
        p[s_at + i] = sum as u8;
        carry = sum >> 8;
    }
    assert_eq!(code_of(&p), Code::Signature);
}

#[test]
fn client_version_and_platform() {
    let package = Issue::sample("a").build();
    let secrets = [request_secret("a")];
    let state = State::default();
    let policy = Policy::with_test_keys();
    let try_client = |c: &str| open_package(&package, &secrets, &state, &policy, &Client::parse(c).unwrap()).map(|_| ());
    assert!(try_client("android/1.2").is_ok());
    assert!(try_client("android/2.0").is_ok());
    assert_eq!(try_client("android/1.1").unwrap_err().code, Code::Client);
    assert_eq!(try_client("windows/9.9").unwrap_err().code, Code::Client);
    assert!(Client::parse("android").is_err());
    assert!(Client::parse("palm/1.0").is_err());
}

#[test]
fn small_order_ephemeral_key_is_refused() {
    let mut issue = Issue::sample("a");
    issue.eph_pub_override = Some([0u8; 32]);
    assert_eq!(code_of(&issue.build()), Code::KeyAgreement);
    let mut issue = Issue::sample("a");
    let mut one = [0u8; 32];
    one[0] = 1;
    issue.eph_pub_override = Some(one);
    assert_eq!(code_of(&issue.build()), Code::KeyAgreement);
}

#[test]
fn archive_rules() {
    let issue = Issue::sample("a");
    let raw = issue.raw_entries();
    let with = |entries: Vec<(String, Vec<u8>)>| {
        let mut i = Issue::sample("a");
        i.archive_override = Some(mpar(&entries));
        i.files_override = Some(i.files_value());
        code_of(&i.build())
    };
    // Path traversal, capitals, reserved names, duplicates, unsorted.
    for bad in ["../frame.png", "a/frame.png", "Frame.png", "con.png", "nul.wav", "x.json", "frame.gif", "frame"] {
        let mut e = raw.clone();
        e.push((bad.to_string(), vec![1, 2, 3]));
        e.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(with(e), Code::Archive, "{bad}");
    }
    let mut dup = raw.clone();
    dup.insert(0, dup[0].clone());
    assert_eq!(with(dup), Code::Archive);
    let mut unsorted = raw.clone();
    unsorted.swap(0, 1);
    assert_eq!(with(unsorted), Code::Archive);
    // An unused file.
    let mut extra = raw.clone();
    extra.push(("zzz.png".into(), meowpuut_skin::image::encode(&end_piece()).unwrap()));
    assert_eq!(with(extra), Code::Archive);
    // No manifest.
    let without: Vec<_> = raw.iter().filter(|(n, _)| n != "manifest.json").cloned().collect();
    assert_eq!(with(without), Code::Archive);
    // Length fields summing past the input, or past 4 MiB.
    let mut archive = mpar(&raw);
    archive[8 + 1 + 9..8 + 1 + 9 + 4].copy_from_slice(&(limits::FILE as u32).to_le_bytes());
    let mut i = Issue::sample("a");
    i.archive_override = Some(archive);
    i.files_override = Some(issue.files_value());
    assert_eq!(code_of(&i.build()), Code::Archive);
    let mut big = vec![];
    for k in 0..5 {
        big.push((format!("big{k}.png"), vec![0u8; limits::FILE]));
    }
    big.push(("manifest.json".into(), b"{}".to_vec()));
    big.sort_by(|a, b| a.0.cmp(&b.0));
    let mut i = Issue::sample("a");
    i.archive_override = Some(mpar(&big));
    i.files_override = Some(issue.files_value());
    assert_eq!(code_of(&i.build()), Code::TooLarge);
}

#[test]
fn archive_parser_survives_random_input() {
    let mut rng = Rng(77);
    for _ in 0..3000 {
        let mut bytes = b"MPAR\x01\x00".to_vec();
        let n = rng.below(600);
        bytes.extend(rng.bytes(n));
        let _ = meowpuut_skin::archive::parse(&bytes);
    }
    let raw = Issue::sample("a").raw_entries();
    let archive = mpar(&raw);
    for n in 0..archive.len().min(4000) {
        assert!(meowpuut_skin::archive::parse(&archive[..n]).is_err());
    }
    assert!(meowpuut_skin::archive::parse(&archive).is_ok());
}

#[test]
fn redeem_writes_nothing_on_failure() {
    let dir = temp_dir("nothing-on-failure");
    let mut p = Issue::sample("a").build();
    let last = p.len() - 1;
    p[last] ^= 1;
    let e = redeem(&p, &[request_secret("a")], &State::default(), &dir, &Policy::with_test_keys(), &android(), 1 << 20).unwrap_err();
    assert_eq!(e.code, Code::Payload);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
}
