#![forbid(unsafe_code)]
//! Stack-allocated phonetic reading parser and UTF-8 Han character utilities
//! with zero heap allocations.
//!
//! # Examples
//!
//! ```
//! use meowpuut_core::reading::{Reading, han_characters_stack, is_tone, zhuyin_symbol_for_key};
//!
//! let reading = Reading::new("ni3hao3", true);
//! assert_eq!(reading.sounds(), "nihao");
//! assert!(Reading::new("nihao", true).unambiguous);
//! assert!(reading.matches(b"ni3hao3", 0, 5));
//! assert!(!reading.matches(b"ni2hao3", 0, 5));
//!
//! assert!(is_tone(b'3', true));
//! assert!(is_tone(b' ', false));
//! assert_eq!(zhuyin_symbol_for_key(b's'), Some("ㄋ"));
//!
//! let han = han_characters_stack("你好世界").expect("all Han");
//! assert_eq!(han.len(), 4);
//! assert_eq!(han.slice("你好世界", 0), "你");
//! assert!(han_characters_stack("你好!").is_none());
//! ```

pub const TONES: &[u8] = b" 6347";
pub const KEYS: &[u8] = b"1qaz2wsxedcrfv5tgbyhnujm8ik,9ol.0p;/- 6347'";
pub const SYMBOLS: [&str; 43] = [
    "ㄅ", "ㄆ", "ㄇ", "ㄈ", "ㄉ", "ㄊ", "ㄋ", "ㄌ", "ㄍ", "ㄎ", "ㄏ", "ㄐ", "ㄑ", "ㄒ", "ㄓ", "ㄔ",
    "ㄕ", "ㄖ", "ㄗ", "ㄘ", "ㄙ", "ㄧ", "ㄨ", "ㄩ", "ㄚ", "ㄛ", "ㄜ", "ㄝ", "ㄞ", "ㄟ", "ㄠ", "ㄡ",
    "ㄢ", "ㄣ", "ㄤ", "ㄥ", "ㄦ", "ˉ", "ˊ", "ˇ", "ˋ", "˙", " ",
];

pub const PAGE_SIZE: usize = 7;
pub const MAX_INPUT: usize = 256;
pub const CANDIDATE_LIMIT: usize = 256;
pub const BEAM: usize = 5;

/// Checks whether an ASCII key byte is a tone marker for Pinyin or Zhuyin.
#[inline]
pub fn is_tone(key: u8, pinyin: bool) -> bool {
    if pinyin {
        key >= b'1' && key <= b'5'
    } else {
        TONES.contains(&key)
    }
}

/// Maps a Da-Chien Zhuyin ASCII key to its Bopomofo symbol.
#[inline]
pub fn zhuyin_symbol_for_key(key: u8) -> Option<&'static str> {
    KEYS.iter().position(|&k| k == key).map(|idx| SYMBOLS[idx])
}

/// Counts UTF-8 codepoints by counting leading bytes (`(b & 0xc0) != 0x80`).
#[inline]
pub fn utf8_char_count(text: &str) -> usize {
    text.bytes().filter(|&b| (b & 0xc0) != 0x80).count()
}

/// Checks if a Unicode scalar value is within the supported CJK Han ranges.
#[inline]
pub fn is_han_scalar(scalar: u32) -> bool {
    (0x3400..=0x9fff).contains(&scalar)
        || (0xf900..=0xfaff).contains(&scalar)
        || (0x20000..=0x323af).contains(&scalar)
}

/// Stack-allocated UTF-8 byte ranges for up to 64 Han characters.
#[derive(Debug, Clone, Copy)]
pub struct HanSliceStack {
    ranges: [(u16, u16); 64],
    len: usize,
}

impl HanSliceStack {
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    pub fn slice<'a>(&self, text: &'a str, idx: usize) -> &'a str {
        let (start, end) = self.ranges[idx];
        &text[start as usize..end as usize]
    }
}

