#![forbid(unsafe_code)]
//! Order of the candidate row after the first candidate (with a pair table),
//! the usual order of a Chinese keyboard:
//! 1. the first candidate, unchanged;
//! 2. other words that read as the whole input, by score;
//! 3. at most `PATHS_KEPT` other multi-word readings, only within
//!    `PATH_MARGIN` nats of the best one (the others go to the end);
//! 4. words for the leading syllables of the first reading, longest prefix
//!    first, heaviest first, down to the words of the first syllable. They are
//!    partial picks: taking one leaves the rest of the input composing.

use crate::lexicon::Lexicon;
use crate::reading::{is_tone, Reading, CANDIDATE_LIMIT};
use crate::workspace::{DecodeOutputSoA, PathLatticeSoA};
use crate::{CandidateKind, CandidateScoreKind};

/// Nats a multi-word reading of the whole input may score below the best one
/// and stay in the row, and how many stay. On the dev sentences 3 nats keep
/// the expected sentence in the first three cells for 604 of the 1,330 rows
/// whose first candidate is wrong (696 with no limit, 528 at 2 nats).
pub const PATH_MARGIN: f64 = 3.0;
pub const PATHS_KEPT: usize = 2;
/// The same for a first candidate of up to `SHORT` characters: there the
/// other readings are mostly one character swapped for a rarer one (哈紦,
/// 好飯 for 好煩); 0.5 nats keeps 她在睡覺 and 按讚 (benchmark rows).
pub const SHORT_PATH_MARGIN: f64 = 0.5;
pub const SHORT_PATHS_KEPT: usize = 1;
const SHORT: usize = 4;
/// Words offered for the first syllable, and for each longer prefix.
const FIRST_SYLLABLE_WORDS: usize = 24;
const PREFIX_WORDS: usize = 3;
/// Leading parts of the first candidate kept from the partial candidates
/// the list already had.
const LEADING_KEPT: usize = 2;
/// Words of the first reading looked at (longer readings keep their first 32).
const MAX_WORDS: usize = 32;

/// Input bytes up to sound `pos`, with the tone or apostrophe typed after it.
fn input_bytes_at(input: &str, pinyin: bool, pos: usize) -> usize {
    let mut sounds = 0;
    for (i, &b) in input.as_bytes().iter().enumerate() {
        let mark = is_tone(b, pinyin) || b == b'\'';
        if sounds == pos && !mark {
            return i;
        }
        sounds += usize::from(!mark);
    }
    input.len()
}

/// Whether the lattice path in `slot` spells `text`.
fn path_is(lexicon: &Lexicon, lattice: &PathLatticeSoA, hans: bool, slot: usize, text: &str) -> bool {
    let mut rest = text;
    for s in lattice.words_from(slot) {
        match rest.strip_prefix(lexicon.words.text(&lexicon.storage, lattice.word[s] as usize, hans)) {
            Some(r) => rest = r,
            None => return false,
        }
    }
    rest.is_empty()
}

/// Whether `text` is one lexicon word for the whole of `reading`.
fn is_whole_word(lexicon: &Lexicon, reading: &Reading, hans: bool, text: &str) -> bool {
    let (lo, hi) = lexicon.range(reading.sounds());
    (lo..hi).any(|i| lexicon.words.text(&lexicon.storage, i, hans) == text)
}

/// Partial candidates for the leading syllables of `decoded`'s first entry,
/// into `out`: the lexicon words for each prefix that ends a syllable of that
/// reading, longest prefix first, heaviest first. Call right
/// after the decode that filled `decoded`, while `lattice` holds its paths.
pub fn prefix_offers(lexicon: &Lexicon, input: &str, hans: bool, lattice: &PathLatticeSoA, decoded: &DecodeOutputSoA, out: &mut DecodeOutputSoA) {
    out.clear();
    if lexicon.pairs().is_none() || decoded.is_empty() {
        return;
    }
    let reading = Reading::new(input, lexicon.pinyin);
    let length = reading.len();
    let first = decoded.text(0);
    // The words of the first candidate: a lattice path, or one whole-input word.
    let mut words = [0u32; MAX_WORDS];
    let mut n = 0;
    if let Some(slot) = (0..lattice.count_at(0))
        .map(|b| PathLatticeSoA::slot_index(0, b))
        .find(|&s| path_is(lexicon, lattice, hans, s, first))
    {
        for s in lattice.words_from(slot).take(MAX_WORDS) {
            words[n] = lattice.word[s];
            n += 1;
        }
    } else {
        let (lo, hi) = lexicon.range(reading.sounds());
        match (lo..hi).find(|&i| lexicon.words.text(&lexicon.storage, i, hans) == first) {
            Some(i) => {
                words[0] = i as u32;
                n = 1;
            }
            None => return,
        }
    }
    // Syllable ends (in sounds) of those words.
    let mut ends = [0usize; 64];
    let mut count = 0;
    let mut pos = 0;
    for &w in &words[..n] {
        for &b in lexicon.words.reading(&lexicon.storage, w as usize).as_bytes() {
            if is_tone(b, lexicon.pinyin) {
                if count == ends.len() {
                    return;
                }
                ends[count] = pos;
                count += 1;
            } else {
                pos += 1;
            }
        }
    }
    // Typed as initials or not matching syllable for syllable: no offers.
    if count == 0 || (pos != length && n < MAX_WORDS) || ends[count - 1] > length {
        return;
    }
    let sounds = reading.sounds();
    for c in (0..count).rev() {
        let end = ends[c];
        if end >= length || end == 0 {
            continue;
        }
        let consumed = input_bytes_at(input, lexicon.pinyin, end);
        let cap = if c == 0 { FIRST_SYLLABLE_WORDS } else { PREFIX_WORDS };
        let (lo, hi) = lexicon.range(&sounds[..end]);
        let mut taken = 0;
        for i in lo..hi.min(lo + 4 * cap) {
            if taken == cap {
                break;
            }
            let wr = lexicon.words.reading(&lexicon.storage, i).as_bytes();
            // As many syllables as the prefix: zhuyin with tones left out
            // would otherwise give 侍衛 for the ㄕㄨㄟˋ of 睡覺.
            if lexicon.pinyin && !lexicon.words.is_fully_spelled(i)
                || wr.iter().filter(|&&b| is_tone(b, lexicon.pinyin)).count() != c + 1
                || !reading.matches(wr, 0, end)
            {
                continue;
            }
            taken += usize::from(push_new(out, lexicon.words.text(&lexicon.storage, i, hans), consumed));
        }
    }
}

