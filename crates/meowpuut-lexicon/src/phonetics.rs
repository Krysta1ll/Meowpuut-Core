#![forbid(unsafe_code)]
//! Phonetic encoding & normalization for Pinyin, Zhuyin (Bopomofo), and Japanese
//! Romaji / Hiragana / Katakana.
//!
//! # Examples
//!
//! ```
//! use meowpuut_lexicon::phonetics::{
//!     encode_syllable, numbered_pinyin, pinyin_kana, romaji_to_hiragana, hiragana_to_katakana,
//! };
//!
//! assert_eq!(encode_syllable("ㄋㄧˇ"), "su3");
//! assert_eq!(encode_syllable("ㄏㄠˇ"), "cl3");
//! assert_eq!(numbered_pinyin("nǐ hǎo"), "ni3hao3");
//! assert_eq!(pinyin_kana("ni"), Some("に"));
//! assert_eq!(romaji_to_hiragana("watashi"), "わたし");
//! assert_eq!(romaji_to_hiragana("nippon"), "にっぽん");
//! assert_eq!(hiragana_to_katakana("ねこ"), "ネコ");
//! ```

const ZHUYIN_KEY_TABLE: [(char, u8); 41] = [
    ('ㄅ', b'1'), ('ㄆ', b'q'), ('ㄇ', b'a'), ('ㄈ', b'z'),
    ('ㄉ', b'2'), ('ㄊ', b'w'), ('ㄋ', b's'), ('ㄌ', b'x'),
    ('ㄍ', b'e'), ('ㄎ', b'd'), ('ㄏ', b'c'), ('ㄐ', b'r'),
    ('ㄑ', b'f'), ('ㄒ', b'v'), ('ㄓ', b'5'), ('ㄔ', b't'),
    ('ㄕ', b'g'), ('ㄖ', b'b'), ('ㄗ', b'y'), ('ㄘ', b'h'),
    ('ㄙ', b'n'), ('ㄧ', b'u'), ('ㄨ', b'j'), ('ㄩ', b'm'),
    ('ㄚ', b'8'), ('ㄛ', b'i'), ('ㄜ', b'k'), ('ㄝ', b','),
    ('ㄞ', b'9'), ('ㄟ', b'o'), ('ㄠ', b'l'), ('ㄡ', b'.'),
    ('ㄢ', b'0'), ('ㄣ', b'p'), ('ㄤ', b';'), ('ㄥ', b'/'),
    ('ㄦ', b'-'), ('ˉ', b' '), ('ˊ', b'6'), ('ˇ', b'3'),
    ('ˋ', b'4'),
];

/// Encodes a Zhuyin (Bopomofo) syllable string (e.g. `"ㄋㄧˇ"`) into standard
/// keyboard keystrokes (`"su3"`). Neutral tone `'˙'` maps to `'7'`, and unmarked
/// first tone appends `' '` when a sound symbol is present without a tone mark.
pub fn encode_syllable(syllable: &str) -> String {
    let mut out = String::with_capacity(syllable.len());
    let mut has_tone = false;
    let mut has_sound = false;
    for ch in syllable.chars() {
        if ch == '˙' {
            out.push('7');
            has_tone = true;
            continue;
        }
        if let Some(&(_, key)) = ZHUYIN_KEY_TABLE.iter().find(|&&(sym, _)| sym == ch) {
            out.push(key as char);
            if matches!(key, b' ' | b'6' | b'3' | b'4' | b'7') {
                has_tone = true;
            } else {
                has_sound = true;
            }
        }
    }
    if has_sound && !has_tone {
        out.push(' ');
    }
    out
}

fn decode_accented_vowel(ch: char) -> Option<(&'static str, u8)> {
    match ch {
        'ā' => Some(("a", 1)), 'á' => Some(("a", 2)), 'ǎ' => Some(("a", 3)), 'à' => Some(("a", 4)),
        'ē' => Some(("e", 1)), 'é' => Some(("e", 2)), 'ě' => Some(("e", 3)), 'è' => Some(("e", 4)),
        'ī' => Some(("i", 1)), 'í' => Some(("i", 2)), 'ǐ' => Some(("i", 3)), 'ì' => Some(("i", 4)),
        'ō' => Some(("o", 1)), 'ó' => Some(("o", 2)), 'ǒ' => Some(("o", 3)), 'ò' => Some(("o", 4)),
        'ū' => Some(("u", 1)), 'ú' => Some(("u", 2)), 'ǔ' => Some(("u", 3)), 'ù' => Some(("u", 4)),
        'ǖ' => Some(("v", 1)), 'ǘ' => Some(("v", 2)), 'ǚ' => Some(("v", 3)), 'ǜ' => Some(("v", 4)),
        'ü' => Some(("v", 0)),
        _ => None,
    }
}

