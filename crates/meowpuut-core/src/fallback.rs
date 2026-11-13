#![forbid(unsafe_code)]
//! Exact-match lookup, QWERTY neighbor typo correction, unfinished Pinyin phrase
//! completion, and single-character fallback candidates using preallocated SoA buffers.
//!
//! # Examples
//!
//! ```
//! use meowpuut_core::Lexicon;
//! use meowpuut_core::fallback::{has_exact_match, unfinished_phrase_into, fallback_into};
//! use meowpuut_core::workspace::DecodeOutputSoA;
//!
//! let tsv = concat!(
//!     "MEOWPUUT-PINYIN-1\n",
//!     "hao\thao3\t好\t好\t4800\n",
//!     "ni\tni3\t你\t你\t5000\n",
//!     "nihao\tni3hao3\t你好\t你好\t5400\n",
//! );
//! let lex = Lexicon::load(tsv).unwrap();
//! assert!(has_exact_match(&lex, "nihao"));
//! let mut out = DecodeOutputSoA::with_capacity(8);
//! unfinished_phrase_into(&lex, "nihaom", false, &mut out);
//! assert_eq!(out.len(), 1);
//! assert_eq!(out.text(0), "你好");
//! ```

use std::cmp::Ordering;

use crate::decoder::decode_with_buffers;
use crate::lexicon::Lexicon;
use crate::merge;
use crate::pairs::{END, START, STEP};
use crate::reading::{strict_han_count, utf8_char_count, Reading, MAX_INPUT};
use crate::workspace::{DecodeOutputSoA, DecoderScratchSoA};
use crate::{CandidateKind, CandidateScoreKind};

/// Returns `true` if `lexicon` has an exact whole-word match for `input`.
pub fn has_exact_match(lexicon: &Lexicon, input: &str) -> bool {
    let reading = Reading::new(input, lexicon.pinyin);
    let (lo, hi) = lexicon.range(reading.sounds());
    for i in lo..hi {
        let wr = lexicon.words.reading(&lexicon.storage, i).as_bytes();
        if reading.matches(wr, 0, reading.len()) {
            return true;
        }
    }
    false
}

#[inline]
fn qwerty_neighbors(ch: u8) -> &'static [u8] {
    match ch {
        b'q' => b"wa",
        b'w' => b"qeas",
        b'e' => b"wrsd",
        b'r' => b"etdf",
        b't' => b"ryfg",
        b'y' => b"tugh",
        b'u' => b"yihj",
        b'i' => b"uojk",
        b'o' => b"ipkl",
        b'p' => b"ol",
        b'a' => b"qwsz",
        b's' => b"awedzx",
        b'd' => b"serfxc",
        b'f' => b"drtgcv",
        b'g' => b"ftyhbv",
        b'h' => b"gyujbn",
        b'j' => b"huiknm",
        b'k' => b"jiolm",
        b'l' => b"kop",
        b'z' => b"asx",
        b'x' => b"zsdc",
        b'c' => b"xdfv",
        b'v' => b"cfgb",
        b'b' => b"vghn",
        b'n' => b"bhjm",
        b'm' => b"njk",
        _ => b"",
    }
}

const FUZZY_EDITS: usize = 16;
const SYLLABLE_LENGTH: usize = 6;
const NASAL_PENALTY: f64 = 2.9;
const INITIAL_PENALTY: f64 = 3.4;
const MIN_INITIAL_WORD_WEIGHT: i32 = 4700;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum SoundEdit {
    #[default]
    Remove,
    Insert(u8),
    Replace(u8),
}

impl SoundEdit {
    fn shift(self) -> isize {
        match self {
            Self::Remove => -1,
            Self::Insert(_) => 1,
            Self::Replace(_) => 0,
        }
    }

    fn write(self, input: &[u8], at: usize, output: &mut [u8]) -> usize {
        let (removed, added) = match self {
            Self::Remove => (1, 0),
            Self::Insert(byte) => { output[at] = byte; (0, 1) }
            Self::Replace(byte) => { output[at] = byte; (1, 1) }
        };
        output[..at].copy_from_slice(&input[..at]);
        let length = input.len() - removed + added;
        output[at + added..length].copy_from_slice(&input[at + removed..]);
        length
    }
}

#[derive(Clone, Copy, Default)]
struct FuzzyEdit {
    at: usize,
    start: usize,
    end: usize,
    change: SoundEdit,
}

