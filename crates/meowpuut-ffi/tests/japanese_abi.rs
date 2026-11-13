//! C ABI of the Japanese lexicon image (`mpn_japanese_*`), over the fixture image of the core tests
//! (`meowpuut-core/tests/data/mimo_jp_fixture.tsv` lists its rows).
#![allow(unsafe_code)]
#![allow(clippy::undocumented_unsafe_blocks, clippy::multiple_unsafe_ops_per_block)]

use std::ffi::{c_char, CStr};
use std::ptr::{null, null_mut};

use meowpuut_ffi::*;

const FIXTURE: &[u8] = include_bytes!("../../meowpuut-core/tests/data/mimo_jp_fixture.bin");
const HEADER: &str = include_str!("../include/meowpuut.h");

const TSV: &str = "MEOWPUUT-PINYIN-1\nhao\thao3\t好\t好\t4800\nni\tni3\t你\t你\t5000\nnihao\tni3hao3\t你好\t你好\t5400\n";

fn s(p: *const c_char) -> String {
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

fn texts(query: *mut MPNJapaneseQuery, japanese: *const MPNJapanese, input: &CStr, mode: i32, limit: usize) -> Vec<String> {
    unsafe {
        let count = mpn_japanese_lookup(query, japanese, input.as_ptr(), mode, limit);
        assert_eq!(s(mpn_japanese_text(query, count)), "");
        (0..count).map(|index| s(mpn_japanese_text(query, index))).collect()
    }
}

fn info(query: *const MPNJapaneseQuery, index: usize) -> MPNJapaneseInfo {
    let mut out = MPNJapaneseInfo::default();
    let written = unsafe { mpn_japanese_info(query, index, &mut out, std::mem::size_of::<MPNJapaneseInfo>()) };
    assert_eq!(written, 1);
    out
}

#[test]
fn info_struct_is_16_bytes() {
    assert_eq!(std::mem::size_of::<MPNJapaneseInfo>(), 16);
    assert_eq!(std::mem::align_of::<MPNJapaneseInfo>(), 4);
}

#[test]
fn lookup_through_the_c_functions() {
    for copy in [0, 1] {
        unsafe {
            let japanese = mpn_japanese_open(FIXTURE.as_ptr().cast(), FIXTURE.len(), copy);
            assert!(!japanese.is_null(), "copy = {copy}");
            assert_eq!(mpn_japanese_word_count(japanese), 30);
            assert_eq!(mpn_japanese_chinese_build(japanese), 0);
            let query = mpn_japanese_query_create();

            assert_eq!(texts(query, japanese, c"sushi", MPN_JP_MODE_JAPANESE, 64), ["寿司", "すし", "鮨", "寿し"]);
            let first = info(query, 0);
            assert_eq!(
                first,
                MPNJapaneseInfo {
                    kind: MPN_JP_KIND_AUTHORED,
                    flags: MPN_JP_FLAG_PINYIN_OK | MPN_JP_FLAG_REVIEWED,
                    weight: 10_000,
                    word: 0
                }
            );
            assert_eq!(s(mpn_japanese_kana(query)), "すし");
            assert_eq!(s(mpn_japanese_katakana(query)), "スシ");
            assert_eq!(texts(query, japanese, c"sushi", MPN_JP_MODE_PINYIN, 5), ["寿司", "すし", "鮨"]);
            assert_eq!(texts(query, japanese, c"sensei", MPN_JP_MODE_JAPANESE, 3), ["先生", "先制", "専制"]);
            assert_eq!(texts(query, japanese, c"sensei", MPN_JP_MODE_PINYIN, 5), ["先生"]);
            assert_eq!(texts(query, japanese, c"Kawaii", MPN_JP_MODE_PINYIN, 5), ["可愛い", "かわいい"]);

            // The same word under two keys has the same `word`.
            assert_eq!(texts(query, japanese, c"renai", MPN_JP_MODE_JAPANESE, 64), ["恋愛"]);
            let variant = info(query, 0);
            assert_eq!(variant.kind, MPN_JP_KIND_VARIANT);
            assert_eq!(texts(query, japanese, c"ren'ai", MPN_JP_MODE_JAPANESE, 64), ["恋愛"]);
            assert_eq!(info(query, 0).kind, MPN_JP_KIND_READING);
            assert_eq!(info(query, 0).word, variant.word);

            assert_eq!(texts(query, japanese, c"tabeta", MPN_JP_MODE_PINYIN, 5), ["食べた"]);
            assert_eq!(info(query, 0).flags, MPN_JP_FLAG_PINYIN_OK | MPN_JP_FLAG_FORM);

            assert_eq!(texts(query, japanese, c"ko-hi-", MPN_JP_MODE_JAPANESE, 64), ["コーヒー", "珈琲"]);
            assert_eq!(s(mpn_japanese_kana(query)), "こーひー");
            assert_eq!(s(mpn_japanese_katakana(query)), "コーヒー");

            // No word, but the kana is still given; input that does not convert gives "".
            assert_eq!(texts(query, japanese, c"neko", MPN_JP_MODE_JAPANESE, 64), Vec::<String>::new());
            assert_eq!(s(mpn_japanese_kana(query)), "ねこ");
            assert_eq!(texts(query, japanese, c"nekoq", MPN_JP_MODE_JAPANESE, 64), Vec::<String>::new());
            assert_eq!(s(mpn_japanese_kana(query)), "");
            assert_eq!(texts(query, japanese, c"", MPN_JP_MODE_JAPANESE, 64), Vec::<String>::new());

            mpn_japanese_query_destroy(query);
            mpn_japanese_close(japanese);
        }
    }
}

#[test]
fn info_writes_only_the_bytes_asked_for() {
    unsafe {
        let japanese = mpn_japanese_open(FIXTURE.as_ptr().cast(), FIXTURE.len(), 0);
        let query = mpn_japanese_query_create();
        assert_eq!(mpn_japanese_lookup(query, japanese, c"sushi".as_ptr(), MPN_JP_MODE_JAPANESE, 64), 4);

        let mut out = [0xAAu8; 20];
        assert_eq!(mpn_japanese_info(query, 0, out.as_mut_ptr().cast(), 8), 1);
        assert_eq!(&out[0..4], &MPN_JP_KIND_AUTHORED.to_ne_bytes());
        assert_eq!(&out[4..8], &(MPN_JP_FLAG_PINYIN_OK | MPN_JP_FLAG_REVIEWED).to_ne_bytes());
        assert!(out[8..].iter().all(|&b| b == 0xAA));

        let mut out = [0xAAu8; 20];
        assert_eq!(mpn_japanese_info(query, 0, out.as_mut_ptr().cast(), 20), 1);
        assert!(out[16..].iter().all(|&b| b == 0xAA));

        let mut one = MPNJapaneseInfo::default();
        assert_eq!(mpn_japanese_info(query, 0, &mut one, 0), 0);
        assert_eq!(mpn_japanese_info(query, 4, &mut one, 16), 0);
        assert_eq!(mpn_japanese_info(query, 0, null_mut(), 16), 0);
        assert_eq!(mpn_japanese_info(null(), 0, &mut one, 16), 0);
        assert_eq!(one, MPNJapaneseInfo::default());

        mpn_japanese_query_destroy(query);
        mpn_japanese_close(japanese);
    }
}

#[test]
fn reviewed_rows() {
    unsafe {
        let japanese = mpn_japanese_open(FIXTURE.as_ptr().cast(), FIXTURE.len(), 0);
        assert_eq!(mpn_japanese_reviewed_count(japanese), 13);
        let mut reading = [0 as c_char; 97];
        let mut text = [0 as c_char; 129];
        let (mut weight, mut flags) = (0i32, 0u32);
        let mut readings = Vec::new();
        for index in 0..13 {
            let ok = mpn_japanese_reviewed_get(
                japanese,
                index,
                reading.as_mut_ptr(),
                reading.len(),
                text.as_mut_ptr(),
                text.len(),
                &mut weight,
                &mut flags,
            );
            assert_eq!(ok, 1);
            let r = s(reading.as_ptr());
            assert_eq!(mpn_japanese_reviewed_find(japanese, reading.as_ptr()), index as i32);
            if r == "sushi" {
                assert_eq!((s(text.as_ptr()), weight, flags), ("寿司".to_string(), 10_000, MPN_JP_REVIEWED_POOL));
            }
            if r == "ko-hi-" {
                assert_eq!((s(text.as_ptr()), weight, flags), ("コーヒー".to_string(), 9000, 0));
            }
            readings.push(r);
        }
        let mut sorted = readings.clone();
        sorted.sort();
        assert_eq!(readings, sorted);

        // Out of range, a short buffer, NULL buffers: 0 and nothing written.
        let mut small = [0x55 as c_char; 5];
        assert_eq!(
            mpn_japanese_reviewed_get(japanese, 13, reading.as_mut_ptr(), 97, text.as_mut_ptr(), 129, null_mut(), null_mut()),
            0
        );
        let sushi = mpn_japanese_reviewed_find(japanese, c"sushi".as_ptr()) as usize;
        assert_eq!(
            mpn_japanese_reviewed_get(japanese, sushi, small.as_mut_ptr(), 5, text.as_mut_ptr(), 129, null_mut(), null_mut()),
            0
        );
        assert!(small.iter().all(|&c| c == 0x55));
        assert_eq!(
            mpn_japanese_reviewed_get(japanese, sushi, small.as_mut_ptr(), 6, text.as_mut_ptr(), 129, null_mut(), null_mut()),
            1
        );
        assert_eq!(s(small.as_ptr()), "sushi");
        assert_eq!(
            mpn_japanese_reviewed_get(japanese, sushi, null_mut(), 97, text.as_mut_ptr(), 129, null_mut(), null_mut()),
            0
        );
        assert_eq!(
            mpn_japanese_reviewed_get(null(), 0, reading.as_mut_ptr(), 97, text.as_mut_ptr(), 129, null_mut(), null_mut()),
            0
        );

        assert_eq!(mpn_japanese_reviewed_find(japanese, c"sushii".as_ptr()), -1);
        assert_eq!(mpn_japanese_reviewed_find(japanese, null()), -1);
        assert_eq!(mpn_japanese_reviewed_find(null(), c"sushi".as_ptr()), -1);
        mpn_japanese_close(japanese);
    }
}

#[test]
fn reviewed_pool_goes_to_the_session() {
    unsafe {
        let lexicon = mpn_lexicon_create(TSV.as_ptr().cast(), TSV.len());
        assert!(!lexicon.is_null());
        let session = mpn_session_create(lexicon);
        assert!(!session.is_null());
        let japanese = mpn_japanese_open(FIXTURE.as_ptr().cast(), FIXTURE.len(), 1);
        // Ten rows carry the pool flag; the session rule accepts all of them.
        assert_eq!(mpn_session_add_japanese_mixed(session, japanese), 10);
        assert_eq!(s(mpn_mixed_pinyin(session, c"nihaosushi".as_ptr(), 0)), "你好寿司");
        assert_eq!(mpn_session_add_japanese_mixed(session, null()), 0);
        assert_eq!(mpn_session_add_japanese_mixed(null_mut(), japanese), 0);
        mpn_japanese_close(japanese);
        mpn_session_destroy(session);
        mpn_lexicon_destroy(lexicon);
    }
}

#[test]
fn bytes_lifetime() {
    unsafe {
        // copy != 0: the caller's bytes may go at once.
        let mut buffer = FIXTURE.to_vec();
        let copied = mpn_japanese_open(buffer.as_ptr().cast(), buffer.len(), 1);
        buffer.fill(0);
        drop(buffer);
        let query = mpn_japanese_query_create();
        assert_eq!(texts(query, copied, c"hon", MPN_JP_MODE_JAPANESE, 64), ["本", "翻"]);
        mpn_japanese_close(copied);

        // copy == 0: the strings of a query are its own; they outlive the image and its bytes.
        let buffer = FIXTURE.to_vec();
        let mapped = mpn_japanese_open(buffer.as_ptr().cast(), buffer.len(), 0);
        assert_eq!(mpn_japanese_lookup(query, mapped, c"toukyou".as_ptr(), MPN_JP_MODE_JAPANESE, 64), 1);
        mpn_japanese_close(mapped);
        drop(buffer);
        assert_eq!(s(mpn_japanese_text(query, 0)), "東京");
        assert_eq!(s(mpn_japanese_kana(query)), "とうきょう");
        assert_eq!(info(query, 0).kind, MPN_JP_KIND_AUTHORED);
        mpn_japanese_query_destroy(query);
    }
}

#[test]
fn bad_images_and_null_handles() {
    unsafe {
        assert!(mpn_japanese_open(null(), 100, 0).is_null());
        assert!(mpn_japanese_open(FIXTURE.as_ptr().cast(), 0, 0).is_null());
        assert!(mpn_japanese_open(FIXTURE.as_ptr().cast(), 63, 1).is_null());
        // size other than file_len
        assert!(mpn_japanese_open(FIXTURE.as_ptr().cast(), FIXTURE.len() - 1, 0).is_null());
        let mut longer = FIXTURE.to_vec();
        longer.push(0);
        assert!(mpn_japanese_open(longer.as_ptr().cast(), longer.len(), 1).is_null());
        let mut magic = FIXTURE.to_vec();
        magic[0] = b'X';
        assert!(mpn_japanese_open(magic.as_ptr().cast(), magic.len(), 1).is_null());
        let chinese = TSV.as_bytes();
        assert!(mpn_japanese_open(chinese.as_ptr().cast(), chinese.len(), 1).is_null());

        assert_eq!(mpn_japanese_word_count(null()), 0);
        assert_eq!(mpn_japanese_chinese_build(null()), 0);
        assert_eq!(mpn_japanese_reviewed_count(null()), 0);
        mpn_japanese_close(null_mut());
        mpn_japanese_query_destroy(null_mut());
        assert_eq!(s(mpn_japanese_text(null(), 0)), "");
        assert_eq!(s(mpn_japanese_kana(null())), "");
        assert_eq!(s(mpn_japanese_katakana(null())), "");

        let japanese = mpn_japanese_open(FIXTURE.as_ptr().cast(), FIXTURE.len(), 0);
        let query = mpn_japanese_query_create();
        assert_eq!(mpn_japanese_lookup(null_mut(), japanese, c"sushi".as_ptr(), 0, 64), 0);
        assert_eq!(mpn_japanese_lookup(query, japanese, c"sushi".as_ptr(), 0, 64), 4);
        // A failed lookup clears the previous result.
        assert_eq!(mpn_japanese_lookup(query, null(), c"sushi".as_ptr(), 0, 64), 0);
        assert_eq!(s(mpn_japanese_text(query, 0)), "");
        assert_eq!(s(mpn_japanese_kana(query)), "");
        assert_eq!(mpn_japanese_lookup(query, japanese, null(), 0, 64), 0);
        assert_eq!(mpn_japanese_lookup(query, japanese, c"sushi".as_ptr(), 2, 64), 0);
        assert_eq!(mpn_japanese_lookup(query, japanese, c"sushi".as_ptr(), -1, 64), 0);
        let not_utf8 = [b's', 0xff, 0];
        assert_eq!(mpn_japanese_lookup(query, japanese, not_utf8.as_ptr().cast(), 0, 64), 0);
        let long = std::ffi::CString::new("a".repeat(81)).unwrap();
        assert_eq!(mpn_japanese_lookup(query, japanese, long.as_ptr(), 0, 64), 0);
        mpn_japanese_query_destroy(query);
        mpn_japanese_close(japanese);
    }
}

#[test]
fn one_image_many_threads() {
    struct Shared(*const MPNJapanese);
    // SAFETY of the test: the image is read-only and outlives the threads (joined below).
    unsafe impl Sync for Shared {}
    let japanese = unsafe { mpn_japanese_open(FIXTURE.as_ptr().cast(), FIXTURE.len(), 0) };
    let shared = Shared(japanese);
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let shared = &shared;
            scope.spawn(move || {
                let query = mpn_japanese_query_create();
                for _ in 0..200 {
                    assert_eq!(texts(query, shared.0, c"shiji", MPN_JP_MODE_JAPANESE, 64), ["支持", "指示", "私事"]);
                    assert_eq!(texts(query, shared.0, c"shiji", MPN_JP_MODE_PINYIN, 5), ["指示"]);
                }
                unsafe { mpn_japanese_query_destroy(query) };
            });
        }
    });
    unsafe { mpn_japanese_close(japanese) };
}

