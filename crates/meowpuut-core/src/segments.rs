#![forbid(unsafe_code)]
//! Segment-level disambiguation, incomplete Pinyin syllable detection,
//! unfinished sentence preservation, and Zhuyin prefix segmentation.
//!
//! # Examples
//!
//! ```
//! use meowpuut_core::{Lexicon, DecoderScratchSoA};
//! use meowpuut_core::segments::{incomplete_pinyin_syllable, pinyin_segments_into};
//!
//! let tsv = concat!(
//!     "MEOWPUUT-PINYIN-1\n",
//!     "dong\tdong1\t東\t东\t4800\n",
//!     "hao\thao3\t好\t好\t4800\n",
//!     "ni\tni3\t你\t你\t5000\n",
//!     "nihao\tni3hao3\t你好\t你好\t5400\n",
//!     "shijie\tshi4jie4\t世界\t世界\t5300\n",
//! );
//! let lex = Lexicon::load(tsv).unwrap();
//! assert!(incomplete_pinyin_syllable(&lex, "don"));
//! assert!(!incomplete_pinyin_syllable(&lex, "dong"));
//!
//! let mut scratch = DecoderScratchSoA::new();
//! let mut out = meowpuut_core::workspace::DecodeOutputSoA::with_capacity(8);
//! pinyin_segments_into(&lex, "nihaoshijie", false, &mut scratch, &mut out);
//! assert!(!out.is_empty());
//! assert_eq!(out.text(0), "你好");
//! assert_eq!(out.consumed[0], 5);
//! ```

use crate::decoder::{decode_with_buffers, has_exact_match};
use crate::lexicon::Lexicon;
use crate::reading::{is_tone, utf8_char_count, Reading, KEYS, MAX_INPUT};
use crate::workspace::{DecodeOutputSoA, DecoderScratchSoA};
use crate::{CandidateKind, CandidateScoreKind};

/// Leading segments checked against the continuation (with a pair table).
const SCORED: usize = 8;

const PINYIN_ONSET_INITIALS: [&str; 22] = [
    "b", "p", "f", "d", "t", "l", "g", "k", "h", "j", "q", "x", "zh", "ch", "sh", "r", "z", "c",
    "s", "y", "w", "n",
];

#[inline]
fn decode_aux(lexicon: &Lexicon, input: &str, hans: bool, complete: bool, scratch: &mut DecoderScratchSoA) {
    decode_with_buffers(
        lexicon,
        input,
        hans,
        complete,
        false,
        true,
        &mut scratch.lattice,
        &mut scratch.step_paths,
        &mut scratch.fallback_raw,
        &mut scratch.fallback_order,
        &mut scratch.fallback_out,
        &mut scratch.aux_decoded,
    );
}