fn nasal_edit(syllable: &str) -> Option<(usize, SoundEdit)> {
    if ["ang", "eng", "ing"].iter().any(|ending| syllable.ends_with(ending)) {
        Some((syllable.len() - 1, SoundEdit::Remove))
    } else if ["an", "en", "in"].iter().any(|ending| syllable.ends_with(ending)) {
        Some((syllable.len(), SoundEdit::Insert(b'g')))
    } else {
        None
    }
}

fn initial_edit(syllable: &str) -> Option<(usize, SoundEdit)> {
    let (first, rest) = syllable.as_bytes().split_first()?;
    match first {
        b'z' | b'c' | b's' if rest.first() == Some(&b'h') => Some((1, SoundEdit::Remove)),
        b'z' | b'c' | b's' => Some((1, SoundEdit::Insert(b'h'))),
        b'n' if !rest.is_empty() => Some((0, SoundEdit::Replace(b'l'))),
        b'l' if !rest.is_empty() => Some((0, SoundEdit::Replace(b'n'))),
        _ => None,
    }
}

/// Visit only complete syllables on a valid path through the original reading.
/// This keeps edits inside their syllable and out of the following initial.
fn fuzzy_edits(
    lexicon: &Lexicon, reading: &Reading, edit: fn(&str) -> Option<(usize, SoundEdit)>,
    edits: &mut [FuzzyEdit; FUZZY_EDITS],
) -> usize {
    let sounds = reading.sounds();
    let length = sounds.len();
    let mut syllables = [0u8; MAX_INPUT + 1];
    let mut suffix = [false; MAX_INPUT + 1];
    suffix[length] = true;
    for start in (0..length).rev() {
        for end in start + 1..=length.min(start + SYLLABLE_LENGTH) {
            if reading.boundary_count[end - 1] != reading.boundary_count[start] { break; }
            if suffix[end] && merge::is_syllable(lexicon, &sounds[start..end]) {
                syllables[start] |= 1 << (end - start);
                suffix[start] = true;
            }
        }
    }
    if !suffix[0] { return 0; }
    let mut prefix = [false; MAX_INPUT + 1];
    prefix[0] = true;
    let mut count = 0;
    for start in 0..length {
        if !prefix[start] { continue; }
        for end in start + 1..=length.min(start + SYLLABLE_LENGTH) {
            if syllables[start] & (1 << (end - start)) == 0 { continue; }
            prefix[end] = true;
            let syllable = &sounds[start..end];
            let Some((offset, change)) = edit(syllable) else { continue; };
            let at = start + offset;
            if edits[..count].iter().any(|e| e.at == at && e.change == change) { continue; }
            let mut key = [0; SYLLABLE_LENGTH + 1];
            let len = change.write(syllable.as_bytes(), offset, &mut key);
            if merge::is_syllable(lexicon, std::str::from_utf8(&key[..len]).unwrap()) {
                edits[count] = FuzzyEdit { at, start, end, change };
                count += 1;
                if count == edits.len() { return count; }
            }
        }
    }
    count
}

/// Best whole dictionary word across the edited syllable and one nearby
/// syllable. Pick a couple of variants for a complete sentence decode.
fn local_word_weight(lexicon: &Lexicon, reading: &Reading, start: usize, end: usize) -> i32 {
    let word_weight = |start, end| {
        let (lo, hi) = lexicon.range(&reading.sounds()[start..end]);
        (lo..hi.min(lo + 4)).filter(|&i| {
            lexicon.words.is_fully_spelled(i)
                && strict_han_count(lexicon.words.text(&lexicon.storage, i, false)) > 1
                && reading.matches(lexicon.words.reading(&lexicon.storage, i).as_bytes(), start, end)
        }).map(|i| lexicon.words.weight(i)).max().unwrap_or_default()
    };
    (start.saturating_sub(SYLLABLE_LENGTH)..start).map(|first| word_weight(first, end))
        .chain((end + 1..=reading.len().min(end + SYLLABLE_LENGTH)).map(|last| word_weight(start, last)))
        .max().unwrap_or_default()
}

