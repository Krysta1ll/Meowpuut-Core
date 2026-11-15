//! Names (§3.3, §3.6 「ID 與名稱」, §4.1).
//!
//! * Skin IDs, kids and image/sound stems: lower case `[a-z0-9_-]{1,64}`,
//!   never a Windows device name (`con`, `prn`, `aux`, `nul`, `com1`–`com9`,
//!   `lpt1`–`lpt9`), so one name is one folder on every file system.
//! * Archive file names: `<stem>.(json|png|wav)`; no slash, dot or capital.
//! * `"<id>@<rev>"` references: the ID rule plus a decimal u32 without a
//!   leading zero, re-parsed on every load instead of being joined into a path.
//! * Display names: 1–24 user-perceived characters, no control, format
//!   (bidi, zero-width), separator, private-use or noncharacter code points.
//!
//! # Examples
//!
//! ```
//! use meowpuut_skin::names::*;
//! assert!(is_name("sakura_tea"));
//! assert!(!is_name("Sakura_tea") && !is_name("con") && !is_name("../x"));
//! assert_eq!(file_name("cat-1.png"), Some(("cat-1", FileKind::Png)));
//! assert_eq!(file_name("con.png"), None);
//! assert_eq!(parse_skin_ref("sakura_tea@3"), Some(("sakura_tea".to_string(), 3)));
//! assert_eq!(parse_skin_ref("sakura_tea@03"), None);
//! assert!(is_display_name("櫻花奶茶") && !is_display_name("evil\u{202E}gnp"));
//! ```

/// Longest name.
pub const NAME_MAX: usize = 64;
/// Most user-perceived characters in a display name.
pub const DISPLAY_MAX: usize = 24;

/// Whether `stem` is a Windows device name (case-insensitive).
pub fn is_reserved(stem: &str) -> bool {
    let lower = stem.to_ascii_lowercase();
    match lower.as_str() {
        "con" | "prn" | "aux" | "nul" => true,
        _ => {
            let b = lower.as_bytes();
            b.len() == 4 && (lower.starts_with("com") || lower.starts_with("lpt")) && (b'1'..=b'9').contains(&b[3])
        }
    }
}

/// A skin ID, kid or file stem.
pub fn is_name(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= NAME_MAX
        && text.bytes().all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'))
        && !is_reserved(text)
}

/// The kind of a file in a skin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileKind {
    /// `manifest.json`.
    Json,
    /// An image.
    Png,
    /// A sound.
    Wav,
}

/// Splits an archive file name into its stem and kind; `None` when not allowed.
pub fn file_name(name: &str) -> Option<(&str, FileKind)> {
    let (stem, ext) = name.rsplit_once('.')?;
    let kind = match ext {
        "json" => FileKind::Json,
        "png" => FileKind::Png,
        "wav" => FileKind::Wav,
        _ => return None,
    };
    is_name(stem).then_some((stem, kind))
}

/// Parses `"<id>@<rev>"` (the platform's `customSkin` setting or a revocation entry).
pub fn parse_skin_ref(text: &str) -> Option<(String, u32)> {
    let (id, rev) = text.split_once('@')?;
    if !is_name(id) {
        return None;
    }
    Some((id.to_string(), parse_rev(rev)?))
}

/// A decimal u32 of at least 1 without sign or leading zero.
pub fn parse_rev(text: &str) -> Option<u32> {
    if text.is_empty() || text.len() > 10 || text.starts_with('0') || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse::<u32>().ok()
}

/// `"<id>@<rev>"`.
pub fn skin_ref(id: &str, rev: u32) -> String {
    format!("{id}@{rev}")
}

/// Code points refused in a display name: Cc, Cf (bidi controls, zero-width
/// characters, tags), Zl, Zp, private use and noncharacters (Unicode 16).
pub fn is_forbidden_char(c: char) -> bool {
    let u = c as u32;
    const RANGES: &[(u32, u32)] = &[
        (0x0000, 0x001F),
        (0x007F, 0x009F),
        (0x00AD, 0x00AD),
        (0x0600, 0x0605),
        (0x061C, 0x061C),
        (0x06DD, 0x06DD),
        (0x070F, 0x070F),
        (0x0890, 0x0891),
        (0x08E2, 0x08E2),
        (0x180E, 0x180E),
        (0x200B, 0x200F),
        (0x2028, 0x202E),
        (0x2060, 0x2064),
        (0x2066, 0x206F),
        (0xE000, 0xF8FF),
        (0xFDD0, 0xFDEF),
        (0xFEFF, 0xFEFF),
        (0xFFF9, 0xFFFB),
        (0x110BD, 0x110BD),
        (0x110CD, 0x110CD),
        (0x13430, 0x1343F),
        (0x1BCA0, 0x1BCA3),
        (0x1D173, 0x1D17A),
        (0xE0001, 0xE0001),
        (0xE0020, 0xE007F),
        (0xF0000, 0x10FFFF),
    ];
    (u & 0xFFFE) == 0xFFFE || RANGES.iter().any(|&(a, b)| (a..=b).contains(&u))
}

