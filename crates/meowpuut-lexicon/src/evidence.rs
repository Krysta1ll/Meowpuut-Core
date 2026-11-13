#![forbid(unsafe_code)]
//! Structure-of-Arrays word evidence (`WordEvidenceSoA`), English lexicon/typo
//! completions (`EnglishEvidenceSoA`), and phrase associations (`AssociationTableSoA`).
//!
//! # Examples
//!
//! ```
//! use meowpuut_lexicon::evidence::{WordEvidenceSoA, EnglishEvidenceSoA, AssociationTableSoA};
//!
//! let mut ev = WordEvidenceSoA::from_pairs(&[("你好", 5400), ("世界", 5200), ("喜歡", 5500)]);
//! assert_eq!(ev.weight("你好"), Some(5400));
//! assert!(ev.contains("喜歡"));
//! assert!(!ev.contains("不存在"));
//!
//! let eng = EnglishEvidenceSoA::default_builtin();
//! let cands = eng.candidates("teh", 4);
//! assert!(cands.iter().any(|c| c == "the"));
//!
//! let assoc = AssociationTableSoA::default_builtin();
//! assert!(!assoc.lookup("你好", false).is_empty());
//! ```

/// Binary-searched Structure-of-Arrays table mapping surface words to lexicon weights.
#[derive(Debug, Clone, Default)]
pub struct WordEvidenceSoA {
    pub arena: Vec<u8>,
    pub text_offset: Vec<u32>,
    pub text_len: Vec<u16>,
    pub weight: Vec<i32>,
}

impl WordEvidenceSoA {
    /// Builds a sorted `WordEvidenceSoA` from `(surface, weight)` pairs, keeping the maximum weight per word.
    pub fn from_pairs(pairs: &[(&str, i32)]) -> Self {
        let mut sorted: Vec<(&str, i32)> = pairs.to_vec();
        sorted.sort_by(|a, b| a.0.cmp(b.0).then_with(|| b.1.cmp(&a.1)));
        let mut out = Self {
            arena: Vec::with_capacity(sorted.len() * 9),
            text_offset: Vec::with_capacity(sorted.len()),
            text_len: Vec::with_capacity(sorted.len()),
            weight: Vec::with_capacity(sorted.len()),
        };
        for (text, w) in sorted {
            if text.is_empty() {
                continue;
            }
            if !out.is_empty() && out.text(out.len() - 1) == text {
                let last = out.len() - 1;
                if w > out.weight[last] {
                    out.weight[last] = w;
                }
                continue;
            }
            let off = out.arena.len() as u32;
            out.arena.extend_from_slice(text.as_bytes());
            out.text_offset.push(off);
            out.text_len.push(text.len() as u16);
            out.weight.push(w);
        }
        out
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.weight.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.weight.is_empty()
    }

    #[inline]
    pub fn text(&self, idx: usize) -> &str {
        let s = self.text_offset[idx] as usize;
        let e = s + self.text_len[idx] as usize;
        std::str::from_utf8(&self.arena[s..e]).unwrap_or("")
    }

    /// Looks up the highest recorded lexicon weight for `word` via binary search.
    pub fn weight(&self, word: &str) -> Option<i32> {
        let mut lo = 0usize;
        let mut hi = self.len();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            match self.text(mid).cmp(word) {
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
                std::cmp::Ordering::Equal => return Some(self.weight[mid]),
            }
        }
        None
    }

    #[inline]
    pub fn contains(&self, word: &str) -> bool {
        self.weight(word).is_some()
    }
}

/// Structure-of-Arrays English word & typo correction index.
#[derive(Debug, Clone, Default)]
pub struct EnglishEvidenceSoA {
    pub arena: Vec<u8>,
    pub key_offset: Vec<u32>,
    pub key_len: Vec<u16>,
    pub display_offset: Vec<u32>,
    pub display_len: Vec<u16>,
    pub rank: Vec<u32>,
}

