#![forbid(unsafe_code)]
//! Binary lexicon format: `MIMO-BIN-1` images (SoA columns, `i16` weights)
//! and the `MIMO-BASE-1` container `mimo_base.bin` that holds one image per mode.
//!
//! Container layout (little-endian `u32` fields):
//!
//! | offset | field |
//! |---|---|
//! | 0 | magic `MIMO-BASE-1\0` |
//! | 12 | version: 1, or 2 when index sections follow |
//! | 16, 20 | pinyin image offset, length |
//! | 24, 28 | zhuyin image offset, length |
//! | 32, 36 | version 2: pinyin `MIMO-IDX-1` offset, length |
//! | 40, 44 | version 2: zhuyin `MIMO-IDX-1` offset, length |
//!
//! A `MIMO-IDX-1` section holds what the loader used to build on the heap:
//! the two hash indexes ([`crate::context_index::PhraseIndex`]), the two
//! character conversion tables and the initials lists
//! ([`crate::lexicon::LexiconIndex`]). Its 96-byte header (little-endian):
//!
//! | offset | field |
//! |---|---|
//! | 0 | magic `MIMO-IDX-1\0\0` |
//! | 12 | image length |
//! | 16, 20, 24 | bucket index: capacity, slots offset, tags offset |
//! | 28, 32, 36 | context index: capacity, slots offset, tags offset |
//! | 40, 44 | simplified-to-traditional table: count, offset |
//! | 48, 52 | traditional-to-simplified table: count, offset |
//! | 56, 60 | initials: word count, offset |
//! | 64 | `u64` [`image_fingerprint`] of the image |
//! | 72, 76 | longest probe distance of the bucket and the context index |
//! | 80, 84 | word ids: offset, number of ids (0 = none) |
//! | 88 | `u64` fingerprint of the word id list |
//!
//! Offsets are from the start of the container, so both modes can point at
//! one copy of the context index (the context table is the same in both
//! images). With a version 1 file the index is built on the heap at load, as
//! before.
//!
//! [`Lexicon::from_base_in_place`] reads columns and index from the caller's
//! bytes without copying them; [`Lexicon::load_from_base`] copies them once.
//!
//! # Examples
//!
//! ```
//! use meowpuut_core::Lexicon;
//!
//! let tsv = concat!(
//!     "MEOWPUUT-PINYIN-1\n",
//!     "hao\thao3\t好\t好\t4800\n",
//!     "ni\tni3\t你\t你\t5000\n",
//!     "nihao\tni3hao3\t你好\t你好\t5400\n",
//!     "# context\n",
//!     "你好\t600\n",
//! );
//! let lex = Lexicon::load(tsv).expect("valid tsv");
//! let bin = lex.to_binary();
//! let loaded = Lexicon::load_bytes(&bin).expect("valid MIMO-BIN-1");
//! assert_eq!(loaded.words.len(), 3);
//! assert_eq!(loaded.range("nihao"), (2, 3));
//! assert_eq!(loaded.context_frequency("你好".as_bytes()), Some(600));
//!
//! let zhuyin_tsv = "MEOWPUUT-ZHUYIN-1\nsu\tsu3\t你\t你\t5000\n";
//! let zhuyin_bin = Lexicon::load(zhuyin_tsv).unwrap().to_binary();
//! let base = Lexicon::pack_base(&bin, &zhuyin_bin);
//! let py_from_base = Lexicon::load_from_base(&base, true).unwrap();
//! let zy_from_base = Lexicon::load_from_base(&base, false).unwrap();
//! assert!(py_from_base.pinyin);
//! assert!(!zy_from_base.pinyin);
//! ```

use crate::context_index::{phrase_hash, PhraseIndex};
use crate::image::{Bytes, I16Col, ImageOwner, Text, U16Col, U32Col, U8Col};
use crate::lexicon::{CharTable, Initials, Lexicon, LexiconIndex, WordIds};
use crate::soa::{BucketTableSoA, ContextTableSoA, WordTableSoA};

