//! Bundled packages (`.mpskinb`): signed by a `skin` subkey, plaintext, not
//! device-bound, installed from the app bundle. Same validator, rewrite,
//! layout, verify-on-load, rollback guard and revocation checks as a
//! recipient package; the kinds can never be confused.

mod support;

use std::fs;
use std::path::Path;

use ed25519_dalek::{Signature, Signer, Verifier, VerifyingKey};
use meowpuut_skin::container::{open_seal, parse_seal, split, Kind, BUNDLED_DOMAIN, BUNDLED_MAGIC, DOMAIN, MAGIC};
use meowpuut_skin::install::{
    install_bundled, install_bundled_json, list_installed, open_bundled, open_package, redeem, remove, verify_installed, Client,
    Listed,
};
use meowpuut_skin::revoke::{parse_file, State};
use meowpuut_skin::trust::Policy;
use meowpuut_skin::{limits, Code};
use serde_json::{json, Value};
use support::*;

fn ios() -> Client {
    Client::parse("ios/1.2").unwrap()
}

fn policy() -> Policy {
    Policy::with_test_keys()
}

fn open(package: &[u8]) -> Result<meowpuut_skin::install::Opened, meowpuut_skin::Error> {
    open_bundled(package, &State::default(), &policy(), &ios())
}

fn code_of(package: &[u8]) -> Code {
    open(package).map(|_| ()).unwrap_err().code
}

fn install(b: &Bundle, state: &State, dir: &Path) -> Result<State, meowpuut_skin::Error> {
    install_bundled(&b.build(), state, dir, &policy(), &ios(), 1 << 20).map(|r| r.state)
}

fn verify(dir: &Path, skin: &str, state: &State) -> Result<(), Code> {
    // A bundled skin ignores the request keys: another device's keys pass too.
    verify_installed(dir, skin, state, &policy(), Some(&[request_public("z")])).map(|_| ()).map_err(|e| e.code)
}

#[test]
fn sample_opens_with_the_same_rewrite_as_a_recipient_package() {
    let opened = open(&Bundle::sample().build()).unwrap();
    assert_eq!(opened.header.kind, Kind::Bundled);
    assert!(opened.header.encryption.is_none());
    assert_eq!(opened.request_index, None);
    assert_eq!(opened.skin.serial, BUNDLED_SERIAL);
    // The same assets through the recipient path rewrite to the same images and sounds.
    let mut issue = Issue::sample("a");
    issue.manifest["serial"] = json!(BUNDLED_SERIAL);
    let redeemed = open_package(&issue.build(), &[request_secret("a")], &State::default(), &policy(), &Client::parse("android/1.2").unwrap())
        .unwrap();
    assert_eq!(opened.skin.files, redeemed.skin.files);
    // An Android client opens it as well; macOS is not listed in the sample's minClient.
    assert!(open_bundled(&Bundle::sample().build(), &State::default(), &policy(), &Client::parse("android/1.2").unwrap()).is_ok());
    let e = open_bundled(&Bundle::sample().build(), &State::default(), &policy(), &Client::parse("macos/9.0").unwrap()).unwrap_err();
    assert_eq!(e.code, Code::Client);
    let e = open_bundled(&Bundle::sample().build(), &State::default(), &policy(), &Client::parse("ios/1.1").unwrap()).unwrap_err();
    assert_eq!(e.code, Code::Client);
}

