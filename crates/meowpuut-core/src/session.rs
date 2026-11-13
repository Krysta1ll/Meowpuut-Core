#![forbid(unsafe_code)]
//! Stateful input session (`Session`) managing preallocated SoA candidate pools,
//! segment context ranking, key editing, and mixed Pinyin/Romaji queries.
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
//!     "# context\n",
//!     "你好\t600\n",
//! );
//! let lex = Arc::new(Lexicon::load(tsv).unwrap());
//! let mut session = Session::new(lex);
//! for b in b"nihao" {
//!     assert!(session.key(i32::from(*b)));
//! }
//! assert_eq!(session.preedit(), "nihao");
//! assert_eq!(session.candidate(0), "你好");
//! session.commit();
//! assert_eq!(session.committed(), "你好");
//! session.clear_commit();
//! assert_eq!(session.committed(), "");
//! ```

use std::sync::Arc;

use crate::lexicon::Lexicon;
use crate::mixed::{mixed_pinyin_into, mixed_pinyin_prefix_into};
use crate::nine_key::{NineKeyIndex, SEPARATOR as NINE_KEY_SEPARATOR};
use crate::reading::{
    is_tone, strict_han_count, zhuyin_symbol_for_key, CANDIDATE_LIMIT, KEYS, MAX_INPUT, PAGE_SIZE,
};
use crate::soa::{CandidatePoolSoA, JapaneseTableSoA};
use crate::decoder::last_word_id;
use crate::pairs::START;
use crate::workspace::{DecodeOutputSoA, DecoderScratchSoA};
use crate::CandidateInfo;

/// Stateful decoding session with preallocated SoA buffers.
#[derive(Debug, Clone)]
pub struct Session {
    pub(crate) lexicon: Arc<Lexicon>,
    pub(crate) nine_key: Option<NineKeyIndex>,
    pub(crate) input: String,
    pub(crate) preedit: String,
    pub(crate) committed: String,
    pub(crate) mixed_pinyin_buf: String,
    pub(crate) segment_context: String,
    pub(crate) tmp_text: String,
    pub(crate) tmp_remaining: String,
    pub(crate) tmp_trad: String,
    pub(crate) tmp_combined: String,
    pub(crate) japanese: JapaneseTableSoA,
    pub(crate) candidates: CandidatePoolSoA,
    pub(crate) ranked_work: DecodeOutputSoA,
    pub(crate) partial_work: DecodeOutputSoA,
    pub(crate) predictive_work: DecodeOutputSoA,
    pub(crate) segments_work: DecodeOutputSoA,
    pub(crate) reorder_buf: DecodeOutputSoA,
    /// Partial candidates for the leading syllables (row.rs).
    pub(crate) row_work: DecodeOutputSoA,
    pub(crate) scratch: DecoderScratchSoA,
    pub(crate) page: usize,
    pub(crate) hans: bool,
    /// Per-`refresh` memo of the complete-only decode: 0 unknown, 1 empty, 2 non-empty.
    pub(crate) complete_memo: u8,
    /// Word id of the last picked word and the segment context it ends, so
    /// that the same context set again (clients retype the rest after a
    /// pick) gives the decoder the same left word.
    pub(crate) picked_id: u32,
    pub(crate) picked_context: String,
}

