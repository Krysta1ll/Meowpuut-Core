#![forbid(unsafe_code)]
//! Candidate ranking, prefix-diversity interleaving, and zero-allocation in-place
//! preedit formatting for nine-key decoding (`ChoicesSoA`).
//!
//! # Examples
//!
//! ```
//! use meowpuut_core::nine_key::KeyInputSoA;
//! use meowpuut_core::nine_key::rank::show_reading;
//!
//! let keys = KeyInputSoA::from_raw(b"644269");
//! let mut preedit = String::from("644269");
//! show_reading(&mut preedit, "ni3 hao3", 5, &keys);
//! assert_eq!(preedit, "ni hao 9");
//! ```

use crate::lexicon::Lexicon;
use crate::reading::CANDIDATE_LIMIT;
use crate::soa::CandidatePoolSoA;
use crate::workspace::text_hash;
use crate::{CandidateKind, CandidateScoreKind};

use super::scratch::{NineKeyLatticeSoA, BEAM_CHINESE, BEAM_INITIALS, BEAM_MIXED};
use super::KeyInputSoA;
pub use super::show_reading;

const LATIN_LEAD_LETTERS: usize = 4;
const LATIN_LEAD_GAP: f64 = 2.0;
const PAGE: usize = 6;
const LEADING_PARTIALS: usize = 24;

/// Preallocated SoA candidate accumulator and ranker for nine-key decoding.
#[derive(Debug, Clone)]
pub struct ChoicesSoA {
    pub arena: String,
    pub read_arena: String,
    pub text_off: Vec<u32>,
    pub text_len: Vec<u16>,
    pub text_hash: Vec<u64>,
    pub prefix_hash: Vec<u64>,
    pub prefix_len: Vec<u16>,
    pub read_off: Vec<u32>,
    pub read_len: Vec<u16>,
    pub consumed: Vec<usize>,
    pub score: Vec<f64>,
    pub kind: Vec<CandidateKind>,
    pub score_kind: Vec<CandidateScoreKind>,
    pub common: Vec<bool>,
    pub complete: Vec<u32>,
    pub partial: Vec<u32>,
    pub japanese: Vec<u32>,
    pub initials: Vec<u32>,
    pub latin: Vec<u32>,
    pub second_latin: Vec<u32>,
    pub latin_first: bool,
    complete_unique: Vec<u32>,
    complete_primary: Vec<u32>,
    complete_rest: Vec<u32>,
    first: Vec<u32>,
    order: Vec<u32>,
    added: Vec<(u64, usize, u32)>,
}

impl ChoicesSoA {
    pub fn new() -> Self {
        let cap = 512;
        Self {
            arena: String::with_capacity(16384),
            read_arena: String::with_capacity(16384),
            text_off: Vec::with_capacity(cap),
            text_len: Vec::with_capacity(cap),
            text_hash: Vec::with_capacity(cap),
            prefix_hash: Vec::with_capacity(cap),
            prefix_len: Vec::with_capacity(cap),
            read_off: Vec::with_capacity(cap),
            read_len: Vec::with_capacity(cap),
            consumed: Vec::with_capacity(cap),
            score: Vec::with_capacity(cap),
            kind: Vec::with_capacity(cap),
            score_kind: Vec::with_capacity(cap),
            common: Vec::with_capacity(cap),
            complete: Vec::with_capacity(256),
            partial: Vec::with_capacity(256),
            japanese: Vec::with_capacity(64),
            initials: Vec::with_capacity(128),
            latin: Vec::with_capacity(64),
            second_latin: Vec::with_capacity(32),
            latin_first: false,
            complete_unique: Vec::with_capacity(256),
            complete_primary: Vec::with_capacity(256),
            complete_rest: Vec::with_capacity(128),
            first: Vec::with_capacity(16),
            order: Vec::with_capacity(cap),
            added: Vec::with_capacity(CANDIDATE_LIMIT + 16),
        }
    }

    pub fn clear(&mut self) {
        self.arena.clear();
        self.read_arena.clear();
        self.text_off.clear();
        self.text_len.clear();
        self.text_hash.clear();
        self.prefix_hash.clear();
        self.prefix_len.clear();
        self.read_off.clear();
        self.read_len.clear();
        self.consumed.clear();
        self.score.clear();
        self.kind.clear();
        self.score_kind.clear();
        self.common.clear();
        self.complete.clear();
        self.partial.clear();
        self.japanese.clear();
        self.initials.clear();
        self.latin.clear();
        self.second_latin.clear();
        self.latin_first = false;
    }

