#![forbid(unsafe_code)]
//! SoA Lexicon loader, binary-searched bucket/context tables, and zero-allocation
//! bigram join bonus calculation.
//!
//! # Examples
//!
//! ```
//! use meowpuut_core::Lexicon;
//!
//! let data = concat!(
//!     "MEOWPUUT-PINYIN-1\n",
//!     "hao\thao3\t好\t好\t4800\n",
//!     "ni\tni3\t你\t你\t5000\n",
//!     "nihao\tni3hao3\t你好\t你好\t5400\n",
//!     "# context\n",
//!     "你好\t600\n",
//! );
//! let lex = Lexicon::load(data).expect("valid lexicon");
//! assert!(lex.pinyin);
//! assert_eq!(lex.words.len(), 3);
//! assert_eq!(lex.range("nihao"), (2, 3));
//! assert!(lex.join_bonus("你", "好", false) > 2.0);
//! assert_eq!(lex.traditionalize("你好", false), "你好");
//! ```
//!
//! A word row may end with a sixth field `n`: the word is on the
//! never-volunteer list (`FLAG_NO_SUGGEST`) and is offered only for its own
//! full spelling; `w`: the same, and it is on the list only as a whole word
//! (`FLAG_NO_SUGGEST_WHOLE`: inside another text it is not looked for);
//! `k`: an everyday word that holds a listed word (`FLAG_LISTED_KEEP`).

use std::collections::HashMap;

use crate::context_index::{phrase_hash, PhraseIndex};
use crate::image::{ImageOwner, Text, U32Col};
use crate::pairs::Pairs;
use crate::trigrams::Trigrams;
use crate::reading::{
    han_characters_stack, is_fully_spelled_reading, is_tone, strict_han_count, Reading, KEYS, MAX_INPUT,
};
use crate::soa::{
    BucketTableSoA, ContextTableSoA, WordTableSoA, FLAG_FULLY_SPELLED, FLAG_LISTED_KEEP, FLAG_NO_SUGGEST, FLAG_NO_SUGGEST_WHOLE,
    FLAG_SINGLE_HAN_HANS, FLAG_SINGLE_HAN_HANT, FLAG_VERIFIED_SPELLING,
};

/// Immutable dictionary stored in Structure-of-Arrays (SoA) layout.
///
/// The tables and the lookup index are either read in place from a lexicon
/// image (no heap copy; see `Lexicon::from_base_in_place`) or owned.
#[derive(Debug, Clone)]
pub struct Lexicon {
    pub storage: Text,
    pub words: WordTableSoA,
    pub buckets: BucketTableSoA,
    pub context: ContextTableSoA,
    pub(crate) index: LexiconIndex,
    pub longest: usize,
    pub pinyin: bool,
    /// Word-pair table; without it the decoder ranks as before it existed.
    pairs: Option<Pairs>,
    /// Stage-2 bridge-trigram and skip-1 bigram table (`mimo_trigrams.bin`).
    trigrams: Option<Trigrams>,
    /// English lexicon (`mimo_en.bin`); the second tier of English inside
    /// pinyin and nine-key typing. Without it both behave as before.
    english: Option<crate::english::English>,
    /// Which Zhuyin syllables have rows, by first symbol (abbrev.rs); all
    /// zero for Pinyin. Built once at load.
    pub(crate) zhuyin_syllables: crate::abbrev::SyllableTable,
    /// Texts of the `FLAG_NO_SUGGEST` rows (both scripts) and their rows.
    no_suggest: NoSuggest,
    /// Keeps in-place bytes alive; `None` when everything is owned.
    owner: Option<ImageOwner>,
}

/// The words of the never-volunteer list in a lexicon: the hash of each text
/// (Traditional and Simplified) of a `FLAG_NO_SUGGEST` row with the row,
/// sorted. Built at load from the flag column (a few thousand rows, 16 bytes
/// each); not counted in `Lexicon::heap_bytes`, which is the size of the
/// tables (0 when they are read in place).
#[derive(Debug, Clone, Default)]
pub struct NoSuggest {
    rows: Vec<(u64, u32)>,
    /// Longest text, in characters.
    longest: usize,
    /// The same for the `FLAG_LISTED_KEEP` rows.
    keep: Vec<(u64, u32)>,
    keep_longest: usize,
}

impl NoSuggest {
    fn build(storage: &str, words: &WordTableSoA) -> Self {
        let (mut rows, mut keep) = (Vec::new(), Vec::new());
        let (mut longest, mut keep_longest) = (0, 0);
        for i in 0..words.len() {
            let (list, most) = if words.is_no_suggest(i) {
                (&mut rows, &mut longest)
            } else if words.is_listed_keep(i) {
                (&mut keep, &mut keep_longest)
            } else {
                continue;
            };
            let (hant, hans) = (words.text(storage, i, false), words.text(storage, i, true));
            *most = (*most).max(hant.chars().count()).max(hans.chars().count());
            list.push((phrase_hash(hant.as_bytes()), i as u32));
            if hans != hant {
                list.push((phrase_hash(hans.as_bytes()), i as u32));
            }
        }
        for list in [&mut rows, &mut keep] {
            list.sort_unstable();
            list.dedup();
            list.shrink_to_fit();
        }
        Self { rows, longest, keep, keep_longest }
    }

