#![forbid(unsafe_code)]
//! Next-word associations ("聯想詞") shown after a commit, like the native
//! Pinyin/Zhuyin keyboards.
//!
//! The lexicon's context table is an n-gram model: traditional-script phrases
//! with corpus frequencies, sorted by bytes. Given the committed text, the
//! trailing 1–3 Han characters are traditionalized and every context phrase
//! that extends them is a continuation. Longer matched context ranks higher.
//! Output follows the requested script. No heap allocation once the output
//! buffers have been reserved.
//!
//! # Examples
//!
//! ```
//! use meowpuut_core::{Lexicon, workspace::DecodeOutputSoA};
//!
//! let tsv = concat!(
//!     "MEOWPUUT-PINYIN-1\n",
//!     "guo\tguo2\t國\t国\t5000\n",
//!     "men\tmen5\t們\t们\t5000\n",
//!     "wo\two3\t我\t我\t5000\n",
//!     "# context\n",
//!     "中國人\t5400\n",
//!     "我們\t9000\n",
//!     "我是\t6000\n",
//!     "我的\t7000\n",
//! );
//! let lex = Lexicon::load(tsv).unwrap();
//! let (mut raw, mut out) = (DecodeOutputSoA::with_capacity(64), DecodeOutputSoA::with_capacity(16));
//! lex.predict_into("今天我", false, 8, &mut raw, &mut out);
//! assert_eq!((out.text(0), out.text(1), out.text(2)), ("們", "的", "是"));
//! lex.predict_into("我", true, 8, &mut raw, &mut out);
//! assert_eq!(out.text(0), "们");
//! lex.predict_into("中国", true, 8, &mut raw, &mut out);
//! assert_eq!(out.text(0), "人");
//! lex.predict_into("hello", false, 8, &mut raw, &mut out);
//! assert!(out.is_empty());
//!
//! // Reusable, preallocated wrapper used by clients:
//! let mut predictor = meowpuut_core::predict::Predictor::new();
//! assert_eq!(predictor.predict(&lex, "我", false, 2), 2);
//! assert_eq!(predictor.text(1), "的");
//! ```

use crate::lexicon::Lexicon;
use crate::reading::is_han_scalar;
use crate::workspace::DecodeOutputSoA;

/// Maximum continuation length in bytes (3 CJK characters).
const MAX_REMAINDER: usize = 12;
/// Upper bound on context rows scanned per prefix (bounds worst-case latency).
const SCAN_LIMIT: usize = 4096;
/// Candidates collected before ranking.
const RAW_LIMIT: usize = 128;
/// Score added per extra matched context character (> max corpus frequency gap).
const CONTEXT_STEP: f64 = 10000.0;

#[inline]
fn is_han_char(ch: char) -> bool {
    let s = ch as u32;
    is_han_scalar(s) || (0xf900..=0xfaff).contains(&s) || (0x20000..=0x323af).contains(&s)
}

/// Byte offsets of the last (up to) three Han characters, oldest first.
fn trailing_han(text: &str) -> ([usize; 3], usize) {
    let mut starts = [0usize; 3];
    let mut count = 0usize;
    for (idx, ch) in text.char_indices().rev() {
        if !is_han_char(ch) || count == 3 {
            break;
        }
        count += 1;
        starts[3 - count] = idx;
    }
    let mut ordered = [0usize; 3];
    ordered[..count].copy_from_slice(&starts[3 - count..]);
    (ordered, count)
}

impl Lexicon {
    fn lower_bound_context(&self, prefix: &[u8]) -> usize {
        let (mut size, mut left) = (self.context.len(), 0usize);
        while size > 0 {
            let half = size / 2;
            let mid = left + half;
            if self.context.phrase(&self.storage, mid).as_bytes() < prefix {
                left = mid + 1;
                size -= half + 1;
            } else {
                size = half;
            }
        }
        left
    }