#[test]
fn the_kinds_are_never_confused() {
    let bundled = Bundle::sample().build();
    let recipient = Issue::sample("a").build();
    // Each entry point refuses the other kind before verifying anything.
    assert_eq!(code_of(&recipient), Code::Kind);
    let e = open_package(&bundled, &[request_secret("a")], &State::default(), &policy(), &ios()).unwrap_err();
    assert_eq!(e.code, Code::Kind);
    // A recipient header re-framed with the bundled magic: verified under the
    // bundled domain, so the recipient signature fails.
    let mut relabeled = recipient.clone();
    relabeled[..8].copy_from_slice(&BUNDLED_MAGIC);
    assert_eq!(code_of(&relabeled), Code::Signature);
    // A bundled package re-framed as a recipient package: likewise.
    let mut relabeled = bundled.clone();
    relabeled[..8].copy_from_slice(&MAGIC);
    let e = open_package(&relabeled, &[request_secret("a")], &State::default(), &policy(), &ios()).unwrap_err();
    assert_eq!(e.code, Code::Signature);
    // A bundled header signed under the recipient domain (bundled magic): refused.
    let mut wrong_domain = Bundle::sample();
    wrong_domain.domain = DOMAIN.to_vec();
    assert_eq!(code_of(&wrong_domain.build()), Code::Signature);
    // A bundled header signed under the recipient domain and framed as a
    // recipient package: the signature verifies, the header does not parse
    // as a recipient header (it has `kind`, lacks `recipient`).
    let mut as_recipient = Bundle::sample();
    as_recipient.domain = DOMAIN.to_vec();
    as_recipient.magic = MAGIC;
    let e = open_package(&as_recipient.build(), &[request_secret("a")], &State::default(), &policy(), &ios()).unwrap_err();
    assert_eq!(e.code, Code::Header);
    // A recipient-shaped header (no `kind`) signed under the bundled domain: Header.
    let mut no_kind = Bundle::sample();
    no_kind.edit_signed = Some(Box::new(|h| {
        h.as_object_mut().unwrap().remove("kind");
    }));
    assert_eq!(code_of(&no_kind.build()), Code::Header);
}

fn signed_edit(edit: impl Fn(&mut Value) + 'static) -> Code {
    let mut b = Bundle::sample();
    b.edit_signed = Some(Box::new(edit));
    code_of(&b.build())
}

#[test]
fn every_header_field_is_checked() {
    assert_eq!(signed_edit(|h| h["kind"] = json!("recipient")), Code::Header);
    assert_eq!(signed_edit(|h| h["kind"] = json!("Bundled")), Code::Header);
    assert_eq!(signed_edit(|h| h["format"] = json!(2)), Code::Header);
    assert_eq!(signed_edit(|h| h["aead"] = json!("xchacha20poly1305")), Code::Header);
    assert_eq!(signed_edit(|h| h["nonce"] = json!("")), Code::Header);
    assert_eq!(signed_edit(|h| h["recipient"] = json!({})), Code::Header);
    assert_eq!(signed_edit(|h| h["id"] = json!("con")), Code::Header);
    assert_eq!(signed_edit(|h| h["rev"] = json!(0)), Code::Header);
    assert_eq!(signed_edit(|h| h["kid"] = json!(CODE_KID)), Code::Header);
    assert_eq!(signed_edit(|h| h["minClient"] = json!({"symbian": "1.0"})), Code::Header);
    assert_eq!(signed_edit(|h| h["payloadSha256"] = json!("00".repeat(32))), Code::Payload);
    assert_eq!(signed_edit(|h| h["payloadSha256"] = json!("AB".repeat(32))), Code::Header);
    assert_eq!(signed_edit(|h| h["payloadLen"] = json!(h["payloadLen"].as_u64().unwrap() - 1)), Code::Payload);
    assert_eq!(signed_edit(|h| h["payloadLen"] = json!(7)), Code::TooLarge);
    assert_eq!(signed_edit(|h| h["payloadLen"] = json!(limits::ARCHIVE + 1)), Code::TooLarge);
    assert_eq!(signed_edit(|h| h["extra"] = json!(1)), Code::Header);
    assert_eq!(signed_edit(|h| h["files"][0]["len"] = json!(1)), Code::Files);
    assert_eq!(signed_edit(|h| h["files"][0]["n"] = json!("../frame.png")), Code::Header);
    // The manifest's id/rev must equal the header's.
    assert_eq!(signed_edit(|h| h["rev"] = json!(4)), Code::Manifest);
}

