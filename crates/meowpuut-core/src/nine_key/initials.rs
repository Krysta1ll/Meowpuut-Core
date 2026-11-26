#![forbid(unsafe_code)]
//! Initials-only abbreviation spans (`xz` -> `現在`), mixed spelled-plus-initials
//! completions (`74366` -> `shen m` -> `什麼`), and whole-input Latin lookup for nine-key.
//!
//! # Examples
//!
//! ```
//! use std::sync::Arc;
//! use meowpuut_core::{Lexicon, Session};
//!
//! let tsv = concat!(
//!     "MEOWPUUT-PINYIN-1\n",
//!     "shen\tshen2\t什\t什\t4020\n",
//!     "shenme\tshen2me5\t什麼\t什么\t9000\n",
//!     "sm\ts2m5\t什麼\t什么\t1125\n",
//! );
//! let lex = Arc::new(Lexicon::load(tsv).unwrap());
//! let mut s = Session::new_nine_key(lex).unwrap();
//! s.set_simplified(true);
//! for b in b"74366" {
//!     assert!(s.key(i32::from(*b)));
//! }
//! let cands: Vec<_> = (0..s.candidate_count()).map(|i| s.candidate(i)).collect();
//! assert!(cands.contains(&"什么"));
//! ```

use std::ops::Range;

use crate::english::IN_CHINESE;
use crate::latin::syllables;
use crate::lexicon::Lexicon;
use crate::pairs::{END, START, STEP};
use crate::reading::is_tone;
use crate::{CandidateKind, CandidateScoreKind};

use super::decoder::{initials_score, word_score, DecodeCtx, FIRST_PAGE_WEIGHT};
use super::rank::ChoicesSoA;
use super::scratch::{NineKeyLatticeSoA, SpanWordsSoA, WordRef, BEAM_CHINESE, EDGE_LIMIT};
use super::{matches_breaks, signature_keys, INITIALS};

const TAIL: usize = 3;
const TAIL_SPELLED: usize = 12;
const TAIL_WORDS: usize = 64;
const TAIL_SCAN: usize = 256;
const TAIL_KEEP: usize = 32;
const NEXT_WORDS: usize = 6;
const INITIALS_WORDS: usize = 32;
const LATIN_FIRST_TERM: f64 = 1.5;
const LATIN_MAX: usize = 24;

fn word_initials(reading: &str, letters: &mut [u8; INITIALS]) -> usize {
    let (mut count, mut boundary) = (0usize, true);
    for byte in reading.bytes() {
        if is_tone(byte, true) {
            boundary = true;
        } else if boundary {
            if count == INITIALS {
                return 0;
            }
            letters[count] = byte;
            count += 1;
            boundary = false;
        }
    }
    count
}

impl<'a> DecodeCtx<'a> {
    pub(crate) fn trailing_initials(&self, lattice: &mut NineKeyLatticeSoA, words: &mut SpanWordsSoA) {
        let length = self.keys.len;
        let (pairs, trigrams) = (self.lexicon.pairs(), self.lexicon.trigrams());
        let first = length.saturating_sub(INITIALS).max(2);
        for start in first..length.saturating_sub(1) {
            if lattice.is_pos_empty(start) {
                continue;
            }
            let breaks = self.keys.internal_breaks(start, length);
            self.initials_words(start..length, breaks, false, words);
            for k in 0..words.kept.len() {
                let w = words.word_ref(words.kept[k]);
                lattice.extend(start, length, w, pairs, trigrams, true);
            }
        }
        for tail in 1..=TAIL {
            let Some(end) = length.checked_sub(tail).filter(|&e| e > 1) else { break };
            for start in end.saturating_sub(TAIL_SPELLED)..(end - 1) {
                if lattice.is_pos_empty(start) || !self.keys.internal_breaks(start, length).is_empty() {
                    continue;
                }
                let span = start..end;
                let hash = signature_keys(&self.keys.slice()[span.clone()]);
                self.chinese_words(span, hash, false, false, TAIL_WORDS, words);
                self.complete_words(words, start, end, lattice);
            }
        }
    }

