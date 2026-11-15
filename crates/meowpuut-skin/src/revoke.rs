//! Revocation (§6.3) and the device's monotonic state (§5.3).
//!
//! # `.mprevoke` (little endian)
//!
//! ```text
//! offset  size  field
//! 0       8     magic "MPREVK" 0x00 0x01
//! 8       4     body_len (1 ..= 131000)
//! 12      n     body: canonical JSON
//!               {"entries":[{"kind":"kid"|"skin"|"serial","value":"…"},…],"root":"<root kid>","seq":<u64>}
//! 12+n    64    Ed25519(root key, "meowpuut-revoke/v1\0" ‖ body), verify_strict
//! ```
//!
//! Only a root key signs a revocation file; a subkey cannot. `kid` values are
//! kids, `skin` values `"<id>@<rev>"`, `serial` values 16 lowercase hex digits
//! (the manifest's per-recipient serial).
//!
//! # Monotonic state
//!
//! Kept by the platform (Android: a Keystore-encrypted private file) and
//! handed to every call as JSON:
//!
//! ```text
//! {"highest":{"<id>":<rev>,…},"revoked":{"kid":[…],"serial":[…],"skin":[…]},"seq":<u64>,"v":1}
//! ```
//!
//! Every merge is a union: revocations are never removed, `highest` only
//! grows (removing a skin does not lower it), and `seq` is the largest
//! revocation sequence seen; a file with a larger `seq` and fewer entries
//! removes nothing. The built-in revocations ([`BUILTIN`]) are added to every
//! state that is read. No time is used for any decision.
//!
//! # Examples
//!
//! ```
//! use meowpuut_skin::revoke::State;
//! let state = State::parse("").unwrap();
//! assert!(state.highest.is_empty());
//! let json = state.to_json();
//! assert_eq!(State::parse(&json).unwrap(), state);
//! ```

use std::collections::{BTreeMap, BTreeSet};

use ed25519_dalek::Signature;
use serde_json::{json, Map, Value};

use crate::codec::{canonical_json, hex_array, parse_canonical};
use crate::names::{is_name, parse_skin_ref};
use crate::trust::{exact_keys, Policy};
use crate::{fail, limits, Code, Error, Result};

/// Magic of a `.mprevoke` file.
pub const MAGIC: [u8; 8] = *b"MPREVK\x00\x01";
/// Domain separation of a revocation signature.
pub const DOMAIN: &[u8] = b"meowpuut-revoke/v1\0";

/// Revocations shipped in the client: the lower bound of every device's set.
/// Filled from the private signer's `revoke` output.
pub struct Builtin {
    /// Revoked kids.
    pub kids: &'static [&'static str],
    /// Revoked `"<id>@<rev>"`.
    pub skins: &'static [&'static str],
    /// Revoked serials.
    pub serials: &'static [&'static str],
}

/// The built-in revocations (none yet).
pub const BUILTIN: Builtin = Builtin { kids: &[], skins: &[], serials: &[] };

/// A set of revocations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Revocations {
    /// Revoked kids (root or subkey).
    pub kids: BTreeSet<String>,
    /// Revoked `"<id>@<rev>"`.
    pub skins: BTreeSet<String>,
    /// Revoked serials (16 lowercase hex digits).
    pub serials: BTreeSet<String>,
}

impl Revocations {
    /// Number of entries.
    pub fn len(&self) -> usize {
        self.kids.len() + self.skins.len() + self.serials.len()
    }

    /// Whether there is no entry.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Adds one entry after checking its value. Returns whether it was new.
    pub fn insert(&mut self, kind: &str, value: &str) -> Result<bool> {
        let bad = || Error::new(Code::Revocation, format!("{kind} {value:?}"));
        Ok(match kind {
            "kid" if is_name(value) => self.kids.insert(value.to_string()),
            "skin" if parse_skin_ref(value).is_some() => self.skins.insert(value.to_string()),
            "serial" if hex_array::<8>(value).is_some() => self.serials.insert(value.to_string()),
            _ => return Err(bad()),
        })
    }

    /// Adds every entry of `other`. Returns how many were new.
    pub fn union(&mut self, other: &Revocations) -> usize {
        let before = self.len();
        self.kids.extend(other.kids.iter().cloned());
        self.skins.extend(other.skins.iter().cloned());
        self.serials.extend(other.serials.iter().cloned());
        self.len() - before
    }

