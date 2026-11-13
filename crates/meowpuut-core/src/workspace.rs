#![forbid(unsafe_code)]
//! Preallocated Structure-of-Arrays (`SoA`) scratch workspace for zero-allocation
//! hot-loop beam decoding and segment prediction.
//!
//! # Examples
//!
//! ```
//! use meowpuut_core::workspace::{DecoderScratchSoA, NasalVariantsStack};
//!
//! let mut scratch = DecoderScratchSoA::new();
//! let sig = scratch.capacity_signature();
//! scratch.clear_decode();
//! assert_eq!(scratch.capacity_signature(), sig);
//!
//! let variants = NasalVariantsStack::from_sounds("wang");
//! assert_eq!(variants.len(), 1);
//! assert_eq!(variants.get(0), "wan");
//! ```

use crate::pairs::Pairs;
use crate::reading::{CANDIDATE_LIMIT, MAX_INPUT};

/// Stack-allocated nasal variants (`n` <-> `ng`) for a Pinyin sound string of length `4..=32`.
#[derive(Debug, Clone)]
pub struct NasalVariantsStack {
    buf: [[u8; 36]; 16],
    lens: [u8; 16],
    count: usize,
}

impl NasalVariantsStack {
    pub fn from_sounds(sounds: &str) -> Self {
        let mut out = Self { buf: [[0u8; 36]; 16], lens: [0u8; 16], count: 0 };
        let bytes = sounds.as_bytes();
        if bytes.len() < 4 || bytes.len() > 32 {
            return out;
        }
        for i in 1..bytes.len() {
            if bytes[i] != b'n' || !b"aeiouv".contains(&bytes[i - 1]) || out.count >= 16 {
                continue;
            }
            let slot = &mut out.buf[out.count];
            if i + 1 < bytes.len() && bytes[i + 1] == b'g' {
                slot[..=i].copy_from_slice(&bytes[..=i]);
                let rem = bytes.len() - (i + 2);
                slot[i + 1..i + 1 + rem].copy_from_slice(&bytes[i + 2..]);
                out.lens[out.count] = (bytes.len() - 1) as u8;
                out.count += 1;
            } else if i + 1 == bytes.len() || !b"aeiouv".contains(&bytes[i + 1]) {
                slot[..=i].copy_from_slice(&bytes[..=i]);
                slot[i + 1] = b'g';
                let rem = bytes.len() - (i + 1);
                slot[i + 2..i + 2 + rem].copy_from_slice(&bytes[i + 1..]);
                out.lens[out.count] = (bytes.len() + 1) as u8;
                out.count += 1;
            }
        }
        out
    }

    #[inline]
    pub fn len(&self) -> usize { self.count }
    #[inline]
    pub fn is_empty(&self) -> bool { self.count == 0 }
    #[inline]
    pub fn get(&self, idx: usize) -> &str {
        std::str::from_utf8(&self.buf[idx][..self.lens[idx] as usize]).unwrap_or("")
    }
}

/// Most paths kept per input position (the decoder keeps fewer without a pair table).
pub const LATTICE_STATES: usize = 16;
/// Slot number meaning "no further path" (the end of the input).
pub const NO_SLOT: u32 = u32::MAX;
const HASH_BASE: u64 = 0x0000_0100_0000_01B3;

/// Polynomial hash of `text` and `HASH_BASE^len`; the hash of `a + b` is
/// `hash(a) * pow(b) + hash(b)`, so a path's text hash is built word by word.
#[inline]
pub fn text_hash(text: &str) -> (u64, u64) {
    text.bytes().fold((0u64, 1u64), |(h, p), b| (h.wrapping_mul(HASH_BASE).wrapping_add(u64::from(b)), p.wrapping_mul(HASH_BASE)))
}

