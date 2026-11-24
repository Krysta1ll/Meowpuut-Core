#![forbid(unsafe_code)]
//! Post-decode fuzzy sound, neighbour-key edit, Zhuyin prefix, and Latin-tail
//! refinement helpers for [`Session::refresh`].
//!
//! # Examples
//!
//! ```
//! use std::sync::Arc;
//! use meowpuut_core::{Lexicon, Session};
//!
//! let tsv = concat!(
//!     "MEOWPUUT-ZHUYIN-1\n",
//!     "cl\tcl3\t好\t好\t5200\n",
//!     "su\tsu3\t你\t你\t5400\n",
//!     "sucl\tsu3cl3\t你好\t你好\t5600\n",
//! );
//! let lex = Arc::new(Lexicon::load(tsv).unwrap());
//! let mut s = Session::new(lex);
//! for b in b"su3cl3" {
//!     assert!(s.key(i32::from(*b)));
//! }
//! assert_eq!(s.candidate(0), "你好");
//! ```

use crate::decoder::has_exact_match;
use crate::fallback::{fuzzy_corrections_into, key_edit_corrections_into, typed_cover_score, Fuzz};
use crate::reading::{strict_han_count, CANDIDATE_LIMIT};
use crate::refresh::{remove_item_at, same_selection};
use crate::session::Session;
use crate::{CandidateKind, CandidateScoreKind};

const FUZZY_LEAD: f64 = 3.0;
const FUZZY_LEAD_FLOOR: f64 = 3000.0 / 700.0 - 19.0 - 3.4;
const EDIT_FIRST: f64 = 3.0;
const COVERED_FIRST: f64 = 4.5;
const ZHUYIN_WHOLE_LEAD: f64 = 1.0;

impl Session {
    pub(crate) fn keep_latin_phrase_first(&mut self) {
        let r = &self.ranked_work;
        let whole = self.input.len();
        if !self.lexicon.pinyin
            || r.len() < 2
            || r.consumed[0] != whole
            || r.kind[0] == CandidateKind::Partial as u32
        {
            return;
        }
        let reading = crate::reading::Reading::new(&self.input, true);
        let sounds = reading.sounds_bytes();
        let n = sounds.len();
        if n != whole || reading.boundary_count[n] != 0 {
            return;
        }
        let initial = if n >= 2 && crate::abbrev::pinyin_initial(sounds, n - 2, n) {
            2
        } else if crate::abbrev::pinyin_initial(sounds, n - 1, n) {
            1
        } else {
            return;
        };
        self.tmp_text.clear();
        self.tmp_text.push_str(self.ranked_work.text(0));
        if !self.tmp_text.bytes().any(|b| b.is_ascii_alphabetic()) {
            return;
        }
        let Some((cut, last)) = self.tmp_text.char_indices().last() else { return };
        if strict_han_count(&self.tmp_text[cut..cut + last.len_utf8()]) != 1 {
            return;
        }
        if self.lead_word_goes_on(&self.tmp_text, n - initial) {
            return;
        }
        let phrase = &self.tmp_text[..cut];
        let consumed = whole - initial;
        let Some(at) = (1..self.ranked_work.len().min(CANDIDATE_LIMIT))
            .find(|&i| self.ranked_work.consumed[i] == consumed && self.ranked_work.text(i) == phrase)
        else {
            return;
        };
        let (t_off, t_len, cons, knd, sknd, scr) = remove_item_at(&mut self.ranked_work, at);
        let w = &mut self.ranked_work;
        w.text_offset.insert(0, t_off);
        w.text_len.insert(0, t_len);
        w.consumed.insert(0, cons);
        w.kind.insert(0, knd);
        w.score_kind.insert(0, sknd);
        w.score.insert(0, scr);
    }

