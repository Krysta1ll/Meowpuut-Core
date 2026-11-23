#![forbid(unsafe_code)]
//! Next-word associations ("聯想詞") shown after a commit, like the native
//! Pinyin/Zhuyin keyboards.
//!
//! The lexicon's context table is an n-gram model: traditional-script phrases
//! with corpus frequencies, sorted by bytes. Given the committed text, the
//! trailing 1–3 Han characters are traditionalized and every context phrase
//! that extends them is a continuation. Longer matched context ranks higher,
//! shorter continuations rank above longer ones of the same strength, and at
//! most two leading suggestions share a first character, so 好 好嗎 好呀 好啊
//! do not fill the bar. A tail that is itself a word with continuations ends there: its later
//! characters are not read again as the start of a new word (晚安 never offers
//! 裝 or 排 from 安裝, 安排; 對不起 never offers 眼 from 不起眼). Output follows the requested script. No heap allocation once the output
//! buffers have been reserved.
//!
//! # Examples
//!
//! ```
//! use meowpuut_core::{Lexicon, workspace::DecodeOutputSoA};
//!
//! let tsv = concat!(
//!     "MEOWPUUT-PINYIN-1\n",
//!     "dian\tdian4\t電\t电\t5000\n",
//!     "men\tmen5\t們\t们\t5000\n",
//!     "wo\two3\t我\t我\t5000\n",
//!     "# context\n",
//!     "我們\t9000\n",
//!     "我是\t6000\n",
//!     "我的\t7000\n",
//!     "電影院\t5400\n",
//! );
//! let lex = Lexicon::load(tsv).unwrap();
//! let (mut raw, mut out) = (DecodeOutputSoA::with_capacity(64), DecodeOutputSoA::with_capacity(16));
//! lex.predict_into("今天我", false, 8, &mut raw, &mut out);
//! assert_eq!((out.text(0), out.text(1), out.text(2)), ("們", "的", "是"));
//! lex.predict_into("我", true, 8, &mut raw, &mut out);
//! assert_eq!(out.text(0), "们");
//! lex.predict_into("电影", true, 8, &mut raw, &mut out);
//! assert_eq!(out.text(0), "院");
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

/// Maximum continuation length in characters.
const MAX_REMAINDER_CHARS: usize = 4;
/// Score taken off per continuation character after the first: a short,
/// common next word beats a long fragment of a title or saying.
const LENGTH_PENALTY: f64 = 1200.0;
/// Leading suggestions allowed to share one first character.
const SAME_HEAD_LIMIT: usize = 2;
/// Upper bound on context rows scanned per prefix (bounds worst-case latency).
const SCAN_LIMIT: usize = 4096;
/// Candidates collected before ranking.
const RAW_LIMIT: usize = 128;
/// Score added per extra matched context character (> max corpus frequency gap).
const CONTEXT_STEP: f64 = 10000.0;
/// Scale from `Assoc` quantized u16 score to `predict_into` score units.
const ASSOC_SCORE_SCALE: f64 = 0.14;

#[inline]
fn is_han_char(ch: char) -> bool {
    let s = ch as u32;
    is_han_scalar(s) || (0xf900..=0xfaff).contains(&s) || (0x20000..=0x323af).contains(&s)
}

