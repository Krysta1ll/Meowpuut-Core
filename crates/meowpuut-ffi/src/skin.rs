//! Custom skins: the `mps_*` C ABI (cargo feature `skin`; `include/meowpuut.h`
//! under `#ifdef MEOWPUUT_SKIN`). Never built into a keyboard extension.
//!
//! Two package kinds: recipient packages (`mps_skin_redeem`, Android only)
//! and bundled packages shipped in an app update (`mps_skin_install_bundled`,
//! iOS and Android). Verify, list and remove handle both.
//!
//! Every function returns `MPS_OK` (0) or a positive error code
//! (`meowpuut_skin::Code`; text from `mps_skin_error`). JSON results are
//! NUL-terminated UTF-8 written into the caller's buffer; on an error the
//! buffer receives `{"error":<code>,"detail":"…"}` when it fits. Trust follows
//! the build: with the `skin-test-keys` feature the published test root and
//! `test-` kids are accepted (Debug/Lab); without it every `test-` kid is
//! refused (release).
//!
//! # Examples
//!
//! ```
//! use meowpuut_ffi::skin::*;
//! # #[allow(unsafe_code)]
//! // SAFETY: valid pointers to live buffers of the stated sizes.
//! unsafe {
//!     let (mut secret, mut public) = ([0u8; 32], [0u8; 32]);
//!     assert_eq!(mps_request_key_generate(secret.as_mut_ptr(), public.as_mut_ptr()), MPS_OK);
//!     let mut id = [0u8; 128];
//!     let n = mps_redemption_id(public.as_ptr(), id.as_mut_ptr().cast(), id.len());
//!     assert!(n > 0 && id.starts_with(b"MPR2-"));
//!     secret.fill(0);
//! }
//! ```

use std::ffi::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

use meowpuut_skin::install::{self, publics_from_bytes, secrets_from_bytes};
use meowpuut_skin::request;
use meowpuut_skin::trust::Policy;
use meowpuut_skin::{limits, Code, Error, Zeroizing};

use crate::arg_str;

/// Success.
pub const MPS_OK: i32 = 0;

/// Recommended size of every JSON result buffer; always enough.
pub const MPS_RESULT_CAP: usize = 256 * 1024;

fn policy() -> Policy {
    Policy::from_build()
}

/// Writes `text` and a NUL into `out[..cap]`; false when it does not fit (then `out[0] = 0`).
///
/// # Safety
/// `out` must be NULL or valid for writes of `cap` bytes.
#[allow(unsafe_code)]
unsafe fn write_out(text: &str, out: *mut c_char, cap: usize) -> bool {
    if out.is_null() || cap == 0 {
        return false;
    }
    let bytes = text.as_bytes();
    if bytes.len() + 1 > cap || bytes.contains(&0) {
        // SAFETY: `out` is valid for `cap >= 1` bytes per the contract.
        unsafe { *out = 0 };
        return false;
    }
    // SAFETY: `out` is valid for `cap > bytes.len()` bytes and does not overlap `bytes` (a Rust string).
    let dst = unsafe { std::slice::from_raw_parts_mut(out.cast::<u8>(), cap) };
    dst[..bytes.len()].copy_from_slice(bytes);
    dst[bytes.len()] = 0;
    true
}

/// Turns a result into a code, writing the JSON (or the error) into `out`.
///
/// # Safety
/// As for [`write_out`].
#[allow(unsafe_code)]
unsafe fn finish(result: std::thread::Result<Result<String, Error>>, out: *mut c_char, cap: usize) -> i32 {
    let result = result.unwrap_or_else(|_| Err(Error::from(Code::Internal)));
    match result {
        Ok(text) => {
            // SAFETY: forwarded contract.
            if unsafe { write_out(&text, out, cap) } {
                MPS_OK
            } else {
                Code::Buffer.number()
            }
        }
        Err(e) => {
            let text = e.to_json();
            // SAFETY: forwarded contract.
            unsafe { write_out(&text, out, cap) };
            e.code.number()
        }
    }
}

/// Borrows `len` bytes; NULL is accepted only with `len == 0`.
///
/// # Safety
/// `ptr` must be NULL or valid for reads of `len` bytes for `'a`.
#[allow(unsafe_code)]
unsafe fn bytes<'a>(ptr: *const u8, len: usize) -> Option<&'a [u8]> {
    if ptr.is_null() {
        return (len == 0).then_some(&[]);
    }
    // SAFETY: non-null and valid for `len` bytes per the contract.
    Some(unsafe { std::slice::from_raw_parts(ptr, len) })
}

