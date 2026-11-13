#![forbid(unsafe_code)]
//! Unified Chinese + Japanese candidate fusion backend (`ChineseWithJapaneseBackend`)
//! using Structure-of-Arrays (`ChoicePoolSoA`), evidence tiers, and Kana bridging.
//!
//! # Examples
//!
//! ```
//! use std::sync::Arc;
//! use meowpuut_core::Lexicon;
//! use meowpuut_lexicon::parse_japanese_tsv;
//! use meowpuut_client::backend::{ChineseWithJapaneseBackend, compact_japanese};
//!
//! let py_tsv = concat!(
//!     "MEOWPUUT-PINYIN-1\n",
//!     "hao\thao3\t好\t好\t4800\n",
//!     "ni\tni3\t你\t你\t5000\n",
//!     "nihao\tni3hao3\t你好\t你好\t5400\n",
//! );
//! let jp_tsv = "MEOWPUUT-JAPANESE-1\nwatashi\t私\t5500\nneko\t猫\t5200\n";
//! let py_lex = Arc::new(Lexicon::load(py_tsv).unwrap());
//! let jp_lex = parse_japanese_tsv(jp_tsv).unwrap();
//! let mut backend = ChineseWithJapaneseBackend::new(py_lex, jp_lex);
//! backend.type_keys("nihao");
//! assert_eq!(backend.candidates()[0], "你好");
//!
//! let compacted = compact_japanese(&[("猫".into(), 5200, 4), ("ねこ".into(), 2400, 4)]);
//! assert_eq!(compacted.len(), 2);
//! ```

use std::cmp::Ordering;
use std::sync::Arc;

use meowpuut_core::{CandidateKind, Lexicon, Session};
use meowpuut_lexicon::{
    pinyin_kana, AssociationTableSoA, JapaneseLexiconSoA, WordEvidenceSoA,
};

use crate::memory::ChoiceMemorySoA;

/// Compacts raw Japanese `(surface, weight, consumed)` candidates by deduplicating
/// surfaces and capping low-weight Kana fallbacks.
pub fn compact_japanese(raw: &[(String, i32, usize)]) -> Vec<(String, i32, usize)> {
    let mut out: Vec<(String, i32, usize)> = Vec::with_capacity(raw.len().min(12));
    for (surf, weight, consumed) in raw {
        if surf.is_empty() {
            continue;
        }
        if let Some(existing) = out.iter_mut().find(|(s, _, _)| s == surf) {
            if *weight > existing.1 {
                existing.1 = *weight;
                existing.2 = *consumed;
            }
            continue;
        }
        out.push((surf.clone(), *weight, *consumed));
        if out.len() >= 12 {
            break;
        }
    }
    out
}

/// Structure-of-Arrays pool of fused Chinese + Japanese choices.
#[derive(Debug, Clone, Default)]
pub struct ChoicePoolSoA {
    pub arena: Vec<u8>,
    pub text_offset: Vec<u32>,
    pub text_len: Vec<u16>,
    pub tier: Vec<u8>,
    pub score: Vec<f64>,
    pub consumed: Vec<usize>,
    pub is_japanese: Vec<bool>,
}

impl ChoicePoolSoA {
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            arena: Vec::with_capacity(cap * 16),
            text_offset: Vec::with_capacity(cap),
            text_len: Vec::with_capacity(cap),
            tier: Vec::with_capacity(cap),
            score: Vec::with_capacity(cap),
            consumed: Vec::with_capacity(cap),
            is_japanese: Vec::with_capacity(cap),
        }
    }

    #[inline]
    pub fn clear(&mut self) {
        self.arena.clear();
        self.text_offset.clear();
        self.text_len.clear();
        self.tier.clear();
        self.score.clear();
        self.consumed.clear();
        self.is_japanese.clear();
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.score.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.score.is_empty()
    }

    #[inline]
    pub fn text(&self, idx: usize) -> &str {
        let s = self.text_offset[idx] as usize;
        let e = s + self.text_len[idx] as usize;
        std::str::from_utf8(&self.arena[s..e]).unwrap_or("")
    }

    pub fn push(&mut self, text: &str, tier: u8, score: f64, consumed: usize, is_japanese: bool) {
        for i in 0..self.len() {
            if self.text(i) == text {
                if tier > self.tier[i] || (tier == self.tier[i] && score > self.score[i]) {
                    self.tier[i] = tier;
                    self.score[i] = score;
                    self.consumed[i] = consumed;
                    self.is_japanese[i] = is_japanese;
                }
                return;
            }
        }
        let off = self.arena.len() as u32;
        self.arena.extend_from_slice(text.as_bytes());
        self.text_offset.push(off);
        self.text_len.push(text.len() as u16);
        self.tier.push(tier);
        self.score.push(score);
        self.consumed.push(consumed);
        self.is_japanese.push(is_japanese);
    }
}