    fn lead_word_goes_on(&self, lead: &str, before: usize) -> bool {
        let lex = &self.lexicon;
        (before.saturating_sub(12)..before).any(|k| {
            let key = &self.input[k..];
            let mut heaviest: Option<(i32, &str)> = None;
            for bucket in (lex.lower_bound_bucket(key)..lex.buckets.len()).take(1024) {
                if !lex.buckets.key(&lex.storage, bucket).starts_with(key) {
                    break;
                }
                let (lo, hi) = lex.buckets.range(bucket);
                for i in lo..hi {
                    let word = lex.words.text(&lexicon_storage(lex), i, self.hans);
                    let weight = lex.words.weight(i);
                    if strict_han_count(word) >= 2 && heaviest.is_none_or(|(w, _)| weight > w) {
                        heaviest = Some((weight, word));
                    }
                }
            }
            heaviest.is_some_and(|(_, word)| lead.ends_with(word))
        })
    }

    pub(crate) fn offer_key_edits(&mut self) {
        if !self.lexicon.pinyin {
            return;
        }
        key_edit_corrections_into(
            &self.lexicon,
            &self.input,
            self.hans,
            &mut self.scratch,
            &mut self.reorder_buf,
        );
        let valid = has_exact_match(&self.lexicon, &self.input)
            || (0..self.japanese.len()).any(|j| self.japanese.reading(j) == self.input);
        let whole = self.input.len();
        let covered_rescue = self.reorder_buf.is_empty();
        if covered_rescue
            && !self.ranked_work.is_empty()
            && self.ranked_work.consumed[0] == whole
        {
            if let Some(lit) = crate::spatial::score_to_nats(
                &self.lexicon,
                self.ranked_work.score_kind[0],
                self.ranked_work.score[0],
            ) {
                crate::spatial::covered_typo_corrections_into(
                    &self.lexicon,
                    &self.input,
                    self.hans,
                    lit,
                    valid,
                    &mut self.scratch,
                    &mut self.reorder_buf,
                );
            }
        }
        if self.reorder_buf.is_empty() {
            return;
        }
        let guessed = !self.ranked_work.is_empty()
            && self.ranked_work.consumed[0] == whole
            && self.ranked_work.kind[0] == CandidateKind::Correction as u32
            && self.ranked_work.score_kind[0] == CandidateScoreKind::Fallback as u32;
        let partial = !self.ranked_work.is_empty()
            && self.ranked_work.consumed[0] < whole
            && self.ranked_work.kind[0] == CandidateKind::Partial as u32
            && self.ranked_work.score_kind[0] == CandidateScoreKind::None as u32;
        let first = !guessed && !partial && !self.abbreviated_lead() && !valid && (!covered_rescue || whole >= 8);
        let first_bar = if covered_rescue { COVERED_FIRST } else { EDIT_FIRST };
        let mut position = 0;
        for i in 0..self.reorder_buf.len() {
            let gain = self.reorder_buf.score[i];
            let mut at = if first && gain >= first_bar {
                position
            } else {
                position.max(if guessed || covered_rescue { 1 } else { 2 })
            };
            if covered_rescue && at > 0 {
                while at < self.ranked_work.len()
                    && self.ranked_work.score_kind[at] == CandidateScoreKind::FuzzyInitial as u32
                {
                    at += 1;
                }
            }
            let at = at.min(self.ranked_work.len());
            let text = self.reorder_buf.text(i);
            if let Some(existing) = (0..self.ranked_work.len()).find(|&r| self.ranked_work.text(r) == text) {
                if existing <= at
                    || (covered_rescue && self.ranked_work.score_kind[existing] == CandidateScoreKind::FuzzyInitial as u32)
                {
                    position = existing + 1;
                    continue;
                }
                if self.ranked_work.consumed[existing] != whole {
                    continue;
                }
                remove_item_at(&mut self.ranked_work, existing);
            }
            self.ranked_work.insert_at(
                at,
                text,
                whole,
                self.reorder_buf.kind[i],
                self.reorder_buf.score_kind[i],
                self.reorder_buf.score[i],
            );
            position = at + 1;
        }
        self.ranked_work.truncate(CANDIDATE_LIMIT);
    }

