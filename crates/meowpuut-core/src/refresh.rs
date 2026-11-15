#![forbid(unsafe_code)]
//! Candidate refresh and segment-context reranking pipeline for [`Session`].
//!
//! # Examples
//!
//! ```
//! use std::sync::Arc;
//! use meowpuut_core::{Lexicon, Session};
//!
//! let tsv = concat!(
//!     "MEOWPUUT-PINYIN-1\n",
//!     "hao\thao3\t好\t好\t4800\n",
//!     "ni\tni3\t你\t你\t5000\n",
//!     "nihao\tni3hao3\t你好\t你好\t5400\n",
//! );
//! let lex = Arc::new(Lexicon::load(tsv).unwrap());
//! let mut session = Session::new(lex);
//! session.key(i32::from(b'n'));
//! session.key(i32::from(b'i'));
//! session.refresh();
//! assert_eq!(session.candidate(0), "你");
//! ```

use std::cmp::Ordering;

use crate::decoder::{decode_into, decode_with_buffers, fallback_into, has_exact_match, unfinished_phrase_into};
use crate::fallback::{dropped_letter_candidate, fuzzy_corrections_into, key_edit_corrections_into, typed_cover_score, Fuzz};
use crate::mixed::unfinished_japanese_prefix_into;
use crate::reading::{strict_han_count, zhuyin_symbol_for_key, CANDIDATE_LIMIT, MAX_INPUT};
use crate::segments::{
    pinyin_segments_into, predictive_segments_into, unfinished_sentence_into, zhuyin_segments_into,
};
use crate::session::Session;
use crate::workspace::DecodeOutputSoA;
use crate::{CandidateKind, CandidateScoreKind};

/// Nats a fuzzy whole-word correction (penalty included) must score above
/// the best word spelled as typed to come first.
const FUZZY_LEAD: f64 = 3.0;
/// Lowest score of a fuzzy correction that comes first: a common word
/// (weight 3000, less the larger fuzzy penalty), not a rare one (erlin 二林).
const FUZZY_LEAD_FLOOR: f64 = 3000.0 / 700.0 - 19.0 - 3.4;
/// Nats above the typed letters that put a key-edit correction first.
const EDIT_FIRST: f64 = 3.0;
/// Nats by which the best whole reading of a Zhuyin input ending with a first
/// symbol after a tone must beat one that keeps the decode of the syllables
/// before that symbol, to lead with its own reading of them.
const ZHUYIN_WHOLE_LEAD: f64 = 1.0;

#[inline]
fn same_selection(ranked: &DecodeOutputSoA, r_idx: usize, text: &str, consumed: usize) -> bool {
    ranked.consumed[r_idx] == consumed && ranked.text(r_idx) == text
}

fn remove_item_at(ranked: &mut DecodeOutputSoA, idx: usize) -> (u32, u16, usize, u32, u32, f64) {
    (
        ranked.text_offset.remove(idx),
        ranked.text_len.remove(idx),
        ranked.consumed.remove(idx),
        ranked.kind.remove(idx),
        ranked.score_kind.remove(idx),
        ranked.score.remove(idx),
    )
}

fn prepend_items(ranked: &mut DecodeOutputSoA, items: &DecodeOutputSoA) {
    for idx in (0..items.len()).rev() {
        let it_text = items.text(idx);
        let it_consumed = items.consumed[idx];
        if let Some(d) = (0..ranked.len()).find(|&r| same_selection(ranked, r, it_text, it_consumed)) {
            let _ = remove_item_at(ranked, d);
        }
        ranked.insert_at(0, it_text, it_consumed, CandidateKind::Partial as u32, CandidateScoreKind::None as u32, 0.0);
        ranked.truncate(CANDIDATE_LIMIT);
    }
}

fn insert_partials_at(ranked: &mut DecodeOutputSoA, items: &DecodeOutputSoA, start_pos: usize, jump_after_three: bool) {
    let mut position = start_pos.min(ranked.len());
    let mut inserted = 0usize;
    for i in 0..items.len() {
        let it_text = items.text(i);
        let it_consumed = items.consumed[i];
        if (0..ranked.len()).any(|r| same_selection(ranked, r, it_text, it_consumed)) {
            continue;
        }
        if jump_after_three && inserted == 3 {
            position = 10.min(ranked.len());
        }
        ranked.insert_at(position, it_text, it_consumed, CandidateKind::Partial as u32, CandidateScoreKind::None as u32, 0.0);
        position += 1;
        inserted += 1;
        ranked.truncate(CANDIDATE_LIMIT);
    }
}

