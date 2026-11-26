#![forbid(unsafe_code)]
//! Completions of `JapaneseQuery::lookup_extended` (`OPTION_COMPLETIONS`): words whose reading
//! continues the input, ranked by weight less a cost per kana still to type (unigram prefix
//! prediction, as in the dictionary predictor of Mozc).
//!
//! The kana of the input is split into a complete head and a pending tail of letters that do
//! not make a kana yet: `arig` gives あり and `g`, `sh` gives nothing and `sh`. A final single
//! `n` is pending too, since `kon` may go on as こんにちは or この. Keys are sorted by bytes, so
//! the keys that start with the normalised head are one range of the key table. With a pending
//! tail only the keys whose next kana can be spelled starting with those letters are read: each
//! such kana (from the romaji table, plus っ for a doubled consonant and ん for `n`) is a
//! sub-range of its own.

use super::{
    Hit, JapaneseLexicon, JapaneseQuery, JapaneseRow, Text, FLAG_COMPLETION, FLAG_PINYIN_OK, KIND_READING,
    KIND_VARIANT, MAX_POSTINGS_PER_KEY,
};
use crate::romaji::{kana_of_tokens_starting_with, normalise_key, to_kana};

/// Completions kept, at most.
pub(super) const MAX_COMPLETIONS: usize = 32;
/// Score bonus of a word or form of the whole input when completions are merged in.
pub(super) const WORD_BONUS: i32 = 1500;
/// Keys read for one lookup, at most.
const MAX_SCANNED_KEYS: usize = 8192;
/// Postings lighter than this are not offered as completions.
const MIN_WEIGHT: i16 = 2500;
/// Score deduction per kana still to type ...
const EXTRA_KANA_COST: i32 = 150;
/// ... for at most this many kana.
const MAX_EXTRA_KANA: usize = 6;
/// Spellings of the next kana for one pending tail, at most (`t` has twelve).
const MAX_NEXT: usize = 24;
/// Longest pending tail: the longest romaji token less its last letter.
const MAX_PENDING: usize = 3;

/// Number of characters in UTF-8 bytes.
fn char_count(bytes: &[u8]) -> usize {
    bytes.iter().filter(|&&byte| byte & 0xC0 != 0x80).count()
}

impl JapaneseQuery {
    /// Fills `extra` with the best completions of the input, best first.
    pub(super) fn add_completions(&mut self, lexicon: &JapaneseLexicon) {
        self.extra.clear();
        self.scratch.clear();
        to_kana(&self.input, &mut self.scratch);
        let split = self.scratch.find(|ch: char| ch.is_ascii()).unwrap_or(self.scratch.len());
        let (Some(mut head), Some(mut pending)) = (self.scratch.get(..split), self.scratch.get(split..)) else {
            return;
        };
        if pending.is_empty() && self.input.ends_with('n') && !self.input.ends_with("nn") {
            // A final single n gave ん; it may still become な, に and so on.
            head = head.strip_suffix('ん').unwrap_or(head);
            pending = "n";
        }
        if pending.len() > MAX_PENDING || !pending.bytes().all(|byte| byte.is_ascii_lowercase()) {
            return;
        }
        let head_kana = head.chars().count();
        if head_kana < 2 && (head_kana == 0 || pending.is_empty()) {
            return;
        }

        // Spellings of the next kana; none that another one starts, so the ranges do not overlap.
        let mut next = [""; MAX_NEXT];
        let mut count = 0;
        let mut add = |kana: &'static str| {
            let fresh = !next.iter().take(count).any(|&other| other == kana);
            if let (true, Some(slot)) = (fresh, next.get_mut(count)) {
                *slot = kana;
                count += 1;
            }
        };
        if pending.is_empty() {
            add("");
        } else {
            kana_of_tokens_starting_with(pending).for_each(&mut add);
            if pending.len() == 1 && !pending.starts_with(['a', 'i', 'u', 'e', 'o', 'n']) {
                add("っ");
            }
            if pending == "n" {
                add("ん");
            }
        }
        let spellings = next.get(..count).unwrap_or(&[]);