#[test]
fn payload_and_validator_failures() {
    let good = Bundle::sample().build();
    let at = seal_len(&good);
    // Any payload byte changed: the signed hash no longer matches.
    for i in [at, at + 7, at + 200, good.len() - 1] {
        let mut p = good.clone();
        p[i] ^= 1;
        assert_eq!(code_of(&p), Code::Payload, "byte {i}");
    }
    assert_eq!(code_of(&good[..good.len() - 1]), Code::Payload);
    let mut longer = good.clone();
    longer.push(0);
    assert_eq!(code_of(&longer), Code::Payload);
    // A broken archive, correctly signed: Archive.
    let mut b = Bundle::sample();
    b.archive_override = Some(b"MPAR\x01\x00\x00\x00junk".to_vec());
    b.files_override = Some(json!([{"len": 1, "n": "manifest.json", "sha256": "00".repeat(32)}]));
    assert_eq!(code_of(&b.build()), Code::Archive);
    // A manifest the validator refuses, correctly signed: Manifest.
    let mut b = Bundle::sample();
    let files = signed_files(&b.raw_entries(), &b.id, b.rev);
    b.manifest["keys"]["light"]["text"] = json!("#FFFFFF");
    b.files_override = Some(files);
    assert_eq!(code_of(&b.build()), Code::Manifest);
    // Signed files that are not what the rewrite produces: Files.
    let mut b = Bundle::sample();
    let mut files = signed_files(&b.raw_entries(), &b.id, b.rev);
    files[1]["sha256"] = json!("00".repeat(32));
    b.files_override = Some(files);
    assert_eq!(code_of(&b.build()), Code::Files);
}

#[test]
fn every_flipped_bit_of_the_seal_is_refused() {
    let good = Bundle::sample().build();
    let end = seal_len(&good);
    for i in 0..end {
        for bit in [0x01u8, 0x80] {
            let mut p = good.clone();
            p[i] ^= bit;
            assert!(open(&p).is_err(), "byte {i} bit {bit:#x} accepted");
        }
    }
}

#[test]
fn trust_policy_revocation_and_strictness() {
    let good = Bundle::sample().build();
    // Release build: test kids refused.
    let e = open_bundled(&good, &State::default(), &Policy::release(), &ios()).unwrap_err();
    assert_eq!(e.code, Code::TestKey);
    // A subkey without the skin use.
    let mut b = Bundle::sample();
    b.cert = cert(&root(), ROOT_KID, SKIN_KID, &signing(SKIN_KID), &["code"]);
    assert_eq!(code_of(&b.build()), Code::KeyUse);
    // Revoked subkey, revoked id@rev, revoked serial.
    for entries in [
        vec![("kid", SKIN_KID)],
        vec![("skin", "sakura_tea@3")],
        vec![("serial", BUNDLED_SERIAL)],
    ] {
        let mut state = State::default();
        state.merge(&parse_file(&revocation(&root(), ROOT_KID, 1, &entries), &policy()).unwrap()).unwrap();
        let e = open_bundled(&good, &state, &policy(), &ios()).unwrap_err();
        assert_eq!(e.code, Code::Revoked, "{entries:?}");
    }
    // Downgrade.
    let mut state = State::default();
    state.see("sakura_tea", 4).unwrap();
    assert_eq!(open_bundled(&good, &state, &policy(), &ios()).unwrap_err().code, Code::Downgrade);
    // Non-canonical S (S + ℓ).
    let at = signature_at(&good);
    let sig: [u8; 64] = good[at..at + 64].try_into().unwrap();
    let mut p = good.clone();
    p[at..at + 64].copy_from_slice(&s_plus_ell(&sig));
    assert_eq!(code_of(&p), Code::Signature);
    // Small-order R: a lax verifier accepts it, verify_strict does not.
    let b = Bundle::sample();
    let header = b.header();
    let message = [BUNDLED_DOMAIN, header.as_slice()].concat();
    let forged = small_order_r_signature(&b.signer, &message);
    let key = b.signer.verifying_key();
    assert!(key.verify(&message, &Signature::from_bytes(&forged)).is_ok(), "the lax verifier accepts small-order R");
    assert!(key.verify_strict(&message, &Signature::from_bytes(&forged)).is_err());
    let p = frame(&BUNDLED_MAGIC, &header, &b.cert, &forged, &b.archive());
    assert_eq!(code_of(&p), Code::Signature);
}

