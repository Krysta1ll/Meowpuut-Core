//! C ABI of the extended Japanese lookup (`mpn_japanese_lookup_ex`), over the image of
//! `meowpuut-core/tests/data/mimo_jp_forms_fixture.tsv`, built by the core test writer.
#![allow(unsafe_code)]
#![allow(clippy::undocumented_unsafe_blocks, clippy::multiple_unsafe_ops_per_block)]

#[path = "../../meowpuut-core/tests/support/mimo_jp_writer.rs"]
mod writer;

use std::ffi::{c_char, CStr, CString};
use std::ptr::{null, null_mut};

use meowpuut_ffi::*;

const FORMS_FIXTURE: &str = include_str!("../../meowpuut-core/tests/data/mimo_jp_forms_fixture.tsv");
const MAIN_FIXTURE: &[u8] = include_bytes!("../../meowpuut-core/tests/data/mimo_jp_fixture.bin");
const ALL: u32 = MPN_JP_OPTION_FORMS | MPN_JP_OPTION_COMPLETIONS | MPN_JP_OPTION_SEGMENTS;

fn image() -> Vec<u8> {
    writer::Image::parse(FORMS_FIXTURE).write()
}

fn s(p: *const c_char) -> String {
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

fn info(query: *const MPNJapaneseQuery, index: usize) -> MPNJapaneseInfo {
    let mut out = MPNJapaneseInfo::default();
    let written = unsafe { mpn_japanese_info(query, index, &mut out, std::mem::size_of::<MPNJapaneseInfo>()) };
    assert_eq!(written, 1);
    out
}

/// Texts and infos of a `mpn_japanese_lookup_ex`.
fn lookup_ex(
    query: *mut MPNJapaneseQuery,
    japanese: *const MPNJapanese,
    input: &str,
    mode: i32,
    options: u32,
) -> Vec<(String, MPNJapaneseInfo)> {
    let input = CString::new(input).expect("no NUL");
    unsafe {
        let count = mpn_japanese_lookup_ex(query, japanese, input.as_ptr(), mode, 64, options);
        assert_eq!(s(mpn_japanese_text(query, count)), "");
        (0..count).map(|index| (s(mpn_japanese_text(query, index)), info(query, index))).collect()
    }
}

#[test]
fn options_zero_matches_the_plain_lookup() {
    unsafe {
        for bytes in [image(), MAIN_FIXTURE.to_vec()] {
            let japanese = mpn_japanese_open(bytes.as_ptr().cast(), bytes.len(), 1);
            assert!(!japanese.is_null());
            let (old, new) = (mpn_japanese_query_create(), mpn_japanese_query_create());
            for input in ["sushi", "sensei", "tabeta", "tabemashita", "itta", "kyou", "arig", "renai", "zzz", ""] {
                let c_input = CString::new(input).expect("no NUL");
                for mode in [MPN_JP_MODE_JAPANESE, MPN_JP_MODE_PINYIN] {
                    let count = mpn_japanese_lookup(old, japanese, c_input.as_ptr(), mode, 64);
                    assert_eq!(mpn_japanese_lookup_ex(new, japanese, c_input.as_ptr(), mode, 64, 0), count, "{input}");
                    for index in 0..count {
                        assert_eq!(s(mpn_japanese_text(old, index)), s(mpn_japanese_text(new, index)));
                        let (a, b) = (info(old, index), info(new, index));
                        assert_eq!(a, b, "{input}");
                        assert_eq!(b.consumed as usize, input.len());
                    }
                    assert_eq!(s(mpn_japanese_kana(old)), s(mpn_japanese_kana(new)));
                    assert_eq!(s(mpn_japanese_katakana(old)), s(mpn_japanese_katakana(new)));
                }
            }
            mpn_japanese_query_destroy(old);
            mpn_japanese_query_destroy(new);
            mpn_japanese_close(japanese);
        }
    }
}

#[test]
fn forms_completions_and_segments() {
    let bytes = image();
    unsafe {
        let japanese = mpn_japanese_open(bytes.as_ptr().cast(), bytes.len(), 0);
        let query = mpn_japanese_query_create();

        let found = lookup_ex(query, japanese, "tabemashita", MPN_JP_MODE_JAPANESE, ALL);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, "食べました");
        assert_eq!(
            found[0].1,
            MPNJapaneseInfo {
                kind: MPN_JP_KIND_READING,
                flags: MPN_JP_FLAG_FORM | MPN_JP_FLAG_PINYIN_OK,
                weight: 6700,
                word: 0,
                consumed: 11
            }
        );
        assert_eq!(s(mpn_japanese_kana(query)), "たべました");
        assert_eq!(s(mpn_japanese_katakana(query)), "タベマシタ");

        let found = lookup_ex(query, japanese, "arig", MPN_JP_MODE_JAPANESE, ALL);
        let names: Vec<&str> = found.iter().map(|(text, _)| text.as_str()).collect();
        assert_eq!(names, ["ありがとう", "有難う", "ありがたい"]);
        assert!(found.iter().all(|(_, info)| info.flags & MPN_JP_FLAG_COMPLETION != 0 && info.consumed == 4));
        assert_eq!(s(mpn_japanese_kana(query)), "");

        // Segments: commit 今日は (6 bytes), keep composing the rest.
        let found = lookup_ex(query, japanese, "kyouhaiitenkidesune", MPN_JP_MODE_JAPANESE, ALL);
        let pairs: Vec<(&str, u32)> = found.iter().map(|(text, info)| (text.as_str(), info.consumed)).collect();
        assert_eq!(pairs, [("今日は", 6), ("教は", 6), ("今日", 4), ("教", 4)]);
        let found = lookup_ex(query, japanese, "iitenkidesune", MPN_JP_MODE_JAPANESE, ALL);
        assert_eq!((found[0].0.as_str(), found[0].1.consumed), ("いい", 2));
        let found = lookup_ex(query, japanese, "tenkidesune", MPN_JP_MODE_JAPANESE, ALL);
        assert_eq!((found[0].0.as_str(), found[0].1.consumed), ("天気ですね", 11));

        // Pinyin mode: no completions, segments, particles.
        assert!(lookup_ex(query, japanese, "arig", MPN_JP_MODE_PINYIN, ALL).is_empty());
        assert!(lookup_ex(query, japanese, "watashiha", MPN_JP_MODE_PINYIN, ALL).is_empty());
        assert_eq!(lookup_ex(query, japanese, "tabeta", MPN_JP_MODE_PINYIN, ALL)[0].0, "食べた");

        // A 16-byte caller gets the first four fields and nothing behind them.
        lookup_ex(query, japanese, "kyouhaiitenkidesune", MPN_JP_MODE_JAPANESE, ALL);
        let mut out = [0xAAu8; 20];
        assert_eq!(mpn_japanese_info(query, 0, out.as_mut_ptr().cast(), 16), 1);
        assert!(out[16..].iter().all(|&b| b == 0xAA));
        assert_eq!(mpn_japanese_info(query, 0, out.as_mut_ptr().cast(), 20), 1);
        assert_eq!(&out[16..20], &6u32.to_ne_bytes());

        // The texts of generated forms belong to the query: they outlive the image and its bytes.
        let count = mpn_japanese_lookup_ex(query, japanese, c"benkyoushimasu".as_ptr(), MPN_JP_MODE_JAPANESE, 64, ALL);
        assert_eq!(count, 1);
        mpn_japanese_close(japanese);
        drop(bytes);
        assert_eq!(s(mpn_japanese_text(query, 0)), "勉強します");
        assert_eq!(info(query, 0).consumed, 14);
        mpn_japanese_query_destroy(query);
    }
}