/// Magic header for a single-mode `MIMO-BIN-1` SoA lexicon image (12 bytes).
pub const MIMO_BIN_MAGIC: &[u8; 12] = b"MIMO-BIN-1\0\0";

/// Magic header for a dual-mode `MIMO-BASE-1` (`mimo_base.bin`) container (12 bytes).
pub const MIMO_BASE_MAGIC: &[u8; 12] = b"MIMO-BASE-1\0";

/// Magic header for a `MIMO-IDX-1` index section (12 bytes).
pub const MIMO_IDX_MAGIC: &[u8; 12] = b"MIMO-IDX-1\0\0";

/// Container version written by [`Lexicon::pack_base`].
pub const MIMO_BASE_VERSION: u32 = 2;

const HEADER_LEN: usize = 32;
const BASE_HEADER_LEN: usize = 64;
const IDX_HEADER_LEN: usize = 96;
const WORD_ROW_BYTES: usize = 21;
const BUCKET_ROW_BYTES: usize = 14;
const CONTEXT_ROW_BYTES: usize = 8;

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset.checked_add(4)?)?;
    let mut arr = [0u8; 4];
    arr.copy_from_slice(slice);
    Some(u32::from_le_bytes(arr))
}

fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from(read_u32(bytes, offset)?) | (u64::from(read_u32(bytes, offset + 4)?) << 32))
}

/// Fingerprint that ties an index section to its image: the hash of 64
/// evenly spaced 64-byte pieces of the image (the whole image when it is
/// 4 KB or smaller). Cheap at load; it catches an index written for another
/// build of the image. Damaged bytes are caught by the offset checks.
pub fn image_fingerprint(image: &[u8]) -> u64 {
    if image.len() <= 4096 {
        return phrase_hash(image);
    }
    let mut sample = [0u8; 4096];
    let last = image.len() - 64;
    for k in 0..64 {
        let at = last * k / 63;
        sample[k * 64..k * 64 + 64].copy_from_slice(&image[at..at + 64]);
    }
    phrase_hash(&sample)
}

/// Splits `count * width` bytes off the front of `cursor`.
fn take<'a>(cursor: &mut &'a [u8], count: usize, width: usize) -> Option<&'a [u8]> {
    let len = count.checked_mul(width)?;
    let section = cursor.get(..len)?;
    *cursor = &cursor[len..];
    Some(section)
}

/// How sections become columns: borrowed in place or copied to the heap.
struct Source<'a> {
    bytes: &'a dyn Fn(&'a [u8]) -> Bytes,
    text: &'a dyn Fn(&'a str) -> Text,
}

struct Tables {
    storage: Text,
    words: WordTableSoA,
    buckets: BucketTableSoA,
    context: ContextTableSoA,
    longest: usize,
    pinyin: bool,
}

