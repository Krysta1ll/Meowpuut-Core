#![forbid(unsafe_code)]
//! Reader for the Japanese lexicon image `MIMO-JP-1` (`mimo_jp.bin`) and the whole-input lookup.
//!
//! The image is read in place: every access is a bounds-checked slice plus `from_le_bytes`.
//! Opening checks the header only. A damaged image can make a lookup return fewer rows; it
//! cannot panic or read outside the buffer.
//!
//! Layout (little-endian, offsets from the start of the file, sections 8-byte aligned):
//!
//! ```text
//! header, 64 bytes
//!    0  12 bytes  magic "MIMO-JP-1\0\0\0"
//!   12  u32 version = 1          16  u32 key_count        20  u32 posting_count
//!   24  u32 word_count           28  u32 reviewed_count   32  u32 keys_offset
//!   36  u32 postings_offset      40  u32 words_offset     44  u32 reviewed_offset
//!   48  u32 blob_offset          52  u32 blob_len         56  u32 file_len
//!   60  u32 chinese_build
//! key record, 12 bytes, sorted by key bytes
//!   u32 key_offset (into the blob)  u16 key_len  u16 posting_count  u32 first_posting
//! posting record, 8 bytes, grouped by key
//!   u32 word  i16 weight  u8 kind  u8 flags
//! word record, 8 bytes
//!   u32 surface_offset (into the blob)  u16 surface_len  u8 class  u8 flags
//! reviewed record, 16 bytes, sorted by reading bytes
//!   u32 reading_offset  u16 reading_len  u16 text_len  u32 text_offset  i16 weight  u16 flags
//! blob: UTF-8 strings without terminators; the key strings first, back to back in key order,
//!   then every other string once
//! ```
//!
//! # Examples
//!
//! ```
//! use meowpuut_core::japanese::{JapaneseLexicon, JapaneseMode, JapaneseQuery};
//!
//! assert!(JapaneseLexicon::from_vec(Vec::new()).is_none());
//!
//! // An image without any word: the header alone.
//! let mut image = Vec::new();
//! image.extend_from_slice(b"MIMO-JP-1\0\0\0");
//! for field in [1u32, 0, 0, 0, 0, 64, 64, 64, 64, 64, 0, 64, 0] {
//!     image.extend_from_slice(&field.to_le_bytes());
//! }
//! let lexicon = JapaneseLexicon::from_vec(image).expect("valid header");
//! assert!(lexicon.verify());
//!
//! let mut query = JapaneseQuery::new();
//! assert_eq!(query.lookup(&lexicon, "neko", JapaneseMode::Japanese, 64), 0);
//! assert_eq!(query.kana(), "ねこ");
//! assert_eq!(query.katakana(), "ネコ");
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use crate::romaji::{normalise_key, to_kana, to_katakana};

/// First 12 bytes of an image.
pub const MAGIC: &[u8; 12] = b"MIMO-JP-1\0\0\0";
/// Format version this reader accepts.
pub const VERSION: u32 = 1;
/// Header size in bytes.
pub const HEADER_LEN: usize = 64;
/// Record sizes in bytes.
pub const KEY_RECORD_LEN: usize = 12;
/// Record sizes in bytes.
pub const POSTING_RECORD_LEN: usize = 8;
/// Record sizes in bytes.
pub const WORD_RECORD_LEN: usize = 8;
/// Record sizes in bytes.
pub const REVIEWED_RECORD_LEN: usize = 16;

/// Limits the writer keeps; `verify` checks them.
pub const MAX_KEY_BYTES: usize = 96;
/// Longest surface or reviewed text in bytes.
pub const MAX_SURFACE_BYTES: usize = 128;
/// Most postings under one key. A lookup never reads more than this from one key.
pub const MAX_POSTINGS_PER_KEY: usize = 64;
/// Most rows in the reviewed section.
pub const MAX_REVIEWED_ROWS: usize = 4096;
/// Longest lookup input in bytes.
pub const MAX_INPUT_BYTES: usize = 80;

/// Posting kind: the key is the normalised kana reading.
pub const KIND_READING: u8 = 0;
/// Posting kind: the key is the kana of a sloppy spelling (`renai`, `tokyo`).
pub const KIND_VARIANT: u8 = 1;
/// Posting kind: ASCII key, pinyin of the kanji plus Hepburn of the kana.
pub const KIND_ALIAS_KANA: u8 = 2;
/// Posting kind: ASCII key, pinyin of the kanji.
pub const KIND_ALIAS_KANJI: u8 = 3;
/// Posting kind: ASCII key written by hand in the reviewed lists.
pub const KIND_AUTHORED: u8 = 4;