/// Consults bounded dictionary completions when an unfinished initial (`1..=3` keys)
/// disambiguates the preceding phrase.
pub fn predictive_segments_at_into(
    lexicon: &Lexicon,
    input: &str,
    hans: bool,
    tail: usize,
    scratch: &mut DecoderScratchSoA,
    out: &mut DecodeOutputSoA,
) {
    out.clear();
    if input.len() < 4 || input.len() > 64 || has_exact_match(lexicon, input) {
        return;
    }
    let count = input.len() - tail;
    let suffix_bytes = input[count..].as_bytes();
    if lexicon.pinyin {
        if !b"bpmdtlgkhjqxzcrsfyw".contains(&suffix_bytes[0])
            || !suffix_bytes.iter().all(|&c| (b'a'..=b'z').contains(&c))
        {
            return;
        }
    } else if !KEYS[..21].contains(&suffix_bytes[0])
        || suffix_bytes.iter().any(|&c| is_tone(c, false) || c == b'\'')
    {
        return;
    }
    if input.as_bytes()[count - 1] == b'\'' {
        return;
    }

    decode_aux(lexicon, &input[..count], hans, lexicon.pinyin, scratch);
    if scratch.aux_decoded.is_empty() {
        return;
    }

    // With a pair table the leading segment is ranked without the word it
    // starts (我喜 of 我喜歡 is rare as two words), so more of its paths are
    // checked against the continuation.
    let checked = if lexicon.pairs().is_some() { SCORED } else { 5 };
    let mut scored_indices = [0usize; SCORED];
    let mut scored_evidence = [0i32; SCORED];
    let mut scored_order = [0usize; SCORED];
    let mut scored_len = 0usize;
    for i in 0..scratch.aux_decoded.len().min(checked) {
        if utf8_char_count(scratch.aux_decoded.text(i)) >= 2 {
            scored_indices[scored_len] = i;
            scored_order[scored_len] = i;
            scored_len += 1;
        }
    }
    if scored_len == 0 {
        return;
    }

    let input_bytes = input.as_bytes();
    for start in (count.saturating_sub(6)..count).rev() {
        if is_tone(input_bytes[start], lexicon.pinyin) || input_bytes[start] == b'\'' {
            continue;
        }
        let continuation = Reading::new(&input[start..], lexicon.pinyin);
        let completed = Reading::new(&input[start..count], lexicon.pinyin);
        if completed.is_empty() || continuation.len() <= completed.len() {
            continue;
        }
        let cont_sounds = continuation.sounds();
        let mut bucket = lexicon.lower_bound_bucket(cont_sounds);
        let mut scanned = 0usize;
        while bucket < lexicon.buckets.len() && scanned < 96 {
            let bkey = lexicon.buckets.key(&lexicon.storage, bucket);
            if !bkey.starts_with(cont_sounds) {
                break;
            }
            if bkey.len() - cont_sounds.len() <= 6 {
                let (lo, hi) = lexicon.buckets.range(bucket);
                for i in lo..hi.min(lo + 5) {
                    let weight = lexicon.words.weight(i);
                    let wr = lexicon.words.reading(&lexicon.storage, i);
                    if weight < 4000 || !continuation.matches(wr.as_bytes(), 0, continuation.len()) {
                        continue;
                    }
                    let wr_bytes = wr.as_bytes();
                    let mut first_tone = 0usize;
                    while first_tone < wr_bytes.len() && !is_tone(wr_bytes[first_tone], lexicon.pinyin) {
                        first_tone += 1;
                    }
                    if first_tone != completed.len() {
                        continue;
                    }
                    let text = lexicon.words.text(&lexicon.storage, i, hans);
                    let t_bytes = text.as_bytes();
                    let mut first_end = 1usize;
                    while first_end < t_bytes.len() && (t_bytes[first_end] & 0xc0) == 0x80 {
                        first_end += 1;
                    }
                    if first_end < t_bytes.len() {
                        let first_char = &text[..first_end];
                        for s_idx in 0..scored_len {
                            if scratch.aux_decoded.text(scored_indices[s_idx]).ends_with(first_char) {
                                scored_evidence[s_idx] = scored_evidence[s_idx].max(weight);
                            }
                        }
                    }
                }
            }
            bucket += 1;
            scanned += 1;
        }
    }

    if !scored_evidence[..scored_len].iter().any(|&ev| ev > 0) {
        return;
    }

    let mut perm: [usize; SCORED] = std::array::from_fn(|i| i);
    perm[..scored_len].sort_by(|&a, &b| {
        if scored_evidence[a] == scored_evidence[b] {
            scored_order[a].cmp(&scored_order[b])
        } else {
            scored_evidence[b].cmp(&scored_evidence[a])
        }
    });

    for &p in &perm[..scored_len] {
        out.push(
            scratch.aux_decoded.text(scored_indices[p]),
            count,
            CandidateKind::Partial as u32,
            CandidateScoreKind::None as u32,
            0.0,
        );
    }
}

/// Checks predictive segments across tail lengths `1..=3`.
pub fn predictive_segments_into(
    lexicon: &Lexicon,
    input: &str,
    hans: bool,
    scratch: &mut DecoderScratchSoA,
    out: &mut DecodeOutputSoA,
) {
    out.clear();
    if input.len() < 4 || input.len() > 64 {
        return;
    }
    for tail in 1..=3.min(input.len() - 1) {
        predictive_segments_at_into(lexicon, input, hans, tail, scratch, out);
        if !out.is_empty() {
            return;
        }
    }
}

/// Offers smaller fully-spelled Pinyin phrase splits for a complete sentence.
pub fn pinyin_segments_into(
    lexicon: &Lexicon,
    input: &str,
    hans: bool,
    scratch: &mut DecoderScratchSoA,
    out: &mut DecodeOutputSoA,
) {
    out.clear();
    if !lexicon.pinyin || input.len() < 6 || input.len() > 64 {
        return;
    }
    let mut inspected = 0usize;
    let min_cut = if input.len() > 8 { input.len() - 8 } else { 2 };
    let input_bytes = input.as_bytes();
    let mut cut = input.len() - 1;
    while cut >= min_cut && inspected < 5 && out.len() < 3 {
        if !is_tone(input_bytes[cut], true) && input_bytes[cut] != b'\'' {
            decode_aux(lexicon, &input[cut..], hans, true, scratch);
            if !scratch.aux_decoded.is_empty() {
                inspected += 1;
                decode_aux(lexicon, &input[..cut], hans, true, scratch);
                if !scratch.aux_decoded.is_empty() {
                    let first_text = scratch.aux_decoded.text(0);
                    if utf8_char_count(first_text) >= 2 {
                        out.push(first_text, cut, CandidateKind::Partial as u32, CandidateScoreKind::None as u32, 0.0);
                    }
                }
            }
        }
        if cut == 0 {
            break;
        }
        cut -= 1;
    }
}

