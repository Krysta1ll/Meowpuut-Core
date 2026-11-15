//! Telephone keypad lookup over the Pinyin buckets and reviewed Japanese readings.
//!
//! Besides spelled readings, the keys can be the initials of a word (99 for
//! x4z4, 現在) and the letters of an English word of the Latin list (65 for
//! OK). Both come from rows of the Pinyin lexicon: the initials-only
//! abbreviation rows and the upper-case Latin rows (see latin.rs).

mod decoder;

use std::borrow::Cow;

use crate::lexicon::Lexicon;
use crate::pairs::START;
use crate::reading::is_tone;
use crate::soa::JapaneseTableSoA;

const DIGITS: &[u8; 26] = b"22233344455566677778889999";
pub(crate) const SEPARATOR: u8 = b'\'';
/// Most initials typed for one abbreviated word (xzsj, 心臟衰竭).
pub(crate) const INITIALS: usize = 6;
/// Lightest abbreviation row indexed (an eighth of 4000); lighter ones are mostly names.
pub(crate) const INITIALS_WEIGHT: i32 = 500;
/// Rows read to find an abbreviated word by its text.
const INITIALS_ROWS: usize = 128;

struct KeyReading<'a> {
    digits: Cow<'a, [u8]>,
    breaks: Vec<usize>,
}

impl<'a> KeyReading<'a> {
    fn new(raw: &'a [u8]) -> Self {
        if !raw.contains(&SEPARATOR) {
            return Self { digits: raw.into(), breaks: Vec::new() };
        }
        let mut digits = Vec::with_capacity(raw.len());
        let mut breaks = Vec::new();
        for &key in raw {
            if key == SEPARATOR { breaks.push(digits.len()); }
            else { digits.push(key); }
        }
        Self { digits: digits.into(), breaks }
    }

    fn consumed(&self, end: usize) -> usize {
        end + self.breaks.partition_point(|&b| b <= end)
    }

    fn internal_breaks(&self, start: usize, end: usize) -> &[usize] {
        let first = self.breaks.partition_point(|&b| b <= start);
        let last = self.breaks.partition_point(|&b| b < end);
        &self.breaks[first..last]
    }
}

fn matches_breaks(reading: &str, start: usize, mut breaks: &[usize]) -> bool {
    if breaks.is_empty() { return true; }
    let mut position = start;
    for byte in reading.bytes() {
        if is_tone(byte, true) {
            if position == breaks[0] {
                breaks = &breaks[1..];
                if breaks.is_empty() { return true; }
            }
        } else if byte.is_ascii_lowercase() {
            position += 1;
        }
    }
    false
}

fn digit(letter: u8) -> u8 {
    DIGITS[usize::from(letter.to_ascii_lowercase() - b'a')]
}

/// True for a reading of one letter per syllable (x4z4), `syllables` long.
pub(crate) fn is_initials(reading: &str, syllables: usize) -> bool {
    let r = reading.as_bytes();
    r.len() == 2 * syllables && r.as_chunks::<2>().0.iter().all(|&[letter, tone]| letter.is_ascii_lowercase() && is_tone(tone, true))
}

fn signature(digits: impl Iterator<Item = u8>) -> u32 {
    digits.fold(2_166_136_261, |hash, digit| {
        (hash ^ u32::from(digit)).wrapping_mul(16_777_619)
    })
}

fn matches(reading: &str, keys: &[u8]) -> bool {
    reading.len() == keys.len() && reading.bytes().map(digit).eq(keys.iter().copied())
}

// A sorted signature and row number in eight bytes. Confirm the original
// spelling against the keys on a hit, including for long readings.
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

#[derive(Debug, Clone)]
pub(crate) struct NineKeyIndex {
    chinese: Vec<u64>,
    /// Initials-only abbreviation rows (x4z4), heaviest first for each signature.
    initials: Vec<u64>,
    /// Buckets of English words (IPHONE), by the digits of their letters.
    latin: Vec<u64>,
    japanese: Vec<u64>,
}

