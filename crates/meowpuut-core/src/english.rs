#![forbid(unsafe_code)]
//! English lexicon (`mimo_en.bin`, MIMO-EN-1, written by `scripts/mimo_en_binary.py`),
//! read in place: completion, correction and next-word prediction for the
//! English keyboard, the keypad (nine-key) spelling of English words, and the
//! second tier of English inside pinyin (see `latin.rs`).
//!
//! The layout is described in `scripts/mimo_en_binary.py`; [`pack`] writes the
//! same bytes from the same TSV files. Loading checks the header, the section
//! bounds and that every offset column is in order (one pass); everything else
//! is read on demand, and a damaged value gives a wrong word, never a read
//! outside the image. Nothing is allocated per lookup: [`Suggestions`] keeps
//! its buffers between calls.
//!
//! Word ids for the pair table: 0 the sentence start, 1 the end, row + 2 a word.
//!
//! # Examples
//!
//! ```
//! use meowpuut_core::english::{pack, English, Suggestions};
//! let words = "the\tthe\t1000\t-\nthey\tthey\t300\t-\nok\tOK\t50\tacronym,in-chinese\n";
//! let pairs = "<s>\tthe\t40\nthe\tthey\t2\n";
//! let image = pack(words, pairs, 10).unwrap();
//! let english = English::load(&image).unwrap();
//! let mut out = Suggestions::new();
//! english.complete("th", "", 0, &mut out);
//! assert_eq!(out.text(0), "the");
//! english.complete("Th", "", 0, &mut out);
//! assert_eq!(out.text(1), "They");
//! english.nine_key("65", "", 0, &mut out);
//! assert_eq!(out.text(0), "OK");
//! ```

use std::cmp::Ordering;

use crate::image::{Bytes, ImageOwner};

/// Magic header of `mimo_en.bin` (12 bytes).
pub const MAGIC: &[u8; 12] = b"MIMO-EN-1\0\0\0";
/// Layout version.
pub const VERSION: u32 = 1;
/// Never suggested: not a completion, correction, next word or Chinese candidate; typed in full it stays.
pub const NO_SUGGEST: u8 = 1;
/// Not offered on its own (reserved; no row has it yet).
pub const SOFT: u8 = 2;
/// Written inside Chinese text (DataSources/mimo-latin, Resources/Lexicon/latin.tsv).
pub const IN_CHINESE: u8 = 4;
/// Capital first letter (London).
pub const PROPER: u8 = 32;
/// All capitals (NASA, OK).
pub const ACRONYM: u8 = 64;
/// Capitals inside the word (iPhone).
pub const MIXED: u8 = 128;
/// Word id of the sentence start.
pub const START: u32 = 0;
/// Word id of the sentence end.
pub const END: u32 = 1;
/// Stored pair scores per nat.
pub const STEP: f64 = 64.0;
/// Longest key handled by the lookups (longer typed words get no suggestion).
pub const LONGEST: usize = 48;
/// Most suggestions one call gives.
pub const LIMIT: usize = 8;

/// `flags` of a typed request: the keyboard's shift is on (capital first letter).
pub const SHIFT: u32 = 1;
/// `flags` of a typed request: caps lock (all capitals).
pub const CAPS_LOCK: u32 = 2;

/// Kind of a suggestion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    /// The typed letters (or keys) spell the word.
    Exact = 1,
    /// The word starts with the typed letters.
    Completion = 2,
    /// The word is one or two edits away from the typed one.
    Correction = 3,
    /// A next word.
    Prediction = 4,
    /// Keypad only, when the keys spell no word that may be offered: the
    /// first letter of each key (4663 -> gmmd), no word and no guess.
    Letters = 5,
}

const HEADER: usize = 160;
const SECTIONS: usize = 13;
const FIRST_TOP: usize = 16;
const NONE: u32 = u32::MAX;
const NO_OFFER: u8 = NO_SUGGEST | SOFT;
const DIGITS: &[u8; 26] = b"22233344455566677778889999";
/// First letter of keys 2-9.
const FIRST_LETTERS: &[u8; 8] = b"adgjmptw";
/// Row of a suggestion that is no word ([`Kind::Letters`]).
const NO_ROW: u32 = u32::MAX;
/// Rows of a prefix scanned in full for completions; a larger range uses the stored first-letter lists.
const SCAN: usize = 6000;
/// Candidates kept while scanning.
const KEEP: usize = 16;
/// Score of one extra letter in a nine-key completion.
const NINE_KEY_EXTRA: f64 = 0.5;
/// Most extra letters of a nine-key completion.
const NINE_KEY_LONGER: usize = 4;

// Section numbers.
const KEY_OFFSETS: usize = 0;
const KEYS: usize = 1;
const SPELLING_OFFSETS: usize = 2;
const SPELLINGS: usize = 3;
const WEIGHTS: usize = 4;
const FLAGS: usize = 5;
const T9: usize = 6;
const FIRST: usize = 7;
const PAIR_OFFSETS: usize = 8;
const BACKOFF: usize = 9;
const PAIRS: usize = 10;
const NEXT_OFFSETS: usize = 11;
const NEXT: usize = 12;

/// The English lexicon, read in place or from an owned copy.
#[derive(Debug, Clone)]
pub struct English {
    data: Bytes,
    sections: [(usize, usize); SECTIONS],
    n: usize,
    fingerprint: u64,
    common_weight: u32,
    /// First row of each two-letter bucket (see [`bucket`]), made at load.
    buckets: [u32; BUCKETS + 1],
    _owner: Option<ImageOwner>,
}

impl English {
    /// Copies an image to the heap; `None` when it is not a MIMO-EN-1 image.
    pub fn load(bytes: &[u8]) -> Option<Self> {
        Self::parse(Bytes::owned(bytes.to_vec()), None)
    }

    /// Reads an image in place; `owner` is kept until the last clone is dropped.
    pub fn in_place(bytes: &'static [u8], owner: Option<ImageOwner>) -> Option<Self> {
        Self::parse(Bytes::mapped(bytes), owner)
    }