pub(crate) fn fuzzy_corrections_into(
    lexicon: &Lexicon, input: &str, hans: bool, scratch: &mut DecoderScratchSoA, out: &mut DecodeOutputSoA, initials: bool,
) {
    out.clear();
    if !lexicon.pinyin || !(2..=MAX_INPUT).contains(&input.len())
        || !input.bytes().all(|b| b.is_ascii_lowercase() || (b'1'..=b'5').contains(&b) || b == b'\'')
    { return; }
    let plausible = if initials {
        input.len() >= 4 && input.bytes().any(|byte| b"zcsnl".contains(&byte))
    } else {
        input.as_bytes().windows(2).any(|w| b"aei".contains(&w[0]) && w[1] == b'n')
    };
    if !plausible { return; }
    let reading = Reading::new(input, true);
    if !reading.unambiguous { return; }
    let edit = if initials { initial_edit } else { nasal_edit };
    let penalty = if initials { INITIAL_PENALTY } else { NASAL_PENALTY };
    let score_kind = if initials { CandidateScoreKind::FuzzyInitial } else { CandidateScoreKind::Fallback } as u32;
    let mut edits = [FuzzyEdit::default(); FUZZY_EDITS];
    let count = fuzzy_edits(lexicon, &reading, edit, &mut edits);
    if count == 0 { return; }
    // The byte offset just after a sound is before any explicit tone or
    // separator. Inserting here leaves both the separator and the tone intact.
    let mut offsets = [0usize; MAX_INPUT + 1];
    let mut sounds = 0;
    for (i, byte) in input.bytes().enumerate() {
        if byte.is_ascii_lowercase() {
            sounds += 1;
            offsets[sounds] = i + 1;
        }
    }
    let raw_at = |edit: FuzzyEdit| match edit.change {
        SoundEdit::Insert(_) => offsets[edit.at],
        SoundEdit::Remove | SoundEdit::Replace(_) => offsets[edit.at + 1] - 1,
    };
    let mut best = [(i32::MIN, FuzzyEdit::default()); 2];
    let mut buffer = [0; MAX_INPUT + 1];
    for &edit in &edits[..count] {
        if matches!(edit.change, SoundEdit::Insert(_)) && input.len() == MAX_INPUT { continue; }
        let len = edit.change.write(input.as_bytes(), raw_at(edit), &mut buffer);
        let revised = Reading::new(std::str::from_utf8(&buffer[..len]).unwrap(), true);
        let (lo, hi) = lexicon.range(revised.sounds());
        let mut found = false;
        for i in lo..hi.min(lo + 7) {
            let wr = lexicon.words.reading(&lexicon.storage, i);
            let text = lexicon.words.text(&lexicon.storage, i, hans);
            if !lexicon.words.is_fully_spelled(i) || !revised.matches(wr.as_bytes(), 0, revised.len()) { continue; }
            let han_count = strict_han_count(text);
            // A one-syllable reading already has enough homophones.
            if han_count == 0 || (initials && (han_count == 1 || lexicon.words.weight(i) < MIN_INITIAL_WORD_WEIGHT)) { continue; }
            merge::push_best(out, text, input.len(), CandidateKind::Correction as u32, score_kind,
                             merge::whole_word_score(lexicon, i) - penalty);
            found = true;
        }
        if !found && reading.len() >= 7 {
            let weight = local_word_weight(lexicon, &revised, edit.start, edit.end.saturating_add_signed(edit.change.shift()));
            let typed_weight = local_word_weight(lexicon, &reading, edit.start, edit.end);
            // A change inside a sentence is worth a second decode only when
            // it makes a stronger word next to the edited syllable.
            if weight <= typed_weight || (initials && weight < MIN_INITIAL_WORD_WEIGHT) { continue; }
            if weight > best[0].0 { best[1] = best[0]; best[0] = (weight, edit); }
            else if weight > best[1].0 { best[1] = (weight, edit); }
        }
    }

    // Direct whole-word corrections are already useful. Only decode longer
    // variants when they still need to be composed from dictionary words.
    if out.is_empty() {
        for &(weight, edit) in &best {
            if weight == i32::MIN { break; }
            let len = edit.change.write(input.as_bytes(), raw_at(edit), &mut buffer);
            let revised = std::str::from_utf8(&buffer[..len]).unwrap();
            decode_with_buffers(lexicon, revised, hans, true, false, false, &mut scratch.lattice, &mut scratch.step_paths,
                                &mut scratch.fallback_raw, &mut scratch.fallback_order, &mut scratch.fallback_out, &mut scratch.aux_decoded);
            for i in 0..scratch.aux_decoded.len().min(3) {
                let text = scratch.aux_decoded.text(i);
                if strict_han_count(text) > 1 {
                    merge::push_best(out, text, input.len(), CandidateKind::Correction as u32, score_kind,
                                     scratch.aux_decoded.score[i] - penalty);
                }
            }
        }
    }
    merge::sort_from(out, 0);
    out.truncate(6);
}