/// Posting flag: may be offered in pinyin mode.
pub const FLAG_PINYIN_OK: u8 = 1;
/// Posting and word flag: reviewed word.
pub const FLAG_REVIEWED: u8 = 2;
/// Posting and word flag: generated inflected form.
pub const FLAG_FORM: u8 = 4;

/// Reviewed-row flag: register the row for mixed sentences.
pub const REVIEWED_POOL: u32 = 1;

#[derive(Clone)]
enum Bytes {
    Static(&'static [u8]),
    Owned(Arc<[u8]>),
}

/// An opened image. Cheap to clone; safe to share between threads.
#[derive(Clone)]
pub struct JapaneseLexicon {
    bytes: Bytes,
    key_count: usize,
    posting_count: usize,
    word_count: usize,
    reviewed_count: usize,
    keys_offset: usize,
    postings_offset: usize,
    words_offset: usize,
    reviewed_offset: usize,
    blob_offset: usize,
    blob_len: usize,
    chinese_build: u32,
}

struct KeyRecord {
    key_offset: u32,
    key_len: u16,
    posting_count: usize,
    first_posting: usize,
}

struct Posting {
    word: u32,
    weight: i16,
    kind: u8,
    flags: u8,
}

struct WordRecord {
    surface_offset: u32,
    surface_len: u16,
    flags: u8,
}

struct ReviewedRecord {
    reading_offset: u32,
    reading_len: u16,
    text_len: u16,
    text_offset: u32,
    weight: i16,
    flags: u16,
}

fn le_u16(bytes: &[u8], at: usize) -> Option<u16> {
    let field = bytes.get(at..at.checked_add(2)?)?;
    Some(u16::from_le_bytes(field.try_into().ok()?))
}

fn le_i16(bytes: &[u8], at: usize) -> Option<i16> {
    let field = bytes.get(at..at.checked_add(2)?)?;
    Some(i16::from_le_bytes(field.try_into().ok()?))
}

fn le_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let field = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes(field.try_into().ok()?))
}

impl JapaneseLexicon {
    /// Opens an image that the caller keeps alive, for example a mapped file.
    pub fn from_static(bytes: &'static [u8]) -> Option<Self> {
        Self::open(Bytes::Static(bytes))
    }

    /// Opens an image held on the heap.
    pub fn from_vec(bytes: Vec<u8>) -> Option<Self> {
        Self::open(Bytes::Owned(Arc::from(bytes)))
    }

    /// Checks the magic, the version, `file_len`, and that the five sections lie inside the
    /// file behind the header and do not overlap. Does not read the tables.
    fn open(bytes: Bytes) -> Option<Self> {
        let data: &[u8] = match &bytes {
            Bytes::Static(data) => data,
            Bytes::Owned(data) => data,
        };
        if data.get(..MAGIC.len())? != MAGIC || le_u32(data, 12)? != VERSION {
            return None;
        }
        let field = |at: usize| le_u32(data, at).map(u64::from);
        let file_len = field(56)?;
        if file_len != u64::try_from(data.len()).ok()? || file_len >= 1 << 31 {
            return None;
        }
        // (offset, record count, record size) in header order.
        let sections = [
            (field(32)?, field(16)?, KEY_RECORD_LEN),
            (field(36)?, field(20)?, POSTING_RECORD_LEN),
            (field(40)?, field(24)?, WORD_RECORD_LEN),
            (field(44)?, field(28)?, REVIEWED_RECORD_LEN),
            (field(48)?, field(52)?, 1),
        ];
        let mut spans = [(0u64, 0u64); 5];
        for (span, &(offset, count, size)) in spans.iter_mut().zip(&sections) {
            let end = offset.checked_add(count.checked_mul(u64::try_from(size).ok()?)?)?;
            if offset < u64::try_from(HEADER_LEN).ok()? || end > file_len {
                return None;
            }
            *span = (offset, end);
        }
        for (index, first) in spans.iter().enumerate() {
            for second in spans.iter().skip(index + 1) {
                let empty = first.0 == first.1 || second.0 == second.1;
                if !empty && first.0 < second.1 && second.0 < first.1 {
                    return None;
                }
            }
        }
        let size = |value: u64| usize::try_from(value).ok();
        let lexicon = Self {
            key_count: size(sections[0].1)?,
            posting_count: size(sections[1].1)?,
            word_count: size(sections[2].1)?,
            reviewed_count: size(sections[3].1)?,
            keys_offset: size(sections[0].0)?,
            postings_offset: size(sections[1].0)?,
            words_offset: size(sections[2].0)?,
            reviewed_offset: size(sections[3].0)?,
            blob_offset: size(sections[4].0)?,
            blob_len: size(sections[4].1)?,
            chinese_build: le_u32(data, 60)?,
            bytes,
        };
        Some(lexicon)
    }

