//! Redeeming, installing bundled skins, verifying on load, listing and removing (§5).
//!
//! # Installed folder (untrusted: verified on every load)
//!
//! ```text
//! <skins_dir>/<id>/<rev>/
//!   manifest.json  *.png  *.wav   the rewritten files, exactly the header's `files`
//!   seal.bin                      the package's magic, header, certificate and signature, unchanged
//!   binding.bin                   recipient kind only: canonical JSON of the header's `recipient`
//! ```
//!
//! Nothing else may be in the folder; a bundled skin's folder has no
//! `binding.bin`. The kind is read from `seal.bin`'s magic, whose signature
//! domain makes it unforgeable. [`verify_installed`] (before reading any
//! image or sound) re-parses `"<id>@<rev>"`, verifies the seal under the
//! build's policy, checks revocations and the highest revision, compares
//! every file's length and SHA-256, refuses extra or non-regular files, and
//! checks the manifest's serial against the revocations. For a recipient
//! skin it also compares `binding.bin` with the seal and checks that the
//! skin was issued to one of this device's request keys; a bundled skin is
//! not device-bound. `SEAL.md` specifies the same algorithm for a verifier
//! that does not link this crate (the iOS keyboard, CryptoKit).
//!
//! [`redeem`] (recipient packages) and [`install_bundled`] (bundled packages
//! read from the app bundle) run the whole package check (see the crate
//! documentation), write into a temporary folder inside `skins_dir`, verify
//! that folder exactly as a load would, and only then rename it into place.
//! A crash leaves at most a temporary folder that the next install removes.
//! Installing the same package again reinstalls it (idempotent); an older
//! revision than the highest seen, or than an installed one, is refused.
//! Both kinds share one ID namespace and one highest-revision table.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use zeroize::Zeroizing;

use crate::archive;
use crate::codec::{canonical_json, hex_array, hex_encode, sha256};
use crate::container::{open_seal, open_seal_of, parse_seal, parse_version, split, Header, Kind};
use crate::crypto::{open_payload, unwrap_cek};
use crate::manifest::{self, Rewritten};
use crate::names::{is_name, parse_rev, parse_skin_ref, skin_ref};
use crate::request::{public_key, recipient_tag};
use crate::revoke::{parse_file, State};
use crate::trust::Policy;
use crate::{fail, limits, Code, Error, Result};

/// Name of the sealed prefix in an installed folder.
pub const SEAL_FILE: &str = "seal.bin";
/// Name of the recipient record in an installed folder.
pub const BINDING_FILE: &str = "binding.bin";
const TEMP_PREFIX: &str = ".incoming-";
const OLD_PREFIX: &str = ".old-";

/// The client asking: platform and version (`"android/1.2"`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Client {
    /// `android`, `ios`, `macos` or `windows`.
    pub platform: String,
    /// `(major, minor)`.
    pub version: (u32, u32),
}

impl Client {
    /// Parses `"<platform>/<major>.<minor>"`.
    pub fn parse(text: &str) -> Result<Client> {
        let (platform, version) = text.split_once('/').ok_or_else(|| Error::new(Code::Argument, "client"))?;
        if !crate::container::PLATFORMS.contains(&platform) {
            return fail(Code::Argument, "client platform");
        }
        let version = parse_version(version).ok_or_else(|| Error::new(Code::Argument, "client version"))?;
        Ok(Client { platform: platform.to_string(), version })
    }
}

/// A package that passed every check, ready to install.
#[derive(Debug, Clone)]
pub struct Opened {
    /// The verified header.
    pub header: Header,
    /// `seal.bin`.
    pub seal: Vec<u8>,
    /// The rewritten files and what the validator read.
    pub skin: Rewritten,
    /// Which of the given request secrets the package was issued to
    /// (`None` for a bundled package).
    pub request_index: Option<usize>,
}

fn check_client(header: &Header, client: &Client) -> Result<()> {
    match header.min_client.get(&client.platform) {
        None => fail(Code::Client, format!("not made for {}", client.platform)),
        Some(min) if client.version < *min => fail(Code::Client, format!("needs {}.{}", min.0, min.1)),
        _ => Ok(()),
    }
}

