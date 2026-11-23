#![forbid(unsafe_code)]
//! 2D QWERTY spatial confusion costs and context-guided real-word typo recovery.
//!
//! Standard key-edit fallback (`crate::fallback::key_edit_corrections_into`) skips
//! inputs whose letters happen to segment into valid syllables ("fake-valid" or
//! real-word typos such as `shijiabugou` for `shijianbugou`, `kashi` for `kaishi`,
//! or `woxianquchifan` for `woxiangquchifan`). This module scores physical QWERTY
//! distances, Pinyin digraph transpositions, and high-probability Pinyin letter
//! omissions, then rescues fake-valid typos using full Bigram + Stage-2 Trigram
//! sentence scores without any heap allocations.
//!
//! # Examples
//!
//! ```
//! use meowpuut_core::spatial::{key_sub_adjustment, swap_adjustment, is_targeted_pinyin_insert};
//!
//! assert!(key_sub_adjustment(b'e', b'r') < key_sub_adjustment(b'e', b'd'));
//! assert!(swap_adjustment(b'n', b'g') < 0.0);
//! assert!(is_targeted_pinyin_insert(b"woxian", 6, b'g'));
//! assert!(is_targeted_pinyin_insert(b"kashi", 2, b'i'));
//! ```

use crate::decoder::decode_with_buffers;
use crate::lexicon::Lexicon;
use crate::merge;
use crate::reading::{strict_han_count, utf8_char_count};
use crate::workspace::{DecodeOutputSoA, DecoderScratchSoA};
use crate::{CandidateKind, CandidateScoreKind};

const MAX_COVERED_LEN: usize = 32;
const MIN_COVERED_LEN: usize = 5;
const MAX_WORD_SPAN: usize = 24;
const TOP_EDITS: usize = 2;
/// Minimum word-cover improvement (nats) before running full Bigram+Trigram decode.
const COVER_PREFILTER: f64 = 1.6;
/// Minimum full-LM gain (after spatial edit cost) required to offer a covered typo fix.
const FULL_LM_MARGIN: f64 = 3.2;
/// Extra margin when the literal input already has a strong exact whole-word entry.
const EXACT_WORD_EXTRA_MARGIN: f64 = 3.5;

/// Physical staggered QWERTY key positions `(x, y)` in key-pitch units for `a..=z`.
const fn qwerty_pos(ch: u8) -> (f64, f64) {
    match ch {
        b'q' => (0.00, 0.0), b'w' => (1.00, 0.0), b'e' => (2.00, 0.0), b'r' => (3.00, 0.0),
        b't' => (4.00, 0.0), b'y' => (5.00, 0.0), b'u' => (6.00, 0.0), b'i' => (7.00, 0.0),
        b'o' => (8.00, 0.0), b'p' => (9.00, 0.0),
        b'a' => (0.25, 1.0), b's' => (1.25, 1.0), b'd' => (2.25, 1.0), b'f' => (3.25, 1.0),
        b'g' => (4.25, 1.0), b'h' => (5.25, 1.0), b'j' => (6.25, 1.0), b'k' => (7.25, 1.0),
        b'l' => (8.25, 1.0),
        b'z' => (0.75, 2.0), b'x' => (1.75, 2.0), b'c' => (2.75, 2.0), b'v' => (3.75, 2.0),
        b'b' => (4.75, 2.0), b'n' => (5.75, 2.0), b'm' => (6.75, 2.0),
        _ => (99.0, 99.0),
    }
}

/// Squared Euclidean distance on staggered QWERTY between lowercase ASCII letters.
#[inline]
pub fn qwerty_dist_sq(a: u8, b: u8) -> f64 {
    let (ax, ay) = qwerty_pos(a);
    let (bx, by) = qwerty_pos(b);
    let (dx, dy) = (ax - bx, ay - by);
    dx * dx + dy * dy
}

/// Spatial cost adjustment (nats) for substituting `typed` with `intended`.
/// Negative favors tight horizontal thumb slips (`e↔r`, `i↔o`, `n↔m`); positive penalizes farther keys.
#[inline]
pub fn key_sub_adjustment(typed: u8, intended: u8) -> f64 {
    let d2 = qwerty_dist_sq(typed, intended);
    if d2 <= 1.01 {
        -0.30
    } else if d2 <= 1.50 {
        0.05
    } else {
        0.45
    }
}

