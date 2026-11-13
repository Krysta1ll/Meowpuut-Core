#![forbid(unsafe_code)]
//! Mixed Chinese-Pinyin and Japanese-Romaji prefix completion and unfinished
//! Japanese prefix preservation.
//!
//! # Examples
//!
//! ```
//! use std::sync::Arc;
//! use meowpuut_core::{Lexicon, Session};
//!
//! let tsv = concat!(
//!     "MEOWPUUT-PINYIN-1\n",
//!     "wo\two3\t我\t我\t5600\n",
//!     "woxihuan\two3xi3huan1\t我喜歡\t我喜欢\t5600\n",
//!     "xihuan\txi3huan1\t喜歡\t喜欢\t5500\n",
//! );
//! let lex = Arc::new(Lexicon::load(tsv).unwrap());
//! let mut session = Session::new(lex);
//! assert!(session.add_japanese_word("neko", "猫", 8000));
//! let (text, consumed) = session.mixed_pinyin_prefix("woxihuannekob", false);
//! assert_eq!(text, "我喜歡猫");
//! assert_eq!(consumed, 12);
//! ```

use crate::decoder::decode_with_buffers;
use crate::lexicon::Lexicon;
use crate::mixed::mixed_pinyin_into;
use crate::reading::{utf8_char_count, MAX_INPUT};
use crate::soa::JapaneseTableSoA;
use crate::workspace::{DecodeOutputSoA, DecoderScratchSoA};
use crate::{CandidateKind, CandidateScoreKind};

const PINYIN_ONSET_INITIALS: [&str; 22] = [
    "b", "p", "f", "d", "t", "l", "g", "k", "h", "j", "q", "x", "zh", "ch", "sh", "r", "z", "c",
    "s", "y", "w", "n",
];

/// Preserves a completed Chinese phrase when a trailing suffix matches the prefix
/// of a recorded high-confidence Japanese reading.
pub fn unfinished_japanese_prefix_into(
    lexicon: &Lexicon,
    japanese: &JapaneseTableSoA,
    input: &str,
    hans: bool,
    allow_one: bool,
    scratch: &mut DecoderScratchSoA,
    out: &mut DecodeOutputSoA,
) {
    out.clear();
    if !lexicon.pinyin || input.len() < 5 || input.len() > MAX_INPUT || japanese.is_empty() {
        return;
    }
    let max_tail = 16.min(input.len() - 2);
    let min_tail = if allow_one { 1 } else { 2 };
    for tail in (min_tail..=max_tail).rev() {
        let suffix = &input[input.len() - tail..];
        let has_reading = (0..japanese.len()).any(|j| {
            let jr = japanese.reading(j);
            japanese.weight[j] >= 4000 && jr.len() > tail && jr.starts_with(suffix)
        });
        if !has_reading {
            continue;
        }
        let cut = input.len() - tail;
        decode_with_buffers(
            lexicon,
            &input[..cut],
            hans,
            true,
            false,
            true,
            &mut scratch.lattice,
            &mut scratch.step_paths,
            &mut scratch.fallback_raw,
            &mut scratch.fallback_order,
            &mut scratch.fallback_out,
            &mut scratch.aux_decoded,
        );
        for i in 0..scratch.aux_decoded.len() {
            let phrase = scratch.aux_decoded.text(i);
            if utf8_char_count(phrase) >= 2 {
                out.push(phrase, cut, CandidateKind::Partial as u32, CandidateScoreKind::None as u32, 0.0);
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

/// Decodes a leading mixed Chinese-Pinyin + Japanese-Romaji prefix and reports
/// the number of ASCII input bytes consumed.
pub fn mixed_pinyin_prefix_into(
    lexicon: &Lexicon,
    japanese: &JapaneseTableSoA,
    input: &str,
    simplified: bool,
    scratch: &mut DecoderScratchSoA,
    out: &mut String,
) -> usize {
    out.clear();
    if !lexicon.pinyin
        || input.len() < 4
        || input.len() > MAX_INPUT
        || !input.bytes().all(|c| (b'a'..=b'z').contains(&c))
    {
        return 0;
    }
    let window = if input.len() > 80 { &input[..80] } else { input };
    let window_bytes = window.as_bytes();

    if window.len() >= 6 {
        for tail in 1..=2.min(window.len() - 4) {
            let suffix = &window[window.len() - tail..];
            if !PINYIN_ONSET_INITIALS.contains(&suffix) {
                continue;
            }
            if tail == 1
                && ((suffix == "n" && window_bytes[window.len() - 2] != b'n')
                    || (suffix == "g" && window_bytes[window.len() - 2] == b'n')
                    || (suffix == "r" && window_bytes[window.len() - 2] == b'e'))
            {
                continue;
            }
            let complete_jp = (0..japanese.len()).any(|j| {
                let jr = japanese.reading(j);
                jr.len() <= window.len() && window.ends_with(jr)
            });
            if complete_jp {
                break;
            }
            let candidate = &window[..window.len() - tail];
            mixed_pinyin_into(lexicon, japanese, candidate, simplified, scratch, out);
            if !out.is_empty() {
                return candidate.len();
            }
        }
    }

    let mut attempts = 0usize;
    for end in 2..window.len() {
        for j in 0..japanese.len() {
            let jr = japanese.reading(j);
            if jr.len() > end || &window[end - jr.len()..end] != jr {
                continue;
            }
            let start = end - jr.len();
            let weight = japanese.weight[j];
            if start == 0 {
                if weight == 3500 {
                    let suffix = &window[end..];
                    let has_jp = (0..japanese.len()).any(|o| {
                        japanese.weight[o] >= 4000
                            && japanese.reading(o).len() >= 3
                            && suffix.contains(japanese.reading(o))
                    });
                    if !has_jp {
                        continue;
                    }
                }
                if weight == 3500 || jr.len() == 2 {
                    mixed_pinyin_into(lexicon, japanese, window, simplified, scratch, out);
                    if out.is_empty() {
                        continue;
                    }
                }
                out.clear();
                out.push_str(japanese.text(j));
                return end;
            } else {
                attempts += 1;
                if attempts > 8 {
                    out.clear();
                    return 0;
                }
                let prefix = &window[..end];
                decode_with_buffers(
                    lexicon,
                    prefix,
                    simplified,
                    true,
                    false,
                    true,
                    &mut scratch.lattice,
                    &mut scratch.step_paths,
                    &mut scratch.fallback_raw,
                    &mut scratch.fallback_order,
                    &mut scratch.fallback_out,
                    &mut scratch.aux_decoded,
                );
                if !scratch.aux_decoded.is_empty() {
                    continue;
                }
                mixed_pinyin_into(lexicon, japanese, prefix, simplified, scratch, out);
                if !out.is_empty() {
                    return end;
                }
            }
        }
    }

    out.clear();
    0
}