/// Parses `text` into Han character slices on the stack without heap allocation.
/// Returns `None` if `text` is empty or contains any non-Han character.
pub fn han_characters_stack(text: &str) -> Option<HanSliceStack> {
    if text.is_empty() {
        return None;
    }
    let mut result = HanSliceStack {
        ranges: [(0, 0); 64],
        len: 0,
    };
    for (start, ch) in text.char_indices() {
        let scalar = ch as u32;
        if !is_han_scalar(scalar) || result.len >= 64 {
            return None;
        }
        let end = start + ch.len_utf8();
        result.ranges[result.len] = (start as u16, end as u16);
        result.len += 1;
    }
    Some(result)
}

/// Returns the number of Han characters if and only if every character in `text` is Han.
#[inline]
pub fn strict_han_count(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    let mut count = 0usize;
    for ch in text.chars() {
        if !is_han_scalar(ch as u32) {
            return 0;
        }
        count += 1;
    }
    count
}

/// Checks whether a Pinyin word reading has at least one vowel in every toned syllable.
#[inline]
pub fn is_fully_spelled_reading(reading: &str) -> bool {
    let mut vowel = false;
    for b in reading.bytes() {
        if is_tone(b, true) {
            if !vowel {
                return false;
            }
            vowel = false;
        } else if b"aeiouv".contains(&b) {
            vowel = true;
        }
    }
    true
}

/// Stack-allocated reading representation (zero heap allocations).
#[derive(Debug, Clone)]
pub struct Reading {
    sounds_buf: [u8; MAX_INPUT + 1],
    sounds_len: usize,
    pub boundaries: [u8; MAX_INPUT + 1],
    pub boundary_count: [u16; MAX_INPUT + 1],
    pub pinyin: bool,
    pub unambiguous: bool,
}

impl Reading {
    /// Parses `input` into untoned sound bytes and tone/apostrophe boundary constraints on the stack.
    #[inline]
    pub fn new(input: &str, latin: bool) -> Self {
        let mut r = Self {
            sounds_buf: [0u8; MAX_INPUT + 1],
            sounds_len: 0,
            boundaries: [0u8; MAX_INPUT + 1],
            boundary_count: [0u16; MAX_INPUT + 1],
            pinyin: latin,
            unambiguous: true,
        };
        for key in input.bytes() {
            if is_tone(key, r.pinyin) || key == b'\'' {
                if r.sounds_len == 0 || r.boundaries[r.sounds_len] != 0 {
                    r.unambiguous = false;
                }
                r.boundaries[r.sounds_len] = key;
            } else if r.sounds_len < MAX_INPUT {
                r.sounds_buf[r.sounds_len] = key;
                r.sounds_len += 1;
            }
        }
        for i in 1..=r.sounds_len {
            r.boundary_count[i] = r.boundary_count[i - 1] + u16::from(r.boundaries[i] != 0);
        }
        r
    }

    #[inline]
    pub fn sounds_bytes(&self) -> &[u8] {
        &self.sounds_buf[..self.sounds_len]
    }

    #[inline]
    pub fn sounds(&self) -> &str {
        std::str::from_utf8(&self.sounds_buf[..self.sounds_len]).unwrap_or("")
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.sounds_len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.sounds_len == 0
    }

    /// Checks whether a word's toned reading matches the tone/separator boundaries in `[start, end]`.
    #[inline]
    pub fn matches(&self, word_reading: &[u8], mut start: usize, end: usize) -> bool {
        let required = self.boundary_count[end] - self.boundary_count[start];
        if required == 0 {
            return true;
        }
        let mut matched = 0u16;
        for &key in word_reading {
            if !is_tone(key, self.pinyin) {
                start += 1;
                continue;
            }
            let tone = self.boundaries[start];
            if tone != 0 && tone != key && tone != b'\'' {
                return false;
            }
            matched += u16::from(tone != 0);
        }
        matched == required
    }
}
