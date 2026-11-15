//! Golden vectors for bundled packages and installed seals in
//! `Tests/skin-vectors/bundled/` (test keys only, kids `test-…`), indexed by
//! `index.json` for the Swift/CryptoKit verifier of `SEAL.md` and the Kotlin
//! side. `bundled_golden_vectors` runs every case through this crate;
//! `bundled_vectors_are_current` fails when the committed files are stale.
//! To write them again after a deliberate change:
//!
//! ```text
//! MEOWPUUT_WRITE_SKIN_VECTORS=1 cargo test --release -p meowpuut-skin --test bundled_vectors -- --ignored regenerate
//! ```

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use ed25519_dalek::Signer;
use meowpuut_skin::codec::{b64_encode, hex_encode, sha256};
use meowpuut_skin::container::{parse_seal, BUNDLED_DOMAIN, BUNDLED_MAGIC, DOMAIN, MAGIC};
use meowpuut_skin::install::{install_bundled, open_bundled, verify_installed, Client};
use meowpuut_skin::revoke::State;
use meowpuut_skin::trust::{Policy, CERT_DOMAIN, TEST_ROOT};
use meowpuut_skin::{limits, Code};
use serde_json::{json, Value};
use support::*;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../Tests/skin-vectors/bundled")
}

const CLIENT: &str = "ios/1.2";
const SKIN: &str = "sakura_tea@3";
const FOLDER: &str = "installed/sakura_tea/3";

fn empty_state() -> Value {
    json!({"highest": {}, "revoked": {"kid": [], "serial": [], "skin": []}, "seq": 0, "v": 1})
}

fn state_with(kind: &str, value: &str) -> Value {
    let mut s = empty_state();
    s["revoked"][kind] = json!([value]);
    s
}

fn highest(rev: u32) -> Value {
    let mut s = empty_state();
    s["highest"]["sakura_tea"] = json!(rev);
    s
}

fn seal_of(package: &[u8]) -> Vec<u8> {
    package[..seal_len(package)].to_vec()
}

fn with_signature(package: &[u8], sig: &[u8; 64]) -> Vec<u8> {
    let at = signature_at(package);
    let mut out = package.to_vec();
    out[at..at + 64].copy_from_slice(sig);
    out
}

