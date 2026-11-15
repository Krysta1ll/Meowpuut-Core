#![forbid(unsafe_code)]
//! Abbreviated input (簡拼): syllables typed as their first letter or symbol.
//!
//! Pinyin abbreviations of whole words are lexicon rows (zmb for 怎麼辦);
//! this module adds what rows cannot hold: a single bare initial (the d of
//! wod, read as 的, 到, ...) and, for Zhuyin, words typed as first symbols,
//! alone or after full syllables (ㄋㄏ for 你好, ㄨㄛˇㄒㄏ for 我喜歡). The
//! decoder scores the rows found here as words less `merge::MIXED_PENALTY`.
//!
//! # Examples
//!
//! ```
//! use meowpuut_core::abbrev::{abbreviated, zhuyin_from, ZhuyinSpans, SCAN_BUDGET};
//! use meowpuut_core::{Lexicon, Reading};
//!
//! // ㄋㄧˇ ㄏㄠˇ 你好, ㄋㄧˇ 你, ㄏㄠˇ 好.
//! let tsv = concat!(
//!     "MEOWPUUT-ZHUYIN-1\n",
//!     "cl\tcl3\t好\t好\t4800\n",
//!     "su\tsu3\t你\t你\t5000\n",
//!     "sucl\tsu3cl3\t你好\t你好\t5400\n",
//! );
//! let lex = Lexicon::load(tsv).unwrap();
//! let reading = Reading::new("sc", false); // ㄋㄏ
//! assert!(abbreviated(&lex, &reading));
//! let mut spans = ZhuyinSpans::default();
//! zhuyin_from(&lex, &reading, 0, &mut SCAN_BUDGET.clone(), &mut spans);
//! let rows = spans.words(2);
//! assert_eq!(rows.len(), 1);
//! assert_eq!(lex.words.text(&lex.storage, rows[0] as usize, false), "你好");
//! ```

use crate::lexicon::Lexicon;
use crate::reading::{is_tone, Reading, KEYS};

/// Rows kept per abbreviated span (the heaviest).
pub const ROWS: usize = crate::merge::MIXED_ROWS;
/// Syllables of an abbreviated Zhuyin word (as the lexicon's initials lists).
const MAX_UNITS: usize = 4;
/// Buckets read for abbreviated Zhuyin words per decode (`zhuyin_from`).
pub const SCAN_BUDGET: usize = 3072;
/// Abbreviated Zhuyin words start only in the last `WINDOW` sounds of the
/// input (a bare symbol before them is left as typed), so the work per
/// decode does not grow with the input.
pub const WINDOW: usize = 16;

/// Zhuyin consonants that are never a syllable of their own (ㄅ to ㄒ); typed
/// alone they can only be an initial. ㄓ to ㄙ follow and are syllables too.
const PURE_INITIALS: usize = 14;
const CONSONANTS: usize = 21;
const MEDIALS: core::ops::Range<usize> = 21..24;
const FINALS: core::ops::Range<usize> = 24..37;
const SYMBOLS: usize = 37;

/// For each Zhuyin symbol, which syllables that start with it have rows:
/// bit `medial * 14 + final`, where 0 is none and 1 on count the medials
/// (ㄧㄨㄩ) and the finals (ㄚ to ㄦ) in key order. Built once at load, so
/// a decode does not look the syllables up.
pub type SyllableTable = [u64; SYMBOLS];

/// Symbol after `first` in a syllable for medial slot `m` and final slot `f` (0: none).
fn syllable_key(first: u8, m: usize, f: usize, key: &mut [u8; 3]) -> usize {
    key[0] = first;
    let mut len = 1;
    if m > 0 {
        key[len] = KEYS[MEDIALS.start + m - 1];
        len += 1;
    }
    if f > 0 {
        key[len] = KEYS[FINALS.start + f - 1];
        len += 1;
    }
    len
}