#[test]
fn header_declares_every_function_and_constant() {
    let block = HEADER.split("typedef struct MPNJapanese MPNJapanese;").nth(1).expect("Japanese block in the header");
    for name in [
        "MPNJapanese *mpn_japanese_open(const void *data, size_t size, int copy);",
        "void mpn_japanese_close(MPNJapanese *japanese);",
        "size_t mpn_japanese_word_count(const MPNJapanese *japanese);",
        "uint32_t mpn_japanese_chinese_build(const MPNJapanese *japanese);",
        "size_t mpn_japanese_reviewed_count(const MPNJapanese *japanese);",
        "int mpn_japanese_reviewed_get(",
        "int32_t mpn_japanese_reviewed_find(const MPNJapanese *japanese, const char *reading);",
        "size_t mpn_session_add_japanese_mixed(MPNSession *session, const MPNJapanese *japanese);",
        "MPNJapaneseQuery *mpn_japanese_query_create(void);",
        "void mpn_japanese_query_destroy(MPNJapaneseQuery *query);",
        "size_t mpn_japanese_lookup(",
        "const char *mpn_japanese_text(const MPNJapaneseQuery *query, size_t index);",
        "int mpn_japanese_info(const MPNJapaneseQuery *query, size_t index, MPNJapaneseInfo *info, size_t info_size);",
        "const char *mpn_japanese_kana(const MPNJapaneseQuery *query);",
        "const char *mpn_japanese_katakana(const MPNJapaneseQuery *query);",
        "MPN_JP_MODE_JAPANESE = 0, MPN_JP_MODE_PINYIN = 1",
        "MPN_JP_KIND_READING = 0, MPN_JP_KIND_VARIANT = 1, MPN_JP_KIND_ALIAS_KANA = 2",
        "MPN_JP_KIND_ALIAS_KANJI = 3, MPN_JP_KIND_AUTHORED = 4",
        "MPN_JP_FLAG_PINYIN_OK = 1, MPN_JP_FLAG_REVIEWED = 2, MPN_JP_FLAG_FORM = 4",
        "MPN_JP_REVIEWED_POOL = 1",
    ] {
        assert!(block.contains(name), "missing in the header: {name}");
    }
    assert_eq!((MPN_JP_MODE_JAPANESE, MPN_JP_MODE_PINYIN), (0, 1));
    assert_eq!((MPN_JP_KIND_READING, MPN_JP_KIND_VARIANT, MPN_JP_KIND_ALIAS_KANA), (0, 1, 2));
    assert_eq!((MPN_JP_KIND_ALIAS_KANJI, MPN_JP_KIND_AUTHORED), (3, 4));
    assert_eq!((MPN_JP_FLAG_PINYIN_OK, MPN_JP_FLAG_REVIEWED, MPN_JP_FLAG_FORM, MPN_JP_REVIEWED_POOL), (1, 2, 4, 1));
}
