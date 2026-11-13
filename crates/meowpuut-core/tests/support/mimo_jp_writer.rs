//! Test writer for `MIMO-JP-1` images, following the byte layout in `src/japanese.rs`.
//! Used by `tests/japanese.rs` and by the ffi tests. The build writes images with
//! `scripts/mimo_jp_binary.py`; this file exists so that the reader can be tested without it.
//!
//! Source format of `tests/data/mimo_jp_fixture.tsv` (tab separated, `#` starts a comment):
//!
//! ```text
//! W  id  surface  class  flags              word; ids count up from 0
//! P  key  word id  weight  kind  flags      posting
//! R  reading  text  weight  flags           reviewed row
//! ```
#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};

/// Class names in the order of their index in a word record.
pub const CLASSES: [&str; 22] = [
    "n", "pn", "v1", "v5k", "v5g", "v5s", "v5t", "v5n", "v5b", "v5m", "v5r", "v5w", "vs", "vk", "v?", "adj-i",
    "adj-na", "adv", "adn", "conj", "int", "place",
];

pub struct Word {
    pub surface: String,
    pub class: u8,
    pub flags: u8,
}

pub struct Posting {
    pub key: String,
    pub word: u32,
    pub weight: i16,
    pub kind: u8,
    pub flags: u8,
}

pub struct Reviewed {
    pub reading: String,
    pub text: String,
    pub weight: i16,
    pub flags: u16,
}

#[derive(Default)]
pub struct Image {
    pub words: Vec<Word>,
    pub postings: Vec<Posting>,
    pub reviewed: Vec<Reviewed>,
    pub chinese_build: u32,
}

/// Offsets of the sections of a written image, for tests that damage one field.
pub struct Layout {
    pub keys: usize,
    pub postings: usize,
    pub words: usize,
    pub reviewed: usize,
    pub blob: usize,
}

impl Image {
    pub fn word(&mut self, surface: &str, flags: u8) -> u32 {
        self.words.push(Word { surface: surface.to_string(), class: 0, flags });
        u32::try_from(self.words.len() - 1).expect("word count")
    }

    pub fn posting(&mut self, key: &str, word: u32, weight: i16, kind: u8, flags: u8) {
        self.postings.push(Posting { key: key.to_string(), word, weight, kind, flags });
    }

    pub fn reviewed(&mut self, reading: &str, text: &str, weight: i16, flags: u16) {
        self.reviewed.push(Reviewed { reading: reading.to_string(), text: text.to_string(), weight, flags });
    }

    pub fn parse(source: &str) -> Image {
        let mut image = Image::default();
        for line in source.lines().filter(|line| !line.is_empty() && !line.starts_with('#')) {
            let fields: Vec<&str> = line.split('\t').collect();
            let number = |index: usize| -> i64 { fields[index].parse().unwrap_or_else(|_| panic!("number in {line:?}")) };
            match (fields[0], fields.len()) {
                ("W", 5) => {
                    assert_eq!(number(1), image.words.len() as i64, "word ids count up from 0: {line:?}");
                    let class = CLASSES.iter().position(|&name| name == fields[3]).expect("class name");
                    image.words.push(Word { surface: fields[2].to_string(), class: class as u8, flags: number(4) as u8 });
                }
                ("P", 6) => image.posting(fields[1], number(2) as u32, number(3) as i16, number(4) as u8, number(5) as u8),
                ("R", 5) => image.reviewed(fields[1], fields[2], number(3) as i16, number(4) as u16),
                _ => panic!("fixture line {line:?}"),
            }
        }
        image
    }

    pub fn write(&self) -> Vec<u8> {
        self.write_with_layout().0
    }

