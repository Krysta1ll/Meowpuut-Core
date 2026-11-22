//! Test writer for `MIMO-JN-1` images, following the byte layout in `src/japanese_next.rs`.
//! Used by `tests/japanese_next.rs` and by the ffi tests. The build writes images with
//! `scripts/mimo_jp_next.py`; this file exists so that the reader can be tested without it.
#![allow(dead_code)]

use std::collections::HashMap;

/// One key and the words that follow it, best first.
pub struct Key {
    pub text: String,
    pub flags: u8,
    pub next: Vec<(String, u16)>,
}

impl Key {
    pub fn new(text: &str, flags: u8, next: &[(&str, u16)]) -> Self {
        Self { text: text.to_owned(), flags, next: next.iter().map(|&(word, score)| (word.to_owned(), score)).collect() }
    }
}

fn align(length: usize) -> usize {
    (length + 7) & !7
}

/// Overwrites the u32 header field at byte `at`.
pub fn header_field(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

/// The image of `keys`, sorted by key bytes as the format requires.
pub fn image(keys: &[Key]) -> Vec<u8> {
    let mut order: Vec<&Key> = keys.iter().collect();
    order.sort_by(|a, b| a.text.as_bytes().cmp(b.text.as_bytes()));
    let mut blob: Vec<u8> = Vec::new();
    let mut offsets: HashMap<String, u32> = HashMap::new();
    let mut intern = |text: &str| -> (u32, u16) {
        let offset = *offsets.entry(text.to_owned()).or_insert_with(|| {
            let at = blob.len() as u32;
            blob.extend_from_slice(text.as_bytes());
            at
        });
        (offset, text.len() as u16)
    };
    let (mut key_records, mut next_records) = (Vec::new(), Vec::new());
    let mut first = 0u32;
    for key in &order {
        let (offset, len) = intern(&key.text);
        key_records.extend_from_slice(&offset.to_le_bytes());
        key_records.extend_from_slice(&len.to_le_bytes());
        key_records.push(key.next.len() as u8);
        key_records.push(key.flags);
        key_records.extend_from_slice(&first.to_le_bytes());
        for (word, score) in &key.next {
            let (offset, len) = intern(word);
            next_records.extend_from_slice(&offset.to_le_bytes());
            next_records.extend_from_slice(&len.to_le_bytes());
            next_records.extend_from_slice(&score.to_le_bytes());
        }
        first += key.next.len() as u32;
    }
    let keys_offset = 48;
    let nexts_offset = align(keys_offset + key_records.len());
    let blob_offset = align(nexts_offset + next_records.len());
    let file_len = align(blob_offset + blob.len());
    let mut bytes = vec![0u8; file_len];
    bytes[..12].copy_from_slice(b"MIMO-JN-1\0\0\0");
    let fields = [1, order.len() as u32, first, keys_offset as u32, nexts_offset as u32, blob_offset as u32, blob.len() as u32, file_len as u32, 0];
    for (index, field) in fields.iter().enumerate() {
        header_field(&mut bytes, 12 + 4 * index, *field);
    }
    bytes[keys_offset..keys_offset + key_records.len()].copy_from_slice(&key_records);
    bytes[nexts_offset..nexts_offset + next_records.len()].copy_from_slice(&next_records);
    bytes[blob_offset..blob_offset + blob.len()].copy_from_slice(&blob);
    bytes
}
