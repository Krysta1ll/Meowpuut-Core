#![forbid(unsafe_code)]
//! Word-pair table (`mimo_pairs.bin`, written by `scripts/mimo_pairs.py`),
//! read in place: how much more or less likely a word is after the word
//! before it, counted in Wikipedia and in spoken text.
//!
//! Layout (little-endian):
//!
//! | offset | field |
//! |---|---|
//! | 0 | magic `MIMO-PAIR-1\0` |
//! | 12, 16 | number of word ids `n`, number of pairs `p` |
//! | 24 | `u64` fingerprint of the lexicon's word list ([`WordIds`]) |
//! | 64 | offsets: `(n + 1) x u32`; the followers of word `g` are `pairs[offsets[g]..offsets[g + 1]]` |
//! | | backoff: `n x i16`, the score of a pair that is not stored |
//! | | pairs, 4-byte aligned: `p x u32`, follower id `<< 12` and a 12-bit signed score, sorted by follower id |
//!
//! Scores are in steps of 1/64 nat and already weighted (0.8 times the log
//! ratio of the pair estimate to the single-word estimate). Word id 0 is the
//! clause start and 1 the clause end, so the start and end terms are pairs
//! like any other.
//!
//! # Examples
//!
//! ```
//! use meowpuut_core::lexicon::WordIds;
//! use meowpuut_core::pairs::{Pairs, END, START};
//!
//! // 4 ids (start, end, two words); start -> 2 scores 1.0 nat, 2 -> end -0.5 nat.
//! let ids = WordIds { count: 4, fingerprint: 7, ..Default::default() };
//! let mut file = b"MIMO-PAIR-1\0".to_vec();
//! for v in [4u32, 2, 0] { file.extend(v.to_le_bytes()); }
//! file.extend(7u64.to_le_bytes());
//! file.resize(64, 0);
//! for v in [0u32, 1, 1, 2, 2] { file.extend(v.to_le_bytes()); }
//! for v in [0i16, 0, -20, 0] { file.extend(v.to_le_bytes()); }
//! for (follower, score) in [(2u32, 64i32), (END, -32)] { file.extend(((follower << 12) | (score as u32 & 0xFFF)).to_le_bytes()); }
//! let pairs = Pairs::load(&file, &ids).unwrap();
//! assert_eq!(pairs.score(START, 2), 64);
//! assert_eq!(pairs.score(2, END), -32);
//! assert_eq!(pairs.score(2, 3), -20);
//! assert!(Pairs::load(&file, &WordIds { count: 4, fingerprint: 8, ..Default::default() }).is_none());
//! ```

use crate::image::{Bytes, I16Col, ImageOwner, U32Col};
use crate::lexicon::WordIds;

/// Magic header of `mimo_pairs.bin` (12 bytes).
pub const MAGIC: &[u8; 12] = b"MIMO-PAIR-1\0";
/// Word id of the clause start.
pub const START: u32 = 0;
/// Word id of the clause end.
pub const END: u32 = 1;
/// Stored scores per nat.
pub const STEP: f64 = 64.0;

const HEADER_LEN: usize = 64;

/// A pair table that belongs to one lexicon (same word list).
#[derive(Debug, Clone)]
pub struct Pairs {
    offsets: U32Col,
    backoff: I16Col,
    pairs: U32Col,
    _owner: Option<ImageOwner>,
}

impl Pairs {
    /// Copies a table to the heap. `None` when the bytes are not a table for
    /// the lexicon whose word ids are `ids`.
    pub fn load(bytes: &[u8], ids: &WordIds) -> Option<Self> {
        Self::parse(bytes, ids, &|b| Bytes::owned(b.to_vec()), None)
    }

    /// Reads a table in place; `owner` is kept until the last clone is dropped.
    pub fn in_place(bytes: &'static [u8], ids: &WordIds, owner: Option<ImageOwner>) -> Option<Self> {
        Self::parse(bytes, ids, &Bytes::mapped, owner)
    }

    fn parse<'a>(bytes: &'a [u8], ids: &WordIds, col: &dyn Fn(&'a [u8]) -> Bytes, owner: Option<ImageOwner>) -> Option<Self> {
        let field = |at: usize| -> Option<u64> {
            let mut arr = [0u8; 8];
            arr.copy_from_slice(bytes.get(at..at + 8)?);
            Some(u64::from_le_bytes(arr))
        };
        if bytes.len() < HEADER_LEN || &bytes[..12] != MAGIC {
            return None;
        }
        let n = (field(12)? & 0xFFFF_FFFF) as usize;
        let p = (field(16)? & 0xFFFF_FFFF) as usize;
        if ids.count == 0 || n != ids.count as usize || field(24)? != ids.fingerprint {
            return None;
        }
        let backoff_at = HEADER_LEN + (n + 1) * 4;
        let pairs_at = (backoff_at + n * 2).div_ceil(4) * 4;
        if bytes.len() != pairs_at.checked_add(p.checked_mul(4)?)? {
            return None;
        }
        let offsets = U32Col::new(col(&bytes[HEADER_LEN..backoff_at]));
        if !rows_in_order(&offsets, p) {
            return None;
        }
        Some(Self {
            offsets,
            backoff: I16Col::new(col(&bytes[backoff_at..backoff_at + n * 2])),
            pairs: U32Col::new(col(&bytes[pairs_at..])),
            _owner: owner,
        })
    }

    /// Score of word `right` after word `left`, in 1/64 nat. Only the header,
    /// the length and the row offsets are checked at load, so a damaged row
    /// gives wrong scores here, never a read outside the table.
    #[inline]
    pub fn score(&self, left: u32, right: u32) -> i32 {
        let g = left as usize;
        if g >= self.backoff.len() {
            return 0;
        }
        let (mut lo, mut hi) = (self.offsets.get(g) as usize, (self.offsets.get(g + 1) as usize).min(self.pairs.len()));
        while lo < hi {
            let mid = (lo + hi) / 2;
            let pair = self.pairs.get(mid);
            match (pair >> 12).cmp(&right) {
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
                std::cmp::Ordering::Equal => return ((pair << 20) as i32) >> 20,
            }
        }
        i32::from(self.backoff.get(g))
    }

    /// Heap bytes held (0 when read in place).
    pub fn heap_bytes(&self) -> usize {
        self.offsets.heap_bytes() + self.backoff.heap_bytes() + self.pairs.heap_bytes()
    }
}

/// Whether the row offsets never decrease and the last one is `p` (one pass
/// at attach; refuses a file whose offsets were damaged).
fn rows_in_order(offsets: &U32Col, p: usize) -> bool {
    let mut prev = 0u32;
    for g in 0..offsets.len() {
        let o = offsets.get(g);
        if o < prev {
            return false;
        }
        prev = o;
    }
    prev as usize == p
}