/// Parses and validates a `MIMO-BIN-1` image. Every offset is checked, so a
/// damaged file is rejected here instead of failing while typing.
fn parse_image<'a>(bytes: &'a [u8], src: &Source<'a>) -> Option<Tables> {
    if bytes.len() < HEADER_LEN || &bytes[..12] != MIMO_BIN_MAGIC {
        return None;
    }
    let meta = read_u32(bytes, 12)?;
    let pinyin = (meta & 1) != 0;
    let longest = ((meta >> 8) & 0xFF) as usize;
    let storage_len = read_u32(bytes, 16)? as usize;
    let words_len = read_u32(bytes, 20)? as usize;
    let buckets_len = read_u32(bytes, 24)? as usize;
    let context_len = read_u32(bytes, 28)? as usize;
    if words_len == 0 || buckets_len == 0 {
        return None;
    }
    let expected_len = HEADER_LEN
        .checked_add(storage_len)?
        .checked_add(words_len.checked_mul(WORD_ROW_BYTES)?)?
        .checked_add(buckets_len.checked_mul(BUCKET_ROW_BYTES)?)?
        .checked_add(context_len.checked_mul(CONTEXT_ROW_BYTES)?)?;
    if bytes.len() != expected_len {
        return None;
    }

    let mut cursor = &bytes[HEADER_LEN..];
    let storage_str = std::str::from_utf8(take(&mut cursor, storage_len, 1)?).ok()?;
    let col = |cursor: &mut &'a [u8], count: usize, width: usize| take(cursor, count, width).map(src.bytes);
    let words = WordTableSoA {
        reading_offset: U32Col::new(col(&mut cursor, words_len, 4)?),
        reading_len: U16Col::new(col(&mut cursor, words_len, 2)?),
        hant_offset: U32Col::new(col(&mut cursor, words_len, 4)?),
        hant_len: U16Col::new(col(&mut cursor, words_len, 2)?),
        hans_offset: U32Col::new(col(&mut cursor, words_len, 4)?),
        hans_len: U16Col::new(col(&mut cursor, words_len, 2)?),
        weight: I16Col::new(col(&mut cursor, words_len, 2)?),
        flags: U8Col::new(col(&mut cursor, words_len, 1)?),
    };
    let buckets = BucketTableSoA {
        key_offset: U32Col::new(col(&mut cursor, buckets_len, 4)?),
        key_len: U16Col::new(col(&mut cursor, buckets_len, 2)?),
        word_start: U32Col::new(col(&mut cursor, buckets_len, 4)?),
        word_end: U32Col::new(col(&mut cursor, buckets_len, 4)?),
    };
    let context = ContextTableSoA {
        phrase_offset: U32Col::new(col(&mut cursor, context_len, 4)?),
        phrase_len: U16Col::new(col(&mut cursor, context_len, 2)?),
        frequency: I16Col::new(col(&mut cursor, context_len, 2)?),
    };

    let in_storage = |start: u32, len: u16, allow_empty: bool| {
        let s = start as usize;
        (allow_empty || len != 0) && s.checked_add(len as usize).and_then(|e| storage_str.get(s..e)).is_some()
    };
    for i in 0..words_len {
        if !in_storage(words.reading_offset.get(i), words.reading_len.get(i), false)
            || !in_storage(words.hant_offset.get(i), words.hant_len.get(i), true)
            || !in_storage(words.hans_offset.get(i), words.hans_len.get(i), true)
        {
            return None;
        }
    }
    for i in 0..buckets_len {
        let (ws, we) = buckets.range(i);
        if !in_storage(buckets.key_offset.get(i), buckets.key_len.get(i), false) || ws >= we || we > words_len {
            return None;
        }
    }
    for i in 0..context_len {
        if !in_storage(context.phrase_offset.get(i), context.phrase_len.get(i), false) {
            return None;
        }
    }
    Some(Tables { storage: (src.text)(storage_str), words, buckets, context, longest, pinyin })
}

/// Parses a `MIMO-IDX-1` section at `offset` of `base` for `image`, which
/// holds `tables`. `None` if anything does not fit or the section belongs to
/// another image.
fn parse_index<'a>(base: &'a [u8], offset: usize, image: &[u8], tables: &Tables, src: &Source<'a>) -> Option<LexiconIndex> {
    let header = base.get(offset..offset.checked_add(IDX_HEADER_LEN)?)?;
    if &header[..12] != MIMO_IDX_MAGIC
        || read_u32(header, 12)? as usize != image.len()
        || read_u64(header, 64)? != image_fingerprint(image)
    {
        return None;
    }
    let field = |at: usize| read_u32(header, at).map(|v| v as usize);
    let section = |at: usize, len: usize| -> Option<Bytes> { base.get(at..at.checked_add(len)?).map(src.bytes) };
    let words_len = tables.words.len();

    let hash = |cap_at: usize, rows: usize, probe_at: usize| -> Option<PhraseIndex> {
        let cap = field(cap_at)?;
        let slots = U32Col::new(section(field(cap_at + 4)?, cap.checked_mul(4)?)?);
        let tags = U16Col::new(section(field(cap_at + 8)?, cap.checked_mul(2)?)?);
        PhraseIndex::from_columns(rows, slots, tags, field(probe_at)?)
    };
    let bucket = hash(16, tables.buckets.len(), 72)?;
    let context = hash(28, tables.context.len(), 76)?;

    let chars = |count_at: usize| -> Option<CharTable> {
        let (n, at) = (field(count_at)?, field(count_at + 4)?);
        let keys = U32Col::new(section(at, n.checked_mul(4)?)?);
        let words = U32Col::new(section(at.checked_add(n * 4)?, n * 4)?);
        CharTable::from_columns(keys, words, words_len)
    };
    let traditional = chars(40)?;
    let simplified = chars(48)?;

    let (n, at) = (field(56)?, field(60)?);
    let starts = U32Col::new(section(at, 129 * 4)?);
    let words = U32Col::new(section(at.checked_add(129 * 4)?, n.checked_mul(4)?)?);
    let initials = Initials::from_columns(starts, words, words_len)?;
    let count = read_u32(header, 84)?;
    let word_ids = if count == 0 {
        WordIds::default()
    } else {
        WordIds { ids: U32Col::new(section(field(80)?, words_len.checked_mul(4)?)?), count, fingerprint: read_u64(header, 88)? }
    };
    Some(LexiconIndex { bucket, context, traditional, simplified, initials, word_ids })
}

