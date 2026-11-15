//! Installed folders: verify-on-load against every kind of change, device
//! binding, rollback, revocation, idempotent reinstall, list and remove.

mod support;

use std::fs;
use std::path::Path;

use meowpuut_skin::install::{list_installed, redeem, remove, verify_installed, Client, Listed};
use meowpuut_skin::revoke::{parse_file, State};
use meowpuut_skin::trust::Policy;
use meowpuut_skin::{limits, Code};
use serde_json::json;
use support::*;

fn android() -> Client {
    Client::parse("android/1.2").unwrap()
}

fn policy() -> Policy {
    Policy::with_test_keys()
}

/// Redeems `issue` for request `label` into `dir`; returns the new state.
fn install(issue: &Issue, label: &str, state: &State, dir: &Path) -> Result<State, meowpuut_skin::Error> {
    redeem(&issue.build(), &[request_secret(label)], state, dir, &policy(), &android(), 1 << 20).map(|r| r.state)
}

fn verify(dir: &Path, skin: &str, state: &State, label: &str) -> Result<(), Code> {
    verify_installed(dir, skin, state, &policy(), Some(&[request_public(label)])).map(|_| ()).map_err(|e| e.code)
}

#[test]
fn redeem_installs_and_load_verifies() {
    let dir = temp_dir("installs");
    let state = install(&Issue::sample("a"), "a", &State::default(), &dir).unwrap();
    assert_eq!(state.highest("sakura_tea"), 3);
    let folder = dir.join("sakura_tea").join("3");
    let mut names: Vec<String> = fs::read_dir(&folder).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    names.sort();
    assert_eq!(
        names,
        [
            "binding.bin",
            "frame.png",
            "key-1.wav",
            "manifest.json",
            "mimimo_peek.png",
            "momo_hold.png",
            "return-1.wav",
            "right.png",
            "seal.bin",
            "space-1.wav"
        ]
    );
    assert_eq!(verify(&dir, "sakura_tea@3", &state, "a"), Ok(()));
    // The platform cannot read its keys (NULL): binding skipped, everything else checked.
    assert!(verify_installed(&dir, "sakura_tea@3", &state, &policy(), None).is_ok());
    // A release build refuses the test-key skin on load as well.
    let e = verify_installed(&dir, "sakura_tea@3", &state, &Policy::release(), Some(&[request_public("a")])).unwrap_err();
    assert_eq!(e.code, Code::TestKey);
    // Only temporaries of this redeem existed; none are left.
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
}

#[test]
fn reinstall_is_idempotent_and_downgrade_is_refused() {
    let dir = temp_dir("idempotent");
    let issue = Issue::sample("a");
    let state = install(&issue, "a", &State::default(), &dir).unwrap();
    let again = install(&issue, "a", &state, &dir).unwrap();
    assert_eq!(again, state);
    assert_eq!(verify(&dir, "sakura_tea@3", &again, "a"), Ok(()));

    let mut old = Issue::sample("a");
    old.rev = 2;
    old.manifest["rev"] = json!(2);
    assert_eq!(install(&old, "a", &state, &dir).unwrap_err().code, Code::Downgrade);
    // Removing the skin does not lower the highest revision.
    remove(&dir, "sakura_tea@3").unwrap();
    assert!(!dir.join("sakura_tea").exists());
    assert_eq!(install(&old, "a", &state, &dir).unwrap_err().code, Code::Downgrade);
    // With a lost state, an installed newer revision still refuses the older one.
    install(&issue, "a", &State::default(), &dir).unwrap();
    assert_eq!(install(&old, "a", &State::default(), &dir).unwrap_err().code, Code::Downgrade);
    // A newer revision replaces the older folder.
    let mut newer = Issue::sample("a");
    newer.rev = 4;
    newer.manifest["rev"] = json!(4);
    let state4 = install(&newer, "a", &state, &dir).unwrap();
    assert_eq!(state4.highest("sakura_tea"), 4);
    assert!(!dir.join("sakura_tea/3").exists());
    assert_eq!(verify(&dir, "sakura_tea@4", &state4, "a"), Ok(()));
}