/// A dropped letter can make a very different word. Only offer the best
/// common dictionary match, after the caller has kept the normal ranking.
pub(crate) fn dropped_letter_candidate<'a>(lexicon: &'a Lexicon, input: &str, hans: bool) -> Option<(&'a str, f64)> {
    let bytes = input.as_bytes();
    if !lexicon.pinyin || !(4..=8).contains(&bytes.len()) || !bytes.iter().all(u8::is_ascii_lowercase) {
        return None;
    }
    let mut best = None;
    let mut key = [0u8; 9];
    for at in 0..=bytes.len() {
        key[..at].copy_from_slice(&bytes[..at]);
        key[at + 1..=bytes.len()].copy_from_slice(&bytes[at..]);
        for letter in b'a'..=b'z' {
            key[at] = letter;
            let (lo, hi) = lexicon.range(std::str::from_utf8(&key[..=bytes.len()]).unwrap());
            for i in lo..hi.min(lo + 7) {
                if lexicon.words.weight(i) < 4000 { break; }
                let text = lexicon.words.text(&lexicon.storage, i, hans);
                if !lexicon.words.is_fully_spelled(i) || strict_han_count(text) < 2 { continue; }
                let score = merge::whole_word_score(lexicon, i) - merge::GUESS_PENALTY - 2.0;
                if best.is_none_or(|(_, prior)| score > prior) { best = Some((text, score)); }
            }
        }
    }
    best
}

/// Finds completed phrases when the final Pinyin syllable (`1..=3` letters) is still unfinished.
pub fn unfinished_phrase_into(
    lexicon: &Lexicon,
    input: &str,
    hans: bool,
    out: &mut DecodeOutputSoA,
) {
    out.clear();
    if !lexicon.pinyin || input.len() < 3 || input.len() > 64 || has_exact_match(lexicon, input) {
        return;
    }
    let max_tail = 3.min(input.len() - 1);
    for tail in 1..=max_tail {
        let suffix = &input[input.len() - tail..];
        if !suffix.bytes().all(|c| (b'a'..=b'z').contains(&c)) {
            continue;
        }
        let complete_suffix = lexicon.range(suffix);
        if complete_suffix.0 != complete_suffix.1 {
            continue;
        }
        let count = input.len() - tail;
        let prefix = Reading::new(&input[..count], true);
        let (lo, hi) = lexicon.range(prefix.sounds());
        for i in lo..hi {
            if out.len() >= 3 {
                break;
            }
            let text = lexicon.words.text(&lexicon.storage, i, hans);
            let wr = lexicon.words.reading(&lexicon.storage, i).as_bytes();
            if lexicon.words.is_fully_spelled(i)
                && utf8_char_count(text) > 1
                && prefix.matches(wr, 0, prefix.len())
            {
                out.push(
                    text,
                    count,
                    CandidateKind::Partial as u32,
                    CandidateScoreKind::None as u32,
                    f64::from(lexicon.words.weight(i)),
                );
            }
        }
        if !out.is_empty() {
            return;
        }
    }
}