    fn data(&self) -> &[u8] {
        match &self.bytes {
            Bytes::Static(data) => data,
            Bytes::Owned(data) => data,
        }
    }

    /// Number of keys.
    pub fn key_count(&self) -> usize {
        self.key_count
    }

    /// Number of words.
    pub fn word_count(&self) -> usize {
        self.word_count
    }

    /// First four bytes of the sha256 of the Chinese lexicon the image was compiled with.
    pub fn chinese_build(&self) -> u32 {
        self.chinese_build
    }

    /// Number of rows in the reviewed section.
    pub fn reviewed_count(&self) -> usize {
        self.reviewed_count
    }

    fn record(&self, table: usize, size: usize, count: usize, index: usize) -> Option<&[u8]> {
        if index >= count {
            return None;
        }
        let start = table.checked_add(index.checked_mul(size)?)?;
        self.data().get(start..start.checked_add(size)?)
    }

    fn blob(&self, offset: u32, len: u16) -> Option<&[u8]> {
        let offset = usize::try_from(offset).ok()?;
        let end = offset.checked_add(usize::from(len))?;
        if end > self.blob_len {
            return None;
        }
        let start = self.blob_offset.checked_add(offset)?;
        self.data().get(start..self.blob_offset.checked_add(end)?)
    }

    fn blob_str(&self, offset: u32, len: u16) -> Option<&str> {
        std::str::from_utf8(self.blob(offset, len)?).ok()
    }

    fn key_record(&self, index: usize) -> Option<KeyRecord> {
        let record = self.record(self.keys_offset, KEY_RECORD_LEN, self.key_count, index)?;
        Some(KeyRecord {
            key_offset: le_u32(record, 0)?,
            key_len: le_u16(record, 4)?,
            posting_count: usize::from(le_u16(record, 6)?),
            first_posting: usize::try_from(le_u32(record, 8)?).ok()?,
        })
    }

    fn posting(&self, index: usize) -> Option<Posting> {
        let record = self.record(self.postings_offset, POSTING_RECORD_LEN, self.posting_count, index)?;
        Some(Posting {
            word: le_u32(record, 0)?,
            weight: le_i16(record, 4)?,
            kind: *record.get(6)?,
            flags: *record.get(7)?,
        })
    }

    fn word_record(&self, index: u32) -> Option<WordRecord> {
        let index = usize::try_from(index).ok()?;
        let record = self.record(self.words_offset, WORD_RECORD_LEN, self.word_count, index)?;
        Some(WordRecord {
            surface_offset: le_u32(record, 0)?,
            surface_len: le_u16(record, 4)?,
            flags: *record.get(7)?,
        })
    }

    fn reviewed_record(&self, index: usize) -> Option<ReviewedRecord> {
        let record = self.record(self.reviewed_offset, REVIEWED_RECORD_LEN, self.reviewed_count, index)?;
        Some(ReviewedRecord {
            reading_offset: le_u32(record, 0)?,
            reading_len: le_u16(record, 4)?,
            text_len: le_u16(record, 6)?,
            text_offset: le_u32(record, 8)?,
            weight: le_i16(record, 12)?,
            flags: le_u16(record, 14)?,
        })
    }

    /// Index of the key record with exactly these bytes.
    fn find_key(&self, key: &[u8]) -> Option<usize> {
        let (mut low, mut high) = (0, self.key_count);
        while low < high {
            let middle = low + (high - low) / 2;
            let record = self.key_record(middle)?;
            match self.blob(record.key_offset, record.key_len)?.cmp(key) {
                std::cmp::Ordering::Less => low = middle + 1,
                std::cmp::Ordering::Greater => high = middle,
                std::cmp::Ordering::Equal => return Some(middle),
            }
        }
        None
    }