#[test]
fn bad_arguments_give_nothing() {
    let bytes = image();
    unsafe {
        let japanese = mpn_japanese_open(bytes.as_ptr().cast(), bytes.len(), 0);
        let query = mpn_japanese_query_create();
        let input = c"tabemashita";
        assert_eq!(mpn_japanese_lookup_ex(null_mut(), japanese, input.as_ptr(), 0, 64, ALL), 0);
        assert_eq!(mpn_japanese_lookup_ex(query, japanese, input.as_ptr(), 0, 64, ALL), 1);
        // A failed lookup clears the previous result.
        assert_eq!(mpn_japanese_lookup_ex(query, null(), input.as_ptr(), 0, 64, ALL), 0);
        assert_eq!(s(mpn_japanese_text(query, 0)), "");
        assert_eq!(s(mpn_japanese_kana(query)), "");
        let mut one = MPNJapaneseInfo::default();
        assert_eq!(mpn_japanese_info(query, 0, &mut one, 20), 0);
        assert_eq!(mpn_japanese_lookup_ex(query, japanese, null(), 0, 64, ALL), 0);
        assert_eq!(mpn_japanese_lookup_ex(query, japanese, input.as_ptr(), 2, 64, ALL), 0);
        assert_eq!(mpn_japanese_lookup_ex(query, japanese, input.as_ptr(), -1, 64, ALL), 0);
        let not_utf8 = [b't', 0xff, 0];
        assert_eq!(mpn_japanese_lookup_ex(query, japanese, not_utf8.as_ptr().cast(), 0, 64, ALL), 0);
        let long = CString::new("a".repeat(81)).expect("no NUL");
        assert_eq!(mpn_japanese_lookup_ex(query, japanese, long.as_ptr(), 0, 64, ALL), 0);
        let longest = CString::new("ka".repeat(40)).expect("no NUL");
        let count = mpn_japanese_lookup_ex(query, japanese, longest.as_ptr(), 0, 64, ALL);
        for index in 0..count {
            assert!(info(query, index).consumed <= 80);
        }
        // Limit 0: no rows, the kana is still given.
        assert_eq!(mpn_japanese_lookup_ex(query, japanese, input.as_ptr(), 0, 0, ALL), 0);
        assert_eq!(s(mpn_japanese_kana(query)), "たべました");
        mpn_japanese_query_destroy(query);
        mpn_japanese_close(japanese);
    }
}
