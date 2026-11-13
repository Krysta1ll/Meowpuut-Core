//! Telephone keypad lookup over the Pinyin buckets and reviewed Japanese readings.

mod decoder;

use std::borrow::Cow;

use crate::lexicon::Lexicon;
use crate::pairs::START;
use crate::reading::is_tone;
use crate::soa::JapaneseTableSoA;

const DIGITS: &[u8; 26] = b"22233344455566677778889999";
pub(crate) const SEPARATOR: u8 = b'\'';

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
    DIGITS[usize::from(letter - b'a')]
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
    japanese: Vec<u64>,
}

impl NineKeyIndex {
    pub(crate) fn new(lexicon: &Lexicon) -> Self {
        let mut chinese = Vec::with_capacity(lexicon.buckets.len());
        for row in 0..lexicon.buckets.len() {
            let reading = lexicon.buckets.key(&lexicon.storage, row);
            let (lo, hi) = lexicon.buckets.range(row);
            if reading.bytes().all(|b| b.is_ascii_lowercase())
                && (reading.len() == 1 || (lo..hi).any(|i| lexicon.words.is_fully_spelled(i)))
            {
                chinese.push(entry(reading, row));
            }
        }
        chinese.sort_unstable();
        chinese.shrink_to_fit();
        Self { chinese, japanese: Vec::new() }
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
        }
        START
    }
}
