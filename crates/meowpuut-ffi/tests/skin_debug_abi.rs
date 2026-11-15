//! The `mps_*` C ABI in a Debug/Lab build (`--features skin-test-keys`), with
//! the shared golden vectors: redeem, verify on load, list, remove.
//! Run: cargo test --release -p meowpuut-ffi --features skin-test-keys --test skin_debug_abi
#![cfg(feature = "skin-test-keys")]
#![allow(unsafe_code)]

use std::ffi::{CStr, CString};
use std::path::PathBuf;
use std::ptr::null;

use meowpuut_ffi::skin::*;
use meowpuut_skin::codec::b64_decode;
use serde_json::Value;

fn vectors() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../Tests/skin-vectors")
}

fn text(buf: &[u8]) -> String {
    CStr::from_bytes_until_nul(buf).unwrap().to_string_lossy().into_owned()
}

#[test]
fn redeem_verify_list_remove() {
    assert_eq!(mps_skin_accepts_test_keys(), 1);
    let cases: Value = serde_json::from_slice(&std::fs::read(vectors().join("cases.json")).unwrap()).unwrap();
    let secret = b64_decode(cases["keys"]["request-a"]["secret"].as_str().unwrap()).unwrap();
    let public = b64_decode(cases["keys"]["request-a"]["public"].as_str().unwrap()).unwrap();
    let other = b64_decode(cases["keys"]["request-b"]["public"].as_str().unwrap()).unwrap();
    let package = std::fs::read(vectors().join("sakura_tea-3.mpskin")).unwrap();
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("skin-debug-abi-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let skins = CString::new(dir.to_str().unwrap()).unwrap();
    let client = CString::new("android/1.2").unwrap();
    let mut out = vec![0u8; MPS_RESULT_CAP];
    // SAFETY: live buffers of the stated sizes, NUL-terminated strings.
    let code = unsafe {
        mps_skin_redeem(package.as_ptr(), package.len(), secret.as_ptr(), 1, null(), skins.as_ptr(), client.as_ptr(), out.as_mut_ptr().cast(), out.len())
    };
    assert_eq!(code, MPS_OK, "{}", text(&out));
    let result: Value = serde_json::from_str(&text(&out)).unwrap();
    assert_eq!(result["skin"], "sakura_tea@3");
    assert_eq!(result["state"]["highest"]["sakura_tea"], 3);
    let state = CString::new(result["state"].to_string()).unwrap();
    let skin = CString::new("sakura_tea@3").unwrap();
    // SAFETY: as above.
    unsafe {
        let code = mps_skin_verify_installed(skins.as_ptr(), skin.as_ptr(), state.as_ptr(), public.as_ptr(), 1, out.as_mut_ptr().cast(), out.len());
        assert_eq!(code, MPS_OK, "{}", text(&out));
        let code = mps_skin_verify_installed(skins.as_ptr(), skin.as_ptr(), state.as_ptr(), other.as_ptr(), 1, out.as_mut_ptr().cast(), out.len());
        assert_eq!(code, 25, "another device's keys");
        assert_eq!(mps_skin_list_installed(skins.as_ptr(), state.as_ptr(), public.as_ptr(), 1, out.as_mut_ptr().cast(), out.len()), MPS_OK);
        assert!(text(&out).contains("\"ok\":true"));
        // Revoke the test subkey: loading stops.
        let revoke = std::fs::read(vectors().join("revoke-kid.mprevoke")).unwrap();
        assert_eq!(mps_revocation_merge(revoke.as_ptr(), revoke.len(), state.as_ptr(), out.as_mut_ptr().cast(), out.len()), MPS_OK);
        let merged: Value = serde_json::from_str(&text(&out)).unwrap();
        assert_eq!(merged["added"], 1);
        let revoked = CString::new(merged["state"].to_string()).unwrap();
        let code = mps_skin_verify_installed(skins.as_ptr(), skin.as_ptr(), revoked.as_ptr(), public.as_ptr(), 1, out.as_mut_ptr().cast(), out.len());
        assert_eq!(code, 7);
        assert_eq!(mps_skin_remove(skins.as_ptr(), skin.as_ptr()), MPS_OK);
        let code = mps_skin_verify_installed(skins.as_ptr(), skin.as_ptr(), state.as_ptr(), public.as_ptr(), 1, out.as_mut_ptr().cast(), out.len());
        assert_eq!(code, 23);
    }
}

#[test]
fn install_bundled_verify_list_remove() {
    let package = std::fs::read(vectors().join("bundled/sakura_tea-3.mpskinb")).unwrap();
    let recipient = std::fs::read(vectors().join("sakura_tea-3.mpskin")).unwrap();
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("skin-debug-abi-bundled-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let skins = CString::new(dir.to_str().unwrap()).unwrap();
    let client = CString::new("ios/1.2").unwrap();
    let mut out = vec![0u8; MPS_RESULT_CAP];
    // SAFETY: live buffers of the stated sizes, NUL-terminated strings.
    unsafe {
        // The other kind is refused.
        let code = mps_skin_install_bundled(recipient.as_ptr(), recipient.len(), null(), skins.as_ptr(), client.as_ptr(), out.as_mut_ptr().cast(), out.len());
        assert_eq!(code, 32, "{}", text(&out));
        let code = mps_skin_install_bundled(package.as_ptr(), package.len(), null(), skins.as_ptr(), client.as_ptr(), out.as_mut_ptr().cast(), out.len());
        assert_eq!(code, MPS_OK, "{}", text(&out));
        let result: Value = serde_json::from_str(&text(&out)).unwrap();
        assert_eq!(result["kind"], "bundled");
        assert_eq!(result["skin"], "sakura_tea@3");
        let state = CString::new(result["state"].to_string()).unwrap();
        let skin = CString::new("sakura_tea@3").unwrap();
        // Not device-bound: NULL keys, no keys or any keys all verify.
        let any = [7u8; 32];
        assert_eq!(mps_skin_verify_installed(skins.as_ptr(), skin.as_ptr(), state.as_ptr(), null(), 0, out.as_mut_ptr().cast(), out.len()), MPS_OK);
        assert_eq!(mps_skin_verify_installed(skins.as_ptr(), skin.as_ptr(), state.as_ptr(), any.as_ptr(), 1, out.as_mut_ptr().cast(), out.len()), MPS_OK);
        assert!(text(&out).contains("\"kind\":\"bundled\""));
        assert_eq!(mps_skin_list_installed(skins.as_ptr(), state.as_ptr(), null(), 0, out.as_mut_ptr().cast(), out.len()), MPS_OK);
        assert!(text(&out).contains("\"ok\":true"));
        // Idempotent.
        let code = mps_skin_install_bundled(package.as_ptr(), package.len(), state.as_ptr(), skins.as_ptr(), client.as_ptr(), out.as_mut_ptr().cast(), out.len());
        assert_eq!(code, MPS_OK, "{}", text(&out));
        // The recipient entry point refuses a bundled package.
        let secret = [1u8; 32];
        let code = mps_skin_redeem(package.as_ptr(), package.len(), secret.as_ptr(), 1, null(), skins.as_ptr(), client.as_ptr(), out.as_mut_ptr().cast(), out.len());
        assert_eq!(code, 32);
        assert_eq!(mps_skin_remove(skins.as_ptr(), skin.as_ptr()), MPS_OK);
        let code = mps_skin_verify_installed(skins.as_ptr(), skin.as_ptr(), state.as_ptr(), null(), 0, out.as_mut_ptr().cast(), out.len());
        assert_eq!(code, 23);
        // NULL arguments.
        assert_eq!(mps_skin_install_bundled(null(), 0, null(), null(), null(), out.as_mut_ptr().cast(), out.len()), 26);
        assert_eq!(CStr::from_ptr(mps_skin_error(32)).to_str().unwrap(), "the package is of the other kind (bundled or per-recipient)");
    }
}