    /// Rows whose hash is that of `text` (the caller compares the text).
    fn candidates(&self, text: &str) -> impl Iterator<Item = u32> + '_ {
        Self::lookup(&self.rows, text)
    }

    /// `FLAG_LISTED_KEEP` rows whose hash is that of `text`.
    fn keep_candidates(&self, text: &str) -> impl Iterator<Item = u32> + '_ {
        Self::lookup(&self.keep, text)
    }

    fn lookup<'a>(list: &'a [(u64, u32)], text: &str) -> impl Iterator<Item = u32> + 'a {
        let h = phrase_hash(text.as_bytes());
        list[list.partition_point(|&(k, _)| k < h)..].iter().take_while(move |&&(k, _)| k == h).map(|&(_, r)| r)
    }

    /// Heap bytes held.
    pub fn heap_bytes(&self) -> usize {
        (self.rows.capacity() + self.keep.capacity()) * std::mem::size_of::<(u64, u32)>()
    }

    /// Whether the lexicon has no `FLAG_NO_SUGGEST` row.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// Single-character conversion table: a character's UTF-8 bytes packed
/// big-endian into a `u32` key, mapped to the word row that holds the
/// converted form. Sorted by key; looked up by binary search.
#[derive(Debug, Clone, Default)]
pub struct CharTable {
    keys: U32Col,
    words: U32Col,
}

impl CharTable {
    /// Packs a string of 1 to 4 bytes into a key (byte order is kept).
    #[inline]
    pub fn pack(text: &str) -> Option<u32> {
        let bytes = text.as_bytes();
        if bytes.is_empty() || bytes.len() > 4 {
            return None;
        }
        let mut arr = [0u8; 4];
        arr[..bytes.len()].copy_from_slice(bytes);
        Some(u32::from_be_bytes(arr))
    }

    fn build(mut entries: Vec<(u32, u32)>) -> Self {
        entries.sort_unstable();
        let mut table = Self { keys: U32Col::with_capacity(entries.len()), words: U32Col::with_capacity(entries.len()) };
        for (key, word) in entries {
            table.keys.push(key);
            table.words.push(word);
        }
        table
    }

    /// A table over prebuilt columns; `None` unless keys are strictly
    /// increasing and every row is below `word_count`.
    pub fn from_columns(keys: U32Col, words: U32Col, word_count: usize) -> Option<Self> {
        if keys.len() != words.len() {
            return None;
        }
        for i in 0..keys.len() {
            if (i > 0 && keys.get(i - 1) >= keys.get(i)) || words.get(i) as usize >= word_count {
                return None;
            }
        }
        Some(Self { keys, words })
    }

    /// Word row stored for `text`, if any.
    #[inline]
    pub fn get(&self, text: &str) -> Option<usize> {
        let key = Self::pack(text)?;
        let (mut size, mut left) = (self.keys.len(), 0usize);
        while size > 0 {
            let half = size / 2;
            let mid = left + half;
            let probe = self.keys.get(mid);
            if probe == key {
                return Some(self.words.get(mid) as usize);
            }
            if probe < key {
                left = mid + 1;
                size -= half + 1;
            } else {
                size = half;
            }
        }
        None
    }

    /// Key and word columns (for writing them into an image).
    pub fn columns(&self) -> (&U32Col, &U32Col) {
        (&self.keys, &self.words)
    }

    fn heap_bytes(&self) -> usize {
        self.keys.heap_bytes() + self.words.heap_bytes()
    }
}

/// Up to 64 short words per first reading byte (ASCII), heaviest first.
/// `starts` has 129 entries; the words for byte `b` are `words[starts[b]..starts[b + 1]]`.
#[derive(Debug, Clone, Default)]
pub struct Initials {
    starts: U32Col,
    words: U32Col,
}

impl Initials {
    /// Words kept per first byte.
    pub const LIMIT: usize = 64;

    /// A table over prebuilt columns; `None` unless `starts` is a valid
    /// partition of `words` and every row is below `word_count`.
    pub fn from_columns(starts: U32Col, words: U32Col, word_count: usize) -> Option<Self> {
        if starts.len() != 129 || starts.get(0) != 0 || starts.get(128) as usize != words.len() {
            return None;
        }
        for b in 0..128 {
            let (lo, hi) = (starts.get(b) as usize, starts.get(b + 1) as usize);
            if lo > hi || hi - lo > Self::LIMIT {
                return None;
            }
        }
        if words.iter().any(|w| w as usize >= word_count) {
            return None;
        }
        Some(Self { starts, words })
    }

    /// Start and word columns (for writing them into an image).
    pub fn columns(&self) -> (&U32Col, &U32Col) {
        (&self.starts, &self.words)
    }

    fn heap_bytes(&self) -> usize {
        self.starts.heap_bytes() + self.words.heap_bytes()
    }
}

