#![forbid(unsafe_code)]
//! Telephone keypad (`2`–`9`) Pinyin/Romaji index, syllable-locked input parser,
//! and preallocated SoA state (`NineKeyState`).
//!
//! # Examples
//!
//! ```
//! use std::sync::Arc;
//! use meowpuut_core::{Lexicon, Session};
//!
//! let tsv = concat!(
//!     "MEOWPUUT-PINYIN-1\n",
//!     "shi\tshi4\t是\t是\t9500\n",
//!     "wo\two3\t我\t我\t9600\n",
//! );
//! let lex = Arc::new(Lexicon::load(tsv).unwrap());
//! let mut session = Session::new_nine_key(lex).unwrap();
//! assert!(session.key(i32::from(b'7')));
//! assert_eq!(session.candidate(0), "是");
//! ```

pub mod decoder;
pub mod initials;
pub mod rank;
pub mod scratch;
pub mod syllables;

use crate::lexicon::Lexicon;
use crate::pairs::START;
use crate::reading::{is_tone, MAX_INPUT};
use crate::soa::JapaneseTableSoA;

use self::scratch::NineKeyScratchSoA;

const DIGITS: &[u8; 26] = b"22233344455566677778889999";
const LOWER_LETTERS: &[u8; 26] = b"abcdefghijklmnopqrstuvwxyz";
/// Syllable separator (`'`) typed via the `分詞` key or after a locked syllable.
pub const SEPARATOR: u8 = b'\'';
/// Most initials typed for one abbreviated word (`xzsj`, 心臟衰竭).
pub const INITIALS: usize = 6;
/// Lightest abbreviation row indexed (an eighth of 4000); lighter ones are mostly names.
pub const INITIALS_WEIGHT: i32 = 500;
const INITIALS_ROWS: usize = 256;

/// Stack-allocated parser for raw nine-key input (`2`–`9`, locked `A`–`Z`, and `'`).
#[derive(Debug, Clone)]
pub struct KeyInputSoA {
    pub keys: [u8; MAX_INPUT],
    pub len: usize,
    pub breaks: [u16; MAX_INPUT],
    pub break_len: usize,
}

impl KeyInputSoA {
    #[inline]
    pub fn from_raw(raw: &[u8]) -> Self {
        let mut out = Self { keys: [0u8; MAX_INPUT], len: 0, breaks: [0u16; MAX_INPUT], break_len: 0 };
        for &byte in raw.iter().take(MAX_INPUT) {
            if byte == SEPARATOR {
                out.breaks[out.break_len] = out.len as u16;
                out.break_len += 1;
            } else {
                out.keys[out.len] = byte;
                out.len += 1;
            }
        }
        out
    }

    #[inline]
    pub fn slice(&self) -> &[u8] { &self.keys[..self.len] }
    #[inline]
    pub fn breaks_slice(&self) -> &[u16] { &self.breaks[..self.break_len] }
    #[inline]
    pub fn consumed(&self, end: usize) -> usize {
        end + self.breaks_slice().partition_point(|&b| usize::from(b) <= end)
    }
    #[inline]
    pub fn internal_breaks(&self, start: usize, end: usize) -> &[u16] {
        let b = self.breaks_slice();
        &b[b.partition_point(|&x| usize::from(x) <= start)..b.partition_point(|&x| usize::from(x) < end)]
    }
}

/// Maps an ASCII letter `a`–`z` / `A`–`Z` to its keypad digit `'2'`–`'9'`.
#[inline]
pub fn digit(letter: u8) -> u8 {
    DIGITS[usize::from(letter.to_ascii_lowercase() - b'a')]
}

/// Maps a nine-key input byte (`'2'`–`'9'` or locked `'A'`–`'Z'`) to its keypad digit.
#[inline]
pub fn key_digit(key: u8) -> u8 {
    if key.is_ascii_uppercase() { digit(key) } else { key }
}

/// Whether reading letter `letter` matches nine-key input byte `key`.
#[inline]
pub fn key_matches(letter: u8, key: u8) -> bool {
    let low = letter.to_ascii_lowercase();
    if key.is_ascii_uppercase() { low == key.to_ascii_lowercase() } else { digit(low) == key }
}

/// Lowercase letters represented by a keypad byte (`'7'` -> `b"pqrs"`, `'S'` -> `b"s"`).
#[inline]
pub fn key_letters(key: u8) -> &'static [u8] {
    if key.is_ascii_uppercase() {
        let idx = usize::from(key - b'A');
        return &LOWER_LETTERS[idx..idx + 1];
    }
    match key {
        b'2' => b"abc",
        b'3' => b"def",
        b'4' => b"ghi",
        b'5' => b"jkl",
        b'6' => b"mno",
        b'7' => b"pqrs",
        b'8' => b"tuv",
        b'9' => b"wxyz",
        _ => b"",
    }
}