/// Runs every check of a recipient package without touching the file system.
/// A bundled package is refused with [`Code::Kind`].
pub fn open_package(
    package: &[u8],
    request_secrets: &[[u8; 32]],
    state: &State,
    policy: &Policy,
    client: &Client,
) -> Result<Opened> {
    let (seal, payload) = split(package)?;
    let (header, _cert) = open_seal_of(Kind::Recipient, &seal, policy, &state.revoked)?;
    check_client(&header, client)?;
    if header.rev < state.highest(&header.id) {
        return fail(Code::Downgrade, format!("{} after rev {}", header.skin_ref(), state.highest(&header.id)));
    }
    if payload.len() != header.payload_len as usize {
        return fail(Code::Payload, "length");
    }
    if sha256(payload) != header.payload_sha256 {
        return fail(Code::Payload, "SHA-256");
    }
    let (to, nonce, aad) = match &header.encryption {
        Some(enc) => (enc.recipient.to, enc.nonce, enc.aad.clone()),
        None => return fail(Code::Kind, "a bundled header"),
    };
    let mut found = None;
    for (i, secret) in request_secrets.iter().enumerate() {
        let public = public_key(secret);
        if recipient_tag(&public) == to {
            found = Some((i, public));
            break;
        }
    }
    let (request_index, request_pub) = found.ok_or_else(|| Error::from(Code::NotForThisDevice))?;
    let cek = unwrap_cek(&header, &request_secrets[request_index], &request_pub)?;
    let plain = open_payload(&cek, &nonce, &aad, payload)?;
    let entries = archive::parse(&plain)?;
    let skin = manifest::validate(&entries, &header.id, header.rev)?;
    if state.revoked.serials.contains(&skin.serial) {
        return fail(Code::Revoked, "serial");
    }
    compare_files(&header, &skin.files)?;
    Ok(Opened { seal: package[..seal.len].to_vec(), header, skin, request_index: Some(request_index) })
}

/// Runs every check of a bundled package without touching the file system:
/// size caps → kind (magic) → certificate (root signature, `skin` use, not
/// revoked) → header signature under the bundled domain (`verify_strict`) →
/// header (`"kind":"bundled"`, kid, `id@rev` not revoked) → client →
/// highest revision → payload length and SHA-256 → MPAR → validator →
/// serial not revoked → rewrite compared with `files`. A recipient package is
/// refused with [`Code::Kind`].
pub fn open_bundled(package: &[u8], state: &State, policy: &Policy, client: &Client) -> Result<Opened> {
    let (seal, payload) = split(package)?;
    let (header, _cert) = open_seal_of(Kind::Bundled, &seal, policy, &state.revoked)?;
    check_client(&header, client)?;
    if header.rev < state.highest(&header.id) {
        return fail(Code::Downgrade, format!("{} after rev {}", header.skin_ref(), state.highest(&header.id)));
    }
    if payload.len() != header.payload_len as usize {
        return fail(Code::Payload, "length");
    }
    if sha256(payload) != header.payload_sha256 {
        return fail(Code::Payload, "SHA-256");
    }
    let entries = archive::parse(payload)?;
    let skin = manifest::validate(&entries, &header.id, header.rev)?;
    if state.revoked.serials.contains(&skin.serial) {
        return fail(Code::Revoked, "serial");
    }
    compare_files(&header, &skin.files)?;
    Ok(Opened { seal: package[..seal.len].to_vec(), header, skin, request_index: None })
}

fn compare_files(header: &Header, files: &[(String, Vec<u8>)]) -> Result<()> {
    if header.files.len() != files.len() {
        return fail(Code::Files, "file count");
    }
    for (signed, (name, data)) in header.files.iter().zip(files) {
        if signed.name != *name || signed.len as usize != data.len() || signed.sha256 != sha256(data) {
            return fail(Code::Files, name.clone());
        }
    }
    Ok(())
}

fn storage(e: std::io::Error) -> Error {
    Error::new(Code::Storage, e.to_string())
}

fn random_suffix() -> Result<String> {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).map_err(|_| Error::from(Code::Random))?;
    Ok(hex_encode(&bytes))
}

/// Reads a regular file (not a link) of at most `cap` bytes.
fn read_capped(path: &Path, cap: usize) -> Result<Vec<u8>> {
    let meta = fs::symlink_metadata(path).map_err(|_| Error::new(Code::Tampered, format!("missing {}", path.display())))?;
    if !meta.file_type().is_file() {
        return fail(Code::Tampered, format!("not a regular file: {}", path.display()));
    }
    if meta.len() > cap as u64 {
        return fail(Code::Tampered, format!("too long: {}", path.display()));
    }
    let mut data = Vec::with_capacity(meta.len() as usize);
    File::open(path).map_err(storage)?.take(cap as u64 + 1).read_to_end(&mut data).map_err(storage)?;
    if data.len() > cap {
        return fail(Code::Tampered, format!("too long: {}", path.display()));
    }
    Ok(data)
}