/// Preallocated suffix lattice: for each input position, the best paths from
/// there to the end of the input. A path is its first word (a lexicon row)
/// and the slot of the path after it, so no text is copied while decoding;
/// paths with the same text are told apart by `hash` and `len`.
#[derive(Debug, Clone)]
pub struct PathLatticeSoA {
    pub score: Vec<f64>,
    /// Lexicon row of the first word.
    pub word: Vec<u32>,
    /// Slot of the rest of the path, `NO_SLOT` at the end of the input.
    pub next: Vec<u32>,
    /// Word id of the first word (`pairs::END` at the end of the input).
    pub id: Vec<u32>,
    pub hash: Vec<u64>,
    pub pow: Vec<u64>,
    /// Bytes of output text.
    pub len: Vec<u16>,
    /// First four characters of the traditional text, for `Lexicon::join_bonus`.
    pub head: Vec<[u8; 16]>,
    pub head_len: Vec<u8>,
    pub spelled: Vec<bool>,
    pub verified: Vec<bool>,
    pub counts: [u8; MAX_INPUT + 1],
    /// Output text of one path (see `PathLatticeSoA::text_into`).
    pub text: String,
    /// Word id before the input: `pairs::START`, or the last word the person picked.
    pub left: u32,
    pub pair_cache: PairCache,
}

impl PathLatticeSoA {
    pub fn new() -> Self {
        let slots = (MAX_INPUT + 1) * LATTICE_STATES;
        Self {
            score: vec![0.0; slots],
            word: vec![0; slots],
            next: vec![NO_SLOT; slots],
            id: vec![0; slots],
            hash: vec![0; slots],
            pow: vec![1; slots],
            len: vec![0; slots],
            head: vec![[0; 16]; slots],
            head_len: vec![0; slots],
            spelled: vec![false; slots],
            verified: vec![false; slots],
            counts: [0u8; MAX_INPUT + 1],
            text: String::with_capacity(MAX_INPUT * 4),
            left: crate::pairs::START,
            pair_cache: PairCache::new(),
        }
    }

    #[inline]
    pub fn reset(&mut self, length: usize) {
        self.counts[..=length].fill(0);
    }

    #[inline]
    pub fn count_at(&self, pos: usize) -> usize { self.counts[pos] as usize }
    #[inline]
    pub fn slot_index(pos: usize, beam_idx: usize) -> usize { pos * LATTICE_STATES + beam_idx }

    /// The empty path at the end of the input.
    pub fn push_end(&mut self, pos: usize, end_id: u32) {
        let slot = Self::slot_index(pos, 0);
        self.score[slot] = 0.0;
        self.word[slot] = NO_SLOT;
        self.next[slot] = NO_SLOT;
        self.id[slot] = end_id;
        self.hash[slot] = 0;
        self.pow[slot] = 1;
        self.len[slot] = 0;
        self.head_len[slot] = 0;
        self.spelled[slot] = true;
        self.verified[slot] = true;
        self.counts[pos] = 1;
    }

    /// Adds step `idx` as a path at `pos`; `traditional` is the first word's traditional text.
    pub fn push_step(&mut self, pos: usize, step: &StepPathsSoA, idx: usize, traditional: &str) {
        let slot = Self::slot_index(pos, self.counts[pos] as usize);
        let next = step.next[idx] as usize;
        let mut head = [0u8; 16];
        let mut head_len = 0usize;
        let mut chars = 0usize;
        for &b in traditional.as_bytes().iter().chain(&self.head[next][..self.head_len[next] as usize]) {
            let starts_char = (b & 0xC0) != 0x80;
            if starts_char && chars == 4 {
                break;
            }
            chars += usize::from(starts_char);
            if head_len < 16 {
                head[head_len] = b;
                head_len += 1;
            }
        }
        self.score[slot] = step.score[idx];
        self.word[slot] = step.word[idx];
        self.next[slot] = step.next[idx];
        self.id[slot] = step.id[idx];
        self.hash[slot] = step.hash[idx];
        self.pow[slot] = step.pow[idx];
        self.len[slot] = step.len[idx];
        self.head[slot] = head;
        self.head_len[slot] = head_len as u8;
        self.spelled[slot] = step.spelled[idx];
        self.verified[slot] = step.verified[idx];
        self.counts[pos] += 1;
    }

    /// First four characters of the traditional text of the path in `slot`.
    #[inline]
    pub fn head(&self, slot: usize) -> &str {
        std::str::from_utf8(&self.head[slot][..self.head_len[slot] as usize]).unwrap_or("")
    }

    /// Slots of the path in `slot`, one per word, up to the end of the input.
    pub fn words_from(&self, slot: usize) -> impl Iterator<Item = usize> + '_ {
        std::iter::successors(Some(slot), move |&s| Some(self.next[s] as usize)).take_while(move |&s| self.word[s] != NO_SLOT)
    }
}

impl Default for PathLatticeSoA { fn default() -> Self { Self::new() } }