    /// Reviewed row `index`: reading, text, weight, flags (`REVIEWED_POOL`).
    pub fn reviewed_row(&self, index: usize) -> Option<(&str, &str, i32, u32)> {
        let record = self.reviewed_record(index)?;
        let reading = self.blob_str(record.reading_offset, record.reading_len)?;
        let text = self.blob_str(record.text_offset, record.text_len)?;
        Some((reading, text, i32::from(record.weight), u32::from(record.flags)))
    }

    /// Index of the reviewed row with exactly this reading.
    pub fn reviewed_find(&self, reading: &str) -> Option<usize> {
        let (mut low, mut high) = (0, self.reviewed_count);
        while low < high {
            let middle = low + (high - low) / 2;
            let record = self.reviewed_record(middle)?;
            match self.blob(record.reading_offset, record.reading_len)?.cmp(reading.as_bytes()) {
                std::cmp::Ordering::Less => low = middle + 1,
                std::cmp::Ordering::Greater => high = middle,
                std::cmp::Ordering::Equal => return Some(middle),
            }
        }
        None
    }

    /// Full pass over the image, for tests and build checks. True when:
    /// - the sections start on 8-byte boundaries;
    /// - keys are 1..=96 bytes of UTF-8, strictly ascending in byte order, and their strings lie
    ///   back to back from the start of the blob;
    /// - every key has 1..=64 postings and every posting belongs to exactly one key;
    /// - postings have a valid word index, weight 1..=10000, kind 0..=4 and no unknown flag, and
    ///   inside a key they are ordered reviewed first, then weight descending, then surface, with
    ///   no surface twice;
    /// - surfaces are 1..=128 bytes of UTF-8, and equal surfaces have the same offset;
    /// - reviewed rows (at most 4096) are strictly ascending by reading, with a reading of
    ///   1..=96 bytes, a text of 1..=128 bytes, weight 1..=10000 and no unknown flag.
    pub fn verify(&self) -> bool {
        self.verify_pass().is_some()
    }

    fn verify_pass(&self) -> Option<()> {
        let check = |condition: bool| condition.then_some(());
        let offsets =
            [self.keys_offset, self.postings_offset, self.words_offset, self.reviewed_offset, self.blob_offset];
        check(offsets.iter().all(|offset| offset % 8 == 0))?;

        let mut offsets_by_surface: HashMap<&str, u32> = HashMap::with_capacity(self.word_count);
        for index in 0..self.word_count {
            let word = self.word_record(u32::try_from(index).ok()?)?;
            let surface = self.blob_str(word.surface_offset, word.surface_len)?;
            check((1..=MAX_SURFACE_BYTES).contains(&surface.len()))?;
            check(word.flags & !(FLAG_REVIEWED | FLAG_FORM) == 0)?;
            check(*offsets_by_surface.entry(surface).or_insert(word.surface_offset) == word.surface_offset)?;
        }

        let mut claimed = vec![false; self.posting_count];
        let mut previous_key: Option<&[u8]> = None;
        let mut next_key_offset = 0usize;
        for index in 0..self.key_count {
            let record = self.key_record(index)?;
            let key = self.blob(record.key_offset, record.key_len)?;
            check((1..=MAX_KEY_BYTES).contains(&key.len()) && std::str::from_utf8(key).is_ok())?;
            check(previous_key.is_none_or(|previous| previous < key))?;
            check(usize::try_from(record.key_offset).ok()? == next_key_offset)?;
            next_key_offset += key.len();
            previous_key = Some(key);

            check((1..=MAX_POSTINGS_PER_KEY).contains(&record.posting_count))?;
            let mut previous: Option<(bool, i16, &str)> = None;
            for slot in 0..record.posting_count {
                let at = record.first_posting.checked_add(slot)?;
                let taken = claimed.get_mut(at)?;
                check(!*taken)?;
                *taken = true;
                let posting = self.posting(at)?;
                let word = self.word_record(posting.word)?;
                let surface = self.blob_str(word.surface_offset, word.surface_len)?;
                check((1..=10_000).contains(&posting.weight) && posting.kind <= KIND_AUTHORED)?;
                check(posting.flags & !(FLAG_PINYIN_OK | FLAG_REVIEWED | FLAG_FORM) == 0)?;
                // Order: reviewed first, weight descending, surface ascending.
                let rank = (posting.flags & FLAG_REVIEWED == 0, -posting.weight, surface);
                check(previous.is_none_or(|before| before < rank))?;
                previous = Some(rank);
                for earlier in 0..slot {
                    let other = self.word_record(self.posting(record.first_posting + earlier)?.word)?;
                    check(self.blob_str(other.surface_offset, other.surface_len)? != surface)?;
                }
            }
        }
        check(claimed.iter().all(|&taken| taken))?;

        check(self.reviewed_count <= MAX_REVIEWED_ROWS)?;
        let mut previous_reading: Option<&str> = None;
        for index in 0..self.reviewed_count {
            let record = self.reviewed_record(index)?;
            let (reading, text, weight, _) = self.reviewed_row(index)?;
            check((1..=MAX_KEY_BYTES).contains(&reading.len()))?;
            check((1..=MAX_SURFACE_BYTES).contains(&text.len()))?;
            check((1..=10_000).contains(&weight) && u32::from(record.flags) & !REVIEWED_POOL == 0)?;
            check(previous_reading.is_none_or(|previous| previous.as_bytes() < reading.as_bytes()))?;
            previous_reading = Some(reading);
        }
        Some(())
    }
}

