//! Keys (§6.1, §9.5): offline root keys certify purpose-tagged subkeys.
//!
//! A client carries only root public keys. A subkey certificate travels in
//! every package and every `seal.bin`:
//!
//! ```text
//! cert JSON (canonical): {"issued":<u32>,"kid":"<kid>","pub":"<b64 32 bytes>","root":"<root kid>","uses":["code","skin"]}
//! root signature:        Ed25519(root, "meowpuut-cert/v2\0" ‖ cert JSON), verify_strict
//! ```
//!
//! `uses` is a non-empty, sorted, duplicate-free subset of `code` and `skin`.
//! A package header must be signed by a subkey with the `skin` use. No time
//! is used for any trust decision: a subkey stops being trusted only when
//! its kid is revoked.
//!
//! **Test keys.** Every test kid starts with `test-`. A [`Policy`] that does
//! not accept test keys (the default of every build without the `test-keys`
//! feature, see [`Policy::from_build`]) refuses any `test-` kid: as a root,
//! as a subkey and as a revocation signer. A test root may certify only
//! `test-` subkeys and a release root only non-test subkeys. Building with
//! the `test-keys` feature while `MEOWPUUT_RELEASE_BUILD` is set in the
//! environment fails to compile.
//!
//! # Examples
//!
//! ```
//! use meowpuut_skin::trust::*;
//! let release = Policy::release();
//! assert!(!release.accepts_test_keys());
//! assert!(release.check_kid("test-2026-skin").is_err());
//! assert!(Policy::with_test_keys().root("test-root-1").is_ok());
//! ```

use ed25519_dalek::{Signature, VerifyingKey};
use serde_json::Value;

use crate::codec::{b64_array, parse_canonical};
use crate::names::is_name;
use crate::revoke::Revocations;
use crate::{fail, limits, Code, Error, Result};

/// Prefix of every test kid.
pub const TEST_PREFIX: &str = "test-";

/// Domain separation of a subkey certificate signature.
pub const CERT_DOMAIN: &[u8] = b"meowpuut-cert/v2\0";

/// A root public key built into the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RootKey {
    /// The root's kid (`root-1`).
    pub kid: &'static str,
    /// Ed25519 public key.
    pub public: [u8; 32],
}

/// The release root keys. **Empty until the developer generates the real root
/// key** with the private signer (`meowpuut-sign root-init`) and pastes its
/// output here. Never a `test-` kid: the assertion below stops the build.
pub const RELEASE_ROOTS: &[RootKey] = &[];

const fn has_test_kid(roots: &[RootKey]) -> bool {
    let mut i = 0;
    while i < roots.len() {
        let kid = roots[i].kid.as_bytes();
        let p = TEST_PREFIX.as_bytes();
        if kid.len() >= p.len() {
            let mut j = 0;
            let mut same = true;
            while j < p.len() {
                if kid[j] != p[j] {
                    same = false;
                }
                j += 1;
            }
            if same {
                return true;
            }
        }
        i += 1;
    }
    false
}

const _: () = assert!(!has_test_kid(RELEASE_ROOTS), "a release root key has a test- kid");

#[cfg(feature = "test-keys")]
const _: () = assert!(
    option_env!("MEOWPUUT_RELEASE_BUILD").is_none(),
    "meowpuut-skin: the `test-keys` feature must not be enabled in a release build (MEOWPUUT_RELEASE_BUILD is set)"
);

/// The published test root. Its private key is derived from a public seed
/// (`Tests/skin-vectors/README.md`), so anyone can sign with it: it is
/// trusted only by [`Policy::with_test_keys`].
pub const TEST_ROOT: RootKey = RootKey {
    kid: "test-root-1",
    public: [
        0x6d, 0x76, 0xfc, 0xd9, 0x1d, 0x46, 0xf4, 0x1e, 0x46, 0x73, 0xcd, 0x41, 0x41, 0x0a, 0x65, 0xe3, 0x97, 0x34, 0x48,
        0xdd, 0x5f, 0xc5, 0x3f, 0x9b, 0x12, 0xe7, 0x0b, 0x6f, 0x43, 0xdb, 0x6d, 0xbb,
    ],
};

/// Whether `kid` is a test kid.
pub fn is_test_kid(kid: &str) -> bool {
    kid.starts_with(TEST_PREFIX)
}

