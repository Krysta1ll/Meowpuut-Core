#![forbid(unsafe_code)]
//! Ranking of the decoder's candidate list with a pair table: typo and
//! completion guesses and whole-input words get a score in the lattice's
//! units (nats), so a fully spelled reading is not beaten by a guess only
//! because the guess is listed first.

use crate::lexicon::Lexicon;
use crate::pairs::{END, START, STEP};
use crate::reading::{Reading, CANDIDATE_LIMIT};
use crate::workspace::DecodeOutputSoA;
use crate::{CandidateKind, CandidateScoreKind};

/// Nats taken off a typo, neighbour-key or completion guess on top of its own
/// edit penalty, so it wins only over a clearly worse spelled reading.
pub const GUESS_PENALTY: f64 = 1.7;

/// Bound on the clause start and end terms of a whole-input word: they are
/// counted mostly from written and chat text and would otherwise decide
/// between two everyday words (鉛筆 and 錢幣 for qianbi; 煎蛋 and 小事, often a
/// whole chat message, over 簡單 and 小時). With no terms at all, 鑰匙, 橘子
/// and 雞肉 lose to 要是, 句子 and 肌肉 (benchmark: 1.0 fixes 5 rows and
/// breaks 1; 0 fixes 12 and breaks 19).
const EDGE_LIMIT: f64 = 1.0;

/// Lattice score of word `index` as the whole input: the word cost of the
/// decoder plus the clause start and end terms.
pub fn whole_word_score(lexicon: &Lexicon, index: usize) -> f64 {
    let base = f64::from(lexicon.words.weight(index)) / 700.0 - 19.0;
    match lexicon.pairs() {
        Some(p) => {
            let id = lexicon.index.word_ids.ids.get(index);
            let term = |v: i32| (f64::from(v) / STEP).clamp(-EDGE_LIMIT, EDGE_LIMIT);
            base + term(p.score(START, id)) + term(p.score(id, END))
        }
        None => base,
    }
}

/// Pushes the words that read as the whole input (every word in zhuyin),
/// scored as one-word paths: the spelled words, and with `abbreviations` the
/// abbreviation rows too (默哀 for ma). An abbreviation row has an eighth of
/// its word's weight, so it comes after the common spelled words (嗎 before
/// 默哀). It gets no clause start and end terms, which would outweigh the
/// small weight differences between abbreviation rows. The kinds are those
/// the decoder gives whole-input words.
pub fn push_whole_words(lexicon: &Lexicon, reading: &Reading, input: &str, hans: bool, abbreviations: bool, metadata: bool, out: &mut DecodeOutputSoA) {
    let (lo, hi) = lexicon.range(reading.sounds());
    for index in lo..hi {
        if out.len() == CANDIDATE_LIMIT {
            return;
        }
        let spelled = !lexicon.pinyin || lexicon.words.is_fully_spelled(index);
        if !spelled && !abbreviations
            || !reading.matches(lexicon.words.reading(&lexicon.storage, index).as_bytes(), 0, reading.len())
        {
            continue;
        }
        let kind = if !reading.unambiguous {
            CandidateKind::Unknown
        } else if !spelled {
            CandidateKind::Correction
        } else if !metadata || !lexicon.pinyin || lexicon.words.is_verified_spelling(index) {
            CandidateKind::Exact
        } else {
            CandidateKind::Unknown
        };
        let text = lexicon.words.text(&lexicon.storage, index, hans);
        if !out.contains_text(text) {
            let score = if spelled { whole_word_score(lexicon, index) } else { f64::from(lexicon.words.weight(index)) / 700.0 - 19.0 };
            out.push(text, input.len(), kind as u32, CandidateScoreKind::Lexicon as u32, score);
        }
    }
}

/// Lattice score of a fallback guess whose score is in weight units (700 per
/// nat, start and end terms included, see `fallback_into`).
pub fn guess_score(fallback_score: f64) -> f64 {
    fallback_score / 700.0 - 19.0 - GUESS_PENALTY
}