pub(crate) fn matches_breaks(reading: &str, start: usize, mut breaks: &[u16]) -> bool {
    if breaks.is_empty() { return true; }
    let mut position = start;
    for byte in reading.bytes() {
        if is_tone(byte, true) {
            if position == usize::from(breaks[0]) {
                breaks = &breaks[1..];
                if breaks.is_empty() { return true; }
            }
        } else if byte.is_ascii_lowercase() {
            position += 1;
        }
    }
    false
}

pub(crate) fn is_initials(reading: &str, syllables: usize) -> bool {
    let r = reading.as_bytes();
    r.len() == 2 * syllables
        && r.as_chunks::<2>().0.iter().all(|&[letter, tone]| letter.is_ascii_lowercase() && is_tone(tone, true))
}

pub(crate) fn signature(digits: impl Iterator<Item = u8>) -> u32 {
    digits.fold(2_166_136_261, |hash, d| (hash ^ u32::from(d)).wrapping_mul(16_777_619))
}

#[inline]
pub(crate) fn signature_keys(keys: &[u8]) -> u32 {
    signature(keys.iter().map(|&k| key_digit(k)))
}

#[inline]
pub(crate) fn matches_keys(reading: &str, keys: &[u8]) -> bool {
    reading.len() == keys.len() && reading.bytes().zip(keys.iter().copied()).all(|(b, k)| key_matches(b, k))
}

fn entry(reading: &str, row: usize) -> u64 {
    (u64::from(signature(reading.bytes().map(digit))) << 32) | row as u64
}

fn lookup(index: &[u64], hash: u32) -> impl Iterator<Item = usize> + '_ {
    let first = u64::from(hash) << 32;
    index[index.partition_point(|&e| e < first)..]
        .iter()
        .take_while(move |&&e| e >> 32 == u64::from(hash))
        .map(|&e| e as u32 as usize)
}

/// Immutable sorted nine-key signature tables shared across all nine-key sessions of a [`Lexicon`].
#[derive(Debug, Clone)]
pub struct NineKeyBaseIndex {
    chinese: Vec<u64>,
    initials: Vec<u64>,
    latin: Vec<u64>,
}

impl NineKeyBaseIndex {
    pub(crate) fn new(lexicon: &Lexicon) -> Self {
        let mut chinese = Vec::with_capacity(lexicon.buckets.len());
        let mut initials = Vec::new();
        let mut latin = Vec::new();
        for row in 0..lexicon.buckets.len() {
            let reading = lexicon.buckets.key(&lexicon.storage, row);
            let (lo, hi) = lexicon.buckets.range(row);
            if reading.bytes().all(|b| b.is_ascii_lowercase()) {
                if reading.len() == 1 || (lo..hi).any(|i| lexicon.words.is_fully_spelled(i)) {
                    chinese.push(entry(reading, row));
                }
                if (2..=INITIALS).contains(&reading.len()) {
                    initials.extend((lo..hi)
                        .filter(|&i| lexicon.words.weight(i) >= INITIALS_WEIGHT
                            && !lexicon.words.is_no_suggest(i)
                            && is_initials(lexicon.words.reading(&lexicon.storage, i), reading.len()))
                        .map(|i| entry(reading, i)));
                }
            } else if lexicon.pinyin && reading.len() >= 2 && reading.bytes().all(|b| b.is_ascii_uppercase()) {
                latin.push(entry(reading, row));
            }
        }
        for index in [&mut chinese, &mut latin] {
            index.sort_unstable();
            index.shrink_to_fit();
        }
        initials.sort_unstable_by_key(|&e| (e >> 32, std::cmp::Reverse(lexicon.words.weight(e as u32 as usize)), e));
        initials.shrink_to_fit();
        Self { chinese, initials, latin }
    }
}

/// Precomputed sorted nine-key signature tables over a Pinyin [`Lexicon`].
/// The immutable lexicon tables (`chinese`, `initials`, `latin`) are shared via
/// [`Lexicon::nine_key_base`]; only the tiny session-local `japanese` list is owned per session.
#[derive(Debug, Clone)]
pub struct NineKeyIndex {
    base: std::sync::Arc<std::sync::OnceLock<NineKeyBaseIndex>>,
    japanese: Vec<u64>,
}

impl NineKeyIndex {
    pub fn new(lexicon: &Lexicon) -> Self {
        Self { base: lexicon.nine_key_base(), japanese: Vec::new() }
    }

    #[inline]
    fn base(&self) -> &NineKeyBaseIndex {
        self.base.get().expect("NineKeyBaseIndex initialized by Lexicon::nine_key_base")
    }

    pub fn add_japanese(&mut self, reading: &str, row: usize, weight: i32) {
        if weight >= 4000 {
            let value = entry(reading, row);
            self.japanese.insert(self.japanese.partition_point(|&e| e < value), value);
        }
    }