/// The `SyllableTable` of a Zhuyin lexicon (all zero for Pinyin).
pub(crate) fn syllable_table(lexicon: &Lexicon) -> SyllableTable {
    let mut table = [0u64; SYMBOLS];
    if lexicon.pinyin {
        return table;
    }
    for (symbol, bits) in table.iter_mut().enumerate() {
        let c = class(KEYS[symbol]);
        for m in 0..=MEDIALS.len() {
            for f in 0..=FINALS.len() {
                // A medial only after a consonant, a final only after a consonant or medial.
                if m > 0 && c != Class::Consonant || f > 0 && c == Class::Final {
                    continue;
                }
                let mut key = [0u8; 3];
                let len = syllable_key(KEYS[symbol], m, f, &mut key);
                if std::str::from_utf8(&key[..len]).is_ok_and(|k| lexicon.range(k) != (0, 0)) {
                    *bits |= 1 << (m * (FINALS.len() + 1) + f);
                }
            }
        }
    }
    table
}

/// Whether the Zhuyin syllable `syllable` has rows (from the table when it
/// has the shape of one: a symbol, a medial, a final).
fn syllable_exists(lexicon: &Lexicon, syllable: &[u8]) -> bool {
    let position = |range: core::ops::Range<usize>, b: u8| KEYS[range.clone()].iter().position(|&k| k == b).map(|i| i + 1);
    if let Some(symbol) = syllable.first().and_then(|&b| KEYS[..SYMBOLS].iter().position(|&k| k == b)) {
        let rest = &syllable[1..];
        let (m, tail) = match rest.first().and_then(|&b| position(MEDIALS, b)) {
            Some(m) => (m, &rest[1..]),
            None => (0, rest),
        };
        let f = match tail {
            [] => Some(0),
            [b] => position(FINALS, *b),
            _ => None,
        };
        if let Some(f) = f {
            return lexicon.zhuyin_syllables[symbol] & (1 << (m * (FINALS.len() + 1) + f)) != 0;
        }
    }
    std::str::from_utf8(syllable).is_ok_and(|k| lexicon.range(k) != (0, 0))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Consonant,
    Medial,
    Final,
    Other,
}

fn class(key: u8) -> Class {
    match KEYS.iter().position(|&k| k == key) {
        Some(i) if i < CONSONANTS => Class::Consonant,
        Some(i) if MEDIALS.contains(&i) => Class::Medial,
        Some(i) if FINALS.contains(&i) => Class::Final,
        _ => Class::Other,
    }
}

/// One syllable of a Zhuyin span: its sounds `start..end`, the tone typed
/// after it (0 for none) and whether only its first symbol was typed.
#[derive(Clone, Copy, Default)]
struct Unit {
    start: u16,
    end: u16,
    tone: u8,
    prefix: bool,
}

/// Whether a Zhuyin syllable starts at sound `i` (a tone, a consonant, a
/// medial after no consonant, a final after a final).
fn starts_syllable(reading: &Reading, i: usize) -> bool {
    let s = reading.sounds_bytes();
    if i == 0 || i >= s.len() || reading.boundaries[i] != 0 {
        return true;
    }
    match (class(s[i - 1]), class(s[i])) {
        (_, Class::Consonant) => true,
        (Class::Consonant, Class::Medial) => false,
        (_, Class::Medial) => true,
        (Class::Consonant | Class::Medial, Class::Final) => false,
        _ => true,
    }
}

/// End of the Zhuyin syllable that starts at sound `at`. A consonant whose
/// symbols after it make no syllable (ㄈㄩ, ㄎㄧ) stands alone.
fn syllable_end(lexicon: &Lexicon, reading: &Reading, at: usize) -> usize {
    let mut next = at + 1;
    while next < reading.len() && !starts_syllable(reading, next) {
        next += 1;
    }
    let s = reading.sounds_bytes();
    if next > at + 1 && class(s[at]) == Class::Consonant && !syllable_exists(lexicon, &s[at..next]) {
        at + 1
    } else {
        next
    }
}

/// Whether a syllable starts at sound `i`, a consonant standing alone counted.
fn unit_start(lexicon: &Lexicon, reading: &Reading, i: usize) -> bool {
    starts_syllable(reading, i) || class(reading.sounds_bytes()[i - 1]) == Class::Consonant && syllable_end(lexicon, reading, i - 1) == i
}