    pub fn write_with_layout(&self) -> (Vec<u8>, Layout) {
        // Keys in byte order; inside a key: reviewed first, weight descending, surface.
        let mut by_key: BTreeMap<&str, Vec<&Posting>> = BTreeMap::new();
        for posting in &self.postings {
            assert!((1..=10_000).contains(&posting.weight) && posting.kind <= 4 && posting.flags <= 7);
            by_key.entry(&posting.key).or_default().push(posting);
        }
        let surface = |posting: &Posting| self.words[posting.word as usize].surface.as_str();
        for (key, list) in &mut by_key {
            assert!((1..=96).contains(&key.len()), "key length: {key:?}");
            assert!(list.len() <= 64, "more than 64 postings under {key:?}");
            list.sort_by_key(|posting| (posting.flags & 2 == 0, -posting.weight, surface(posting)));
            for pair in list.windows(2) {
                assert_ne!(surface(pair[0]), surface(pair[1]), "surface twice under {key:?}");
            }
        }
        let mut reviewed: Vec<&Reviewed> = self.reviewed.iter().collect();
        reviewed.sort_by_key(|row| row.reading.as_bytes());
        assert!(reviewed.len() <= 4096);

        // Blob part 1: the key strings, back to back in key order.
        let mut blob: Vec<u8> = Vec::new();
        let mut key_records: Vec<u8> = Vec::new();
        let mut posting_records: Vec<u8> = Vec::new();
        let mut first_posting = 0u32;
        for (key, list) in &by_key {
            key_records.extend_from_slice(&(blob.len() as u32).to_le_bytes());
            key_records.extend_from_slice(&(key.len() as u16).to_le_bytes());
            key_records.extend_from_slice(&(list.len() as u16).to_le_bytes());
            key_records.extend_from_slice(&first_posting.to_le_bytes());
            blob.extend_from_slice(key.as_bytes());
            for posting in list {
                posting_records.extend_from_slice(&posting.word.to_le_bytes());
                posting_records.extend_from_slice(&posting.weight.to_le_bytes());
                posting_records.push(posting.kind);
                posting_records.push(posting.flags);
            }
            first_posting += list.len() as u32;
        }
        // Blob part 2: surfaces, then reviewed readings and texts; each distinct string once.
        let mut offsets: HashMap<String, u32> = HashMap::new();
        let mut place = |blob: &mut Vec<u8>, text: &str| -> u32 {
            if let Some(&offset) = offsets.get(text) {
                return offset;
            }
            let offset = blob.len() as u32;
            blob.extend_from_slice(text.as_bytes());
            offsets.insert(text.to_string(), offset);
            offset
        };
        let mut word_records: Vec<u8> = Vec::new();
        for word in &self.words {
            assert!((1..=128).contains(&word.surface.len()), "surface length: {:?}", word.surface);
            word_records.extend_from_slice(&place(&mut blob, &word.surface).to_le_bytes());
            word_records.extend_from_slice(&(word.surface.len() as u16).to_le_bytes());
            word_records.push(word.class);
            word_records.push(word.flags);
        }
        let mut reviewed_records: Vec<u8> = Vec::new();
        for row in &reviewed {
            let reading_offset = place(&mut blob, &row.reading);
            let text_offset = place(&mut blob, &row.text);
            reviewed_records.extend_from_slice(&reading_offset.to_le_bytes());
            reviewed_records.extend_from_slice(&(row.reading.len() as u16).to_le_bytes());
            reviewed_records.extend_from_slice(&(row.text.len() as u16).to_le_bytes());
            reviewed_records.extend_from_slice(&text_offset.to_le_bytes());
            reviewed_records.extend_from_slice(&row.weight.to_le_bytes());
            reviewed_records.extend_from_slice(&row.flags.to_le_bytes());
        }

        // Header, then the sections in header order, each on an 8-byte boundary.
        let align = |length: usize| length.div_ceil(8) * 8;
        let keys = 64;
        let postings = align(keys + key_records.len());
        let words = align(postings + posting_records.len());
        let reviewed_at = align(words + word_records.len());
        let blob_at = align(reviewed_at + reviewed_records.len());
        let file_len = blob_at + blob.len();
        assert!(file_len < 1 << 31);
        let mut out = Vec::with_capacity(file_len);
        out.extend_from_slice(b"MIMO-JP-1\0\0\0");
        let header = [
            1,
            by_key.len(),
            self.postings.len(),
            self.words.len(),
            reviewed.len(),
            keys,
            postings,
            words,
            reviewed_at,
            blob_at,
            blob.len(),
            file_len,
        ];
        for field in header {
            out.extend_from_slice(&(field as u32).to_le_bytes());
        }
        out.extend_from_slice(&self.chinese_build.to_le_bytes());
        for (start, section) in
            [(keys, &key_records), (postings, &posting_records), (words, &word_records), (reviewed_at, &reviewed_records), (blob_at, &blob)]
        {
            out.resize(start, 0);
            out.extend_from_slice(section);
        }
        assert_eq!(out.len(), file_len);
        (out, Layout { keys, postings, words, reviewed: reviewed_at, blob: blob_at })
    }
}