#[test]
fn the_small_order_encodings_are_the_ones_dalek_refuses() {
    for enc in small_order_encodings() {
        let key = VerifyingKey::from_bytes(&enc).expect("decompresses");
        assert!(key.is_weak(), "{}", meowpuut_skin::codec::hex_encode(&enc));
    }
    // A real key and the base point are not weak.
    assert!(!signing(SKIN_KID).verifying_key().is_weak());
    let sig = signing(SKIN_KID).sign(b"x");
    assert!(signing(SKIN_KID).verifying_key().verify_strict(b"x", &sig).is_ok());
}

#[test]
fn install_verify_list_remove() {
    let dir = temp_dir("bundled-install");
    let state = install(&Bundle::sample(), &State::default(), &dir).unwrap();
    assert_eq!(state.highest("sakura_tea"), 3);
    let folder = dir.join("sakura_tea/3");
    let mut names: Vec<String> = fs::read_dir(&folder).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    names.sort();
    assert_eq!(
        names,
        ["frame.png", "key-1.wav", "manifest.json", "mimimo_peek.png", "momo_hold.png", "return-1.wav", "right.png", "seal.bin", "space-1.wav"]
    );
    let package = Bundle::sample().build();
    assert_eq!(fs::read(folder.join("seal.bin")).unwrap(), package[..seal_len(&package)]);
    assert_eq!(verify(&dir, "sakura_tea@3", &state), Ok(()));
    // Not device-bound: no keys, an empty key list and NULL all pass.
    assert!(verify_installed(&dir, "sakura_tea@3", &state, &policy(), Some(&[])).is_ok());
    assert!(verify_installed(&dir, "sakura_tea@3", &state, &policy(), None).is_ok());
    let installed = verify_installed(&dir, "sakura_tea@3", &state, &policy(), None).unwrap();
    assert_eq!(installed.header.kind, Kind::Bundled);
    // A release build refuses it on load.
    let e = verify_installed(&dir, "sakura_tea@3", &state, &Policy::release(), None).unwrap_err();
    assert_eq!(e.code, Code::TestKey);
    // A planted binding.bin is an extra file.
    fs::write(folder.join("binding.bin"), b"{}").unwrap();
    assert_eq!(verify(&dir, "sakura_tea@3", &state), Err(Code::Tampered));
    fs::remove_file(folder.join("binding.bin")).unwrap();
    // Every file change is caught.
    for name in &names {
        let path = folder.join(name);
        let original = fs::read(&path).unwrap();
        let mut changed = original.clone();
        changed[original.len() / 2] ^= 1;
        fs::write(&path, &changed).unwrap();
        assert!(verify(&dir, "sakura_tea@3", &state).is_err(), "{name}");
        fs::remove_file(&path).unwrap();
        assert!(verify(&dir, "sakura_tea@3", &state).is_err(), "{name} removed");
        fs::write(&path, &original).unwrap();
    }
    assert_eq!(verify(&dir, "sakura_tea@3", &state), Ok(()));
    // Listing shows it with its kind.
    let rows = list_installed(&dir, &state, &policy(), None).unwrap();
    assert!(matches!(&rows[..], [Listed::Ok(i)] if i.header.kind == Kind::Bundled));
    let json = meowpuut_skin::install::list_json(&dir, &state.to_json(), &policy(), None).unwrap();
    assert!(json.contains("\"kind\":\"bundled\""), "{json}");
    // Idempotent reinstall (every app launch after an update may call it).
    let again = install(&Bundle::sample(), &state, &dir).unwrap();
    assert_eq!(again, state);
    remove(&dir, "sakura_tea@3").unwrap();
    assert_eq!(verify(&dir, "sakura_tea@3", &state), Err(Code::NotInstalled));
    // The removed revision does not come back below the highest seen.
    assert_eq!(install(&Bundle::at_rev(2), &state, &dir).unwrap_err().code, Code::Downgrade);
}