    fn typed_score(&mut self) -> f64 {
        let reading = crate::reading::Reading::new(&self.input, true);
        let (lo, hi) = self.lexicon.range(reading.sounds());
        let word = (lo..hi)
            .filter(|&i| {
                self.lexicon.words.is_fully_spelled(i)
                    && reading.matches(
                        self.lexicon.words.reading(&self.lexicon.storage, i).as_bytes(),
                        0,
                        reading.len(),
                    )
            })
            .map(|i| crate::merge::whole_word_score(&self.lexicon, i))
            .fold(f64::NEG_INFINITY, f64::max);
        word.max(typed_cover_score(&self.lexicon, &self.input, &mut self.scratch))
    }

    pub(crate) fn zhuyin_typed_prefix_range(&self) -> Option<(usize, usize, usize)> {
        let whole = self.input.len();
        let seg = self.segments_work.text(0);
        let seg_consumed = self.segments_work.consumed[0];
        if seg_consumed + 1 != whole {
            return None;
        }
        let reading = crate::reading::Reading::new(&self.input, false);
        let n = reading.len();
        let tones = usize::from(reading.boundary_count[n.saturating_sub(1)]);
        if tones == 0 {
            return None;
        }
        let r = &self.ranked_work;
        let agrees = |i: usize| r.consumed[i] == whole && strict_han_count(r.text(i)) == tones + 1;
        let limit = r.len().min(64);
        let best = (0..limit).find(|&i| agrees(i))?;
        let text = r.text(best);
        let cut = text.char_indices().last().map(|(at, _)| at)?;
        if &text[..cut] == seg {
            return None;
        }
        let nats = |i: usize| match r.score_kind[i] {
            k if k == CandidateScoreKind::Path as u32 => Some(r.score[i]),
            k if k == CandidateScoreKind::Lexicon as u32 => Some(r.score[i] / 700.0 - 19.0),
            _ => None,
        };
        if let Some(own) = (best + 1..limit).find(|&i| agrees(i) && r.text(i).starts_with(seg)) {
            if let (Some(b), Some(o)) = (nats(best), nats(own)) {
                if r.score_kind[best] == r.score_kind[own] && b - o < ZHUYIN_WHOLE_LEAD {
                    return None;
                }
            }
        }
        Some((r.text_offset[best] as usize, cut, seg_consumed))
    }

    pub(crate) fn zhuyin_initial_after_tone(&self) -> bool {
        let reading = crate::reading::Reading::new(&self.input, false);
        let n = reading.len();
        n >= 2 && reading.boundaries[n - 1] != 0 && crate::abbrev::ends_abbreviated(&self.lexicon, &reading)
    }

    pub(crate) fn abbreviated_lead(&self) -> bool {
        let r = &self.ranked_work;
        let reading = crate::reading::Reading::new(&self.input, self.lexicon.pinyin);
        let sounds = reading.sounds_bytes();
        let n = sounds.len();
        let bare = if self.lexicon.pinyin {
            n > 0
                && reading.boundaries[n] == 0
                && (crate::abbrev::pinyin_initial(sounds, n - 1, n)
                    || n > 1 && crate::abbrev::pinyin_initial(sounds, n - 2, n))
        } else {
            crate::abbrev::ends_abbreviated(&self.lexicon, &reading)
        };
        bare && !r.is_empty()
            && r.consumed[0] == self.input.len()
            && r.kind[0] == CandidateKind::Correction as u32
            && r.score_kind[0] == CandidateScoreKind::Lexicon as u32
    }

