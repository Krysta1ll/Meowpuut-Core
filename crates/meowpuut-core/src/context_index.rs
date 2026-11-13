#![forbid(unsafe_code)]
//! Open-addressing exact-match hash index over byte-string rows.
//!
//! Used for the context (n-gram) table probed by `Lexicon::join_bonus` and for
//! exact reading-bucket lookups in `Lexicon::range`, both of which sit in the
//! innermost decoder loop. A sorted binary search costs ~18 string compares per
//! probe; this index answers with one hash, one 16-bit tag compare, and (only on
//! a tag hit) one byte compare. Lookups are exact, so results are identical to
//! the binary search they replace. Ordered prefix scans keep using the sorted
//! tables.
//!
//! The index is stored in `mimo_base.bin` version 2 so that the lexicon can be
//! read in place without building it on the heap. Binary search over the
//! sorted columns would have kept the file at its old size, but measured on
//! the parity set (2,980 items, 31,278 keystrokes, Mac, 2026-11-11) it made
//! a keystroke 47 % slower at the median and 90 % slower at the 95th
//! percentile (all modes: 178 us / 1027 us against 121 us / 540 us).
//!
//! # Examples
//!
//! ```
//! use meowpuut_core::context_index::PhraseIndex;
//!
//! let rows = ["你好", "世界", "喜歡"];
//! let key = |i: usize| rows[i].as_bytes();
//! let index = PhraseIndex::build(rows.len(), key);
//! assert_eq!(index.get("世界".as_bytes(), key), Some(1));
//! assert_eq!(index.get("喜歡".as_bytes(), key), Some(2));
//! assert_eq!(index.get("再見".as_bytes(), key), None);
//! assert_eq!(PhraseIndex::default().get(b"x", key), None);
//! assert!(index.heap_bytes() >= 16 * 6);
//! let (slots, tags) = index.columns();
//! let again = PhraseIndex::from_columns(rows.len(), slots.clone(), tags.clone(), index.probe()).unwrap();
//! assert_eq!(again.get("世界".as_bytes(), key), Some(1));
//! assert!(PhraseIndex::from_columns(rows.len(), slots.clone(), tags.clone(), 256).is_none());
//! ```

use crate::image::{Bytes, U16Col, U32Col};

const K: u64 = 0x9E37_79B9_7F4A_7C15;

/// Longest probe distance a prebuilt table may declare. At load factor 0.5
/// the shipped tables need under 64; scripts/mimo_binary.py refuses to write more.
pub const MAX_PROBE: usize = 255;

/// Fast 64-bit hash for short UTF-8 phrases (8-byte chunks, multiply-rotate mix).
#[inline]
pub fn phrase_hash(bytes: &[u8]) -> u64 {
    let mut h = (bytes.len() as u64).wrapping_mul(K);
    let mut chunks = bytes.chunks_exact(8);
    for chunk in &mut chunks {
        let mut word = [0u8; 8];
        word.copy_from_slice(chunk);
        h = (h ^ u64::from_le_bytes(word)).wrapping_mul(K).rotate_left(29);
    }
    let rest = chunks.remainder();
    if !rest.is_empty() {
        let mut word = [0u8; 8];
        word[..rest.len()].copy_from_slice(rest);
        h = (h ^ u64::from_le_bytes(word)).wrapping_mul(K).rotate_left(29);
    }
    h ^= h >> 32;
    h = h.wrapping_mul(0xD6E8_FEB8_6659_FD93);
    h ^ (h >> 32)
}

/// Hash index from row bytes to row number.
///
/// SoA layout: `slots` holds `row + 1` (0 = empty) and `tags` holds the high
/// 16 bits of the hash, so a failed probe never touches row storage. The two
/// columns are built on the heap or read in place from a `MIMO-IDX-1` section
/// (see `bin_format`); both give the same answers. `probe` is the longest
/// distance of a row from its home slot; a lookup never looks further, so a
/// damaged table cannot make it scan the whole table.
#[derive(Debug, Clone, Default)]
pub struct PhraseIndex {
    mask: usize,
    rows: usize,
    probe: usize,
    slots: U32Col,
    tags: U16Col,
}

impl PhraseIndex {
    /// Number of slots for `len` rows (load factor <= 0.5, power of two, at least 16).
    pub fn capacity_for(len: usize) -> usize {
        if len == 0 { 0 } else { (len * 2).next_power_of_two().max(16) }
    }

    /// Builds an index with load factor <= 0.5 over rows `0..len`.
    /// Rows are expected to be unique (the lexicon loader enforces this).
    pub fn build<'a>(len: usize, key: impl Fn(usize) -> &'a [u8]) -> Self {
        if len == 0 {
            return Self::default();
        }
        let capacity = Self::capacity_for(len);
        let mask = capacity - 1;
        let mut slots = vec![0u32; capacity];
        let mut tags = vec![0u16; capacity];
        let mut probe = 0usize;
        for row in 0..len {
            let h = phrase_hash(key(row));
            let mut pos = (h as usize) & mask;
            let mut distance = 0usize;
            while slots[pos] != 0 {
                pos = (pos + 1) & mask;
                distance += 1;
            }
            probe = probe.max(distance);
            slots[pos] = row as u32 + 1;
            tags[pos] = (h >> 48) as u16;
        }
        let slot_bytes: Vec<u8> = slots.iter().flat_map(|v| v.to_le_bytes()).collect();
        let tag_bytes: Vec<u8> = tags.iter().flat_map(|v| v.to_le_bytes()).collect();
        Self { mask, rows: len, probe, slots: U32Col::new(Bytes::owned(slot_bytes)), tags: U16Col::new(Bytes::owned(tag_bytes)) }
    }

    /// An index over prebuilt columns for `rows` rows with the longest probe
    /// distance `probe`. `None` unless both columns have `capacity_for(rows)`
    /// entries and `probe` is at most [`MAX_PROBE`].
    pub fn from_columns(rows: usize, slots: U32Col, tags: U16Col, probe: usize) -> Option<Self> {
        let capacity = Self::capacity_for(rows);
        if slots.len() != capacity || tags.len() != capacity {
            return None;
        }
        if rows == 0 {
            return Some(Self::default());
        }
        if probe > MAX_PROBE {
            return None;
        }
        Some(Self { mask: capacity - 1, rows, probe, slots, tags })
    }

    /// Longest distance of a row from its home slot.
    pub fn probe(&self) -> usize {
        self.probe
    }

    /// Returns the row whose bytes equal `phrase`, if any.
    #[inline]
    pub fn get<'a>(&self, phrase: &[u8], key: impl Fn(usize) -> &'a [u8]) -> Option<usize> {
        if self.rows == 0 {
            return None;
        }
        let h = phrase_hash(phrase);
        let tag = (h >> 48) as u16;
        let mut pos = (h as usize) & self.mask;
        for _ in 0..=self.probe {
            let slot = self.slots.get(pos);
            if slot == 0 {
                return None;
            }
            if self.tags.get(pos) == tag {
                let row = slot as usize - 1;
                if row < self.rows && key(row) == phrase {
                    return Some(row);
                }
            }
            pos = (pos + 1) & self.mask;
        }
        None
    }

    /// The slot and tag columns (for writing them into an image).
    pub fn columns(&self) -> (&U32Col, &U16Col) {
        (&self.slots, &self.tags)
    }

    /// Heap bytes held by the index (0 when read in place).
    pub fn heap_bytes(&self) -> usize {
        self.slots.heap_bytes() + self.tags.heap_bytes()
    }
}