/// Returns `true` when `suffix` is a prefix of a recorded single-character Pinyin syllable
/// and not itself a complete single-character Pinyin reading.
pub fn incomplete_pinyin_syllable(lexicon: &Lexicon, suffix: &str) -> bool {
    if suffix.is_empty() || suffix.len() > 5 {
        return false;
    }
    let mut bucket = lexicon.lower_bound_bucket(suffix);
    let mut completion = false;
    let mut scanned = 0usize;
    while bucket < lexicon.buckets.len() && scanned < 64 {
        let bkey = lexicon.buckets.key(&lexicon.storage, bucket);
        if !bkey.starts_with(suffix) {
            break;
        }
        if bkey.len() <= 6 {
            let (lo, hi) = lexicon.buckets.range(bucket);
            for i in lo..hi.min(lo + 64) {
                if !lexicon.words.is_single_han_hant(i) || !lexicon.words.is_fully_spelled(i) {
                    continue;
                }
                if bkey.len() == suffix.len() {
                    return false;
                }
                completion = true;
                break;
            }
        }
        bucket += 1;
        scanned += 1;
    }
    completion
}

/// Preserves a multi-word sentence while its trailing Pinyin syllable is incomplete.
pub fn unfinished_sentence_into(
    lexicon: &Lexicon,
    input: &str,
    hans: bool,
    onset_only: bool,
    syllable_only: bool,
    scratch: &mut DecoderScratchSoA,
    out: &mut DecodeOutputSoA,
) {
    out.clear();
    if !lexicon.pinyin || input.len() < 4 || input.len() > MAX_INPUT {
        return;
    }
    let input_bytes = input.as_bytes();
    for tail in 1..=5.min(input.len() - 1) {
        let suffix = &input[input.len() - tail..];
        if !suffix.bytes().all(|c| (b'a'..=b'z').contains(&c)) {
            continue;
        }
        if syllable_only && !incomplete_pinyin_syllable(lexicon, suffix) {
            continue;
        }
        if onset_only {
            if !PINYIN_ONSET_INITIALS.contains(&suffix) {
                continue;
            }
            if suffix == "n" && (input.len() < 2 || input_bytes[input.len() - 2] != b'n') {
                continue;
            }
        }
        let possible = lexicon.lower_bound_bucket(suffix);
        if possible == lexicon.buckets.len()
            || !lexicon.buckets.key(&lexicon.storage, possible).starts_with(suffix)
        {
            continue;
        }
        let count = input.len() - tail;
        decode_aux(lexicon, &input[..count], hans, true, scratch);
        let min_chars = if onset_only { 3 } else { 2 };
        for i in 0..scratch.aux_decoded.len() {
            let phrase = scratch.aux_decoded.text(i);
            if utf8_char_count(phrase) >= min_chars {
                out.push(phrase, count, CandidateKind::Partial as u32, CandidateScoreKind::None as u32, 0.0);
            }
            if out.len() == 3 {
                break;
            }
        }
        if !out.is_empty() {
            return;
        }
    }
}

/// Offers completed Zhuyin phrase prefixes while keeping following raw keys intact.
pub fn zhuyin_segments_into(
    lexicon: &Lexicon,
    input: &str,
    hans: bool,
    scratch: &mut DecoderScratchSoA,
    out: &mut DecodeOutputSoA,
) {
    out.clear();
    if lexicon.pinyin || input.len() < 4 {
        return;
    }
    let upper = (input.len() - 1).min(64);
    let long_composition = input.len() > 64;
    let input_bytes = input.as_bytes();

    let mut inspected = 0usize;
    let tone_limit = if long_composition { 2 } else { 6 };
    for count in (1..=upper).rev() {
        if inspected >= tone_limit {
            break;
        }
        if !is_tone(input_bytes[count - 1], false)
            || is_tone(input_bytes[count], false)
            || input_bytes[count] == b'\''
        {
            continue;
        }
        inspected += 1;
        decode_aux(lexicon, &input[..count], hans, false, scratch);
        if !scratch.aux_decoded.is_empty() {
            let phrase = scratch.aux_decoded.text(0);
            if utf8_char_count(phrase) >= 2 {
                out.push(phrase, count, CandidateKind::Partial as u32, CandidateScoreKind::None as u32, 0.0);
            }
        }
    }

    inspected = 0;
    let init_limit = if long_composition { 2 } else { 4 };
    for count in (4..=upper).rev() {
        if inspected >= init_limit || out.len() >= 6 {
            break;
        }
        if is_tone(input_bytes[count - 1], false) || !KEYS[..21].contains(&input_bytes[count]) {
            continue;
        }
        inspected += 1;
        decode_aux(lexicon, &input[..count], hans, false, scratch);
        if !scratch.aux_decoded.is_empty() {
            let phrase = scratch.aux_decoded.text(0);
            if utf8_char_count(phrase) >= 2 {
                out.push(phrase, count, CandidateKind::Partial as u32, CandidateScoreKind::None as u32, 0.0);
            }
        }
    }
}