impl Session {
    /// Creates a new `Session` with preallocated SoA buffers so the hot simulation loop
    /// performs zero heap allocations.
    pub fn new(lexicon: Arc<Lexicon>) -> Self {
        Self {
            lexicon,
            nine_key: None,
            input: String::with_capacity(MAX_INPUT + 8),
            preedit: String::with_capacity(MAX_INPUT * 3 + 8),
            committed: String::with_capacity(MAX_INPUT * 3 + 8),
            mixed_pinyin_buf: String::with_capacity(256),
            segment_context: String::with_capacity(64),
            tmp_text: String::with_capacity(128),
            tmp_remaining: String::with_capacity(MAX_INPUT + 8),
            tmp_trad: String::with_capacity(64),
            tmp_combined: String::with_capacity(128),
            japanese: JapaneseTableSoA::with_capacity(512, 8192),
            candidates: CandidatePoolSoA::with_capacity(CANDIDATE_LIMIT + 32),
            ranked_work: DecodeOutputSoA::with_capacity(CANDIDATE_LIMIT + 32),
            partial_work: DecodeOutputSoA::with_capacity(32),
            predictive_work: DecodeOutputSoA::with_capacity(32),
            segments_work: DecodeOutputSoA::with_capacity(32),
            reorder_buf: DecodeOutputSoA::with_capacity(CANDIDATE_LIMIT + 32),
            row_work: DecodeOutputSoA::with_capacity(CANDIDATE_LIMIT),
            scratch: DecoderScratchSoA::new(),
            page: 0,
            hans: false,
            complete_memo: 0,
            picked_id: START,
            picked_context: String::with_capacity(256),
        }
    }

    /// Creates a session taking the telephone keypad digits 2–9 as Pinyin/Romaji.
    pub fn new_nine_key(lexicon: Arc<Lexicon>) -> Option<Self> {
        if !lexicon.pinyin {
            return None;
        }
        let index = NineKeyIndex::new(&lexicon);
        let mut session = Self::new(lexicon);
        session.nine_key = Some(index);
        Some(session)
    }

    /// Returns a combined capacity signature of internal SoA buffers for zero-allocation testing.
    pub fn capacity_signature(&self) -> usize {
        self.input.capacity()
            + self.preedit.capacity()
            + self.committed.capacity()
            + self.candidates.arena.capacity()
            + self.candidates.score.capacity()
            + self.ranked_work.arena.capacity()
            + self.scratch.capacity_signature()
    }

    #[inline]
    pub fn reset(&mut self) {
        self.input.clear();
        self.preedit.clear();
        self.candidates.clear();
        self.segment_context.clear();
        self.page = 0;
        self.scratch.lattice.left = START;
    }

    #[inline]
    pub fn clear(&mut self) {
        self.reset();
        self.committed.clear();
    }

    #[inline]
    pub fn input(&self) -> &str {
        &self.input
    }

    #[inline]
    pub fn preedit(&self) -> &str {
        &self.preedit
    }

    #[inline]
    pub fn committed(&self) -> &str {
        &self.committed
    }

    #[inline]
    pub fn clear_commit(&mut self) {
        self.committed.clear();
    }

    #[inline]
    pub fn page(&self) -> usize {
        self.page
    }

    #[inline]
    pub fn candidate_count(&self) -> usize {
        self.candidates.len()
    }

    #[inline]
    pub fn candidate(&self, index: usize) -> &str {
        if index < self.candidates.len() {
            self.candidates.text(index)
        } else {
            ""
        }
    }

    #[inline]
    pub fn candidate_consumed(&self, index: usize) -> usize {
        if index < self.candidates.len() {
            self.candidates.consumed[index]
        } else {
            0
        }
    }

    #[inline]
    pub fn candidate_info(&self, index: usize) -> Option<CandidateInfo> {
        if index >= self.candidates.len() {
            return None;
        }
        let consumed = self.candidates.consumed[index] as u64;
        Some(CandidateInfo {
            kind: self.candidates.kind[index],
            score_kind: self.candidates.score_kind[index],
            consumed_bytes: consumed,
            remaining_offset_bytes: consumed,
            score: self.candidates.score[index],
        })
    }

    #[inline]
    pub fn segment_context(&self) -> &str {
        &self.segment_context
    }