        self.prefix.clear();
        normalise_key(head, &mut self.prefix);
        let head_len = self.prefix.len();
        let head_chars = char_count(self.prefix.as_bytes());
        // The pending kana counts as typed.
        let typed_kana = head_chars + usize::from(!pending.is_empty());
        // The key of the whole input belongs to the exact lookup: the head itself, or the head
        // and ん for a final `n`.
        let whole_tail = match pending {
            "" => Some(""),
            "n" => Some("ん"),
            _ => None,
        };
        let consumed = self.input.len();
        let mut scanned = 0;
        for &spelling in spellings {
            if spellings.iter().any(|other| other.len() < spelling.len() && spelling.starts_with(other)) {
                continue;
            }
            self.prefix.truncate(head_len);
            self.prefix.push_str(spelling);
            let mut index = lexicon.lower_bound(self.prefix.as_bytes());
            while index < lexicon.key_count() && scanned < MAX_SCANNED_KEYS {
                scanned += 1;
                let Some(key) = lexicon.key_bytes(index) else { break };
                if !key.starts_with(self.prefix.as_bytes()) {
                    break;
                }
                let key_index = index;
                index += 1;
                if whole_tail.is_some_and(|tail| key.len() == head_len + tail.len() && key.ends_with(tail.as_bytes())) {
                    continue;
                }
                let still_to_type = char_count(key).saturating_sub(typed_kana).min(MAX_EXTRA_KANA);
                let deduction = EXTRA_KANA_COST * i32::try_from(still_to_type).unwrap_or(0);
                let Some(record) = lexicon.key_record(key_index) else { continue };
                for slot in 0..record.posting_count.min(MAX_POSTINGS_PER_KEY) {
                    let Some(posting) = record.first_posting.checked_add(slot).and_then(|at| lexicon.posting(at)) else {
                        break;
                    };
                    if posting.weight < MIN_WEIGHT || (posting.kind != KIND_READING && posting.kind != KIND_VARIANT) {
                        continue;
                    }
                    let Some(word) = lexicon.word_record(posting.word) else { continue };
                    if lexicon.blob_str(word.surface_offset, word.surface_len).is_none() {
                        continue;
                    }
                    let row = JapaneseRow {
                        word: posting.word,
                        weight: (i32::from(posting.weight) - deduction).max(1),
                        kind: posting.kind,
                        flags: FLAG_COMPLETION | (posting.flags & FLAG_PINYIN_OK),
                    };
                    let text = Text::Image { offset: word.surface_offset, len: word.surface_len };
                    keep_best(&mut self.extra, Hit { row, text, consumed });
                }
            }
        }
        let mut head_buf = [0u8; 192];
        let mut pend_buf = [0u8; 8];
        let (head_len, pend_len) = (head.len(), pending.len());
        if head_len <= head_buf.len() && pend_len <= pend_buf.len() {
            head_buf[..head_len].copy_from_slice(head.as_bytes());
            pend_buf[..pend_len].copy_from_slice(pending.as_bytes());
            let h = std::str::from_utf8(&head_buf[..head_len]).unwrap_or("");
            let p = std::str::from_utf8(&pend_buf[..pend_len]).unwrap_or("");
            self.add_sentence_endings(lexicon, h, p, consumed);
        }
    }

    /// Offers polite/clause sentence-end completions when the pending tail matches
    /// an unfinished auxiliary ending (`tabemas` -> `食べます`, `tenkides` -> `天気です`).
    fn add_sentence_endings(&mut self, lexicon: &JapaneseLexicon, head: &str, pending: &str, consumed: usize) {
        if pending.is_empty() || head.chars().count() < 2 {
            return;
        }
        // `(head_kana_suffix, pending_romaji_prefix, complete_ending_kana, penalty)`
        const ENDINGS: &[(&str, &str, &str, i32)] = &[
            ("ま", "s", "ます", 180),
            ("ま", "sh", "ました", 220),
            ("ま", "sh", "ましょう", 320),
            ("ま", "se", "ません", 220),
            ("まし", "t", "ました", 140),
            ("まし", "y", "ましょう", 180),
            ("ませ", "n", "ません", 140),
            ("で", "s", "です", 180),
            ("です", "n", "ですね", 160),
            ("です", "y", "ですよ", 220),
            ("です", "k", "ですか", 220),
            ("でし", "t", "でした", 160),
            ("し", "m", "します", 220),
            ("ござい", "m", "ございます", 220),
            ("ございま", "s", "ございます", 140),
            ("てい", "r", "ている", 240),
            ("てい", "m", "ています", 240),
        ];
        let base_hits = self.hits.len();
        for &(req_head, req_pend, full_kana, penalty) in ENDINGS {
            if !head.ends_with(req_head) || !req_pend.starts_with(pending) {
                continue;
            }
            let stem_head = &head[..head.len() - req_head.len()];
            self.scratch.clear();
            self.scratch.push_str(stem_head);
            self.scratch.push_str(full_kana);
            self.key.clear();
            normalise_key(&self.scratch, &mut self.key);
            let _ = self.add_forms(lexicon, super::JapaneseMode::Japanese, base_hits, consumed, 64);
            if self.hits.len() > base_hits {
                for idx in base_hits..self.hits.len() {
                    let mut hit = self.hits[idx];
                    hit.row.weight = (hit.row.weight - penalty).max(1);
                    hit.row.flags |= FLAG_COMPLETION;
                    keep_best(&mut self.extra, hit);
                }
                self.hits.truncate(base_hits);
            }
        }
    }
}

/// Inserts a completion into `list` (best first, equal scores in arrival order, at most
/// `MAX_COMPLETIONS`); a surface is kept once, with its best score.
fn keep_best(list: &mut Vec<Hit>, hit: Hit) {
    if let Some(at) = list.iter().position(|other| other.text == hit.text) {
        if list.get(at).is_some_and(|other| other.row.weight >= hit.row.weight) {
            return;
        }
        list.remove(at);
    }
    if list.len() >= MAX_COMPLETIONS && list.last().is_some_and(|last| last.row.weight >= hit.row.weight) {
        return;
    }
    let at = list.partition_point(|other| other.row.weight >= hit.row.weight);
    list.insert(at, hit);
    list.truncate(MAX_COMPLETIONS);
}