#[test]
fn copied_back_old_revision_is_refused_on_load() {
    let dir = temp_dir("restore-old");
    let mut old = Issue::sample("a");
    old.rev = 2;
    old.manifest["rev"] = json!(2);
    let state2 = install(&old, "a", &State::default(), &dir).unwrap();
    let backup = temp_dir("restore-old-backup");
    copy_dir(&dir.join("sakura_tea/2"), &backup.join("2"));
    let state3 = install(&Issue::sample("a"), "a", &state2, &dir).unwrap();
    copy_dir(&backup.join("2"), &dir.join("sakura_tea/2"));
    assert_eq!(verify(&dir, "sakura_tea@2", &state3, "a"), Err(Code::Downgrade));
    assert_eq!(verify(&dir, "sakura_tea@3", &state3, "a"), Ok(()));
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        fs::copy(e.path(), to.join(e.file_name())).unwrap();
    }
}

fn installed(label: &str) -> (std::path::PathBuf, State) {
    let dir = temp_dir(label);
    let state = install(&Issue::sample("a"), "a", &State::default(), &dir).unwrap();
    (dir, state)
}

#[test]
fn every_file_change_is_caught_on_load() {
    let (dir, state) = installed("tamper-files");
    let folder = dir.join("sakura_tea/3");
    let names: Vec<String> = fs::read_dir(&folder).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    for name in &names {
        let path = folder.join(name);
        let original = fs::read(&path).unwrap();
        for at in [0, original.len() / 2, original.len() - 1] {
            let mut changed = original.clone();
            changed[at] ^= 0x01;
            fs::write(&path, &changed).unwrap();
            assert!(verify(&dir, "sakura_tea@3", &state, "a").is_err(), "{name} byte {at}");
        }
        fs::write(&path, &original[..original.len() - 1]).unwrap();
        assert!(verify(&dir, "sakura_tea@3", &state, "a").is_err(), "{name} truncated");
        let mut longer = original.clone();
        longer.push(0);
        fs::write(&path, &longer).unwrap();
        assert!(verify(&dir, "sakura_tea@3", &state, "a").is_err(), "{name} extended");
        fs::remove_file(&path).unwrap();
        assert!(verify(&dir, "sakura_tea@3", &state, "a").is_err(), "{name} removed");
        fs::write(&path, &original).unwrap();
        assert_eq!(verify(&dir, "sakura_tea@3", &state, "a"), Ok(()));
    }
}

#[test]
fn extra_files_folders_and_links_are_caught() {
    let (dir, state) = installed("tamper-extra");
    let folder = dir.join("sakura_tea/3");
    fs::write(folder.join("extra.png"), b"x").unwrap();
    assert_eq!(verify(&dir, "sakura_tea@3", &state, "a"), Err(Code::Tampered));
    fs::remove_file(folder.join("extra.png")).unwrap();
    fs::create_dir(folder.join("sub")).unwrap();
    assert_eq!(verify(&dir, "sakura_tea@3", &state, "a"), Err(Code::Tampered));
    fs::remove_dir(folder.join("sub")).unwrap();
    #[cfg(unix)]
    {
        let real = folder.join("frame.png");
        let moved = dir.join("frame-elsewhere.png");
        fs::rename(&real, &moved).unwrap();
        std::os::unix::fs::symlink(&moved, &real).unwrap();
        assert_eq!(verify(&dir, "sakura_tea@3", &state, "a"), Err(Code::Tampered));
        fs::remove_file(&real).unwrap();
        fs::rename(&moved, &real).unwrap();
        // The whole revision folder as a link.
        let elsewhere = dir.join("elsewhere");
        fs::rename(&folder, &elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &folder).unwrap();
        assert_eq!(verify(&dir, "sakura_tea@3", &state, "a"), Err(Code::NotInstalled));
        fs::remove_file(&folder).unwrap();
        fs::rename(&elsewhere, &folder).unwrap();
    }
    assert_eq!(verify(&dir, "sakura_tea@3", &state, "a"), Ok(()));
}

#[test]
fn manifest_swapped_from_another_skin_is_caught() {
    let (dir, state) = installed("swap-manifest");
    let mut other = Issue::sample("a");
    other.manifest["names"]["en"] = json!("Other Tea");
    let other_dir = temp_dir("swap-manifest-other");
    install(&other, "a", &State::default(), &other_dir).unwrap();
    fs::copy(other_dir.join("sakura_tea/3/manifest.json"), dir.join("sakura_tea/3/manifest.json")).unwrap();
    assert_eq!(verify(&dir, "sakura_tea@3", &state, "a"), Err(Code::Tampered));
    // Both files and seal from the other folder: a complete, genuinely signed skin, so it loads.
    copy_dir(&other_dir.join("sakura_tea/3"), &dir.join("sakura_tea/3"));
    assert_eq!(verify(&dir, "sakura_tea@3", &state, "a"), Ok(()));
}