    /// The built-in set.
    pub fn builtin() -> Self {
        let mut out = Self::default();
        out.kids.extend(BUILTIN.kids.iter().map(|s| s.to_string()));
        out.skins.extend(BUILTIN.skins.iter().map(|s| s.to_string()));
        out.serials.extend(BUILTIN.serials.iter().map(|s| s.to_string()));
        out
    }
}

/// The device's monotonic state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    /// Highest revision seen per skin ID; never lowered.
    pub highest: BTreeMap<String, u32>,
    /// Revocations; never removed.
    pub revoked: Revocations,
    /// Largest revocation file sequence merged.
    pub seq: u64,
}

impl State {
    /// Parses the platform's JSON (empty text: a new device) and adds the
    /// built-in revocations.
    pub fn parse(text: &str) -> Result<State> {
        let mut state = State::default();
        if !text.trim().is_empty() {
            if text.len() > limits::STATE_JSON {
                return fail(Code::TooLarge, "state");
            }
            let bad = |what: &str| Error::new(Code::State, what.to_string());
            let value: Value = serde_json::from_str(text).map_err(|_| bad("not JSON"))?;
            let map = value.as_object().ok_or_else(|| bad("not an object"))?;
            if !exact_keys(map, &["highest", "revoked", "seq", "v"]) || map["v"].as_u64() != Some(1) {
                return Err(bad("fields"));
            }
            state.seq = map["seq"].as_u64().ok_or_else(|| bad("seq"))?;
            for (id, rev) in map["highest"].as_object().ok_or_else(|| bad("highest"))? {
                let rev = rev.as_u64().and_then(|r| u32::try_from(r).ok()).filter(|r| *r >= 1);
                match rev {
                    Some(rev) if is_name(id) => state.highest.insert(id.clone(), rev),
                    _ => return Err(bad("highest entry")),
                };
            }
            let revoked = map["revoked"].as_object().ok_or_else(|| bad("revoked"))?;
            if !exact_keys(revoked, &["kid", "serial", "skin"]) {
                return Err(bad("revoked fields"));
            }
            for kind in ["kid", "serial", "skin"] {
                for item in revoked[kind].as_array().ok_or_else(|| bad("revoked list"))? {
                    let value = item.as_str().ok_or_else(|| bad("revoked value"))?;
                    state.revoked.insert(kind, value).map_err(|_| bad("revoked value"))?;
                }
            }
            if state.highest.len() > limits::HIGHEST || state.revoked.len() > limits::REVOCATIONS {
                return fail(Code::TooLarge, "state");
            }
        }
        state.revoked.union(&Revocations::builtin());
        Ok(state)
    }

    /// Canonical JSON of the state.
    pub fn to_value(&self) -> Value {
        let list = |set: &BTreeSet<String>| Value::Array(set.iter().cloned().map(Value::String).collect());
        let highest: Map<String, Value> = self.highest.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
        json!({
            "highest": highest,
            "revoked": {"kid": list(&self.revoked.kids), "serial": list(&self.revoked.serials), "skin": list(&self.revoked.skins)},
            "seq": self.seq,
            "v": 1,
        })
    }

    /// Canonical JSON text of the state.
    pub fn to_json(&self) -> String {
        String::from_utf8(canonical_json(&self.to_value())).unwrap_or_default()
    }

    /// The highest revision seen for `id` (0 when none).
    pub fn highest(&self, id: &str) -> u32 {
        self.highest.get(id).copied().unwrap_or(0)
    }

    /// Records a revision as seen; never lowers.
    pub fn see(&mut self, id: &str, rev: u32) -> Result<()> {
        let entry = self.highest.entry(id.to_string()).or_insert(0);
        *entry = (*entry).max(rev);
        if self.highest.len() > limits::HIGHEST {
            return fail(Code::TooLarge, "state");
        }
        Ok(())
    }

    /// Merges a verified revocation file: union, `seq` = max. Returns how many entries were new.
    pub fn merge(&mut self, file: &RevocationFile) -> Result<usize> {
        let added = self.revoked.union(&file.entries);
        if self.revoked.len() > limits::REVOCATIONS {
            return fail(Code::TooLarge, "revocation set");
        }
        self.seq = self.seq.max(file.seq);
        Ok(added)
    }
}