/// Converts accented or space-delimited Pinyin (e.g. `"nǐ hǎo"`, `"lǜ"`, `"ni3 hao3"`)
/// into normalized numbered Pinyin (`"ni3hao3"`, `"lv4"`).
pub fn numbered_pinyin(reading: &str) -> String {
    let mut out = String::with_capacity(reading.len() + 4);
    for raw_token in reading.split(|c: char| c.is_whitespace() || c == '\'') {
        if raw_token.is_empty() {
            continue;
        }
        let mut syllable = String::with_capacity(8);
        let mut explicit_tone: u8 = 0;
        for ch in raw_token.chars() {
            if let Some((base, tone)) = decode_accented_vowel(ch) {
                syllable.push_str(base);
                if tone > 0 {
                    explicit_tone = tone;
                }
            } else if ch.is_ascii_alphabetic() {
                syllable.push(ch.to_ascii_lowercase());
            } else if (b'1'..=b'5').contains(&(ch as u8)) {
                explicit_tone = (ch as u8) - b'0';
            }
        }
        if !syllable.is_empty() {
            out.push_str(&syllable);
            if explicit_tone > 0 {
                out.push((b'0' + explicit_tone) as char);
            }
        }
    }
    out
}

/// Strips tone digits (`1..=5`) and apostrophes from a Pinyin reading to form a bucket key.
pub fn strip_pinyin_tones(reading: &str) -> String {
    let mut out = String::with_capacity(reading.len());
    for b in reading.bytes() {
        if (b'a'..=b'z').contains(&b) {
            out.push(b as char);
        }
    }
    out
}

/// Maps single-syllable Pinyin strings to phonetic Japanese Hiragana equivalents
/// when used as a cross-language bridge.
pub fn pinyin_kana(pinyin_syllable: &str) -> Option<&'static str> {
    match pinyin_syllable {
        "a" => Some("あ"), "i" | "yi" => Some("い"), "u" | "wu" => Some("う"),
        "e" => Some("え"), "o" | "wo" => Some("を"), "ka" => Some("か"),
        "ki" => Some("き"), "ku" => Some("く"), "ke" => Some("け"), "ko" => Some("こ"),
        "sa" => Some("さ"), "shi" | "xi" => Some("し"), "su" => Some("す"),
        "se" => Some("せ"), "so" => Some("そ"), "ta" => Some("た"),
        "chi" | "qi" => Some("ち"), "tsu" | "ci" => Some("つ"), "te" => Some("て"),
        "to" => Some("と"), "na" => Some("な"), "ni" => Some("に"),
        "nu" => Some("ぬ"), "ne" => Some("ね"), "no" => Some("の"),
        "ha" => Some("は"), "hi" => Some("ひ"), "fu" | "hu" => Some("ふ"),
        "he" => Some("へ"), "ho" => Some("ほ"), "ma" => Some("ま"),
        "mi" => Some("み"), "mu" => Some("む"), "me" => Some("め"), "mo" => Some("も"),
        "ya" => Some("や"), "yu" => Some("ゆ"), "yo" => Some("よ"),
        "ra" | "la" => Some("ら"), "ri" | "li" => Some("り"), "ru" | "lu" => Some("る"),
        "re" | "le" => Some("れ"), "ro" | "lo" => Some("ろ"), "wa" => Some("わ"),
        "ga" => Some("が"), "gi" => Some("ぎ"), "gu" => Some("ぐ"),
        "ge" => Some("げ"), "go" => Some("ご"), "za" => Some("ざ"),
        "ji" | "zhi" => Some("じ"), "zu" | "zi" => Some("ず"), "ze" => Some("ぜ"),
        "zo" => Some("ぞ"), "da" => Some("だ"), "de" => Some("で"), "do" => Some("ど"),
        "ba" => Some("ば"), "bi" => Some("び"), "bu" => Some("ぶ"),
        "be" => Some("べ"), "bo" => Some("ぼ"), "pa" => Some("ぱ"),
        "pi" => Some("ぴ"), "pu" => Some("ぷ"), "pe" => Some("ぺ"), "po" => Some("ぽ"),
        "hao" => Some("はお"),
        _ => None,
    }
}