/// Reads one mode from a `MIMO-BASE-1` container (version 1 or 2).
fn parse_base<'a>(base: &'a [u8], pinyin: bool, src: &Source<'a>) -> Option<(Tables, Option<LexiconIndex>)> {
    if base.len() < HEADER_LEN || &base[..12] != MIMO_BASE_MAGIC {
        return None;
    }
    let version = read_u32(base, 12)?;
    if version != 1 && version != 2 {
        return None;
    }
    let field = |at: usize| read_u32(base, at).map(|v| v as usize);
    let (offset, len) = if pinyin { (field(16)?, field(20)?) } else { (field(24)?, field(28)?) };
    let image = base.get(offset..offset.checked_add(len)?)?;
    let tables = parse_image(image, src)?;
    let index = if version == 2 {
        let idx_offset = if pinyin { field(32)? } else { field(40)? };
        Some(parse_index(base, idx_offset, image, &tables, src)?)
    } else {
        None
    };
    Some((tables, index))
}

fn copied(bytes: &[u8]) -> Bytes {
    Bytes::owned(bytes.to_vec())
}

fn copied_text(text: &str) -> Text {
    Text::owned(text.to_owned())
}

fn assemble(tables: Tables, index: Option<LexiconIndex>, owner: Option<ImageOwner>) -> Option<Lexicon> {
    let Tables { storage, words, buckets, context, longest, pinyin } = tables;
    Lexicon::assemble(storage, words, buckets, context, longest, pinyin, index, owner)
}

fn pad_to(out: &mut Vec<u8>, align: usize) {
    out.resize(out.len().div_ceil(align) * align, 0);
}