/// Largest `.mpskin` the platform layer may read (check before reading the file).
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn mps_skin_max_package_len() -> usize {
    limits::PACKAGE
}

/// 1 when this build accepts test keys (`skin-test-keys`), 0 in a release build.
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn mps_skin_accepts_test_keys() -> i32 {
    i32::from(policy().accepts_test_keys())
}

/// 1 when this build can trust skin packages, 0 when no signing root is configured.
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn mps_skin_has_trusted_roots() -> i32 {
    i32::from(policy().has_trusted_roots())
}

/// A new request key pair from the system random source.
///
/// # Safety
/// Both pointers must be valid for writes of 32 bytes.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mps_request_key_generate(secret_out: *mut u8, public_out: *mut u8) -> i32 {
    if secret_out.is_null() || public_out.is_null() {
        return Code::Argument.number();
    }
    let Ok(Ok((secret, public))) = catch_unwind(request::generate) else {
        return Code::Random.number();
    };
    // SAFETY: both pointers are valid for 32 bytes per the contract; the sources are local arrays.
    unsafe { std::ptr::copy_nonoverlapping(secret.as_ptr(), secret_out, 32) };
    // SAFETY: as above.
    unsafe { std::ptr::copy_nonoverlapping(public.as_ptr(), public_out, 32) };
    MPS_OK
}

/// The public key of a request secret.
///
/// # Safety
/// `secret` must be valid for reads and `public_out` for writes of 32 bytes.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mps_request_public_key(secret: *const u8, public_out: *mut u8) -> i32 {
    if secret.is_null() || public_out.is_null() {
        return Code::Argument.number();
    }
    let mut key = Zeroizing::new([0u8; 32]);
    // SAFETY: `secret` is valid for 32 bytes per the contract.
    unsafe { std::ptr::copy_nonoverlapping(secret, key.as_mut_ptr(), 32) };
    let public = request::public_key(&key);
    // SAFETY: `public_out` is valid for 32 bytes per the contract.
    unsafe { std::ptr::copy_nonoverlapping(public.as_ptr(), public_out, 32) };
    MPS_OK
}

/// The redemption ID text of a request public key. Returns its length
/// (without the NUL), or 0 when an argument is NULL or `cap` is too small (80 is enough).
///
/// # Safety
/// `public_key` must be valid for reads of 32 bytes; `out` for writes of `cap` bytes.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mps_redemption_id(public_key: *const u8, out: *mut c_char, cap: usize) -> usize {
    if public_key.is_null() {
        return 0;
    }
    let mut key = [0u8; 32];
    // SAFETY: valid for 32 bytes per the contract.
    unsafe { std::ptr::copy_nonoverlapping(public_key, key.as_mut_ptr(), 32) };
    let text = request::redemption_id(&key);
    // SAFETY: forwarded contract.
    if unsafe { write_out(&text, out, cap) } {
        text.len()
    } else {
        0
    }
}

/// Verifies, decrypts, validates, rewrites and installs a package.
///
/// # Safety
/// `package` valid for `package_len` bytes; `request_secrets` for
/// `secret_count * 32` bytes; strings NUL-terminated; `result_json` valid
/// for writes of `cap` bytes.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mps_skin_redeem(
    package: *const u8,
    package_len: usize,
    request_secrets: *const u8,
    secret_count: usize,
    state_json: *const c_char,
    skins_dir: *const c_char,
    client: *const c_char,
    result_json: *mut c_char,
    cap: usize,
) -> i32 {
    if package_len > limits::PACKAGE {
        // SAFETY: forwarded contract.
        return unsafe { finish(Ok(Err(Error::new(Code::TooLarge, "package"))), result_json, cap) };
    }
    // SAFETY: valid for `package_len` bytes per the contract.
    let package = unsafe { bytes(package, package_len) };
    // SAFETY: valid for `secret_count * 32` bytes per the contract.
    let secrets = unsafe { bytes(request_secrets, secret_count.saturating_mul(32)) };
    // SAFETY: NULL or NUL-terminated per the contract.
    let state = unsafe { arg_str(state_json) };
    // SAFETY: NULL or NUL-terminated per the contract.
    let dir = unsafe { arg_str(skins_dir) };
    // SAFETY: NULL or NUL-terminated per the contract.
    let client = unsafe { arg_str(client) };
    let (Some(package), Some(secrets), Some(dir), Some(client)) = (package, secrets, dir, client) else {
        // SAFETY: forwarded contract.
        return unsafe { finish(Ok(Err(Error::from(Code::Argument))), result_json, cap) };
    };
    let Some(secrets) = secrets_from_bytes(secrets) else {
        // SAFETY: forwarded contract.
        return unsafe { finish(Ok(Err(Error::new(Code::Argument, "request secrets"))), result_json, cap) };
    };
    let result = catch_unwind(AssertUnwindSafe(|| {
        install::redeem_json(package, &secrets, state.unwrap_or(""), Path::new(dir), client, &policy(), cap)
    }));
    // SAFETY: forwarded contract.
    unsafe { finish(result, result_json, cap) }
}