/// Byte offsets of the last (up to) four Han characters, oldest first.
fn trailing_han(text: &str) -> ([usize; 4], usize) {
    let mut starts = [0usize; 4];
    let mut count = 0usize;
    for (idx, ch) in text.char_indices().rev() {
        if !is_han_char(ch) || count == 4 {
            break;
        }
        count += 1;
        starts[4 - count] = idx;
    }
    let mut ordered = [0usize; 4];
    ordered[..count].copy_from_slice(&starts[4 - count..]);
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
        let (starts, full_count) = trailing_han(context);
        if full_count == 0 || (self.context.is_empty() && self.assoc().is_none()) {
            return;
        }
        // 1) Intra-word continuations from `self.context` (last 1..=3 Traditional chars).
        let ctx_skip = full_count.saturating_sub(3);
        let count = full_count - ctx_skip;
        let mut tail = [0u8; 12];
        let mut tail_len = 0usize;
        let mut char_ends = [0usize; 3];
        for i in 0..count {
            let src_i = ctx_skip + i;
            let end = if src_i + 1 < full_count { starts[src_i + 1] } else { context.len() };
            let trad = self.traditional_char(&context[starts[src_i]..end]).as_bytes();
            if tail_len + trad.len() > tail.len() {
                tail_len = 0;
                break;
            }
            tail[tail_len..tail_len + trad.len()].copy_from_slice(trad);
            tail_len += trad.len();
            char_ends[i] = tail_len;
        }
        if tail_len > 0 && !self.context.is_empty() {
            // Longest context first: k = number of trailing characters matched.
            let mut inside_word = false;
            for k in (1..=count).rev() {
                let begin = if k == count { 0 } else { char_ends[count - k - 1] };
                let prefix = &tail[begin..tail_len];
                let mut row = self.lower_bound_context(prefix);
                if inside_word && !raw.is_empty() {
                    break;
                }
                inside_word = row < self.context.len() && self.context.phrase(&self.storage, row).as_bytes() == prefix;
                let mut scanned = 0usize;
                while row < self.context.len() && scanned < SCAN_LIMIT && raw.len() < RAW_LIMIT {
                    let phrase = self.context.phrase(&self.storage, row).as_bytes();
                    if !phrase.starts_with(prefix) {
                        break;
                    }
                    let rest = &phrase[prefix.len()..];
                    let freq = self.context.frequency(row);
                    let floor = if k == 1 && count > 1 { 4500 } else { 500 };
                    let echoes = k < count && tail[..tail_len].ends_with(phrase);
                    if !rest.is_empty() && freq >= floor && !echoes {
                        if let Ok(text) = std::str::from_utf8(rest) {
                            if self.completes_no_suggest(std::str::from_utf8(&tail[..tail_len]).unwrap_or(""), text)
                                || std::str::from_utf8(phrase).is_ok_and(|p| self.is_no_suggest_text(p))
                            {
                                row += 1;
                                scanned += 1;
                                continue;
                            }
                            let chars = text.chars().count();
                            if chars > 3 {
                                row += 1;
                                scanned += 1;
                                continue;
                            }
                            let score = f64::from(freq) + CONTEXT_STEP * (k - 1) as f64
                                - LENGTH_PENALTY * (chars - 1) as f64;
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
        }
        // 2) Cross-word script-aware associations from `mimo_assoc.bin` (last 1..=4 Han chars).
        if let Some(assoc) = self.assoc() {
            let mut atail = [0u8; 16];
            let mut alen = 0usize;
            let mut aends = [0usize; 4];
            let mut ok = true;
            for i in 0..full_count {
                let end = if i + 1 < full_count { starts[i + 1] } else { context.len() };
                let src = &context[starts[i]..end];
                let norm = if simplified { self.simplified_char(src) } else { self.traditional_char(src) };
                if alen + norm.len() > atail.len() {
                    ok = false;
                    break;
                }
                atail[alen..alen + norm.len()].copy_from_slice(norm.as_bytes());
                alen += norm.len();
                aends[i] = alen;
            }
            if ok && alen > 0 {
                let full_tail = std::str::from_utf8(&atail[..alen]).unwrap_or("");
                for k in (1..=full_count).rev() {
                    if raw.len() >= RAW_LIMIT {
                        break;
                    }
                    let begin = if k == full_count { 0 } else { aends[full_count - k - 1] };
                    let key = &atail[begin..alen];
                    assoc.visit(simplified, key, |text, q| {
                        if raw.len() >= RAW_LIMIT || text.is_empty() {
                            return;
                        }
                        if self.completes_no_suggest(full_tail, text) || self.is_no_suggest_text(text) {
                            return;
                        }
                        let chars = text.chars().count();
                        if chars == 0 || chars > MAX_REMAINDER_CHARS {
                            return;
                        }
                        let score = f64::from(q) * ASSOC_SCORE_SCALE
                            + CONTEXT_STEP * (k - 1) as f64
                            - (LENGTH_PENALTY * 0.35) * (chars - 1) as f64;
                        match (0..raw.len()).find(|&i| raw.text(i) == text) {
                            Some(i) => raw.score[i] = raw.score[i].max(score),
                            None => raw.push(text, k, 0, 1, score),
                        }
                    });
                }
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
        // Spread first characters: past SAME_HEAD_LIMIT, a suggestion waits
        // behind every other one (stable, so ranks within each group hold).
        let mut spread = [0usize; RAW_LIMIT];
        let mut deferred = [0usize; RAW_LIMIT];
        let (mut kept, mut waiting) = (0usize, 0usize);
        for (pos, &i) in order[..n].iter().enumerate() {
            let head = raw.text(i).chars().next();
            let same = order[..pos].iter().filter(|&&j| raw.text(j).chars().next() == head).count();
            if same < SAME_HEAD_LIMIT {
                spread[kept] = i;
                kept += 1;
            } else {
                deferred[waiting] = i;
                waiting += 1;
            }
        }
        spread[kept..n].copy_from_slice(&deferred[..waiting]);
        for &i in spread.iter().take(n) {
            if out.len() >= limit {
                break;
            }
            let start = out.arena.len();
            if raw.score_kind[i] == 1 {
                out.arena.push_str(raw.text(i));
            } else {
                for (ci, ch) in raw.text(i).char_indices() {
                    let src = &raw.text(i)[ci..ci + ch.len_utf8()];
                    out.arena.push_str(if simplified { self.simplified_char(src) } else { src });
                }
            }
            let emitted_len = out.arena.len() - start;
            let dup = (0..out.len()).any(|j| out.text(j).as_bytes() == &out.arena.as_bytes()[start..]);
            if dup {
                out.arena.truncate(start);
                continue;
            }
            out.text_offset.push(start as u32);
            out.text_len.push(emitted_len as u16);
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

#[cfg(test)]
mod tests {
    use super::Predictor;
    use crate::Lexicon;

    fn lexicon(context: &str) -> Lexicon {
        let mut rows: Vec<&str> = context.lines().collect();
        rows.sort_unstable();
        let tsv = format!("MEOWPUUT-PINYIN-1\nwo\two3\t我\t我\t5000\n# context\n{}\n", rows.join("\n"));
        Lexicon::load(&tsv).unwrap()
    }

    fn predict(lex: &Lexicon, context: &str) -> Vec<String> {
        let mut predictor = Predictor::new();
        let n = predictor.predict(lex, context, false, 8);
        (0..n).map(|i| predictor.text(i).to_owned()).collect()
    }

    #[test]
    fn a_whole_word_tail_is_not_reread_as_a_word_start() {
        let lex = lexicon("安排\t9000\n安裝\t9000\n晚安\t8000\n晚安好夢\t6000\n");
        assert_eq!(predict(&lex, "晚安"), ["好夢"]);
        // Without continuations of its own, the shorter tail is still read.
        let lex = lexicon("什麼\t9000\n什麼時候\t8000\n為什麼\t8000\n");
        assert_eq!(predict(&lex, "為什麼"), ["時候"]);
    }

    #[test]
    fn shorter_continuations_rank_first_and_long_ones_are_dropped() {
        let lex = lexicon("我們\t9000\n我們的\t6000\n我們的日子\t6500\n我們曾是戰士\t9000\n");
        assert_eq!(predict(&lex, "我們"), ["的", "的日子"]);
    }

    #[test]
    fn at_most_two_leading_suggestions_share_a_first_character() {
        let lex = lexicon("你好\t9000\n你好嗎\t8900\n你好呀\t8800\n你好啊\t8700\n你們\t5000\n");
        assert_eq!(predict(&lex, "你"), ["好", "好嗎", "們", "好呀", "好啊"]);
    }

    #[test]
    fn assoc_cross_word_prediction_is_script_aware_and_zero_alloc() {
        let mut lex = lexicon("電影\t9000\n電影院\t7000\n");
        let bytes = crate::assoc::pack_image(
            &[("幫我", &[("看一下", 50000), ("查一下", 40000)]), ("今天天氣", &[("真好", 55000)])],
            &[("帮我", &[("看一下", 50000), ("发一下", 42000)])],
        ).unwrap();
        lex.set_assoc(crate::assoc::Assoc::load(&bytes).unwrap());
        let mut p = Predictor::new();
        assert_eq!(p.predict(&lex, "幫我", false, 4), 2);
        assert_eq!((p.text(0), p.text(1)), ("看一下", "查一下"));
        let cap = p.capacity_signature();
        assert_eq!(p.predict(&lex, "今天天氣", false, 4), 1);
        assert_eq!(p.text(0), "真好");
        assert_eq!(p.predict(&lex, "帮我", true, 4), 2);
        assert_eq!((p.text(0), p.text(1)), ("看一下", "发一下"));
        assert_eq!(p.capacity_signature(), cap);
    }
}