    fn complete_words(&self, words: &SpanWordsSoA, start: usize, end: usize, lattice: &mut NineKeyLatticeSoA) {
        let lexicon = self.lexicon;
        let tail = &self.keys.slice()[end..];
        let count_kept = words.kept.len().min(TAIL_WORDS);
        let mut inits = [([0u8; INITIALS], 0usize); TAIL_WORDS];
        for (slot, &w_idx) in inits[..count_kept].iter_mut().zip(&words.kept[..count_kept]) {
            slot.1 = word_initials(words.reading(w_idx), &mut slot.0);
        }
        for i in 0..count_kept {
            let (letters, count) = inits[i];
            if count == 0 || count + tail.len() > INITIALS || inits[..i].contains(&(letters, count)) {
                continue;
            }
            let mut q_keys = [0u8; INITIALS];
            for (dst, &l) in q_keys.iter_mut().zip(&letters[..count]) {
                *dst = l.to_ascii_uppercase();
            }
            q_keys[count..count + tail.len()].copy_from_slice(tail);
            let mut matched_count = 0usize;
            for row in self.index.initials_rows(lexicon, &q_keys[..count + tail.len()]).take(TAIL_SCAN) {
                let weight = lexicon.words.weight(row);
                let reading = lexicon.words.reading(&lexicon.storage, row);
                let text = lexicon.words.text(&lexicon.storage, row, self.hans);
                let matched = (i..count_kept).find(|&m| {
                    let w_text = words.text(words.kept[m]);
                    inits[m] == (letters, count) && text.len() > w_text.len() && text.starts_with(w_text)
                });
                let Some(m_idx) = matched else { continue };
                let id = lexicon.pairs().map_or(START, |_| lexicon.index.word_ids.ids.get(row));
                let completed = WordRef {
                    text,
                    reading: words.reading(words.kept[m_idx]),
                    tail: &reading[2 * count..],
                    initials: true,
                    common: weight >= FIRST_PAGE_WEIGHT,
                    score: initials_score(weight),
                    id,
                    kind: CandidateKind::Correction,
                };
                lattice.extend(start, self.keys.len, completed, lexicon.pairs(), lexicon.trigrams(), true);
                matched_count += 1;
                if matched_count >= TAIL_KEEP {
                    break;
                }
            }
        }
    }

    pub(crate) fn initials_words(&self, span: Range<usize>, breaks: &[u16], whole: bool, words: &mut SpanWordsSoA) {
        let lexicon: &Lexicon = self.lexicon;
        let start = span.start;
        let slice = &self.keys.slice()[span];
        words.clear();
        if !(2..=INITIALS).contains(&slice.len()) {
            return;
        }
        let limit = if whole { INITIALS_WORDS } else { NEXT_WORDS };
        for row in self.index.initials_rows(lexicon, slice).take(2 * limit) {
            let reading = lexicon.words.reading(&lexicon.storage, row);
            if !breaks.is_empty() && !matches_breaks(reading, start, breaks) {
                continue;
            }
            let weight = lexicon.words.weight(row);
            let id = lexicon.pairs().map_or(START, |_| lexicon.index.word_ids.ids.get(row));
            words.push(
                lexicon.words.text(&lexicon.storage, row, self.hans), reading, true,
                whole && weight >= FIRST_PAGE_WEIGHT, initials_score(weight), id, CandidateKind::Correction,
            );
        }
        words.finish(limit);
    }

    pub(crate) fn whole_initials(&self, words: &mut SpanWordsSoA, choices: &mut ChoicesSoA) {
        let length = self.keys.len;
        self.initials_words(0..length, self.keys.internal_breaks(0, length), true, words);
        let consumed = self.keys.consumed(length);
        for &idx in &words.kept {
            let w = words.word_ref(idx);
            let c = choices.push_item(w.text, w.reading, 0, 0, consumed, w.score, w.kind, CandidateScoreKind::Lexicon, w.common);
            choices.initials.push(c);
        }
    }