const BUILTIN_ENGLISH: [(&str, &str); 64] = [
    ("the", "the"), ("and", "and"), ("that", "that"), ("have", "have"),
    ("for", "for"), ("not", "not"), ("with", "with"), ("you", "you"),
    ("this", "this"), ("but", "but"), ("his", "his"), ("from", "from"),
    ("they", "they"), ("say", "say"), ("her", "her"), ("she", "she"),
    ("will", "will"), ("one", "one"), ("all", "all"), ("would", "would"),
    ("there", "there"), ("their", "their"), ("what", "what"), ("about", "about"),
    ("which", "which"), ("when", "when"), ("make", "make"), ("can", "can"),
    ("like", "like"), ("time", "time"), ("just", "just"), ("know", "know"),
    ("take", "take"), ("people", "people"), ("into", "into"), ("year", "year"),
    ("your", "your"), ("good", "good"), ("some", "some"), ("could", "could"),
    ("them", "them"), ("see", "see"), ("other", "other"), ("than", "than"),
    ("then", "then"), ("now", "now"), ("look", "look"), ("only", "only"),
    ("come", "come"), ("its", "its"), ("over", "over"), ("think", "think"),
    ("also", "also"), ("back", "back"), ("after", "after"), ("use", "use"),
    ("hello", "hello"), ("world", "world"), ("keyboard", "keyboard"), ("input", "input"),
    ("im", "I'm"), ("dont", "don't"), ("cant", "can't"), ("wont", "won't"),
];

const BUILTIN_ENGLISH_TYPOS: [(&str, &str); 12] = [
    ("teh", "the"), ("adn", "and"), ("taht", "that"), ("recieve", "receive"),
    ("seperate", "separate"), ("occured", "occurred"), ("definately", "definitely"),
    ("wierd", "weird"), ("untill", "until"), ("alot", "a lot"),
    ("thier", "their"), ("becuase", "because"),
];

impl EnglishEvidenceSoA {
    /// Constructs the built-in English frequency and typo-correction SoA index.
    pub fn default_builtin() -> Self {
        let mut out = Self::default();
        for (rank, &(key, display)) in BUILTIN_ENGLISH.iter().enumerate() {
            out.push(key, display, rank as u32 + 1);
        }
        out
    }

    pub fn push(&mut self, key: &str, display: &str, rank: u32) {
        let k_off = self.arena.len() as u32;
        self.arena.extend_from_slice(key.as_bytes());
        let d_off = self.arena.len() as u32;
        self.arena.extend_from_slice(display.as_bytes());
        self.key_offset.push(k_off);
        self.key_len.push(key.len() as u16);
        self.display_offset.push(d_off);
        self.display_len.push(display.len() as u16);
        self.rank.push(rank);
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.rank.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.rank.is_empty()
    }

    #[inline]
    pub fn key(&self, idx: usize) -> &str {
        let s = self.key_offset[idx] as usize;
        let e = s + self.key_len[idx] as usize;
        std::str::from_utf8(&self.arena[s..e]).unwrap_or("")
    }

    #[inline]
    pub fn display(&self, idx: usize) -> &str {
        let s = self.display_offset[idx] as usize;
        let e = s + self.display_len[idx] as usize;
        std::str::from_utf8(&self.arena[s..e]).unwrap_or("")
    }

    fn apply_casing(raw_input: &str, canonical: &str) -> String {
        if raw_input.chars().all(|c| c.is_ascii_uppercase()) && raw_input.len() > 1 {
            return canonical.to_ascii_uppercase();
        }
        if raw_input.chars().next().map_or(false, |c| c.is_ascii_uppercase()) {
            let mut chars = canonical.chars();
            if let Some(first) = chars.next() {
                let mut s = String::with_capacity(canonical.len());
                s.push(first.to_ascii_uppercase());
                s.push_str(chars.as_str());
                return s;
            }
        }
        String::from(canonical)
    }

