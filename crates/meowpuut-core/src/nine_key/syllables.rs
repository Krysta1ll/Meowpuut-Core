#![forbid(unsafe_code)]
//! Zero-allocation Pinyin syllable rail enumeration for nine-key composition.
//!
//! Given raw nine-key input (`2`–`9`, locked `A`–`Z`, and `'`), enumerates valid
//! leading Pinyin syllables (or single-letter initials on a 1-digit input) for
//! the first unlocked digit span so the client can render a scrollable syllable
//! filter bar without heap allocation.
//!
//! # Examples
//!
//! ```
//! use std::sync::Arc;
//! use meowpuut_core::{Lexicon, Session};
//!
//! let tsv = concat!(
//!     "MEOWPUUT-PINYIN-1\n",
//!     "wa\twa1\t哇\t哇\t5800\n",
//!     "wan\twan2\t完\t完\t7200\n",
//!     "wang\twang3\t往\t往\t7050\n",
//!     "yang\tyang2\t陽\t阳\t6500\n",
//! );
//! let lex = Arc::new(Lexicon::load(tsv).unwrap());
//! let mut session = Session::new_nine_key(lex).unwrap();
//! for b in b"9264" {
//!     session.key(i32::from(*b));
//! }
//! let mut out = [""; 32];
//! let n = session.nine_key_syllables(&mut out);
//! assert_eq!(&out[..n], &["wang", "yang", "wan", "wa"]);
//! ```

use crate::lexicon::Lexicon;
use crate::reading::is_tone;

use super::{key_letters, signature_keys, NineKeyIndex, SEPARATOR};

const MAX_SYLLABLE_LEN: usize = 6;
const COMMON_FLOOR: i32 = 2000;
const INITIAL_STRINGS: [&str; 26] = [
    "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m",
    "n", "o", "p", "q", "r", "s", "t", "u", "v", "w", "x", "y", "z",
];

/// Returns the first unlocked digit slice (`'2'`..=`'9'`) in `raw` after any locked `'A'`..=`'Z'` prefix.
pub fn first_unlocked_digits(raw: &[u8]) -> &[u8] {
    let mut i = 0;
    while i < raw.len() && (raw[i].is_ascii_uppercase() || raw[i] == SEPARATOR) {
        i += 1;
    }
    let start = i;
    while i < raw.len() && (b'2'..=b'9').contains(&raw[i]) {
        i += 1;
    }
    &raw[start..i]
}

fn single_syllable_weight(lexicon: &Lexicon, lo: usize, hi: usize) -> Option<i32> {
    let mut best = None;
    for row in lo..hi {
        if !lexicon.words.is_fully_spelled(row) || lexicon.words.is_no_suggest(row) {
            continue;
        }
        let r = lexicon.words.reading(&lexicon.storage, row).as_bytes();
        if r.iter().filter(|&&b| is_tone(b, true)).count() == 1 {
            let w = lexicon.words.weight(row);
            best = Some(best.map_or(w, |cur: i32| cur.max(w)));
        }
    }
    best
}

/// Fills `out` with up to 32 valid Pinyin syllables matching prefixes of the first
/// unlocked digit run in `raw`, ordered longest common syllable first.
pub fn leading_syllables_into<'a>(
    index: &NineKeyIndex,
    lexicon: &'a Lexicon,
    raw: &[u8],
    out: &mut [&'a str; 32],
) -> usize {
    let digits = first_unlocked_digits(raw);
    if digits.is_empty() {
        return 0;
    }
    let mut common: [(&'a str, usize, i32); 32] = [("", 0, 0); 32];
    let mut rare: [(&'a str, usize, i32); 32] = [("", 0, 0); 32];
    let (mut c_len, mut r_len) = (0usize, 0usize);

    let max_len = digits.len().min(MAX_SYLLABLE_LEN);
    if max_len >= 2 {
        for len in (2..=max_len).rev() {
            let prefix = &digits[..len];
            let hash = signature_keys(prefix);
            let mut group: [(&'a str, i32); 16] = [("", 0); 16];
            let mut g_len = 0usize;
            for b_row in index.chinese_bucket_rows(lexicon, prefix, hash) {
                let (lo, hi) = lexicon.buckets.range(b_row);
                if let Some(w) = single_syllable_weight(lexicon, lo, hi) {
                    if g_len < group.len() {
                        group[g_len] = (lexicon.buckets.key(&lexicon.storage, b_row), w);
                        g_len += 1;
                    }
                }
            }
            group[..g_len].sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
            for &(s, w) in &group[..g_len] {
                if w >= COMMON_FLOOR {
                    if c_len < common.len() {
                        common[c_len] = (s, len, w);
                        c_len += 1;
                    }
                } else if r_len < rare.len() {
                    rare[r_len] = (s, len, w);
                    r_len += 1;
                }
            }
        }
    }

    let mut total = 0usize;
    for &(s, _, _) in &common[..c_len] {
        if total < out.len() {
            out[total] = s;
            total += 1;
        }
    }
    for &(s, _, _) in &rare[..r_len] {
        if total < out.len() {
            out[total] = s;
            total += 1;
        }
    }
    if total == 0 {
        let mut inits: [(&'static str, i32); 4] = [("", 0); 4];
        let mut i_len = 0usize;
        for &letter in key_letters(digits[0]) {
            if let Some(first_row) = lexicon.initials(letter).next() {
                inits[i_len] = (
                    INITIAL_STRINGS[usize::from(letter - b'a')],
                    lexicon.words.weight(first_row),
                );
                i_len += 1;
            }
        }
        inits[..i_len].sort_unstable_by(|a, b| b.1.cmp(&a.1));
        for &(s, _) in &inits[..i_len] {
            if total < out.len() {
                out[total] = s;
                total += 1;
            }
        }
    }
    total
}
