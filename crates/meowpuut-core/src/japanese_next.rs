#![forbid(unsafe_code)]
//! Reader for the Japanese next-word image `MIMO-JN-1` (`mimo_jp_next.bin`): what may follow
//! the text that was just committed (ありがとう → ございます, 今日 → は).
//!
//! The image is read in place: every access is a bounds-checked slice plus `from_le_bytes`.
//! Opening checks the header only. A damaged image can make a prediction return fewer rows; it
//! cannot panic or read outside the buffer.
//!
//! Layout (little-endian, offsets from the start of the file, sections 8-byte aligned):
//!
//! ```text
//! header, 48 bytes
//!    0  12 bytes  magic "MIMO-JN-1\0\0\0"
//!   12  u32 version = 1          16  u32 key_count        20  u32 next_count
//!   24  u32 keys_offset          28  u32 nexts_offset     32  u32 blob_offset
//!   36  u32 blob_len             40  u32 file_len         44  u32 reserved = 0
//! key record, 12 bytes, sorted by key bytes
//!   u32 key_offset (into the blob)  u16 key_len  u8 next_count  u8 flags  u32 first_next
//! next record, 8 bytes, grouped by key, best first
//!   u32 text_offset (into the blob)  u16 text_len  u16 score
//! blob: UTF-8 strings without terminators
//! ```
//!
//! A key is the text of one committed piece (今日), of two pieces in a row (ご飯を), or of the
//! ending of a piece (ました). Only a key with [`FLAG_TAIL`] may match as an ending.
//!
//! The context of a prediction is the committed pieces, oldest first, separated by
//! [`PIECE_SEPARATOR`]. Three lookups fill the result, in this order, each word once:
//! the last two pieces together, the last piece, and the longest [`FLAG_TAIL`] key the last
//! piece ends with.
//!
//! # Examples
//!
//! ```
//! use meowpuut_core::japanese_next::{JapaneseNext, NextQuery};
//!
//! assert!(JapaneseNext::from_vec(Vec::new()).is_none());
//!
//! // An image without any key: the header alone.
//! let mut image = Vec::new();
//! image.extend_from_slice(b"MIMO-JN-1\0\0\0");
//! for field in [1u32, 0, 0, 48, 48, 48, 0, 48, 0] {
//!     image.extend_from_slice(&field.to_le_bytes());
//! }
//! let next = JapaneseNext::from_vec(image).expect("valid header");
//! assert!(next.verify());
//! let mut query = NextQuery::new();
//! assert_eq!(query.predict(&next, "今日", 8), 0);
//! ```

use std::sync::Arc;

/// First 12 bytes of an image.
pub const MAGIC: &[u8; 12] = b"MIMO-JN-1\0\0\0";
/// Format version this reader accepts.
pub const VERSION: u32 = 1;
/// Header size in bytes.
pub const HEADER_LEN: usize = 48;
/// Key record size in bytes.
pub const KEY_RECORD_LEN: usize = 12;
/// Next record size in bytes.
pub const NEXT_RECORD_LEN: usize = 8;
/// Key flag: the key may match as the ending of a longer piece.
pub const FLAG_TAIL: u8 = 1;
/// Longest key or next text in bytes (the writer's limit; `verify` checks it).
pub const MAX_TEXT_BYTES: usize = 48;
/// Most next words under one key.
pub const MAX_NEXT_PER_KEY: usize = 12;
/// Separates committed pieces in a context (U+001F, unit separator).
pub const PIECE_SEPARATOR: char = '\u{1f}';
/// Most rows a query returns.
pub const MAX_RESULTS: usize = 32;
/// Characters of the last piece that are tried as an ending.
const MAX_TAIL_CHARS: usize = 8;

#[derive(Clone)]
enum Bytes {
    Static(&'static [u8]),
    Owned(Arc<[u8]>),
}

/// An opened image. Cheap to clone; safe to share between threads.
#[derive(Clone)]
pub struct JapaneseNext {
    bytes: Bytes,
    key_count: usize,
    next_count: usize,
    keys_offset: usize,
    nexts_offset: usize,
    blob_offset: usize,
    blob_len: usize,
}

struct KeyRecord {
    key_offset: u32,
    key_len: u16,
    next_count: usize,
    flags: u8,
    first_next: usize,
}

fn le_u16(bytes: &[u8], at: usize) -> Option<u16> {
    let field = bytes.get(at..at.checked_add(2)?)?;
    Some(u16::from_le_bytes(field.try_into().ok()?))
}

fn le_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let field = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes(field.try_into().ok()?))
}