/// Word id of every row, the key of the word-pair table (`crate::pairs`).
///
/// The ids number the distinct simplified texts of both modes in byte
/// order, from 2; 0 and 1 are the clause start and end. The writer of
/// `mimo_base.bin` stores them; a lexicon built on the heap has none.
/// `fingerprint` is the hash of that sorted text list joined by newlines; a
/// pair table carries the same number.
#[derive(Debug, Clone, Default)]
pub struct WordIds {
    pub ids: U32Col,
    /// Number of ids, the two markers included (0 = no ids).
    pub count: u32,
    pub fingerprint: u64,
}

impl WordIds {
    /// Ids for two images (pinyin, zhuyin) of one build, as the writer stores them.
    pub fn number(images: [(&str, &WordTableSoA); 2]) -> [Self; 2] {
        let mut texts: Vec<&str> = Vec::new();
        for (storage, words) in images {
            texts.extend((0..words.len()).map(|i| words.text(storage, i, true)));
        }
        texts.sort_unstable();
        texts.dedup();
        let fingerprint = phrase_hash(texts.join("\n").as_bytes());
        let count = texts.len() as u32 + 2;
        images.map(|(storage, words)| {
            let mut ids = U32Col::with_capacity(words.len());
            for i in 0..words.len() {
                let at = texts.binary_search(&words.text(storage, i, true)).unwrap_or_default();
                ids.push(at as u32 + 2);
            }
            Self { ids, count, fingerprint }
        })
    }
}

/// Lookup structures derived from the tables: exact-match hash indexes for
/// bucket keys and context phrases, the two character conversion tables, the
/// initials lists and the word ids. Built on the heap (without word ids), or
/// read in place from a `MIMO-IDX-1` section of a version 2 `mimo_base.bin`.
#[derive(Debug, Clone, Default)]
pub struct LexiconIndex {
    pub bucket: PhraseIndex,
    pub context: PhraseIndex,
    /// Simplified character -> word whose traditional text is the most frequent form.
    pub traditional: CharTable,
    /// Traditional character -> word whose simplified text is the most frequent form.
    pub simplified: CharTable,
    pub initials: Initials,
    pub word_ids: WordIds,
}

impl LexiconIndex {
    /// Builds every structure on the heap from the tables.
    pub fn build(storage: &str, words: &WordTableSoA, buckets: &BucketTableSoA, context: &ContextTableSoA) -> Self {
        let mut traditional: HashMap<u32, (u32, i32)> = HashMap::new();
        let mut simplified: HashMap<u32, (u32, i32)> = HashMap::new();
        let mut initials: Vec<Vec<u32>> = vec![Vec::new(); 128];
        for i in 0..words.len() {
            let flags = words.flags(i);
            let weight = words.weight(i);
            if (flags & FLAG_SINGLE_HAN_HANT) != 0 && (flags & FLAG_SINGLE_HAN_HANS) != 0 {
                let hans = words.text(storage, i, true);
                if let Some(key) = CharTable::pack(hans) {
                    if traditional.get(&key).map_or(true, |&(_, old_w)| old_w < weight) {
                        traditional.insert(key, (i as u32, weight));
                    }
                }
                let hant = words.text(storage, i, false);
                if hant != hans {
                    if let Some(key) = CharTable::pack(hant) {
                        if simplified.get(&key).map_or(true, |&(_, old_w)| old_w < weight) {
                            simplified.insert(key, (i as u32, weight));
                        }
                    }
                }
            }
            if words.hant_len.get(i) <= 4 {
                let first_byte = storage.as_bytes()[words.reading_offset.get(i) as usize] as usize;
                if first_byte < initials.len() { initials[first_byte].push(i as u32); }
            }
        }
        let mut starts = U32Col::with_capacity(129);
        let mut flat = U32Col::default();
        starts.push(0);
        for items in &mut initials {
            items.sort_by(|&a, &b| words.weight(b as usize).cmp(&words.weight(a as usize)));
            items.truncate(Initials::LIMIT);
            for &w in items.iter() { flat.push(w); }
            starts.push(flat.len() as u32);
        }
        flat.shrink_to_fit();
        Self {
            bucket: PhraseIndex::build(buckets.len(), |i| buckets.key(storage, i).as_bytes()),
            context: PhraseIndex::build(context.len(), |i| context.phrase(storage, i).as_bytes()),
            traditional: CharTable::build(traditional.into_iter().map(|(k, (w, _))| (k, w)).collect()),
            simplified: CharTable::build(simplified.into_iter().map(|(k, (w, _))| (k, w)).collect()),
            initials: Initials { starts, words: flat },
            word_ids: WordIds::default(),
        }
    }

    /// Heap bytes held (0 when read in place).
    pub fn heap_bytes(&self) -> usize {
        self.bucket.heap_bytes()
            + self.context.heap_bytes()
            + self.traditional.heap_bytes()
            + self.simplified.heap_bytes()
            + self.initials.heap_bytes()
            + self.word_ids.ids.heap_bytes()
    }
}

fn parse_non_negative_i32(s: &str) -> Option<i32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<i32>().ok().filter(|&v| v >= 0)
}