/// Whether the input has a bare ㄅ to ㄒ in its last `WINDOW` sounds: one
/// typed alone, which only an abbreviation reads. Then every single symbol
/// typed without a tone is a first symbol too (ㄨ of ㄨㄉ for 我的);
/// otherwise the input is read as typed in full and keeps its lexicon rows.
pub fn abbreviated(lexicon: &Lexicon, reading: &Reading) -> bool {
    let s = reading.sounds_bytes();
    if lexicon.pinyin {
        return false;
    }
    (s.len().saturating_sub(WINDOW)..s.len()).any(|i| {
        KEYS[..PURE_INITIALS].contains(&s[i]) && syllable_end(lexicon, reading, i) == i + 1 && reading.boundaries[i + 1] == 0
    })
}

/// Whether the Zhuyin input is `abbreviated` and ends with a first symbol
/// (ㄋㄏ, ㄨㄛˇㄒ; not ㄋㄏㄇㄚ˙).
pub fn ends_abbreviated(lexicon: &Lexicon, reading: &Reading) -> bool {
    let n = reading.len();
    n > 0 && reading.boundaries[n] == 0 && unit_start(lexicon, reading, n - 1) && abbreviated(lexicon, reading)
}

/// Whether a Zhuyin word reading (ji32k7) has one syllable per unit: equal to
/// a full unit (and its tone, when one was typed), starting with a bare one.
/// `None` when the syllables differ, else the number of syllables (at most
/// `units.len()`; fewer when the reading is shorter).
fn fits(word: &[u8], sounds: &[u8], units: &[Unit]) -> Option<usize> {
    let mut k = 0usize;
    let mut from = 0usize;
    for (i, &b) in word.iter().enumerate() {
        if !is_tone(b, false) {
            continue;
        }
        let u = units.get(k)?;
        let syllable = &word[from..i];
        let typed = &sounds[usize::from(u.start)..usize::from(u.end)];
        let ok = if u.prefix {
            syllable.first() == typed.first()
        } else {
            syllable == typed && (u.tone == 0 || u.tone == b'\'' || u.tone == b)
        };
        if !ok {
            return None;
        }
        k += 1;
        from = i + 1;
    }
    (from == word.len()).then_some(k)
}

/// The `ROWS` heaviest rows with distinct texts, heaviest first.
#[derive(Clone, Copy)]
pub struct Best {
    rows: [u32; ROWS],
    weights: [i32; ROWS],
    count: usize,
}

impl Default for Best {
    fn default() -> Self {
        Self { rows: [0; ROWS], weights: [i32::MIN; ROWS], count: 0 }
    }
}

impl Best {
    /// The rows kept, heaviest first.
    pub fn rows(&self) -> &[u32] {
        &self.rows[..self.count]
    }

    /// Whether a row of `weight` would be kept.
    fn wants(&self, weight: i32) -> bool {
        self.count < ROWS || weight > self.weights[ROWS - 1]
    }

    fn offer(&mut self, lexicon: &Lexicon, row: usize, weight: i32) {
        let text = |r: usize| lexicon.words.text(&lexicon.storage, r, false);
        // Initials are never the full spelling of a never-volunteer word.
        if lexicon.words.is_no_suggest(row) || !self.wants(weight) || self.rows[..self.count].iter().any(|&r| text(r as usize) == text(row)) {
            return;
        }
        let mut at = self.count.min(ROWS - 1);
        while at > 0 && self.weights[at - 1] < weight {
            self.weights[at] = self.weights[at - 1];
            self.rows[at] = self.rows[at - 1];
            at -= 1;
        }
        self.weights[at] = weight;
        self.rows[at] = row as u32;
        self.count = (self.count + 1).min(ROWS);
    }
}