impl JapaneseNext {
    /// Opens an image that the caller keeps alive, for example a mapped file.
    pub fn from_static(bytes: &'static [u8]) -> Option<Self> {
        Self::open(Bytes::Static(bytes))
    }

    /// Opens an image held on the heap.
    pub fn from_vec(bytes: Vec<u8>) -> Option<Self> {
        Self::open(Bytes::Owned(Arc::from(bytes)))
    }

    /// Checks the magic, the version, `file_len`, and that the three sections lie inside the
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
        let file_len = field(40)?;
        if file_len != u64::try_from(data.len()).ok()? || file_len >= 1 << 31 {
            return None;
        }
        // (offset, record count, record size) in file order.
        let sections = [
            (field(24)?, field(16)?, KEY_RECORD_LEN),
            (field(28)?, field(20)?, NEXT_RECORD_LEN),
            (field(32)?, field(36)?, 1),
        ];
        let mut spans = [(0u64, 0u64); 3];
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
        Some(Self {
            key_count: size(sections[0].1)?,
            next_count: size(sections[1].1)?,
            keys_offset: size(sections[0].0)?,
            nexts_offset: size(sections[1].0)?,
            blob_offset: size(sections[2].0)?,
            blob_len: size(sections[2].1)?,
            bytes,
        })
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

    /// Number of next records.
    pub fn next_count(&self) -> usize {
        self.next_count
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

    fn key_record(&self, index: usize) -> Option<KeyRecord> {
        if index >= self.key_count {
            return None;
        }
        let start = self.keys_offset.checked_add(index.checked_mul(KEY_RECORD_LEN)?)?;
        let record = self.data().get(start..start.checked_add(KEY_RECORD_LEN)?)?;
        Some(KeyRecord {
            key_offset: le_u32(record, 0)?,
            key_len: le_u16(record, 4)?,
            next_count: usize::from(*record.get(6)?),
            flags: *record.get(7)?,
            first_next: usize::try_from(le_u32(record, 8)?).ok()?,
        })
    }

    /// Text and score of next record `index`.
    fn next_record(&self, index: usize) -> Option<(&str, u16)> {
        if index >= self.next_count {
            return None;
        }
        let start = self.nexts_offset.checked_add(index.checked_mul(NEXT_RECORD_LEN)?)?;
        let record = self.data().get(start..start.checked_add(NEXT_RECORD_LEN)?)?;
        let text = std::str::from_utf8(self.blob(le_u32(record, 0)?, le_u16(record, 4)?)?).ok()?;
        Some((text, le_u16(record, 6)?))
    }

    /// The record of the key with exactly these bytes.
    fn find_key(&self, key: &[u8]) -> Option<KeyRecord> {
        let (mut low, mut high) = (0, self.key_count);
        while low < high {
            let middle = low + (high - low) / 2;
            let record = self.key_record(middle)?;
            match self.blob(record.key_offset, record.key_len)?.cmp(key) {
                std::cmp::Ordering::Less => low = middle + 1,
                std::cmp::Ordering::Greater => high = middle,
                std::cmp::Ordering::Equal => return Some(record),
            }
        }
        None
    }

    /// Reads every record: keys sorted and distinct, texts valid UTF-8 within the limits, every
    /// key's next records inside the table, no flag this reader does not know. For tests and the
    /// build; a client does not need it before `predict`.
    pub fn verify(&self) -> bool {
        self.verify_records().is_some()
    }

    fn verify_records(&self) -> Option<()> {
        let check = |ok: bool| ok.then_some(());
        let mut previous: Option<&[u8]> = None;
        let mut expected_first = 0usize;
        for index in 0..self.key_count {
            let record = self.key_record(index)?;
            let key = self.blob(record.key_offset, record.key_len)?;
            check(!key.is_empty() && key.len() <= MAX_TEXT_BYTES && std::str::from_utf8(key).is_ok())?;
            check(previous.is_none_or(|before| before < key))?;
            previous = Some(key);
            check(record.flags & !FLAG_TAIL == 0)?;
            check((1..=MAX_NEXT_PER_KEY).contains(&record.next_count) && record.first_next == expected_first)?;
            for slot in 0..record.next_count {
                let (text, score) = self.next_record(record.first_next.checked_add(slot)?)?;
                check(!text.is_empty() && text.len() <= MAX_TEXT_BYTES && score > 0)?;
            }
            expected_first = expected_first.checked_add(record.next_count)?;
        }
        check(expected_first == self.next_count)
    }
}

/// Buffers of one prediction. Reused between calls: no allocation once they have grown.
pub struct NextQuery {
    texts: String,
    spans: Vec<(usize, usize)>,
    scores: Vec<u16>,
    key: String,
}

impl Default for NextQuery {
    fn default() -> Self {
        Self::new()
    }
}

impl NextQuery {
    /// Empty buffers, reserved for a full result.
    pub fn new() -> Self {
        Self {
            texts: String::with_capacity(MAX_RESULTS * 16),
            spans: Vec::with_capacity(MAX_RESULTS),
            scores: Vec::with_capacity(MAX_RESULTS),
            key: String::with_capacity(2 * MAX_TEXT_BYTES),
        }
    }

