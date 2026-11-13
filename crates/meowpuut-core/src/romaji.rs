#![forbid(unsafe_code)]
//! Romaji to kana, and the kana key used by the Japanese lexicon.
//!
//! `to_kana` is a port of `Romaji.reading` in `Sources/JapaneseBackend.swift`: the same token
//! table and the same rules. Two tokens are added, `dhu` (でゅ) and `thu` (てゅ).
//! `tests/data/romaji_golden.tsv` holds the output of the Swift function.
//!
//! # Examples
//!
//! ```
//! use meowpuut_core::romaji::{normalise_key, to_kana, to_katakana};
//!
//! let mut kana = String::new();
//! let result = to_kana("ko-hi-", &mut kana);
//! assert_eq!(kana, "こーひー");
//! assert!(result.complete && result.strict);
//!
//! let mut katakana = String::new();
//! to_katakana(&kana, &mut katakana);
//! assert_eq!(katakana, "コーヒー");
//!
//! let mut key = String::new();
//! normalise_key(&katakana, &mut key);
//! assert_eq!(key, "こおひい");
//!
//! kana.clear();
//! let result = to_kana("syasin", &mut kana);
//! assert_eq!(kana, "しゃしん");
//! assert!(result.complete && !result.strict);
//! ```

/// What `to_kana` found out about the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KanaResult {
    /// No ASCII letter is left in the output.
    pub complete: bool,
    /// Only Hepburn tokens were used: no Kunrei spelling (`si`, `tya`, `zi`), no `x`/`l` small
    /// kana, and no character was left unconverted.
    pub strict: bool,
}