    pub fn set_segment_context(&mut self, text: &str) {
        if text.len() <= 64 {
            self.lexicon.traditionalize_into(text, true, &mut self.tmp_trad);
            self.segment_context.clear();
            self.segment_context.push_str(&self.tmp_trad);
        } else {
            self.segment_context.clear();
        }
        let same = !self.segment_context.is_empty() && self.segment_context == self.picked_context;
        self.scratch.lattice.left = if same { self.picked_id } else { START };
        if !self.input.is_empty() {
            self.refresh();
        }
    }

    pub fn set_simplified(&mut self, simplified: bool) {
        if self.hans == simplified {
            return;
        }
        self.hans = simplified;
        self.refresh();
    }

    /// Registers a Japanese romaji/alias entry for mixed Pinyin/Romaji decoding.
    pub fn add_japanese_word(&mut self, reading: &str, text: &str, weight: i32) -> bool {
        if (weight < 4000 && weight != 3500) || self.japanese.len() >= 512 {
            return false;
        }
        if (reading.len() < 3 && !(reading == "au" && weight >= 4000))
            || reading.len() > 20
            || text.is_empty()
            || text.len() > 128
            || !reading.bytes().all(|c| (b'a'..=b'z').contains(&c))
        {
            return false;
        }
        let is_single_han = strict_han_count(text) == 1;
        if let Some(index) = &mut self.nine_key {
            index.add_japanese(reading, self.japanese.len(), weight);
        }
        self.japanese.push(reading, text, weight, is_single_han);
        true
    }

    /// Decodes a mixed Chinese-Pinyin + Japanese-Romaji reading.
    pub fn mixed_pinyin(&mut self, reading: &str, simplified: bool) -> &str {
        mixed_pinyin_into(
            &self.lexicon,
            &self.japanese,
            reading,
            simplified,
            &mut self.scratch,
            &mut self.mixed_pinyin_buf,
        );
        &self.mixed_pinyin_buf
    }

    /// Decodes a leading mixed Chinese-Pinyin + Japanese-Romaji prefix and returns `(text, consumed)`.
    pub fn mixed_pinyin_prefix(&mut self, reading: &str, simplified: bool) -> (&str, usize) {
        let consumed = mixed_pinyin_prefix_into(
            &self.lexicon,
            &self.japanese,
            reading,
            simplified,
            &mut self.scratch,
            &mut self.mixed_pinyin_buf,
        );
        (&self.mixed_pinyin_buf, consumed)
    }

    pub fn commit(&mut self) {
        if !self.candidates.is_empty() {
            let _ = self.choose(self.page * PAGE_SIZE);
            return;
        }
        self.committed.clear();
        self.append_literal_input();
        self.reset();
    }

    fn append_literal_input(&mut self) {
        if self.nine_key.is_some() {
            self.committed.extend(self.input.bytes().filter(|&key| key != NINE_KEY_SEPARATOR).map(char::from));
        } else {
            self.committed.push_str(&self.input);
        }
    }

    pub fn choose(&mut self, index: usize) -> bool {
        if index >= self.candidates.len() {
            return false;
        }
        self.tmp_text.clear();
        self.tmp_text.push_str(self.candidates.text(index));
        let consumed = self.candidates.consumed[index];
        let reading = &self.input[..consumed];
        let picked = if let Some(index) = &self.nine_key {
            index.last_word_id(&self.lexicon, reading.as_bytes(), &self.tmp_text, self.hans)
        } else {
            last_word_id(&self.lexicon, reading, &self.tmp_text, self.hans)
        };
        self.tmp_remaining.clear();
        self.tmp_remaining.push_str(&self.input[consumed..]);
        self.lexicon
            .traditionalize_into(&self.tmp_text, true, &mut self.tmp_trad);
        self.tmp_combined.clear();
        if !self.tmp_trad.is_empty() {
            self.tmp_combined.push_str(&self.segment_context);
            self.tmp_combined.push_str(&self.tmp_trad);
            self.lexicon
                .traditionalize_into(&self.tmp_combined, true, &mut self.tmp_trad);
        }
        self.reset();
        self.input.push_str(&self.tmp_remaining);
        if !self.input.is_empty() {
            self.segment_context.push_str(&self.tmp_trad);
            self.picked_id = picked;
            self.picked_context.clear();
            self.picked_context.push_str(&self.segment_context);
            self.scratch.lattice.left = picked;
        }
        self.refresh();
        self.committed.clear();
        self.committed.push_str(&self.tmp_text);
        true
    }