/// Which words a lookup may return.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JapaneseMode {
    /// Japanese mode: every word of the reading, any romaji spelling.
    Japanese,
    /// Pinyin mode: only postings with `FLAG_PINYIN_OK`, and kana keys only for Hepburn input.
    Pinyin,
}

/// One result of a lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JapaneseRow {
    /// Index of the word in the image; equal for the same word under two keys.
    pub word: u32,
    /// Posting weight, 1..=10000.
    pub weight: i32,
    /// `KIND_*`.
    pub kind: u8,
    /// `FLAG_*` of the posting.
    pub flags: u8,
}

#[derive(Clone, Copy)]
struct Hit {
    row: JapaneseRow,
    surface_offset: u32,
    surface_len: u16,
}

/// Buffers and result of one lookup. Reused between lookups, so a lookup does not allocate
/// once the buffers have grown to their working size.
pub struct JapaneseQuery {
    input: String,
    kana: String,
    katakana: String,
    key: String,
    scratch: String,
    hits: Vec<Hit>,
    texts: String,
    spans: Vec<(usize, usize)>,
}

impl Default for JapaneseQuery {
    fn default() -> Self {
        Self::new()
    }
}

impl JapaneseQuery {
    /// Empty query with its buffers reserved.
    pub fn new() -> Self {
        Self {
            input: String::with_capacity(MAX_INPUT_BYTES),
            kana: String::with_capacity(256),
            katakana: String::with_capacity(256),
            key: String::with_capacity(256),
            scratch: String::with_capacity(256),
            // Two keys can be read, each with up to MAX_POSTINGS_PER_KEY postings.
            hits: Vec::with_capacity(2 * MAX_POSTINGS_PER_KEY),
            texts: String::with_capacity(2048),
            spans: Vec::with_capacity(MAX_POSTINGS_PER_KEY),
        }
    }

    /// Looks up the whole input and returns the number of words found (at most `limit`).
    ///
    /// The input is romaji or an ASCII alias: a-z (capitals are read as lower case), `'` and
    /// `-`, at most 80 bytes. Anything else gives 0 and an empty `kana()`.
    ///
    /// Two keys are tried: the input as typed (again without `'` when that misses), and the
    /// normalised kana of the input. The kana key is used when the input converts completely
    /// and, in pinyin mode, only when it is Hepburn. In Japanese mode a kana key that misses is
    /// tried once more with every んん read as ん (`sinnbun`).
    ///
    /// Result order: reviewed words first, then weight descending; equal rows keep the stored
    /// order, postings of the typed key before those of the kana key. A surface is returned
    /// once.
    pub fn lookup(&mut self, lexicon: &JapaneseLexicon, input: &str, mode: JapaneseMode, limit: usize) -> usize {
        self.input.clear();
        self.kana.clear();
        self.katakana.clear();
        self.hits.clear();
        self.texts.clear();
        self.spans.clear();
        if input.is_empty() || input.len() > MAX_INPUT_BYTES {
            return 0;
        }
        for byte in input.bytes() {
            let lower = byte.to_ascii_lowercase();
            if !(lower.is_ascii_lowercase() || lower == b'\'' || lower == b'-') {
                self.input.clear();
                return 0;
            }
            self.input.push(char::from(lower));
        }
        let converted = to_kana(&self.input, &mut self.kana);
        if converted.complete {
            to_katakana(&self.kana, &mut self.katakana);
        } else {
            self.kana.clear();
        }

        let mut typed = lexicon.find_key(self.input.as_bytes());
        if typed.is_none() && self.input.contains('\'') {
            self.key.clear();
            self.key.extend(self.input.chars().filter(|&ch| ch != '\''));
            typed = lexicon.find_key(self.key.as_bytes());
        }
        if let Some(index) = typed {
            self.collect(lexicon, index, mode);
        }

        if converted.complete && !self.kana.is_empty() && (mode == JapaneseMode::Japanese || converted.strict) {
            self.key.clear();
            normalise_key(&self.kana, &mut self.key);
            let mut reading = lexicon.find_key(self.key.as_bytes());
            if reading.is_none() && mode == JapaneseMode::Japanese && self.kana.contains("んん") {
                self.key_without_double_n();
                reading = lexicon.find_key(self.key.as_bytes());
            }
            if let Some(index) = reading {
                self.collect(lexicon, index, mode);
            }
        }

        // Stable insertion sort: at most 128 short records, and no allocation.
        for index in 1..self.hits.len() {
            let mut slot = index;
            while slot > 0 {
                let (Some(&before), Some(&current)) = (self.hits.get(slot - 1), self.hits.get(slot)) else { break };
                if rank(&before.row) <= rank(&current.row) {
                    break;
                }
                self.hits.swap(slot - 1, slot);
                slot -= 1;
            }
        }
        self.hits.truncate(limit);
        for hit in &self.hits {
            let text = lexicon.blob_str(hit.surface_offset, hit.surface_len).unwrap_or("");
            self.spans.push((self.texts.len(), self.texts.len() + text.len()));
            self.texts.push_str(text);
        }
        self.hits.len()
    }