    /// Fills `out` with up to `limit` continuations of `context`, best first.
    /// `raw` is caller-owned scratch (reserve ≥ 128 rows to stay allocation-free).
    pub fn predict_into(
        &self,
        context: &str,
        simplified: bool,
        limit: usize,
        raw: &mut DecodeOutputSoA,
        out: &mut DecodeOutputSoA,
    ) {
        raw.clear();
        out.clear();
        let (starts, count) = trailing_han(context);
        if count == 0 || self.context.is_empty() {
            return;
        }
        // Traditionalized tail in a stack buffer (each char ≤ 4 bytes).
        let mut tail = [0u8; 12];
        let mut tail_len = 0usize;
        let mut char_ends = [0usize; 3];
        for i in 0..count {
            let end = if i + 1 < count { starts[i + 1] } else { context.len() };
            let trad = self.traditional_char(&context[starts[i]..end]).as_bytes();
            if tail_len + trad.len() > tail.len() {
                return;
            }
            tail[tail_len..tail_len + trad.len()].copy_from_slice(trad);
            tail_len += trad.len();
            char_ends[i] = tail_len;
        }
        // Longest context first: k = number of trailing characters matched.
        for k in (1..=count).rev() {
            let begin = if k == count { 0 } else { char_ends[count - k - 1] };
            let prefix = &tail[begin..tail_len];
            let mut row = self.lower_bound_context(prefix);
            let mut scanned = 0usize;
            while row < self.context.len() && scanned < SCAN_LIMIT && raw.len() < RAW_LIMIT {
                let phrase = self.context.phrase(&self.storage, row).as_bytes();
                if !phrase.starts_with(prefix) {
                    break;
                }
                let rest = &phrase[prefix.len()..];
                let freq = self.context.frequency(row);
                // A 1-char fallback behind a longer context is weak evidence:
                // only keep common continuations so the bar stays clean.
                let floor = if k == 1 && count > 1 { 4500 } else { 500 };
                // Skip a fallback that just re-forms the word already typed
                // (e.g. 謝 + 謝 after 謝謝).
                let echoes = k < count && tail[..tail_len].ends_with(phrase);
                if !rest.is_empty() && rest.len() <= MAX_REMAINDER && freq >= floor && !echoes {
                    if let Ok(text) = std::str::from_utf8(rest) {
                        let score = f64::from(freq) + CONTEXT_STEP * (k - 1) as f64;
                        match (0..raw.len()).find(|&i| raw.text(i) == text) {
                            Some(i) => raw.score[i] = raw.score[i].max(score),
                            None => raw.push(text, k, 0, 0, score),
                        }
                    }
                }
                row += 1;
                scanned += 1;
            }
        }
        let mut order = [0usize; RAW_LIMIT];
        let n = raw.len();
        for (i, slot) in order.iter_mut().enumerate().take(n) {
            *slot = i;
        }
        order[..n].sort_by(|&a, &b| {
            raw.score[b].total_cmp(&raw.score[a]).then_with(|| raw.text(a).cmp(raw.text(b)))
        });
        for &i in order.iter().take(n.min(limit)) {
            let start = out.arena.len();
            for (ci, ch) in raw.text(i).char_indices() {
                let src = &raw.text(i)[ci..ci + ch.len_utf8()];
                out.arena.push_str(if simplified { self.simplified_char(src) } else { src });
            }
            out.text_offset.push(start as u32);
            out.text_len.push((out.arena.len() - start) as u16);
            out.consumed.push(raw.consumed[i]);
            out.kind.push(0);
            out.score_kind.push(0);
            out.score.push(raw.score[i]);
        }
    }
}

/// Preallocated association buffers; one per client session.
#[derive(Debug, Clone)]
pub struct Predictor {
    raw: DecodeOutputSoA,
    out: DecodeOutputSoA,
}

impl Default for Predictor {
    fn default() -> Self {
        Self::new()
    }
}

impl Predictor {
    /// Reserves enough capacity that `predict` never allocates.
    pub fn new() -> Self {
        Self { raw: DecodeOutputSoA::with_capacity(RAW_LIMIT), out: DecodeOutputSoA::with_capacity(64) }
    }

    /// Computes up to `limit` (≤ 64) continuations; returns how many were found.
    pub fn predict(&mut self, lexicon: &Lexicon, context: &str, simplified: bool, limit: usize) -> usize {
        lexicon.predict_into(context, simplified, limit.min(64), &mut self.raw, &mut self.out);
        self.out.len()
    }

    /// Number of continuations from the last `predict`.
    pub fn len(&self) -> usize {
        self.out.len()
    }

    /// Whether the last `predict` found nothing.
    pub fn is_empty(&self) -> bool {
        self.out.is_empty()
    }

    /// Continuation `index` (empty string when out of range).
    pub fn text(&self, index: usize) -> &str {
        if index < self.out.len() { self.out.text(index) } else { "" }
    }

    /// Capacity signature for zero-allocation tests.
    pub fn capacity_signature(&self) -> usize {
        self.raw.arena.capacity() + self.out.arena.capacity() + self.raw.score.capacity() + self.out.score.capacity()
    }
}
