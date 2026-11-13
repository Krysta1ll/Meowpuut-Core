#![forbid(unsafe_code)]
//! English words inside pinyin (wozaiyongiphone -> 我在用iPhone).
//!
//! The words are rows of the pinyin lexicon under an upper-case key (IPHONE),
//! written by compile-native-lexicon.py from DataSources/mimo-latin. Pinyin
//! lookups use lower-case keys and never reach them; the decoder asks for
//! them here, for a span of typed letters:
//! - a span that is one pinyin syllable (an, men, she, long) gets no English word;
//! - a span that does not split into full pinyin syllables (iphone, app, pdf)
//!   gets the word with `LATIN_TERM`;
//! - a span of two or more syllables (line, nice, meeting) gets it with
//!   `PINYIN_SPAN_TERM` on top, so it wins only when the Chinese reading of
//!   those letters is weak.
//!
//! First-letter abbreviation rows are not used on a span an English word
//! covers (pdf is PDF, not 裴多菲).

use crate::lexicon::Lexicon;
use crate::pairs::{END, START, STEP};
use crate::reading::Reading;
use crate::workspace::{text_hash, DecodeOutputSoA, PathLatticeSoA, StepPathsSoA, NO_SLOT};
use crate::{CandidateKind, CandidateScoreKind};

/// Added to an English word's lexicon score (weight / 700 - 19).
pub const LATIN_TERM: f64 = -2.0;
/// Added on top when the letters also split into pinyin syllables.
pub const PINYIN_SPAN_TERM: f64 = -1.5;
/// Longest key of an English word (mimo_latin.py keeps up to 24 letters).
const LONGEST: usize = 24;
/// Longest pinyin syllable (zhuang).
const SYLLABLE: usize = 6;

/// True for a row of an English word (its key and reading are upper case).
#[inline]
pub fn is_latin(lexicon: &Lexicon, index: usize) -> bool {
    lexicon.words.reading(&lexicon.storage, index).as_bytes().first().is_some_and(u8::is_ascii_uppercase)
}

/// English rows for the letters `sounds[start..end]` and the term they get, or
/// `None` where no English word may stand.
pub fn span(lexicon: &Lexicon, sounds: &[u8], start: usize, end: usize) -> Option<(usize, usize, f64)> {
    let n = end - start;
    if !lexicon.pinyin || !(2..=LONGEST).contains(&n) {
        return None;
    }
    let mut key = [0u8; LONGEST];
    for (k, &s) in key.iter_mut().zip(&sounds[start..end]) {
        *k = s.to_ascii_uppercase();
    }
    let (lo, hi) = lexicon.range(std::str::from_utf8(&key[..n]).ok()?);
    if lo == hi {
        return None;
    }
    match syllables(lexicon, &sounds[start..end]) {
        0 => Some((lo, hi, LATIN_TERM)),
        1 => None,
        _ => Some((lo, hi, LATIN_TERM + PINYIN_SPAN_TERM)),
    }
}

/// Fewest full pinyin syllables `letters` splits into; 0 if it does not split.
fn syllables(lexicon: &Lexicon, letters: &[u8]) -> u8 {
    let n = letters.len();
    let mut best = [u8::MAX; LONGEST + 1];
    best[0] = 0;
    for i in 0..n {
        if best[i] == u8::MAX {
            continue;
        }
        for j in (i + 1)..=n.min(i + SYLLABLE) {
            if best[i] + 1 < best[j] && is_syllable(lexicon, &letters[i..j]) {
                best[j] = best[i] + 1;
            }
        }
    }
    if best[n] == u8::MAX { 0 } else { best[n] }
}

/// A key with a one-syllable row that has a vowel (not n, ng, hm).
fn is_syllable(lexicon: &Lexicon, letters: &[u8]) -> bool {
    let Ok(key) = std::str::from_utf8(letters) else { return false };
    let (lo, hi) = lexicon.range(key);
    (lo..hi).any(|i| {
        let r = lexicon.words.reading(&lexicon.storage, i).as_bytes();
        r.len() == letters.len() + 1 && r.starts_with(letters) && letters.iter().any(|b| b"aeiouv".contains(b))
    })
}

