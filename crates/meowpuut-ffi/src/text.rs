#![forbid(unsafe_code)]
//! NUL-terminated string mirrors handed out through the C ABI.
//!
//! Buffers keep their capacity across updates, so steady-state typing does not
//! allocate. Pointers stay valid until the next update of the same buffer.
//!
//! # Examples
//!
//! ```
//! use meowpuut_ffi::text::{CArena, CBuf};
//! let mut buf = CBuf::with_capacity(8);
//! buf.set("你好");
//! assert_eq!(buf.as_cstr().to_str(), Ok("你好"));
//! let mut arena = CArena::with_capacity(2, 16);
//! arena.push("甲");
//! arena.push("乙\0丙");
//! assert_eq!(arena.len(), 2);
//! assert_eq!(arena.get(1).to_str(), Ok("乙"));
//! assert_eq!(arena.get(9).to_str(), Ok(""));
//! arena.clear();
//! assert!(arena.is_empty());
//! ```

use std::ffi::CStr;

/// One NUL-terminated UTF-8 string.
#[derive(Debug, Clone)]
pub struct CBuf {
    bytes: Vec<u8>,
}

/// Truncates at an interior NUL so C sees a well-formed string.
#[inline]
fn c_bytes(text: &str) -> &[u8] {
    let bytes = text.as_bytes();
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    &bytes[..end]
}

impl CBuf {
    /// Empty string with reserved capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        let mut bytes = Vec::with_capacity(capacity.max(1));
        bytes.push(0);
        Self { bytes }
    }

    /// Replaces the contents.
    pub fn set(&mut self, text: &str) {
        self.bytes.clear();
        self.bytes.extend_from_slice(c_bytes(text));
        self.bytes.push(0);
    }

    /// Borrowed C string.
    pub fn as_cstr(&self) -> &CStr {
        CStr::from_bytes_until_nul(&self.bytes).unwrap_or_default()
    }
}

/// Many NUL-terminated strings in one contiguous arena (SoA offsets).
#[derive(Debug, Clone)]
pub struct CArena {
    bytes: Vec<u8>,
    offsets: Vec<usize>,
}

impl CArena {
    /// Empty arena with reserved capacity.
    pub fn with_capacity(items: usize, bytes: usize) -> Self {
        Self { bytes: Vec::with_capacity(bytes), offsets: Vec::with_capacity(items) }
    }

    /// Removes all strings, keeping capacity.
    pub fn clear(&mut self) {
        self.bytes.clear();
        self.offsets.clear();
    }

    /// Appends one string.
    pub fn push(&mut self, text: &str) {
        self.offsets.push(self.bytes.len());
        self.bytes.extend_from_slice(c_bytes(text));
        self.bytes.push(0);
    }

    /// Number of strings.
    pub fn len(&self) -> usize {
        self.offsets.len()
    }

    /// Whether the arena is empty.
    pub fn is_empty(&self) -> bool {
        self.offsets.is_empty()
    }

    /// String `index`, or `""` when out of range.
    pub fn get(&self, index: usize) -> &CStr {
        match self.offsets.get(index) {
            Some(&start) => CStr::from_bytes_until_nul(&self.bytes[start..]).unwrap_or_default(),
            None => c"",
        }
    }
}
