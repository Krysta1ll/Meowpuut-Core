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
use crate::mixed::unfinished_japanese_prefix_into;
use crate::reading::{zhuyin_symbol_for_key, CANDIDATE_LIMIT, MAX_INPUT};
use crate::segments::{
    pinyin_segments_into, predictive_segments_into, unfinished_sentence_into, zhuyin_segments_into,
};
use crate::session::Session;
use crate::workspace::DecodeOutputSoA;
use crate::{CandidateKind, CandidateScoreKind};

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
        prepend_items(&mut self.ranked_work, &self.partial_work);

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
            if no_complete {
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
                    prepend_items(&mut self.ranked_work, &self.segments_work);
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

        crate::row::shape(&self.lexicon, &self.input, self.hans, &mut self.ranked_work, &self.row_work, &mut self.reorder_buf);
        crate::latin::promote(&mut self.ranked_work, self.input.len());
        self.candidates.clear();
        for i in 0..self.ranked_work.len() {
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
}
