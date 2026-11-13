#![forbid(unsafe_code)]
//! Mixed Chinese-Pinyin and Japanese-Romaji sentence segmentation
//! using Structure-of-Arrays (`JapaneseTableSoA`) and stack-only span caches.
//!
//! # Examples
//!
//! ```
//! use std::sync::Arc;
//! use meowpuut_core::{Lexicon, Session};
//!
//! let tsv = concat!(
//!     "MEOWPUUT-PINYIN-1\n",
//!     "de\tde5\t的\t的\t5500\n",
//!     "hen\then3\t很\t很\t5200\n",
//!     "keai\tke3ai4\t可愛\t可爱\t5400\n",
//!     "ni\tni3\t你\t你\t5500\n",
//!     "nide\tni3de5\t你的\t你的\t5400\n",
//!     "wo\two3\t我\t我\t5600\n",
//!     "woxihuan\two3xi3huan1\t我喜歡\t我喜欢\t5600\n",
//!     "xihuan\txi3huan1\t喜歡\t喜欢\t5500\n",
//! );
//! let lex = Arc::new(Lexicon::load(tsv).unwrap());
//! let mut session = Session::new(lex);
//! assert!(session.add_japanese_word("neko", "猫", 8000));
//! assert_eq!(
//!     session.mixed_pinyin("woxihuannidenekohenkeai", false),
//!     "我喜歡你的猫很可愛"
//! );
//! ```

pub use crate::mixed_prefix::{mixed_pinyin_prefix_into, unfinished_japanese_prefix_into};

use crate::decoder::{decode_with_buffers, has_exact_match};
use crate::lexicon::Lexicon;
use crate::reading::{han_characters_stack, utf8_char_count, Reading};
use crate::soa::JapaneseTableSoA;
use crate::workspace::DecoderScratchSoA;

#[derive(Debug, Clone, Copy, Default)]
struct Match {
    start: usize,
    end: usize,
    word_idx: usize,
    weight: i32,
}

#[inline]
fn match_precedes(a: &Match, b: &Match) -> bool {
    if a.start != b.start {
        return a.start < b.start;
    }
    if a.end != b.end {
        return a.end > b.end;
    }
    a.weight > b.weight
}

#[derive(Clone, Copy)]
struct SpanEntry {
    off: u16,
    len: u16,
    weight: i32,
}

struct StackSpanCache {
    keys: [(u8, u8); 64],
    entries: [SpanEntry; 64],
    arena: [u8; 4096],
    arena_len: usize,
    count: usize,
    decodes: usize,
}

impl StackSpanCache {
    fn new() -> Self {
        Self { keys: [(0, 0); 64], entries: [SpanEntry { off: 0, len: 0, weight: 0 }; 64], arena: [0u8; 4096], arena_len: 0, count: 0, decodes: 0 }
    }

    #[inline]
    fn text(&self, entry: SpanEntry) -> &str {
        std::str::from_utf8(&self.arena[entry.off as usize..(entry.off + entry.len) as usize]).unwrap_or("")
    }

    fn push_entry(&mut self, key: (u8, u8), text: &str, weight: i32) -> SpanEntry {
        let mut entry = SpanEntry { off: 0, len: 0, weight: 0 };
        let bytes = text.as_bytes();
        if !bytes.is_empty() && self.arena_len + bytes.len() <= self.arena.len() {
            entry = SpanEntry { off: self.arena_len as u16, len: bytes.len() as u16, weight };
            self.arena[self.arena_len..self.arena_len + bytes.len()].copy_from_slice(bytes);
            self.arena_len += bytes.len();
        }
        if self.count < 64 {
            self.keys[self.count] = key;
            self.entries[self.count] = entry;
            self.count += 1;
        }
        entry
    }

    fn lookup(&mut self, lexicon: &Lexicon, input: &str, simplified: bool, start: usize, end: usize, sc: &mut DecoderScratchSoA) -> SpanEntry {
        if start >= end {
            return SpanEntry { off: 0, len: 0, weight: 0 };
        }
        let key = (start as u8, end as u8);
        if let Some(i) = (0..self.count).find(|&i| self.keys[i] == key) {
            return self.entries[i];
        }
        let part = &input[start..end];
        let parsed = Reading::new(part, true);
        let (lo, hi) = lexicon.range(parsed.sounds());
        for i in lo..hi {
            let w = lexicon.words.weight(i);
            if w < 2000 { break; }
            if parsed.matches(lexicon.words.reading(&lexicon.storage, i).as_bytes(), 0, parsed.len()) {
                return self.push_entry(key, lexicon.words.text(&lexicon.storage, i, simplified), w);
            }
        }
        if self.decodes < 12 {
            self.decodes += 1;
            decode_with_buffers(lexicon, part, simplified, true, false, true, &mut sc.lattice, &mut sc.step_paths, &mut sc.fallback_raw, &mut sc.fallback_order, &mut sc.fallback_out, &mut sc.aux_decoded);
            // The Chinese part is Han text: no English words (sushiswap is not 寿司swap).
            if !sc.aux_decoded.is_empty() && !sc.aux_decoded.text(0).bytes().any(|b| b.is_ascii_alphabetic()) {
                return self.push_entry(key, sc.aux_decoded.text(0), 2000);
            }
        }
        self.push_entry(key, "", 0)
    }
}