/// Transposition cost adjustment (nats) for swapping adjacent keys `(a, b)`.
/// Common Pinyin fast-typing digraph reversals receive a bonus.
#[inline]
pub fn swap_adjustment(a: u8, b: u8) -> f64 {
    match (a, b) {
        (b'g', b'n') | (b'n', b'g') | (b'e', b'i') | (b'i', b'e')
        | (b'o', b'u') | (b'u', b'o') | (b'a', b'i') | (b'i', b'a')
        | (b'a', b'o') | (b'o', b'a') | (b'a', b'n') | (b'n', b'a')
        | (b'e', b'n') | (b'n', b'e') | (b'i', b'n') | (b'n', b'i')
        | (b'i', b'u') | (b'u', b'i') | (b'u', b'n') | (b'n', b'u')
        | (b'h', b'z') | (b'h', b'c') | (b'h', b's') => -0.35,
        _ => 0.15,
    }
}

/// Returns `true` when inserting `ch` at `pos` inside `letters` repairs a common
/// Pinyin letter omission (dropped `-g`, `-n`, `-i`, `-u`, `-h`, or medial vowel).
#[inline]
pub fn is_targeted_pinyin_insert(letters: &[u8], pos: usize, ch: u8) -> bool {
    if pos == 0 || pos > letters.len() {
        return false;
    }
    let prev = letters[pos - 1];
    let next = letters.get(pos).copied().unwrap_or(0);
    match ch {
        b'g' => prev == b'n',
        b'n' => matches!(prev, b'a' | b'e' | b'i' | b'u') && !matches!(next, b'a' | b'e' | b'i' | b'o' | b'u'),
        b'i' => matches!(prev, b'a' | b'e' | b'u' | b'j' | b'q' | b'x' | b'y' | b'l' | b'n' | b't' | b'd' | b'm' | b'b' | b'p'),
        b'u' => matches!(prev, b'a' | b'o' | b'g' | b'k' | b'h' | b'z' | b'c' | b's' | b'd' | b't' | b'l' | b'j' | b'q' | b'x'),
        b'a' => !matches!(prev, b'a' | b'e' | b'o'),
        b'e' => !matches!(prev, b'a' | b'e' | b'o'),
        b'o' => !matches!(prev, b'e' | b'o'),
        _ => false,
    }
}

/// Staggered QWERTY physical neighbours for fast covered-typo search.
#[inline]
fn tight_neighbors(ch: u8) -> &'static [u8] {
    match ch {
        b'q' => b"wa", b'w' => b"qeas", b'e' => b"wrsd", b'r' => b"etdf", b't' => b"ryfg",
        b'y' => b"tugh", b'u' => b"yihj", b'i' => b"uojk", b'o' => b"ipkl", b'p' => b"ol",
        b'a' => b"qwsz", b's' => b"wedazx", b'd' => b"erfsxc", b'f' => b"rtgdcv", b'g' => b"tyhfvb",
        b'h' => b"yujgbn", b'j' => b"uikhnm", b'k' => b"iojlm", b'l' => b"opk",
        b'z' => b"asx", b'x' => b"sdzc", b'c' => b"dfxv", b'v' => b"fgcb", b'b' => b"ghvn",
        b'n' => b"hjbm", b'm' => b"jkn", _ => b"",
    }
}

/// Fast zero-alloc dictionary-word cover score for `letters` (sum of heaviest non-nosuggest word spans).
fn fast_word_cover(lexicon: &Lexicon, letters: &[u8]) -> f64 {
    let n = letters.len();
    if n == 0 || n > MAX_COVERED_LEN + 1 {
        return f64::NEG_INFINITY;
    }
    let mut dp = [f64::NEG_INFINITY; MAX_COVERED_LEN + 2];
    dp[0] = 0.0;
    for i in 0..n {
        if dp[i] == f64::NEG_INFINITY {
            continue;
        }
        let max_end = (i + MAX_WORD_SPAN).min(n);
        for end in i + 1..=max_end {
            let Ok(span) = std::str::from_utf8(&letters[i..end]) else { continue };
            let (lo, hi) = lexicon.range(span);
            if lo == hi {
                continue;
            }
            let best = (lo..hi.min(lo + 6))
                .filter(|&row| lexicon.words.is_fully_spelled(row) && !lexicon.words.is_no_suggest(row))
                .map(|row| f64::from(lexicon.words.weight(row)) / 700.0 - 19.0)
                .fold(f64::NEG_INFINITY, f64::max);
            if best > f64::NEG_INFINITY {
                let cand = dp[i] + best;
                if cand > dp[end] {
                    dp[end] = cand;
                }
            }
        }
    }
    dp[n]
}

