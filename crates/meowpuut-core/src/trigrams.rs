//! Binary bridge-trigram and skip-1 bigram context table (`MIMO-TRI-1\0`).
//!
//! Stores SoA packed `u64` entries read either in place (`heap_bytes() == 0`) or
//! copied into owned buffers, and scores sliding 3-word windows in zero heap
//! allocations during candidate path reranking.
//!
//! ```rust
//! use meowpuut_core::image::{Bytes, U32Col};
//! use meowpuut_core::lexicon::WordIds;
//! use meowpuut_core::trigrams::{pack_image, Trigrams};
//!
//! let mut raw = Vec::with_capacity(32);
//! for id in 0..8u32 { raw.extend_from_slice(&id.to_le_bytes()); }
//! let ids = WordIds { count: 8, fingerprint: 0x1234_5678, ids: U32Col::new(Bytes::owned(raw)) };
//! let bytes = pack_image(&ids, &[3], &[(2, 0, 4, 128)], &[(2, 5, 64)], 256, 256).unwrap();
//! let tri = Trigrams::load(&bytes, &ids).unwrap();
//! assert!((tri.score_path(&[2, 3, 4]) - 2.0).abs() < 1e-9);
//! assert!((tri.score_path(&[2, 6, 5]) - 1.0).abs() < 1e-9);
//! ```

use std::cmp::Ordering;

use crate::image::{Bytes, ImageOwner, U64Col};
use crate::lexicon::WordIds;

/// 12-byte magic identifying a `MIMO-TRI-1` binary image.
const MAGIC: &[u8; 12] = b"MIMO-TRI-1\0\0";
const VERSION: u32 = 1;
const HEADER_BYTES: usize = 64;
/// Maximum number of inline bridge words (` 的 / 了 / 在 / ... `).
pub const MAX_BRIDGES: usize = 64;
const DATA_OFFSET: usize = HEADER_BYTES + MAX_BRIDGES * 4;
const MAX_WORD_ID_COUNT: u32 = 1 << 20;
const SCORE_MASK: u64 = 0xFFF;
const SCORE_DIVISOR: f64 = 64.0 * 256.0;
const MIN_SCORE_Q6: i32 = -2048;
const MAX_SCORE_Q6: i32 = 2047;

/// Immutable bridge-trigram and skip-1 bigram scoring table (`MIMO-TRI-1\0`).
#[derive(Debug, Clone)]
pub struct Trigrams {
    word_count: u32,
    bridge_count: usize,
    bridge_ids: [u32; MAX_BRIDGES],
    tri_weight_q8: i32,
    skip_weight_q8: i32,
    tri_entries: U64Col,
    skip_entries: U64Col,
    _owner: Option<ImageOwner>,
}

impl Trigrams {
    /// Loads a `MIMO-TRI-1` image by copying its packed arrays onto the heap.
    pub fn load(bytes: &[u8], ids: &WordIds) -> Option<Self> {
        Self::parse(bytes, ids, &|b| Bytes::owned(b.to_vec()), None)
    }

    /// Reads a `MIMO-TRI-1` image directly from caller-owned `'static` bytes (`heap_bytes() == 0`).
    pub fn in_place(bytes: &'static [u8], ids: &WordIds, owner: Option<ImageOwner>) -> Option<Self> {
        Self::parse(bytes, ids, &Bytes::mapped, owner)
    }

    fn parse<'a>(bytes: &'a [u8], ids: &WordIds, col: &dyn Fn(&'a [u8]) -> Bytes, owner: Option<ImageOwner>) -> Option<Self> {
        if bytes.len() < DATA_OFFSET || &bytes[0..12] != MAGIC {
            return None;
        }
        let version = read_u32(bytes, 12)?;
        let word_count = read_u32(bytes, 16)?;
        let bridge_count = read_u32(bytes, 20)? as usize;
        let fingerprint = read_u64(bytes, 24)?;
        let tri_count = read_u32(bytes, 32)? as usize;
        let skip_count = read_u32(bytes, 36)? as usize;
        let tri_weight_q8 = read_i32(bytes, 40)?;
        let skip_weight_q8 = read_i32(bytes, 44)?;

        if version != VERSION
            || ids.count == 0
            || word_count == 0
            || word_count > MAX_WORD_ID_COUNT
            || word_count != ids.count
            || fingerprint != ids.fingerprint
            || bridge_count > MAX_BRIDGES
        {
            return None;
        }
        let tri_end = DATA_OFFSET.checked_add(tri_count.checked_mul(8)?)?;
        if bytes.len() != tri_end.checked_add(skip_count.checked_mul(8)?)? {
            return None;
        }
        let mut bridge_ids = [0u32; MAX_BRIDGES];
        for i in 0..bridge_count {
            let id = read_u32(bytes, HEADER_BYTES + i * 4)?;
            if id >= word_count || bridge_ids[..i].contains(&id) {
                return None;
            }
            bridge_ids[i] = id;
        }
        let tri_entries = U64Col::new(col(&bytes[DATA_OFFSET..tri_end]));
        let skip_entries = U64Col::new(col(&bytes[tri_end..]));
        if !validate_entries(&tri_entries, word_count, Some(bridge_count))
            || !validate_entries(&skip_entries, word_count, None)
        {
            return None;
        }
        Some(Self {
            word_count,
            bridge_count,
            bridge_ids,
            tri_weight_q8,
            skip_weight_q8,
            tri_entries,
            skip_entries,
            _owner: owner,
        })
    }