impl Lexicon {
    /// Parses a UTF-8 native lexicon table into SoA arrays.
    ///
    /// The input is not retained: only the strings the engine reads (bucket keys,
    /// readings, both scripts, context phrases) are copied into a compact arena,
    /// and identical traditional/simplified texts share one copy. This keeps the
    /// steady-state footprint well below the raw file size for iOS extensions.
    pub fn load(input_data: &str) -> Option<Self> {
        let data = input_data;
        let pinyin = data.starts_with("MEOWPUUT-PINYIN-1\n");
        let header = if pinyin { "MEOWPUUT-PINYIN-1\n" } else { "MEOWPUUT-ZHUYIN-1\n" };
        if !data.starts_with(header) {
            return None;
        }
        let line_estimate = data.bytes().filter(|&b| b == b'\n').count();
        let mut arena = String::with_capacity(data.len() / 2);
        let mut words = WordTableSoA::with_capacity(line_estimate);
        let mut buckets = BucketTableSoA::default();
        let mut context = ContextTableSoA::default();
        let mut longest = 0usize;
        let mut reading_context = false;
        let mut last_weight = 0i32;

        let mut offset_cursor = header.len();
        while offset_cursor < data.len() {
            let rem = &data[offset_cursor..];
            let (raw_line, next_offset) = match rem.find('\n') {
                Some(pos) => (&rem[..pos], offset_cursor + pos + 1),
                None => (rem, data.len()),
            };
            offset_cursor = next_offset;
            let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
            if line.is_empty() { continue; }
            if line == "# context" && !reading_context {
                reading_context = true;
                continue;
            }
            // A sixth field: `n` never volunteered, `w` the same but only as a whole word,
            // `k` an everyday word that holds a listed word.
            let (line, no_suggest) = match (line.strip_suffix("\tn"), line.strip_suffix("\tw"), line.strip_suffix("\tk")) {
                (Some(rest), _, _) if !reading_context => (rest, FLAG_NO_SUGGEST),
                (_, Some(rest), _) if !reading_context => (rest, FLAG_NO_SUGGEST | FLAG_NO_SUGGEST_WHOLE),
                (_, _, Some(rest)) if !reading_context => (rest, FLAG_LISTED_KEEP),
                _ => (line, 0),
            };
            let first_tab = line.find('\t')?;
            if first_tab == 0 { return None; }
            let bare = &line[..first_tab];
            let weight_tab = line.rfind('\t')?;
            let weight = parse_non_negative_i32(&line[weight_tab + 1..])?;

            if reading_context {
                if weight_tab != first_tab { return None; }
                if !context.is_empty() && context.phrase(&arena, context.len() - 1) >= bare {
                    return None;
                }
                context.push(arena.len() as u32, first_tab as u16, weight);
                arena.push_str(bare);
                continue;
            }
            if weight_tab == first_tab { return None; }
            let row = &line[first_tab + 1..weight_tab];
            let first_sep = row.find('\t')?;
            let second_sep = row[first_sep + 1..].find('\t').map(|i| first_sep + 1 + i)?;
            let reading = &row[..first_sep];
            if reading.is_empty()
                || first_sep + 1 == second_sep
                || second_sep + 1 == row.len()
                || !is_tone(*reading.as_bytes().last()?, pinyin)
            {
                return None;
            }
            let mut position = 0usize;
            let mut has_sound = false;
            let bare_bytes = bare.as_bytes();
            for &key in reading.as_bytes() {
                if is_tone(key, pinyin) {
                    if !has_sound { return None; }
                    has_sound = false;
                } else {
                    // Upper-case keys are English words (crate::latin).
                    let valid_key = if pinyin { key.is_ascii_alphabetic() } else { KEYS.contains(&key) };
                    if position == bare_bytes.len() || key != bare_bytes[position] || !valid_key || key == b'\'' {
                        return None;
                    }
                    position += 1;
                    has_sound = true;
                }
            }
            if position != bare_bytes.len() || bare_bytes.len() > MAX_INPUT { return None; }

            let word_idx = words.len() as u32;
            if buckets.is_empty() || buckets.key(&arena, buckets.len() - 1) != bare {
                if !buckets.is_empty() && buckets.key(&arena, buckets.len() - 1) > bare {
                    return None;
                }
                buckets.push(arena.len() as u32, first_tab as u16, word_idx, word_idx + 1);
                arena.push_str(bare);
                longest = longest.max(bare.len());
            } else {
                if last_weight < weight { return None; }
                let last = buckets.len() - 1;
                buckets.word_end.set(last, word_idx + 1);
            }

            let hant = &row[first_sep + 1..second_sep];
            let hans = &row[second_sep + 1..];
            let r_off = arena.len() as u32;
            arena.push_str(reading);
            let t_off = arena.len() as u32;
            arena.push_str(hant);
            let s_off = if hans == hant {
                t_off
            } else {
                let off = arena.len() as u32;
                arena.push_str(hans);
                off
            };

            let mut flags = 0u8;
            if is_fully_spelled_reading(reading) { flags |= FLAG_FULLY_SPELLED; }
            if hant.len() <= 4 && strict_han_count(hant) == 1 { flags |= FLAG_SINGLE_HAN_HANT; }
            if hans.len() <= 4 && strict_han_count(hans) == 1 { flags |= FLAG_SINGLE_HAN_HANS; }
            flags |= no_suggest;
            words.push(r_off, first_sep as u16, t_off, hant.len() as u16, s_off, hans.len() as u16, weight, flags);
            last_weight = weight;
        }
        arena.shrink_to_fit();
        words.shrink_to_fit();
        let mut lex = Self::assemble(Text::owned(arena), words, buckets, context, longest, pinyin, None, None)?;
        lex.precompute_verified_spellings();
        Some(lex)
    }