/// Abbreviated Zhuyin words that start at one sound, by the number of
/// syllables they span (`zhuyin_from`). Lives on the stack.
#[derive(Clone, Copy, Default)]
pub struct ZhuyinSpans {
    units: usize,
    ends: [u16; MAX_UNITS],
    best: [Best; MAX_UNITS],
}

impl ZhuyinSpans {
    /// Rows of the words that end at sound `end`, heaviest first.
    pub fn rows(&self, end: usize) -> &[u32] {
        (0..self.units).find(|&i| usize::from(self.ends[i]) == end).map_or(&[], |i| self.best[i].rows())
    }

    /// Like `rows`, for words of two syllables or more (an input of one bare
    /// symbol keeps its single-character list).
    pub fn words(&self, end: usize) -> &[u32] {
        if self.units > 0 && usize::from(self.ends[0]) == end { &[] } else { self.rows(end) }
    }
}

/// Finds the Zhuyin words that start at sound `start` and spell the next one
/// to `MAX_UNITS` syllables with at least one typed as its first symbol
/// (ㄋㄏ, ㄨㄛˇㄒㄏ, ㄨㄉ), keeping the heaviest per end. One bare symbol
/// gives the heaviest single characters that start with it (ㄉ: 的, 到, ...).
/// Only `start` in the last `WINDOW` sounds is tried, and `budget` caps the
/// buckets read and is shared by the calls of one decode, so a long input
/// stays fast. Only for an input that is `abbreviated` (checked once by the
/// caller). Nothing allocates: keys are built on the stack.
pub fn zhuyin_from(lexicon: &Lexicon, reading: &Reading, start: usize, budget: &mut usize, out: &mut ZhuyinSpans) {
    *out = ZhuyinSpans::default();
    if lexicon.pinyin || start >= reading.len() || start + WINDOW < reading.len() || !unit_start(lexicon, reading, start) {
        return;
    }
    let sounds = reading.sounds_bytes();
    let mut units = [Unit::default(); MAX_UNITS];
    let mut n = 0usize;
    let mut at = start;
    while at < sounds.len() && n < MAX_UNITS {
        if class(sounds[at]) == Class::Other {
            break;
        }
        let next = syllable_end(lexicon, reading, at);
        let tone = reading.boundaries[next];
        units[n] = Unit { start: at as u16, end: next as u16, tone, prefix: next == at + 1 && tone == 0 };
        out.ends[n] = next as u16;
        n += 1;
        at = next;
    }
    out.units = n;
    let units = &units[..n];
    // Words up to the first bare unit are lexicon rows; the first bare one
    // is tried as each syllable that starts with its symbol.
    let Some(k) = units.iter().position(|u| u.prefix) else { return };
    let symbol = sounds[usize::from(units[k].start)];
    if k == 0 {
        for row in lexicon.initials(symbol) {
            if lexicon.words.is_single_han_hant(row) {
                out.best[0].offer(lexicon, row, lexicon.words.weight(row));
            }
            if out.best[0].count == ROWS {
                break;
            }
        }
    }
    let fixed = &sounds[start..usize::from(units[k].start)];
    let Some(bits) = KEYS[..SYMBOLS].iter().position(|&b| b == symbol).map(|i| lexicon.zhuyin_syllables[i]) else { return };
    let mut key = [0u8; 64];
    for m in 0..=MEDIALS.len() {
        for f in 0..=FINALS.len() {
            // Only the syllables that have rows (syllable_table).
            if bits & (1 << (m * (FINALS.len() + 1) + f)) == 0 {
                continue;
            }
            let mut len = fixed.len();
            if len + 3 + 3 > key.len() {
                return;
            }
            key[..len].copy_from_slice(fixed);
            let mut syllable = [0u8; 3];
            let n = syllable_key(symbol, m, f, &mut syllable);
            key[len..len + n].copy_from_slice(&syllable[..n]);
            len += n;
            // Words that end with this syllable: one bucket.
            if k > 0 {
                if let Ok(whole) = std::str::from_utf8(&key[..len]) {
                    let (lo, hi) = lexicon.range(whole);
                    offer_rows(lexicon, lo..hi, sounds, units, &mut out.best);
                }
            }
            // Longer words: the buckets that go on with the next unit.
            let Some(u) = units.get(k + 1) else { continue };
            let next = if u.prefix { &sounds[usize::from(u.start)..=usize::from(u.start)] } else { &sounds[usize::from(u.start)..usize::from(u.end)] };
            if len + next.len() > key.len() {
                continue;
            }
            key[len..len + next.len()].copy_from_slice(next);
            len += next.len();
            let Ok(prefix) = std::str::from_utf8(&key[..len]) else { continue };
            let mut bucket = lexicon.lower_bound_bucket(prefix);
            while bucket < lexicon.buckets.len() {
                if *budget == 0 {
                    return;
                }
                *budget -= 1;
                let bucket_key = lexicon.buckets.key(&lexicon.storage, bucket);
                if !bucket_key.starts_with(prefix) {
                    break;
                }
                let (lo, hi) = lexicon.buckets.range(bucket);
                bucket += 1;
                offer_rows(lexicon, lo..hi, sounds, units, &mut out.best);
            }
        }
    }
}