    pub(crate) fn chinese_bucket_rows<'a>(&'a self, lexicon: &'a Lexicon, keys: &'a [u8], hash: u32) -> impl Iterator<Item = usize> + 'a {
        lookup(&self.base().chinese, hash).filter(move |&row| matches_keys(lexicon.buckets.key(&lexicon.storage, row), keys))
    }

    pub(crate) fn chinese_rows<'a>(&'a self, lexicon: &'a Lexicon, keys: &'a [u8], hash: u32) -> impl Iterator<Item = (usize, usize)> + 'a {
        self.chinese_bucket_rows(lexicon, keys, hash).map(|row| lexicon.buckets.range(row))
    }

    pub(crate) fn initials_rows<'a>(&'a self, lexicon: &'a Lexicon, keys: &'a [u8]) -> impl Iterator<Item = usize> + 'a {
        lookup(&self.base().initials, signature_keys(keys)).filter(move |&row| {
            if lexicon.words.is_no_suggest(row) { return false; }
            let r = lexicon.words.reading(&lexicon.storage, row);
            r.len() == 2 * keys.len() && r.bytes().step_by(2).zip(keys.iter().copied()).all(|(b, k)| key_matches(b, k))
        })
    }

    pub(crate) fn latin_rows<'a>(&'a self, lexicon: &'a Lexicon, keys: &'a [u8]) -> impl Iterator<Item = (usize, usize)> + 'a {
        lookup(&self.base().latin, signature_keys(keys))
            .filter(move |&row| matches_keys(lexicon.buckets.key(&lexicon.storage, row), keys))
            .map(|row| lexicon.buckets.range(row))
    }

    pub(crate) fn japanese_rows<'a>(&'a self, words: &'a JapaneseTableSoA, keys: &'a [u8], hash: u32) -> impl Iterator<Item = usize> + 'a {
        lookup(&self.japanese, hash).filter(move |&row| matches_keys(words.reading(row), keys))
    }

    pub(crate) fn last_word_id(&self, lexicon: &Lexicon, raw: &[u8], text: &str, hans: bool) -> u32 {
        if lexicon.pairs().is_none() { return START; }
        let keys = KeyInputSoA::from_raw(raw);
        let end = keys.len;
        for start in end.saturating_sub(lexicon.longest)..end {
            let suffix = &keys.slice()[start..];
            let hash = signature_keys(suffix);
            let breaks = keys.internal_breaks(start, end);
            for (lo, hi) in self.chinese_rows(lexicon, suffix, hash) {
                let found = (lo..hi).find(|&row| {
                    text.ends_with(lexicon.words.text(&lexicon.storage, row, hans))
                        && matches_breaks(lexicon.words.reading(&lexicon.storage, row), start, breaks)
                });
                if let Some(row) = found { return lexicon.index.word_ids.ids.get(row); }
            }
            if suffix.len() == 1 && breaks.is_empty() {
                for &letter in key_letters(suffix[0]) {
                    if let Some(row) = lexicon.initials(letter).find(|&r| text.ends_with(lexicon.words.text(&lexicon.storage, r, hans))) {
                        return lexicon.index.word_ids.ids.get(row);
                    }
                }
            } else if suffix.len() <= INITIALS && breaks.is_empty() {
                if let Some(row) = self.initials_rows(lexicon, suffix).take(INITIALS_ROWS).find(|&r| text.ends_with(lexicon.words.text(&lexicon.storage, r, hans))) {
                    return lexicon.index.word_ids.ids.get(row);
                }
            }
        }
        START
    }
}

/// Session-owned nine-key index paired with preallocated SoA decoding buffers.
#[derive(Debug, Clone)]
pub struct NineKeyState {
    pub index: NineKeyIndex,
    pub scratch: NineKeyScratchSoA,
}

impl NineKeyState {
    pub fn new(lexicon: &Lexicon) -> Self {
        Self { index: NineKeyIndex::new(lexicon), scratch: NineKeyScratchSoA::new() }
    }
}

/// Replaces the raw keys in `preedit` with `reading` for the first `consumed` bytes in place.
pub fn show_reading(preedit: &mut String, reading: &str, consumed: usize, keys: &KeyInputSoA) {
    let raw = preedit.len();
    if !push_reading(preedit, reading, consumed, keys) {
        preedit.truncate(raw);
        return;
    }
    if consumed < raw {
        if !preedit.ends_with('\'') { preedit.push(' '); }
        preedit.extend_from_within(consumed..raw);
    }
    preedit.drain(..raw);
}

fn push_reading(out: &mut String, reading: &str, consumed: usize, keys: &KeyInputSoA) -> bool {
    let (mut position, mut boundary) = (0usize, false);
    let mut breaks = keys.breaks_slice().iter().copied().peekable();
    let slice = keys.slice();
    for byte in reading.bytes() {
        let byte = byte.to_ascii_lowercase();
        if byte.is_ascii_lowercase() {
            if !slice.get(position).is_some_and(|&k| key_matches(byte, k)) { return false; }
            if breaks.peek().copied().map(usize::from) == Some(position) {
                breaks.next();
                out.push('\'');
            } else if boundary {
                out.push(' ');
            }
            boundary = false;
            out.push(char::from(byte));
            position += 1;
        } else if is_tone(byte, true) || byte == b' ' || byte == b'\'' {
            boundary = true;
        }
    }
    if position == 0 || keys.consumed(position) != consumed { return false; }
    if breaks.peek().copied().map(usize::from) == Some(position) { out.push('\''); }
    true
}