    pub fn capacity_signature(&self) -> usize {
        self.arena.capacity() + self.read_arena.capacity() + self.score.capacity()
            + self.complete.capacity() + self.partial.capacity() + self.japanese.capacity()
            + self.initials.capacity() + self.latin.capacity() + self.order.capacity() + self.added.capacity()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn push_item(&mut self, text: &str, reading: &str, prefix_hash: u64, prefix_len: u16, consumed: usize, score: f64, kind: CandidateKind, score_kind: CandidateScoreKind, common: bool) -> u32 {
        let idx = self.score.len() as u32;
        let (t_off, r_off) = (self.arena.len() as u32, self.read_arena.len() as u32);
        self.arena.push_str(text);
        self.read_arena.push_str(reading);
        self.text_off.push(t_off);
        self.text_len.push(text.len() as u16);
        self.text_hash.push(text_hash(text).0);
        self.prefix_hash.push(prefix_hash);
        self.prefix_len.push(prefix_len);
        self.read_off.push(r_off);
        self.read_len.push(reading.len() as u16);
        self.consumed.push(consumed);
        self.score.push(score);
        self.kind.push(kind);
        self.score_kind.push(score_kind);
        self.common.push(common);
        idx
    }

    /// Concatenates two abbreviated words directly into `arena` without temporary `String`s.
    pub fn push_pair_initials(&mut self, t1: &str, t2: &str, r1: &str, r2: &str, consumed: usize, score: f64) {
        let idx = self.score.len() as u32;
        let t_off = self.arena.len() as u32;
        self.arena.push_str(t1);
        self.arena.push_str(t2);
        let t_slice = &self.arena[t_off as usize..];
        let (t_hash, t_len) = (text_hash(t_slice).0, t_slice.len() as u16);
        let r_off = self.read_arena.len() as u32;
        self.read_arena.push_str(r1);
        self.read_arena.push(' ');
        self.read_arena.push_str(r2);
        let r_len = (self.read_arena.len() - r_off as usize) as u16;
        self.text_off.push(t_off);
        self.text_len.push(t_len);
        self.text_hash.push(t_hash);
        self.prefix_hash.push(0);
        self.prefix_len.push(0);
        self.read_off.push(r_off);
        self.read_len.push(r_len);
        self.consumed.push(consumed);
        self.score.push(score);
        self.kind.push(CandidateKind::Correction);
        self.score_kind.push(CandidateScoreKind::Path);
        self.common.push(false);
        self.initials.push(idx);
    }

    #[inline]
    pub fn text(&self, idx: u32) -> &str {
        let i = idx as usize;
        let s = self.text_off[i] as usize;
        &self.arena[s..s + self.text_len[i] as usize]
    }

    #[inline]
    pub fn reading(&self, idx: u32) -> &str {
        let i = idx as usize;
        let s = self.read_off[i] as usize;
        &self.read_arena[s..s + self.read_len[i] as usize]
    }

    fn collect_lattice(&mut self, lattice: &NineKeyLatticeSoA, keys: &KeyInputSoA, lexicon: &Lexicon) {
        let (end, full, trigrams) = (keys.len, keys.consumed(keys.len), lexicon.trigrams());
        for &n_u16 in &lattice.slots[NineKeyLatticeSoA::cell(end, BEAM_CHINESE)][..lattice.count(end, BEAM_CHINESE)] {
            let n = usize::from(n_u16);
            let sc = lattice.trigram_score(n, trigrams, true);
            let idx = self.push_item(lattice.node_text(n), lattice.node_reading(n), lattice.prefix_hash[n], lattice.prefix_len[n], full, sc, CandidateKind::Path, CandidateScoreKind::Path, lattice.common[n]);
            self.complete.push(idx);
        }
        for &n_u16 in &lattice.slots[NineKeyLatticeSoA::cell(end, BEAM_INITIALS)][..lattice.count(end, BEAM_INITIALS)] {
            let n = usize::from(n_u16);
            let sc = lattice.trigram_score(n, trigrams, true);
            let idx = self.push_item(lattice.node_text(n), lattice.node_reading(n), lattice.prefix_hash[n], lattice.prefix_len[n], full, sc, CandidateKind::Correction, CandidateScoreKind::Path, lattice.common[n]);
            self.initials.push(idx);
        }
        let trailing_vowel = end >= 2 && matches!(keys.slice().last(), Some(b'2' | b'3' | b'6'));
        if self.complete.is_empty() || trailing_vowel {
            if let Some(pos) = (1..end).rev().find(|&p| lattice.count(p, BEAM_CHINESE) > 0) {
                let (cons, cnt) = (keys.consumed(pos), lattice.count(pos, BEAM_CHINESE).min(8));
                for &n_u16 in &lattice.slots[NineKeyLatticeSoA::cell(pos, BEAM_CHINESE)][..cnt] {
                    let n = usize::from(n_u16);
                    let sc = lattice.trigram_score(n, trigrams, false);
                    let idx = self.push_item(lattice.node_text(n), lattice.node_reading(n), 0, 0, cons, sc, CandidateKind::Partial, CandidateScoreKind::Path, lattice.common[n]);
                    self.partial.push(idx);
                }
            }
        }
        let (s, t_off, t_len) = (&self.arena, &self.text_off, &self.text_len);
        self.initials.retain(|&idx| {
            let o = t_off[idx as usize] as usize;
            !lexicon.is_no_suggest_text(&s[o..o + t_len[idx as usize] as usize])
        });
        for &n_u16 in &lattice.slots[NineKeyLatticeSoA::cell(end, BEAM_MIXED)][..lattice.count(end, BEAM_MIXED)] {
            let n = usize::from(n_u16);
            let idx = self.push_item(lattice.node_text(n), lattice.node_reading(n), 0, 0, full, lattice.score[n], CandidateKind::Japanese, CandidateScoreKind::Path, lattice.common[n]);
            self.japanese.push(idx);
        }
    }

    pub fn write_into(&mut self, lattice: &NineKeyLatticeSoA, keys: &KeyInputSoA, lexicon: &Lexicon, out: &mut CandidatePoolSoA, preedit: &mut String, volunteers: impl Fn(&str, usize) -> bool) {
        self.collect_lattice(lattice, keys, lexicon);
        let (sc, cons) = (&self.score, &self.consumed);
        self.complete.sort_by(|&a, &b| sc[b as usize].total_cmp(&sc[a as usize]));
        self.initials.sort_by(|&a, &b| sc[b as usize].total_cmp(&sc[a as usize]));
        let jp_words = self.japanese.iter().take_while(|&&i| self.score_kind[i as usize] == CandidateScoreKind::Lexicon).count();
        self.japanese[..jp_words].sort_by(|&a, &b| sc[b as usize].total_cmp(&sc[a as usize]));
        self.partial.sort_by(|&a, &b| cons[b as usize].cmp(&cons[a as usize]).then_with(|| sc[b as usize].total_cmp(&sc[a as usize])));

        self.complete_unique.clear();
        for i in 0..self.complete.len() {
            let idx = self.complete[i];
            let (h, l) = (self.text_hash[idx as usize], self.text_len[idx as usize]);
            if !self.complete_unique.iter().any(|&u| self.text_hash[u as usize] == h && self.text_len[u as usize] == l && self.text(u) == self.text(idx)) {
                self.complete_unique.push(idx);
            }
        }
        self.complete_primary.clear();
        self.complete_rest.clear();
        for i in 0..self.complete_unique.len() {
            let idx = self.complete_unique[i];
            let (p_len, p_hash) = (self.prefix_len[idx as usize], self.prefix_hash[idx as usize]);
            let prior = if p_len == 0 {
                0
            } else {
                self.complete_unique[..i].iter().filter(|&&u| {
                    self.prefix_len[u as usize] == p_len && self.prefix_hash[u as usize] == p_hash
                }).count()
            };
            self.complete_primary.push(idx);
            // Soft prefix-diversity demotion after the top four completions of a prefix so
            // homophones like 往后滑 (#4 under 往后) stay unpenalized before rarer prefixes (王后*).
            if prior >= 4 {
                self.score[idx as usize] -= (((prior - 3) as f64) * 0.45).min(1.0);
            }
        }
        let sc2 = &self.score;
        self.complete_primary.sort_by(|&a, &b| sc2[b as usize].total_cmp(&sc2[a as usize]));
        let mut jp_write = 0usize;
        for r in 0..self.japanese.len() {
            let j = self.japanese[r];
            let (h, l) = (self.text_hash[j as usize], self.text_len[j as usize]);
            if !self.complete_unique.iter().any(|&u| self.text_hash[u as usize] == h && self.text_len[u as usize] == l && self.text(u) == self.text(j)) {
                self.japanese[jp_write] = j;
                jp_write += 1;
            }
        }
        self.japanese.truncate(jp_write);

        let spelled = !self.complete_unique.is_empty();
        let trailing_vowel_trap = spelled && keys.len >= 3
            && matches!(keys.slice().last(), Some(b'2' | b'3' | b'6'))
            && self.partial.first().is_some_and(|&p| {
                self.consumed[p as usize] == keys.consumed(keys.len - 1)
                    && self.score[p as usize] > self.score[self.complete_primary[0] as usize] + 0.5
            });
        let common_initial_lead = !spelled
            && self.japanese.is_empty()
            && !self.initials.is_empty()
            && self.partial.first().is_none_or(|&p| self.consumed[p as usize] <= 1);
        let typing = !spelled && !common_initial_lead && self.japanese.is_empty() && self.partial.first().is_some_and(|&p| self.consumed[p as usize] >= 2);
        let decoded = spelled || typing || common_initial_lead;
        let lead = self.latin_first && !typing && self.latin.first().is_some_and(|&l| {
            let l_sc = self.score[l as usize];
            self.complete_unique.first().is_none_or(|&c| {
                self.score_kind[c as usize] == CandidateScoreKind::Path && keys.len >= LATIN_LEAD_LETTERS && l_sc >= self.score[c as usize] + LATIN_LEAD_GAP
            }) && self.initials.first().is_none_or(|&ini| l_sc >= self.score[ini as usize])
        });
        let one_syllable = self.complete_unique.first().is_some_and(|&c| self.text(c).chars().nth(1).is_none());

        let (mut c_pos, mut j_pos, mut p_pos, mut i_pos, mut l_pos) = (0, 0, 0, 0, 0);
        self.first.clear();
        if lead && l_pos < self.latin.len() { self.first.push(self.latin[l_pos]); l_pos += 1; }
        if typing || trailing_vowel_trap {
            let end_p = (p_pos + 2).min(self.partial.len());
            self.first.extend_from_slice(&self.partial[p_pos..end_p]);
            p_pos = end_p;
        }
        if common_initial_lead {
            let end_i = (i_pos + 4).min(self.initials.len());
            self.first.extend_from_slice(&self.initials[i_pos..end_i]);
            i_pos = end_i;
        }
        let end_c2 = (c_pos + 2).min(self.complete_primary.len());
        self.first.extend_from_slice(&self.complete_primary[c_pos..end_c2]);
        c_pos = end_c2;
        if self.latin_first && decoded && !lead && l_pos < self.latin.len() { self.first.push(self.latin[l_pos]); l_pos += 1; }
        if j_pos < self.japanese.len() { self.first.push(self.japanese[j_pos]); j_pos += 1; }
        let extra_c = if one_syllable {
            if self.initials.is_empty() { PAGE.saturating_sub(self.first.len()) } else { 2 }
        } else {
            3
        };
        let end_cm = (c_pos + extra_c).min(self.complete_primary.len());
        self.first.extend_from_slice(&self.complete_primary[c_pos..end_cm]);
        c_pos = end_cm;
        if l_pos < self.latin.len() { self.first.push(self.latin[l_pos]); l_pos += 1; }
        if i_pos < self.initials.len() {
            let cap = if typing { 4 } else if one_syllable { 3 } else { 2 };
            let strong = self.initials[i_pos..].iter().take(cap).take_while(|&&c| !decoded || self.common[c as usize]).count();
            let end_is = i_pos + strong;
            self.first.extend_from_slice(&self.initials[i_pos..end_is]);
            i_pos = end_is;
        }
        let end_p2 = (p_pos + 2).min(self.partial.len());
        self.first.extend_from_slice(&self.partial[p_pos..end_p2]);
        p_pos = end_p2;
        if typing || common_initial_lead {
            let src = if common_initial_lead { &self.initials } else { &self.partial };
            let pos = if common_initial_lead { &mut i_pos } else { &mut p_pos };
            let end_pt = (*pos + PAGE.saturating_sub(self.first.len())).min(src.len());
            self.first.extend_from_slice(&src[*pos..end_pt]);
            *pos = end_pt;
        }

        self.order.clear();
        self.order.extend_from_slice(&self.first);
        self.order.extend_from_slice(&self.complete_primary[c_pos..]);
        self.order.extend_from_slice(&self.complete_rest);
        let p_mid = (p_pos + LEADING_PARTIALS).min(self.partial.len());
        self.order.extend_from_slice(&self.partial[p_pos..p_mid]);
        self.order.extend_from_slice(&self.initials[i_pos..]);
        self.order.extend_from_slice(&self.latin[l_pos..]);
        self.order.extend_from_slice(&self.second_latin);
        self.order.extend_from_slice(&self.japanese[j_pos..]);
        self.order.extend_from_slice(&self.partial[p_mid..]);

        self.added.clear();
        let mut rank = 0usize;
        for k in 0..self.order.len() {
            if rank >= CANDIDATE_LIMIT { break; }
            let idx = self.order[k];
            let i = idx as usize;
            let (c_bytes, h) = (self.consumed[i], self.text_hash[i]);
            let is_skip = {
                let t = self.text(idx);
                volunteers(t, c_bytes) || self.added.iter().any(|&(ah, ac, aidx)| ah == h && ac == c_bytes && self.text(aidx) == t)
            };
            if is_skip {
                continue;
            }
            self.added.push((h, c_bytes, idx));
            if rank == 0 { show_reading(preedit, self.reading(idx), c_bytes, keys); }
            out.push(self.text(idx), c_bytes, self.kind[i] as u32, self.score_kind[i] as u32, self.score[i]);
            rank += 1;
        }
    }
}

impl Default for ChoicesSoA {
    fn default() -> Self { Self::new() }
}