/// Offers the rows `rows` (one bucket, heaviest first) that fit the first
/// units to the list of their syllable count.
fn offer_rows(lexicon: &Lexicon, rows: core::ops::Range<usize>, sounds: &[u8], units: &[Unit], best: &mut [Best; MAX_UNITS]) {
    for row in rows {
        let weight = lexicon.words.weight(row);
        if !best[1..units.len()].iter().any(|b| b.wants(weight)) {
            break;
        }
        if let Some(m @ 2..) = fits(lexicon.words.reading(&lexicon.storage, row).as_bytes(), sounds, units) {
            best[m - 1].offer(lexicon, row, weight);
        }
    }
}

/// Whether the Pinyin sounds `start..end` are a bare initial (b, d, zh, ...):
/// a consonant that starts no syllable here, followed by another consonant
/// or the end of the input, and not the end of the syllable before it
/// (n, ng, er).
pub fn pinyin_initial(sounds: &[u8], start: usize, end: usize) -> bool {
    let vowel = |c: u8| b"aeiouv".contains(&c);
    let Some(&c) = sounds.get(start) else { return false };
    if vowel(c) || !c.is_ascii_lowercase() || end > sounds.len() {
        return false;
    }
    let next = sounds.get(end).copied();
    let prev = start.checked_sub(1).map(|i| sounds[i]);
    let sibilant = b"zcs".contains(&c);
    let shape = match end - start {
        1 => !(sibilant && next == Some(b'h')),
        2 => sibilant && sounds[start + 1] == b'h',
        _ => false,
    };
    shape
        && next.is_none_or(|n| !vowel(n) && !(c == b'n' && n == b'g'))
        && !(c == b'n' && prev.is_some_and(vowel))
        && !(c == b'g' && prev == Some(b'n'))
        && !(c == b'r' && prev == Some(b'e'))
        && !(c == b'h' && end - start == 1 && prev.is_some_and(|p| b"zcs".contains(&p)))
}

/// Whether the Pinyin sound before `start` is a bare initial (or `start`
/// begins the input): `start` is inside a run of initials (jtqkdy).
pub fn after_initial(sounds: &[u8], start: usize) -> bool {
    start == 0 || pinyin_initial(sounds, start - 1, start) || start >= 2 && pinyin_initial(sounds, start - 2, start)
}

/// Whether the Pinyin sounds `start..end` are all bare initials (jtqk, zhd),
/// each one as `pinyin_initial` reads it: the span cannot be the end of a
/// spelled syllable before it (the n of henha) or the start of one after it.
pub fn pinyin_initials(sounds: &[u8], start: usize, end: usize) -> bool {
    let mut at = start;
    while at < end {
        let len = if at + 1 < end && sounds[at + 1] == b'h' && b"zcs".contains(&sounds[at]) { 2 } else { 1 };
        // Inside the span the next initial follows; at its end, what the input has.
        let next = if at + len < end { at + len + 1 } else { sounds.len() };
        if !pinyin_initial(&sounds[..next], at, at + len) {
            return false;
        }
        at += len;
    }
    start < end
}