    /// Generates ranked English candidates (exact, contraction/typo, prefix completions) for `raw`.
    pub fn candidates(&self, raw: &str, limit: usize) -> Vec<String> {
        if raw.is_empty() || limit == 0 {
            return Vec::new();
        }
        let lower = raw.to_ascii_lowercase();
        let mut out = Vec::with_capacity(limit);
        out.push(String::from(raw));

        for &(typo, fixed) in &BUILTIN_ENGLISH_TYPOS {
            if lower == typo {
                let cased = Self::apply_casing(raw, fixed);
                if !out.contains(&cased) {
                    out.push(cased);
                }
            }
        }

        let mut matches: Vec<usize> = (0..self.len())
            .filter(|&i| self.key(i).starts_with(&lower))
            .collect();
        matches.sort_by_key(|&i| (self.key(i) != lower, self.rank[i]));

        for idx in matches {
            if out.len() >= limit {
                break;
            }
            let cased = Self::apply_casing(raw, self.display(idx));
            if !out.contains(&cased) {
                out.push(cased);
            }
        }
        out.truncate(limit);
        out
    }
}

/// Structure-of-Arrays next-word phrase association table.
#[derive(Debug, Clone, Default)]
pub struct AssociationTableSoA {
    pub arena: Vec<u8>,
    pub prefix_offset: Vec<u32>,
    pub prefix_len: Vec<u16>,
    pub follow_offset: Vec<u32>,
    pub follow_len: Vec<u16>,
    pub simplified_flag: Vec<bool>,
}

const BUILTIN_ASSOCIATIONS: [(&str, &str, bool); 28] = [
    ("你好", "嗎", false), ("你好", "世界", false), ("謝謝", "你", false),
    ("謝謝", "大家", false), ("喜歡", "你", false), ("今天", "天氣", false),
    ("今天", "下午", false), ("明天", "見", false), ("大家", "好", false),
    ("沒問", "題", false), ("怎麼", "辦", false), ("好久", "不見", false),
    ("生日", "快樂", false), ("越來", "越好", false),
    ("你好", "吗", true), ("你好", "世界", true), ("谢谢", "你", true),
    ("谢谢", "大家", true), ("喜欢", "你", true), ("今天", "天气", true),
    ("今天", "下午", true), ("明天", "见", true), ("大家", "好", true),
    ("没问", "题", true), ("怎么", "办", true), ("好久", "不见", true),
    ("生日", "快乐", true), ("越来", "越好", true),
];

impl AssociationTableSoA {
    pub fn default_builtin() -> Self {
        let mut table = Self::default();
        for &(prefix, follow, hans) in &BUILTIN_ASSOCIATIONS {
            table.push(prefix, follow, hans);
        }
        table
    }

    pub fn push(&mut self, prefix: &str, follow: &str, simplified: bool) {
        let p_off = self.arena.len() as u32;
        self.arena.extend_from_slice(prefix.as_bytes());
        let f_off = self.arena.len() as u32;
        self.arena.extend_from_slice(follow.as_bytes());
        self.prefix_offset.push(p_off);
        self.prefix_len.push(prefix.len() as u16);
        self.follow_offset.push(f_off);
        self.follow_len.push(follow.len() as u16);
        self.simplified_flag.push(simplified);
    }

    /// Looks up associated follow-on tokens for `committed` surface text.
    pub fn lookup(&self, committed: &str, simplified: bool) -> Vec<&str> {
        let mut out = Vec::new();
        for i in 0..self.prefix_offset.len() {
            if self.simplified_flag[i] != simplified {
                continue;
            }
            let ps = self.prefix_offset[i] as usize;
            let pe = ps + self.prefix_len[i] as usize;
            let prefix = std::str::from_utf8(&self.arena[ps..pe]).unwrap_or("");
            if committed.ends_with(prefix) {
                let fs = self.follow_offset[i] as usize;
                let fe = fs + self.follow_len[i] as usize;
                if let Ok(follow) = std::str::from_utf8(&self.arena[fs..fe]) {
                    if !out.contains(&follow) {
                        out.push(follow);
                    }
                }
            }
        }
        out
    }
}
