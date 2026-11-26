#![forbid(unsafe_code)]
//! Zero-allocation SoA lattice decoder for nine-key Pinyin, initials, Japanese, and Latin.
//!
//! # Examples
//!
//! ```
//! use std::sync::Arc;
//! use meowpuut_core::{Lexicon, Session};
//!
//! let tsv = concat!(
//!     "MEOWPUUT-PINYIN-1\n",
//!     "gou\tgou4\t購\t购\t6200\n",
//!     "hou\thou4\t後\t后\t6000\n",
//!     "hua\thua4\t話\t话\t7368\n",
//!     "hua\thua2\t滑\t滑\t6372\n",
//!     "wang\twang3\t網\t网\t7200\n",
//!     "wang\twang3\t往\t往\t7057\n",
//!     "wanggou\twang3gou4\t網購\t网购\t6400\n",
//!     "wanghou\twang3hou4\t往後\t往后\t4818\n",
//! );
//! let lex = Arc::new(Lexicon::load(tsv).unwrap());
//! let mut session = Session::new_nine_key(lex).unwrap();
//! session.set_simplified(true);
//! for b in b"9264468482" {
//!     assert!(session.key(i32::from(*b)));
//! }
//! let all: Vec<_> = (0..session.candidate_count()).map(|i| session.candidate(i)).collect();
//! assert!(all.contains(&"往后滑"));
//! ```

use std::ops::Range;

use crate::lexicon::Lexicon;
use crate::merge::whole_word_score;
use crate::pairs::START;
use crate::soa::{CandidatePoolSoA, JapaneseTableSoA};
use crate::{CandidateKind, CandidateScoreKind};

use super::rank::ChoicesSoA;
use super::scratch::{NineKeyLatticeSoA, SpanWordsSoA, WordRef, NO_WORD_ID};
use super::{
    key_letters, key_matches, matches_breaks, signature_keys, KeyInputSoA, NineKeyIndex,
    NineKeyState,
};

const SPAN_WORDS: usize = 32;
const WHOLE_WORDS: usize = 160;
const INITIALS_TERM: f64 = -3.0;
const SINGLE_INITIAL_PENALTY: f64 = 1.2;
const TRAILING_SINGLE_VOWEL_PENALTY: f64 = 4.5;
pub(crate) const FIRST_PAGE_WEIGHT: i32 = 750;

#[inline]
pub(crate) fn word_score(weight: i32) -> f64 {
    f64::from(weight) / 700.0 - 19.0
}

#[inline]
pub(crate) fn initials_score(weight: i32) -> f64 {
    word_score(weight.saturating_mul(8)) + INITIALS_TERM
}

impl NineKeyState {
    #[allow(clippy::too_many_arguments)]
    pub fn decode(
        &mut self,
        lexicon: &Lexicon,
        japanese: &JapaneseTableSoA,
        raw_keys: &[u8],
        hans: bool,
        left: u32,
        out: &mut CandidatePoolSoA,
        preedit: &mut String,
    ) {
        out.clear();
        if raw_keys.is_empty() {
            return;
        }
        let keys = KeyInputSoA::from_raw(raw_keys);
        if keys.len == 0 {
            return;
        }
        self.scratch.words.clear();
        self.scratch.lattice.reset(keys.len, left);
        self.scratch.choices.clear();

        let ctx = DecodeCtx {
            index: &self.index,
            lexicon,
            japanese,
            keys: &keys,
            hans,
        };
        ctx.run(
            &mut self.scratch.lattice,
            &mut self.scratch.words,
            &mut self.scratch.choices,
            out,
            preedit,
        );
    }
}

pub(crate) struct DecodeCtx<'a> {
    pub(crate) index: &'a NineKeyIndex,
    pub(crate) lexicon: &'a Lexicon,
    pub(crate) japanese: &'a JapaneseTableSoA,
    pub(crate) keys: &'a KeyInputSoA,
    pub(crate) hans: bool,
}

impl<'a> DecodeCtx<'a> {
    fn run(
        &self,
        lattice: &mut NineKeyLatticeSoA,
        words: &mut SpanWordsSoA,
        choices: &mut ChoicesSoA,
        out: &mut CandidatePoolSoA,
        preedit: &mut String,
    ) {
        let length = self.keys.len;
        let longest = self.lexicon.longest.max(20);
        for start in 0..length {
            if lattice.is_pos_empty(start) {
                continue;
            }
            for end in (start + 1)..=(start + longest).min(length) {
                self.advance(start..end, lattice, words, choices);
            }
        }
        self.trailing_initials(lattice, words);
        self.whole_initials(words, choices);
        self.whole_latin(choices);
        self.two_initials(lattice, words, choices);
        choices.write_into(lattice, self.keys, self.lexicon, out, preedit, |t, c| {
            self.volunteers(t, c)
        });
    }

    fn volunteers(&self, text: &str, consumed: usize) -> bool {
        let lexicon = self.lexicon;
        if !lexicon.pinyin || lexicon.no_suggest().is_empty() {
            return false;
        }
        let slice = self.keys.slice();
        let end_d = (0..=slice.len())
            .find(|&d| self.keys.consumed(d) >= consumed)
            .unwrap_or(slice.len());
        let typed = &slice[..end_d];
        let keypad = |letters: &[u8], at: &[u8]| {
            letters.len() == at.len()
                && letters
                    .iter()
                    .zip(at)
                    .all(|(&l, &k)| l.is_ascii_lowercase() && key_matches(l, k))
        };
        lexicon.volunteers_by(
            text,
            |letters| keypad(letters, typed),
            |letters| typed.windows(letters.len()).any(|w| keypad(letters, w)),
        )
    }