    fn parse(data: Bytes, owner: Option<ImageOwner>) -> Option<Self> {
        let bytes: &[u8] = &data;
        let u32_at = |at: usize| -> Option<u32> { Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?)) };
        if bytes.len() < HEADER || &bytes[..12] != MAGIC || u32_at(12)? != VERSION || u32_at(40)? as usize != SECTIONS {
            return None;
        }
        let n = u32_at(16)? as usize;
        let (p, q) = (u32_at(20)? as usize, u32_at(24)? as usize);
        let fingerprint = u64::from_le_bytes(bytes.get(28..36)?.try_into().ok()?);
        let common_weight = u32_at(36)?;
        let mut sections = [(0, 0); SECTIONS];
        for (i, section) in sections.iter_mut().enumerate() {
            let (offset, len) = (u32_at(44 + 8 * i)? as usize, u32_at(48 + 8 * i)? as usize);
            if offset < HEADER || offset.checked_add(len)? > bytes.len() {
                return None;
            }
            *section = (offset, len);
        }
        let sized = [
            (KEY_OFFSETS, (n + 1) * 4), (SPELLING_OFFSETS, (n + 1) * 4), (WEIGHTS, n * 2), (FLAGS, n), (T9, n * 4),
            (FIRST, 26 * FIRST_TOP * 4), (PAIR_OFFSETS, (n + 3) * 4), (BACKOFF, (n + 2) * 2), (PAIRS, p * 4),
            (NEXT_OFFSETS, (n + 3) * 4), (NEXT, q * 4),
        ];
        if n == 0 || sized.iter().any(|&(s, len)| sections[s].1 != len) {
            return None;
        }
        let mut english = Self { data, sections, n, fingerprint, common_weight, buckets: [0; BUCKETS + 1], _owner: owner };
        let in_order = |s: usize, count: usize, last: usize| {
            let mut previous = 0;
            for i in 0..count {
                let o = english.u32(s, i);
                if o < previous {
                    return false;
                }
                previous = o;
            }
            previous as usize == last
        };
        if !in_order(KEY_OFFSETS, n + 1, english.sections[KEYS].1)
            || !in_order(SPELLING_OFFSETS, n + 1, english.sections[SPELLINGS].1)
            || !in_order(PAIR_OFFSETS, n + 3, p)
            || !in_order(NEXT_OFFSETS, n + 3, q)
        {
            return None;
        }
        let mut buckets = [0u32; BUCKETS + 1];
        for (b, start) in buckets.iter_mut().enumerate() {
            *start = partition(n, |row| bucket(english.key(row)) < b) as u32;
        }
        english.buckets = buckets;
        Some(english)
    }

    #[inline]
    fn u32(&self, section: usize, i: usize) -> u32 {
        let at = self.sections[section].0 + 4 * i;
        self.data.get(at..at + 4).map_or(NONE, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    #[inline]
    fn slice(&self, section: usize, from: u32, to: u32) -> &[u8] {
        let (offset, len) = self.sections[section];
        let (from, to) = (from as usize, (to as usize).min(len));
        self.data.get(offset + from.min(to)..offset + to).unwrap_or(&[])
    }

    /// Number of words.
    #[inline]
    pub fn len(&self) -> usize {
        self.n
    }

    /// Always false for a loaded lexicon.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// FNV-1a 64 of the keys, spellings and weights (as written by the packer).
    pub fn fingerprint(&self) -> u64 {
        self.fingerprint
    }

    /// Heap bytes held (0 when read in place).
    pub fn heap_bytes(&self) -> usize {
        self.data.heap_bytes()
    }

    /// Key of a row: lower-case letters, no apostrophe or hyphen (dont, wifi).
    #[inline]
    pub fn key(&self, row: usize) -> &[u8] {
        self.slice(KEYS, self.u32(KEY_OFFSETS, row), self.u32(KEY_OFFSETS, row + 1))
    }

    /// Spelling of a row (don't, Wi-Fi, iPhone).
    pub fn spelling(&self, row: usize) -> &str {
        let (from, to) = (self.u32(SPELLING_OFFSETS, row), self.u32(SPELLING_OFFSETS, row + 1));
        let bytes = if to > from { self.slice(SPELLINGS, from, to) } else { self.key(row) };
        std::str::from_utf8(bytes).unwrap_or("")
    }

    /// Weight on the lexicon's scale (700 per nat).
    #[inline]
    pub fn weight(&self, row: usize) -> u16 {
        let at = self.sections[WEIGHTS].0 + 2 * row;
        self.data.get(at..at + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]))
    }

    /// `weight / 700 - 19`, the natural log of the word's share of the counted text.
    #[inline]
    pub fn score(&self, row: usize) -> f64 {
        f64::from(self.weight(row)) / 700.0 - 19.0
    }

    /// Flags of a row (`NO_SUGGEST`, `IN_CHINESE`, `PROPER`, ...).
    #[inline]
    pub fn flags(&self, row: usize) -> u8 {
        self.data.get(self.sections[FLAGS].0 + row).copied().unwrap_or(NO_SUGGEST)
    }

    /// True for one of the `COMMON` (30,000) heaviest rows.
    #[inline]
    pub fn is_common(&self, row: usize) -> bool {
        u32::from(self.weight(row)) >= self.common_weight
    }

    /// May be offered without being typed in full.
    #[inline]
    pub fn offered(&self, row: usize) -> bool {
        self.flags(row) & NO_OFFER == 0
    }

    /// True when `word` (any casing; apostrophes and hyphens ignored) is a
    /// `NO_SUGGEST` word: for a client's own spelling suggestions (iOS
    /// `UITextChecker`), which must not offer it either.
    pub fn is_blocked(&self, word: &str) -> bool {
        let mut key = [0u8; LONGEST];
        let Some(len) = key_of(word, &mut key) else { return false };
        let (lo, hi) = self.range(&key[..len]);
        (lo..hi).any(|row| self.flags(row) & NO_SUGGEST != 0)
    }

    /// Word id of a row for [`English::pair`].
    #[inline]
    pub fn id(row: usize) -> u32 {
        row as u32 + 2
    }

    fn lower_bound(&self, key: &[u8]) -> usize {
        if key.is_empty() {
            return 0;
        }
        // Rows of earlier buckets sort before `key`, rows of later ones after it.
        let b = bucket(key);
        let (lo, hi) = (self.buckets[b] as usize, (self.buckets[b + 1] as usize).max(self.buckets[b] as usize));
        lo + partition(hi.min(self.n) - lo.min(hi.min(self.n)), |i| self.key(lo + i) < key)
    }

    /// Rows whose key is `key`, heaviest first.
    pub fn range(&self, key: &[u8]) -> (usize, usize) {
        let lo = self.lower_bound(key);
        if lo >= self.n || self.key(lo) != key {
            return (lo, lo);
        }
        (lo, lo + partition(self.n - lo, |i| self.key(lo + i) == key))
    }

    /// True when some key starts with `prefix` (one binary search).
    fn has_prefix(&self, prefix: &[u8]) -> bool {
        let lo = self.lower_bound(prefix);
        lo < self.n && self.key(lo).starts_with(prefix)
    }

    /// Length of the longest prefix of `key` that some key starts with.
    fn live_prefix(&self, key: &[u8]) -> usize {
        (1..=key.len()).find(|&j| !self.has_prefix(&key[..j])).map_or(key.len(), |j| j - 1)
    }

    /// Rows whose key starts with `prefix`.
    pub fn prefix_range(&self, prefix: &[u8]) -> (usize, usize) {
        let lo = self.lower_bound(prefix);
        (lo, lo + partition(self.n - lo, |i| self.key(lo + i).starts_with(prefix)))
    }

    /// Positions in the nine-key order whose key's digits start with `digits`;
    /// [`English::nine_key_row`] gives the row at a position.
    pub fn nine_key_range(&self, digits: &[u8]) -> (usize, usize) {
        let lo = partition(self.n, |i| compare_digits(self.key(self.nine_key_row(i)), digits) == Ordering::Less);
        let hi = lo + partition(self.n - lo, |i| compare_digits(self.key(self.nine_key_row(lo + i)), digits) == Ordering::Equal);
        (lo, hi)
    }

    /// Row at a position of the nine-key order.
    #[inline]
    pub fn nine_key_row(&self, position: usize) -> usize {
        (self.u32(T9, position) as usize).min(self.n - 1)
    }

    /// Score of word `right` after word `left` (ids), in 1/64 nat.
    pub fn pair(&self, left: u32, right: u32) -> i32 {
        let g = left as usize;
        if g >= self.n + 2 {
            return 0;
        }
        let (mut lo, mut hi) = (self.u32(PAIR_OFFSETS, g) as usize, self.u32(PAIR_OFFSETS, g + 1) as usize);
        while lo < hi {
            let mid = (lo + hi) / 2;
            let pair = self.u32(PAIRS, mid);
            match (pair >> 12).cmp(&right) {
                Ordering::Less => lo = mid + 1,
                Ordering::Greater => hi = mid,
                Ordering::Equal => return ((pair << 20) as i32) >> 20,
            }
        }
        let at = self.sections[BACKOFF].0 + 2 * g;
        self.data.get(at..at + 2).map_or(0, |b| i32::from(i16::from_le_bytes([b[0], b[1]])))
    }

    /// The stored next words of `id` (at most eight rows, most frequent first; never `NO_SUGGEST`).
    pub fn next_words(&self, id: u32) -> impl Iterator<Item = usize> + '_ {
        let g = (id as usize).min(self.n + 2);
        let (lo, hi) = (self.u32(NEXT_OFFSETS, g) as usize, self.u32(NEXT_OFFSETS, g + 1) as usize);
        (lo..hi.max(lo)).map(|i| self.u32(NEXT, i) as usize).filter(|&row| row < self.n)
    }

    /// The heaviest suggestible rows whose key starts with `letter`.
    pub fn first_letter(&self, letter: u8) -> impl Iterator<Item = usize> + '_ {
        let l = usize::from(letter.wrapping_sub(b'a')).min(26);
        (0..if l < 26 { FIRST_TOP } else { 0 })
            .map(move |i| self.u32(FIRST, l * FIRST_TOP + i) as usize)
            .filter(|&row| row < self.n)
    }

    /// The row of a written word: the row with its spelling, else one whose
    /// spelling differs only in case, else the heaviest row of its key.
    /// The second value is true when the spelling matched (ignoring case).
    pub fn find(&self, word: &str) -> Option<(usize, bool)> {
        let mut key = [0u8; LONGEST];
        let len = key_of(word, &mut key)?;
        let (lo, hi) = self.range(&key[..len]);
        if lo == hi {
            return None;
        }
        if let Some(row) = (lo..hi).find(|&row| self.spelling(row) == word) {
            return Some((row, true));
        }
        if let Some(row) = (lo..hi).find(|&row| self.spelling(row).eq_ignore_ascii_case(word)) {
            return Some((row, true));
        }
        Some((lo, false))
    }

    /// Word id of the last word of `before` for the pair table: `START` at the
    /// start of a sentence, `None` when the last word is not in the lexicon
    /// (a number, an unknown name) or the text before it is not English.
    pub fn left_id(&self, before: &str) -> Option<u32> {
        match last_word(before) {
            Left::Start => Some(START),
            Left::Word(word) => self.find(word).map(|(row, _)| Self::id(row)),
            Left::Other => None,
        }
    }
}

