#![forbid(unsafe_code)]
//! Columnar Structure-of-Arrays (`LexiconCatalogSoA` and `ContextColumnSoA`)
//! for `mimo-tc`, `mimo-sc`, and `mimo-jp` lexicon compilation, frequency calibration,
//! and tiered polyphony/heterophony discounting.
//!
//! # Examples
//!
//! ```
//! use meowpuut_lexicon::soa_columns::{LexiconCatalogSoA, calibrate_weight};
//!
//! let mut catalog = LexiconCatalogSoA::with_capacity(8, 256);
//! catalog.push_entry("he", "han4", "和", "和", 5000, 1);
//! catalog.push_entry("he", "he2", "和", "和", 5000, 1);
//! catalog.push_entry("he", "huo4", "和", "和", 5000, 1);
//! catalog.apply_tiered_heterophony(&[("和", "han4")], &[("和", "he2")], &[]);
//! assert_eq!(catalog.weight[0], 5000);
//! assert_eq!(catalog.weight[1], 3600);
//! assert_eq!(catalog.weight[2], 625);
//! catalog.apply_polyphony_discount();
//! assert!(calibrate_weight(1000, 5000, 2) >= 2000);
//! ```

use std::cmp::Ordering;

/// Calibrates a raw corpus frequency against `source_scale` and character length
/// into the `1200..=6200` engine weight range.
pub fn calibrate_weight(raw_score: i32, source_scale: i32, char_count: usize) -> i32 {
    let clamped_raw = raw_score.max(1) as f64;
    let clamped_scale = source_scale.max(10) as f64;
    let ratio = (clamped_raw.ln() / clamped_scale.ln()).clamp(0.15, 1.25);
    let length_bonus = match char_count {
        0 | 1 => 0.0,
        2 => 260.0,
        3 => 340.0,
        4 => 420.0,
        _ => 300.0,
    };
    let calibrated = 1600.0 + ratio * 3800.0 + length_bonus;
    (calibrated.round() as i32).clamp(1200, 6200)
}

/// Columnar Structure-of-Arrays lexicon catalog used during compilation and merging.
#[derive(Debug, Clone, Default)]
pub struct LexiconCatalogSoA {
    pub arena: Vec<u8>,
    pub key_offset: Vec<u32>,
    pub key_len: Vec<u16>,
    pub reading_offset: Vec<u32>,
    pub reading_len: Vec<u16>,
    pub hant_offset: Vec<u32>,
    pub hant_len: Vec<u16>,
    pub hans_offset: Vec<u32>,
    pub hans_len: Vec<u16>,
    pub weight: Vec<i32>,
    pub source_tier: Vec<u8>,
}