/// Pushes `text`, or, when the list has it already with a lower score, gives
/// that entry this kind and score.
pub fn push_best(out: &mut DecodeOutputSoA, text: &str, consumed: usize, kind: u32, score_kind: u32, score: f64) {
    match (0..out.len()).find(|&i| out.text(i) == text) {
        Some(i) if out.score[i] < score => {
            out.kind[i] = kind;
            out.score_kind[i] = score_kind;
            out.score[i] = score;
        }
        Some(_) => {}
        None => out.push(text, consumed, kind, score_kind, score),
    }
}

/// Stable sort of `out[from..]` by score, highest first (insertion sort: the
/// list is short and nothing is allocated).
pub fn sort_from(out: &mut DecodeOutputSoA, from: usize) {
    for i in from + 1..out.len() {
        let mut j = i;
        while j > from && out.score[j - 1] < out.score[j] {
            out.text_offset.swap(j - 1, j);
            out.text_len.swap(j - 1, j);
            out.consumed.swap(j - 1, j);
            out.kind.swap(j - 1, j);
            out.score_kind.swap(j - 1, j);
            out.score.swap(j - 1, j);
            j -= 1;
        }
    }
}

/// Nats taken off a word typed as full syllables mixed with initials
/// (zenmb for 怎麼辦), on top of its full-reading weight.
pub const MIXED_PENALTY: f64 = 3.0;
/// Rows kept per mixed span.
pub const MIXED_ROWS: usize = 8;

/// One unit of a mixed span: a full syllable or an initial (z, zh, m, ...).
#[derive(Clone, Copy, Default)]
struct Unit {
    start: u8,
    end: u8,
    full: bool,
}

pub(crate) fn is_syllable(lexicon: &Lexicon, s: &str) -> bool {
    let (lo, hi) = lexicon.range(s);
    (lo..hi).any(|i| {
        let r = lexicon.words.reading(&lexicon.storage, i).as_bytes();
        lexicon.words.is_fully_spelled(i) && r.iter().filter(|&&b| crate::reading::is_tone(b, true)).count() == 1
    })
}

/// Rows of words whose reading is `span` typed as full syllables mixed with
/// initials (zenmb, buzd, meiwt), at least one of each, heaviest first, into
/// `rows`. Returns how many. Only spans of letters that are no key of their own.
pub fn mixed_rows(lexicon: &Lexicon, span: &str, rows: &mut [u32; MIXED_ROWS]) -> usize {
    let b = span.as_bytes();
    if !lexicon.pinyin || !(3..=16).contains(&b.len()) || !b.iter().all(u8::is_ascii_lowercase) {
        return 0;
    }
    let (lo, hi) = lexicon.range(span);
    if lo != hi {
        return 0;
    }
    // A consonant that starts no syllable here: followed by another consonant
    // (not h after z, c, s; not g after n) or last and not n.
    let vowel = |c: u8| b"aeiouv".contains(&c);
    let dangling = (0..b.len()).any(|i| {
        !vowel(b[i]) && match b.get(i + 1) {
            None => b[i] != b'n' && b[i] != b'g' && b[i] != b'r',
            Some(&n) => !vowel(n) && !(n == b'h' && b"zcs".contains(&b[i])) && !(n == b'g' && b[i] == b'n') && b[i] != b'n' && b[i] != b'r',
        }
    });
    if !dangling {
        return 0;
    }
    let mut weights = [i32::MIN; MIXED_ROWS];
    let mut count = 0usize;
    let mut units = [Unit::default(); 16];
    let mut budget = 32usize;
    parse(lexicon, b, 0, &mut units, 0, &mut budget, &mut |units: &[Unit]| {
        if !units.iter().any(|u| u.full) || units.iter().all(|u| u.full) {
            return;
        }
        // Full keys start with everything up to the first initial.
        let first = units.iter().position(|u| !u.full).unwrap_or(0);
        let prefix = &span[..usize::from(units[first].end)];
        let mut bucket = lexicon.lower_bound_bucket(prefix);
        let mut scanned = 0;
        while bucket < lexicon.buckets.len() && scanned < 256 {
            scanned += 1;
            let key = lexicon.buckets.key(&lexicon.storage, bucket);
            if !key.starts_with(prefix) {
                break;
            }
            let (lo, hi) = lexicon.buckets.range(bucket);
            bucket += 1;
            if lo == hi || !fits(lexicon.words.reading(&lexicon.storage, lo).as_bytes(), b, units) {
                continue;
            }
            for index in lo..hi {
                if !lexicon.words.is_fully_spelled(index) {
                    continue;
                }
                let w = i32::from(lexicon.words.weight(index));
                // Keep the heaviest MIXED_ROWS rows.
                let slot = if count < MIXED_ROWS { count } else { (0..MIXED_ROWS).min_by_key(|&k| weights[k]).unwrap_or(0) };
                if count < MIXED_ROWS || w > weights[slot] {
                    rows[slot] = index as u32;
                    weights[slot] = w;
                    count = (count + 1).min(MIXED_ROWS);
                }
            }
        }
    });
    // Heaviest first.
    for i in 1..count {
        let mut j = i;
        while j > 0 && weights[j - 1] < weights[j] {
            weights.swap(j - 1, j);
            rows.swap(j - 1, j);
            j -= 1;
        }
    }
    count
}

