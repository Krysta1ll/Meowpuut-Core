#![forbid(unsafe_code)]
//! Lexicon compiler, TSV serializer/parser (`MEOWPUUT-PINYIN-1`, `MEOWPUUT-ZHUYIN-1`,
//! `MEOWPUUT-JAPANESE-1`), Japanese Romaji/Kana SoA dictionary, and overlay merger.
//!
//! # Examples
//!
//! ```
//! use meowpuut_lexicon::compiler::{
//!     compile_tsv, parse_japanese_tsv, merge_catalogs, JapaneseLexiconSoA,
//! };
//! use meowpuut_lexicon::soa_columns::{ContextColumnSoA, LexiconCatalogSoA};
//!
//! let mut cat = LexiconCatalogSoA::with_capacity(4, 128);
//! cat.push_entry("nihao", "ni3hao3", "你好", "你好", 5400, 1);
//! let mut ctx = ContextColumnSoA::with_capacity(2, 32);
//! ctx.push("你好", 600);
//! let tsv = compile_tsv(true, &mut cat, &ctx);
//! assert!(tsv.starts_with("MEOWPUUT-PINYIN-1\n"));
//! let core_lex = meowpuut_core::Lexicon::load(&tsv).unwrap();
//! assert!(core_lex.pinyin);
//!
//! let jp_tsv = "MEOWPUUT-JAPANESE-1\nwatashi\t私\t5500\nわたし\t私\t5500\n";
//! let jp = parse_japanese_tsv(jp_tsv).unwrap();
//! assert_eq!(jp.lookup("watashi")[0].0, "私");
//!
//! let mut overlay = LexiconCatalogSoA::with_capacity(2, 64);
//! overlay.push_entry("nihao", "ni3hao3", "你好", "你好", 5800, 2);
//! overlay.push_entry("zaoan", "zao3an1", "早安", "早安", 5100, 2);
//! let report = merge_catalogs(&mut cat, &overlay);
//! assert_eq!(report.added, 1);
//! assert_eq!(report.updated, 1);
//! ```

use std::cmp::Ordering;

use crate::phonetics::{hiragana_to_katakana, romaji_to_hiragana};
use crate::soa_columns::{ContextColumnSoA, LexiconCatalogSoA};

/// Summary statistics returned when merging an overlay catalog into a base catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MergeReport {
    pub added: usize,
    pub updated: usize,
    pub unchanged: usize,
}

/// Merges `overlay` entries into `base`, deduplicating and reporting counts.
pub fn merge_catalogs(base: &mut LexiconCatalogSoA, overlay: &LexiconCatalogSoA) -> MergeReport {
    let mut report = MergeReport::default();
    let base_initial_len = base.len();
    for o_idx in 0..overlay.len() {
        let ok = overlay.key(o_idx);
        let or = overlay.reading(o_idx);
        let ot = overlay.hant(o_idx);
        let os = overlay.hans(o_idx);
        let ow = overlay.weight[o_idx];
        let otier = overlay.source_tier[o_idx];

        let mut matched_idx = None;
        for b_idx in 0..base_initial_len {
            if base.key(b_idx) == ok
                && base.reading(b_idx) == or
                && base.hant(b_idx) == ot
                && base.hans(b_idx) == os
            {
                matched_idx = Some(b_idx);
                break;
            }
        }
        if let Some(b_idx) = matched_idx {
            if ow > base.weight[b_idx] {
                base.weight[b_idx] = ow;
                base.source_tier[b_idx] = otier;
                report.updated += 1;
            } else {
                report.unchanged += 1;
            }
        } else {
            base.push_entry(ok, or, ot, os, ow, otier);
            report.added += 1;
        }
    }
    base.sort_and_dedup();
    report
}

/// Serializes a `LexiconCatalogSoA` and `ContextColumnSoA` into `MEOWPUUT-PINYIN-1`
/// or `MEOWPUUT-ZHUYIN-1` TSV text ready for `meowpuut_core::Lexicon::load`.
pub fn compile_tsv(
    pinyin: bool,
    catalog: &mut LexiconCatalogSoA,
    context: &ContextColumnSoA,
) -> String {
    catalog.apply_polyphony_discount();
    catalog.sort_and_dedup();
    let header = if pinyin {
        "MEOWPUUT-PINYIN-1\n"
    } else {
        "MEOWPUUT-ZHUYIN-1\n"
    };
    let mut out = String::with_capacity(catalog.arena.len() + context.arena.len() + 256);
    out.push_str(header);
    for i in 0..catalog.len() {
        out.push_str(catalog.key(i));
        out.push('\t');
        out.push_str(catalog.reading(i));
        out.push('\t');
        out.push_str(catalog.hant(i));
        out.push('\t');
        out.push_str(catalog.hans(i));
        out.push('\t');
        out.push_str(&catalog.weight[i].to_string());
        out.push('\n');
    }
    if !context.is_empty() {
        out.push_str("# context\n");
        let mut order: Vec<usize> = (0..context.len()).collect();
        order.sort_by(|&a, &b| context.text(a).cmp(context.text(b)));
        for &idx in &order {
            out.push_str(context.text(idx));
            out.push('\t');
            out.push_str(&context.bonus[idx].to_string());
            out.push('\n');
        }
    }
    out
}

/// Compiles a `LexiconCatalogSoA` and `ContextColumnSoA` directly into a
/// `MIMO-BIN-1` binary image ready for `meowpuut_core::Lexicon::load_binary`.
pub fn compile_binary(
    pinyin: bool,
    catalog: &mut LexiconCatalogSoA,
    context: &ContextColumnSoA,
) -> Option<Vec<u8>> {
    let tsv = compile_tsv(pinyin, catalog, context);
    let lex = meowpuut_core::Lexicon::load(&tsv)?;
    Some(lex.to_binary())
}