    /// Writes into `key` the normalised key of the kana with every んん read as ん.
    fn key_without_double_n(&mut self) {
        self.scratch.clear();
        let mut pending = false;
        for ch in self.kana.chars() {
            if ch == 'ん' && pending {
                pending = false;
                continue;
            }
            pending = ch == 'ん';
            self.scratch.push(ch);
        }
        self.key.clear();
        normalise_key(&self.scratch, &mut self.key);
    }

    fn collect(&mut self, lexicon: &JapaneseLexicon, key_index: usize, mode: JapaneseMode) {
        let Some(record) = lexicon.key_record(key_index) else { return };
        for slot in 0..record.posting_count.min(MAX_POSTINGS_PER_KEY) {
            if self.hits.len() >= 2 * MAX_POSTINGS_PER_KEY {
                return;
            }
            let Some(posting) = record.first_posting.checked_add(slot).and_then(|at| lexicon.posting(at)) else {
                return;
            };
            if mode == JapaneseMode::Pinyin && posting.flags & FLAG_PINYIN_OK == 0 {
                continue;
            }
            let Some(word) = lexicon.word_record(posting.word) else { continue };
            // The text must be readable; equal surfaces share one offset in the blob.
            if lexicon.blob_str(word.surface_offset, word.surface_len).is_none() {
                continue;
            }
            let seen = self
                .hits
                .iter()
                .any(|hit| hit.surface_offset == word.surface_offset && hit.surface_len == word.surface_len);
            if seen {
                continue;
            }
            self.hits.push(Hit {
                row: JapaneseRow {
                    word: posting.word,
                    weight: i32::from(posting.weight),
                    kind: posting.kind,
                    flags: posting.flags,
                },
                surface_offset: word.surface_offset,
                surface_len: word.surface_len,
            });
        }
    }

    /// Number of words of the last lookup.
    pub fn len(&self) -> usize {
        self.hits.len()
    }

    /// Whether the last lookup found nothing.
    pub fn is_empty(&self) -> bool {
        self.hits.is_empty()
    }

    /// Result `index` of the last lookup.
    pub fn row(&self, index: usize) -> Option<JapaneseRow> {
        self.hits.get(index).map(|hit| hit.row)
    }

    /// Written form of result `index`; `""` when out of range.
    pub fn text(&self, index: usize) -> &str {
        self.spans.get(index).and_then(|&(start, end)| self.texts.get(start..end)).unwrap_or("")
    }

    /// Hiragana of the whole input of the last lookup; `""` when it did not convert completely.
    pub fn kana(&self) -> &str {
        &self.kana
    }

    /// Katakana of the whole input of the last lookup; `""` when it did not convert completely.
    pub fn katakana(&self) -> &str {
        &self.katakana
    }
}

/// Sort rank of a result: reviewed first, then weight descending.
fn rank(row: &JapaneseRow) -> (bool, i32) {
    (row.flags & FLAG_REVIEWED == 0, -row.weight)
}