#[test]
fn folder_moved_to_another_device_fails_the_binding() {
    let (dir, state) = installed("binding");
    assert_eq!(verify(&dir, "sakura_tea@3", &state, "b"), Err(Code::Binding));
    assert_eq!(
        verify_installed(&dir, "sakura_tea@3", &state, &policy(), Some(&[])).unwrap_err().code,
        Code::Binding,
        "a device without request keys"
    );
    // binding.bin replaced by device B's record.
    let issue_b = Issue::sample("b");
    let dir_b = temp_dir("binding-b");
    install(&issue_b, "b", &State::default(), &dir_b).unwrap();
    fs::copy(dir_b.join("sakura_tea/3/binding.bin"), dir.join("sakura_tea/3/binding.bin")).unwrap();
    assert_eq!(verify(&dir, "sakura_tea@3", &state, "b"), Err(Code::Tampered));
}

#[test]
fn custom_skin_string_is_reparsed() {
    let (dir, state) = installed("custom-skin");
    for bad in ["../x@1", "sakura_tea@03", "sakura_tea@3/..", "Sakura_tea@3", "con@1", "sakura_tea", "sakura_tea@4294967296", ""] {
        assert_eq!(verify(&dir, bad, &state, "a"), Err(Code::Argument), "{bad}");
    }
    assert_eq!(verify(&dir, "sakura_tea@2", &state, "a"), Err(Code::NotInstalled));
    // A folder renamed to another revision: the seal disagrees with the path.
    fs::rename(dir.join("sakura_tea/3"), dir.join("sakura_tea/5")).unwrap();
    assert_eq!(verify(&dir, "sakura_tea@5", &state, "a"), Err(Code::Tampered));
}

#[test]
fn revocation_by_kid_skin_and_serial() {
    let (dir, state) = installed("revocation");
    let root = root();
    for (kind, value) in [("kid", SKIN_KID), ("skin", "sakura_tea@3"), ("serial", "0123456789abcdef")] {
        let file = revocation(&root, ROOT_KID, 7, &[(kind, value)]);
        let parsed = parse_file(&file, &policy()).unwrap();
        let mut revoked = state.clone();
        assert_eq!(revoked.merge(&parsed).unwrap(), 1);
        assert_eq!(verify(&dir, "sakura_tea@3", &revoked, "a"), Err(Code::Revoked), "{kind}");
        let fresh = temp_dir(&format!("revocation-{kind}"));
        assert_eq!(install(&Issue::sample("a"), "a", &revoked, &fresh).unwrap_err().code, Code::Revoked, "{kind}");
    }
}

#[test]
fn revocation_files_are_root_signed_and_add_only() {
    let root = root();
    let p = policy();
    // A subkey (even with both uses) cannot sign a revocation.
    let by_subkey = revocation(&signing(SKIN_KID), SKIN_KID, 9, &[("kid", "test-2026-other")]);
    assert_eq!(parse_file(&by_subkey, &p).unwrap_err().code, Code::Revocation);
    let forged = revocation(&signing(SKIN_KID), ROOT_KID, 9, &[]);
    assert_eq!(parse_file(&forged, &p).unwrap_err().code, Code::Revocation);
    // A release build refuses a test root's file.
    let file = revocation(&root, ROOT_KID, 1, &[("kid", "x")]);
    assert_eq!(parse_file(&file, &Policy::release()).unwrap_err().code, Code::TestKey);
    // Union: a later file with fewer entries removes nothing; seq only grows.
    let mut state = State::default();
    state.merge(&parse_file(&revocation(&root, ROOT_KID, 5, &[("kid", "a"), ("serial", "00000000000000ff")]), &p).unwrap()).unwrap();
    let added = state.merge(&parse_file(&revocation(&root, ROOT_KID, 9, &[]), &p).unwrap()).unwrap();
    assert_eq!(added, 0);
    assert_eq!(state.revoked.len(), 2);
    assert_eq!(state.seq, 9);
    state.merge(&parse_file(&revocation(&root, ROOT_KID, 2, &[("skin", "a@1")]), &p).unwrap()).unwrap();
    assert_eq!((state.revoked.len(), state.seq), (3, 9));
    // Malformed entries and files.
    for entries in [&[("kid", "Bad")][..], &[("skin", "a@0")], &[("serial", "xyz")], &[("time", "1")]] {
        assert_eq!(parse_file(&revocation(&root, ROOT_KID, 1, entries), &p).unwrap_err().code, Code::Revocation);
    }
    let good = revocation(&root, ROOT_KID, 1, &[("kid", "a")]);
    for n in 0..good.len() {
        assert!(parse_file(&good[..n], &p).is_err());
    }
    for i in 0..good.len() {
        let mut b = good.clone();
        b[i] ^= 1;
        assert!(parse_file(&b, &p).is_err(), "byte {i}");
    }
    assert_eq!(parse_file(&vec![0u8; limits::REVOCATION_FILE + 1], &p).unwrap_err().code, Code::TooLarge);
    let mut rng = Rng(99);
    for _ in 0..2000 {
        let mut b = meowpuut_skin::revoke::MAGIC.to_vec();
        let n = rng.below(300);
        b.extend(rng.bytes(n));
        assert!(parse_file(&b, &p).is_err());
    }
}