fn push_new(out: &mut DecodeOutputSoA, text: &str, consumed: usize) -> bool {
    if (0..out.len()).any(|i| out.consumed[i] == consumed && out.text(i) == text) {
        return false;
    }
    out.push(text, consumed, CandidateKind::Partial as u32, CandidateScoreKind::None as u32, 0.0);
    true
}

/// Rewrites `ranked` in the row order above, keeping its first entry; the
/// partial candidates already in it and `offers` (from `prefix_offers`) come
/// last, longest first. Only with a pair table and when the first entry reads
/// the whole input. `buf` is scratch.
pub fn shape(lexicon: &Lexicon, input: &str, hans: bool, ranked: &mut DecodeOutputSoA, offers: &DecodeOutputSoA, buf: &mut DecodeOutputSoA) {
    let length = input.len();
    if lexicon.pairs().is_none() || ranked.is_empty() || ranked.consumed[0] != length || ranked.kind[0] == CandidateKind::Partial as u32 {
        return;
    }
    ranked.truncate(CANDIDATE_LIMIT);
    let reading = Reading::new(input, lexicon.pinyin);
    let path = CandidateScoreKind::Path as u32;
    let best = (0..ranked.len())
        .filter(|&i| ranked.consumed[i] == length && (ranked.score_kind[i] == path || ranked.score_kind[i] == CandidateScoreKind::Lexicon as u32))
        .map(|i| ranked.score[i])
        .fold(f64::NEG_INFINITY, f64::max);
    buf.clear();
    let copy = |buf: &mut DecodeOutputSoA, i: usize| buf.push(ranked.text(i), ranked.consumed[i], ranked.kind[i], ranked.score_kind[i], ranked.score[i]);
    copy(buf, 0);
    let short = crate::reading::utf8_char_count(ranked.text(0)) <= SHORT;
    let (margin, kept) = if short { (SHORT_PATH_MARGIN, SHORT_PATHS_KEPT) } else { (PATH_MARGIN, PATHS_KEPT) };
    let mut paths = 0;
    let mut late = [0u16; CANDIDATE_LIMIT];
    let mut lates = 0;
    for i in 1..ranked.len() {
        if ranked.consumed[i] != length {
            continue;
        }
        if ranked.score_kind[i] == path && !is_whole_word(lexicon, &reading, hans, ranked.text(i)) {
            if paths == kept || ranked.score[i] < best - margin {
                late[lates] = i as u16;
                lates += 1;
                continue;
            }
            paths += 1;
        }
        copy(buf, i);
    }
    // Partial candidates, longest first; at the same length the ones already listed first.
    let mut lengths = [0usize; 64];
    let mut kinds = 0;
    for c in ranked.consumed.iter().chain(offers.consumed.iter()) {
        if *c < length && kinds < lengths.len() && !lengths[..kinds].contains(c) {
            lengths[kinds] = *c;
            kinds += 1;
        }
    }
    lengths[..kinds].sort_unstable_by(|a, b| b.cmp(a));
    // Of the partial candidates already listed (prefix decodes kept for a
    // following Japanese word or an unfinished syllable), the first at each
    // length and only when it starts the first candidate, the two longest;
    // the others are compositions such as 握今天去上, or cut the sentence
    // after every syllable (這裡是我們的學, 這裡是我們的, 這裡是我們, ...).
    let first = ranked.text(0);
    let mut leading = 0;
    for &c in &lengths[..kinds] {
        if let Some(i) = (0..ranked.len()).find(|&i| ranked.consumed[i] == c) {
            if leading < LEADING_KEPT && first.starts_with(ranked.text(i)) && !has(buf, ranked.text(i), c) {
                copy(buf, i);
                leading += 1;
            }
        }
        for i in 0..offers.len() {
            if offers.consumed[i] == c && !has(buf, offers.text(i), c) {
                buf.push(offers.text(i), c, offers.kind[i], offers.score_kind[i], offers.score[i]);
            }
        }
    }
    // The other readings stay at the end: the Swift layer would otherwise
    // list a database word for the whole input as an addition, and put it
    // first when the first candidate is not in the database.
    for &i in &late[..lates] {
        copy(buf, usize::from(i));
    }
    buf.truncate(CANDIDATE_LIMIT);
    std::mem::swap(ranked, buf);
}

fn has(out: &DecodeOutputSoA, text: &str, consumed: usize) -> bool {
    (0..out.len()).any(|i| out.consumed[i] == consumed && out.text(i) == text)
}
