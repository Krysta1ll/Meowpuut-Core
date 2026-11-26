#![forbid(unsafe_code)]
//! Preallocated Structure-of-Arrays (`SoA`) scratch workspace for zero-allocation
//! nine-key beam decoding, span word deduplication, and candidate ranking.
//!
//! # Examples
//!
//! ```
//! use meowpuut_core::nine_key::scratch::NineKeyScratchSoA;
//!
//! let mut scratch = NineKeyScratchSoA::new();
//! let warm = scratch.capacity_signature();
//! scratch.reset(12);
//! assert_eq!(scratch.capacity_signature(), warm);
//! ```

use crate::pairs::{Pairs, END, START, STEP};
use crate::reading::MAX_INPUT;
use crate::trigrams::Trigrams;
use crate::workspace::text_hash;
use crate::CandidateKind;

/// Paths retained per `(position, beam)` in the nine-key lattice.
pub const PATHS: usize = 40;
const MAX_PER_PREFIX_END: usize = 6;
const MAX_PER_PREFIX_MID: usize = 6;
const MAX_PER_LAST: usize = 8;
/// Clause start/end pair score bound (matching `merge::EDGE_LIMIT`).
pub const EDGE_LIMIT: f64 = 1.0;
/// Sentinel `last_id` for Japanese words without a Chinese pair-table id.
pub const NO_WORD_ID: u32 = u32::MAX;

pub const BEAM_CHINESE: usize = 0;
pub const BEAM_MIXED: usize = 1;
pub const BEAM_INITIALS: usize = 2;
const BEAMS: usize = 3;

/// Borrowed span word descriptor before lattice extension.
#[derive(Clone, Copy)]
pub struct WordRef<'a> {
    pub text: &'a str,
    pub reading: &'a str,
    pub tail: &'a str,
    pub initials: bool,
    pub common: bool,
    pub score: f64,
    pub id: u32,
    pub kind: CandidateKind,
}

/// Preallocated SoA buffer for collecting, sorting, and deduplicating span words.
#[derive(Debug, Clone)]
pub struct SpanWordsSoA {
    pub arena: String,
    pub read_arena: String,
    pub text_off: Vec<u32>,
    pub text_len: Vec<u16>,
    pub text_hash: Vec<u64>,
    pub read_off: Vec<u32>,
    pub read_len: Vec<u16>,
    pub score: Vec<f64>,
    pub id: Vec<u32>,
    pub initials: Vec<bool>,
    pub common: Vec<bool>,
    pub kind: Vec<CandidateKind>,
    pub order: Vec<usize>,
    pub kept: Vec<usize>,
}

impl SpanWordsSoA {
    pub fn new() -> Self {
        let cap = 256;
        Self {
            arena: String::with_capacity(8192), read_arena: String::with_capacity(8192),
            text_off: Vec::with_capacity(cap), text_len: Vec::with_capacity(cap), text_hash: Vec::with_capacity(cap),
            read_off: Vec::with_capacity(cap), read_len: Vec::with_capacity(cap), score: Vec::with_capacity(cap),
            id: Vec::with_capacity(cap), initials: Vec::with_capacity(cap), common: Vec::with_capacity(cap),
            kind: Vec::with_capacity(cap), order: Vec::with_capacity(cap), kept: Vec::with_capacity(cap),
        }
    }

    #[inline]
    pub fn clear(&mut self) {
        self.arena.clear(); self.read_arena.clear();
        self.text_off.clear(); self.text_len.clear(); self.text_hash.clear();
        self.read_off.clear(); self.read_len.clear(); self.score.clear();
        self.id.clear(); self.initials.clear(); self.common.clear();
        self.kind.clear(); self.order.clear(); self.kept.clear();
    }

    #[inline]
    #[allow(clippy::too_many_arguments)]
    pub fn push(&mut self, text: &str, reading: &str, initials: bool, common: bool, score: f64, id: u32, kind: CandidateKind) {
        let idx = self.score.len();
        let (t_off, r_off) = (self.arena.len() as u32, self.read_arena.len() as u32);
        self.arena.push_str(text);
        self.read_arena.push_str(reading);
        self.text_off.push(t_off); self.text_len.push(text.len() as u16); self.text_hash.push(text_hash(text).0);
        self.read_off.push(r_off); self.read_len.push(reading.len() as u16);
        self.score.push(score); self.id.push(id); self.initials.push(initials);
        self.common.push(common); self.kind.push(kind); self.order.push(idx);
    }