/// Index section bytes laid out at `start` (64-aligned) of the container.
/// `shared_context` is the (slots, tags) offset pair of an identical context
/// index already written, which is then referenced instead of repeated.
fn write_index(out: &mut Vec<u8>, image: &[u8], index: &LexiconIndex, shared_context: Option<(u32, u32)>) -> (u32, u32) {
    let start = out.len();
    out.resize(start + IDX_HEADER_LEN, 0);
    let mut fields: Vec<u32> = Vec::with_capacity(20);
    let put = |out: &mut Vec<u8>, parts: &[&[u8]]| -> u32 {
        pad_to(out, 8);
        let at = out.len() as u32;
        for part in parts {
            out.extend_from_slice(part);
        }
        at
    };
    let (slots, tags) = index.bucket.columns();
    fields.push(slots.len() as u32);
    fields.push(put(out, &[slots.as_bytes()]));
    fields.push(put(out, &[tags.as_bytes()]));
    let (slots, tags) = index.context.columns();
    fields.push(slots.len() as u32);
    let context_at = match shared_context {
        Some(pair) => pair,
        None => (put(out, &[slots.as_bytes()]), put(out, &[tags.as_bytes()])),
    };
    fields.push(context_at.0);
    fields.push(context_at.1);
    for table in [&index.traditional, &index.simplified] {
        let (keys, words) = table.columns();
        fields.push(keys.len() as u32);
        fields.push(put(out, &[keys.as_bytes(), words.as_bytes()]));
    }
    let (starts, words) = index.initials.columns();
    fields.push(words.len() as u32);
    fields.push(put(out, &[starts.as_bytes(), words.as_bytes()]));
    let fingerprint = image_fingerprint(image);
    fields.push(fingerprint as u32);
    fields.push((fingerprint >> 32) as u32);
    fields.push(index.bucket.probe() as u32);
    fields.push(index.context.probe() as u32);
    let ids = &index.word_ids;
    fields.push(put(out, &[ids.ids.as_bytes()]));
    fields.push(ids.count);
    fields.push(ids.fingerprint as u32);
    fields.push((ids.fingerprint >> 32) as u32);
    let header = &mut out[start..start + IDX_HEADER_LEN];
    header[..12].copy_from_slice(MIMO_IDX_MAGIC);
    header[12..16].copy_from_slice(&(image.len() as u32).to_le_bytes());
    for (k, v) in fields.iter().enumerate() {
        header[16 + 4 * k..20 + 4 * k].copy_from_slice(&v.to_le_bytes());
    }
    context_at
}

