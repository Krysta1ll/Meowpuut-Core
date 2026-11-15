//! The plaintext archive MPAR (§3.3), little endian:
//!
//! ```text
//! "MPAR" u16 version = 1, u16 count (1 ..= 48)
//! per entry: u8 name_len, name, u32 len (≤ 1 MiB), 32 bytes SHA-256 of the data, data
//! ```
//!
//! Names follow `names::file_name` (no slash, no capital, no device name),
//! are strictly ascending (so unique) and include `manifest.json`; only
//! `manifest.json` may be JSON. Every length field is read and summed, and
//! the sum compared with the input, before any entry is used; the files
//! together are at most 4 MiB.
//!
//! # Examples
//!
//! ```
//! use meowpuut_skin::archive::parse;
//! assert!(parse(b"MPAR\x01\x00\x00\x00").is_err());
//! ```

use crate::codec::sha256;
use crate::names::{file_name, FileKind};
use crate::{fail, limits, Code, Error, Result};

/// Magic of an archive.
pub const MAGIC: &[u8; 4] = b"MPAR";
/// The only version.
pub const VERSION: u16 = 1;

/// One file in an archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry<'a> {
    /// File name.
    pub name: &'a str,
    /// Data.
    pub data: &'a [u8],
}

fn bad(what: &str) -> Error {
    Error::new(Code::Archive, what.to_string())
}

/// Parses an archive; see the module documentation for every rule.
pub fn parse(bytes: &[u8]) -> Result<Vec<Entry<'_>>> {
    if bytes.len() > limits::ARCHIVE {
        return fail(Code::TooLarge, "archive");
    }
    if bytes.len() < 8 || &bytes[..4] != MAGIC {
        return Err(bad("magic"));
    }
    if u16::from_le_bytes([bytes[4], bytes[5]]) != VERSION {
        return Err(bad("version"));
    }
    let count = usize::from(u16::from_le_bytes([bytes[6], bytes[7]]));
    if count == 0 || count > limits::FILE_COUNT {
        return Err(bad("count"));
    }
    // Pass 1: lengths only.
    let mut spans = Vec::with_capacity(count);
    let mut at = 8usize;
    let mut total = 0usize;
    for _ in 0..count {
        let name_len = usize::from(*bytes.get(at).ok_or_else(|| bad("truncated"))?);
        let len_at = at + 1 + name_len;
        let len_bytes = bytes.get(len_at..len_at + 4).ok_or_else(|| bad("truncated"))?;
        let len = u32::from_le_bytes([len_bytes[0], len_bytes[1], len_bytes[2], len_bytes[3]]) as usize;
        if len > limits::FILE {
            return fail(Code::TooLarge, "archive entry");
        }
        total += len;
        if total > limits::FILES_TOTAL {
            return fail(Code::TooLarge, "archive files");
        }
        let data_at = len_at + 4 + 32;
        spans.push((at + 1, name_len, len_at + 4, data_at, len));
        at = data_at.checked_add(len).ok_or_else(|| bad("length"))?;
        if at > bytes.len() {
            return Err(bad("truncated"));
        }
    }
    if at != bytes.len() {
        return Err(bad("bytes after the last entry"));
    }
    // Pass 2: names, order and hashes.
    let mut out: Vec<Entry<'_>> = Vec::with_capacity(count);
    for (name_at, name_len, sha_at, data_at, len) in spans {
        let name = std::str::from_utf8(&bytes[name_at..name_at + name_len]).map_err(|_| bad("name"))?;
        let Some((_, kind)) = file_name(name) else {
            return Err(Error::new(Code::Archive, format!("file name {name:?}")));
        };
        if kind == FileKind::Json && name != "manifest.json" {
            return Err(bad("only manifest.json may be JSON"));
        }
        if out.last().is_some_and(|last| last.name >= name) {
            return Err(bad("names must be sorted and unique"));
        }
        let data = &bytes[data_at..data_at + len];
        if sha256(data)[..] != bytes[sha_at..sha_at + 32] {
            return Err(Error::new(Code::Archive, format!("{name}: SHA-256")));
        }
        out.push(Entry { name, data });
    }
    if !out.iter().any(|e| e.name == "manifest.json") {
        return Err(bad("no manifest.json"));
    }
    Ok(out)
}