    #[inline]
    pub fn text(&self, idx: usize) -> &str {
        let s = self.text_off[idx] as usize;
        &self.arena[s..s + self.text_len[idx] as usize]
    }
    #[inline]
    pub fn reading(&self, idx: usize) -> &str {
        let s = self.read_off[idx] as usize;
        &self.read_arena[s..s + self.read_len[idx] as usize]
    }
    #[inline]
    pub fn word_ref(&self, idx: usize) -> WordRef<'_> {
        WordRef {
            text: self.text(idx), reading: self.reading(idx), tail: "",
            initials: self.initials[idx], common: self.common[idx],
            score: self.score[idx], id: self.id[idx], kind: self.kind[idx],
        }
    }

    pub fn finish(&mut self, limit: usize) {
        let (scores, arena, offs, lens) = (&self.score, &self.arena, &self.text_off, &self.text_len);
        self.order.sort_unstable_by(|&a, &b| {
            scores[b].total_cmp(&scores[a]).then_with(|| {
                let (sa, sb) = (offs[a] as usize, offs[b] as usize);
                arena[sa..sa + lens[a] as usize].cmp(&arena[sb..sb + lens[b] as usize])
            })
        });
        self.kept.clear();
        for &idx in &self.order {
            if self.kept.len() >= limit { break; }
            let (h, l) = (self.text_hash[idx], self.text_len[idx]);
            if !self.kept.iter().any(|&k| self.text_hash[k] == h && self.text_len[k] == l && self.text(k) == self.text(idx)) {
                self.kept.push(idx);
            }
        }
    }
}

impl Default for SpanWordsSoA { fn default() -> Self { Self::new() } }

/// Preallocated SoA beam lattice across positions `0..=MAX_INPUT` and 3 beams.
#[derive(Debug, Clone)]
pub struct NineKeyLatticeSoA {
    pub arena: String,
    pub read_arena: String,
    pub text_off: Vec<u32>,
    pub text_len: Vec<u16>,
    pub text_hash: Vec<u64>,
    pub prefix_hash: Vec<u64>,
    pub prefix_len: Vec<u16>,
    pub read_off: Vec<u32>,
    pub read_len: Vec<u16>,
    pub score: Vec<f64>,
    pub tri_sum: Vec<i64>,
    pub skip_sum: Vec<i64>,
    pub prev2_id: Vec<u32>,
    pub last_id: Vec<u32>,
    pub common: Vec<bool>,
    pub slots: Vec<[u16; PATHS]>,
    pub counts: Vec<u8>,
}

impl NineKeyLatticeSoA {
    pub fn new() -> Self {
        let (cap, cells) = (4096, (MAX_INPUT + 1) * BEAMS);
        Self {
            arena: String::with_capacity(65536), read_arena: String::with_capacity(65536),
            text_off: Vec::with_capacity(cap), text_len: Vec::with_capacity(cap), text_hash: Vec::with_capacity(cap),
            prefix_hash: Vec::with_capacity(cap), prefix_len: Vec::with_capacity(cap),
            read_off: Vec::with_capacity(cap), read_len: Vec::with_capacity(cap),
            score: Vec::with_capacity(cap), tri_sum: Vec::with_capacity(cap), skip_sum: Vec::with_capacity(cap),
            prev2_id: Vec::with_capacity(cap), last_id: Vec::with_capacity(cap), common: Vec::with_capacity(cap),
            slots: vec![[0u16; PATHS]; cells], counts: vec![0u8; cells],
        }
    }

    pub fn reset(&mut self, length: usize, left: u32) {
        self.arena.clear(); self.read_arena.clear();
        self.text_off.clear(); self.text_len.clear(); self.text_hash.clear();
        self.prefix_hash.clear(); self.prefix_len.clear();
        self.read_off.clear(); self.read_len.clear();
        self.score.clear(); self.tri_sum.clear(); self.skip_sum.clear();
        self.prev2_id.clear(); self.last_id.clear(); self.common.clear();
        self.counts[..(length + 1) * BEAMS].fill(0);
        self.text_off.push(0); self.text_len.push(0); self.text_hash.push(0);
        self.prefix_hash.push(0); self.prefix_len.push(0);
        self.read_off.push(0); self.read_len.push(0);
        self.score.push(0.0); self.tri_sum.push(0); self.skip_sum.push(0);
        self.prev2_id.push(u32::MAX); self.last_id.push(left); self.common.push(true);
        self.slots[0][0] = 0; self.counts[0] = 1;
    }