impl Lexicon {
    /// Auto-detects `MIMO-BIN-1` binary format or `MEOWPUUT-*-1` UTF-8 TSV text.
    pub fn load_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(MIMO_BIN_MAGIC) {
            Self::load_binary(bytes)
        } else {
            let text = std::str::from_utf8(bytes).ok()?;
            Self::load(text)
        }
    }

    /// Serializes this `Lexicon` into a compact `MIMO-BIN-1` binary image with
    /// `i16` Log-Probability weights and precomputed `FLAG_VERIFIED_SPELLING` bits.
    pub fn to_binary(&self) -> Vec<u8> {
        let storage_len = self.storage.len();
        let words_len = self.words.len();
        let buckets_len = self.buckets.len();
        let context_len = self.context.len();
        let total = HEADER_LEN
            + storage_len
            + words_len * WORD_ROW_BYTES
            + buckets_len * BUCKET_ROW_BYTES
            + context_len * CONTEXT_ROW_BYTES;
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(MIMO_BIN_MAGIC);
        let meta = u32::from(self.pinyin) | ((self.longest.min(255) as u32) << 8);
        out.extend_from_slice(&meta.to_le_bytes());
        out.extend_from_slice(&(storage_len as u32).to_le_bytes());
        out.extend_from_slice(&(words_len as u32).to_le_bytes());
        out.extend_from_slice(&(buckets_len as u32).to_le_bytes());
        out.extend_from_slice(&(context_len as u32).to_le_bytes());
        out.extend_from_slice(self.storage.as_bytes());
        let w = &self.words;
        for col in [
            w.reading_offset.as_bytes(),
            w.reading_len.as_bytes(),
            w.hant_offset.as_bytes(),
            w.hant_len.as_bytes(),
            w.hans_offset.as_bytes(),
            w.hans_len.as_bytes(),
            w.weight.as_bytes(),
            w.flags.as_bytes(),
        ] {
            out.extend_from_slice(col);
        }
        let b = &self.buckets;
        for col in [b.key_offset.as_bytes(), b.key_len.as_bytes(), b.word_start.as_bytes(), b.word_end.as_bytes()] {
            out.extend_from_slice(col);
        }
        let c = &self.context;
        for col in [c.phrase_offset.as_bytes(), c.phrase_len.as_bytes(), c.frequency.as_bytes()] {
            out.extend_from_slice(col);
        }
        out
    }

    /// Loads a `MIMO-BIN-1` binary image, copying it to the heap and building the index there.
    pub fn load_binary(bytes: &[u8]) -> Option<Self> {
        let tables = parse_image(bytes, &Source { bytes: &copied, text: &copied_text })?;
        assemble(tables, None, None)
    }

    /// Packs Pinyin and Zhuyin `MIMO-BIN-1` images into a version 2 `MIMO-BASE-1`
    /// container (`mimo_base.bin`) with an index section per mode, word ids included. Returns an
    /// empty vector if either image does not load. `scripts/mimo_binary.py`
    /// writes the same bytes.
    pub fn pack_base(pinyin_bin: &[u8], zhuyin_bin: &[u8]) -> Vec<u8> {
        let (Some(mut py), Some(mut zy)) = (Self::load_binary(pinyin_bin), Self::load_binary(zhuyin_bin)) else {
            return Vec::new();
        };
        let [py_ids, zy_ids] = WordIds::number([(&py.storage, &py.words), (&zy.storage, &zy.words)]);
        py.index.word_ids = py_ids;
        zy.index.word_ids = zy_ids;
        let mut out = Self::pack_base_v1(pinyin_bin, zhuyin_bin);
        out[12..16].copy_from_slice(&MIMO_BASE_VERSION.to_le_bytes());
        pad_to(&mut out, 64);
        let py_idx = out.len();
        let context_at = write_index(&mut out, pinyin_bin, py.index(), None);
        let py_idx_len = out.len() - py_idx;
        pad_to(&mut out, 64);
        let zy_idx = out.len();
        let shared = (py.index().context.columns() == zy.index().context.columns()).then_some(context_at);
        write_index(&mut out, zhuyin_bin, zy.index(), shared);
        let zy_idx_len = out.len() - zy_idx;
        for (at, v) in [(32, py_idx), (36, py_idx_len), (40, zy_idx), (44, zy_idx_len)] {
            out[at..at + 4].copy_from_slice(&(v as u32).to_le_bytes());
        }
        out
    }

    /// Packs two images into a version 1 container (no index sections), the
    /// format written before version 2. Readers still accept it.
    pub fn pack_base_v1(pinyin_bin: &[u8], zhuyin_bin: &[u8]) -> Vec<u8> {
        let py_offset = BASE_HEADER_LEN;
        let py_len = pinyin_bin.len();
        let after_py = py_offset + py_len;
        let zy_offset = (after_py + 63) & !63;
        let zy_len = zhuyin_bin.len();
        let mut out = Vec::with_capacity(zy_offset + zy_len);
        out.extend_from_slice(MIMO_BASE_MAGIC);
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&(py_offset as u32).to_le_bytes());
        out.extend_from_slice(&(py_len as u32).to_le_bytes());
        out.extend_from_slice(&(zy_offset as u32).to_le_bytes());
        out.extend_from_slice(&(zy_len as u32).to_le_bytes());
        out.resize(py_offset, 0);
        out.extend_from_slice(pinyin_bin);
        out.resize(zy_offset, 0);
        out.extend_from_slice(zhuyin_bin);
        out
    }

    /// Loads the Pinyin or Zhuyin lexicon from a `MIMO-BASE-1` container,
    /// copying what it reads to the heap; `base_bytes` may be freed afterwards.
    /// A version 2 index section is copied as is, a version 1 file gets its
    /// index built.
    pub fn load_from_base(base_bytes: &[u8], pinyin: bool) -> Option<Self> {
        let (tables, index) = parse_base(base_bytes, pinyin, &Source { bytes: &copied, text: &copied_text })?;
        assemble(tables, index, None)
    }

    /// Loads the Pinyin or Zhuyin lexicon from a `MIMO-BASE-1` container
    /// without copying it: columns and index are read from `base_bytes`.
    /// `owner` is kept until the last clone of the lexicon is dropped; use it
    /// to unmap or free the bytes. With a version 1 file only the index is
    /// built on the heap (about 6 MiB for the shipped lexicon).
    pub fn from_base_in_place(base_bytes: &'static [u8], pinyin: bool, owner: Option<ImageOwner>) -> Option<Self> {
        let (tables, index) = parse_base(base_bytes, pinyin, &Source { bytes: &Bytes::mapped, text: &Text::mapped })?;
        assemble(tables, index, owner)
    }
}