/// Fused Chinese (Pinyin/Zhuyin) and Japanese (Romaji/Kana) input backend.
#[derive(Debug, Clone)]
pub struct ChineseWithJapaneseBackend {
    pub session: Session,
    pub japanese: JapaneseLexiconSoA,
    pub evidence: WordEvidenceSoA,
    pub memory: ChoiceMemorySoA,
    pub associations: AssociationTableSoA,
    pub pool: ChoicePoolSoA,
    pub ranked_texts: Vec<String>,
    pub ranked_consumed: Vec<usize>,
    pub simplified: bool,
}

impl ChineseWithJapaneseBackend {
    /// Creates a new fused backend from a core `Lexicon` and `JapaneseLexiconSoA`.
    pub fn new(lexicon: Arc<Lexicon>, japanese: JapaneseLexiconSoA) -> Self {
        let mut session = Session::new(Arc::clone(&lexicon));
        for i in 0..japanese.len() {
            let key = japanese.key(i);
            let surf = japanese.surface(i);
            let w = japanese.weight[i];
            if key.bytes().all(|b| (b'a'..=b'z').contains(&b)) {
                let _ = session.add_japanese_word(key, surf, w);
            }
        }
        let mut pairs = Vec::with_capacity(lexicon.words.len());
        for i in 0..lexicon.words.len() {
            pairs.push((
                lexicon.words.text(&lexicon.storage, i, false),
                lexicon.words.weight(i),
            ));
        }
        let evidence = WordEvidenceSoA::from_pairs(&pairs);

        Self {
            session,
            japanese,
            evidence,
            memory: ChoiceMemorySoA::new(),
            associations: AssociationTableSoA::default_builtin(),
            pool: ChoicePoolSoA::with_capacity(96),
            ranked_texts: Vec::with_capacity(96),
            ranked_consumed: Vec::with_capacity(96),
            simplified: false,
        }
    }

    pub fn set_simplified(&mut self, simplified: bool) {
        self.simplified = simplified;
        self.session.set_simplified(simplified);
        self.rebuild_candidates();
    }

    pub fn clear(&mut self) {
        self.session.clear();
        self.pool.clear();
        self.ranked_texts.clear();
        self.ranked_consumed.clear();
    }

    pub fn type_key(&mut self, ch: u8) -> bool {
        let handled = self.session.key(i32::from(ch));
        self.rebuild_candidates();
        handled
    }

    pub fn type_keys(&mut self, keys: &str) {
        for b in keys.bytes() {
            let _ = self.session.key(i32::from(b));
        }
        self.rebuild_candidates();
    }

    pub fn backspace(&mut self) -> bool {
        let handled = self.session.key(0xff08);
        self.rebuild_candidates();
        handled
    }

    #[inline]
    pub fn input(&self) -> &str {
        self.session.input()
    }

    #[inline]
    pub fn preedit(&self) -> &str {
        self.session.preedit()
    }

    #[inline]
    pub fn candidates(&self) -> &[String] {
        &self.ranked_texts
    }

    /// Selects the candidate at `index`, updating personal memory and returning committed text.
    pub fn select(&mut self, index: usize) -> Option<String> {
        if index >= self.ranked_texts.len() {
            return None;
        }
        let chosen = self.ranked_texts[index].clone();
        let consumed = self.ranked_consumed[index];
        let full_input = String::from(self.session.input());
        let reading_slice = &full_input[..consumed.min(full_input.len())];
        self.memory.learn(reading_slice, &chosen);

        for k in 0..self.session.candidate_count() {
            if self.session.candidate(k) == chosen && self.session.candidate_consumed(k) == consumed {
                let _ = self.session.choose(k);
                self.rebuild_candidates();
                return Some(chosen);
            }
        }

        let remaining = String::from(&full_input[consumed.min(full_input.len())..]);
        self.session.reset();
        for b in remaining.bytes() {
            let _ = self.session.key(i32::from(b));
        }
        self.rebuild_candidates();
        Some(chosen)
    }