/// Every vector file (path relative to `bundled/`) and the index.
fn build() -> (BTreeMap<String, Vec<u8>>, Value) {
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let sample = Bundle::sample();
    let good = sample.build();
    files.insert("sakura_tea-3.mpskinb".into(), good.clone());

    // ---- package variants ----
    let n = u32::from_le_bytes(good[8..12].try_into().unwrap()) as usize;
    let header_text = std::str::from_utf8(&good[12..12 + n]).unwrap().to_string();
    let rev_at = 12 + header_text.find("\"rev\":3").unwrap() + 6;
    let mut tampered_header = good.clone();
    tampered_header[rev_at] = b'4';
    let at = signature_at(&good);
    let sig: [u8; 64] = good[at..at + 64].try_into().unwrap();
    let noncanonical = with_signature(&good, &s_plus_ell(&sig));
    let message = [BUNDLED_DOMAIN, &sample.header()].concat();
    let small_r = with_signature(&good, &small_order_r_signature(&sample.signer, &message));
    let mut wrong_domain_bundle = Bundle::sample();
    wrong_domain_bundle.domain = DOMAIN.to_vec();
    let wrong_domain = wrong_domain_bundle.build();
    let recipient = Issue::sample("a").build();
    let mut relabeled = recipient.clone();
    relabeled[..8].copy_from_slice(&BUNDLED_MAGIC);
    let mut code_only = Bundle::sample();
    code_only.cert = cert(&root(), ROOT_KID, SKIN_KID, &signing(SKIN_KID), &["code"]);
    let code_only = code_only.build();
    let mut payload_flip = good.clone();
    let last = payload_flip.len() - 1;
    payload_flip[last] ^= 1;
    let rev4 = Bundle::at_rev(4).build();
    let mut macos_only = Bundle::sample();
    macos_only.min_client = json!({"macos": "1.2"});
    let macos_only = macos_only.build();

    let packages: Vec<(&str, &str, Vec<u8>)> = vec![
        ("tampered-header", "header \"rev\":3 changed to \"rev\":4 after signing", tampered_header.clone()),
        ("noncanonical-s", "signature S replaced by S + ℓ", noncanonical.clone()),
        ("small-order-r", "R = identity (01 00…00), S = k·a: a lax verifier accepts it, verify_strict refuses", small_r.clone()),
        ("wrong-domain", "bundled header signed under the recipient domain \"meowpuut-skin/v2\\0\"", wrong_domain.clone()),
        ("relabeled-recipient", "the recipient vector ../sakura_tea-3.mpskin with the bundled magic", relabeled.clone()),
        ("code-only-subkey", "certificate with uses [\"code\"]", code_only.clone()),
        ("payload-flipped", "last payload byte changed", payload_flip),
        ("sakura_tea-4", "the same skin at rev 4 (valid)", rev4.clone()),
        ("macos-only", "minClient names only macos", macos_only),
    ];
    for (name, _, bytes) in &packages {
        files.insert(format!("packages/{name}.mpskinb"), bytes.clone());
    }

    // ---- the installed folder of the good package ----
    let opened = open_bundled(&good, &State::default(), &Policy::with_test_keys(), &Client::parse(CLIENT).unwrap()).unwrap();
    for (name, data) in &opened.skin.files {
        files.insert(format!("{FOLDER}/{name}"), data.clone());
    }
    files.insert(format!("{FOLDER}/seal.bin"), seal_of(&good));

    // ---- seal variants for load cases ----
    let mut trailing = seal_of(&good);
    trailing.push(0);
    let mut bad_magic = seal_of(&good);
    bad_magic[7] = 0x09;
    let seals: Vec<(&str, Vec<u8>)> = vec![
        ("tampered-header", seal_of(&tampered_header)),
        ("noncanonical-s", seal_of(&noncanonical)),
        ("small-order-r", seal_of(&small_r)),
        ("wrong-domain", seal_of(&wrong_domain)),
        ("relabeled-recipient", seal_of(&relabeled)),
        ("code-only-subkey", seal_of(&code_only)),
        ("rev4", seal_of(&rev4)),
        ("trailing-byte", trailing),
        ("unknown-magic", bad_magic),
    ];
    for (name, bytes) in &seals {
        files.insert(format!("seals/{name}.seal.bin"), bytes.clone());
    }

    // ---- index ----
    let package_cases = json!([
        {"name": "valid", "file": "sakura_tea-3.mpskinb", "policy": "test", "expect": 0},
        {"name": "release build", "file": "sakura_tea-3.mpskinb", "policy": "release", "expect": Code::TestKey.number()},
        {"name": "a recipient package", "file": "../sakura_tea-3.mpskin", "policy": "test", "expect": Code::Kind.number()},
        {"name": "header changed after signing", "file": "packages/tampered-header.mpskinb", "policy": "test", "expect": Code::Signature.number()},
        {"name": "non-canonical S", "file": "packages/noncanonical-s.mpskinb", "policy": "test", "expect": Code::Signature.number()},
        {"name": "small-order R", "file": "packages/small-order-r.mpskinb", "policy": "test", "expect": Code::Signature.number()},
        {"name": "signed under the recipient domain", "file": "packages/wrong-domain.mpskinb", "policy": "test", "expect": Code::Signature.number()},
        {"name": "recipient package relabeled as bundled", "file": "packages/relabeled-recipient.mpskinb", "policy": "test", "expect": Code::Signature.number()},
        {"name": "subkey without the skin use", "file": "packages/code-only-subkey.mpskinb", "policy": "test", "expect": Code::KeyUse.number()},
        {"name": "payload changed", "file": "packages/payload-flipped.mpskinb", "policy": "test", "expect": Code::Payload.number()},
        {"name": "revoked subkey", "file": "sakura_tea-3.mpskinb", "policy": "test", "state": state_with("kid", SKIN_KID), "expect": Code::Revoked.number()},
        {"name": "revoked skin revision", "file": "sakura_tea-3.mpskinb", "policy": "test", "state": state_with("skin", SKIN), "expect": Code::Revoked.number()},
        {"name": "revoked serial", "file": "sakura_tea-3.mpskinb", "policy": "test", "state": state_with("serial", BUNDLED_SERIAL), "expect": Code::Revoked.number()},
        {"name": "downgrade", "file": "sakura_tea-3.mpskinb", "policy": "test", "state": highest(4), "expect": Code::Downgrade.number()},
        {"name": "not made for this platform", "file": "packages/macos-only.mpskinb", "policy": "test", "expect": Code::Client.number()},
        {"name": "next revision", "file": "packages/sakura_tea-4.mpskinb", "policy": "test", "expect": 0}
    ]);
    let seal_file = |name: &str| format!("seals/{name}.seal.bin");
    let f = |name: &str| format!("sakura_tea/3/{name}");
    let load_cases = json!([
        {"name": "valid", "policy": "test", "ops": [], "expect": 0},
        {"name": "release build", "policy": "release", "ops": [], "expect": Code::TestKey.number()},
        {"name": "image byte changed", "policy": "test", "ops": [{"op": "flip", "path": f("frame.png"), "offset": 100}], "expect": Code::Tampered.number()},
        {"name": "manifest truncated", "policy": "test", "ops": [{"op": "truncate", "path": f("manifest.json"), "length": 100}], "expect": Code::Tampered.number()},
        {"name": "sound extended", "policy": "test", "ops": [{"op": "write", "path": f("key-1.wav"), "hex": "00", "append": true}], "expect": Code::Tampered.number()},
        {"name": "extra file", "policy": "test", "ops": [{"op": "write", "path": f("extra.png"), "hex": "78"}], "expect": Code::Tampered.number()},
        {"name": "planted binding.bin", "policy": "test", "ops": [{"op": "write", "path": f("binding.bin"), "hex": "7b7d"}], "expect": Code::Tampered.number()},
        {"name": "missing sound", "policy": "test", "ops": [{"op": "delete", "path": f("key-1.wav")}], "expect": Code::Tampered.number()},
        {"name": "missing seal", "policy": "test", "ops": [{"op": "delete", "path": f("seal.bin")}], "expect": Code::Tampered.number()},
        {"name": "image replaced by a symbolic link", "policy": "test",
         "ops": [{"op": "move", "path": f("frame.png"), "to": "sakura_tea/frame-real.png"},
                 {"op": "symlink", "path": f("frame.png"), "target": "../frame-real.png"}],
         "expect": Code::Tampered.number()},
        {"name": "folder moved to another revision", "skin": "sakura_tea@4", "policy": "test",
         "ops": [{"op": "move", "path": "sakura_tea/3", "to": "sakura_tea/4"}], "expect": Code::Tampered.number()},
        {"name": "seal of rev 4 in the rev 3 folder", "policy": "test", "ops": [{"op": "copy", "from": seal_file("rev4"), "path": f("seal.bin")}], "expect": Code::Tampered.number()},
        {"name": "seal header changed after signing", "policy": "test", "ops": [{"op": "copy", "from": seal_file("tampered-header"), "path": f("seal.bin")}], "expect": Code::Signature.number()},
        {"name": "seal with non-canonical S", "policy": "test", "ops": [{"op": "copy", "from": seal_file("noncanonical-s"), "path": f("seal.bin")}], "expect": Code::Signature.number()},
        {"name": "seal with small-order R", "policy": "test", "ops": [{"op": "copy", "from": seal_file("small-order-r"), "path": f("seal.bin")}], "expect": Code::Signature.number()},
        {"name": "seal signed under the recipient domain", "policy": "test", "ops": [{"op": "copy", "from": seal_file("wrong-domain"), "path": f("seal.bin")}], "expect": Code::Signature.number()},
        {"name": "recipient seal relabeled as bundled", "policy": "test", "ops": [{"op": "copy", "from": seal_file("relabeled-recipient"), "path": f("seal.bin")}], "expect": Code::Signature.number()},
        {"name": "seal from a subkey without the skin use", "policy": "test", "ops": [{"op": "copy", "from": seal_file("code-only-subkey"), "path": f("seal.bin")}], "expect": Code::KeyUse.number()},
        {"name": "seal with a trailing byte", "policy": "test", "ops": [{"op": "copy", "from": seal_file("trailing-byte"), "path": f("seal.bin")}], "expect": Code::Malformed.number()},
        {"name": "seal with an unknown magic", "policy": "test", "ops": [{"op": "copy", "from": seal_file("unknown-magic"), "path": f("seal.bin")}], "expect": Code::Malformed.number()},
        {"name": "revoked subkey", "policy": "test", "state": state_with("kid", SKIN_KID), "ops": [], "expect": Code::Revoked.number()},
        {"name": "revoked root", "policy": "test", "state": state_with("kid", ROOT_KID), "ops": [], "expect": Code::Revoked.number()},
        {"name": "revoked skin revision", "policy": "test", "state": state_with("skin", SKIN), "ops": [], "expect": Code::Revoked.number()},
        {"name": "revoked serial", "policy": "test", "state": state_with("serial", BUNDLED_SERIAL), "ops": [], "expect": Code::Revoked.number()},
        {"name": "older than the highest revision seen", "policy": "test", "state": highest(4), "ops": [], "expect": Code::Downgrade.number()},
        {"name": "not installed", "skin": "sakura_tea@9", "policy": "test", "ops": [], "expect": Code::NotInstalled.number()},
        {"name": "malformed customSkin", "skin": "sakura_tea@03", "policy": "test", "ops": [], "expect": Code::Argument.number()}
    ]);

    // Layout of the good seal, for parsers.
    let seal = seal_of(&good);
    let c = u16::from_le_bytes(seal[12 + n..14 + n].try_into().unwrap()) as usize;
    let header: Value = serde_json::from_slice(&seal[12..12 + n]).unwrap();
    let cert_json: Value = serde_json::from_slice(&seal[14 + n..14 + n + c]).unwrap();
    let installed_files: Vec<Value> = opened
        .skin
        .files
        .iter()
        .map(|(name, d)| json!({"n": name, "len": d.len(), "sha256": hex_encode(&sha256(d))}))
        .collect();
    let (cert_bytes, cert_sig) = cert(&root(), ROOT_KID, SKIN_KID, &signing(SKIN_KID), &["code", "skin"]);
    let seed_hint = |label: &str| format!("SHA-256(\"meowpuut-skin test key {label}\")");
    let doc = json!({
        "about": "Bundled skin packages (.mpskinb) and installed seals. Test keys only; see crates/meowpuut-skin/SEAL.md. \
                  Paths are relative to this folder. expect: 0 = accepted, otherwise meowpuut_skin::Code (MPS_E_*).",
        "client": CLIENT,
        "constants": {
            "bundledMagic": hex_encode(&BUNDLED_MAGIC),
            "recipientMagic": hex_encode(&MAGIC),
            "bundledDomain": hex_encode(BUNDLED_DOMAIN),
            "recipientDomain": hex_encode(DOMAIN),
            "certDomain": hex_encode(CERT_DOMAIN),
            "ell": "edd3f55c1a631258d69cf7a2def9de1400000000000000000000000000000010",
            "smallOrderEncodings": small_order_encodings().iter().map(|e| hex_encode(e)).collect::<Vec<_>>(),
            "maxSeal": limits::SEAL,
            "maxHeader": limits::HEADER,
            "maxCert": limits::CERT_JSON
        },
        "keys": {
            "root": {"kid": ROOT_KID, "seed": seed_hint(ROOT_KID), "public": b64_encode(&root().verifying_key().to_bytes())},
            "subkey": {"kid": SKIN_KID, "seed": seed_hint(SKIN_KID), "public": b64_encode(&signing(SKIN_KID).verifying_key().to_bytes()),
                       "cert": String::from_utf8(cert_bytes).unwrap(), "certSignature": b64_encode(&cert_sig)}
        },
        "packages": package_cases,
        "packageFiles": packages.iter().map(|(name, what, _)| json!({"file": format!("packages/{name}.mpskinb"), "what": what})).collect::<Vec<_>>(),
        "installed": {
            "skinsDir": "installed",
            "skin": SKIN,
            "folder": FOLDER,
            "kind": "bundled",
            "files": installed_files,
            "seal": {
                "file": format!("{FOLDER}/seal.bin"),
                "length": seal.len(),
                "headerOffset": 12, "headerLength": n,
                "certOffset": 14 + n, "certLength": c,
                "certSignatureOffset": 14 + n + c,
                "signatureOffset": 78 + n + c,
                "header": header,
                "cert": cert_json,
                "signedMessageSha256": hex_encode(&sha256(&[BUNDLED_DOMAIN, &seal[12..12 + n]].concat()))
            }
        },
        "loadCases": load_cases,
        "loadCaseRules": "Copy the folder `installed` to a fresh skins directory, apply `ops` in order (paths relative to that \
                          directory; `from` relative to this folder), then verify `skin` (default sakura_tea@3) with the \
                          state (default empty) under the policy (test: test-root-1 trusted and test- kids accepted; release: \
                          no root trusted, test- kids refused). Ops: flip (XOR 0x01 at offset), truncate (to length), \
                          write (hex bytes; append when `append`), delete, move (rename to `to`), symlink (create a \
                          symbolic link at path pointing to target), copy (replace path with a vector file)."
    });
    (files, doc)
}