/// Rows of the heaviest single characters whose Pinyin starts with the bare
/// initial `initial` (z also reads zh) and has the tone `tone` typed after
/// it (0 or an apostrophe for any), heaviest first; returns how many.
pub fn pinyin_initial_rows(lexicon: &Lexicon, initial: &[u8], tone: u8, rows: &mut [u32; ROWS]) -> usize {
    let mut best = Best::default();
    let Some(&first) = initial.first() else { return 0 };
    for row in lexicon.initials(first) {
        if lexicon.words.is_single_han_hant(row)
            && lexicon.words.is_fully_spelled(row)
            && lexicon.words.reading(&lexicon.storage, row).as_bytes().starts_with(initial)
            && (tone == 0 || tone == b'\'' || lexicon.words.reading(&lexicon.storage, row).as_bytes().last() == Some(&tone))
        {
            best.offer(lexicon, row, lexicon.words.weight(row));
        }
        if best.count == ROWS {
            break;
        }
    }
    rows[..best.count].copy_from_slice(best.rows());
    best.count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinyin_bare_initials() {
        let s = b"wod";
        assert!(pinyin_initial(s, 2, 3));
        assert!(!pinyin_initial(s, 0, 1)); // w starts wo
        let s = b"woxhn";
        assert!(pinyin_initial(s, 2, 3) && pinyin_initial(s, 3, 4) && pinyin_initial(s, 4, 5));
        let s = b"xian";
        assert!(!pinyin_initial(s, 3, 4)); // the n of xian
        let s = b"fang";
        assert!(!pinyin_initial(s, 2, 3) && !pinyin_initial(s, 3, 4));
        let s = b"zhd";
        assert!(pinyin_initial(s, 0, 2) && !pinyin_initial(s, 0, 1) && !pinyin_initial(s, 1, 2));
        let s = b"er";
        assert!(!pinyin_initial(s, 1, 2));
        assert!(pinyin_initials(b"jtqkdy", 0, 6) && pinyin_initials(b"jtqkdy", 2, 4));
        assert!(pinyin_initials(b"wobzd", 2, 5) && pinyin_initials(b"zhd", 0, 3));
        assert!(!pinyin_initials(b"henha", 2, 5)); // hen ha, not he nha
        assert!(!pinyin_initials(b"jtqkdya", 4, 6)); // y starts ya
        assert!(after_initial(b"jtqkdy", 2) && after_initial(b"zhd", 2) && after_initial(b"jt", 0));
        assert!(!after_initial(b"mail", 3) && !after_initial(b"xiand", 4));
    }

    fn zhuyin_lexicon() -> Lexicon {
        // ㄨㄛˇ 我, ㄉㄜ˙ 的, ㄉㄠˋ 到, ㄉㄧˇ 底, ㄨㄛˇㄉㄜ˙ 我的, ㄨㄛˋㄉㄧˇ 臥底, ㄨ 烏, ㄨˇ 五.
        Lexicon::load(concat!(
            "MEOWPUUT-ZHUYIN-1\n",
            "2k\t2k7\t的\t的\t9000\n",
            "2kji\t2k6ji3\t得我\t得我\t3000\n",
            "2l\t2l4\t到\t到\t7000\n",
            "2u\t2u3\t底\t底\t5000\n",
            "j\tj3\t五\t五\t6000\n",
            "j\tj \t烏\t乌\t3000\n",
            "ji\tji3\t我\t我\t9500\n",
            "ji2k\tji32k7\t我的\t我的\t7500\n",
            "ji2u\tji42u3\t臥底\t卧底\t5000\n",
        ))
        .unwrap()
    }

    fn texts(lex: &Lexicon, input: &str) -> Vec<String> {
        let reading = Reading::new(input, false);
        let mut spans = ZhuyinSpans::default();
        if abbreviated(lex, &reading) {
            zhuyin_from(lex, &reading, 0, &mut SCAN_BUDGET.clone(), &mut spans);
        }
        spans.rows(reading.len()).iter().map(|&r| lex.words.text(&lex.storage, r as usize, false).to_owned()).collect()
    }

    #[test]
    fn syllable_table_matches_lookups() {
        let lex = zhuyin_lexicon();
        let mut key = [0u8; 3];
        for symbol in 0..SYMBOLS {
            for m in 0..=MEDIALS.len() {
                for f in 0..=FINALS.len() {
                    let len = syllable_key(KEYS[symbol], m, f, &mut key);
                    let looked_up = lex.range(std::str::from_utf8(&key[..len]).unwrap()) != (0, 0);
                    let allowed = !(m > 0 && class(KEYS[symbol]) != Class::Consonant || f > 0 && class(KEYS[symbol]) == Class::Final);
                    assert_eq!(syllable_exists(&lex, &key[..len]), looked_up && allowed);
                }
            }
        }
        assert!(syllable_exists(&lex, b"ji") && syllable_exists(&lex, b"2k") && !syllable_exists(&lex, b"2j"));
    }

    #[test]
    fn zhuyin_window() {
        let lex = zhuyin_lexicon();
        // ㄉ then 16 full syllables ㄨ: the bare ㄉ is before the window.
        let far = format!("2{}", "j3".repeat(WINDOW));
        assert!(!abbreviated(&lex, &Reading::new(&far, false)));
        let near = format!("2{}", "j3".repeat(WINDOW - 1));
        assert!(abbreviated(&lex, &Reading::new(&near, false)));
        // ends_abbreviated: the last unit is a first symbol.
        assert!(ends_abbreviated(&lex, &Reading::new("ji2", false))); // ㄨㄛㄉ
        assert!(!ends_abbreviated(&lex, &Reading::new("2ji", false))); // ㄉㄨㄛ: ends with a syllable
        assert!(!ends_abbreviated(&lex, &Reading::new("ji2k7", false))); // ㄨㄛㄉㄜ˙: typed in full
    }

    #[test]
    fn pinyin_initial_rows_by_tone() {
        let lex = Lexicon::load(concat!(
            "MEOWPUUT-PINYIN-1\n",
            "dao\tdao4\t到\t到\t7000\n",
            "de\tde5\t的\t的\t9000\n",
            "dui\tdui4\t對\t对\t6000\n",
        ))
        .unwrap();
        let text = |rows: &[u32]| rows.iter().map(|&r| lex.words.text(&lex.storage, r as usize, false)).collect::<Vec<_>>();
        let mut rows = [0u32; ROWS];
        let n = pinyin_initial_rows(&lex, b"d", 0, &mut rows);
        assert_eq!(text(&rows[..n]), ["的", "到", "對"]);
        let n = pinyin_initial_rows(&lex, b"d", b'4', &mut rows);
        assert_eq!(text(&rows[..n]), ["到", "對"]);
        let n = pinyin_initial_rows(&lex, b"d", b'\'', &mut rows);
        assert_eq!(n, 3);
    }

    #[test]
    fn zhuyin_first_symbols() {
        let lex = zhuyin_lexicon();
        assert_eq!(texts(&lex, "j2"), ["我的", "臥底"]); // ㄨㄉ
        assert_eq!(texts(&lex, "ji2"), ["我的", "臥底"]); // ㄨㄛㄉ
        assert_eq!(texts(&lex, "ji32"), ["我的"]); // ㄨㄛˇㄉ: the tone leaves 臥底 out
        assert_eq!(texts(&lex, "ji42"), ["臥底"]);
        assert_eq!(texts(&lex, "2"), ["的", "到", "底"]); // ㄉ alone: single characters
        assert!(texts(&lex, "ji2k").is_empty()); // typed in full: lexicon rows
        assert!(texts(&lex, "jj").is_empty()); // ㄨㄨ: no bare ㄅ to ㄒ
        assert_eq!(texts(&lex, "2j"), ["得我"]); // this lexicon has no ㄉㄨ: ㄉ, ㄨ
    }
}