/// Whether `path` is a real directory (not a link).
fn is_real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).map(|m| m.file_type().is_dir()).unwrap_or(false)
}

/// A verified installed skin.
#[derive(Debug, Clone)]
pub struct Installed {
    /// The verified header.
    pub header: Header,
    /// The folder.
    pub dir: PathBuf,
    /// `base` of the manifest.
    pub base: String,
    /// Display names.
    pub names: BTreeMap<String, String>,
}

impl Installed {
    fn to_value(&self) -> Value {
        json!({
            "base": self.base,
            "dir": self.dir.to_string_lossy(),
            // The signed list the folder was just checked against: a platform
            // that reads the files afterwards hashes the bytes it keeps against
            // this, so it never uses bytes other than the verified ones.
            "files": self.header.files.iter()
                .map(|f| json!({"len": f.len, "n": f.name, "sha256": hex_encode(&f.sha256)}))
                .collect::<Vec<_>>(),
            "id": self.header.id,
            "kind": self.header.kind.name(),
            "names": self.names,
            "rev": self.header.rev,
            "skin": self.header.skin_ref(),
        })
    }
}

/// Verifies one installed folder for `id@rev` of either kind (see the module
/// documentation). `request_publics` (recipient skins only; ignored for a
/// bundled skin): `None` skips the device binding check (only for a platform
/// that cannot read its request keys); `Some(&[])` means the device has
/// none, so the check fails.
pub fn verify_dir(
    dir: &Path,
    id: &str,
    rev: u32,
    state: &State,
    policy: &Policy,
    request_publics: Option<&[[u8; 32]]>,
) -> Result<Installed> {
    if !is_real_dir(dir) {
        return fail(Code::NotInstalled, skin_ref(id, rev));
    }
    let seal_bytes = read_capped(&dir.join(SEAL_FILE), limits::SEAL)?;
    let seal = parse_seal(&seal_bytes)?;
    let (header, _cert) = open_seal(&seal, policy, &state.revoked)?;
    if header.id != id || header.rev != rev {
        return fail(Code::Tampered, "the seal is for another skin");
    }
    if rev < state.highest(id) {
        return fail(Code::Downgrade, skin_ref(id, rev));
    }
    let bound = header.encryption.is_some();
    let expected = header.files.len() + 1 + usize::from(bound);
    let mut seen = 0usize;
    for entry in fs::read_dir(dir).map_err(storage)? {
        let entry = entry.map_err(storage)?;
        seen += 1;
        let name = entry.file_name();
        let name = name.to_str().unwrap_or("");
        let known = name == SEAL_FILE || (bound && name == BINDING_FILE) || header.files.iter().any(|f| f.name == name);
        if !known || !entry.file_type().map_err(storage)?.is_file() {
            return fail(Code::Tampered, format!("unexpected entry {name:?}"));
        }
        if seen > expected {
            return fail(Code::Tampered, "extra files");
        }
    }
    if seen != expected {
        return fail(Code::Tampered, "missing files");
    }
    let mut manifest_bytes = Vec::new();
    for file in &header.files {
        let data = read_capped(&dir.join(&file.name), file.len as usize)?;
        if data.len() != file.len as usize || sha256(&data) != file.sha256 {
            return fail(Code::Tampered, file.name.clone());
        }
        if file.name == "manifest.json" {
            manifest_bytes = data;
        }
    }
    if let Some(enc) = &header.encryption {
        let binding = read_capped(&dir.join(BINDING_FILE), 1024)?;
        if binding != enc.recipient_json {
            return fail(Code::Tampered, BINDING_FILE);
        }
        if let Some(publics) = request_publics {
            if !publics.iter().any(|p| recipient_tag(p) == enc.recipient.to) {
                return fail(Code::Binding, skin_ref(id, rev));
            }
        }
    }
    // The manifest is signed (its hash is in the header); read what the platform needs.
    let manifest: Value = serde_json::from_slice(&manifest_bytes).map_err(|_| Error::from(Code::Tampered))?;
    let serial = manifest.get("serial").and_then(Value::as_str).unwrap_or("");
    if state.revoked.serials.contains(serial) {
        return fail(Code::Revoked, "serial");
    }
    let base = manifest.get("base").and_then(Value::as_str).unwrap_or("current").to_string();
    let names = manifest
        .get("names")
        .and_then(Value::as_object)
        .map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect())
        .unwrap_or_default();
    Ok(Installed { header, dir: dir.to_path_buf(), base, names })
}