/// Which roots are trusted and whether test kids are accepted.
#[derive(Debug, Clone)]
pub struct Policy {
    roots: Vec<(String, VerifyingKey)>,
    accept_test_keys: bool,
}

impl Policy {
    /// Release: [`RELEASE_ROOTS`] only; every `test-` kid is refused.
    pub fn release() -> Self {
        Self::custom(&RELEASE_ROOTS.iter().map(|r| (r.kid, r.public)).collect::<Vec<_>>(), false)
            .unwrap_or(Self { roots: Vec::new(), accept_test_keys: false })
    }

    /// Debug/Lab: the release roots plus [`TEST_ROOT`]; `test-` kids accepted.
    pub fn with_test_keys() -> Self {
        let mut roots: Vec<(&str, [u8; 32])> = RELEASE_ROOTS.iter().map(|r| (r.kid, r.public)).collect();
        roots.push((TEST_ROOT.kid, TEST_ROOT.public));
        Self::custom(&roots, true).unwrap_or(Self { roots: Vec::new(), accept_test_keys: true })
    }

    /// The policy of this build: [`Policy::with_test_keys`] with the
    /// `test-keys` feature, otherwise [`Policy::release`]. The C ABI uses only this.
    pub fn from_build() -> Self {
        if cfg!(feature = "test-keys") {
            Self::with_test_keys()
        } else {
            Self::release()
        }
    }

    /// Explicit roots. Refuses a `test-` root when test keys are not accepted,
    /// a malformed kid, a duplicate kid and a weak or invalid key.
    pub fn custom(roots: &[(&str, [u8; 32])], accept_test_keys: bool) -> Result<Self> {
        let mut out: Vec<(String, VerifyingKey)> = Vec::new();
        for &(kid, public) in roots {
            if !is_name(kid) || out.iter().any(|(k, _)| k == kid) {
                return fail(Code::Certificate, format!("root kid {kid:?}"));
            }
            if is_test_kid(kid) && !accept_test_keys {
                return fail(Code::TestKey, format!("root {kid}"));
            }
            let key = VerifyingKey::from_bytes(&public).map_err(|_| Error::new(Code::Certificate, "root key"))?;
            if key.is_weak() {
                return fail(Code::Certificate, "weak root key");
            }
            out.push((kid.to_string(), key));
        }
        Ok(Self { roots: out, accept_test_keys })
    }

    /// Whether this policy accepts `test-` kids.
    pub fn accepts_test_keys(&self) -> bool {
        self.accept_test_keys
    }

    /// Whether the policy trusts any signing root.
    pub fn has_trusted_roots(&self) -> bool {
        !self.roots.is_empty()
    }

    /// Refuses a `test-` kid unless test keys are accepted.
    pub fn check_kid(&self, kid: &str) -> Result<()> {
        if is_test_kid(kid) && !self.accept_test_keys {
            return fail(Code::TestKey, format!("kid {kid}"));
        }
        Ok(())
    }

    /// The trusted root with this kid.
    pub fn root(&self, kid: &str) -> Result<&VerifyingKey> {
        self.check_kid(kid)?;
        self.roots
            .iter()
            .find(|(k, _)| k == kid)
            .map(|(_, key)| key)
            .ok_or_else(|| Error::new(Code::Certificate, format!("unknown root {kid}")))
    }
}

/// Uses a subkey can be certified for.
pub const USES: [&str; 2] = ["code", "skin"];

/// A verified subkey certificate.
#[derive(Debug, Clone)]
pub struct Cert {
    /// The subkey's kid.
    pub kid: String,
    /// The subkey.
    pub key: VerifyingKey,
    /// Sorted uses.
    pub uses: Vec<String>,
    /// When it was issued (Unix seconds; informational only).
    pub issued: u32,
    /// The certifying root's kid.
    pub root: String,
}

impl Cert {
    /// Whether the subkey may be used for `purpose`.
    pub fn allows(&self, purpose: &str) -> bool {
        self.uses.iter().any(|u| u == purpose)
    }
}

/// Reads a u32 field of a JSON object.
pub(crate) fn u32_field(map: &serde_json::Map<String, Value>, key: &str) -> Option<u32> {
    map.get(key)?.as_u64().and_then(|v| u32::try_from(v).ok())
}

