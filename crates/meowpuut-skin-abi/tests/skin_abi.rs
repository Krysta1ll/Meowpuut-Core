//! The `mps_*` C ABI as a release build sees it.

use std::ffi::{c_char, CStr, CString};
use std::path::PathBuf;
use std::ptr::{null, null_mut};

use meowpuut_ffi::skin::*;

fn vectors() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../Tests/skin-vectors")
}

fn temp(label: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("skin-abi-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn c(text: &str) -> CString {
    CString::new(text).unwrap()
}

fn text(buf: &[u8]) -> String {
    CStr::from_bytes_until_nul(buf).unwrap().to_string_lossy().into_owned()
}

fn b64(text: &str) -> Vec<u8> {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let mut acc = 0u32;
    let mut bits = 0;
    for ch in text.bytes().filter(|&b| b != b'=') {
        acc = (acc << 6) | A.iter().position(|&a| a == ch).unwrap() as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

fn request_a_secret() -> Vec<u8> {
    let cases = std::fs::read_to_string(vectors().join("cases.json")).unwrap();
    let at = cases.find("\"request-a\"").unwrap();
    let rest = &cases[at..];
    let s = rest.find("\"secret\": \"").unwrap() + 11;
    let e = s + rest[s..].find('"').unwrap();
    b64(&rest[s..e])
}

/// This test binary links meowpuut-ffi without `skin-test-keys`: the release configuration.
#[test]
fn release_build_refuses_the_test_vectors() {
    assert_eq!(mps_skin_accepts_test_keys(), 0, "this crate must not enable skin-test-keys");
    let package = std::fs::read(vectors().join("sakura_tea-3.mpskin")).unwrap();
    let secret = request_a_secret();
    let dir = temp("release");
    let (state, skins, client) = (c(""), c(dir.to_str().unwrap()), c("android/1.2"));
    let mut out = vec![0u8; MPS_RESULT_CAP];
    // SAFETY: live buffers of the stated sizes and NUL-terminated strings.
    let code = unsafe {
        mps_skin_redeem(
            package.as_ptr(),
            package.len(),
            secret.as_ptr(),
            1,
            state.as_ptr(),
            skins.as_ptr(),
            client.as_ptr(),
            out.as_mut_ptr().cast(),
            out.len(),
        )
    };
    assert_eq!(code, 6, "{}", text(&out));
    assert!(text(&out).contains("\"error\":6"));
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "nothing installed");
    // SAFETY: static string.
    assert_eq!(unsafe { CStr::from_ptr(mps_skin_error(6)) }.to_str().unwrap(), "a test key is not accepted by this build");
    // The test root's revocation file too.
    let revoke = std::fs::read(vectors().join("revoke-kid.mprevoke")).unwrap();
    // SAFETY: as above.
    let code = unsafe { mps_revocation_merge(revoke.as_ptr(), revoke.len(), null(), out.as_mut_ptr().cast(), out.len()) };
    assert_eq!(code, 6);
    // The bundled test vector too, on install and on load of its committed installed folder.
    let bundled = std::fs::read(vectors().join("bundled/sakura_tea-3.mpskinb")).unwrap();
    let ios = c("ios/1.2");
    // SAFETY: as above.
    let code = unsafe {
        mps_skin_install_bundled(bundled.as_ptr(), bundled.len(), null(), skins.as_ptr(), ios.as_ptr(), out.as_mut_ptr().cast(), out.len())
    };
    assert_eq!(code, 6, "{}", text(&out));
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "nothing installed");
    let installed = c(vectors().join("bundled/installed").to_str().unwrap());
    let skin = c("sakura_tea@3");
    // SAFETY: as above.
    let code = unsafe { mps_skin_verify_installed(installed.as_ptr(), skin.as_ptr(), null(), null(), 0, out.as_mut_ptr().cast(), out.len()) };
    assert_eq!(code, 6);
}

#[test]
fn request_keys_and_redemption_ids() {
    let (mut secret, mut public, mut again) = ([0u8; 32], [0u8; 32], [0u8; 32]);
    // SAFETY: 32-byte buffers.
    unsafe {
        assert_eq!(mps_request_key_generate(secret.as_mut_ptr(), public.as_mut_ptr()), MPS_OK);
        assert_eq!(mps_request_public_key(secret.as_ptr(), again.as_mut_ptr()), MPS_OK);
        assert_eq!(mps_request_key_generate(null_mut(), public.as_mut_ptr()), 26);
    }
    assert_eq!(public, again);
    let mut id = [0u8; 80];
    // SAFETY: as above.
    let n = unsafe { mps_redemption_id(public.as_ptr(), id.as_mut_ptr().cast(), id.len()) };
    assert_eq!(n, 74);
    assert!(text(&id).starts_with("MPR2-"));
    // SAFETY: as above; a buffer one byte short.
    assert_eq!(unsafe { mps_redemption_id(public.as_ptr(), id.as_mut_ptr().cast(), 74) }, 0);
    // SAFETY: NULL key.
    assert_eq!(unsafe { mps_redemption_id(null(), id.as_mut_ptr().cast(), id.len()) }, 0);
}

#[test]
fn arguments_and_buffers() {
    let mut out = vec![0u8; 256];
    let dir = temp("args");
    let skins = c(dir.to_str().unwrap());
    // SAFETY: NULL arguments are refused, not read.
    unsafe {
        let code = mps_skin_redeem(null(), 0, null(), 0, null(), null(), null(), out.as_mut_ptr().cast(), out.len());
        assert_eq!(code, 26);
        let code = mps_skin_redeem(null(), mps_skin_max_package_len() + 1, null(), 0, null(), skins.as_ptr(), null(), out.as_mut_ptr().cast(), out.len());
        assert_eq!(code, 1);
        assert_eq!(mps_skin_remove(null(), null()), 26);
        assert_eq!(mps_skin_remove(skins.as_ptr(), c("../x@1").as_ptr()), 26);
        assert_eq!(mps_skin_remove(skins.as_ptr(), c("x@1").as_ptr()), 23);
        let bad = c("../x@1");
        assert_eq!(mps_skin_verify_installed(skins.as_ptr(), bad.as_ptr(), null(), null(), 0, out.as_mut_ptr().cast(), out.len()), 26);
        let none = c("sakura_tea@3");
        let keys = [0u8; 32];
        assert_eq!(mps_skin_verify_installed(skins.as_ptr(), none.as_ptr(), null(), keys.as_ptr(), 1, out.as_mut_ptr().cast(), out.len()), 23);
        let bad_state = c("{\"v\":2}");
        assert_eq!(mps_skin_verify_installed(skins.as_ptr(), none.as_ptr(), bad_state.as_ptr(), null(), 0, out.as_mut_ptr().cast(), out.len()), 21);
        // An empty folder lists nothing; a 3-byte buffer is too small for {"skins":[]}.
        assert_eq!(mps_skin_list_installed(skins.as_ptr(), null(), null(), 0, out.as_mut_ptr().cast(), out.len()), MPS_OK);
        assert_eq!(text(&out), "{\"skins\":[]}");
        assert_eq!(mps_skin_list_installed(skins.as_ptr(), null(), null(), 0, out.as_mut_ptr().cast(), 3), 27);
        assert_eq!(out[0], 0);
        // A random revocation file.
        let junk = [7u8; 100];
        assert_eq!(mps_revocation_merge(junk.as_ptr(), junk.len(), null(), out.as_mut_ptr().cast(), out.len()), 28);
        assert_eq!(mps_revocation_merge(junk.as_ptr(), 200 * 1024, null(), out.as_mut_ptr().cast::<c_char>(), out.len()), 1);
    }
    // SAFETY: static strings.
    unsafe {
        assert_eq!(CStr::from_ptr(mps_skin_error(0)).to_bytes(), b"");
        assert_eq!(CStr::from_ptr(mps_skin_error(999)).to_bytes(), b"unknown error");
    }
}