/// Recent word-pair scores, one per slot by hash, kept from key to key: the
/// decode after the next key looks up mostly the same pairs again.
#[derive(Debug, Clone)]
pub struct PairCache {
    keys: Vec<u64>,
    scores: Vec<i32>,
    table: usize,
}

impl PairCache {
    const BITS: u32 = 14;

    pub fn new() -> Self {
        Self { keys: vec![u64::MAX; 1 << Self::BITS], scores: vec![0; 1 << Self::BITS], table: 0 }
    }

    /// Empties the cache when `table` (the address of the pair table) changed.
    pub fn use_table(&mut self, table: usize) {
        if self.table != table {
            self.keys.fill(u64::MAX);
            self.table = table;
        }
    }

    /// `pairs.score(left, right)`, from the cache when it holds it.
    #[inline]
    pub fn score(&mut self, pairs: &Pairs, left: u32, right: u32) -> i32 {
        let key = (u64::from(left) << 32) | u64::from(right);
        let slot = (key.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> (64 - Self::BITS)) as usize;
        if self.keys[slot] != key {
            self.keys[slot] = key;
            self.scores[slot] = pairs.score(left, right);
        }
        self.scores[slot]
    }
}

impl Default for PairCache { fn default() -> Self { Self::new() } }

/// Preallocated candidate paths for one input position: a word followed by a path of the lattice.
#[derive(Debug, Clone)]
pub struct StepPathsSoA {
    pub score: Vec<f64>,
    pub word: Vec<u32>,
    pub next: Vec<u32>,
    pub id: Vec<u32>,
    pub hash: Vec<u64>,
    pub pow: Vec<u64>,
    pub len: Vec<u16>,
    pub spelled: Vec<bool>,
    pub verified: Vec<bool>,
    pub order: Vec<usize>,
}

impl StepPathsSoA {
    pub fn new() -> Self {
        let cap = 4096;
        Self {
            score: Vec::with_capacity(cap),
            word: Vec::with_capacity(cap),
            next: Vec::with_capacity(cap),
            id: Vec::with_capacity(cap),
            hash: Vec::with_capacity(cap),
            pow: Vec::with_capacity(cap),
            len: Vec::with_capacity(cap),
            spelled: Vec::with_capacity(cap),
            verified: Vec::with_capacity(cap),
            order: Vec::with_capacity(cap),
        }
    }

    #[inline]
    pub fn clear(&mut self) {
        self.score.clear();
        self.word.clear();
        self.next.clear();
        self.id.clear();
        self.hash.clear();
        self.pow.clear();
        self.len.clear();
        self.spelled.clear();
        self.verified.clear();
        self.order.clear();
    }

    /// Word `word` (with word id `id`, text hash `hash`/`pow`, `len` bytes) before the lattice path in `next`.
    #[inline]
    #[allow(clippy::too_many_arguments)]
    pub fn push(&mut self, lattice: &PathLatticeSoA, next: usize, word: u32, id: u32, hash: u64, pow: u64, len: usize, score: f64, spelled: bool, verified: bool) {
        self.order.push(self.score.len());
        self.score.push(score);
        self.word.push(word);
        self.next.push(next as u32);
        self.id.push(id);
        self.hash.push(hash.wrapping_mul(lattice.pow[next]).wrapping_add(lattice.hash[next]));
        self.pow.push(pow.wrapping_mul(lattice.pow[next]));
        self.len.push((len + lattice.len[next] as usize) as u16);
        self.spelled.push(spelled);
        self.verified.push(verified);
    }
}

impl Default for StepPathsSoA { fn default() -> Self { Self::new() } }

/// Preallocated SoA output buffer for `decode_into` and `Fallback` lists.
#[derive(Debug, Clone)]
pub struct DecodeOutputSoA {
    pub arena: String,
    pub text_offset: Vec<u32>,
    pub text_len: Vec<u16>,
    pub consumed: Vec<usize>,
    pub kind: Vec<u32>,
    pub score_kind: Vec<u32>,
    pub score: Vec<f64>,
}