#[test]
fn twelve_skins_at_most() {
    let dir = temp_dir("twelve");
    let mut state = State::default();
    for k in 0..limits::INSTALLED {
        let mut issue = Issue::sample("a");
        issue.id = format!("skin{k}");
        issue.manifest["id"] = json!(issue.id);
        state = install(&issue, "a", &state, &dir).unwrap();
    }
    let mut thirteenth = Issue::sample("a");
    thirteenth.id = "skin99".into();
    thirteenth.manifest["id"] = json!("skin99");
    assert_eq!(install(&thirteenth, "a", &state, &dir).unwrap_err().code, Code::TooManySkins);
    // Reinstalling one of the twelve is fine.
    let mut again = Issue::sample("a");
    again.id = "skin3".into();
    again.manifest["id"] = json!("skin3");
    install(&again, "a", &state, &dir).unwrap();
    // List verifies every folder; a tampered one is reported, not hidden.
    fs::write(dir.join("skin5/3/manifest.json"), b"{}").unwrap();
    let rows = list_installed(&dir, &state, &policy(), Some(&[request_public("a")])).unwrap();
    assert_eq!(rows.len(), limits::INSTALLED);
    let refused: Vec<_> = rows.iter().filter_map(|r| if let Listed::Refused(s, c) = r { Some((s.clone(), *c)) } else { None }).collect();
    assert_eq!(refused, [("skin5@3".to_string(), Code::Tampered)]);
}

#[test]
fn small_result_buffer_installs_nothing() {
    let dir = temp_dir("small-buffer");
    let e = redeem(&Issue::sample("a").build(), &[request_secret("a")], &State::default(), &dir, &policy(), &android(), 64).unwrap_err();
    assert_eq!(e.code, Code::Buffer);
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
}

#[test]
fn remove_rules() {
    let (dir, _state) = installed("remove");
    assert_eq!(remove(&dir, "../sakura_tea@3").unwrap_err().code, Code::Argument);
    assert_eq!(remove(&dir, "sakura_tea@2").unwrap_err().code, Code::NotInstalled);
    remove(&dir, "sakura_tea@3").unwrap();
    assert_eq!(remove(&dir, "sakura_tea@3").unwrap_err().code, Code::NotInstalled);
}

/// The verify result carries the signed file list, so a platform that reads the
/// files afterwards checks the bytes it keeps (Android's CustomSkinLoader).
#[test]
fn verify_json_lists_the_signed_files() {
    let dir = temp_dir("verify-json");
    let state = install(&Issue::sample("a"), "a", &State::default(), &dir).unwrap();
    let text = meowpuut_skin::install::verify_json(&dir, "sakura_tea@3", &state.to_json(), &policy(), None).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["kind"], "recipient");
    let files = v["files"].as_array().unwrap();
    let folder = dir.join("sakura_tea").join("3");
    let mut names = Vec::new();
    for f in files {
        let name = f["n"].as_str().unwrap();
        let data = fs::read(folder.join(name)).unwrap();
        assert_eq!(f["len"].as_u64().unwrap(), data.len() as u64, "{name}");
        assert_eq!(f["sha256"].as_str().unwrap(), meowpuut_skin::codec::hex_encode(&meowpuut_skin::codec::sha256(&data)), "{name}");
        names.push(name.to_string());
    }
    assert!(names.contains(&"manifest.json".to_string()) && !names.contains(&"seal.bin".to_string()) && !names.contains(&"binding.bin".to_string()));
}