/// Decodes a mixed Chinese-Pinyin + Japanese-Romaji sentence into `out`.
pub fn mixed_pinyin_into(
    lexicon: &Lexicon,
    japanese: &JapaneseTableSoA,
    input: &str,
    simplified: bool,
    scratch: &mut DecoderScratchSoA,
    out: &mut String,
) {
    out.clear();
    if !lexicon.pinyin
        || input.len() < 5
        || input.len() > 80
        || !input.bytes().all(|c| (b'a'..=b'z').contains(&c))
    {
        return;
    }

    let mut shortlist = [Match::default(); 32];
    let mut shortlist_len = 0usize;
    for j in 0..japanese.len() {
        let reading = japanese.reading(j);
        let weight = japanese.weight[j];
        let mut search_from = 0usize;
        while search_from <= input.len() {
            let Some(rel) = input[search_from..].find(reading) else { break };
            let pos = search_from + rel;
            search_from = pos + 1;
            if pos == 0 && reading.len() == input.len() {
                continue;
            }
            let m = Match { start: pos, end: pos + reading.len(), word_idx: j, weight };
            if shortlist_len == 32 && !match_precedes(&m, &shortlist[31]) {
                break;
            }
            let ins = (0..shortlist_len).find(|&k| match_precedes(&m, &shortlist[k])).unwrap_or(shortlist_len);
            if shortlist_len < 32 {
                shortlist_len += 1;
            }
            for k in (ins + 1..shortlist_len).rev() {
                shortlist[k] = shortlist[k - 1];
            }
            if ins < shortlist_len {
                shortlist[ins] = m;
            }
        }
    }

    let mut matches = [Match::default(); 16];
    let mut matches_len = 0usize;
    for s_idx in 0..shortlist_len {
        let m = shortlist[s_idx];
        let m_text = japanese.text(m.word_idx);
        let covered = (0..matches_len).any(|c_idx| {
            let other = matches[c_idx];
            other.start == m.start && other.end > m.end && japanese.text(other.word_idx).starts_with(m_text)
        });
        if !covered {
            matches[matches_len] = m;
            matches_len += 1;
            if matches_len == 16 {
                break;
            }
        }
    }

    let mut cache = StackSpanCache::new();
    let mut best_score = -1i32;

    let mut consider = |first: Match,
                        second: Option<Match>,
                        third: Option<Match>,
                        sc: &mut DecoderScratchSoA,
                        best_out: &mut String| {
        if first.weight == 3500
            && second.map_or(true, |s| s.weight == 3500)
            && third.map_or(true, |t| t.weight == 3500)
        {
            return;
        }
        let end = third.map_or_else(|| second.map_or(first.end, |s| s.end), |t| t.end);
        if first.start == 0
            && end == input.len()
            && second.map_or(true, |s| first.end == s.start)
            && third.map_or(true, |t| second.map_or(false, |s| s.end == t.start))
        {
            return;
        }
        let before = cache.lookup(lexicon, input, simplified, 0, first.start, sc);
        let middle = second.map_or(SpanEntry { off: 0, len: 0, weight: 0 }, |s| {
            cache.lookup(lexicon, input, simplified, first.end, s.start, sc)
        });
        let middle2 = third.map_or(SpanEntry { off: 0, len: 0, weight: 0 }, |t| {
            let s_end = second.map_or(first.end, |s| s.end);
            cache.lookup(lexicon, input, simplified, s_end, t.start, sc)
        });
        let after = cache.lookup(lexicon, input, simplified, end, input.len(), sc);

        if (first.start > 0 && before.len == 0)
            || second.map_or(false, |s| first.end != s.start && middle.len == 0)
            || third.map_or(false, |t| second.map_or(first.end, |s| s.end) != t.start && middle2.len == 0)
            || (end < input.len() && after.len == 0)
        {
            return;
        }

        let short_reading = [Some(first), second, third]
            .into_iter()
            .flatten()
            .any(|m| japanese.reading(m.word_idx).len() == 2);
        if short_reading {
            let strong_ch = [before, middle, middle2, after]
                .iter()
                .any(|p| p.weight >= 4000 && utf8_char_count(cache.text(*p)) >= 2);
            let strong_jp = [Some(first), second, third]
                .into_iter()
                .flatten()
                .any(|p| japanese.reading(p.word_idx).len() >= 3 && p.weight >= 4000);
            if !strong_ch && !strong_jp {
                return;
            }
        }

        let jp_len = (first.end - first.start)
            + second.map_or(0, |s| s.end - s.start)
            + third.map_or(0, |t| t.end - t.start);

        let mut ambig_before = 0i32;
        for part in [Some(first), second, third].into_iter().flatten() {
            if part.start == 0
                || !japanese.single_han[part.word_idx]
                || !has_exact_match(lexicon, japanese.reading(part.word_idx))
            {
                continue;
            }
            let stronger_next = (0..matches_len).any(|idx| {
                let o = matches[idx];
                o.start == part.end && o.weight >= 4000 && japanese.reading(o.word_idx).len() >= 4
            });
            if stronger_next {
                let merged = cache.lookup(lexicon, input, simplified, 0, part.end, sc);
                if han_characters_stack(cache.text(merged)).map_or(0, |h| h.len()) >= 2 {
                    ambig_before += 1;
                }
            }
        }

        let mut ambig_after = 0i32;
        for part in [second, third].into_iter().flatten() {
            if !japanese.single_han[part.word_idx]
                || !has_exact_match(lexicon, japanese.reading(part.word_idx))
            {
                continue;
            }
            let mut triggered = false;
            for prev in [Some(first), second, third].into_iter().flatten() {
                if (prev.start == part.start && prev.end == part.end && prev.word_idx == part.word_idx)
                    || prev.end >= part.start
                    || prev.weight < 4000
                    || japanese.reading(prev.word_idx).len() < 4
                {
                    continue;
                }
                let merged = cache.lookup(lexicon, input, simplified, prev.end, part.end, sc);
                let merged_text = cache.text(merged);
                let Some(chars) = han_characters_stack(merged_text) else { continue };
                if chars.len() < 2 {
                    continue;
                }
                let mut trad_buf = [0u8; 64];
                let mut trad_len = 0usize;
                for idx in 0..chars.len() {
                    let ch = lexicon.traditional_char(chars.slice(merged_text, idx));
                    if trad_len + ch.len() <= trad_buf.len() {
                        trad_buf[trad_len..trad_len + ch.len()].copy_from_slice(ch.as_bytes());
                        trad_len += ch.len();
                    }
                }
                let trad = std::str::from_utf8(&trad_buf[..trad_len]).unwrap_or("");
                if let Some(tchars) = han_characters_stack(trad) {
                    let last = tchars.slice(trad, tchars.len() - 1);
                    let head = &trad[..trad.len() - last.len()];
                    if merged.weight >= 4000 || lexicon.join_bonus(head, last, false) > 0.0 {
                        triggered = true;
                        break;
                    }
                }
            }
            if triggered {
                ambig_after += 1;
            }
        }

        let score = before.weight
            + middle.weight
            + middle2.weight
            + after.weight
            + first.weight
            + second.map_or(0, |s| s.weight)
            + third.map_or(0, |t| t.weight)
            + (jp_len as i32) * 1000
            - 24000 * (ambig_before + ambig_after);

        if score > best_score {
            best_score = score;
            best_out.clear();
            best_out.push_str(cache.text(before));
            best_out.push_str(japanese.text(first.word_idx));
            best_out.push_str(cache.text(middle));
            if let Some(s) = second {
                best_out.push_str(japanese.text(s.word_idx));
            }
            best_out.push_str(cache.text(middle2));
            if let Some(t) = third {
                best_out.push_str(japanese.text(t.word_idx));
            }
            best_out.push_str(cache.text(after));
        }
    };

    let mut combinations = 0usize;
    for i in 0..matches_len {
        if combinations >= 256 { break; }
        combinations += 1;
        consider(matches[i], None, None, scratch, out);
        for j in (i + 1)..matches_len {
            if combinations >= 256 { break; }
            if matches[j].start < matches[i].end { continue; }
            combinations += 1;
            consider(matches[i], Some(matches[j]), None, scratch, out);
            for k in (j + 1)..matches_len {
                if combinations >= 256 { break; }
                if matches[k].start < matches[j].end { continue; }
                combinations += 1;
                consider(matches[i], Some(matches[j]), Some(matches[k]), scratch, out);
            }
        }
    }
}