    fn advance(
        &self,
        span: Range<usize>,
        lattice: &mut NineKeyLatticeSoA,
        words: &mut SpanWordsSoA,
        choices: &mut ChoicesSoA,
    ) {
        let slice = &self.keys.slice()[span.clone()];
        let hash = signature_keys(slice);
        let at_end = span.end == self.keys.len;
        let whole = span.start == 0 && at_end;
        let consumed = self.keys.consumed(span.end);
        let breaks = self.keys.internal_breaks(span.start, span.end);
        let limit = if whole { WHOLE_WORDS } else { SPAN_WORDS };
        self.chinese_words(span.clone(), hash, whole, at_end && slice.len() == 1, limit, words);
        let (pairs, trigrams) = (self.lexicon.pairs(), self.lexicon.trigrams());
        for k in 0..words.kept.len() {
            let w = words.word_ref(words.kept[k]);
            if whole {
                let c = choices.push_item(
                    w.text, w.reading, 0, 0, consumed, w.score, w.kind, CandidateScoreKind::Lexicon, w.common || !w.initials,
                );
                choices.complete.push(c);
            } else {
                if span.start == 0 && !w.initials {
                    let c = choices.push_item(
                        w.text, w.reading, 0, 0, consumed, w.score, CandidateKind::Partial, CandidateScoreKind::Lexicon, true,
                    );
                    choices.partial.push(c);
                }
                lattice.extend(span.start, span.end, w, pairs, trigrams, at_end);
            }
        }
        let minimum = if whole { 3 } else { 4 };
        if slice.len() < minimum || !breaks.is_empty() {
            return;
        }
        for row in self.index.japanese_rows(self.japanese, slice, hash) {
            let w = WordRef {
                text: self.japanese.text(row),
                reading: self.japanese.reading(row),
                tail: "",
                initials: false,
                common: false,
                score: word_score(self.japanese.weight[row]),
                id: NO_WORD_ID,
                kind: CandidateKind::Japanese,
            };
            if whole {
                let c = choices.push_item(
                    w.text, w.reading, 0, 0, consumed, w.score, w.kind, CandidateScoreKind::Lexicon, true,
                );
                choices.japanese.push(c);
            } else {
                lattice.extend(span.start, span.end, w, pairs, trigrams, at_end);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn chinese_words(
        &self,
        span: Range<usize>,
        hash: u32,
        whole: bool,
        initial: bool,
        limit: usize,
        words: &mut SpanWordsSoA,
    ) {
        let lexicon = self.lexicon;
        let breaks = self.keys.internal_breaks(span.start, span.end);
        let slice = &self.keys.slice()[span.clone()];
        words.clear();
        for (lo, hi) in self.index.chinese_rows(lexicon, slice, hash) {
            let rows = (lo..hi).filter(|&row| {
                (initial || lexicon.words.is_fully_spelled(row))
                    && (breaks.is_empty()
                        || matches_breaks(lexicon.words.reading(&lexicon.storage, row), span.start, breaks))
            });
            for row in rows.take(limit) {
                let spelled = lexicon.words.is_fully_spelled(row);
                let mut sc = if whole && slice.len() > 1 {
                    whole_word_score(lexicon, row)
                } else {
                    word_score(lexicon.words.weight(row))
                };
                if !spelled {
                    sc -= 5.0;
                } else if span.start > 0 && initial && slice.len() == 1 {
                    sc -= TRAILING_SINGLE_VOWEL_PENALTY;
                }
                let id = lexicon.pairs().map_or(START, |_| lexicon.index.word_ids.ids.get(row));
                let kind = if spelled { CandidateKind::Exact } else { CandidateKind::Correction };
                words.push(
                    lexicon.words.text(&lexicon.storage, row, self.hans),
                    lexicon.words.reading(&lexicon.storage, row),
                    false, false, sc, id, kind,
                );
            }
        }
        if slice.len() == 1 && breaks.is_empty() && (whole || (initial && span.start >= 2)) {
            let per_letter = if whole { 16 } else { 6 };
            for &letter in key_letters(slice[0]) {
                for row in lexicon
                    .initials(letter)
                    .filter(|&r| lexicon.words.is_fully_spelled(r) && lexicon.words.is_single_han_hant(r))
                    .take(per_letter)
                {
                    let full_read = lexicon.words.reading(&lexicon.storage, row);
                    let id = lexicon.pairs().map_or(START, |_| lexicon.index.word_ids.ids.get(row));
                    let (sc, is_init, read_slice) = if whole {
                        (word_score(lexicon.words.weight(row)) - SINGLE_INITIAL_PENALTY, false, full_read)
                    } else {
                        (word_score(lexicon.words.weight(row)) + INITIALS_TERM, true, &full_read[..1])
                    };
                    words.push(
                        lexicon.words.text(&lexicon.storage, row, self.hans),
                        read_slice, is_init, whole, sc, id, CandidateKind::Correction,
                    );
                }
            }
        }
        words.finish(limit);
    }
}
