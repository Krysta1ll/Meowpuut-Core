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

use crate::lexicon::Lexicon;
use crate::pairs::{END, START, STEP};
use crate::reading::{utf8_char_count, Reading};
use crate::workspace::{DecodeOutputSoA, NasalVariantsStack};
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
                if i + 1 < slen {
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
            let variants = NasalVariantsStack::from_sounds(sounds);
            for v_idx in 0..variants.len() {
                collect(variants.get(v_idx), 900.0, raw);
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