    pub(crate) fn offer_pinyin_corrections(&mut self, fuzz: Fuzz) {
        let initials = fuzz != Fuzz::Final;
        if initials
            && self.lexicon.pinyin
            && !self.ranked_work.is_empty()
            && self.ranked_work.kind[0] == CandidateKind::Exact as u32
            && self.ranked_work.score_kind[0] == CandidateScoreKind::Lexicon as u32
        {
            let reading = crate::reading::Reading::new(&self.input, true);
            let (lo, hi) = self.lexicon.range(reading.sounds());
            if (lo..hi.min(lo + 7)).any(|i| {
                self.lexicon.words.weight(i) >= 6500
                    && self.lexicon.words.text(&self.lexicon.storage, i, self.hans) == self.ranked_work.text(0)
            }) {
                return;
            }
        }
        let words = fuzzy_corrections_into(
            &self.lexicon,
            &self.input,
            self.hans,
            &mut self.scratch,
            &mut self.reorder_buf,
            fuzz,
        );
        if !self.reorder_buf.is_empty() {
            let literal = !self.ranked_work.is_empty()
                && self.ranked_work.consumed[0] == self.input.len()
                && self.ranked_work.score_kind[0] != CandidateScoreKind::Lexicon as u32
                && [CandidateKind::Exact, CandidateKind::Path, CandidateKind::Unknown]
                    .iter()
                    .any(|&k| self.ranked_work.kind[0] == k as u32);
            let guessed_lead = !initials
                && !self.ranked_work.is_empty()
                && self.ranked_work.consumed[0] == self.input.len()
                && self.ranked_work.kind[0] == CandidateKind::Correction as u32
                && self.ranked_work.score_kind[0] == CandidateScoreKind::Fallback as u32
                && self.reorder_buf.score[0] > self.ranked_work.score[0];
            let lead = words
                && (literal || guessed_lead)
                && strict_han_count(self.reorder_buf.text(0)) > 1
                && self.reorder_buf.score[0] >= FUZZY_LEAD_FLOOR
                && self.reorder_buf.score[0] > self.typed_score() + FUZZY_LEAD;
            let whole = self.input.len();
            let listed_at = if words
                && strict_han_count(self.reorder_buf.text(0)) > 1
                && self.reorder_buf.score[0] >= FUZZY_LEAD_FLOOR
            {
                (0..self.ranked_work.len().min(CANDIDATE_LIMIT)).find(|&r| {
                    self.ranked_work.consumed[r] == whole
                        && self.lexicon.is_no_suggest_text(self.ranked_work.text(r))
                })
            } else {
                None
            };
            if fuzz == Fuzz::Rare && !lead && listed_at.is_none() {
                return;
            }
            let spelled = |i: usize| {
                let (kind, score_kind) = (self.ranked_work.kind[i], self.ranked_work.score_kind[i]);
                let unverified = kind == CandidateKind::Unknown as u32
                    && (score_kind == CandidateScoreKind::Lexicon as u32
                        || score_kind == CandidateScoreKind::Path as u32);
                self.ranked_work.consumed[i] == whole
                    && (kind == CandidateKind::Exact as u32
                        || kind == CandidateKind::Path as u32
                        || unverified)
            };
            let position = if lead {
                0
            } else if initials {
                2.min(self.ranked_work.len())
            } else if (0..self.ranked_work.len()).any(spelled) {
                3.min(self.ranked_work.len())
            } else {
                0
            };
            let mut position = listed_at.map_or(position, |at| position.min(at));
            let mut inserted = 0;
            for i in 0..self.reorder_buf.len() {
                let text = self.reorder_buf.text(i);
                if let Some(existing) =
                    (0..self.ranked_work.len()).find(|&r| same_selection(&self.ranked_work, r, text, whole))
                {
                    if existing < position || self.ranked_work.kind[existing] != CandidateKind::Correction as u32 {
                        continue;
                    }
                    remove_item_at(&mut self.ranked_work, existing);
                }
                self.ranked_work.insert_at(
                    position,
                    text,
                    whole,
                    self.reorder_buf.kind[i],
                    self.reorder_buf.score_kind[i],
                    self.reorder_buf.score[i],
                );
                position += 1;
                inserted += 1;
                if inserted == if initials { 1 } else { 2 } {
                    break;
                }
            }
            self.ranked_work.truncate(CANDIDATE_LIMIT);
        }
    }
}

#[inline]
fn lexicon_storage(lex: &crate::lexicon::Lexicon) -> &str {
    &lex.storage
}