/// Two-letter buckets: the first letter, then no second letter or one of 26.
const BUCKETS: usize = 26 * 27;

/// Bucket of a key (keys sort by bucket first).
#[inline]
fn bucket(key: &[u8]) -> usize {
    let letter = |b: u8| usize::from(b.wrapping_sub(b'a')).min(25);
    match key {
        [] => 0,
        [a] => letter(*a) * 27,
        [a, b, ..] => letter(*a) * 27 + 1 + letter(*b),
    }
}

/// First `i` in `0..n` for which `before(i)` is false (`before` holds for a prefix of the range).
fn partition(n: usize, before: impl Fn(usize) -> bool) -> usize {
    let (mut lo, mut hi) = (0, n);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if before(mid) { lo = mid + 1 } else { hi = mid }
    }
    lo
}

/// Compares the keypad digits of `key` with `digits`; `Equal` when they start with `digits`.
fn compare_digits(key: &[u8], digits: &[u8]) -> Ordering {
    for (i, &d) in digits.iter().enumerate() {
        let Some(&letter) = key.get(i) else { return Ordering::Less };
        let k = DIGITS.get(usize::from(letter.wrapping_sub(b'a'))).copied().unwrap_or(b'0');
        match k.cmp(&d) {
            Ordering::Equal => {}
            other => return other,
        }
    }
    Ordering::Equal
}

/// Keypad digit of a lower-case letter.
#[inline]
pub fn digit(letter: u8) -> u8 {
    DIGITS.get(usize::from(letter.to_ascii_lowercase().wrapping_sub(b'a'))).copied().unwrap_or(b'1')
}