impl NineKeyIndex {
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
        // Grouped by signature like the others, so `lookup` finds them, but heaviest first.
        initials.sort_unstable_by_key(|&e| (e >> 32, std::cmp::Reverse(lexicon.words.weight(e as u32 as usize)), e));
        initials.shrink_to_fit();
        Self { chinese, initials, latin, japanese: Vec::new() }
    }

    pub(crate) fn add_japanese(&mut self, reading: &str, row: usize, weight: i32) {
        // The 3500-weight kanji aliases need corroboration, which digits alone cannot supply.
        if weight >= 4000 {
            let value = entry(reading, row);
            self.japanese.insert(self.japanese.partition_point(|&e| e < value), value);
        }
    }

    fn chinese_rows<'a>(
        &'a self,
        lexicon: &'a Lexicon,
        keys: &'a [u8],
        hash: u32,
    ) -> impl Iterator<Item = (usize, usize)> + 'a {
        lookup(&self.chinese, hash)
            .filter(move |&row| matches(lexicon.buckets.key(&lexicon.storage, row), keys))
            .map(|row| lexicon.buckets.range(row))
    }

    /// Initials-only abbreviation rows (x4z4) whose initials read as `keys`,
    /// heaviest first; never a `FLAG_NO_SUGGEST` word.
    pub(crate) fn initials_rows<'a>(&'a self, lexicon: &'a Lexicon, keys: &'a [u8]) -> impl Iterator<Item = usize> + 'a {
        lookup(&self.initials, signature(keys.iter().copied())).filter(move |&row| {
            if lexicon.words.is_no_suggest(row) {
                return false;
            }
            let reading = lexicon.words.reading(&lexicon.storage, row);
            reading.len() == 2 * keys.len() && reading.bytes().step_by(2).map(digit).eq(keys.iter().copied())
        })
    }

    /// Row ranges of the English words spelled by `keys`.
    fn latin_rows<'a>(&'a self, lexicon: &'a Lexicon, keys: &'a [u8]) -> impl Iterator<Item = (usize, usize)> + 'a {
        lookup(&self.latin, signature(keys.iter().copied()))
            .filter(move |&row| matches(lexicon.buckets.key(&lexicon.storage, row), keys))
            .map(|row| lexicon.buckets.range(row))
    }

    fn japanese_rows<'a>(
        &'a self,
        words: &'a JapaneseTableSoA,
        keys: &'a [u8],
        hash: u32,
    ) -> impl Iterator<Item = usize> + 'a {
        lookup(&self.japanese, hash).filter(move |&row| matches(words.reading(row), keys))
    }

    pub(crate) fn last_word_id(&self, lexicon: &Lexicon, keys: &[u8], text: &str, hans: bool) -> u32 {
        if lexicon.pairs().is_none() {
            return START;
        }
        let keys = KeyReading::new(keys);
        let end = keys.digits.len();
        for start in end.saturating_sub(lexicon.longest)..end {
            let suffix = &keys.digits[start..];
            let hash = signature(suffix.iter().copied());
            let breaks = keys.internal_breaks(start, end);
            for (lo, hi) in self.chinese_rows(lexicon, suffix, hash) {
                let last = (lo..hi).find(|&row| {
                    text.ends_with(lexicon.words.text(&lexicon.storage, row, hans))
                        && matches_breaks(lexicon.words.reading(&lexicon.storage, row), start, breaks)
                });
                if let Some(row) = last {
                    return lexicon.index.word_ids.ids.get(row);
                }
            }
            if suffix.len() <= INITIALS && breaks.is_empty() {
                let last = self.initials_rows(lexicon, suffix).take(INITIALS_ROWS)
                    .find(|&row| text.ends_with(lexicon.words.text(&lexicon.storage, row, hans)));
                if let Some(row) = last {
                    return lexicon.index.word_ids.ids.get(row);
                }
            }
        }
        START
    }
}