    #[inline]
    pub fn cell(pos: usize, beam: usize) -> usize { pos * BEAMS + beam }
    #[inline]
    pub fn count(&self, pos: usize, beam: usize) -> usize { usize::from(self.counts[Self::cell(pos, beam)]) }
    #[inline]
    pub fn is_pos_empty(&self, pos: usize) -> bool { self.count(pos, BEAM_CHINESE) == 0 && self.count(pos, BEAM_MIXED) == 0 }
    #[inline]
    pub fn node_text(&self, node: usize) -> &str {
        let s = self.text_off[node] as usize;
        &self.arena[s..s + self.text_len[node] as usize]
    }
    #[inline]
    pub fn node_reading(&self, node: usize) -> &str {
        let s = self.read_off[node] as usize;
        &self.read_arena[s..s + self.read_len[node] as usize]
    }

    /// Stage-2 trigram + skip-1 adjusted score for `node` (`at_end` folds the `END` window).
    #[inline]
    pub fn trigram_score(&self, node: usize, trigrams: Option<&Trigrams>, at_end: bool) -> f64 {
        let Some(tri) = trigrams else { return self.score[node]; };
        let (mut tri_s, mut skip_s) = (self.tri_sum[node], self.skip_sum[node]);
        let (p2, p1) = (self.prev2_id[node], self.last_id[node]);
        if at_end && p2 != u32::MAX && p1 != NO_WORD_ID {
            let (dt, ds) = tri.score_window(p2, p1, END);
            tri_s += i64::from(dt);
            skip_s += i64::from(ds);
        }
        self.score[node] + tri.scale_sums(tri_s, skip_s)
    }