/// Converts a decoder candidate score to lattice nats, handling both pair-ranked
/// (`Lexicon` already in nats) and unranked (`Lexicon` in raw weight units) lexicons.
#[inline]
pub(crate) fn score_to_nats(lexicon: &Lexicon, score_kind: u32, score: f64) -> Option<f64> {
    if score_kind == CandidateScoreKind::Path as u32 {
        Some(score)
    } else if score_kind == CandidateScoreKind::Lexicon as u32 {
        if lexicon.pairs().is_some() { Some(score) } else { Some(score / 700.0 - 19.0) }
    } else {
        None
    }
}

/// Rescues "fake-valid" (real-word) Pinyin typos whose letters happen to segment into valid
/// syllables (`woxianquchifan` → `我想去吃飯`, `shijiabugou` → `時間不夠`, `kashi` → `開始`),
/// verified against full Bigram + Stage-2 Trigram sentence scores with zero heap allocations.
pub(crate) fn covered_typo_corrections_into(
    lexicon: &Lexicon,
    input: &str,
    hans: bool,
    literal_score: f64,
    literal_is_exact: bool,
    scratch: &mut DecoderScratchSoA,
    out: &mut DecodeOutputSoA,
) {
    out.clear();
    let letters = input.as_bytes();
    let n = letters.len();
    if !lexicon.pinyin || !(MIN_COVERED_LEN..=MAX_COVERED_LEN).contains(&n) || !letters.iter().all(u8::is_ascii_lowercase) {
        return;
    }
    let base_cover = fast_word_cover(lexicon, letters);
    if base_cover == f64::NEG_INFINITY {
        return;
    }
    let floor = base_cover + COVER_PREFILTER;
    let mut best_cover = [f64::NEG_INFINITY; TOP_EDITS];
    let mut best_pen = [0.0f64; TOP_EDITS];
    let mut kept = [[0u8; MAX_COVERED_LEN + 2]; TOP_EDITS];
    let mut kept_len = [0usize; TOP_EDITS];
    let mut buf = [0u8; MAX_COVERED_LEN + 2];

    let mut consider = |len: usize, cand: &[u8], pen: f64| {
        if cand == letters {
            return;
        }
        let sc = fast_word_cover(lexicon, cand) - pen;
        if sc <= floor.max(best_cover[TOP_EDITS - 1]) {
            return;
        }
        if (0..TOP_EDITS).any(|k| kept_len[k] == len && &kept[k][..len] == cand) {
            return;
        }
        let mut slot = TOP_EDITS - 1;
        while slot > 0 && best_cover[slot - 1] < sc {
            best_cover[slot] = best_cover[slot - 1];
            best_pen[slot] = best_pen[slot - 1];
            kept[slot] = kept[slot - 1];
            kept_len[slot] = kept_len[slot - 1];
            slot -= 1;
        }
        best_cover[slot] = sc;
        best_pen[slot] = pen;
        kept[slot][..len].copy_from_slice(cand);
        kept_len[slot] = len;
    };

    // 1) Targeted Pinyin letter omissions (e.g. woxian->woxiang, shijia->shijian, kashi->kaishi).
    //    `h` after `z/c/s` is left to `Fuzz::Initial` so its provenance stays `FuzzyInitial`.
    const INSERT_CHARS: &[u8] = b"gniuaeo";
    for pos in 1..=n {
        for &ch in INSERT_CHARS {
            if !is_targeted_pinyin_insert(letters, pos, ch) {
                continue;
            }
            buf[..pos].copy_from_slice(&letters[..pos]);
            buf[pos] = ch;
            buf[pos + 1..n + 1].copy_from_slice(&letters[pos..]);
            consider(n + 1, &buf[..n + 1], 0.45);
        }
    }
    // 2) Adjacent transpositions (e.g. keiy->keyi, zheg->zhen? / xianazi->xianzai).
    for i in 0..n - 1 {
        if letters[i] == letters[i + 1] {
            continue;
        }
        buf[..n].copy_from_slice(letters);
        buf.swap(i, i + 1);
        consider(n, &buf[..n], (0.60 + swap_adjustment(letters[i], letters[i + 1])).max(0.15));
    }
    // 3) Tight QWERTY neighbor substitutions.
    for i in 0..n {
        for &near in tight_neighbors(letters[i]) {
            buf[..n].copy_from_slice(letters);
            buf[i] = near;
            consider(n, &buf[..n], (0.80 + key_sub_adjustment(letters[i], near)).max(0.25));
        }
    }

    let margin = FULL_LM_MARGIN + if literal_is_exact { EXACT_WORD_EXTRA_MARGIN } else { 0.0 };
    for k in 0..TOP_EDITS {
        if best_cover[k] == f64::NEG_INFINITY {
            break;
        }
        let revised = std::str::from_utf8(&kept[k][..kept_len[k]]).unwrap_or("");
        scratch.lattice.skip_no_suggest = true;
        decode_with_buffers(
            lexicon, revised, hans, true, false, false,
            &mut scratch.lattice, &mut scratch.step_paths,
            &mut scratch.fallback_raw, &mut scratch.fallback_order,
            &mut scratch.fallback_out, &mut scratch.aux_decoded,
        );
        scratch.lattice.skip_no_suggest = false;
        let Some(idx) = (0..scratch.aux_decoded.len().min(3)).find(|&i| {
            let text = scratch.aux_decoded.text(i);
            scratch.aux_decoded.consumed[i] == revised.len()
                && strict_han_count(text) > 1
                && strict_han_count(text) == utf8_char_count(text)
        }) else {
            continue;
        };
        let Some(cand_score) = score_to_nats(lexicon, scratch.aux_decoded.score_kind[idx], scratch.aux_decoded.score[idx]) else {
            continue;
        };
        let gain = cand_score - best_pen[k] - literal_score;
        if gain > margin {
            merge::push_best(
                out,
                scratch.aux_decoded.text(idx),
                n,
                CandidateKind::Correction as u32,
                CandidateScoreKind::Fallback as u32,
                gain - margin,
            );
        }
    }
    merge::sort_from(out, 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spatial_distances_and_swaps_prioritize_tight_keys() {
        assert!(qwerty_dist_sq(b'e', b'r') < qwerty_dist_sq(b'e', b'd'));
        assert!(qwerty_dist_sq(b'a', b's') < qwerty_dist_sq(b'a', b'z'));
        assert!(key_sub_adjustment(b'i', b'o') < key_sub_adjustment(b'i', b'k'));
        assert!(key_sub_adjustment(b'e', b'd') < key_sub_adjustment(b'q', b'm'));
        assert!(swap_adjustment(b'g', b'n') < swap_adjustment(b'q', b'x'));
        assert!(is_targeted_pinyin_insert(b"shijia", 6, b'n'));
        assert!(is_targeted_pinyin_insert(b"woxian", 6, b'g'));
        assert!(!is_targeted_pinyin_insert(b"nihao", 0, b'g'));
    }

    #[test]
    fn rescues_fake_valid_covered_typo_without_allocating() {
        let tsv = concat!(
            "MEOWPUUT-PINYIN-1\n",
            "bugou\tbu4gou4\t不夠\t不够\t7000\n",
            "ka\tka3\t卡\t卡\t3200\n",
            "kaishi\tkai1shi3\t開始\t开始\t8200\n",
            "shi\tshi4\t氏\t氏\t3000\n",
            "shijia\tshi4jia1\t世家\t世家\t3200\n",
            "shijian\tshi2jian1\t時間\t时间\t8500\n",
        );
        let lex = Lexicon::load(tsv).unwrap();
        let mut scratch = DecoderScratchSoA::new();
        let mut out = DecodeOutputSoA::with_capacity(8);
        decode_with_buffers(
            &lex, "kashi", false, true, false, false,
            &mut scratch.lattice, &mut scratch.step_paths,
            &mut scratch.fallback_raw, &mut scratch.fallback_order,
            &mut scratch.fallback_out, &mut scratch.decoded,
        );
        let lit = score_to_nats(&lex, scratch.decoded.score_kind[0], scratch.decoded.score[0]).unwrap();
        covered_typo_corrections_into(&lex, "kashi", false, lit, false, &mut scratch, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out.text(0), "開始");

        // Sentence-level fake-valid typo: shijiabugou -> 時間不夠.
        decode_with_buffers(
            &lex, "shijiabugou", false, true, false, false,
            &mut scratch.lattice, &mut scratch.step_paths,
            &mut scratch.fallback_raw, &mut scratch.fallback_order,
            &mut scratch.fallback_out, &mut scratch.decoded,
        );
        let lit2 = score_to_nats(&lex, scratch.decoded.score_kind[0], scratch.decoded.score[0]).unwrap();
        covered_typo_corrections_into(&lex, "shijiabugou", false, lit2, false, &mut scratch, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out.text(0), "時間不夠");
    }
}