#[test]
fn rollback_guard_is_shared_with_redeemed_skins() {
    let dir = temp_dir("bundled-rollback");
    // A redeemed rev 4 on Android, then an app update bundling rev 3: refused.
    let mut issue = Issue::sample("a");
    issue.rev = 4;
    issue.manifest["rev"] = json!(4);
    let android = Client::parse("android/1.2").unwrap();
    let state = redeem(&issue.build(), &[request_secret("a")], &State::default(), &dir, &policy(), &android, 1 << 20).unwrap().state;
    let e = install_bundled(&Bundle::sample().build(), &state, &dir, &policy(), &android, 1 << 20).unwrap_err();
    assert_eq!(e.code, Code::Downgrade);
    // With the state lost, the installed rev 4 still refuses it.
    let e = install_bundled(&Bundle::sample().build(), &State::default(), &dir, &policy(), &android, 1 << 20).unwrap_err();
    assert_eq!(e.code, Code::Downgrade);
    // A bundled rev 5 replaces the redeemed rev 4.
    let state5 = install_bundled(&Bundle::at_rev(5).build(), &state, &dir, &policy(), &android, 1 << 20).unwrap().state;
    assert_eq!(state5.highest("sakura_tea"), 5);
    assert!(!dir.join("sakura_tea/4").exists());
    assert_eq!(verify(&dir, "sakura_tea@5", &state5), Ok(()));
    // A copied-back bundled rev 3 folder is refused on load.
    let other = temp_dir("bundled-rollback-old");
    install(&Bundle::sample(), &State::default(), &other).unwrap();
    fs::rename(other.join("sakura_tea/3"), dir.join("sakura_tea/3")).unwrap();
    assert_eq!(verify(&dir, "sakura_tea@3", &state5), Err(Code::Downgrade));
}

#[test]
fn recipient_seal_planted_as_bundled_folder_is_refused() {
    // A redeemed folder with its binding.bin removed and the seal's magic
    // changed to the bundled one (an attempt to drop the device binding).
    let dir = temp_dir("bundled-plant");
    let android = Client::parse("android/1.2").unwrap();
    let state = redeem(&Issue::sample("a").build(), &[request_secret("a")], &State::default(), &dir, &policy(), &android, 1 << 20)
        .unwrap()
        .state;
    let folder = dir.join("sakura_tea/3");
    fs::remove_file(folder.join("binding.bin")).unwrap();
    let mut seal = fs::read(folder.join("seal.bin")).unwrap();
    seal[..8].copy_from_slice(&BUNDLED_MAGIC);
    fs::write(folder.join("seal.bin"), &seal).unwrap();
    assert_eq!(verify(&dir, "sakura_tea@3", &state), Err(Code::Signature));
    // The seal alone: same answer.
    let parsed = parse_seal(&seal).unwrap();
    assert_eq!(parsed.kind, Kind::Bundled);
    assert_eq!(open_seal(&parsed, &policy(), &Default::default()).unwrap_err().code, Code::Signature);
}

#[test]
fn json_entry_point_and_limits() {
    let dir = temp_dir("bundled-json");
    let package = Bundle::sample().build();
    let out = install_bundled_json(&package, "", &dir, "ios/1.2", &policy(), 1 << 20).unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["kind"], "bundled");
    assert_eq!(v["skin"], "sakura_tea@3");
    assert_eq!(v["state"]["highest"]["sakura_tea"], 3);
    assert_eq!(install_bundled_json(&package, "", &dir, "ios", &policy(), 1 << 20).unwrap_err().code, Code::Argument);
    assert_eq!(install_bundled_json(&package, "{}", &dir, "ios/1.2", &policy(), 1 << 20).unwrap_err().code, Code::State);
    // A result buffer too small: nothing changes.
    let fresh = temp_dir("bundled-json-small");
    assert_eq!(install_bundled_json(&package, "", &fresh, "ios/1.2", &policy(), 10).unwrap_err().code, Code::Buffer);
    assert_eq!(fs::read_dir(&fresh).unwrap().count(), 0);
    // Size limit before reading.
    assert_eq!(code_of(&vec![0u8; limits::PACKAGE + 1]), Code::TooLarge);
    let (seal, payload) = split(&package).unwrap();
    assert_eq!(seal.kind, Kind::Bundled);
    assert_eq!(seal.len + payload.len(), package.len());
}