    /// Returns post-commit next-word association suggestions for `committed`.
    pub fn associations_for(&self, committed: &str) -> Vec<String> {
        self.associations
            .lookup(committed, self.simplified)
            .into_iter()
            .map(String::from)
            .collect()
    }

    fn rebuild_candidates(&mut self) {
        self.pool.clear();
        self.ranked_texts.clear();
        self.ranked_consumed.clear();
        let raw_input = String::from(self.session.input());
        if raw_input.is_empty() {
            return;
        }

        let mixed = String::from(self.session.mixed_pinyin(&raw_input, self.simplified));
        if !mixed.is_empty() {
            self.pool.push(&mixed, 95, 9500.0, raw_input.len(), true);
        }
        let (mixed_pref_str, mixed_pref_len) =
            self.session.mixed_pinyin_prefix(&raw_input, self.simplified);
        if !mixed_pref_str.is_empty() && mixed_pref_len > 0 {
            let pref_owned = String::from(mixed_pref_str);
            self.pool.push(&pref_owned, 82, 7800.0, mixed_pref_len, true);
        }

        let count = self.session.candidate_count();
        for i in 0..count {
            let text = self.session.candidate(i);
            let info = self.session.candidate_info(i);
            let consumed = self.session.candidate_consumed(i);
            let kind = info.map_or(CandidateKind::Unknown as u32, |m| m.kind);
            let base_score = info.map_or(0.0, |m| m.score);

            let mut tier: u8 = match kind {
                x if x == CandidateKind::Exact as u32 => 90,
                x if x == CandidateKind::Path as u32 => 80,
                x if x == CandidateKind::Partial as u32 => {
                    if i < 3 { 85 } else { 70 }
                }
                x if x == CandidateKind::Correction as u32 => 60,
                _ => 50,
            };
            let reading_part = &raw_input[..consumed.min(raw_input.len())];
            if self.memory.is_pinned(reading_part, text) {
                tier = 100;
            }
            let boost = f64::from(self.memory.boost_for(text) + self.memory.bigram_bonus_for(text));
            let rank_bias = -((i as f64) * 12.0);
            self.pool
                .push(text, tier, base_score + boost + rank_bias, consumed, false);
        }

        if raw_input.bytes().all(|b| (b'a'..=b'z').contains(&b) || b == b'-') {
            let jp_raw = self.japanese.lookup(&raw_input);
            let jp_compact = compact_japanese(&jp_raw);
            for (rank, (surf, weight, consumed)) in jp_compact.into_iter().enumerate() {
                let is_full = consumed == raw_input.len();
                let mut tier = if is_full && weight >= 4500 {
                    88
                } else if is_full {
                    72
                } else {
                    64
                };
                if self.memory.is_pinned(&raw_input[..consumed], &surf) {
                    tier = 100;
                }
                let boost = f64::from(self.memory.boost_for(&surf));
                self.pool.push(
                    &surf,
                    tier,
                    f64::from(weight) + boost - (rank as f64) * 15.0,
                    consumed,
                    true,
                );
            }
            if let Some(kana) = pinyin_kana(&raw_input) {
                self.pool.push(kana, 55, 2100.0, raw_input.len(), true);
            }
        }

        let mut order: Vec<usize> = (0..self.pool.len()).collect();
        order.sort_by(|&a, &b| match self.pool.tier[b].cmp(&self.pool.tier[a]) {
            Ordering::Equal => {
                if self.pool.score[b] > self.pool.score[a] {
                    Ordering::Greater
                } else if self.pool.score[b] < self.pool.score[a] {
                    Ordering::Less
                } else {
                    a.cmp(&b)
                }
            }
            other => other,
        });

        for idx in order {
            self.ranked_texts.push(String::from(self.pool.text(idx)));
            self.ranked_consumed.push(self.pool.consumed[idx]);
        }
    }
}