/// Generates bounded prefix completions, single-edit typo corrections, and single-char fallbacks
/// into preallocated SoA buffers without heap allocation.
pub fn fallback_into(
    lexicon: &Lexicon,
    input: &str,
    hans: bool,
    allow_single: bool,
    raw: &mut DecodeOutputSoA,
    order: &mut Vec<usize>,
    out: &mut DecodeOutputSoA,
) {
    raw.clear();
    order.clear();
    out.clear();
    let reading = Reading::new(input, lexicon.pinyin);
    let sounds = reading.sounds();
    let sounds_bytes = reading.sounds_bytes();
    // With a pair table a word that fills the whole input also gets the
    // clause start and end terms, in weight units (700 per nat), as in the lattice.
    let edges = |i: usize, whole: bool| -> f64 {
        match lexicon.pairs() {
            Some(p) => {
                let id = lexicon.index.word_ids.ids.get(i);
                let end = if whole { p.score(id, END) } else { 0 };
                700.0 * f64::from(p.score(START, id) + end) / STEP
            }
            None => 0.0,
        }
    };

    let collect = |key: &str, penalty: f64, raw_buf: &mut DecodeOutputSoA| {
        let (lo, hi) = lexicon.range(key);
        for i in lo..hi.min(lo + 7) {
            let text = lexicon.words.text(&lexicon.storage, i, hans);
            raw_buf.push(
                text,
                input.len(),
                CandidateKind::Correction as u32,
                CandidateScoreKind::Fallback as u32,
                f64::from(lexicon.words.weight(i)) - penalty + edges(i, true),
            );
        }
    };

    if sounds_bytes.len() == 1 {
        for idx in lexicon.initials(sounds_bytes[0]) {
            raw.push(
                lexicon.words.text(&lexicon.storage, idx, hans),
                input.len(),
                CandidateKind::Correction as u32,
                CandidateScoreKind::Fallback as u32,
                f64::from(lexicon.words.weight(idx)) + edges(idx, true),
            );
        }
    }

    if lexicon.pinyin && (2..=32).contains(&sounds.len()) && input == sounds {
        let mut scanned = 0usize;
        let mut bucket_idx = lexicon.lower_bound_bucket(sounds);
        while bucket_idx < lexicon.buckets.len() && scanned < 256 {
            scanned += 1;
            let bkey = lexicon.buckets.key(&lexicon.storage, bucket_idx);
            if !bkey.starts_with(sounds) {
                break;
            }
            let extra = bkey.len() - sounds.len();
            if extra <= 6 {
                collect(bkey, 1200.0 * (extra as f64), raw);
            }
            bucket_idx += 1;
        }

        if sounds_bytes.len() >= 4 {
            let mut key_buf = [0u8; 36];
            let slen = sounds_bytes.len();
            for i in 0..slen {
                if i + 1 < slen && sounds_bytes[i] != sounds_bytes[i + 1] {
                    key_buf[..slen].copy_from_slice(sounds_bytes);
                    key_buf.swap(i, i + 1);
                    if let Ok(s) = std::str::from_utf8(&key_buf[..slen]) {
                        collect(s, 800.0, raw);
                    }
                }
                if i > 0 && sounds_bytes[i] == sounds_bytes[i - 1] {
                    key_buf[..i].copy_from_slice(&sounds_bytes[..i]);
                    key_buf[i..slen - 1].copy_from_slice(&sounds_bytes[i + 1..slen]);
                    if let Ok(s) = std::str::from_utf8(&key_buf[..slen - 1]) {
                        collect(s, 800.0, raw);
                    }
                }
                for &ch in qwerty_neighbors(sounds_bytes[i]) {
                    key_buf[..slen].copy_from_slice(sounds_bytes);
                    key_buf[i] = ch;
                    if let Ok(s) = std::str::from_utf8(&key_buf[..slen]) {
                        collect(s, 1200.0, raw);
                    }
                }
            }
        }

    }

    if raw.is_empty() && allow_single {
        let mut count = input.len().min(7);
        while count > 0 {
            let prefix = Reading::new(&input[..count], lexicon.pinyin);
            if !prefix.is_empty() {
                let (lo, hi) = lexicon.range(prefix.sounds());
                for i in lo..hi {
                    if raw.len() >= 28 {
                        break;
                    }
                    let text = lexicon.words.text(&lexicon.storage, i, hans);
                    let wr = lexicon.words.reading(&lexicon.storage, i).as_bytes();
                    if utf8_char_count(text) == 1 && prefix.matches(wr, 0, prefix.len()) {
                        raw.push(
                            text,
                            count,
                            CandidateKind::Partial as u32,
                            CandidateScoreKind::Fallback as u32,
                            f64::from(lexicon.words.weight(i)) + edges(i, count == input.len()),
                        );
                    }
                }
                if !raw.is_empty() {
                    break;
                }
            }
            count -= 1;
        }
    }

    for i in 0..raw.len() {
        order.push(i);
    }
    order.sort_by(|&a, &b| {
        if raw.score[a] > raw.score[b] {
            Ordering::Less
        } else if raw.score[a] < raw.score[b] {
            Ordering::Greater
        } else {
            Ordering::Equal
        }
    });

    for &idx in order.iter() {
        let text = raw.text(idx);
        if !out.contains_text(text) {
            out.push(text, raw.consumed[idx], raw.kind[idx], raw.score_kind[idx], raw.score[idx]);
            if out.len() == 28 {
                break;
            }
        }
    }
}