/// Checks that an object has exactly these keys.
pub(crate) fn exact_keys(map: &serde_json::Map<String, Value>, keys: &[&str]) -> bool {
    map.len() == keys.len() && keys.iter().all(|k| map.contains_key(*k))
}

/// Verifies a subkey certificate: canonical JSON, a known root under the
/// policy, the root's strict signature, the test/release pairing, and that
/// neither kid is revoked.
pub fn verify_cert(json: &[u8], signature: &[u8; 64], policy: &Policy, revoked: &Revocations) -> Result<Cert> {
    if json.len() > limits::CERT_JSON {
        return fail(Code::TooLarge, "certificate");
    }
    let bad = |what: &str| Error::new(Code::Certificate, what.to_string());
    let value = parse_canonical(json).ok_or_else(|| bad("not canonical JSON"))?;
    let map = value.as_object().ok_or_else(|| bad("not an object"))?;
    if !exact_keys(map, &["issued", "kid", "pub", "root", "uses"]) {
        return Err(bad("fields"));
    }
    let kid = map["kid"].as_str().filter(|k| is_name(k)).ok_or_else(|| bad("kid"))?;
    let root_kid = map["root"].as_str().filter(|k| is_name(k)).ok_or_else(|| bad("root"))?;
    let issued = u32_field(map, "issued").ok_or_else(|| bad("issued"))?;
    let public: [u8; 32] = map["pub"].as_str().and_then(b64_array).ok_or_else(|| bad("pub"))?;
    let uses: Vec<String> = map["uses"]
        .as_array()
        .ok_or_else(|| bad("uses"))?
        .iter()
        .map(|u| u.as_str().filter(|u| USES.contains(u)).map(str::to_string))
        .collect::<Option<_>>()
        .ok_or_else(|| bad("uses"))?;
    if uses.is_empty() || uses.windows(2).any(|w| w[0] >= w[1]) {
        return Err(bad("uses must be sorted and unique"));
    }
    // Test kids before the signature: a release build says why it refuses.
    policy.check_kid(root_kid)?;
    policy.check_kid(kid)?;
    if is_test_kid(root_kid) != is_test_kid(kid) {
        return Err(bad("a test root certifies only test kids, a release root only release kids"));
    }
    let root = policy.root(root_kid)?;
    let message = [CERT_DOMAIN, json].concat();
    root.verify_strict(&message, &Signature::from_bytes(signature))
        .map_err(|_| Error::new(Code::Certificate, "root signature"))?;
    if revoked.kids.contains(root_kid) || revoked.kids.contains(kid) {
        return fail(Code::Revoked, format!("kid {kid}"));
    }
    let key = VerifyingKey::from_bytes(&public).map_err(|_| bad("subkey"))?;
    if key.is_weak() {
        return Err(bad("weak subkey"));
    }
    Ok(Cert { kid: kid.to_string(), key, uses, issued, root: root_kid.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    #[test]
    fn test_root_matches_its_published_seed() {
        let seed = crate::codec::sha256(b"meowpuut-skin test key test-root-1");
        let key = SigningKey::from_bytes(&seed);
        assert_eq!(key.verifying_key().to_bytes(), TEST_ROOT.public);
    }

    #[test]
    fn release_refuses_test_roots() {
        assert_eq!(Policy::custom(&[("test-x", TEST_ROOT.public)], false).unwrap_err().code, Code::TestKey);
        assert!(Policy::custom(&[("test-x", TEST_ROOT.public)], true).is_ok());
        assert_eq!(Policy::custom(&[("Root", TEST_ROOT.public)], true).unwrap_err().code, Code::Certificate);
        assert_eq!(Policy::release().root("test-root-1").unwrap_err().code, Code::TestKey);
        assert!(!has_test_kid(RELEASE_ROOTS));
        assert!(has_test_kid(&[TEST_ROOT]));
    }

    #[cfg(not(feature = "test-keys"))]
    #[test]
    fn this_build_refuses_test_kids() {
        let policy = Policy::from_build();
        assert!(!policy.accepts_test_keys());
        assert_eq!(policy.check_kid("test-2026-skin").unwrap_err().code, Code::TestKey);
    }
}