/// A verified `.mprevoke`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevocationFile {
    /// Sequence number (informational; merging is a union either way).
    pub seq: u64,
    /// The signing root's kid.
    pub root: String,
    /// The entries.
    pub entries: Revocations,
}

/// Parses and verifies a `.mprevoke`: size and structure first, then the
/// root (test kids refused unless the policy accepts them), then the strict
/// signature, then the entries.
pub fn parse_file(bytes: &[u8], policy: &Policy) -> Result<RevocationFile> {
    if bytes.len() > limits::REVOCATION_FILE {
        return fail(Code::TooLarge, "revocation file");
    }
    let bad = |what: &str| Error::new(Code::Revocation, what.to_string());
    if bytes.len() < 12 + 64 || bytes[..8] != MAGIC {
        return Err(bad("magic"));
    }
    let n = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
    if n == 0 || bytes.len() != 12 + n + 64 {
        return Err(bad("length"));
    }
    let body = &bytes[12..12 + n];
    let signature: [u8; 64] = bytes[12 + n..].try_into().map_err(|_| bad("signature"))?;
    let value = parse_canonical(body).ok_or_else(|| bad("not canonical JSON"))?;
    let map = value.as_object().ok_or_else(|| bad("not an object"))?;
    if !exact_keys(map, &["entries", "root", "seq"]) {
        return Err(bad("fields"));
    }
    let root_kid = map["root"].as_str().filter(|k| is_name(k)).ok_or_else(|| bad("root"))?;
    let seq = map["seq"].as_u64().ok_or_else(|| bad("seq"))?;
    let root = policy.root(root_kid).map_err(|e| match e.code {
        Code::TestKey => e,
        _ => bad("not signed by a root key"),
    })?;
    root.verify_strict(&[DOMAIN, body].concat(), &Signature::from_bytes(&signature))
        .map_err(|_| bad("signature"))?;
    let mut entries = Revocations::default();
    let list = map["entries"].as_array().ok_or_else(|| bad("entries"))?;
    if list.len() > limits::REVOCATIONS {
        return fail(Code::TooLarge, "revocation entries");
    }
    for entry in list {
        let e = entry.as_object().filter(|e| exact_keys(e, &["kind", "value"])).ok_or_else(|| bad("entry"))?;
        let (Some(kind), Some(value)) = (e["kind"].as_str(), e["value"].as_str()) else {
            return Err(bad("entry"));
        };
        entries.insert(kind, value)?;
    }
    Ok(RevocationFile { seq, root: root_kid.to_string(), entries })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trip_and_union() {
        let mut a = State::parse("").unwrap();
        a.see("sakura_tea", 3).unwrap();
        a.see("sakura_tea", 2).unwrap();
        assert_eq!(a.highest("sakura_tea"), 3);
        a.revoked.insert("kid", "2026a-skin").unwrap();
        a.revoked.insert("serial", "0011223344556677").unwrap();
        a.revoked.insert("skin", "sakura_tea@1").unwrap();
        let text = a.to_json();
        assert_eq!(State::parse(&text).unwrap(), a);
        assert!(a.revoked.insert("serial", "00112233445566").is_err());
        assert!(a.revoked.insert("skin", "x@0").is_err());
        assert!(a.revoked.insert("other", "x").is_err());
    }

    #[test]
    fn state_rejects_bad_json() {
        for bad in [
            "[]",
            "{}",
            r#"{"highest":{},"revoked":{"kid":[],"serial":[],"skin":[]},"seq":0,"v":2}"#,
            r#"{"highest":{"A":1},"revoked":{"kid":[],"serial":[],"skin":[]},"seq":0,"v":1}"#,
            r#"{"highest":{"a":0},"revoked":{"kid":[],"serial":[],"skin":[]},"seq":0,"v":1}"#,
            r#"{"highest":{"a":4294967296},"revoked":{"kid":[],"serial":[],"skin":[]},"seq":0,"v":1}"#,
            r#"{"highest":{},"revoked":{"kid":["../x"],"serial":[],"skin":[]},"seq":0,"v":1}"#,
            r#"{"highest":{},"revoked":{"kid":[],"serial":[],"skin":[]},"seq":-1,"v":1}"#,
            r#"{"highest":{},"revoked":{"kid":[],"serial":[],"skin":[]},"seq":0,"v":1,"x":0}"#,
        ] {
            assert_eq!(State::parse(bad).unwrap_err().code, Code::State, "{bad}");
        }
    }
}