    /// Number of bridge words registered in this table.
    #[inline]
    pub fn bridge_count(&self) -> usize {
        self.bridge_count
    }

    /// Total number of packed bridge-trigram entries.
    #[inline]
    pub fn tri_len(&self) -> usize {
        self.tri_entries.len()
    }

    /// Total number of packed skip-1 bigram entries.
    #[inline]
    pub fn skip_len(&self) -> usize {
        self.skip_entries.len()
    }

    /// Current Q8 runtime weights `(tri_weight_q8, skip_weight_q8)`.
    #[inline]
    pub fn weights_q8(&self) -> (i32, i32) {
        (self.tri_weight_q8, self.skip_weight_q8)
    }

    /// Overrides runtime Q8 weights (mainly for sweep tests).
    #[inline]
    pub fn set_weights_q8(&mut self, tri_weight_q8: i32, skip_weight_q8: i32) {
        self.tri_weight_q8 = tri_weight_q8;
        self.skip_weight_q8 = skip_weight_q8;
    }

    /// Returns the compact bridge slot `[0, bridge_count)` if `word_id` is a bridge word.
    #[inline]
    pub fn bridge_index(&self, word_id: u32) -> Option<u32> {
        (0..self.bridge_count).find(|&i| self.bridge_ids[i] == word_id).map(|i| i as u32)
    }

    /// Looks up a raw Q6 score for `(w1, bridge_idx, w3)` in `tri_entries`.
    #[inline]
    pub fn score_trigram(&self, w1: u32, bridge_idx: u32, w3: u32) -> Option<i32> {
        lookup_key(&self.tri_entries, tri_key(w1, bridge_idx, w3)?)
    }

    /// Looks up a raw Q6 score for `(w1, w3)` in `skip_entries`.
    #[inline]
    pub fn score_skip1(&self, w1: u32, w3: u32) -> Option<i32> {
        lookup_key(&self.skip_entries, skip_key(w1, w3)?)
    }

    /// Evaluates one 3-word window `(w1, w2, w3)` returning `(tri_q6_delta, skip_q6_delta)`.
    ///
    /// Bridge-specific trigram takes precedence over generic skip-1 bigram to avoid double counting.
    #[inline]
    pub fn score_window(&self, w1: u32, w2: u32, w3: u32) -> (i32, i32) {
        if w1 >= self.word_count || w2 >= self.word_count || w3 >= self.word_count {
            return (0, 0);
        }
        if let Some(b_idx) = self.bridge_index(w2) {
            if let Some(tri) = self.score_trigram(w1, b_idx, w3) {
                return (tri, 0);
            }
        }
        if let Some(skip) = self.score_skip1(w1, w3) {
            return (0, skip);
        }
        (0, 0)
    }

    /// Converts accumulated integer Q6 sums into a log-probability bonus in nats.
    #[inline]
    pub fn scale_sums(&self, tri_sum: i64, skip_sum: i64) -> f64 {
        (tri_sum as f64) * (f64::from(self.tri_weight_q8) / SCORE_DIVISOR)
            + (skip_sum as f64) * (f64::from(self.skip_weight_q8) / SCORE_DIVISOR)
    }

    /// Scores a complete word-id path without allocating heap memory.
    #[inline]
    pub fn score_path(&self, ids: &[u32]) -> f64 {
        let (mut tri_sum, mut skip_sum) = (0i64, 0i64);
        for win in ids.windows(3) {
            let (dt, ds) = self.score_window(win[0], win[1], win[2]);
            tri_sum += i64::from(dt);
            skip_sum += i64::from(ds);
        }
        self.scale_sums(tri_sum, skip_sum)
    }

    /// Owned heap bytes; returns `0` when loaded in place.
    #[inline]
    pub fn heap_bytes(&self) -> usize {
        self.tri_entries.heap_bytes() + self.skip_entries.heap_bytes()
    }
}

fn validate_entries(col: &U64Col, word_count: u32, bridge_limit: Option<usize>) -> bool {
    let mut prev_key: Option<u64> = None;
    for i in 0..col.len() {
        let key = col.get(i) >> 12;
        let ok = match bridge_limit {
            Some(lim) => {
                (key >> 48) == 0
                    && (((key >> 28) & 0xF_FFFF) as u32) < word_count
                    && (((key >> 20) & 0xFF) as usize) < lim
                    && ((key & 0xF_FFFF) as u32) < word_count
            }
            None => {
                (key >> 40) == 0
                    && (((key >> 20) & 0xF_FFFF) as u32) < word_count
                    && ((key & 0xF_FFFF) as u32) < word_count
            }
        };
        if !ok || prev_key.is_some_and(|p| key <= p) {
            return false;
        }
        prev_key = Some(key);
    }
    true
}