/// Every split of `b[at..]` into full syllables and initials (`budget` caps
/// the number of steps tried).
fn parse(lexicon: &Lexicon, b: &[u8], at: usize, units: &mut [Unit; 16], n: usize, budget: &mut usize, found: &mut dyn FnMut(&[Unit])) {
    if at == b.len() {
        found(&units[..n]);
        return;
    }
    if n == units.len() || *budget == 0 {
        return;
    }
    *budget -= 1;
    let span = std::str::from_utf8(b).unwrap_or("");
    for end in at + 1..=b.len().min(at + 6) {
        let initial = match end - at {
            1 => !b"aeiouv".contains(&b[at]),
            2 => b[at + 1] == b'h' && b"zcs".contains(&b[at]),
            _ => false,
        };
        // The first unit is a full syllable, so the key scan below starts from a real prefix.
        for full in [true, false] {
            if !full && n == 0 {
                continue;
            }
            if if full { is_syllable(lexicon, &span[at..end]) } else { initial } {
                units[n] = Unit { start: at as u8, end: end as u8, full };
                parse(lexicon, b, end, units, n + 1, budget, found);
            }
        }
    }
}

/// Whether a full reading (zen3me5ban4) has one syllable per unit, equal to a
/// full unit and starting with an initial one.
fn fits(reading: &[u8], b: &[u8], units: &[Unit]) -> bool {
    let mut syllables = reading.split(|&c| crate::reading::is_tone(c, true)).filter(|s| !s.is_empty());
    for u in units {
        let Some(s) = syllables.next() else { return false };
        let typed = &b[usize::from(u.start)..usize::from(u.end)];
        if if u.full { s != typed } else { !s.starts_with(typed) } {
            return false;
        }
    }
    syllables.next().is_none()
}

/// Pushes the words the whole input spells as full syllables mixed with
/// initials, scored as one-word paths less `MIXED_PENALTY`.
pub fn push_mixed_whole(lexicon: &Lexicon, input: &str, hans: bool, out: &mut DecodeOutputSoA) {
    let mut rows = [0u32; MIXED_ROWS];
    let count = mixed_rows(lexicon, input, &mut rows);
    for &row in &rows[..count] {
        let index = row as usize;
        let text = lexicon.words.text(&lexicon.storage, index, hans);
        if !out.contains_text(text) && out.len() < CANDIDATE_LIMIT {
            out.push(text, input.len(), CandidateKind::Correction as u32, CandidateScoreKind::Lexicon as u32, whole_word_score(lexicon, index) - MIXED_PENALTY);
        }
    }
}