/// Structure-of-Arrays Japanese dictionary (`MEOWPUUT-JAPANESE-1`) supporting
/// exact and prefix lookup by Romaji or Hiragana reading.
#[derive(Debug, Clone, Default)]
pub struct JapaneseLexiconSoA {
    pub arena: Vec<u8>,
    pub key_offset: Vec<u32>,
    pub key_len: Vec<u16>,
    pub surface_offset: Vec<u32>,
    pub surface_len: Vec<u16>,
    pub weight: Vec<i32>,
}

impl JapaneseLexiconSoA {
    pub fn with_capacity(entries: usize, arena_bytes: usize) -> Self {
        Self {
            arena: Vec::with_capacity(arena_bytes),
            key_offset: Vec::with_capacity(entries),
            key_len: Vec::with_capacity(entries),
            surface_offset: Vec::with_capacity(entries),
            surface_len: Vec::with_capacity(entries),
            weight: Vec::with_capacity(entries),
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.weight.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.weight.is_empty()
    }

    pub fn push(&mut self, key: &str, surface: &str, weight: i32) {
        let k_off = self.arena.len() as u32;
        self.arena.extend_from_slice(key.as_bytes());
        let s_off = self.arena.len() as u32;
        self.arena.extend_from_slice(surface.as_bytes());
        self.key_offset.push(k_off);
        self.key_len.push(key.len() as u16);
        self.surface_offset.push(s_off);
        self.surface_len.push(surface.len() as u16);
        self.weight.push(weight);
    }

    #[inline]
    pub fn key(&self, idx: usize) -> &str {
        let s = self.key_offset[idx] as usize;
        let e = s + self.key_len[idx] as usize;
        std::str::from_utf8(&self.arena[s..e]).unwrap_or("")
    }

    #[inline]
    pub fn surface(&self, idx: usize) -> &str {
        let s = self.surface_offset[idx] as usize;
        let e = s + self.surface_len[idx] as usize;
        std::str::from_utf8(&self.arena[s..e]).unwrap_or("")
    }

    pub fn sort(&mut self) {
        let n = self.len();
        if n <= 1 {
            return;
        }
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by(|&a, &b| {
            self.key(a)
                .cmp(self.key(b))
                .then_with(|| self.weight[b].cmp(&self.weight[a]))
                .then_with(|| self.surface(a).cmp(self.surface(b)))
        });
        let mut sorted = Self::with_capacity(n, self.arena.len());
        for &idx in &order {
            sorted.push(self.key(idx), self.surface(idx), self.weight[idx]);
        }
        *self = sorted;
    }

    fn lower_bound(&self, key: &str) -> usize {
        let mut lo = 0usize;
        let mut hi = self.len();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.key(mid) < key {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo
    }

    /// Looks up Japanese candidates for `romaji_or_kana`, combining exact matches,
    /// Hiragana/Katakana conversions, and bounded prefix completions.
    pub fn lookup(&self, romaji_or_kana: &str) -> Vec<(String, i32, usize)> {
        if romaji_or_kana.is_empty() {
            return Vec::new();
        }
        let hira = romaji_to_hiragana(romaji_or_kana);
        let kata = hiragana_to_katakana(&hira);
        let mut out: Vec<(String, i32, usize)> = Vec::new();

        for query in [romaji_or_kana, hira.as_str()] {
            let start = self.lower_bound(query);
            let mut idx = start;
            while idx < self.len() && self.key(idx) == query {
                let surf = self.surface(idx);
                if !out.iter().any(|(s, _, _)| s == surf) {
                    out.push((String::from(surf), self.weight[idx], romaji_or_kana.len()));
                }
                idx += 1;
            }
        }

        for len in (2..romaji_or_kana.len()).rev() {
            let sub = &romaji_or_kana[..len];
            let sub_hira = romaji_to_hiragana(sub);
            for query in [sub, sub_hira.as_str()] {
                let start = self.lower_bound(query);
                let mut idx = start;
                while idx < self.len() && self.key(idx) == query {
                    let surf = self.surface(idx);
                    if !out.iter().any(|(s, _, _)| s == surf) {
                        out.push((String::from(surf), self.weight[idx] - 300, len));
                    }
                    idx += 1;
                }
            }
            if out.len() >= 12 {
                break;
            }
        }

        out.sort_by(|a, b| match b.1.cmp(&a.1) {
            Ordering::Equal => b.2.cmp(&a.2),
            other => other,
        });

        if !hira.is_empty() && !out.iter().any(|(s, _, _)| s == &hira) {
            out.push((hira, 2400, romaji_or_kana.len()));
        }
        if !kata.is_empty() && !out.iter().any(|(s, _, _)| s == &kata) {
            out.push((kata, 2200, romaji_or_kana.len()));
        }
        out
    }
}

/// Parses a `MEOWPUUT-JAPANESE-1` TSV string into a sorted `JapaneseLexiconSoA`.
pub fn parse_japanese_tsv(text: &str) -> Option<JapaneseLexiconSoA> {
    let mut lines = text.split('\n');
    let header = lines.next()?.trim_end_matches('\r');
    if header != "MEOWPUUT-JAPANESE-1" {
        return None;
    }
    let mut jp = JapaneseLexiconSoA::with_capacity(1024, text.len());
    for raw in lines {
        let line = raw.trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split('\t');
        let key = parts.next()?;
        let surface = parts.next()?;
        let weight: i32 = parts.next()?.parse().ok()?;
        jp.push(key, surface, weight);
    }
    jp.sort();
    Some(jp)
}