impl LexiconCatalogSoA {
    /// Creates a preallocated columnar lexicon catalog.
    pub fn with_capacity(entries: usize, arena_bytes: usize) -> Self {
        Self {
            arena: Vec::with_capacity(arena_bytes),
            key_offset: Vec::with_capacity(entries),
            key_len: Vec::with_capacity(entries),
            reading_offset: Vec::with_capacity(entries),
            reading_len: Vec::with_capacity(entries),
            hant_offset: Vec::with_capacity(entries),
            hant_len: Vec::with_capacity(entries),
            hans_offset: Vec::with_capacity(entries),
            hans_len: Vec::with_capacity(entries),
            weight: Vec::with_capacity(entries),
            source_tier: Vec::with_capacity(entries),
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

    fn intern(&mut self, s: &str) -> (u32, u16) {
        let off = self.arena.len() as u32;
        let len = s.len() as u16;
        self.arena.extend_from_slice(s.as_bytes());
        (off, len)
    }

    /// Appends a single entry to the SoA columns.
    pub fn push_entry(
        &mut self,
        key: &str,
        reading: &str,
        hant: &str,
        hans: &str,
        weight: i32,
        source_tier: u8,
    ) {
        let (k_off, k_len) = self.intern(key);
        let (r_off, r_len) = self.intern(reading);
        let (t_off, t_len) = self.intern(hant);
        let (s_off, s_len) = self.intern(hans);
        self.key_offset.push(k_off);
        self.key_len.push(k_len);
        self.reading_offset.push(r_off);
        self.reading_len.push(r_len);
        self.hant_offset.push(t_off);
        self.hant_len.push(t_len);
        self.hans_offset.push(s_off);
        self.hans_len.push(s_len);
        self.weight.push(weight);
        self.source_tier.push(source_tier);
    }

    #[inline]
    pub fn key(&self, idx: usize) -> &str {
        let s = self.key_offset[idx] as usize;
        let e = s + self.key_len[idx] as usize;
        std::str::from_utf8(&self.arena[s..e]).unwrap_or("")
    }

    #[inline]
    pub fn reading(&self, idx: usize) -> &str {
        let s = self.reading_offset[idx] as usize;
        let e = s + self.reading_len[idx] as usize;
        std::str::from_utf8(&self.arena[s..e]).unwrap_or("")
    }

    #[inline]
    pub fn hant(&self, idx: usize) -> &str {
        let s = self.hant_offset[idx] as usize;
        let e = s + self.hant_len[idx] as usize;
        std::str::from_utf8(&self.arena[s..e]).unwrap_or("")
    }

    #[inline]
    pub fn hans(&self, idx: usize) -> &str {
        let s = self.hans_offset[idx] as usize;
        let e = s + self.hans_len[idx] as usize;
        std::str::from_utf8(&self.arena[s..e]).unwrap_or("")
    }

    /// Applies `mimo-tc` 3-tier heterophony discounting to single-character entries:
    /// `tier1` retains 100%, `tier2` retains 72%, `tier3` retains 45%, and unlisted
    /// readings of characters present in `tier1` are discounted by `// 8`.
    pub fn apply_tiered_heterophony(
        &mut self,
        tier1: &[(&str, &str)],
        tier2: &[(&str, &str)],
        tier3: &[(&str, &str)],
    ) {
        for idx in 0..self.len() {
            let word = self.hant(idx);
            if word.chars().count() != 1 {
                continue;
            }
            let has_tier1 = tier1.iter().any(|&(w, _)| w == word);
            if !has_tier1 {
                continue;
            }
            let code = self.reading(idx);
            if tier1.iter().any(|&(w, r)| w == word && r == code) {
                continue;
            }
            let w = self.weight[idx];
            if tier2.iter().any(|&(ch, r)| ch == word && r == code) {
                self.weight[idx] = ((w as f64 * 0.72).round() as i32).max(1);
            } else if tier3.iter().any(|&(ch, r)| ch == word && r == code) {
                self.weight[idx] = ((w as f64 * 0.45).round() as i32).max(1);
            } else {
                self.weight[idx] = (w / 8).max(1);
            }
        }
    }

    /// Discounts secondary readings when multiple single-character entries share the
    /// same sound key or character surface, preventing rare readings from dominating.
    pub fn apply_polyphony_discount(&mut self) {
        let n = self.len();
        if n <= 1 {
            return;
        }
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by(|&a, &b| {
            let ka = self.key(a);
            let kb = self.key(b);
            match ka.cmp(kb) {
                Ordering::Equal => self.weight[b].cmp(&self.weight[a]),
                other => other,
            }
        });

        let mut i = 0usize;
        while i < n {
            let first_idx = order[i];
            let k = self.key(first_idx).to_owned();
            let top_weight = self.weight[first_idx];
            let mut rank = 0usize;
            while i < n && self.key(order[i]) == k {
                let idx = order[i];
                if rank >= 1 && self.hant(idx).chars().count() == 1 {
                    let discount = (rank.min(4) as i32) * 120;
                    if self.weight[idx] + 200 >= top_weight {
                        self.weight[idx] = (self.weight[idx] - discount).max(1000);
                    }
                }
                rank += 1;
                i += 1;
            }
        }
    }

    /// Sorts all entries by `(key ASC, weight DESC, reading ASC, hant ASC)` and deduplicates
    /// identical `(key, reading, hant, hans)` rows, keeping the highest weight.
    pub fn sort_and_dedup(&mut self) {
        let n = self.len();
        if n <= 1 {
            return;
        }
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by(|&a, &b| {
            self.key(a)
                .cmp(self.key(b))
                .then_with(|| self.weight[b].cmp(&self.weight[a]))
                .then_with(|| self.reading(a).cmp(self.reading(b)))
                .then_with(|| self.hant(a).cmp(self.hant(b)))
        });

        let mut compacted = Self::with_capacity(n, self.arena.len());
        for &idx in &order {
            let k = self.key(idx);
            let r = self.reading(idx);
            let t = self.hant(idx);
            let s = self.hans(idx);
            let w = self.weight[idx];
            let tier = self.source_tier[idx];
            let mut dup = false;
            for prev in (0..compacted.len()).rev() {
                if compacted.key(prev) != k {
                    break;
                }
                if compacted.reading(prev) == r
                    && compacted.hant(prev) == t
                    && compacted.hans(prev) == s
                {
                    if w > compacted.weight[prev] {
                        compacted.weight[prev] = w;
                    }
                    dup = true;
                    break;
                }
            }
            if !dup {
                compacted.push_entry(k, r, t, s, w, tier);
            }
        }
        *self = compacted;
    }
}

/// Columnar Structure-of-Arrays table for `# context` n-gram transition bonuses.
#[derive(Debug, Clone, Default)]
pub struct ContextColumnSoA {
    pub arena: Vec<u8>,
    pub text_offset: Vec<u32>,
    pub text_len: Vec<u16>,
    pub bonus: Vec<i32>,
}

impl ContextColumnSoA {
    pub fn with_capacity(entries: usize, arena_bytes: usize) -> Self {
        Self {
            arena: Vec::with_capacity(arena_bytes),
            text_offset: Vec::with_capacity(entries),
            text_len: Vec::with_capacity(entries),
            bonus: Vec::with_capacity(entries),
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.bonus.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.bonus.is_empty()
    }

    pub fn push(&mut self, text: &str, bonus: i32) {
        let off = self.arena.len() as u32;
        let len = text.len() as u16;
        self.arena.extend_from_slice(text.as_bytes());
        self.text_offset.push(off);
        self.text_len.push(len);
        self.bonus.push(bonus);
    }

    #[inline]
    pub fn text(&self, idx: usize) -> &str {
        let s = self.text_offset[idx] as usize;
        let e = s + self.text_len[idx] as usize;
        std::str::from_utf8(&self.arena[s..e]).unwrap_or("")
    }
}