/// The key of a written word (lower case, apostrophes and hyphens dropped) into `out`;
/// `None` for a word with another character or longer than [`LONGEST`].
pub fn key_of(word: &str, out: &mut [u8; LONGEST]) -> Option<usize> {
    let mut len = 0;
    for ch in word.chars() {
        match ch {
            'a'..='z' | 'A'..='Z' => {
                *out.get_mut(len)? = (ch as u8).to_ascii_lowercase();
                len += 1;
            }
            '\'' | '’' | '-' => {}
            _ => return None,
        }
    }
    (len > 0).then_some(len)
}

enum Left<'a> {
    Start,
    Word(&'a str),
    Other,
}

/// The last word of the text before the cursor, or the sentence start.
fn last_word(before: &str) -> Left<'_> {
    let trimmed = before.trim_end_matches([' ', '\t', '\n', '\r', '"', '“', '”', '(', ')']);
    match trimmed.chars().next_back() {
        None => return Left::Start,
        Some('.' | '!' | '?' | '…' | ';' | ':') => return Left::Start,
        _ => {}
    }
    let trimmed = trimmed.trim_end_matches([',']);
    let start = trimmed
        .char_indices()
        .rev()
        .find(|&(_, c)| !(c.is_ascii_alphabetic() || c == '\'' || c == '’' || c == '-'))
        .map_or(0, |(i, c)| i + c.len_utf8());
    let word = &trimmed[start..];
    if word.is_empty() { Left::Other } else { Left::Word(word) }
}

/// True when the next word starts a sentence (empty text, or after . ! ?).
pub fn at_sentence_start(before: &str) -> bool {
    let trimmed = before.trim_end_matches([' ', '\t', '\n', '\r', '"', '“', '”']);
    matches!(trimmed.chars().next_back(), None | Some('.' | '!' | '?' | '…'))
}

/// Casing asked for: as spelled, capital first letter, all capitals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Case {
    AsSpelled,
    Capital,
    Upper,
}

impl Case {
    /// The casing of typed letters: two or more all in capitals, a capital
    /// first letter, or neither; `flags` (`SHIFT`, `CAPS_LOCK`) add to it.
    pub fn of_typed(typed: &str, flags: u32) -> Self {
        let mut letters = typed.bytes().filter(u8::is_ascii_alphabetic);
        let first = letters.next();
        let count = typed.bytes().filter(u8::is_ascii_alphabetic).count();
        if flags & CAPS_LOCK != 0 || (count >= 2 && typed.bytes().filter(u8::is_ascii_alphabetic).all(|b| b.is_ascii_uppercase())) {
            Case::Upper
        } else if flags & SHIFT != 0 || first.is_some_and(|b| b.is_ascii_uppercase()) {
            Case::Capital
        } else {
            Case::AsSpelled
        }
    }
}