    /// Processes a single key event (ASCII `32..=126` or X11 editing keys `0xff08`, `0xff0d`, `0xff1b`, `0xff55`, `0xff56`).
    pub fn key(&mut self, key: i32) -> bool {
        if key == 0xff55 || key == 0xff56 {
            if self.input.is_empty() {
                return false;
            }
            if key == 0xff55 && self.page > 0 {
                self.page -= 1;
            }
            if key == 0xff56 && (self.page + 1) * PAGE_SIZE < self.candidates.len() {
                self.page += 1;
            }
            return true;
        }
        if key == 0xff1b && !self.input.is_empty() {
            self.clear();
            return true;
        }
        if key == 0xff08 && !self.input.is_empty() {
            self.input.pop();
            if self.input.is_empty() {
                self.segment_context.clear();
            }
            self.refresh();
            return true;
        }
        if key == 0xff0d && !self.input.is_empty() {
            self.commit();
            return true;
        }
        if !(32..=126).contains(&key) {
            return false;
        }
        let ch = key as u8;
        if self.nine_key.is_some() {
            if ch == b' ' && !self.input.is_empty() {
                let literal = self.candidates.is_empty();
                self.commit();
                if literal { self.committed.push(' '); }
                return true;
            }
            if ch == NINE_KEY_SEPARATOR {
                if !self.input.as_bytes().last().is_some_and(|b| (b'2'..=b'9').contains(b)) {
                    return false;
                }
                if self.input.len() == MAX_INPUT {
                    return true;
                }
            } else if !(b'2'..=b'9').contains(&ch) {
                return false;
            }
            if self.input.len() == MAX_INPUT {
                self.append_literal_input();
                self.reset();
            }
            self.input.push(char::from(ch));
            self.refresh();
            return true;
        }
        if self.lexicon.pinyin {
            if ch == b' ' && self.input.len() == MAX_INPUT {
                self.committed.push_str(&self.preedit);
                self.committed.push(' ');
                self.reset();
                return true;
            }
            if ch == b' ' && !self.input.is_empty() {
                self.commit();
                return true;
            }
            let boundary = is_tone(ch, true) || ch == b'\'';
            if !(b'a'..=b'z').contains(&ch) && !boundary {
                return false;
            }
            if boundary {
                let last = self.input.as_bytes().last().copied();
                if last.map_or(true, |l| is_tone(l, true) || l == b'\'') {
                    return false;
                }
            }
            if self.input.len() == MAX_INPUT {
                self.committed.push_str(&self.preedit);
                if boundary {
                    self.committed.push(ch as char);
                }
                self.reset();
                if boundary {
                    return true;
                }
            }
            self.input.push(ch as char);
            self.refresh();
            return true;
        }

        let boundary = is_tone(ch, false) || ch == b'\'';
        if !KEYS.contains(&ch) || (boundary && self.input.is_empty()) {
            return false;
        }
        if boundary {
            let last = *self.input.as_bytes().last().unwrap();
            if is_tone(last, false) || last == b'\'' {
                if ch == b' ' {
                    self.commit();
                }
                return true;
            }
        }
        if self.input.len() == MAX_INPUT {
            self.committed.push_str(&self.preedit);
            if boundary {
                if let Some(sym) = zhuyin_symbol_for_key(ch) {
                    self.committed.push_str(sym);
                }
            }
            self.reset();
            if boundary {
                return true;
            }
        }
        self.input.push(ch as char);
        self.refresh();
        true
    }
}