/// Pushes the English words of one span as steps onto every path that starts at `end`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn push_steps(
    lexicon: &Lexicon,
    reading: &Reading,
    (lo, hi, term): (usize, usize, f64),
    start: usize,
    end: usize,
    hans: bool,
    open_end: u32,
    lattice: &PathLatticeSoA,
    step_paths: &mut StepPathsSoA,
) {
    let pairs = lexicon.pairs();
    for index in lo..hi {
        if !reading.matches(lexicon.words.reading(&lexicon.storage, index).as_bytes(), start, end) {
            continue;
        }
        let text = lexicon.words.text(&lexicon.storage, index, hans);
        let score = f64::from(lexicon.words.weight(index)) / 700.0 - 19.0 + term;
        let (hash, pow) = text_hash(text);
        let id = if pairs.is_some() { lexicon.index.word_ids.ids.get(index) } else { 0 };
        let start_term = match pairs {
            Some(p) if start == 0 => f64::from(p.score(START, id)) / STEP,
            _ => 0.0,
        };
        for b_idx in 0..lattice.count_at(end) {
            let slot = PathLatticeSoA::slot_index(end, b_idx);
            // Two English words typed without a space are not one word (OK and in are not OKin).
            if lattice.word[slot] != NO_SLOT && is_latin(lexicon, lattice.word[slot] as usize) {
                continue;
            }
            let next = lattice.id[slot];
            // The pair table counts Han text only; the word after an English word starts a run of Han text.
            let pair = match pairs {
                Some(p) if next != open_end && next != END => f64::from(p.score(START, next)) / STEP,
                _ => 0.0,
            };
            let total = score + lattice.score[slot] + pair + start_term;
            // Stored with the id of an open end, so the word before it gets no pair score either.
            step_paths.push(lattice, slot, index as u32, open_end, hash, pow, text.len(), total, lattice.spelled[slot], lattice.verified[slot]);
        }
    }
}

/// An English word for the whole input: first when the letters are not
/// pinyin and have a vowel (iphone, ok), else after the first two candidates
/// (pdf, which is also an abbreviation; nice, which is also pinyin).
pub(crate) fn whole_into(lexicon: &Lexicon, reading: &Reading, input_len: usize, hans: bool, out: &mut DecodeOutputSoA) {
    let sounds = reading.sounds_bytes();
    let Some((lo, hi, term)) = span(lexicon, sounds, 0, sounds.len()) else { return };
    let first = term == LATIN_TERM && sounds.iter().any(|b| b"aeiou".contains(b));
    let mut at = if first { 0 } else { 2.min(out.len()) };
    for index in lo..hi {
        let text = lexicon.words.text(&lexicon.storage, index, hans);
        if reading.matches(lexicon.words.reading(&lexicon.storage, index).as_bytes(), 0, sounds.len()) && !out.contains_text(text) {
            let weight = f64::from(lexicon.words.weight(index));
            out.insert_at(at, text, input_len, CandidateKind::Exact as u32, CandidateScoreKind::Lexicon as u32, weight);
            at += 1;
        }
    }
}

/// Puts the first candidate for the whole input first when it has an English
/// word in Chinese (我OK) and only partial candidates came before it (我哦 with
/// k left over); a complete reading with English beats a guess at an unfinished
/// syllable.
pub fn promote(ranked: &mut DecodeOutputSoA, input_len: usize) {
    let Some(j) = (0..ranked.len()).find(|&i| ranked.consumed[i] == input_len) else { return };
    let text = ranked.text(j);
    if j == 0 || !text.bytes().any(|b| b.is_ascii_alphabetic()) || !text.bytes().any(|b| b >= 0x80) {
        return;
    }
    let (consumed, kind, score_kind, score) = (ranked.consumed[j], ranked.kind[j], ranked.score_kind[j], ranked.score[j]);
    let (offset, len) = (ranked.text_offset[j], ranked.text_len[j]);
    ranked.remove_at(j);
    ranked.text_offset.insert(0, offset);
    ranked.text_len.insert(0, len);
    ranked.consumed.insert(0, consumed);
    ranked.kind.insert(0, kind);
    ranked.score_kind.insert(0, score_kind);
    ranked.score.insert(0, score);
}