    pub fn extend(
        &mut self,
        prev_pos: usize,
        next_pos: usize,
        word: WordRef<'_>,
        pairs: Option<&Pairs>,
        trigrams: Option<&Trigrams>,
        at_end: bool,
    ) {
        let japanese = word.id == NO_WORD_ID;
        let (w_hash, w_pow) = text_hash(word.text);
        for (src_beam, already_mixed) in [(BEAM_CHINESE, false), (BEAM_MIXED, true)] {
            if japanese && already_mixed { break; }
            let dst_beam = if word.initials { BEAM_INITIALS } else if japanese || already_mixed { BEAM_MIXED } else { BEAM_CHINESE };
            let src_cell = Self::cell(prev_pos, src_beam);
            let (src_count, src_nodes) = (usize::from(self.counts[src_cell]), self.slots[src_cell]);
            for &prev_u16 in &src_nodes[..src_count] {
                let prev = usize::from(prev_u16);
                let mut sc = self.score[prev] + word.score;
                if let (Some(p), true) = (pairs, word.id != NO_WORD_ID) {
                    let last = self.last_id[prev];
                    if last != NO_WORD_ID {
                        let step = f64::from(p.score(last, word.id)) / STEP;
                        sc += if last == START { step.clamp(-EDGE_LIMIT, EDGE_LIMIT) } else { step };
                    }
                    if at_end { sc += (f64::from(p.score(word.id, END)) / STEP).clamp(-EDGE_LIMIT, EDGE_LIMIT); }
                }
                let next_prev2 = self.last_id[prev];
                let (mut tri_s, mut skip_s) = (self.tri_sum[prev], self.skip_sum[prev]);
                if let (Some(tri), true) = (trigrams, word.id != NO_WORD_ID && next_prev2 != NO_WORD_ID) {
                    let p2 = self.prev2_id[prev];
                    if p2 != u32::MAX {
                        let (dt, ds) = tri.score_window(p2, next_prev2, word.id);
                        tri_s += i64::from(dt);
                        skip_s += i64::from(ds);
                    }
                }
                self.push_path(next_pos, dst_beam, prev, word, w_hash, w_pow, sc, tri_s, skip_s, next_prev2, at_end);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn push_path(
        &mut self,
        pos: usize,
        beam: usize,
        prev: usize,
        word: WordRef<'_>,
        w_hash: u64,
        w_pow: u64,
        sc: f64,
        tri_s: i64,
        skip_s: i64,
        next_prev2: u32,
        at_end: bool,
    ) {
        if self.score.len() >= u16::MAX as usize { return; }
        let cell = Self::cell(pos, beam);
        let mut count = usize::from(self.counts[cell]);
        let (prev_off, prev_len, prev_hash) = (self.text_off[prev] as usize, self.text_len[prev] as usize, self.text_hash[prev]);
        let (new_len, new_hash) = ((prev_len + word.text.len()) as u16, prev_hash.wrapping_mul(w_pow).wrapping_add(w_hash));

        if let Some(same) = (0..count).find(|&i| {
            let n = usize::from(self.slots[cell][i]);
            self.last_id[n] == word.id && self.text_hash[n] == new_hash && self.text_len[n] == new_len && {
                let t = self.node_text(n);
                t[..prev_len] == self.arena[prev_off..prev_off + prev_len] && &t[prev_len..] == word.text
            }
        }) {
            if self.score[usize::from(self.slots[cell][same])] >= sc { return; }
            self.slots[cell].copy_within(same + 1..count, same);
            count -= 1;
        }

        if beam == BEAM_CHINESE {
            if prev_len > 0 {
                let (cap, p_len) = (if at_end { MAX_PER_PREFIX_END } else { MAX_PER_PREFIX_MID }, prev_len as u16);
                let same_pref = |n: usize| self.prefix_hash[n] == prev_hash && self.prefix_len[n] == p_len;
                if (0..count).filter(|&i| { let n = usize::from(self.slots[cell][i]); same_pref(n) && self.score[n] >= sc }).count() >= cap {
                    self.counts[cell] = count as u8;
                    return;
                }
                if (0..count).filter(|&i| same_pref(usize::from(self.slots[cell][i]))).count() >= cap {
                    if let Some(w) = (0..count).rev().find(|&i| same_pref(usize::from(self.slots[cell][i]))) {
                        self.slots[cell].copy_within(w + 1..count, w);
                        count -= 1;
                    }
                }
            }
            if !at_end && word.id != NO_WORD_ID {
                let same_id = |n: usize| self.last_id[n] == word.id;
                if (0..count).filter(|&i| { let n = usize::from(self.slots[cell][i]); same_id(n) && self.score[n] >= sc }).count() >= MAX_PER_LAST {
                    self.counts[cell] = count as u8;
                    return;
                }
                if (0..count).filter(|&i| same_id(usize::from(self.slots[cell][i]))).count() >= MAX_PER_LAST {
                    if let Some(w) = (0..count).rev().find(|&i| same_id(usize::from(self.slots[cell][i]))) {
                        self.slots[cell].copy_within(w + 1..count, w);
                        count -= 1;
                    }
                }
            }
        }

        let at = self.slots[cell][..count].partition_point(|&n| self.score[usize::from(n)] >= sc);
        if at >= PATHS { self.counts[cell] = count as u8; return; }
        let (node, t_off) = (self.score.len() as u16, self.arena.len() as u32);
        self.arena.extend_from_within(prev_off..prev_off + prev_len);
        self.arena.push_str(word.text);

        let (r_prev_off, r_prev_len, r_off) = (self.read_off[prev] as usize, self.read_len[prev] as usize, self.read_arena.len() as u32);
        self.read_arena.extend_from_within(r_prev_off..r_prev_off + r_prev_len);
        if r_prev_len > 0 { self.read_arena.push(' '); }
        self.read_arena.push_str(word.reading);
        if !word.tail.is_empty() { self.read_arena.push(' '); self.read_arena.push_str(word.tail); }
        let r_len = (self.read_arena.len() - r_off as usize) as u16;
        let common = if word.initials { word.common && prev_len == 0 } else { self.common[prev] };

        self.text_off.push(t_off); self.text_len.push(new_len); self.text_hash.push(new_hash);
        self.prefix_hash.push(prev_hash); self.prefix_len.push(prev_len as u16);
        self.read_off.push(r_off); self.read_len.push(r_len);
        self.score.push(sc); self.tri_sum.push(tri_s); self.skip_sum.push(skip_s);
        self.prev2_id.push(next_prev2); self.last_id.push(word.id); self.common.push(common);

        let new_count = (count + 1).min(PATHS);
        if at + 1 < new_count { self.slots[cell].copy_within(at..new_count - 1, at + 1); }
        self.slots[cell][at] = node;
        self.counts[cell] = new_count as u8;
    }
}

impl Default for NineKeyLatticeSoA { fn default() -> Self { Self::new() } }

/// Complete preallocated scratch workspace for a nine-key session.
#[derive(Debug, Clone, Default)]
pub struct NineKeyScratchSoA {
    pub words: SpanWordsSoA,
    pub lattice: NineKeyLatticeSoA,
    pub choices: super::rank::ChoicesSoA,
}

impl NineKeyScratchSoA {
    pub fn new() -> Self {
        Self { words: SpanWordsSoA::new(), lattice: NineKeyLatticeSoA::new(), choices: super::rank::ChoicesSoA::new() }
    }
    pub fn reset(&mut self, length: usize) {
        self.words.clear(); self.lattice.reset(length, START); self.choices.clear();
    }
    pub fn capacity_signature(&self) -> usize {
        self.words.arena.capacity() + self.words.score.capacity() + self.lattice.arena.capacity()
            + self.lattice.read_arena.capacity() + self.lattice.score.capacity() + self.choices.capacity_signature()
    }
}