const ROMAJI_TABLE: [(&str, &str); 139] = [
    ("kya", "きゃ"), ("kyu", "きゅ"), ("kyo", "きょ"),
    ("sha", "しゃ"), ("shu", "しゅ"), ("sho", "しょ"), ("sya", "しゃ"), ("syu", "しゅ"), ("syo", "しょ"),
    ("cha", "ちゃ"), ("chu", "ちゅ"), ("cho", "ちょ"), ("tya", "ちゃ"), ("tyu", "ちゅ"), ("tyo", "ちょ"),
    ("nya", "にゃ"), ("nyu", "にゅ"), ("nyo", "にょ"),
    ("hya", "ひゃ"), ("hyu", "ひゅ"), ("hyo", "ひょ"),
    ("mya", "みゃ"), ("myu", "みゅ"), ("myo", "みょ"),
    ("rya", "りゃ"), ("ryu", "りゅ"), ("ryo", "りょ"),
    ("gya", "ぎゃ"), ("gyu", "ぎゅ"), ("gyo", "ぎょ"),
    ("bya", "びゃ"), ("byu", "びゅ"), ("byo", "びょ"),
    ("pya", "ぴゃ"), ("pyu", "ぴゅ"), ("pyo", "ぴょ"),
    ("jya", "じゃ"), ("jyu", "じゅ"), ("jyo", "じょ"),
    ("zya", "じゃ"), ("zyu", "じゅ"), ("zyo", "じょ"),
    ("shi", "し"), ("chi", "ち"), ("tsu", "つ"),
    ("ja", "じゃ"), ("ju", "じゅ"), ("jo", "じょ"),
    ("fu", "ふ"),
    ("ka", "か"), ("ki", "き"), ("ku", "く"), ("ke", "け"), ("ko", "こ"),
    ("sa", "さ"), ("si", "し"), ("su", "す"), ("se", "せ"), ("so", "そ"),
    ("ta", "た"), ("ti", "ち"), ("tu", "つ"), ("te", "て"), ("to", "と"),
    ("na", "な"), ("ni", "に"), ("nu", "ぬ"), ("ne", "ね"), ("no", "の"),
    ("ha", "は"), ("hi", "ひ"), ("hu", "ふ"), ("he", "へ"), ("ho", "ほ"),
    ("ma", "ま"), ("mi", "み"), ("mu", "む"), ("me", "め"), ("mo", "も"),
    ("ya", "や"), ("yi", "い"), ("yu", "ゆ"), ("ye", "いぇ"), ("yo", "よ"),
    ("ra", "ら"), ("ri", "り"), ("ru", "る"), ("re", "れ"), ("ro", "ろ"),
    ("la", "ら"), ("li", "り"), ("lu", "る"), ("le", "れ"), ("lo", "ろ"),
    ("wa", "わ"), ("wi", "うぃ"), ("wu", "う"), ("we", "うぇ"), ("wo", "を"),
    ("ga", "が"), ("gi", "ぎ"), ("gu", "ぐ"), ("ge", "げ"), ("go", "ご"),
    ("za", "ざ"), ("zi", "じ"), ("zu", "ず"), ("ze", "ぜ"), ("zo", "ぞ"),
    ("ji", "じ"),
    ("da", "だ"), ("di", "ぢ"), ("du", "づ"), ("de", "で"), ("do", "ど"),
    ("ba", "ば"), ("bi", "び"), ("bu", "ぶ"), ("be", "べ"), ("bo", "ぼ"),
    ("pa", "ぱ"), ("pi", "ぴ"), ("pu", "ぷ"), ("pe", "ぺ"), ("po", "ぽ"),
    ("fa", "ふぁ"), ("fi", "ふぃ"), ("fe", "ふぇ"), ("fo", "ふぉ"),
    ("va", "ヴぁ"), ("vi", "ヴぃ"), ("vu", "ヴ"), ("ve", "ヴぇ"), ("vo", "ヴぉ"),
    ("a", "あ"), ("i", "い"), ("u", "う"), ("e", "え"), ("o", "お"),
];

#[inline]
fn is_vowel_byte(b: u8) -> bool {
    matches!(b, b'a' | b'i' | b'u' | b'e' | b'o')
}

/// Converts a Romaji string into Japanese Hiragana, handling double consonants (`っ`),
/// syllabic `n` (`ん`), and long vowel marks (`ー`).
pub fn romaji_to_hiragana(romaji: &str) -> String {
    let lower = romaji.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut out = String::with_capacity(bytes.len() * 3);
    let mut idx = 0usize;
    while idx < bytes.len() {
        let b = bytes[idx];
        if b == b'-' {
            out.push('ー');
            idx += 1;
            continue;
        }
        if b == b'\'' {
            idx += 1;
            continue;
        }
        if b == b'n' {
            if idx + 1 == bytes.len() {
                out.push('ん');
                idx += 1;
                continue;
            }
            let next = bytes[idx + 1];
            if next == b'n' || next == b'\'' {
                out.push('ん');
                idx += 2;
                continue;
            }
            if !is_vowel_byte(next) && next != b'y' {
                out.push('ん');
                idx += 1;
                continue;
            }
        }
        if idx + 1 < bytes.len()
            && bytes[idx] == bytes[idx + 1]
            && b.is_ascii_alphabetic()
            && !is_vowel_byte(b)
            && b != b'n'
        {
            out.push('っ');
            idx += 1;
            continue;
        }
        let rest = &lower[idx..];
        let mut matched = false;
        for &(rom, kana) in &ROMAJI_TABLE {
            if rest.starts_with(rom) {
                out.push_str(kana);
                idx += rom.len();
                matched = true;
                break;
            }
        }
        if !matched {
            out.push(b as char);
            idx += 1;
        }
    }
    out
}

/// Converts Hiragana codepoints (`U+3041..=U+3096`) to Katakana (`U+30A1..=U+30F6`).
pub fn hiragana_to_katakana(hiragana: &str) -> String {
    let mut out = String::with_capacity(hiragana.len());
    for ch in hiragana.chars() {
        let code = ch as u32;
        if (0x3041..=0x3096).contains(&code) {
            if let Some(kata) = char::from_u32(code + 0x60) {
                out.push(kata);
                continue;
            }
        }
        out.push(ch);
    }
    out
}