fn policy(name: &str) -> Policy {
    match name {
        "test" => Policy::with_test_keys(),
        _ => Policy::release(),
    }
}

fn state(case: &Value) -> State {
    match case.get("state") {
        Some(s) => State::parse(&s.to_string()).unwrap(),
        None => State::default(),
    }
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let target = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_tree(&e.path(), &target);
        } else {
            fs::copy(e.path(), target).unwrap();
        }
    }
}

fn apply(op: &Value, skins: &Path, vectors: &Path) {
    let path = skins.join(op["path"].as_str().unwrap());
    match op["op"].as_str().unwrap() {
        "flip" => {
            let mut b = fs::read(&path).unwrap();
            b[op["offset"].as_u64().unwrap() as usize] ^= 1;
            fs::write(&path, b).unwrap();
        }
        "truncate" => {
            let b = fs::read(&path).unwrap();
            fs::write(&path, &b[..op["length"].as_u64().unwrap() as usize]).unwrap();
        }
        "write" => {
            let data = meowpuut_skin::codec::hex_decode(op["hex"].as_str().unwrap()).unwrap();
            let mut b = if op["append"] == json!(true) { fs::read(&path).unwrap() } else { Vec::new() };
            b.extend_from_slice(&data);
            fs::write(&path, b).unwrap();
        }
        "delete" => fs::remove_file(&path).unwrap(),
        "move" => fs::rename(&path, skins.join(op["to"].as_str().unwrap())).unwrap(),
        "symlink" => std::os::unix::fs::symlink(op["target"].as_str().unwrap(), &path).unwrap(),
        "copy" => {
            fs::copy(vectors.join(op["from"].as_str().unwrap()), &path).unwrap();
        }
        other => panic!("unknown op {other}"),
    }
}