/// Token, kana, strict. Sorted by token bytes for the binary search in `token`.
const TABLE: &[(&str, &str, bool)] = &[
    ("a", "あ", true), ("ba", "ば", true), ("be", "べ", true), ("bi", "び", true), ("bo", "ぼ", true),
    ("bu", "ぶ", true), ("bya", "びゃ", true), ("bye", "びぇ", true), ("byo", "びょ", true),
    ("byu", "びゅ", true), ("cha", "ちゃ", true), ("che", "ちぇ", true), ("chi", "ち", true),
    ("cho", "ちょ", true), ("chu", "ちゅ", true), ("cya", "ちゃ", false), ("cyo", "ちょ", false),
    ("cyu", "ちゅ", false), ("da", "だ", true), ("de", "で", true), ("dhi", "でぃ", true),
    ("dhu", "でゅ", true), ("di", "ぢ", true), ("do", "ど", true), ("du", "づ", true),
    ("dwu", "どぅ", true), ("e", "え", true), ("fa", "ふぁ", true), ("fe", "ふぇ", true),
    ("fi", "ふぃ", true), ("fo", "ふぉ", true), ("fu", "ふ", true), ("fyu", "ふゅ", true),
    ("ga", "が", true), ("ge", "げ", true), ("gi", "ぎ", true), ("go", "ご", true), ("gu", "ぐ", true),
    ("gya", "ぎゃ", true), ("gye", "ぎぇ", true), ("gyo", "ぎょ", true), ("gyu", "ぎゅ", true),
    ("ha", "は", true), ("he", "へ", true), ("hi", "ひ", true), ("ho", "ほ", true), ("hu", "ふ", false),
    ("hya", "ひゃ", true), ("hye", "ひぇ", true), ("hyo", "ひょ", true), ("hyu", "ひゅ", true),
    ("i", "い", true), ("ja", "じゃ", true), ("je", "じぇ", true), ("ji", "じ", true), ("jo", "じょ", true),
    ("ju", "じゅ", true), ("jya", "じゃ", false), ("jyo", "じょ", false), ("jyu", "じゅ", false),
    ("ka", "か", true), ("ke", "け", true), ("ki", "き", true), ("ko", "こ", true), ("ku", "く", true),
    ("kya", "きゃ", true), ("kye", "きぇ", true), ("kyo", "きょ", true), ("kyu", "きゅ", true),
    ("la", "ぁ", false), ("le", "ぇ", false), ("li", "ぃ", false), ("lo", "ぉ", false),
    ("ltsu", "っ", false), ("ltu", "っ", false), ("lu", "ぅ", false), ("lwa", "ゎ", false),
    ("lya", "ゃ", false), ("lyo", "ょ", false), ("lyu", "ゅ", false), ("ma", "ま", true),
    ("me", "め", true), ("mi", "み", true), ("mo", "も", true), ("mu", "む", true), ("mya", "みゃ", true),
    ("mye", "みぇ", true), ("myo", "みょ", true), ("myu", "みゅ", true), ("na", "な", true),
    ("ne", "ね", true), ("ni", "に", true), ("no", "の", true), ("nu", "ぬ", true), ("nya", "にゃ", true),
    ("nye", "にぇ", true), ("nyo", "にょ", true), ("nyu", "にゅ", true), ("o", "お", true),
    ("pa", "ぱ", true), ("pe", "ぺ", true), ("pi", "ぴ", true), ("po", "ぽ", true), ("pu", "ぷ", true),
    ("pya", "ぴゃ", true), ("pye", "ぴぇ", true), ("pyo", "ぴょ", true), ("pyu", "ぴゅ", true),
    ("ra", "ら", true), ("re", "れ", true), ("ri", "り", true), ("ro", "ろ", true), ("ru", "る", true),
    ("rya", "りゃ", true), ("rye", "りぇ", true), ("ryo", "りょ", true), ("ryu", "りゅ", true),
    ("sa", "さ", true), ("se", "せ", true), ("sha", "しゃ", true), ("she", "しぇ", true),
    ("shi", "し", true), ("sho", "しょ", true), ("shu", "しゅ", true), ("si", "し", false),
    ("so", "そ", true), ("su", "す", true), ("sya", "しゃ", false), ("syo", "しょ", false),
    ("syu", "しゅ", false), ("ta", "た", true), ("te", "て", true), ("thi", "てぃ", true),
    ("thu", "てゅ", true), ("ti", "ち", false), ("to", "と", true), ("tsu", "つ", true),
    ("tu", "つ", false), ("twu", "とぅ", true), ("tya", "ちゃ", false), ("tyo", "ちょ", false),
    ("tyu", "ちゅ", false), ("u", "う", true), ("va", "ゔぁ", true), ("ve", "ゔぇ", true),
    ("vi", "ゔぃ", true), ("vo", "ゔぉ", true), ("vu", "ゔ", true), ("wa", "わ", true),
    ("we", "うぇ", true), ("who", "うぉ", true), ("wi", "うぃ", true), ("wo", "を", true),
    ("xa", "ぁ", false), ("xe", "ぇ", false), ("xi", "ぃ", false), ("xo", "ぉ", false),
    ("xtsu", "っ", false), ("xtu", "っ", false), ("xu", "ぅ", false), ("xwa", "ゎ", false),
    ("xya", "ゃ", false), ("xyo", "ょ", false), ("xyu", "ゅ", false), ("ya", "や", true),
    ("ye", "いぇ", true), ("yo", "よ", true), ("yu", "ゆ", true), ("za", "ざ", true), ("ze", "ぜ", true),
    ("zi", "じ", false), ("zo", "ぞ", true), ("zu", "ず", true), ("zya", "じゃ", false),
    ("zyo", "じょ", false), ("zyu", "じゅ", false),
];

/// Longest token in `TABLE`, in bytes.
const MAX_TOKEN: usize = 4;

fn token(bytes: &[u8]) -> Option<(&'static str, bool)> {
    TABLE
        .binary_search_by(|(key, _, _)| key.as_bytes().cmp(bytes))
        .ok()
        .and_then(|index| TABLE.get(index))
        .map(|&(_, kana, strict)| (kana, strict))
}