    /// Puts the tables together. Without a prebuilt `index` it is built on the heap.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn assemble(
        storage: Text,
        words: WordTableSoA,
        buckets: BucketTableSoA,
        context: ContextTableSoA,
        longest: usize,
        pinyin: bool,
        index: Option<LexiconIndex>,
        owner: Option<ImageOwner>,
    ) -> Option<Self> {
        if words.is_empty() { return None; }
        let index = index.unwrap_or_else(|| LexiconIndex::build(&storage, &words, &buckets, &context));
        let no_suggest = NoSuggest::build(&storage, &words);
        let mut lex = Self { storage, words, buckets, context, index, longest, pinyin, pairs: None, trigrams: None, english: None, zhuyin_syllables: [0; 37], no_suggest, owner };
        lex.zhuyin_syllables = crate::abbrev::syllable_table(&lex);
        Some(lex)
    }

    /// Uses `pairs` for ranking (see `crate::pairs`).
    pub fn set_pairs(&mut self, pairs: Pairs) {
        self.pairs = Some(pairs);
    }

    /// Uses `trigrams` for Stage-2 path reranking (see `crate::trigrams`).
    pub fn set_trigrams(&mut self, trigrams: Trigrams) {
        self.trigrams = Some(trigrams);
    }

    /// Uses `english` for English words inside pinyin and nine-key typing (see `crate::english`).
    pub fn set_english(&mut self, english: crate::english::English) {
        self.english = Some(english);
    }

    /// The English lexicon, if one is attached.
    #[inline]
    pub fn english(&self) -> Option<&crate::english::English> {
        self.english.as_ref()
    }

    /// The word-pair table, if one is attached.
    #[inline]
    pub fn pairs(&self) -> Option<&Pairs> {
        self.pairs.as_ref()
    }

    /// The Stage-2 bridge-trigram / skip-1 table, if one is attached.
    #[inline]
    pub fn trigrams(&self) -> Option<&Trigrams> {
        self.trigrams.as_ref()
    }

    /// The lookup index (for writing it into an image).
    pub fn index(&self) -> &LexiconIndex {
        &self.index
    }

    /// True when the tables are read in place from caller-owned bytes.
    pub fn is_in_place(&self) -> bool {
        self.owner.is_some()
    }

    fn precompute_verified_spellings(&mut self) {
        if !self.pinyin {
            for idx in 0..self.words.len() {
                let f = self.words.flags(idx);
                self.words.flags.set(idx, f | FLAG_VERIFIED_SPELLING);
            }
            return;
        }
        let mut single_str_cache: HashMap<String, bool> = HashMap::new();
        let mut verified: Vec<(usize, u8)> = Vec::new();
        for idx in 0..self.words.len() {
            let r_off = self.words.reading_offset.get(idx) as usize;
            let r_len = self.words.reading_len.get(idx) as usize;
            let spelling = &self.storage[r_off..r_off + r_len];
            let bytes = spelling.as_bytes();
            let (mut start, mut ok) = (0usize, true);
            for end in 0..bytes.len() {
                if !is_tone(bytes[end], true) { continue; }
                let syl = &spelling[start..=end];
                let found = if let Some(&cached) = single_str_cache.get(syl) {
                    cached
                } else {
                    let (lo, hi) = self.range(&syl[..syl.len() - 1]);
                    let has_single = (lo..hi).any(|e| {
                        self.words.reading(&self.storage, e) == syl && self.words.is_single_han_hant(e)
                    });
                    single_str_cache.insert(String::from(syl), has_single);
                    has_single
                };
                if !found { ok = false; break; }
                start = end + 1;
            }
            if ok && start == bytes.len() {
                let f = self.words.flags(idx);
                verified.push((idx, f | FLAG_VERIFIED_SPELLING));
            }
        }
        for (idx, f) in verified {
            self.words.flags.set(idx, f);
        }
    }

    #[inline]
    pub fn lower_bound_bucket(&self, key: &str) -> usize {
        let (mut size, mut left) = (self.buckets.len(), 0usize);
        while size > 0 {
            let half = size / 2;
            let mid = left + half;
            if self.buckets.key(&self.storage, mid) < key {
                left = mid + 1;
                size -= half + 1;
            } else {
                size = half;
            }
        }
        left
    }

    #[inline]
    pub fn range(&self, key: &str) -> (usize, usize) {
        let (storage, buckets): (&str, _) = (&self.storage, &self.buckets);
        self.index
            .bucket
            .get(key.as_bytes(), |i| buckets.key(storage, i).as_bytes())
            .map_or((0, 0), |idx| buckets.range(idx))
    }

    /// Heap bytes of the lexicon tables (storage, SoA columns, index).
    /// Bytes read in place are not counted.
    pub fn heap_bytes(&self) -> usize {
        self.storage.heap_bytes()
            + self.words.heap_bytes()
            + self.buckets.heap_bytes()
            + self.context.heap_bytes()
            + self.index.heap_bytes()
            + self.pairs.as_ref().map_or(0, Pairs::heap_bytes)
            + self.trigrams.as_ref().map_or(0, Trigrams::heap_bytes)
            + self.english.as_ref().map_or(0, crate::english::English::heap_bytes)
    }

    /// The never-volunteer index (built at load; its heap is not in `heap_bytes`).
    pub fn no_suggest(&self) -> &NoSuggest {
        &self.no_suggest
    }

    /// The `FLAG_NO_SUGGEST` rows whose text (either script) is `text`.
    pub fn no_suggest_rows<'a>(&'a self, text: &'a str) -> impl Iterator<Item = u32> + 'a {
        self.no_suggest.candidates(text).filter(move |&r| {
            self.words.text(&self.storage, r as usize, false) == text || self.words.text(&self.storage, r as usize, true) == text
        })
    }

    /// Whether `text` is a word of the never-volunteer list (`FLAG_NO_SUGGEST`), in either script.
    pub fn is_no_suggest_text(&self, text: &str) -> bool {
        self.no_suggest_rows(text).next().is_some()
    }

    /// Whether `text` is an everyday word that holds a listed word (`FLAG_LISTED_KEEP`), in either script.
    pub fn is_listed_keep_text(&self, text: &str) -> bool {
        self.no_suggest.keep_candidates(text).any(|r| {
            self.words.text(&self.storage, r as usize, false) == text || self.words.text(&self.storage, r as usize, true) == text
        })
    }

    /// Whether `text` is or holds a word of the never-volunteer list (one that
    /// counts inside other text; not inside an everyday word that holds it).
    pub fn holds_no_suggest(&self, text: &str) -> bool {
        self.is_no_suggest_text(text) || self.mentions_no_suggest(text)
    }

    /// Whether `input` (Pinyin letters or Zhuyin keys, tones and apostrophes
    /// allowed) is the full spelling of a `FLAG_NO_SUGGEST` row with text `text`:
    /// every letter (every Zhuyin symbol) of the row and no other, the tones
    /// that are typed matching. Zhuyin typed with the tone of the last syllable
    /// only (ㄓㄨㄥㄍㄨㄥˋ for 中共) is a full spelling, as it is for every other
    /// word, unless the same symbols also read as fewer syllables (ㄕㄨㄟˇ is 水
    /// as well as ㄕㄨㄟˇ for 市委): then every syllable but the last needs its tone.
    pub fn spells_no_suggest(&self, input: &str, text: &str) -> bool {
        let reading = Reading::new(input, self.pinyin);
        self.no_suggest_rows(text).any(|row| {
            let wr = self.words.reading(&self.storage, row as usize).as_bytes();
            let mut letters = wr.iter().filter(|&&b| !is_tone(b, self.pinyin));
            let toned = self.pinyin || {
                let (mut sounds, mut ends) = (0usize, true);
                for (i, &b) in wr.iter().enumerate() {
                    if !is_tone(b, false) {
                        sounds += 1;
                    } else if i + 1 < wr.len() {
                        ends &= reading.boundaries.get(sounds).is_some_and(|&t| t != 0);
                    }
                }
                ends || !self.reads_as_fewer_syllables(reading.sounds(), wr)
            };
            reading.sounds_bytes().iter().all(|b| letters.next() == Some(b))
                && letters.next().is_none()
                && toned
                && self.is_spelled_row(row as usize)
                && reading.matches(wr, 0, reading.len())
        })
    }

    /// Whether a row read by the Zhuyin symbols `sounds` has fewer syllables
    /// (tones) than the reading `than`.
    fn reads_as_fewer_syllables(&self, sounds: &str, than: &[u8]) -> bool {
        let syllables = |r: &[u8]| r.iter().filter(|&&b| is_tone(b, false)).count();
        let n = syllables(than);
        let (lo, hi) = self.range(sounds);
        (lo..hi).any(|i| syllables(self.words.reading(&self.storage, i).as_bytes()) < n)
    }

    /// Whether row `row` spells its word in full: every Zhuyin row, a Pinyin
    /// row of whole syllables (not an initials row such as k1f1g3).
    fn is_spelled_row(&self, row: usize) -> bool {
        !self.pinyin || self.words.is_fully_spelled(row)
    }

    /// Letters of row `row`'s reading (tones left out) into `out`; their count.
    fn row_letters(&self, row: usize, out: &mut [u8; MAX_INPUT]) -> usize {
        let mut n = 0;
        for &b in self.words.reading(&self.storage, row).as_bytes() {
            if !is_tone(b, self.pinyin) && n < out.len() {
                out[n] = b;
                n += 1;
            }
        }
        n
    }

    /// Whether some `FLAG_NO_SUGGEST` row with text `text` that counts inside
    /// other text (not `FLAG_NO_SUGGEST_WHOLE`) has letters for which
    /// `typed` holds (`None`: no such row).
    fn inside_spelled(&self, text: &str, typed: &impl Fn(&[u8]) -> bool) -> Option<bool> {
        let mut any = false;
        let mut letters = [0u8; MAX_INPUT];
        for row in self.no_suggest_rows(text) {
            if self.words.is_no_suggest_whole(row as usize) || !self.is_spelled_row(row as usize) {
                continue;
            }
            any = true;
            let n = self.row_letters(row as usize, &mut letters);
            if n > 0 && typed(&letters[..n]) {
                return Some(true);
            }
        }
        any.then_some(false)
    }

    /// Whether offering `text` for `input` would volunteer a word of the
    /// never-volunteer list: `text` is such a word and `input` is not its
    /// full spelling, or `text` holds one (two characters or more, not a
    /// whole-word-only one) whose letters the input does not have (a word
    /// typed by its initials or with a typo inside a longer reading).
    pub fn volunteers(&self, input: &str, text: &str) -> bool {
        if self.no_suggest.rows.is_empty() {
            return false;
        }
        if self.is_no_suggest_text(text) {
            return !self.spells_no_suggest(input, text);
        }
        let reading = Reading::new(input, self.pinyin);
        let sounds = reading.sounds_bytes();
        self.volunteers_by(text, |_| false, |letters| sounds.windows(letters.len()).any(|w| w == letters))
    }

    /// `volunteers` for keys of another kind (nine-key digits): `whole`
    /// says whether the keys are the full spelling of a row with these
    /// letters (its reading without tones), `inside` whether they contain it.
    pub fn volunteers_by(&self, text: &str, whole: impl Fn(&[u8]) -> bool, inside: impl Fn(&[u8]) -> bool) -> bool {
        if self.no_suggest.rows.is_empty() {
            return false;
        }
        if self.is_no_suggest_text(text) {
            let mut letters = [0u8; MAX_INPUT];
            return !self.no_suggest_rows(text).any(|row| {
                let n = self.row_letters(row as usize, &mut letters);
                self.is_spelled_row(row as usize) && whole(&letters[..n])
            });
        }
        self.each_part(text, |part| self.inside_spelled(part, &inside) == Some(false))
    }

    /// Whether `text` holds a word of the never-volunteer list of two
    /// characters or more that counts inside other text (for corrections,
    /// which are never the typed spelling).
    pub fn mentions_no_suggest(&self, text: &str) -> bool {
        !self.no_suggest.rows.is_empty() && self.each_part(text, |part| self.inside_spelled(part, &|_| false).is_some())
    }

    /// Whether `right` (a next word) after `left` (the text before it) makes
    /// a word of the never-volunteer list: `right` is one, or a part of
    /// `left` and `right` together that starts in `left` (a first character
    /// and the rest of the word) is one
    /// that counts inside other text.
    pub fn completes_no_suggest(&self, left: &str, right: &str) -> bool {
        if self.no_suggest.rows.is_empty() || right.is_empty() {
            return false;
        }
        if self.is_no_suggest_text(right) {
            return true;
        }
        let mut joined = [0u8; 64];
        let (l, r) = (left.as_bytes(), right.as_bytes());
        let l = &l[l.len().saturating_sub(16)..];
        if l.len() + r.len() > joined.len() {
            return false;
        }
        joined[..l.len()].copy_from_slice(l);
        joined[l.len()..l.len() + r.len()].copy_from_slice(r);
        let Ok(text) = std::str::from_utf8(&joined[..l.len() + r.len()]) else { return false };
        let boundary = l.len();
        let longest = self.no_suggest.longest.max(2);
        let mut starts = [0usize; 65];
        let mut n = 0;
        for (i, _) in text.char_indices() {
            starts[n] = i;
            n += 1;
        }
        starts[n] = text.len();
        let starts = &starts[..=n];
        for a in 0..starts.len() - 1 {
            if starts[a] >= boundary {
                break;
            }
            for b in a + 2..starts.len().min(a + longest + 1) {
                if starts[b] <= boundary {
                    continue;
                }
                if self.inside_spelled(&text[starts[a]..starts[b]], &|_| false).is_some() {
                    return true;
                }
            }
        }
        false
    }

    /// Whether `test` holds for some part of `text` of two characters or more
    /// (at most the longest never-volunteer word; the first 64 characters).
    /// A part inside an everyday word that holds a listed word (`FLAG_LISTED_KEEP`:
    /// 印度支那, 總統套房) is not tested.
    fn each_part(&self, text: &str, mut test: impl FnMut(&str) -> bool) -> bool {
        let mut starts = [0usize; 65];
        let mut n = 0;
        let mut end = text.len();
        for (i, _) in text.char_indices() {
            if n == 64 {
                end = i;
                break;
            }
            starts[n] = i;
            n += 1;
        }
        starts[n] = end;
        // kept[a]: the end (in characters) of the longest everyday word that starts at a.
        let mut kept = [0usize; 64];
        if !self.no_suggest.keep.is_empty() {
            let keep_longest = self.no_suggest.keep_longest;
            for a in 0..n {
                for b in (a + 3..=n.min(a + keep_longest)).rev() {
                    if self.is_listed_keep_text(&text[starts[a]..starts[b]]) {
                        kept[a] = b;
                        break;
                    }
                }
            }
        }
        let inside_kept = |a: usize, b: usize| (a.saturating_sub(self.no_suggest.keep_longest)..=a).any(|k| kept[k] >= b);
        let longest = self.no_suggest.longest.max(2);
        for a in 0..n {
            for b in a + 2..=n.min(a + longest) {
                if !(self.no_suggest.keep_longest > 0 && inside_kept(a, b)) && test(&text[starts[a]..starts[b]]) {
                    return true;
                }
            }
        }
        false
    }

    /// Words with a short text whose reading starts with `first_byte`,
    /// heaviest first (at most 64; none for non-ASCII bytes).
    /// `FLAG_NO_SUGGEST` words are left out: a first letter is never their spelling.
    pub fn initials(&self, first_byte: u8) -> impl Iterator<Item = usize> + '_ {
        let (starts, words) = self.index.initials.columns();
        let (lo, hi) = if first_byte < 128 {
            (starts.get(first_byte as usize) as usize, starts.get(first_byte as usize + 1) as usize)
        } else {
            (0, 0)
        };
        (lo..hi).map(move |k| words.get(k) as usize).filter(move |&w| !self.words.is_no_suggest(w))
    }

    /// Frequency of an exact context phrase, via the O(1) hashed index.
    #[inline]
    pub fn context_frequency(&self, phrase: &[u8]) -> Option<i32> {
        let (storage, context): (&str, _) = (&self.storage, &self.context);
        self.index
            .context
            .get(phrase, |i| context.phrase(storage, i).as_bytes())
            .map(|row| context.frequency(row))
    }

    /// Computes context join bonus between `left` and `right` using a stack-allocated 64-byte buffer.
    pub fn join_bonus(&self, left: &str, right: &str, distinguish_strong: bool) -> f64 {
        if self.context.is_empty() || right.is_empty() { return 0.0; }
        let (mut starts, mut ends) = ([0usize; 4], [0usize; 4]);
        let (mut left_count, mut right_count) = (0usize, 0usize);
        let is_start = |byte: u8| (byte & 0xC0) != 0x80;

        let left_bytes = left.as_bytes();
        let mut i = left_bytes.len();
        while i > 0 && left_count < starts.len() {
            i -= 1;
            if is_start(left_bytes[i]) {
                starts[left_count] = i;
                left_count += 1;
            }
        }
        let right_bytes = right.as_bytes();
        for idx in 1..=right_bytes.len() {
            if right_count >= ends.len() { break; }
            if idx == right_bytes.len() || is_start(right_bytes[idx]) {
                ends[right_count] = idx;
                right_count += 1;
            }
        }
        if left_count == 0 || right_count == 0 { return 0.0; }
        let offset = starts[left_count - 1];
        let left_tail = &left_bytes[offset..];
        let right_head = &right_bytes[..ends[right_count - 1]];
        let total_len = left_tail.len() + right_head.len();
        if total_len > 64 { return 0.0; }
        let mut joined_buf = [0u8; 64];
        joined_buf[..left_tail.len()].copy_from_slice(left_tail);
        joined_buf[left_tail.len()..total_len].copy_from_slice(right_head);
        let joined = &joined_buf[..total_len];

        let mut frequency = 0i32;
        for li in 0..left_count {
            let begin = starts[li] - offset;
            for rj in 0..right_count {
                if li + rj >= 4 { break; }
                let len = left_bytes.len() - starts[li] + ends[rj];
                if let Some(f) = self.context_frequency(&joined[begin..begin + len]) {
                    frequency = frequency.max(f);
                }
            }
        }
        if distinguish_strong && right_count > 1 {
            (3.0 * (f64::from(frequency).ln_1p() / 10000.0_f64.ln_1p())).min(3.0)
        } else {
            (0.4 * f64::from(frequency).ln_1p()).min(3.0)
        }
    }

    #[inline]
    pub fn traditional_char<'a>(&'a self, ch: &'a str) -> &'a str {
        if let Some(word) = self.index.traditional.get(ch) {
            self.words.text(&self.storage, word, false)
        } else {
            ch
        }
    }

    /// Most frequent simplified form of one traditional character (identity if unknown).
    #[inline]
    pub fn simplified_char<'a>(&'a self, ch: &'a str) -> &'a str {
        if let Some(word) = self.index.simplified.get(ch) {
            self.words.text(&self.storage, word, true)
        } else {
            ch
        }
    }

    pub fn traditionalize_into(&self, value: &str, tail: bool, out: &mut String) {
        out.clear();
        let Some(chars) = han_characters_stack(value) else { return; };
        let start = if tail && chars.len() > 4 { chars.len() - 4 } else { 0 };
        let end = chars.len().min(start + 4);
        for i in start..end {
            out.push_str(self.traditional_char(chars.slice(value, i)));
        }
    }

    pub fn traditionalize(&self, value: &str, tail: bool) -> String {
        let mut out = String::with_capacity(16);
        self.traditionalize_into(value, tail, &mut out);
        out
    }
}