    pub(crate) fn two_initials(&self, lattice: &NineKeyLatticeSoA, words: &mut SpanWordsSoA, choices: &mut ChoicesSoA) {
        let length = self.keys.len;
        if lattice.count(length, BEAM_CHINESE) > 0
            || choices.initials.len() >= 8
            || !(4..=2 * INITIALS).contains(&length)
        {
            return;
        }
        let (consumed, pairs, breaks) = (self.keys.consumed(length), self.lexicon.pairs(), self.keys.internal_breaks(0, length));
        for split in (2..=length - 2).filter(|&sp| breaks.iter().all(|&b| usize::from(b) == sp)) {
            self.initials_words(0..split, &[], false, words);
            let mut firsts = [([0u8; 24], 0usize, [0u8; 16], 0usize, 0.0f64, START); NEXT_WORDS];
            let mut f_cnt = 0usize;
            for &idx in words.kept.iter().take(NEXT_WORDS) {
                let (t, r) = (words.text(idx).as_bytes(), words.reading(idx).as_bytes());
                if t.len() <= 24 && r.len() <= 16 {
                    firsts[f_cnt].0[..t.len()].copy_from_slice(t); firsts[f_cnt].1 = t.len();
                    firsts[f_cnt].2[..r.len()].copy_from_slice(r); firsts[f_cnt].3 = r.len();
                    firsts[f_cnt].4 = words.score[idx]; firsts[f_cnt].5 = words.id[idx];
                    f_cnt += 1;
                }
            }
            self.initials_words(split..length, &[], false, words);
            for &(ref tb, tl, ref rb, rl, sc1, id1) in &firsts[..f_cnt] {
                let (Ok(t1), Ok(r1)) = (std::str::from_utf8(&tb[..tl]), std::str::from_utf8(&rb[..rl])) else { continue };
                for &idx2 in words.kept.iter().take(NEXT_WORDS) {
                    let id2 = words.id[idx2];
                    let mut sc = sc1 + words.score[idx2];
                    if let Some(p) = pairs {
                        sc += f64::from(p.score(id1, id2)) / STEP + (f64::from(p.score(id2, END)) / STEP).clamp(-EDGE_LIMIT, EDGE_LIMIT);
                    }
                    choices.push_pair_initials(t1, words.text(idx2), r1, words.reading(idx2), consumed, sc);
                }
            }
        }
    }

    pub(crate) fn whole_latin(&self, choices: &mut ChoicesSoA) {
        let lexicon = self.lexicon;
        let slice = self.keys.slice();
        if slice.len() > LATIN_MAX || !self.keys.breaks_slice().is_empty() { return; }
        let consumed = self.keys.consumed(slice.len());
        let mut letters = [0u8; LATIN_MAX];
        for (lo, hi) in self.index.latin_rows(lexicon, slice) {
            for row in lo..hi {
                let reading = lexicon.words.reading(&lexicon.storage, row);
                for (dst, byte) in letters.iter_mut().zip(reading.bytes()) { *dst = byte.to_ascii_lowercase(); }
                let l_slice = &letters[..slice.len()];
                let first = match syllables(lexicon, l_slice) {
                    1 => continue,
                    0 => l_slice.iter().any(|b| b"aeiou".contains(b)),
                    _ => false,
                };
                choices.latin_first |= first;
                let sc = word_score(lexicon.words.weight(row)) + if first { LATIN_FIRST_TERM } else { 0.0 };
                let c = choices.push_item(lexicon.words.text(&lexicon.storage, row, self.hans), reading, 0, 0, consumed, sc, CandidateKind::Exact, CandidateScoreKind::Lexicon, true);
                choices.latin.push(c);
            }
        }
        let sc = &choices.score;
        choices.latin.sort_by(|&a, &b| sc[b as usize].total_cmp(&sc[a as usize]));

        if let Some(en) = lexicon.english() {
            if slice.len() >= 3 && slice.iter().all(|b| (b'2'..=b'9').contains(b)) {
                let mut added_en = 0usize;
                en.nine_key_words(slice, None, 0, |row, score, exact| {
                    if !exact || added_en >= 6 || (en.flags(row) & IN_CHINESE) != 0 { return; }
                    let key_bytes = en.key(row);
                    if syllables(lexicon, key_bytes) == 1 { return; }
                    let Ok(reading) = std::str::from_utf8(key_bytes) else { return };
                    let spelling = en.spelling(row);
                    if choices.latin.iter().any(|&l| choices.text(l).eq_ignore_ascii_case(spelling)) { return; }
                    let c = choices.push_item(spelling, reading, 0, 0, consumed, score, CandidateKind::Exact, CandidateScoreKind::Lexicon, false);
                    choices.second_latin.push(c);
                    added_en += 1;
                });
            }
        }
    }
}