/// Whether `c` extends the previous character (combining marks, variation
/// selectors, emoji skin tones, Hangul vowel and final jamo). An
/// approximation of grapheme clusters without a segmentation table: it never
/// counts fewer clusters than there are, so the 24 limit can only get stricter.
fn extends(c: char) -> bool {
    let u = c as u32;
    const RANGES: &[(u32, u32)] = &[
        (0x0300, 0x036F),
        (0x0483, 0x0489),
        (0x0591, 0x05BD),
        (0x05BF, 0x05C7),
        (0x0610, 0x061A),
        (0x064B, 0x065F),
        (0x0670, 0x0670),
        (0x06D6, 0x06ED),
        (0x0900, 0x0903),
        (0x093A, 0x094F),
        (0x0951, 0x0957),
        (0x0962, 0x0963),
        (0x0E31, 0x0E31),
        (0x0E34, 0x0E3A),
        (0x0E47, 0x0E4E),
        (0x1160, 0x11FF),
        (0x1AB0, 0x1AFF),
        (0x1DC0, 0x1DFF),
        (0x20D0, 0x20FF),
        (0x302A, 0x302F),
        (0x3099, 0x309A),
        (0xFE00, 0xFE0F),
        (0xFE20, 0xFE2F),
        (0x1F3FB, 0x1F3FF),
        (0xE0100, 0xE01EF),
    ];
    RANGES.iter().any(|&(a, b)| (a..=b).contains(&u))
}

/// User-perceived characters (see [`extends`]).
pub fn display_len(text: &str) -> usize {
    let mut count = 0;
    for (i, c) in text.chars().enumerate() {
        if i == 0 || !extends(c) {
            count += 1;
        }
    }
    count
}

/// A display name: 1–24 user-perceived characters, no forbidden code point,
/// no leading or trailing white space, not starting with a combining mark.
pub fn is_display_name(text: &str) -> bool {
    let n = display_len(text);
    (1..=DISPLAY_MAX).contains(&n)
        && text.trim() == text
        && !text.chars().next().is_some_and(extends)
        && !text.chars().any(is_forbidden_char)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_names() {
        for name in ["con", "prn", "aux", "nul", "com1", "com9", "lpt1", "lpt9", "CON", "Lpt3"] {
            assert!(is_reserved(name), "{name}");
        }
        for name in ["com0", "com10", "console", "lpt", "nul_", "auxx"] {
            assert!(!is_reserved(name), "{name}");
        }
        assert_eq!(file_name("nul.wav"), None);
        assert_eq!(file_name("com1.json"), None);
        assert_eq!(file_name("con.png.png"), None);
    }

    #[test]
    fn file_names() {
        assert_eq!(file_name("manifest.json"), Some(("manifest", FileKind::Json)));
        for bad in ["../a.png", "a/b.png", "A.png", "a.PNG", ".png", "a.png\0", "a..png", "a.gif", "a", "a b.png", "é.png"] {
            assert_eq!(file_name(bad), None, "{bad}");
        }
        assert!(file_name(&format!("{}.png", "a".repeat(64))).is_some());
        assert!(file_name(&format!("{}.png", "a".repeat(65))).is_none());
    }

    #[test]
    fn skin_refs() {
        assert_eq!(parse_skin_ref("a@4294967295"), Some(("a".into(), u32::MAX)));
        for bad in ["a@4294967296", "a@0", "a@", "@1", "A@1", "con@1", "../x@1", "a@+1", "a@1@2", "a@ 1", "a"] {
            assert_eq!(parse_skin_ref(bad), None, "{bad}");
        }
    }

    #[test]
    fn display_names() {
        assert!(is_display_name("Sakura Tea"));
        assert!(is_display_name("桜ミルクティー"));
        assert!(is_display_name("e\u{301}"));
        assert_eq!(display_len("e\u{301}"), 1);
        assert!(is_display_name(&"喵".repeat(24)));
        for bad in [
            String::new(),
            "喵".repeat(25),
            "a\nb".into(),
            "a\u{202E}b".into(),
            "a\u{200B}b".into(),
            "a\u{200D}b".into(),
            "a\u{2028}b".into(),
            " a".into(),
            "a ".into(),
            "\u{301}a".into(),
            "a\u{E000}".into(),
            "a\u{FFFF}".into(),
            "a\u{E0041}".into(),
        ] {
            assert!(!is_display_name(&bad), "{bad:?}");
        }
    }
}