/// Writes `spelling` in `case`: a capital first letter leaves acronyms and
/// words with inner capitals (iPhone) as they are.
pub fn write_cased(spelling: &str, flags: u8, case: Case, out: &mut String) {
    match case {
        Case::AsSpelled => out.push_str(spelling),
        Case::Upper => out.extend(spelling.chars().map(|c| c.to_ascii_uppercase())),
        Case::Capital if flags & (ACRONYM | MIXED) != 0 => out.push_str(spelling),
        Case::Capital => {
            let mut chars = spelling.chars();
            if let Some(first) = chars.next() {
                out.push(first.to_ascii_uppercase());
                out.push_str(chars.as_str());
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Item {
    row: u32,
    score: f64,
    kind: Kind,
}

/// Best `KEEP` items by score, highest first, one per row; no allocation.
struct Best {
    items: [Item; KEEP],
    len: usize,
    /// Lookups [`English::offer`] may still make (the two-edit search is cut at [`TWO_EDITS`]).
    budget: usize,
}

impl Best {
    fn new() -> Self {
        Self { items: [Item { row: 0, score: 0.0, kind: Kind::Exact }; KEEP], len: 0, budget: usize::MAX }
    }

    fn push(&mut self, row: usize, score: f64, kind: Kind) {
        let row = row as u32;
        if let Some(same) = self.items[..self.len].iter().position(|i| i.row == row) {
            if self.items[same].score >= score {
                return;
            }
            self.items.copy_within(same + 1..self.len, same);
            self.len -= 1;
        }
        let at = self.items[..self.len].partition_point(|i| i.score >= score);
        if at >= KEEP {
            return;
        }
        let end = self.len.min(KEEP - 1);
        self.items.copy_within(at..end, at + 1);
        self.items[at] = Item { row, score, kind };
        self.len = (self.len + 1).min(KEEP);
    }

    fn as_slice(&self) -> &[Item] {
        &self.items[..self.len]
    }
}

/// Suggestions of one call, reused between calls (its buffers are made once).
#[derive(Debug)]
pub struct Suggestions {
    text: String,
    spans: Vec<(u32, u32)>,
    rows: Vec<u32>,
    kinds: Vec<Kind>,
    scores: Vec<f64>,
}

impl Default for Suggestions {
    fn default() -> Self {
        Self::new()
    }
}

impl Suggestions {
    /// Empty suggestions with room for [`LIMIT`] words.
    pub fn new() -> Self {
        Self {
            text: String::with_capacity(LIMIT * 4 * LONGEST),
            spans: Vec::with_capacity(LIMIT),
            rows: Vec::with_capacity(LIMIT),
            kinds: Vec::with_capacity(LIMIT),
            scores: Vec::with_capacity(LIMIT),
        }
    }

    fn clear(&mut self) {
        self.text.clear();
        self.spans.clear();
        self.rows.clear();
        self.kinds.clear();
        self.scores.clear();
    }

    /// Capacities of the buffers (unchanged by lookups once made).
    pub fn capacity_signature(&self) -> [usize; 5] {
        [self.text.capacity(), self.spans.capacity(), self.rows.capacity(), self.kinds.capacity(), self.scores.capacity()]
    }

    /// Number of suggestions.
    pub fn len(&self) -> usize {
        self.spans.len()
    }

    /// True when there is none.
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    /// Text of suggestion `i` ("" past the end).
    pub fn text(&self, i: usize) -> &str {
        self.spans.get(i).map_or("", |&(a, b)| &self.text[a as usize..b as usize])
    }

    /// Kind of suggestion `i`.
    pub fn kind(&self, i: usize) -> Option<Kind> {
        self.kinds.get(i).copied()
    }

    /// Score of suggestion `i` (log scale; higher is better).
    pub fn score(&self, i: usize) -> f64 {
        self.scores.get(i).copied().unwrap_or(f64::NEG_INFINITY)
    }

    /// Row of suggestion `i` (`None` for [`Kind::Letters`]).
    pub fn row(&self, i: usize) -> Option<usize> {
        self.rows.get(i).filter(|&&r| r != NO_ROW).map(|&r| r as usize)
    }

    /// The first letter of each keypad digit, as [`Kind::Letters`].
    fn push_letters(&mut self, digits: &[u8], case: Case) {
        if self.spans.len() >= LIMIT {
            return;
        }
        let mut letters = [0u8; LONGEST];
        let n = digits.len().min(LONGEST);
        for (out, &d) in letters.iter_mut().zip(&digits[..n]) {
            *out = FIRST_LETTERS.get(usize::from(d.wrapping_sub(b'2'))).copied().unwrap_or(b'a');
        }
        let start = self.text.len();
        write_cased(std::str::from_utf8(&letters[..n]).unwrap_or(""), 0, case, &mut self.text);
        self.spans.push((start as u32, self.text.len() as u32));
        self.rows.push(NO_ROW);
        self.kinds.push(Kind::Letters);
        self.scores.push(f64::NEG_INFINITY);
    }

    fn push(&mut self, english: &English, item: Item, case: Case, limit: usize) {
        if self.spans.len() >= limit.min(LIMIT) {
            return;
        }
        let start = self.text.len();
        let row = item.row as usize;
        write_cased(english.spelling(row), english.flags(row), case, &mut self.text);
        let end = self.text.len();
        if (0..self.spans.len()).any(|i| self.text(i) == &self.text[start..end]) {
            self.text.truncate(start);
            return;
        }
        self.spans.push((start as u32, end as u32));
        self.rows.push(item.row);
        self.kinds.push(item.kind);
        self.scores.push(item.score);
    }
}

/// Keys next to each letter on a QWERTY keyboard.
const NEIGHBOURS: [&[u8]; 26] = [
    b"qwsz", b"vghn", b"xdfv", b"serfcx", b"wsdr", b"drtgvc", b"ftyhbv", b"gyujnb", b"ujko", b"huikmn", b"jiolm", b"kop",
    b"njk", b"bhjm", b"iklp", b"ol", b"wa", b"edft", b"awedxz", b"rfgy", b"yhji", b"cfgb", b"qase", b"zsdc", b"tghu", b"asx",
];
const VOWELS: &[u8] = b"aeiouy";
/// Edit costs, in nats.
const SUBSTITUTE: f64 = 3.0;
const VOWEL: f64 = 3.5;
const SWAP: f64 = 3.0;
const DELETE: f64 = 3.5;
const DOUBLE: f64 = 2.0;
const INSERT: f64 = 4.0;
/// Cost of a word whose only difference is the apostrophe or capitals (dont -> don't).
const SAME_KEY: f64 = 0.5;
/// Most lookups of the two-edit correction search (each one binary search in a
/// two-letter bucket); keeps a long non-word under the 1 ms of spec 10.4.
const TWO_EDITS: usize = 2500;
/// Weight of the pair score with the word before.
const PAIR_WEIGHT: f64 = 1.0;

impl English {
    fn context_score(&self, left: Option<u32>, row: usize) -> f64 {
        let pair = left.map_or(0, |l| self.pair(l, Self::id(row)));
        self.score(row) + PAIR_WEIGHT * f64::from(pair) / STEP
    }

    /// Completions of `prefix` (as typed: its capitals set the casing), the
    /// typed word itself first when it is the likeliest; `before` is the text
    /// before the word (only read). Never a `NO_SUGGEST` word unless typed in full.
    pub fn complete(&self, prefix: &str, before: &str, flags: u32, out: &mut Suggestions) {
        out.clear();
        let mut key = [0u8; LONGEST];
        let Some(len) = key_of(prefix, &mut key) else { return };
        let key = &key[..len];
        let left = self.left_id(before);
        let case = Case::of_typed(prefix, flags);
        let mut best = Best::new();
        let (lo, hi) = self.prefix_range(key);
        if hi - lo <= SCAN {
            for row in lo..hi {
                let exact = self.key(row).len() == len;
                if exact || self.offered(row) {
                    let kind = if exact { Kind::Exact } else { Kind::Completion };
                    best.push(row, self.context_score(left, row), kind);
                }
            }
        } else {
            for row in self.first_letter(key[0]).filter(|&r| self.key(r).starts_with(key)) {
                let kind = if self.key(row).len() == len { Kind::Exact } else { Kind::Completion };
                best.push(row, self.context_score(left, row), kind);
            }
        }
        if let Some(l) = left {
            for row in self.next_words(l).filter(|&r| self.key(r).starts_with(key)) {
                let kind = if self.key(row).len() == len { Kind::Exact } else { Kind::Completion };
                best.push(row, self.context_score(left, row), kind);
            }
        }
        for &item in best.as_slice() {
            out.push(self, item, case, LIMIT);
        }
    }

    /// Corrections of a typed word that is not in the lexicon: words one edit
    /// away (a neighbouring key, a vowel, two letters swapped, a letter left
    /// out or typed twice), else two (two letters dropped, or a neighbouring
    /// key and one more edit). Nothing for a known word.
    pub fn correct(&self, word: &str, before: &str, flags: u32, out: &mut Suggestions) {
        out.clear();
        let mut key = [0u8; LONGEST];
        let Some(len) = key_of(word, &mut key) else { return };
        if len + 1 >= LONGEST {
            return;
        }
        let case = Case::of_typed(word, flags);
        let left = self.left_id(before);
        let (lo, hi) = self.range(&key[..len]);
        if (lo..hi).any(|row| self.spelling(row).eq_ignore_ascii_case(word)) {
            return;
        }
        let mut best = Best::new();
        for row in (lo..hi).filter(|&r| self.offered(r)) {
            best.push(row, self.context_score(left, row) - SAME_KEY, Kind::Correction);
        }
        if best.len == 0 {
            self.edits(&key[..len], 0.0, left, &mut best);
        }
        if best.len == 0 && len > 0 {
            // Two edits: two letters dropped (helllloo), or a neighbouring key and
            // one more edit; at most `TWO_EDITS` lookups (spec 10.4: under 1 ms).
            best.budget = TWO_EDITS;
            let mut first = [0u8; LONGEST];
            let typed = &key[..len];
            for i in 0..len {
                let deleted = remove(typed, i, &mut first);
                self.deletions(&first[..deleted], DELETE, left, &mut best);
            }
            // One edit is where the typed letters stop starting a word, or
            // before: try the neighbouring keys nearest that place first.
            let live = self.live_prefix(typed).min(len - 1);
            for step in 0..2 * len {
                let i = if step % 2 == 0 { live + step / 2 } else { live.wrapping_sub(step / 2 + 1) };
                if i >= len {
                    continue;
                }
                let letter = typed[i];
                let Some(near) = NEIGHBOURS.get(usize::from(letter.wrapping_sub(b'a'))) else { continue };
                for &other in near.iter() {
                    first[..len].copy_from_slice(typed);
                    first[i] = other;
                    self.edits(&first[..len], SUBSTITUTE, left, &mut best);
                }
            }
        }
        for &item in best.as_slice() {
            out.push(self, item, case, LIMIT);
        }
    }

    fn deletions(&self, key: &[u8], cost: f64, left: Option<u32>, best: &mut Best) {
        if best.budget == 0 || key.is_empty() {
            return;
        }
        let live = self.live_prefix(key);
        best.budget = best.budget.saturating_sub(live.min(key.len()) + 1);
        let mut buffer = [0u8; LONGEST];
        for i in 0..key.len().min(live + 1) {
            let len = remove(key, i, &mut buffer);
            self.offer(&buffer[..len], cost + DELETE, left, best);
        }
    }

    fn offer(&self, key: &[u8], cost: f64, left: Option<u32>, best: &mut Best) {
        if key.is_empty() || best.budget == 0 {
            return;
        }
        best.budget -= 1;
        let (lo, hi) = self.range(key);
        for row in (lo..hi).filter(|&r| self.offered(r)) {
            best.push(row, self.context_score(left, row) - cost, Kind::Correction);
        }
    }

    /// Offers every word one edit from `key` with `cost` already spent.
    fn edits(&self, key: &[u8], cost: f64, left: Option<u32>, best: &mut Best) {
        let len = key.len();
        if len + 1 >= LONGEST {
            return;
        }
        // Every edit at position `i` keeps `key[..i]`: none past the longest
        // prefix some word starts with can give a word.
        if best.budget == 0 {
            return;
        }
        let live = self.live_prefix(key);
        best.budget = best.budget.saturating_sub(live.min(len) + 1);
        let mut buffer = [0u8; LONGEST];
        for i in 0..len.min(live + 1) {
            // A letter typed once too many: cheaper when it doubles the one before.
            let n = remove(key, i, &mut buffer);
            let doubled = i > 0 && key[i - 1] == key[i];
            self.offer(&buffer[..n], cost + if doubled { DOUBLE } else { DELETE }, left, best);
            // A neighbouring key, or another vowel.
            let letter = key[i];
            buffer[..len].copy_from_slice(key);
            if let Some(near) = NEIGHBOURS.get(usize::from(letter.wrapping_sub(b'a'))) {
                for &other in near.iter() {
                    buffer[i] = other;
                    self.offer(&buffer[..len], cost + SUBSTITUTE, left, best);
                }
            }
            if VOWELS.contains(&letter) {
                for &other in VOWELS.iter().filter(|&&v| v != letter) {
                    if !NEIGHBOURS[usize::from(letter - b'a')].contains(&other) {
                        buffer[i] = other;
                        self.offer(&buffer[..len], cost + VOWEL, left, best);
                    }
                }
            }
            // Two letters swapped.
            if i + 1 < len && key[i] != key[i + 1] {
                buffer[..len].copy_from_slice(key);
                buffer.swap(i, i + 1);
                self.offer(&buffer[..len], cost + SWAP, left, best);
            }
        }
        // A letter left out: past the first two letters, only the letters some
        // word has after `key[..i]` (found one lookup each, in key order).
        for i in 0..=len.min(live) {
            buffer[..i].copy_from_slice(&key[..i]);
            buffer[i + 1..=len].copy_from_slice(&key[i..]);
            let mut letter = b'a';
            while letter <= b'z' {
                if i >= 2 {
                    match self.next_letter(&mut buffer, i, letter, best) {
                        Some(found) if (letter..=b'z').contains(&found) => letter = found,
                        _ => break,
                    }
                }
                buffer[i] = letter;
                let doubled = (i > 0 && key[i - 1] == letter) || key.get(i) == Some(&letter);
                self.offer(&buffer[..=len], cost + if doubled { DOUBLE } else { INSERT }, left, best);
                letter += 1;
            }
        }
    }

    /// The first letter `from` or later that some key has at `i` after
    /// `buffer[..i]` (one lookup, charged to the budget); `buffer[i]` is overwritten.
    fn next_letter(&self, buffer: &mut [u8; LONGEST], i: usize, from: u8, best: &mut Best) -> Option<u8> {
        if best.budget == 0 {
            return None;
        }
        best.budget -= 1;
        buffer[i] = from;
        let lo = self.lower_bound(&buffer[..=i]);
        let key = if lo < self.n { self.key(lo) } else { &[] };
        (key.len() > i && key[..i] == buffer[..i]).then(|| key[i])
    }

    /// Next words after `before` (the text before the cursor, only read):
    /// the stored followers of its last word, the sentence starters after
    /// . ! ? or at the start (with a capital first letter), or the commonest
    /// words when the last word is not in the lexicon.
    pub fn predict(&self, before: &str, flags: u32, out: &mut Suggestions) {
        out.clear();
        let start = at_sentence_start(before);
        let case = if flags & CAPS_LOCK != 0 { Case::Upper } else if start || flags & SHIFT != 0 { Case::Capital } else { Case::AsSpelled };
        let mut best = Best::new();
        match self.left_id(before) {
            Some(id) => {
                for (rank, row) in self.next_words(id).enumerate() {
                    best.push(row, -(rank as f64), Kind::Prediction);
                }
            }
            None => {
                for letter in b'a'..=b'z' {
                    for row in self.first_letter(letter).take(3) {
                        best.push(row, self.score(row), Kind::Prediction);
                    }
                }
            }
        }
        for &item in best.as_slice() {
            out.push(self, item, case, LIMIT);
        }
    }

    /// English words for keypad `digits` (65: OK, 4663: good, home, gone):
    /// the words the keys spell, by weight and the pair with the word before,
    /// then up to three longer words (four letters more at most). A
    /// `NO_SUGGEST` word only when the keys spell it, after the others and
    /// never first: when no other word fits, the first is the keys' first
    /// letters ([`Kind::Letters`]; also alone when nothing fits at all).
    pub fn nine_key(&self, digits: &str, before: &str, flags: u32, out: &mut Suggestions) {
        out.clear();
        let digits = digits.as_bytes();
        if digits.is_empty() || digits.len() >= LONGEST || !digits.iter().all(|d| (b'2'..=b'9').contains(d)) {
            return;
        }
        let left = self.left_id(before);
        let case = if flags & CAPS_LOCK != 0 { Case::Upper } else if flags & SHIFT != 0 || at_sentence_start(before) { Case::Capital } else { Case::AsSpelled };
        let mut exact = Best::new();
        let mut longer = Best::new();
        let mut blocked = Best::new();
        self.nine_key_rows(digits, left, &mut exact, &mut longer, &mut blocked);
        if exact.len == 0 && longer.len == 0 {
            out.push_letters(digits, case);
        }
        for &item in exact.as_slice() {
            out.push(self, item, case, LIMIT);
        }
        for &item in longer.as_slice().iter().take(3) {
            out.push(self, item, case, LIMIT);
        }
        for &item in blocked.as_slice() {
            out.push(self, item, case, LIMIT);
        }
    }

    fn nine_key_rows(&self, digits: &[u8], left: Option<u32>, exact: &mut Best, longer: &mut Best, blocked: &mut Best) {
        let (lo, hi) = self.nine_key_range(digits);
        let len = digits.len();
        // Heaviest first within one digit string; a long range is cut, the rest are lighter.
        for position in lo..hi.min(lo + SCAN) {
            let row = self.nine_key_row(position);
            let key_len = self.key(row).len();
            if key_len == len {
                if self.offered(row) {
                    exact.push(row, self.context_score(left, row), Kind::Exact);
                } else {
                    blocked.push(row, self.context_score(left, row), Kind::Exact);
                }
            } else if key_len <= len + NINE_KEY_LONGER && self.offered(row) {
                let score = self.context_score(left, row) - NINE_KEY_EXTRA * (key_len - len) as f64;
                longer.push(row, score, Kind::Completion);
            }
        }
    }

    /// Words for keypad `digits` inside Chinese nine-key typing (`nine_key/decoder.rs`):
    /// calls `each(row, score, exact)` for the words the keys spell (and, for
    /// three keys or more, up to `longer` words that start with them), never
    /// a `NO_SUGGEST` row.
    pub fn nine_key_words(&self, digits: &[u8], left: Option<u32>, longer: usize, mut each: impl FnMut(usize, f64, bool)) {
        if digits.is_empty() || digits.len() >= LONGEST {
            return;
        }
        let (mut exact, mut more, mut blocked) = (Best::new(), Best::new(), Best::new());
        self.nine_key_rows(digits, left, &mut exact, &mut more, &mut blocked);
        for item in exact.as_slice() {
            each(item.row as usize, item.score, true);
        }
        if digits.len() >= 3 {
            for item in more.as_slice().iter().take(longer) {
                each(item.row as usize, item.score, false);
            }
        }
    }
}

/// `key` without the letter at `i`, into `out`; returns the length.
fn remove(key: &[u8], i: usize, out: &mut [u8; LONGEST]) -> usize {
    out[..i].copy_from_slice(&key[..i]);
    out[i..key.len() - 1].copy_from_slice(&key[i + 1..]);
    key.len() - 1
}

// ---------------------------------------------------------------- packing

/// Rows kept by default (the iOS budget of the spec).
pub const PACK_LIMIT: usize = 50_000;
const COMMON: usize = 30_000;
const PREDICT: usize = 8;

fn fnv(data: &[u8], mut h: u64) -> u64 {
    for &b in data {
        h = (h ^ u64::from(b)).wrapping_mul(0x100_0000_01B3);
    }
    h
}

fn flag_bits(flags: &str) -> u8 {
    flags.split(',').fold(0, |bits, flag| {
        bits | match flag {
            "nosuggest" => NO_SUGGEST,
            "soft" => SOFT,
            "in-chinese" => IN_CHINESE,
            "proper" => PROPER,
            "acronym" => ACRONYM,
            "mixed" => MIXED,
            _ => 0,
        }
    })
}

fn rows_of(tsv: &str) -> impl Iterator<Item = Vec<&str>> {
    tsv.split('\n').filter(|l| !l.is_empty() && !l.starts_with('#')).map(|l| l.split('\t').collect())
}

/// Writes the MIMO-EN-1 image of `english` (english.tsv) and `pairs`
/// (pairs.tsv) with at most `limit` rows, byte for byte as
/// `scripts/mimo_en_binary.py` does. `None` for a malformed file.
pub fn pack(english: &str, pairs: &str, limit: usize) -> Option<Vec<u8>> {
    use std::collections::HashMap;
    struct Row<'a> {
        key: &'a str,
        spelling: &'a str,
        count: u64,
        flags: u8,
        weight: u16,
    }
    let mut words = Vec::new();
    for fields in rows_of(english) {
        let (key, spelling, count, flags) = (*fields.first()?, *fields.get(1)?, fields.get(2)?.parse::<u64>().ok()?, *fields.get(3)?);
        if key.is_empty() || !key.bytes().all(|b| b.is_ascii_lowercase()) || count == 0 {
            return None;
        }
        words.push(Row { key, spelling, count, flags: flag_bits(flags), weight: 0 });
    }
    let total: u64 = words.iter().map(|w| w.count).sum();
    let (mut kept, mut others): (Vec<Row>, Vec<Row>) = words.into_iter().partition(|w| w.flags & (NO_SUGGEST | IN_CHINESE) != 0);
    others.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.key.cmp(b.key)).then_with(|| a.spelling.cmp(b.spelling)));
    others.truncate(limit.saturating_sub(kept.len()));
    kept.extend(others);
    for row in &mut kept {
        let x = (700.0 * ((row.count as f64 / total as f64).ln() + 19.0) + 0.5).floor();
        row.weight = x.clamp(0.0, 65535.0) as u16;
    }
    kept.sort_by(|a, b| a.key.cmp(b.key).then_with(|| b.weight.cmp(&a.weight)).then_with(|| a.spelling.cmp(b.spelling)));
    let rows = kept;
    let n = rows.len();
    let row_of: HashMap<&str, usize> = rows.iter().enumerate().map(|(i, r)| (r.spelling, i)).collect();

    let (mut keys, mut key_offsets) = (Vec::new(), vec![0u32]);
    let (mut spellings, mut spelling_offsets) = (Vec::new(), vec![0u32]);
    for row in &rows {
        keys.extend_from_slice(row.key.as_bytes());
        key_offsets.push(keys.len() as u32);
        if row.spelling != row.key {
            spellings.extend_from_slice(row.spelling.as_bytes());
        }
        spelling_offsets.push(spellings.len() as u32);
    }
    let weights: Vec<u8> = rows.iter().flat_map(|r| r.weight.to_le_bytes()).collect();
    let flags: Vec<u8> = rows.iter().map(|r| r.flags).collect();
    let digit_key = |i: usize| -> Vec<u8> { rows[i].key.bytes().map(|b| DIGITS[usize::from(b - b'a')]).collect() };
    let mut t9: Vec<usize> = (0..n).collect();
    t9.sort_by_cached_key(|&i| (digit_key(i), std::cmp::Reverse(rows[i].weight), i));
    let mut first = Vec::with_capacity(26 * FIRST_TOP);
    for letter in b'a'..=b'z' {
        let mut best: Vec<usize> = (0..n).filter(|&i| rows[i].key.as_bytes()[0] == letter && rows[i].flags & NO_OFFER == 0).collect();
        best.sort_by_key(|&i| (std::cmp::Reverse(rows[i].weight), i));
        best.truncate(FIRST_TOP);
        first.extend(best.iter().map(|&i| i as u32));
        first.resize(first.len() + FIRST_TOP - best.len(), NONE);
    }
    let mut by_weight: Vec<u16> = rows.iter().map(|r| r.weight).collect();
    by_weight.sort_unstable_by(|a, b| b.cmp(a));
    let common = if n > 0 { u32::from(by_weight[COMMON.min(n) - 1]) } else { 0 };

    let pairs: Vec<(&str, &str, u64)> = rows_of(pairs)
        .map(|f| Some((*f.first()?, *f.get(1)?, f.get(2)?.parse::<u64>().ok()?)))
        .collect::<Option<_>>()?;
    let start_total: u64 = pairs.iter().filter(|p| p.0 == "<s>").map(|p| p.2).sum();
    let end_total: u64 = pairs.iter().filter(|p| p.1 == "</s>").map(|p| p.2).sum();
    let mut followers: Vec<Vec<(usize, u64)>> = vec![Vec::new(); n + 2];
    for &(left, right, count) in &pairs {
        let a = if left == "<s>" { Some(0) } else { row_of.get(left).map(|r| r + 2) };
        let b = if right == "</s>" { Some(1) } else { row_of.get(right).map(|r| r + 2) };
        if let (Some(a), Some(b)) = (a, b) {
            if a != 1 {
                followers[a].push((b, count));
            }
        }
    }
    let count_of = |id: usize, left: bool| -> u64 {
        match id {
            0 => if left { start_total } else { 0 },
            1 => if left { 0 } else { end_total },
            _ => rows[id - 2].count,
        }
    };
    let (mut pair_offsets, mut backoff, mut packed) = (vec![0u32], Vec::new(), Vec::new());
    let (mut next_offsets, mut next) = (vec![0u32], Vec::new());
    for (a, items) in followers.iter_mut().enumerate() {
        items.sort_unstable();
        let (mut sum_p, mut sum_q) = (0.0f64, 0.0f64);
        let left_count = count_of(a, true);
        for &(b, count) in items.iter() {
            let pba = if left_count > 0 { (count as f64 / left_count as f64).min(1.0) } else { 1.0 };
            let pb = count_of(b, false) as f64 / total as f64;
            let score = (0.8 * (pba / pb).ln() * 64.0 + 0.5).floor().clamp(-2048.0, 2047.0) as i32;
            packed.push(((b as u32) << 12) | (score as u32 & 0xFFF));
            sum_p += pba;
            sum_q += pb;
        }
        backoff.push(if items.is_empty() {
            0i16
        } else {
            let x = 0.8 * ((1.0 - sum_p).max(1e-3) / (1.0 - sum_q).max(1e-3)).ln() * 64.0;
            (x + 0.5).floor().clamp(-2048.0, 0.0) as i16
        });
        pair_offsets.push(packed.len() as u32);
        let mut best: Vec<(usize, u64)> = items.iter().copied().filter(|&(b, _)| b >= 2 && rows[b - 2].flags & NO_OFFER == 0).collect();
        best.sort_by(|x, y| y.1.cmp(&x.1).then(x.0.cmp(&y.0)));
        next.extend(best.iter().take(PREDICT).map(|&(b, _)| (b - 2) as u32));
        next_offsets.push(next.len() as u32);
    }

    let u32s = |v: &[u32]| -> Vec<u8> { v.iter().flat_map(|x| x.to_le_bytes()).collect() };
    let t9: Vec<u32> = t9.iter().map(|&i| i as u32).collect();
    let sections: [Vec<u8>; SECTIONS] = [
        u32s(&key_offsets), keys.clone(), u32s(&spelling_offsets), spellings.clone(), weights.clone(), flags,
        u32s(&t9), u32s(&first), u32s(&pair_offsets), backoff.iter().flat_map(|x| x.to_le_bytes()).collect(),
        u32s(&packed), u32s(&next_offsets), u32s(&next),
    ];
    let fingerprint = fnv(&weights, fnv(&spellings, fnv(&keys, 0xCBF2_9CE4_8422_2325)));
    let mut body = Vec::new();
    let mut table = Vec::new();
    for section in &sections {
        table.push(((HEADER + body.len()) as u32, section.len() as u32));
        body.extend_from_slice(section);
        body.resize(body.len().div_ceil(8) * 8, 0);
    }
    let mut out = MAGIC.to_vec();
    for v in [VERSION, n as u32, packed.len() as u32, next.len() as u32] {
        out.extend(v.to_le_bytes());
    }
    out.extend(fingerprint.to_le_bytes());
    out.extend(common.to_le_bytes());
    out.extend((SECTIONS as u32).to_le_bytes());
    for (offset, len) in table {
        out.extend(offset.to_le_bytes());
        out.extend(len.to_le_bytes());
    }
    out.resize(HEADER, 0);
    out.extend(body);
    Some(out)
}