/// Appends the kana for `input` to `out`.
///
/// The input is expected to be a-z, `'` and `-`; ASCII capitals are read as lower case. Rules, in
/// this order at each position: `-` gives ー; `'` is skipped; `n` gives ん before `'`, at the end,
/// before another `n` and before any letter other than a vowel or `y`; a doubled consonant gives
/// っ; then the longest token of up to four letters; a character that matches nothing is copied.
pub fn to_kana(input: &str, out: &mut String) -> KanaResult {
    let bytes = input.as_bytes();
    let mut result = KanaResult { complete: true, strict: true };
    let mut position = 0;
    while let Some(&raw) = bytes.get(position) {
        if !raw.is_ascii() {
            // Not romaji. Copied as it is, with the doubling rule applied as in the Swift code.
            let Some(ch) = input.get(position..).and_then(|rest| rest.chars().next()) else { break };
            let after = position + ch.len_utf8();
            let next = input.get(after..).and_then(|rest| rest.chars().next());
            if next == Some(ch) {
                out.push('っ');
            } else {
                out.push(ch);
                result.strict = false;
            }
            position = after;
            continue;
        }
        let key = raw.to_ascii_lowercase();
        let next = bytes.get(position + 1).map(u8::to_ascii_lowercase);
        if key == b'-' {
            out.push('ー');
            position += 1;
            continue;
        }
        if key == b'\'' {
            position += 1;
            continue;
        }
        if key == b'n' {
            let step = match next {
                Some(b'\'') => 2,
                None => 1,
                // A final "nn" is one ん; elsewhere the second n starts the next token.
                Some(b'n') => {
                    if position + 2 == bytes.len() {
                        2
                    } else {
                        1
                    }
                }
                Some(other) if !b"aiueoy".contains(&other) => 1,
                Some(_) => 0,
            };
            if step > 0 {
                out.push('ん');
                position += step;
                continue;
            }
        }
        if next == Some(key) && !b"aiueon".contains(&key) {
            out.push('っ');
            position += 1;
            continue;
        }
        let mut lower = [0u8; MAX_TOKEN];
        let mut available = 0;
        for (slot, &byte) in lower.iter_mut().zip(bytes.iter().skip(position)) {
            *slot = byte.to_ascii_lowercase();
            available += 1;
        }
        let found = (1..=available)
            .rev()
            .find_map(|count| lower.get(..count).and_then(token).map(|hit| (count, hit)));
        match found {
            Some((count, (kana, strict))) => {
                out.push_str(kana);
                result.strict &= strict;
                position += count;
            }
            None => {
                out.push(char::from(key));
                result.complete &= !key.is_ascii_alphabetic();
                result.strict = false;
                position += 1;
            }
        }
    }
    result
}

/// Hiragana to katakana: code point + 0x60 for U+3041..U+3096; other characters unchanged.
pub fn to_katakana(hiragana: &str, out: &mut String) {
    for ch in hiragana.chars() {
        let code = u32::from(ch);
        let mapped = if (0x3041..=0x3096).contains(&code) { char::from_u32(code + 0x60) } else { None };
        out.push(mapped.unwrap_or(ch));
    }
}

/// The vowel kana a following ー stands for, or `None` (ん, anything that is not hiragana).
fn vowel_of(ch: char) -> Option<char> {
    const ROWS: [(&str, char); 5] = [
        ("あかさたなはまやらわがざだばぱぁゃゎ", 'あ'),
        ("いきしちにひみりぎじぢびぴぃ", 'い'),
        ("うくすつぬふむゆるぐずづぶぷぅゅゔっ", 'う'),
        ("えけせてねへめれげぜでべぺぇ", 'え'),
        ("おこそとのほもよろをごぞどぼぽぉょ", 'お'),
    ];
    ROWS.iter().find(|(row, _)| row.contains(ch)).map(|&(_, vowel)| vowel)
}

/// Appends the lexicon key of a kana reading to `out`: katakana (U+30A1..U+30F6) becomes
/// hiragana, and ー becomes the vowel kana of the kana before it. ー stays when nothing with a
/// vowel precedes it. こーひー, コーヒー and こおひい give the same key.
pub fn normalise_key(kana: &str, out: &mut String) {
    let mut last = None;
    for ch in kana.chars() {
        let code = u32::from(ch);
        let hiragana = if (0x30A1..=0x30F6).contains(&code) { char::from_u32(code - 0x60) } else { None };
        let mut ch = hiragana.unwrap_or(ch);
        if ch == 'ー' {
            ch = last.and_then(vowel_of).unwrap_or(ch);
        }
        out.push(ch);
        last = Some(ch);
    }
}