impl Session {
    /// Whether a complete-only decode of the current input is non-empty.
    /// Memoized for the duration of one `refresh` (input and script are fixed).
    pub(crate) fn has_complete_decode(&mut self) -> bool {
        if self.complete_memo != 0 {
            return self.complete_memo == 2;
        }
        decode_with_buffers(
            &self.lexicon,
            &self.input,
            self.hans,
            true,
            false,
            false,
            &mut self.scratch.lattice,
            &mut self.scratch.step_paths,
            &mut self.scratch.fallback_raw,
            &mut self.scratch.fallback_order,
            &mut self.scratch.fallback_out,
            &mut self.scratch.aux_decoded,
        );
        let found = !self.scratch.aux_decoded.is_empty();
        self.complete_memo = if found { 2 } else { 1 };
        found
    }

    /// Recomputes preedit and ranked candidates into the session's SoA candidate pool.
    pub fn refresh(&mut self) {
        self.complete_memo = 0;
        self.preedit.clear();
        if self.lexicon.pinyin {
            self.preedit.push_str(&self.input);
        } else {
            for &b in self.input.as_bytes() {
                if let Some(sym) = zhuyin_symbol_for_key(b) {
                    self.preedit.push_str(sym);
                }
            }
        }

        if let Some(index) = &self.nine_key {
            index.decode(&self.lexicon, &self.japanese, self.input.as_bytes(), self.hans, self.scratch.lattice.left, &mut self.candidates, &mut self.preedit);
            self.page = 0;
            return;
        }
        decode_into(&self.lexicon, &self.input, self.hans, false, true, &mut self.scratch);
        crate::row::prefix_offers(&self.lexicon, &self.input, self.hans, &self.scratch.lattice, &self.scratch.decoded, &mut self.row_work);
        self.ranked_work.clear();
        for i in 0..self.scratch.decoded.len() {
            self.ranked_work.push(
                self.scratch.decoded.text(i),
                self.input.len(),
                self.scratch.decoded.kind[i],
                self.scratch.decoded.score_kind[i],
                self.scratch.decoded.score[i],
            );
        }

        unfinished_phrase_into(&self.lexicon, &self.input, self.hans, &mut self.partial_work);
        // A completion of a shorter reading (chibaomei to 吃飽沒事幹) only when
        // the letters typed do not spell a reading of their own.
        if !self.partial_work.is_empty() && self.has_complete_decode() {
            self.partial_work.clear();
        }
        if self.abbreviated_lead() {
            insert_partials_at(&mut self.ranked_work, &self.partial_work, 1, false);
        } else {
            prepend_items(&mut self.ranked_work, &self.partial_work);
        }

        predictive_segments_into(
            &self.lexicon,
            &self.input,
            self.hans,
            &mut self.scratch,
            &mut self.predictive_work,
        );
        if !self.predictive_work.is_empty() {
            let no_complete = if self.lexicon.pinyin {
                !self.has_complete_decode()
            } else {
                self.ranked_work.is_empty()
            };
            if no_complete && self.abbreviated_lead() {
                insert_partials_at(&mut self.ranked_work, &self.predictive_work, 1, false);
            } else if no_complete {
                prepend_items(&mut self.ranked_work, &self.predictive_work);
            } else {
                insert_partials_at(&mut self.ranked_work, &self.predictive_work, 3, false);
            }
        }

        if !self.ranked_work.is_empty()
            && self.partial_work.is_empty()
            && self.predictive_work.is_empty()
            && self.lexicon.pinyin
            && (6..=MAX_INPUT).contains(&self.input.len())
        {
            let bytes = self.input.as_bytes();
            let last = bytes[bytes.len() - 1];
            let prev = bytes[bytes.len() - 2];
            if b"bpftdlgkhjqxzcrsywn".contains(&last)
                && (last != b'n' || prev == b'n')
                && !has_exact_match(&self.lexicon, &self.input)
            {
                let allow_onset = if self.input.len() <= 64 {
                    !self.has_complete_decode()
                } else {
                    !((last == b'g' && prev == b'n') || (last == b'r' && prev == b'e'))
                };
                if allow_onset {
                    unfinished_sentence_into(&self.lexicon, &self.input, self.hans, true, false, &mut self.scratch, &mut self.segments_work);
                    if self.abbreviated_lead() {
                        insert_partials_at(&mut self.ranked_work, &self.segments_work, 1, false);
                    } else {
                        prepend_items(&mut self.ranked_work, &self.segments_work);
                    }
                }
            }
        }

        if self.ranked_work.is_empty() {
            unfinished_sentence_into(&self.lexicon, &self.input, self.hans, false, false, &mut self.scratch, &mut self.segments_work);
            for i in 0..self.segments_work.len() {
                self.ranked_work.push(
                    self.segments_work.text(i),
                    self.segments_work.consumed[i],
                    CandidateKind::Partial as u32,
                    CandidateScoreKind::None as u32,
                    0.0,
                );
            }
        }

        if self.lexicon.pinyin
            && !self.ranked_work.is_empty()
            && self.partial_work.is_empty()
            && self.predictive_work.is_empty()
            && self.input.len() >= 6
            && !self.ranked_work.consumed.iter().any(|&c| c < self.input.len())
            && !has_exact_match(&self.lexicon, &self.input)
        {
            unfinished_sentence_into(&self.lexicon, &self.input, self.hans, false, true, &mut self.scratch, &mut self.segments_work);
            if !self.segments_work.is_empty() && !self.has_complete_decode() {
                prepend_items(&mut self.ranked_work, &self.segments_work);
            }
        }

        if self.lexicon.pinyin
            && !self.japanese.is_empty()
            && (5..=MAX_INPUT).contains(&self.input.len())
            && !has_exact_match(&self.lexicon, &self.input)
        {
            unfinished_japanese_prefix_into(
                &self.lexicon,
                &self.japanese,
                &self.input,
                self.hans,
                self.ranked_work.is_empty(),
                &mut self.scratch,
                &mut self.segments_work,
            );
            let mut position = 3.min(self.ranked_work.len());
            for i in 0..self.segments_work.len() {
                let it_text = self.segments_work.text(i);
                let it_consumed = self.segments_work.consumed[i];
                let existing = (0..self.ranked_work.len())
                    .find(|&r| same_selection(&self.ranked_work, r, it_text, it_consumed));
                if let Some(ex_idx) = existing {
                    if ex_idx < position {
                        continue;
                    }
                    let (t_off, t_len, cons, knd, sknd, scr) = remove_item_at(&mut self.ranked_work, ex_idx);
                    self.ranked_work.text_offset.insert(position, t_off);
                    self.ranked_work.text_len.insert(position, t_len);
                    self.ranked_work.consumed.insert(position, cons);
                    self.ranked_work.kind.insert(position, knd);
                    self.ranked_work.score_kind.insert(position, sknd);
                    self.ranked_work.score.insert(position, scr);
                } else {
                    self.ranked_work.insert_at(
                        position,
                        it_text,
                        it_consumed,
                        CandidateKind::Partial as u32,
                        CandidateScoreKind::None as u32,
                        0.0,
                    );
                }
                position += 1;
                self.ranked_work.truncate(CANDIDATE_LIMIT);
            }
        }

        if !self.lexicon.pinyin {
            zhuyin_segments_into(&self.lexicon, &self.input, self.hans, &mut self.scratch, &mut self.segments_work);
            // A first symbol typed right after a tone (ㄋㄧˇㄏㄠˇㄒ) is most often
            // the next syllable being typed, not an abbreviation: the words
            // before it keep the lead they had before abbreviations (你好, ㄒ
            // left), the abbreviated reads (你好想) follow. Two first symbols
            // (ㄨㄛˇㄒㄏ) or none typed in full (ㄋㄏ) still lead with them.
            let after_tone = self.zhuyin_initial_after_tone();
            if after_tone && self.segments_work.is_empty() {
                // One syllable before the symbol (ㄋㄧˇㄒ): its characters, as before.
                fallback_into(&self.lexicon, &self.input, self.hans, true, &mut self.scratch.fallback_raw,
                              &mut self.scratch.fallback_order, &mut self.scratch.fallback_out);
                for i in 0..self.scratch.fallback_out.len() {
                    if self.scratch.fallback_out.consumed[i] < self.input.len() {
                        self.segments_work.push(self.scratch.fallback_out.text(i), self.scratch.fallback_out.consumed[i],
                                                CandidateKind::Partial as u32, CandidateScoreKind::Fallback as u32, 0.0);
                    }
                }
            }
            if !self.segments_work.is_empty() && after_tone {
                // The syllables typed in full lead, read as the best reading of
                // the whole input reads them (你是什 from 你是什麼 for
                // ㄋㄧˇㄕˋㄕㄣˊㄇ, not 你是神 read without the last symbol).
                // The decode of those syllables alone leads only when no
                // whole reading agrees with them.
                let (text, consumed) = self.zhuyin_typed_prefix().unwrap_or_else(|| {
                    (self.segments_work.text(0).to_owned(), self.segments_work.consumed[0])
                });
                if let Some(d) = (0..self.ranked_work.len()).find(|&r| same_selection(&self.ranked_work, r, &text, consumed)) {
                    let _ = remove_item_at(&mut self.ranked_work, d);
                }
                self.ranked_work.insert_at(0, &text, consumed, CandidateKind::Partial as u32, CandidateScoreKind::None as u32, 0.0);
                self.ranked_work.truncate(CANDIDATE_LIMIT);
            }
            insert_partials_at(&mut self.ranked_work, &self.segments_work, 3, true);
        }

        if self.lexicon.pinyin && !self.ranked_work.is_empty() && self.has_complete_decode() {
            pinyin_segments_into(&self.lexicon, &self.input, self.hans, &mut self.scratch, &mut self.segments_work);
            insert_partials_at(&mut self.ranked_work, &self.segments_work, 3, false);
        }

        if self.ranked_work.is_empty() {
            fallback_into(
                &self.lexicon,
                &self.input,
                self.hans,
                true,
                &mut self.scratch.fallback_raw,
                &mut self.scratch.fallback_order,
                &mut self.scratch.fallback_out,
            );
            for i in 0..self.scratch.fallback_out.len() {
                let cons = self.scratch.fallback_out.consumed[i];
                let kind = if cons < self.input.len() {
                    CandidateKind::Partial as u32
                } else {
                    CandidateKind::Correction as u32
                };
                self.ranked_work.push(
                    self.scratch.fallback_out.text(i),
                    cons,
                    kind,
                    CandidateScoreKind::Fallback as u32,
                    self.scratch.fallback_out.score[i],
                );
            }
        }

        if !self.segment_context.is_empty() && self.ranked_work.len() > 1 {
            let limit = 64.min(self.ranked_work.len());
            let exact = has_exact_match(&self.lexicon, &self.input);
            let mut order_idx = [0usize; 64];
            let mut order_score = [0.0f64; 64];
            for i in 0..limit {
                self.lexicon.traditionalize_into(self.ranked_work.text(i), false, &mut self.tmp_trad);
                let bonus = if self.tmp_trad.is_empty() || (exact && self.ranked_work.consumed[i] != self.input.len()) {
                    0.0
                } else {
                    self.lexicon.join_bonus(&self.segment_context, &self.tmp_trad, true)
                };
                order_idx[i] = i;
                order_score[i] = -(i as f64) * 0.18 + if bonus >= 2.5 { bonus } else { 0.0 };
            }
            order_idx[..limit].sort_by(|&a, &b| {
                if order_score[a] > order_score[b] {
                    Ordering::Less
                } else if order_score[a] < order_score[b] {
                    Ordering::Greater
                } else {
                    Ordering::Equal
                }
            });

            self.reorder_buf.clear();
            for &orig in &order_idx[..limit] {
                self.reorder_buf.push(
                    self.ranked_work.text(orig),
                    self.ranked_work.consumed[orig],
                    self.ranked_work.kind[orig],
                    self.ranked_work.score_kind[orig],
                    self.ranked_work.score[orig],
                );
            }
            for i in limit..self.ranked_work.len() {
                self.reorder_buf.push(
                    self.ranked_work.text(i),
                    self.ranked_work.consumed[i],
                    self.ranked_work.kind[i],
                    self.ranked_work.score_kind[i],
                    self.ranked_work.score[i],
                );
            }
            std::mem::swap(&mut self.ranked_work, &mut self.reorder_buf);
        }

        self.offer_pinyin_corrections(Fuzz::Final);
        crate::row::shape(&self.lexicon, &self.input, self.hans, &mut self.ranked_work, &self.row_work, &mut self.reorder_buf);
        crate::latin::promote(&mut self.ranked_work, self.input.len());
        self.offer_pinyin_corrections(Fuzz::Initial);
        self.offer_pinyin_corrections(Fuzz::Rare);
        self.keep_latin_phrase_first();
        self.offer_key_edits();
        let whole = self.input.len();
        if self.lexicon.pinyin && (4..=8).contains(&whole) && !(0..self.ranked_work.len()).any(|i| {
            self.ranked_work.consumed[i] == whole && self.ranked_work.kind[i] == CandidateKind::Exact as u32
        }) {
            if let Some((text, score)) = dropped_letter_candidate(&self.lexicon, &self.input, self.hans) {
                if !(0..self.ranked_work.len()).any(|i| same_selection(&self.ranked_work, i, text, whole)) {
                    self.ranked_work.insert_at(3.min(self.ranked_work.len()), text, whole, CandidateKind::Correction as u32,
                                               CandidateScoreKind::Fallback as u32, score);
                    self.ranked_work.truncate(CANDIDATE_LIMIT);
                }
            }
        }
        self.candidates.clear();
        for i in 0..self.ranked_work.len() {
            if self.volunteered(i) {
                continue;
            }
            self.candidates.push(
                self.ranked_work.text(i),
                self.ranked_work.consumed[i],
                self.ranked_work.kind[i],
                self.ranked_work.score_kind[i],
                self.ranked_work.score[i],
            );
        }
        self.page = 0;
    }