/// Verifies, validates, rewrites and installs a **bundled** package (`.mpskinb`)
/// read from the app bundle: signed by a `skin` subkey, not encrypted, not
/// device-bound. Same checks, layout, rollback guard and revocation checks as
/// `mps_skin_redeem`, without request keys. A recipient `.mpskin` is refused
/// with code 32 (`Kind`).
///
/// # Safety
/// `package` valid for `package_len` bytes; strings NUL-terminated;
/// `result_json` valid for writes of `cap` bytes.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mps_skin_install_bundled(
    package: *const u8,
    package_len: usize,
    state_json: *const c_char,
    skins_dir: *const c_char,
    client: *const c_char,
    result_json: *mut c_char,
    cap: usize,
) -> i32 {
    if package_len > limits::PACKAGE {
        // SAFETY: forwarded contract.
        return unsafe { finish(Ok(Err(Error::new(Code::TooLarge, "package"))), result_json, cap) };
    }
    // SAFETY: valid for `package_len` bytes per the contract.
    let package = unsafe { bytes(package, package_len) };
    // SAFETY: NULL or NUL-terminated per the contract.
    let state = unsafe { arg_str(state_json) };
    // SAFETY: NULL or NUL-terminated per the contract.
    let dir = unsafe { arg_str(skins_dir) };
    // SAFETY: NULL or NUL-terminated per the contract.
    let client = unsafe { arg_str(client) };
    let (Some(package), Some(dir), Some(client)) = (package, dir, client) else {
        // SAFETY: forwarded contract.
        return unsafe { finish(Ok(Err(Error::from(Code::Argument))), result_json, cap) };
    };
    let result = catch_unwind(AssertUnwindSafe(|| {
        install::install_bundled_json(package, state.unwrap_or(""), Path::new(dir), client, &policy(), cap)
    }));
    // SAFETY: forwarded contract.
    unsafe { finish(result, result_json, cap) }
}

/// Reads optional request public keys: NULL → `None` (binding check skipped).
///
/// # Safety
/// `ptr` NULL or valid for `count * 32` bytes.
#[allow(unsafe_code)]
unsafe fn publics(ptr: *const u8, count: usize) -> Result<Option<Vec<[u8; 32]>>, Error> {
    if ptr.is_null() {
        return Ok(None);
    }
    // SAFETY: per the contract.
    let raw = unsafe { bytes(ptr, count.saturating_mul(32)) }.ok_or_else(|| Error::from(Code::Argument))?;
    publics_from_bytes(raw).map(Some).ok_or_else(|| Error::new(Code::Argument, "request public keys"))
}

/// Verify-on-load of the skin named by the `customSkin` setting (`"<id>@<rev>"`).
///
/// # Safety
/// Strings NUL-terminated; `request_publics` NULL or valid for
/// `public_count * 32` bytes; `result_json` valid for writes of `cap` bytes.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mps_skin_verify_installed(
    skins_dir: *const c_char,
    custom_skin: *const c_char,
    state_json: *const c_char,
    request_publics: *const u8,
    public_count: usize,
    result_json: *mut c_char,
    cap: usize,
) -> i32 {
    // SAFETY: NULL or NUL-terminated per the contract.
    let dir = unsafe { arg_str(skins_dir) };
    // SAFETY: NULL or NUL-terminated per the contract.
    let skin = unsafe { arg_str(custom_skin) };
    // SAFETY: NULL or NUL-terminated per the contract.
    let state = unsafe { arg_str(state_json) };
    // SAFETY: per the contract.
    let keys = unsafe { publics(request_publics, public_count) };
    let result = catch_unwind(AssertUnwindSafe(|| {
        let (Some(dir), Some(skin)) = (dir, skin) else {
            return Err(Error::from(Code::Argument));
        };
        install::verify_json(Path::new(dir), skin, state.unwrap_or(""), &policy(), keys?.as_deref())
    }));
    // SAFETY: forwarded contract.
    unsafe { finish(result, result_json, cap) }
}