#[test]
#[ignore]
fn regenerate() {
    if std::env::var("MEOWPUUT_WRITE_SKIN_VECTORS").as_deref() != Ok("1") {
        eprintln!("set MEOWPUUT_WRITE_SKIN_VECTORS=1 to write Tests/skin-vectors/bundled");
        return;
    }
    let (files, doc) = build();
    let _ = fs::remove_dir_all(dir());
    for (name, data) in files {
        let path = dir().join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, data).unwrap();
    }
    let mut text = serde_json::to_string_pretty(&doc).unwrap();
    text.push('\n');
    fs::write(dir().join("index.json"), text).unwrap();
}

#[test]
fn bundled_golden_vectors() {
    let doc: Value = serde_json::from_slice(&fs::read(dir().join("index.json")).unwrap()).unwrap();
    assert_eq!(doc["keys"]["root"]["public"], b64_encode(&TEST_ROOT.public));
    assert!(doc["keys"]["subkey"]["kid"].as_str().unwrap().starts_with("test-"));
    let client = Client::parse(doc["client"].as_str().unwrap()).unwrap();
    let read = |name: &str| fs::read(dir().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
    // Install-bundled cases.
    let mut checked = 0;
    for case in doc["packages"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let skins = temp_dir(&format!("bv-pkg-{checked}"));
        let got = install_bundled(&read(case["file"].as_str().unwrap()), &state(case), &skins, &policy(case["policy"].as_str().unwrap()), &client, 1 << 20)
            .map(|_| 0)
            .unwrap_or_else(|e| e.code.number());
        assert_eq!(got, case["expect"].as_i64().unwrap() as i32, "package case {name}");
        checked += 1;
    }
    assert_eq!(checked, 16);
    // The committed installed folder is exactly what installing the package writes.
    let skins = temp_dir("bv-installed");
    install_bundled(&read("sakura_tea-3.mpskinb"), &State::default(), &skins, &Policy::with_test_keys(), &client, 1 << 20).unwrap();
    let mut written: Vec<String> = fs::read_dir(skins.join("sakura_tea/3")).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    written.sort();
    let mut committed: Vec<String> = fs::read_dir(dir().join(FOLDER)).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    committed.sort();
    assert_eq!(written, committed);
    for name in &written {
        assert_eq!(fs::read(skins.join("sakura_tea/3").join(name)).unwrap(), read(&format!("{FOLDER}/{name}")), "{name}");
    }
    // Load cases.
    let mut checked = 0;
    for case in doc["loadCases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let skins = temp_dir(&format!("bv-load-{checked}"));
        copy_tree(&dir().join("installed"), &skins);
        for op in case["ops"].as_array().unwrap() {
            apply(op, &skins, &dir());
        }
        let skin = case["skin"].as_str().unwrap_or(SKIN);
        let got = verify_installed(&skins, skin, &state(case), &policy(case["policy"].as_str().unwrap()), None)
            .map(|_| 0)
            .unwrap_or_else(|e| e.code.number());
        assert_eq!(got, case["expect"].as_i64().unwrap() as i32, "load case {name}");
        checked += 1;
    }
    assert_eq!(checked, 27);
    // The documented layout of the good seal.
    let seal_info = &doc["installed"]["seal"];
    let seal = read(seal_info["file"].as_str().unwrap());
    let parsed = parse_seal(&seal).unwrap();
    let ho = seal_info["headerOffset"].as_u64().unwrap() as usize;
    let hl = seal_info["headerLength"].as_u64().unwrap() as usize;
    assert_eq!(&seal[ho..ho + hl], parsed.header_bytes);
    let so = seal_info["signatureOffset"].as_u64().unwrap() as usize;
    assert_eq!(seal[so..so + 64], parsed.signature);
    assert_eq!(seal.len(), seal_info["length"].as_u64().unwrap() as usize);
    // A subkey signature from the published seed reproduces the committed one (Ed25519 is deterministic).
    let sig = signing(SKIN_KID).sign(&[BUNDLED_DOMAIN, parsed.header_bytes].concat()).to_bytes();
    assert_eq!(sig, parsed.signature);
}

#[test]
fn bundled_vectors_are_current() {
    let (files, doc) = build();
    for (name, data) in &files {
        assert_eq!(&fs::read(dir().join(name)).unwrap_or_default(), data, "{name} is stale: regenerate the vectors");
    }
    // No stray committed files.
    fn walk(base: &Path, rel: &str, out: &mut Vec<String>) {
        for e in fs::read_dir(base.join(rel)).unwrap() {
            let e = e.unwrap();
            let name = e.file_name().into_string().unwrap();
            let r = if rel.is_empty() { name } else { format!("{rel}/{name}") };
            if e.file_type().unwrap().is_dir() {
                walk(base, &r, out);
            } else {
                out.push(r);
            }
        }
    }
    let mut committed = Vec::new();
    walk(&dir(), "", &mut committed);
    committed.retain(|n| n != "index.json");
    committed.sort();
    assert_eq!(committed, files.keys().cloned().collect::<Vec<_>>());
    let index: Value = serde_json::from_slice(&fs::read(dir().join("index.json")).unwrap()).unwrap();
    assert_eq!(index, doc);
}