/// Verifies the installed skin named by the platform's `customSkin` setting.
pub fn verify_installed(
    skins_dir: &Path,
    custom_skin: &str,
    state: &State,
    policy: &Policy,
    request_publics: Option<&[[u8; 32]]>,
) -> Result<Installed> {
    let (id, rev) = parse_skin_ref(custom_skin).ok_or_else(|| Error::new(Code::Argument, "customSkin"))?;
    verify_dir(&skins_dir.join(&id).join(rev.to_string()), &id, rev, state, policy, request_publics)
}

/// Installed skin IDs (folders with a valid name), sorted.
fn installed_ids(skins_dir: &Path) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    let Ok(listing) = fs::read_dir(skins_dir) else {
        return Ok(ids);
    };
    for entry in listing {
        let entry = entry.map_err(storage)?;
        if let Some(name) = entry.file_name().to_str() {
            if is_name(name) && entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                ids.push(name.to_string());
            }
        }
    }
    ids.sort();
    Ok(ids)
}

/// Revision folders under one skin ID, sorted.
fn installed_revs(id_dir: &Path) -> Vec<u32> {
    let mut revs: Vec<u32> = fs::read_dir(id_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| e.file_name().to_str().and_then(parse_rev))
        .collect();
    revs.sort_unstable();
    revs
}

/// The result of a redeem.
#[derive(Debug, Clone)]
pub struct Redeemed {
    /// The installed skin.
    pub installed: Installed,
    /// The new monotonic state to store.
    pub state: State,
    /// Validator warnings.
    pub warnings: Vec<String>,
}

impl Redeemed {
    /// The C ABI's result JSON.
    pub fn to_json(&self) -> String {
        let mut value = self.installed.to_value();
        if let Some(map) = value.as_object_mut() {
            map.insert("state".into(), self.state.to_value());
            map.insert("warnings".into(), json!(self.warnings));
        }
        String::from_utf8(canonical_json(&value)).unwrap_or_default()
    }
}