    /// Whether candidate `i` of `ranked_work` would volunteer a word of the
    /// never-volunteer list (`FLAG_NO_SUGGEST`): the word itself for keys that
    /// are not its full spelling, a text holding one whose letters the keys do
    /// not have, or a typo or fuzzy-sound fix that holds one. The last check before the list is shown; the stages that make
    /// candidates leave such words out themselves.
    fn volunteered(&self, i: usize) -> bool {
        let text = self.ranked_work.text(i);
        let consumed = self.ranked_work.consumed[i].min(self.input.len());
        if self.lexicon.volunteers(&self.input[..consumed], text) {
            return true;
        }
        let score_kind = self.ranked_work.score_kind[i];
        (score_kind == CandidateScoreKind::Fallback as u32 || score_kind == CandidateScoreKind::FuzzyInitial as u32)
            && self.lexicon.mentions_no_suggest(text)
    }

    /// A Pinyin input with an English word ending in one bare initial
    /// (bangwochecky, yongpythonx): the words read up to that initial
    /// (帮我check) lead, not a whole read that guesses one character for the
    /// initial (帮我check有). The initial is the next syllable being typed,
    /// as for the `partial` guard of `offer_key_edits`. Abbreviations without
    /// English (wod: 我的) keep their lead.
    fn keep_latin_phrase_first(&mut self) {
        let r = &self.ranked_work;
        let whole = self.input.len();
        if !self.lexicon.pinyin || r.len() < 2 || r.consumed[0] != whole || r.kind[0] == CandidateKind::Partial as u32 {
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
        let lead = r.text(0).to_owned();
        let lead = lead.as_str();
        if !lead.bytes().any(|b| b.is_ascii_alphabetic()) {
            return;
        }
        let Some((cut, last)) = lead.char_indices().last() else { return };
        if strict_han_count(&lead[cut..cut + last.len_utf8()]) != 1 {
            return;
        }
        // A last word of more characters that the initial goes on (一下 for
        // ...chayix, 會議 for kaizoomhuiy) is a word being typed: it keeps its lead.
        if self.lead_word_goes_on(lead, n - initial) {
            return;
        }
        let phrase = &lead[..cut];
        let consumed = whole - initial;
        let Some(at) = (1..r.len().min(CANDIDATE_LIMIT)).find(|&i| r.consumed[i] == consumed && r.text(i) == phrase) else { return };
        let (t_off, t_len, cons, knd, sknd, scr) = remove_item_at(&mut self.ranked_work, at);
        let w = &mut self.ranked_work;
        w.text_offset.insert(0, t_off);
        w.text_len.insert(0, t_len);
        w.consumed.insert(0, cons);
        w.kind.insert(0, knd);
        w.score_kind.insert(0, sknd);
        w.score.insert(0, scr);
    }

    /// Whether `lead` ends with the heaviest lexicon word of two or more
    /// characters whose bare reading goes on from the typed letters `start..`,
    /// for some `start` before `before` (一下 of 用Google查一下 for ...chayix;
    /// not 會有 for ...huiy, where 會議 is heavier).
    fn lead_word_goes_on(&self, lead: &str, before: usize) -> bool {
        let lex = &self.lexicon;
        (before.saturating_sub(12)..before).any(|k| {
            let key = &self.input[k..];
            let mut heaviest: Option<(i32, &str)> = None;
            // Readings that go on from a syllable and an initial (buc: bucuo,
            // buchi, buchulai ...) run to a few hundred buckets.
            for bucket in (lex.lower_bound_bucket(key)..lex.buckets.len()).take(1024) {
                if !lex.buckets.key(&lex.storage, bucket).starts_with(key) {
                    break;
                }
                let (lo, hi) = lex.buckets.range(bucket);
                for i in lo..hi {
                    let word = lex.words.text(&lex.storage, i, self.hans);
                    let weight = lex.words.weight(i);
                    if strict_han_count(word) >= 2 && heaviest.is_none_or(|(w, _)| weight > w) {
                        heaviest = Some((weight, word));
                    }
                }
            }
            heaviest.is_some_and(|(_, word)| lead.ends_with(word))
        })
    }

    /// One wrong, missing, extra or swapped key: offer the fixed input's
    /// decode when it reads much better than the letters as typed. First
    /// when no word spells the typed letters, the fix is clearly stronger
    /// and the list does not already start with a guess or an abbreviation.
    fn offer_key_edits(&mut self) {
        if !self.lexicon.pinyin { return; }
        key_edit_corrections_into(&self.lexicon, &self.input, self.hans, &mut self.scratch, &mut self.reorder_buf);
        if self.reorder_buf.is_empty() { return; }
        let whole = self.input.len();
        // A one-edit guess of the decoder for the whole input (fallback.rs).
        let guessed = !self.ranked_work.is_empty() && self.ranked_work.consumed[0] == whole
            && self.ranked_work.kind[0] == CandidateKind::Correction as u32
            && self.ranked_work.score_kind[0] == CandidateScoreKind::Fallback as u32;
        // An abbreviation read of the whole input (woxhn: 我喜歡你) is not a
        // typo: a fix may follow it but not take its place.
        // Words read up to a syllable still being typed (今晚的月亮 for
        // ...yueliangki) keep the lead: the fix only guesses that last syllable.
        let partial = !self.ranked_work.is_empty() && self.ranked_work.consumed[0] < whole
            && self.ranked_work.kind[0] == CandidateKind::Partial as u32
            && self.ranked_work.score_kind[0] == CandidateScoreKind::None as u32;
        // Letters valid as typed are not a typo to lead with: a word (or
        // Japanese word of the session, taberu: 食べる) spelled as typed. A fix
        // may still follow at 2. Letters that are all words but for a filler
        // syllable (nidaoleema) are left to the fix: the edit search already
        // asks more of them (`COVERED_PENALTY`).
        let valid = has_exact_match(&self.lexicon, &self.input)
            || (0..self.japanese.len()).any(|j| self.japanese.reading(j) == self.input);
        let first = !guessed && !partial && !self.abbreviated_lead() && !valid;
        let mut position = 0;
        for i in 0..self.reorder_buf.len() {
            let gain = self.reorder_buf.score[i];
            let at = if first && gain >= EDIT_FIRST { position } else { position.max(if guessed { 1 } else { 2 }) };
            let at = at.min(self.ranked_work.len());
            let text = self.reorder_buf.text(i);
            if let Some(existing) = (0..self.ranked_work.len()).find(|&r| self.ranked_work.text(r) == text) {
                if existing <= at { position = existing + 1; continue; }
                // The same text for fewer keys stays (订了 for dingleu).
                if self.ranked_work.consumed[existing] != whole { continue; }
                remove_item_at(&mut self.ranked_work, existing);
            }
            self.ranked_work.insert_at(at, text, whole, self.reorder_buf.kind[i], self.reorder_buf.score_kind[i], self.reorder_buf.score[i]);
            position = at + 1;
        }
        self.ranked_work.truncate(CANDIDATE_LIMIT);
    }

    /// Score of the letters as typed: the best fully spelled whole word or
    /// cover with dictionary words (你是 for nishi), in nats.
    fn typed_score(&mut self) -> f64 {
        let reading = crate::reading::Reading::new(&self.input, true);
        let (lo, hi) = self.lexicon.range(reading.sounds());
        let word = (lo..hi).filter(|&i| {
            self.lexicon.words.is_fully_spelled(i)
                && reading.matches(self.lexicon.words.reading(&self.lexicon.storage, i).as_bytes(), 0, reading.len())
        }).map(|i| crate::merge::whole_word_score(&self.lexicon, i)).fold(f64::NEG_INFINITY, f64::max);
        word.max(typed_cover_score(&self.lexicon, &self.input, &mut self.scratch))
    }

    /// For a Zhuyin input ending with one first symbol after a tone: the
    /// characters the best whole-input reading gives the syllables before
    /// that symbol (你是什 of 你是什麼), with the keys they consume. A
    /// reading agrees when it spends exactly one Han character on the last
    /// symbol and one on each toned syllable before it; one that splits a
    /// syllable (該是為把 for ㄍㄞˉㄕㄨㄟˋㄅ) is skipped. The decode of the
    /// syllables alone (`segments_work[0]`) keeps the lead (None) when a whole
    /// reading starting with it scores within [`ZHUYIN_WHOLE_LEAD`] nats of the
    /// best one (等一下要 for 等一下要錢 against 等一下藥球), and for one
    /// syllable, where the characters alone (快 for ㄎㄨㄞˋㄐ) read better
    /// than the first one of a guessed word (會 of 會計).
    fn zhuyin_typed_prefix(&self) -> Option<(String, usize)> {
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
        // Path scores are nats; whole words (one syllable and a first
        // symbol: 時代 against 十點) carry their weight, 700 to a nat.
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
        Some((text[..cut].to_owned(), seg_consumed))
    }

    /// Whether a Zhuyin input ends with one first symbol typed right after a
    /// tone mark (ㄋㄧˇㄏㄠˇㄒ): the next syllable being typed, not an abbreviation.
    fn zhuyin_initial_after_tone(&self) -> bool {
        let reading = crate::reading::Reading::new(&self.input, false);
        let n = reading.len();
        n >= 2 && reading.boundaries[n - 1] != 0 && crate::abbrev::ends_abbreviated(&self.lexicon, &reading)
    }

    /// Whether the input ends with a bare initial (a first symbol in Zhuyin)
    /// and the first candidate is
    /// one word that reads the whole input with initials (qingch: 清楚, xians:
    /// 先生). Phrases for the keys before the last initials then come after
    /// it instead of before. A syllable still being typed (zhefe, zho) and a
    /// sentence guessed to its last initial (bangwoch) keep the phrase first.
    fn abbreviated_lead(&self) -> bool {
        let r = &self.ranked_work;
        let reading = crate::reading::Reading::new(&self.input, self.lexicon.pinyin);
        let sounds = reading.sounds_bytes();
        let n = sounds.len();
        let bare = if self.lexicon.pinyin {
            n > 0 && reading.boundaries[n] == 0 && (crate::abbrev::pinyin_initial(sounds, n - 1, n) || n > 1 && crate::abbrev::pinyin_initial(sounds, n - 2, n))
        } else {
            crate::abbrev::ends_abbreviated(&self.lexicon, &reading)
        };
        bare && !r.is_empty()
            && r.consumed[0] == self.input.len()
            && r.kind[0] == CandidateKind::Correction as u32
            && r.score_kind[0] == CandidateScoreKind::Lexicon as u32
    }

    fn offer_pinyin_corrections(&mut self, fuzz: Fuzz) {
        let initials = fuzz != Fuzz::Final;
        if initials && self.lexicon.pinyin && !self.ranked_work.is_empty()
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
        let words = fuzzy_corrections_into(&self.lexicon, &self.input, self.hans, &mut self.scratch, &mut self.reorder_buf, fuzz);
        if !self.reorder_buf.is_empty() {
            // A corrected word clearly heavier than the letters as typed comes
            // first (kafeiguang for 咖啡館 over 咖啡光, not nishi for 歷史 over
            // 你是), but only over a path read from those letters: not over a
            // word spelled as typed (qinchu keeps 禽畜, taiyan 台鹽), a completion
            // or a guess (a longer word being typed), and not for one syllable.
            let literal = !self.ranked_work.is_empty() && self.ranked_work.consumed[0] == self.input.len()
                && self.ranked_work.score_kind[0] != CandidateScoreKind::Lexicon as u32
                && [CandidateKind::Exact, CandidateKind::Path, CandidateKind::Unknown].iter().any(|&k| self.ranked_work.kind[0] == k as u32);
            let lead = words && literal && strict_han_count(self.reorder_buf.text(0)) > 1
                && self.reorder_buf.score[0] >= FUZZY_LEAD_FLOOR
                && self.reorder_buf.score[0] > self.typed_score() + FUZZY_LEAD;
            // A word of the never-volunteer list spelled as typed does not come
            // before an everyday word that sounds the same (chuanpu: 床鋪 before
            // 川普, liusi: 六十 before 六四); corrections are never such words.
            let whole = self.input.len();
            let listed_at = if words && strict_han_count(self.reorder_buf.text(0)) > 1 && self.reorder_buf.score[0] >= FUZZY_LEAD_FLOOR {
                (0..self.ranked_work.len().min(CANDIDATE_LIMIT)).find(|&r| {
                    self.ranked_work.consumed[r] == whole && self.lexicon.is_no_suggest_text(self.ranked_work.text(r))
                })
            } else {
                None
            };
            if fuzz == Fuzz::Rare && !lead && listed_at.is_none() { return; }
            // The exact candidates already have their context ranking. Put the
            // alternatives on the first page without moving a spelled leader.
            let spelled = |i: usize| {
                let kind = self.ranked_work.kind[i];
                let score_kind = self.ranked_work.score_kind[i];
                // A path can spell every syllable even when the lexicon
                // cannot verify a word's tones against its single characters.
                let unverified = kind == CandidateKind::Unknown as u32
                    && (score_kind == CandidateScoreKind::Lexicon as u32 || score_kind == CandidateScoreKind::Path as u32);
                self.ranked_work.consumed[i] == whole
                    && (kind == CandidateKind::Exact as u32 || kind == CandidateKind::Path as u32 || unverified)
            };
            let position = if lead {
                0
            } else if initials {
                // A confident literal or complete spelling keeps its lead.
                // Put one verified correction near it instead of filling the
                // row with homophones from the alternative syllable.
                2.min(self.ranked_work.len())
            } else if (0..self.ranked_work.len()).any(spelled) { 3.min(self.ranked_work.len()) } else { 0 };
            let mut position = listed_at.map_or(position, |at| position.min(at));
            let mut inserted = 0;
            for i in 0..self.reorder_buf.len() {
                let text = self.reorder_buf.text(i);
                if let Some(existing) = (0..self.ranked_work.len()).find(|&r| same_selection(&self.ranked_work, r, text, whole)) {
                    if existing < position || self.ranked_work.kind[existing] != CandidateKind::Correction as u32 { continue; }
                    remove_item_at(&mut self.ranked_work, existing);
                }
                self.ranked_work.insert_at(position, text, whole, self.reorder_buf.kind[i], self.reorder_buf.score_kind[i], self.reorder_buf.score[i]);
                position += 1;
                inserted += 1;
                if inserted == if initials { 1 } else { 2 } { break; }
            }
            self.ranked_work.truncate(CANDIDATE_LIMIT);
        }
    }
}