impl DecodeOutputSoA {
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            arena: String::with_capacity((cap * 64).max(16384)),
            text_offset: Vec::with_capacity(cap),
            text_len: Vec::with_capacity(cap),
            consumed: Vec::with_capacity(cap),
            kind: Vec::with_capacity(cap),
            score_kind: Vec::with_capacity(cap),
            score: Vec::with_capacity(cap),
        }
    }

    #[inline]
    pub fn clear(&mut self) {
        self.arena.clear();
        self.text_offset.clear();
        self.text_len.clear();
        self.consumed.clear();
        self.kind.clear();
        self.score_kind.clear();
        self.score.clear();
    }

    #[inline]
    pub fn len(&self) -> usize { self.score.len() }
    #[inline]
    pub fn is_empty(&self) -> bool { self.score.is_empty() }
    #[inline]
    pub fn text(&self, idx: usize) -> &str {
        let s = self.text_offset[idx] as usize;
        &self.arena[s..s + self.text_len[idx] as usize]
    }
    #[inline]
    pub fn contains_text(&self, candidate: &str) -> bool {
        (0..self.len()).any(|i| self.text(i) == candidate)
    }

    #[inline]
    pub fn push(&mut self, text: &str, consumed: usize, kind: u32, score_kind: u32, score: f64) {
        let off = self.arena.len() as u32;
        self.arena.push_str(text);
        self.text_offset.push(off);
        self.text_len.push(text.len() as u16);
        self.consumed.push(consumed);
        self.kind.push(kind);
        self.score_kind.push(score_kind);
        self.score.push(score);
    }

    #[inline]
    pub fn insert_at(&mut self, pos: usize, text: &str, consumed: usize, kind: u32, score_kind: u32, score: f64) {
        let off = self.arena.len() as u32;
        self.arena.push_str(text);
        self.text_offset.insert(pos, off);
        self.text_len.insert(pos, text.len() as u16);
        self.consumed.insert(pos, consumed);
        self.kind.insert(pos, kind);
        self.score_kind.insert(pos, score_kind);
        self.score.insert(pos, score);
    }

    #[inline]
    pub fn remove_at(&mut self, idx: usize) {
        self.text_offset.remove(idx);
        self.text_len.remove(idx);
        self.consumed.remove(idx);
        self.kind.remove(idx);
        self.score_kind.remove(idx);
        self.score.remove(idx);
    }

    #[inline]
    pub fn truncate(&mut self, max_len: usize) {
        if self.len() > max_len {
            self.text_offset.truncate(max_len);
            self.text_len.truncate(max_len);
            self.consumed.truncate(max_len);
            self.kind.truncate(max_len);
            self.score_kind.truncate(max_len);
            self.score.truncate(max_len);
        }
    }
}

/// Complete preallocated scratch workspace owned by a `Session`.
#[derive(Debug, Clone)]
pub struct DecoderScratchSoA {
    pub lattice: PathLatticeSoA,
    pub step_paths: StepPathsSoA,
    pub decoded: DecodeOutputSoA,
    pub aux_decoded: DecodeOutputSoA,
    pub fallback_raw: DecodeOutputSoA,
    pub fallback_out: DecodeOutputSoA,
    pub fallback_order: Vec<usize>,
    pub trad_scratch: String,
}

impl DecoderScratchSoA {
    pub fn new() -> Self {
        Self {
            lattice: PathLatticeSoA::new(),
            step_paths: StepPathsSoA::new(),
            decoded: DecodeOutputSoA::with_capacity(CANDIDATE_LIMIT + 32),
            aux_decoded: DecodeOutputSoA::with_capacity(CANDIDATE_LIMIT + 32),
            fallback_raw: DecodeOutputSoA::with_capacity(8192),
            fallback_out: DecodeOutputSoA::with_capacity(256),
            fallback_order: Vec::with_capacity(8192),
            trad_scratch: String::with_capacity(256),
        }
    }

    #[inline]
    pub fn clear_decode(&mut self) {
        self.decoded.clear();
        self.step_paths.clear();
    }

    pub fn capacity_signature(&self) -> usize {
        self.lattice.text.capacity()
            + self.lattice.score.capacity()
            + self.step_paths.score.capacity()
            + self.decoded.arena.capacity()
            + self.decoded.score.capacity()
            + self.aux_decoded.arena.capacity()
            + self.fallback_raw.arena.capacity()
            + self.fallback_out.arena.capacity()
            + self.fallback_order.capacity()
            + self.trad_scratch.capacity()
    }
}

impl Default for DecoderScratchSoA { fn default() -> Self { Self::new() } }