/// Every installed skin, each verified: `{"skins":[{…,"ok":true}|{"skin","ok":false,"error"}]}`.
///
/// # Safety
/// As for `mps_skin_verify_installed`.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mps_skin_list_installed(
    skins_dir: *const c_char,
    state_json: *const c_char,
    request_publics: *const u8,
    public_count: usize,
    result_json: *mut c_char,
    cap: usize,
) -> i32 {
    // SAFETY: NULL or NUL-terminated per the contract.
    let dir = unsafe { arg_str(skins_dir) };
    // SAFETY: NULL or NUL-terminated per the contract.
    let state = unsafe { arg_str(state_json) };
    // SAFETY: per the contract.
    let keys = unsafe { publics(request_publics, public_count) };
    let result = catch_unwind(AssertUnwindSafe(|| {
        let dir = dir.ok_or_else(|| Error::from(Code::Argument))?;
        install::list_json(Path::new(dir), state.unwrap_or(""), &policy(), keys?.as_deref())
    }));
    // SAFETY: forwarded contract.
    unsafe { finish(result, result_json, cap) }
}

/// Removes an installed skin's folder (the highest revision seen stays in the state).
///
/// # Safety
/// Strings NUL-terminated.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mps_skin_remove(skins_dir: *const c_char, custom_skin: *const c_char) -> i32 {
    // SAFETY: NULL or NUL-terminated per the contract.
    let dir = unsafe { arg_str(skins_dir) };
    // SAFETY: NULL or NUL-terminated per the contract.
    let skin = unsafe { arg_str(custom_skin) };
    let (Some(dir), Some(skin)) = (dir, skin) else {
        return Code::Argument.number();
    };
    match catch_unwind(|| install::remove(Path::new(dir), skin)) {
        Ok(Ok(())) => MPS_OK,
        Ok(Err(e)) => e.code.number(),
        Err(_) => Code::Internal.number(),
    }
}

/// Merges a root-signed `.mprevoke` into the state: `{"added":n,"state":{…}}`.
///
/// # Safety
/// `file` valid for `len` bytes; `state_json` NULL or NUL-terminated;
/// `out_json` valid for writes of `cap` bytes.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mps_revocation_merge(
    file: *const u8,
    len: usize,
    state_json: *const c_char,
    out_json: *mut c_char,
    cap: usize,
) -> i32 {
    if len > limits::REVOCATION_FILE {
        // SAFETY: forwarded contract.
        return unsafe { finish(Ok(Err(Error::new(Code::TooLarge, "revocation file"))), out_json, cap) };
    }
    // SAFETY: valid for `len` bytes per the contract.
    let data = unsafe { bytes(file, len) };
    // SAFETY: NULL or NUL-terminated per the contract.
    let state = unsafe { arg_str(state_json) };
    let result = catch_unwind(AssertUnwindSafe(|| {
        let data = data.ok_or_else(|| Error::from(Code::Argument))?;
        install::merge_revocation_json(data, state.unwrap_or(""), &policy())
    }));
    // SAFETY: forwarded contract.
    unsafe { finish(result, out_json, cap) }
}

/// A static English description of a code ("" for MPS_OK, "unknown error" otherwise).
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn mps_skin_error(code: i32) -> *const c_char {
    static TEXTS: std::sync::OnceLock<Vec<(i32, std::ffi::CString)>> = std::sync::OnceLock::new();
    if code == MPS_OK {
        return c"".as_ptr();
    }
    let texts = TEXTS.get_or_init(|| {
        Code::ALL.iter().map(|c| (c.number(), std::ffi::CString::new(c.message()).unwrap_or_default())).collect()
    });
    texts.iter().find(|(n, _)| *n == code).map(|(_, t)| t.as_ptr()).unwrap_or(c"unknown error".as_ptr())
}