#[inline]
const fn tri_key(w1: u32, bridge_idx: u32, w3: u32) -> Option<u64> {
    if w1 >= MAX_WORD_ID_COUNT || w3 >= MAX_WORD_ID_COUNT || bridge_idx >= MAX_BRIDGES as u32 {
        return None;
    }
    Some(((w1 as u64) << 28) | ((bridge_idx as u64) << 20) | (w3 as u64))
}

#[inline]
const fn skip_key(w1: u32, w3: u32) -> Option<u64> {
    if w1 >= MAX_WORD_ID_COUNT || w3 >= MAX_WORD_ID_COUNT {
        return None;
    }
    Some(((w1 as u64) << 20) | (w3 as u64))
}

#[inline]
const fn decode_score_q6(entry: u64) -> i32 {
    (((entry << 52) as i64) >> 52) as i32
}

#[inline]
fn lookup_key(col: &U64Col, target: u64) -> Option<i32> {
    let (mut lo, mut hi) = (0usize, col.len());
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let entry = col.get(mid);
        match (entry >> 12).cmp(&target) {
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid,
            Ordering::Equal => return Some(decode_score_q6(entry)),
        }
    }
    None
}

fn read_u32(bytes: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(off..off + 4)?.try_into().ok()?))
}

fn read_i32(bytes: &[u8], off: usize) -> Option<i32> {
    Some(i32::from_le_bytes(bytes.get(off..off + 4)?.try_into().ok()?))
}

fn read_u64(bytes: &[u8], off: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(off..off + 8)?.try_into().ok()?))
}

/// Encodes and sorts bridge-trigram and skip-1 entries into a `MIMO-TRI-1\0` binary image.
///
/// Each element of `trigrams` is `(w1, bridge_idx, w3, score_q6)` and each
/// element of `skip1` is `(w1, w3, score_q6)` with `score_q6` in `-2048..=2047`.
pub fn pack_image(
    ids: &WordIds,
    bridge_ids: &[u32],
    trigrams: &[(u32, u32, u32, i32)],
    skip1: &[(u32, u32, i32)],
    tri_weight_q8: i32,
    skip_weight_q8: i32,
) -> Option<Vec<u8>> {
    let word_count = ids.count;
    if word_count == 0 || word_count > MAX_WORD_ID_COUNT || bridge_ids.len() > MAX_BRIDGES {
        return None;
    }
    for (idx, &bid) in bridge_ids.iter().enumerate() {
        if bid >= word_count || bridge_ids[..idx].contains(&bid) {
            return None;
        }
    }
    let mut packed_tri = pack_rows(word_count, trigrams.iter().copied(), bridge_ids.len() as u32)?;
    let mut packed_skip = pack_rows(word_count, skip1.iter().map(|&(w1, w3, s)| (w1, 0, w3, s)), u32::MAX)?;
    let total = DATA_OFFSET + (packed_tri.len() + packed_skip.len()) * 8;
    let mut out = vec![0u8; total];
    out[0..12].copy_from_slice(MAGIC);
    out[12..16].copy_from_slice(&VERSION.to_le_bytes());
    out[16..20].copy_from_slice(&word_count.to_le_bytes());
    out[20..24].copy_from_slice(&(bridge_ids.len() as u32).to_le_bytes());
    out[24..32].copy_from_slice(&ids.fingerprint.to_le_bytes());
    out[32..36].copy_from_slice(&(packed_tri.len() as u32).to_le_bytes());
    out[36..40].copy_from_slice(&(packed_skip.len() as u32).to_le_bytes());
    out[40..44].copy_from_slice(&tri_weight_q8.to_le_bytes());
    out[44..48].copy_from_slice(&skip_weight_q8.to_le_bytes());
    for (i, &bid) in bridge_ids.iter().enumerate() {
        let off = HEADER_BYTES + i * 4;
        out[off..off + 4].copy_from_slice(&bid.to_le_bytes());
    }
    let mut pos = DATA_OFFSET;
    for entry in packed_tri.drain(..).chain(packed_skip.drain(..)) {
        out[pos..pos + 8].copy_from_slice(&entry.to_le_bytes());
        pos += 8;
    }
    Some(out)
}

fn pack_rows<I: Iterator<Item = (u32, u32, u32, i32)>>(word_count: u32, rows: I, bridge_limit: u32) -> Option<Vec<u64>> {
    let mut out = Vec::new();
    for (w1, b_idx, w3, score_q6) in rows {
        if w1 >= word_count || w3 >= word_count || !(MIN_SCORE_Q6..=MAX_SCORE_Q6).contains(&score_q6) {
            return None;
        }
        let key = if bridge_limit == u32::MAX {
            skip_key(w1, w3)?
        } else if b_idx >= bridge_limit {
            return None;
        } else {
            tri_key(w1, b_idx, w3)?
        };
        out.push((key << 12) | ((score_q6 as u64) & SCORE_MASK));
    }
    out.sort_unstable();
    out.dedup_by(|a, b| (*a >> 12) == (*b >> 12));
    Some(out)
}