/// Removes leftovers of an interrupted redeem.
fn clean_temporaries(skins_dir: &Path) {
    if let Ok(listing) = fs::read_dir(skins_dir) {
        for entry in listing.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(TEMP_PREFIX) || name.starts_with(OLD_PREFIX) {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
    }
}

fn write_file(path: &Path, data: &[u8]) -> Result<()> {
    let mut file = File::create(path).map_err(storage)?;
    file.write_all(data).map_err(storage)?;
    file.sync_all().map_err(storage)
}

/// Redeems a recipient package (see the module documentation). `result_cap`
/// is the caller's buffer size: when the result JSON (including its NUL)
/// would not fit, nothing is installed and [`Code::Buffer`] is returned.
pub fn redeem(
    package: &[u8],
    request_secrets: &[[u8; 32]],
    state: &State,
    skins_dir: &Path,
    policy: &Policy,
    client: &Client,
    result_cap: usize,
) -> Result<Redeemed> {
    let opened = open_package(package, request_secrets, state, policy, client)?;
    let publics: Vec<[u8; 32]> = request_secrets.iter().map(public_key).collect();
    install_opened(&opened, state, skins_dir, policy, Some(&publics), result_cap)
}

/// Installs a bundled package read from the app bundle (see the module
/// documentation): the same checks, rewrite, layout (no `binding.bin`),
/// rollback guard and revocation checks as [`redeem`], without any request
/// key. `result_cap` as for [`redeem`].
pub fn install_bundled(
    package: &[u8],
    state: &State,
    skins_dir: &Path,
    policy: &Policy,
    client: &Client,
    result_cap: usize,
) -> Result<Redeemed> {
    let opened = open_bundled(package, state, policy, client)?;
    install_opened(&opened, state, skins_dir, policy, None, result_cap)
}

/// Writes a checked package into place: temporary folder, verify-on-load of
/// that folder, atomic rename, older revisions removed.
fn install_opened(
    opened: &Opened,
    state: &State,
    skins_dir: &Path,
    policy: &Policy,
    publics: Option<&[[u8; 32]]>,
    result_cap: usize,
) -> Result<Redeemed> {
    let (id, rev) = (opened.header.id.clone(), opened.header.rev);
    fs::create_dir_all(skins_dir).map_err(storage)?;
    clean_temporaries(skins_dir);
    let id_dir = skins_dir.join(&id);
    if id_dir.exists() && !is_real_dir(&id_dir) {
        return fail(Code::Storage, "skin folder is not a directory");
    }
    // An installed newer revision only ever refuses more (it may be planted).
    if installed_revs(&id_dir).last().is_some_and(|newest| *newest > rev) {
        return fail(Code::Downgrade, format!("rev {rev} below an installed revision"));
    }
    let others = installed_ids(skins_dir)?.into_iter().filter(|other| *other != id).count();
    if others >= limits::INSTALLED {
        return fail(Code::TooManySkins, format!("{others} installed"));
    }

    let temp = skins_dir.join(format!("{TEMP_PREFIX}{}", random_suffix()?));
    let result = (|| {
        fs::create_dir(&temp).map_err(storage)?;
        for (name, data) in &opened.skin.files {
            write_file(&temp.join(name), data)?;
        }
        write_file(&temp.join(SEAL_FILE), &opened.seal)?;
        if let Some(enc) = &opened.header.encryption {
            write_file(&temp.join(BINDING_FILE), &enc.recipient_json)?;
        }
        let mut installed = verify_dir(&temp, &id, rev, state, policy, publics)?;
        if installed.header.kind != opened.header.kind {
            return fail(Code::Internal, "kind changed while installing");
        }
        let mut new_state = state.clone();
        new_state.see(&id, rev)?;
        let final_dir = id_dir.join(rev.to_string());
        installed.dir = final_dir.clone();
        let redeemed = Redeemed { installed, state: new_state, warnings: opened.skin.warnings.clone() };
        if redeemed.to_json().len() + 1 > result_cap {
            return fail(Code::Buffer, "result");
        }
        fs::create_dir_all(&id_dir).map_err(storage)?;
        if fs::symlink_metadata(&final_dir).is_ok() {
            let old = skins_dir.join(format!("{OLD_PREFIX}{}", random_suffix()?));
            fs::rename(&final_dir, &old).map_err(storage)?;
            fs::rename(&temp, &final_dir).map_err(storage)?;
            let _ = fs::remove_dir_all(&old);
        } else {
            fs::rename(&temp, &final_dir).map_err(storage)?;
        }
        if let Ok(d) = File::open(&id_dir) {
            let _ = d.sync_all();
        }
        for old_rev in installed_revs(&id_dir) {
            if old_rev != rev {
                let _ = fs::remove_dir_all(id_dir.join(old_rev.to_string()));
            }
        }
        Ok(redeemed)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&temp);
    }
    result
}

/// One row of [`list_installed`].
#[derive(Debug, Clone)]
pub enum Listed {
    /// Verified.
    Ok(Box<Installed>),
    /// Present but refused, with the reason.
    Refused(String, Code),
}

/// Every installed skin folder, verified (at most 64 rows).
pub fn list_installed(skins_dir: &Path, state: &State, policy: &Policy, request_publics: Option<&[[u8; 32]]>) -> Result<Vec<Listed>> {
    let mut out = Vec::new();
    for id in installed_ids(skins_dir)? {
        for rev in installed_revs(&skins_dir.join(&id)) {
            if out.len() >= 64 {
                return Ok(out);
            }
            let dir = skins_dir.join(&id).join(rev.to_string());
            out.push(match verify_dir(&dir, &id, rev, state, policy, request_publics) {
                Ok(installed) => Listed::Ok(Box::new(installed)),
                Err(e) => Listed::Refused(skin_ref(&id, rev), e.code),
            });
        }
    }
    Ok(out)
}

/// Removes an installed skin's folder. The highest revision seen is kept in
/// the state, so the removed revision or an older one cannot come back.
pub fn remove(skins_dir: &Path, custom_skin: &str) -> Result<()> {
    let (id, rev) = parse_skin_ref(custom_skin).ok_or_else(|| Error::new(Code::Argument, "customSkin"))?;
    let id_dir = skins_dir.join(&id);
    let dir = id_dir.join(rev.to_string());
    match fs::symlink_metadata(&dir) {
        Err(_) => return fail(Code::NotInstalled, custom_skin),
        Ok(m) if m.file_type().is_dir() => fs::remove_dir_all(&dir).map_err(storage)?,
        Ok(_) => fs::remove_file(&dir).map_err(storage)?,
    }
    if fs::read_dir(&id_dir).map(|mut d| d.next().is_none()).unwrap_or(false) {
        let _ = fs::remove_dir(&id_dir);
    }
    Ok(())
}

// ---- JSON entry points of the C ABI ----

/// Redeem with JSON in and out (the C ABI's `mps_skin_redeem`).
#[allow(clippy::too_many_arguments)]
pub fn redeem_json(
    package: &[u8],
    request_secrets: &[[u8; 32]],
    state_json: &str,
    skins_dir: &Path,
    client: &str,
    policy: &Policy,
    result_cap: usize,
) -> Result<String> {
    let state = State::parse(state_json)?;
    let client = Client::parse(client)?;
    redeem(package, request_secrets, &state, skins_dir, policy, &client, result_cap).map(|r| r.to_json())
}

/// Install-bundled with JSON in and out (the C ABI's `mps_skin_install_bundled`).
pub fn install_bundled_json(
    package: &[u8],
    state_json: &str,
    skins_dir: &Path,
    client: &str,
    policy: &Policy,
    result_cap: usize,
) -> Result<String> {
    let state = State::parse(state_json)?;
    let client = Client::parse(client)?;
    install_bundled(package, &state, skins_dir, policy, &client, result_cap).map(|r| r.to_json())
}

/// Verify-on-load with JSON out (`mps_skin_verify_installed`).
pub fn verify_json(
    skins_dir: &Path,
    custom_skin: &str,
    state_json: &str,
    policy: &Policy,
    request_publics: Option<&[[u8; 32]]>,
) -> Result<String> {
    let state = State::parse(state_json)?;
    let installed = verify_installed(skins_dir, custom_skin, &state, policy, request_publics)?;
    Ok(String::from_utf8(canonical_json(&installed.to_value())).unwrap_or_default())
}

/// List with JSON out (`mps_skin_list_installed`).
pub fn list_json(skins_dir: &Path, state_json: &str, policy: &Policy, request_publics: Option<&[[u8; 32]]>) -> Result<String> {
    let state = State::parse(state_json)?;
    let rows: Vec<Value> = list_installed(skins_dir, &state, policy, request_publics)?
        .into_iter()
        .map(|row| match row {
            Listed::Ok(installed) => {
                let mut v = installed.to_value();
                if let Some(m) = v.as_object_mut() {
                    m.insert("ok".into(), json!(true));
                }
                v
            }
            Listed::Refused(skin, code) => json!({"error": code.number(), "ok": false, "skin": skin}),
        })
        .collect();
    Ok(String::from_utf8(canonical_json(&json!({ "skins": rows }))).unwrap_or_default())
}

/// Merges a `.mprevoke` into the state (`mps_revocation_merge`): `{"added":n,"state":{…}}`.
pub fn merge_revocation_json(file: &[u8], state_json: &str, policy: &Policy) -> Result<String> {
    let mut state = State::parse(state_json)?;
    let parsed = parse_file(file, policy)?;
    let added = state.merge(&parsed)?;
    Ok(String::from_utf8(canonical_json(&json!({"added": added, "state": state.to_value()}))).unwrap_or_default())
}

/// Request secrets from a flat byte buffer of 32-byte keys.
pub fn secrets_from_bytes(bytes: &[u8]) -> Option<Zeroizing<Vec<[u8; 32]>>> {
    if !bytes.len().is_multiple_of(32) || bytes.len() / 32 > 256 {
        return None;
    }
    Some(Zeroizing::new(bytes.chunks_exact(32).map(|c| c.try_into().unwrap_or([0u8; 32])).collect()))
}

/// Public keys from a flat byte buffer of 32-byte keys.
pub fn publics_from_bytes(bytes: &[u8]) -> Option<Vec<[u8; 32]>> {
    if !bytes.len().is_multiple_of(32) || bytes.len() / 32 > 256 {
        return None;
    }
    Some(bytes.chunks_exact(32).map(|c| c.try_into().unwrap_or([0u8; 32])).collect())
}

/// The serial of an installed skin's manifest (developer tools, tests).
pub fn serial_of(manifest_json: &[u8]) -> Option<String> {
    let v: Value = serde_json::from_slice(manifest_json).ok()?;
    let s = v.get("serial")?.as_str()?;
    hex_array::<8>(s).map(|_| s.to_string())
}