    /// Fills the result with up to `limit` (at most [`MAX_RESULTS`]) words that may follow
    /// `context`, best first, and returns their number. `context` is the committed pieces,
    /// oldest first, separated by [`PIECE_SEPARATOR`]; only the last two are read.
    pub fn predict(&mut self, next: &JapaneseNext, context: &str, limit: usize) -> usize {
        self.texts.clear();
        self.spans.clear();
        self.scores.clear();
        let limit = limit.min(MAX_RESULTS);
        let mut pieces = context.rsplit(PIECE_SEPARATOR).filter(|piece| !piece.is_empty());
        let Some(last) = pieces.next() else { return 0 };
        if limit == 0 {
            return 0;
        }
        if let Some(before) = pieces.next() {
            if before.len() + last.len() <= MAX_TEXT_BYTES {
                self.key.clear();
                self.key.push_str(before);
                self.key.push_str(last);
                if let Some(record) = next.find_key(self.key.as_bytes()) {
                    self.take(next, &record, limit);
                }
            }
        }
        if last.len() <= MAX_TEXT_BYTES {
            if let Some(record) = next.find_key(last.as_bytes()) {
                self.take(next, &record, limit);
            }
        }
        // The longest ending first: ました before た.
        let starts = last.char_indices().map(|(at, _)| at).skip(1);
        let skipped = last.chars().count().saturating_sub(MAX_TAIL_CHARS + 1);
        for start in starts.skip(skipped) {
            if self.spans.len() >= limit {
                break;
            }
            let Some(tail) = last.get(start..) else { continue };
            if let Some(record) = next.find_key(tail.as_bytes()) {
                if record.flags & FLAG_TAIL != 0 {
                    self.take(next, &record, limit);
                    break;
                }
            }
        }
        self.spans.len()
    }

    fn take(&mut self, next: &JapaneseNext, record: &KeyRecord, limit: usize) {
        for slot in 0..record.next_count.min(MAX_NEXT_PER_KEY) {
            if self.spans.len() >= limit {
                return;
            }
            let Some((text, score)) = record.first_next.checked_add(slot).and_then(|at| next.next_record(at)) else {
                return;
            };
            let seen = self.spans.iter().any(|&(start, end)| self.texts.get(start..end) == Some(text));
            if seen || text.is_empty() {
                continue;
            }
            self.spans.push((self.texts.len(), self.texts.len() + text.len()));
            self.texts.push_str(text);
            self.scores.push(score);
        }
    }

    /// Number of words of the last prediction.
    pub fn len(&self) -> usize {
        self.spans.len()
    }

    /// Whether the last prediction found nothing.
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    /// Word `index` of the last prediction; `""` when out of range.
    pub fn text(&self, index: usize) -> &str {
        self.spans.get(index).and_then(|&(start, end)| self.texts.get(start..end)).unwrap_or("")
    }

    /// Score of word `index` (how often it followed its key in the counted text); 0 when out of range.
    pub fn score(&self, index: usize) -> u16 {
        self.scores.get(index).copied().unwrap_or(0)
    }
}
