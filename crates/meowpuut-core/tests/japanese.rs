//! Reader and lookup of the Japanese lexicon image.
//!
//! `tests/data/mimo_jp_fixture.bin` is a small image; `mimo_jp_fixture.tsv` is its source. Every
//! fixture assertion runs twice: on the committed file and on an image written from the source by
//! the test writer in `tests/support/mimo_jp_writer.rs`. To rewrite the committed file with the
//! test writer: `MIMO_JP_WRITE_FIXTURE=1 cargo test -p meowpuut-core --test japanese fixture`.

#[path = "support/mimo_jp_writer.rs"]
mod writer;

use std::path::{Path, PathBuf};

use meowpuut_core::japanese::{
    JapaneseLexicon, JapaneseMode, JapaneseQuery, JapaneseRow, FLAG_FORM, FLAG_PINYIN_OK, FLAG_REVIEWED,
    KIND_ALIAS_KANA, KIND_ALIAS_KANJI, KIND_AUTHORED, KIND_READING, KIND_VARIANT, REVIEWED_POOL,
};
use meowpuut_core::romaji::{normalise_key, to_kana};
use writer::Image;

use JapaneseMode::{Japanese, Pinyin};

fn data_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(name)
}

fn fixture_source() -> Image {
    let path = data_path("mimo_jp_fixture.tsv");
    Image::parse(&std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display())))
}

fn open(bytes: Vec<u8>) -> JapaneseLexicon {
    JapaneseLexicon::from_vec(bytes).expect("image opens")
}

/// Texts of a lookup.
fn words(lexicon: &JapaneseLexicon, input: &str, mode: JapaneseMode, limit: usize) -> Vec<String> {
    let mut query = JapaneseQuery::new();
    let count = query.lookup(lexicon, input, mode, limit);
    assert_eq!(count, query.len());
    assert!(count <= limit);
    assert!(query.row(count).is_none());
    assert_eq!(query.text(count), "");
    (0..count).map(|index| query.text(index).to_string()).collect()
}

fn first_row(lexicon: &JapaneseLexicon, input: &str, mode: JapaneseMode) -> JapaneseRow {
    let mut query = JapaneseQuery::new();
    query.lookup(lexicon, input, mode, 64);
    query.row(0).unwrap_or_else(|| panic!("no word for {input:?}"))
}

fn check_fixture(lexicon: &JapaneseLexicon) {
    assert!(lexicon.verify());
    assert_eq!(lexicon.key_count(), 34);
    assert_eq!(lexicon.word_count(), 30);
    assert_eq!(lexicon.chinese_build(), 0);

    // Exact reading; reviewed words first; one surface under two word records is returned once.
    assert_eq!(words(lexicon, "sushi", Japanese, 64), ["寿司", "すし", "鮨", "寿し"]);
    let sushi = first_row(lexicon, "sushi", Japanese);
    assert_eq!(sushi, JapaneseRow { word: 0, weight: 10_000, kind: KIND_AUTHORED, flags: FLAG_PINYIN_OK | FLAG_REVIEWED });
    // Pinyin mode drops 寿し, which has no PINYIN_OK.
    assert_eq!(words(lexicon, "sushi", Pinyin, 5), ["寿司", "すし", "鮨"]);

    // A reviewed word stands before a heavier derived homophone; the limit cuts the list.
    assert_eq!(words(lexicon, "sensei", Japanese, 64), ["先生", "先制", "専制", "宣誓", "先聖"]);
    assert_eq!(words(lexicon, "sensei", Japanese, 3), ["先生", "先制", "専制"]);
    assert_eq!(words(lexicon, "sensei", Japanese, 0), Vec::<String>::new());
    assert_eq!(words(lexicon, "sensei", Pinyin, 5), ["先生"]);

    // Hepburn, Kunrei and nn spellings in Japanese mode.
    for input in ["shinbun", "sinbun", "sinnbun", "shinnbun", "SHINBUN"] {
        assert_eq!(words(lexicon, input, Japanese, 64), ["新聞"], "{input}");
    }
    // Pinyin mode reads kana keys for Hepburn input only, and does not retry んん.
    assert_eq!(words(lexicon, "shinbun", Pinyin, 5), ["新聞"]);
    assert_eq!(words(lexicon, "sinbun", Pinyin, 5), Vec::<String>::new());
    assert_eq!(words(lexicon, "shinnbun", Pinyin, 5), Vec::<String>::new());

    // Long vowels: ー and the doubled vowel give the same key.
    assert_eq!(words(lexicon, "ko-hi-", Japanese, 64), ["コーヒー", "珈琲"]);
    assert_eq!(first_row(lexicon, "ko-hi-", Japanese).kind, KIND_AUTHORED);
    assert_eq!(words(lexicon, "koohii", Japanese, 64), ["コーヒー", "珈琲"]);
    assert_eq!(first_row(lexicon, "koohii", Japanese).kind, KIND_READING);
    for input in ["ra-men", "ramen", "toukyou", "tokyo", "dongjing", "ongaku", "au"] {
        let found = words(lexicon, input, Japanese, 64);
        assert_eq!(found.len(), 1, "{input}: {found:?}");
        let row = first_row(lexicon, input, Japanese);
        assert_eq!(row.kind, KIND_AUTHORED, "{input}");
        assert_ne!(row.flags & FLAG_REVIEWED, 0, "{input}");
    }
    assert_eq!(first_row(lexicon, "dongjing", Pinyin).weight, 3500);

    // Variant key: the sloppy spelling reaches the word through its own kana.
    assert_eq!(words(lexicon, "renai", Japanese, 64), ["恋愛"]);
    assert_eq!(first_row(lexicon, "renai", Japanese).kind, KIND_VARIANT);
    assert_eq!(first_row(lexicon, "renai", Japanese).weight, 3341);
    assert_eq!(first_row(lexicon, "ren'ai", Japanese).kind, KIND_READING);
    assert_eq!(first_row(lexicon, "renai", Japanese).word, first_row(lexicon, "ren'ai", Japanese).word);

    // Apostrophes: the typed key first, then the key without them.
    assert_eq!(words(lexicon, "sin'ya", Japanese, 64), ["深夜"]);
    assert_eq!(first_row(lexicon, "sin'ya", Japanese).kind, KIND_AUTHORED);
    assert_eq!(words(lexicon, "shin'ya", Japanese, 64), ["深夜"]);
    assert_eq!(first_row(lexicon, "shin'ya", Japanese).kind, KIND_AUTHORED);
    assert_eq!(words(lexicon, "sin'ya", Pinyin, 5), ["深夜"]);

    // Aliases and the pinyin gate.
    assert_eq!(words(lexicon, "shiberu", Pinyin, 5), ["食べる"]);
    assert_eq!(first_row(lexicon, "shiberu", Pinyin).kind, KIND_ALIAS_KANA);
    assert_eq!(first_row(lexicon, "shiberu", Pinyin).word, first_row(lexicon, "taberu", Pinyin).word);
    assert_eq!(words(lexicon, "sanbu", Pinyin, 5), ["散歩"]);
    assert_eq!(first_row(lexicon, "sanbu", Pinyin).kind, KIND_ALIAS_KANJI);
    // Both keys hit in Japanese mode; the heavier kana posting goes first.
    assert_eq!(words(lexicon, "sanbu", Japanese, 64), ["三部", "散歩"]);
    assert_eq!(words(lexicon, "shiji", Japanese, 64), ["支持", "指示", "私事"]);
    assert_eq!(words(lexicon, "shiji", Pinyin, 5), ["指示"]);
    assert_eq!(words(lexicon, "hon", Japanese, 64), ["本", "翻"]);
    assert_eq!(words(lexicon, "hon", Pinyin, 5), ["本"]);
    assert_eq!(words(lexicon, "Kawaii", Pinyin, 5), ["可愛い", "かわいい"]);
    assert_eq!(words(lexicon, "yinyue", Pinyin, 5), ["音楽"]);
    assert_eq!(first_row(lexicon, "tabeta", Pinyin).flags, FLAG_PINYIN_OK | FLAG_FORM);

    // Kana of the input.
    let mut query = JapaneseQuery::new();
    assert_eq!(query.lookup(lexicon, "ko-hi-", Japanese, 64), 2);
    assert_eq!((query.kana(), query.katakana()), ("こーひー", "コーヒー"));
    assert_eq!(query.lookup(lexicon, "sinnbun", Japanese, 64), 1);
    assert_eq!((query.kana(), query.katakana()), ("しんんぶん", "シンンブン"));
    assert_eq!(query.lookup(lexicon, "nekoneko", Pinyin, 5), 0);
    assert_eq!((query.kana(), query.katakana()), ("ねこねこ", "ネコネコ"));
    // Not complete: no kana, but the typed key is still looked up.
    assert_eq!(query.lookup(lexicon, "yinyue", Japanese, 64), 1);
    assert_eq!((query.kana(), query.katakana()), ("", ""));
    assert_eq!(query.lookup(lexicon, "sush", Japanese, 64), 0);
    assert_eq!((query.kana(), query.katakana()), ("", ""));

    // Rejected input: empty, too long, or a byte other than a-z ' -.
    let long = "a".repeat(81);
    for input in ["", "sushi1", "su shi", "寿司", "sushi\n", long.as_str()] {
        assert_eq!(query.lookup(lexicon, input, Japanese, 64), 0, "{input:?}");
        assert_eq!((query.kana(), query.katakana(), query.len()), ("", "", 0), "{input:?}");
        assert!(query.is_empty());
    }
    assert_eq!(query.lookup(lexicon, &"a".repeat(80), Japanese, 64), 0);
    assert_eq!(query.kana(), "あ".repeat(80));
    assert_eq!(query.lookup(lexicon, "'", Japanese, 64), 0);
    assert_eq!(query.kana(), "");

    // Reviewed section.
    assert_eq!(lexicon.reviewed_count(), 13);
    let readings: Vec<&str> =
        (0..13).map(|index| lexicon.reviewed_row(index).expect("reviewed row").0).collect();
    let expected = [
        "au", "dongjing", "hon", "ko-hi-", "ongaku", "ra-men", "ramen", "shinya", "sin'ya", "sushi", "tokyo",
        "toukyou", "yinyue",
    ];
    assert_eq!(readings, expected);
    assert!(lexicon.reviewed_row(13).is_none());
    for (index, reading) in expected.iter().enumerate() {
        assert_eq!(lexicon.reviewed_find(reading), Some(index));
    }
    for reading in ["", "sush", "sushii", "a", "zz", "すし"] {
        assert_eq!(lexicon.reviewed_find(reading), None, "{reading:?}");
    }
    let row = |reading: &str| lexicon.reviewed_row(lexicon.reviewed_find(reading).expect("reviewed reading"));
    assert_eq!(row("sushi"), Some(("sushi", "寿司", 10_000, REVIEWED_POOL)));
    assert_eq!(row("dongjing"), Some(("dongjing", "東京", 3500, REVIEWED_POOL)));
    assert_eq!(row("sin'ya"), Some(("sin'ya", "深夜", 5000, 0)));
    assert_eq!(row("ko-hi-"), Some(("ko-hi-", "コーヒー", 9000, 0)));
    let pool = (0..13).filter(|&index| lexicon.reviewed_row(index).is_some_and(|row| row.3 & REVIEWED_POOL != 0));
    assert_eq!(pool.count(), 10);
}

#[test]
fn fixture_from_the_test_writer() {
    check_fixture(&open(fixture_source().write()));
}

#[test]
fn fixture_file() {
    let path = data_path("mimo_jp_fixture.bin");
    if std::env::var_os("MIMO_JP_WRITE_FIXTURE").is_some() {
        std::fs::write(&path, fixture_source().write()).expect("write fixture");
    }
    let bytes = std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    check_fixture(&open(bytes.clone()));
    // The same bytes used in place.
    let leaked: &'static [u8] = Box::leak(bytes.into_boxed_slice());
    let lexicon = JapaneseLexicon::from_static(leaked).expect("image opens in place");
    check_fixture(&lexicon);
    check_fixture(&lexicon.clone());
}

/// Probes used on damaged images: none of this may panic.
fn exercise(lexicon: &JapaneseLexicon) {
    let mut query = JapaneseQuery::new();
    let inputs = [
        "sushi", "sensei", "sinnbun", "ko-hi-", "koohii", "tokyo", "shin'ya", "sanbu", "shiji", "hon", "au",
        "renai", "tabeta", "kawaii", "yinyue", "zzz", "a",
    ];
    for input in inputs {
        for mode in [Japanese, Pinyin] {
            let count = query.lookup(lexicon, input, mode, 64);
            assert!(count <= 64);
            for index in 0..count {
                assert!(query.row(index).is_some());
                let _ = query.text(index);
            }
        }
    }
    for index in 0..lexicon.reviewed_count().min(5000) {
        let _ = lexicon.reviewed_row(index);
    }
    for reading in ["sushi", "au", "zzz", ""] {
        let _ = lexicon.reviewed_find(reading);
    }
    let _ = lexicon.verify();
}

fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn get_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"))
}

#[test]
fn damaged_headers_are_rejected() {
    let good = fixture_source().write();
    let len = good.len() as u32;
    assert!(JapaneseLexicon::from_vec(good.clone()).is_some());
    assert!(JapaneseLexicon::from_vec(Vec::new()).is_none());
    assert!(JapaneseLexicon::from_vec(good[..63].to_vec()).is_none());
    assert!(JapaneseLexicon::from_static(b"MIMO-JP-1\0\0\0").is_none());

    let damaged = |change: &dyn Fn(&mut Vec<u8>)| {
        let mut bytes = good.clone();
        change(&mut bytes);
        JapaneseLexicon::from_vec(bytes).is_none()
    };
    // Magic and version.
    assert!(damaged(&|bytes| bytes[0] = b'N'));
    assert!(damaged(&|bytes| bytes[9] = b'2'));
    assert!(damaged(&|bytes| bytes[11] = 1));
    assert!(damaged(&|bytes| put_u32(bytes, 12, 2)));
    assert!(damaged(&|bytes| put_u32(bytes, 12, 0)));
    // file_len must be the buffer length.
    assert!(damaged(&|bytes| put_u32(bytes, 56, len - 1)));
    assert!(damaged(&|bytes| put_u32(bytes, 56, len + 1)));
    assert!(damaged(&|bytes| bytes.push(0)));
    assert!(damaged(&|bytes| bytes.truncate(bytes.len() - 1)));
    // A section outside the file, inside the header, or with a count that runs past the end.
    for offset_field in [32, 36, 40, 44, 48] {
        assert!(damaged(&|bytes| put_u32(bytes, offset_field, len)), "offset field {offset_field}");
        assert!(damaged(&|bytes| put_u32(bytes, offset_field, 56)), "offset field {offset_field}");
        assert!(damaged(&|bytes| put_u32(bytes, offset_field, u32::MAX)), "offset field {offset_field}");
    }
    for count_field in [16, 20, 24, 28, 52] {
        assert!(damaged(&|bytes| put_u32(bytes, count_field, len)), "count field {count_field}");
        assert!(damaged(&|bytes| put_u32(bytes, count_field, u32::MAX)), "count field {count_field}");
    }
    // Two sections on the same bytes.
    assert!(damaged(&|bytes| {
        let keys = get_u32(bytes, 32);
        put_u32(bytes, 36, keys);
    }));
    assert!(damaged(&|bytes| {
        let words = get_u32(bytes, 40);
        put_u32(bytes, 44, words + 8);
        put_u32(bytes, 28, 1);
    }));
    // chinese_build is not checked.
    assert!(!damaged(&|bytes| put_u32(bytes, 60, 0xDEAD_BEEF)));
}

#[test]
fn truncated_files_never_panic() {
    let good = fixture_source().write();
    for len in 0..good.len() {
        // As cut: rejected, because file_len no longer matches.
        assert!(JapaneseLexicon::from_vec(good[..len].to_vec()).is_none(), "length {len}");
        // Cut, with file_len set to the new length: may open; reading it must not panic.
        if len >= 64 {
            let mut bytes = good[..len].to_vec();
            put_u32(&mut bytes, 56, len as u32);
            if let Some(lexicon) = JapaneseLexicon::from_vec(bytes) {
                exercise(&lexicon);
            }
        }
    }
}

#[test]
fn damaged_tables_never_panic() {
    let good = fixture_source().write();
    // Every byte behind the magic set to 0x00, 0xFF and 0x80 in turn.
    for at in 12..good.len() {
        for value in [0x00, 0xFF, 0x80] {
            let mut bytes = good.clone();
            bytes[at] = value;
            if let Some(lexicon) = JapaneseLexicon::from_vec(bytes) {
                exercise(&lexicon);
            }
        }
    }
    // Several bytes at once, from a fixed sequence.
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for _ in 0..3000 {
        let mut bytes = good.clone();
        for _ in 0..(1 + next() % 8) {
            let at = 16 + (next() as usize) % (bytes.len() - 16);
            bytes[at] = next() as u8;
        }
        if let Some(lexicon) = JapaneseLexicon::from_vec(bytes) {
            exercise(&lexicon);
        }
    }
}

#[test]
fn verify_finds_broken_tables() {
    let (good, layout) = fixture_source().write_with_layout();
    assert!(open(good.clone()).verify());
    let broken = |change: &dyn Fn(&mut Vec<u8>)| {
        let mut bytes = good.clone();
        change(&mut bytes);
        let lexicon = JapaneseLexicon::from_vec(bytes).expect("header still valid");
        exercise(&lexicon);
        !lexicon.verify()
    };
    let (keys, postings, word_table, reviewed, blob) =
        (layout.keys, layout.postings, layout.words, layout.reviewed, layout.blob);
    // Keys out of order: swap the first two key records.
    assert!(broken(&|bytes| {
        let (first, second) = (bytes[keys..keys + 12].to_vec(), bytes[keys + 12..keys + 24].to_vec());
        bytes[keys..keys + 12].copy_from_slice(&second);
        bytes[keys + 12..keys + 24].copy_from_slice(&first);
    }));
    // A key string that does not follow the one before it.
    assert!(broken(&|bytes| {
        let offset = get_u32(bytes, keys + 12);
        put_u32(bytes, keys + 12, offset + 1);
    }));
    // A key without postings, with too many, or sharing postings with its neighbour.
    assert!(broken(&|bytes| bytes[keys + 6..keys + 8].copy_from_slice(&0u16.to_le_bytes())));
    assert!(broken(&|bytes| bytes[keys + 6..keys + 8].copy_from_slice(&65u16.to_le_bytes())));
    assert!(broken(&|bytes| put_u32(bytes, keys + 12 + 8, 0)));
    // Posting fields: word index, weight, kind, flags.
    assert!(broken(&|bytes| put_u32(bytes, postings, 30)));
    assert!(broken(&|bytes| bytes[postings + 4..postings + 6].copy_from_slice(&0i16.to_le_bytes())));
    assert!(broken(&|bytes| bytes[postings + 4..postings + 6].copy_from_slice(&10_001i16.to_le_bytes())));
    assert!(broken(&|bytes| bytes[postings + 4..postings + 6].copy_from_slice(&(-1i16).to_le_bytes())));
    assert!(broken(&|bytes| bytes[postings + 6] = 5));
    assert!(broken(&|bytes| bytes[postings + 7] = 8));
    // Surfaces: not UTF-8, empty, outside the blob.
    let first_surface = blob + get_u32(&good, word_table) as usize;
    assert!(broken(&|bytes| bytes[first_surface] = 0xFF));
    assert!(broken(&|bytes| bytes[word_table + 4..word_table + 6].copy_from_slice(&0u16.to_le_bytes())));
    assert!(broken(&|bytes| put_u32(bytes, word_table, u32::MAX)));
    assert!(broken(&|bytes| bytes[word_table + 7] = 1));
    // Reviewed rows out of order, with an unknown flag, with weight 0.
    assert!(broken(&|bytes| {
        let (first, second) = (bytes[reviewed..reviewed + 16].to_vec(), bytes[reviewed + 16..reviewed + 32].to_vec());
        bytes[reviewed..reviewed + 16].copy_from_slice(&second);
        bytes[reviewed + 16..reviewed + 32].copy_from_slice(&first);
    }));
    assert!(broken(&|bytes| bytes[reviewed + 14] = 2));
    assert!(broken(&|bytes| bytes[reviewed + 12..reviewed + 14].copy_from_slice(&0i16.to_le_bytes())));

    // Order inside a key: せんせい holds 先生 (reviewed) before 先制 (heavier). Swapped, the
    // image still reads, but verify fails.
    let mut image = fixture_source();
    for posting in &mut image.postings {
        if posting.key == "せんせい" && posting.word == 9 {
            posting.flags &= !FLAG_REVIEWED;
            posting.weight = 7000;
        }
    }
    let (mut bytes, layout) = image.write_with_layout();
    assert!(open(bytes.clone()).verify());
    // Find the two postings (words 9 and 10) and swap their records.
    let table = layout.postings;
    let count = get_u32(&bytes, 20) as usize;
    let at = |word: u32| (0..count).find(|&index| get_u32(&bytes, table + 8 * index) == word).expect("posting");
    let (first, second) = (table + 8 * at(9), table + 8 * at(10));
    let (one, two) = (bytes[first..first + 8].to_vec(), bytes[second..second + 8].to_vec());
    bytes[first..first + 8].copy_from_slice(&two);
    bytes[second..second + 8].copy_from_slice(&one);
    assert!(!open(bytes).verify());

    // Equal surfaces stored at two offsets: the lookup could no longer drop the duplicate.
    let mut image = Image::default();
    let first = image.word("亀", 0);
    let second = image.word("亀亀", 0);
    image.posting("かめ", first, 5000, KIND_READING, 0);
    image.posting("kame", second, 4000, KIND_ALIAS_KANJI, 0);
    let (mut bytes, layout) = image.write_with_layout();
    assert!(open(bytes.clone()).verify());
    // Make the second word read the last three bytes of its own text: also 亀, at another offset.
    let offset = get_u32(&bytes, layout.words + 8);
    put_u32(&mut bytes, layout.words + 8, offset + 3);
    bytes[layout.words + 12..layout.words + 14].copy_from_slice(&3u16.to_le_bytes());
    assert!(!open(bytes).verify());

    // A section that does not start on an 8-byte boundary.
    let mut image = Image::default();
    let word = image.word("亀", 0);
    image.posting("かめ", word, 5000, KIND_READING, 0);
    let mut bytes = image.write();
    assert!(open(bytes.clone()).verify());
    let blob_at = get_u32(&bytes, 48) as usize;
    let blob_len = get_u32(&bytes, 52) as usize;
    let blob_bytes = bytes[blob_at..blob_at + blob_len].to_vec();
    bytes.truncate(blob_at);
    bytes.push(0);
    bytes.extend_from_slice(&blob_bytes);
    put_u32(&mut bytes, 48, blob_at as u32 + 1);
    let len = bytes.len() as u32;
    put_u32(&mut bytes, 56, len);
    let lexicon = open(bytes);
    assert_eq!(words(&lexicon, "kame", Japanese, 64), ["亀"]);
    assert!(!lexicon.verify());
}

#[test]
fn gate_is_applied_before_duplicates_are_dropped() {
    // The typed key holds 亀 without PINYIN_OK, the kana key holds the same surface with it.
    let mut image = Image::default();
    let alias = image.word("亀", 0);
    let reading = image.word("亀", 0);
    image.posting("kame", alias, 9000, KIND_AUTHORED, 0);
    image.posting("かめ", reading, 5000, KIND_READING, FLAG_PINYIN_OK);
    let lexicon = open(image.write());
    assert!(lexicon.verify());
    assert_eq!(words(&lexicon, "kame", Japanese, 64), ["亀"]);
    assert_eq!(first_row(&lexicon, "kame", Japanese).kind, KIND_AUTHORED);
    assert_eq!(words(&lexicon, "kame", Pinyin, 5), ["亀"]);
    assert_eq!(first_row(&lexicon, "kame", Pinyin).kind, KIND_READING);
}

#[test]
fn equal_rows_keep_the_stored_order_and_the_typed_key_goes_first() {
    let mut image = Image::default();
    for (key, surfaces) in [("kame", ["丙", "甲"]), ("かめ", ["乙", "丁"])] {
        for surface in surfaces {
            let word = image.word(surface, 0);
            image.posting(key, word, 5000, KIND_READING, 0);
        }
    }
    let reviewed = image.word("戊", FLAG_REVIEWED);
    image.posting("かめ", reviewed, 100, KIND_READING, FLAG_REVIEWED);
    let lexicon = open(image.write());
    assert!(lexicon.verify());
    // Stored order inside a key is by surface; 戊 is reviewed and goes first in spite of its weight.
    assert_eq!(words(&lexicon, "kame", Japanese, 64), ["戊", "丙", "甲", "丁", "乙"]);
}

#[test]
fn two_full_keys() {
    // 64 postings under the typed key and 64 under the kana key.
    let mut image = Image::default();
    for index in 0..64i16 {
        let word = image.word(&format!("甲{index}"), 0);
        image.posting("kame", word, 1000 + index, KIND_AUTHORED, 0);
        let word = image.word(&format!("乙{index}"), 0);
        image.posting("かめ", word, 5000 + index, KIND_READING, 0);
    }
    let lexicon = open(image.write());
    assert!(lexicon.verify());
    let all = words(&lexicon, "kame", Japanese, 1000);
    assert_eq!(all.len(), 128);
    assert_eq!((all[0].as_str(), all[63].as_str(), all[64].as_str(), all[127].as_str()), ("乙63", "乙0", "甲63", "甲0"));
    assert_eq!(words(&lexicon, "kame", Japanese, 64).len(), 64);
}

#[test]
fn double_n_retry() {
    let mut image = Image::default();
    let shinbun = image.word("新聞", 0);
    image.posting("しんぶん", shinbun, 5000, KIND_READING, FLAG_PINYIN_OK);
    let onna = image.word("女", 0);
    image.posting("おんな", onna, 5000, KIND_READING, FLAG_PINYIN_OK);
    let honnin = image.word("本人", 0);
    image.posting("ほんにん", honnin, 5000, KIND_READING, FLAG_PINYIN_OK);
    let lexicon = open(image.write());
    // んん in the kana, key without it: retried in Japanese mode only.
    assert_eq!(words(&lexicon, "shinnbunn", Japanese, 64), ["新聞"]);
    assert_eq!(words(&lexicon, "shinnbunn", Pinyin, 5), Vec::<String>::new());
    // nn before a vowel is ん + n-row kana and needs no retry.
    assert_eq!(words(&lexicon, "onna", Pinyin, 5), ["女"]);
    assert_eq!(words(&lexicon, "honnin", Pinyin, 5), ["本人"]);
    // Typed with nn for every ん: ほんんにんん.
    assert_eq!(words(&lexicon, "honnninn", Japanese, 64), ["本人"]);
}

/// Deterministic romaji strings of two to five syllables.
fn synthetic_romaji(count: usize, seed: u64) -> Vec<String> {
    const SYLLABLES: [&str; 60] = [
        "a", "i", "u", "e", "o", "ka", "ki", "ku", "ke", "ko", "sa", "shi", "su", "se", "so", "ta", "chi", "tsu",
        "te", "to", "na", "ni", "nu", "ne", "no", "ha", "hi", "fu", "he", "ho", "ma", "mi", "mu", "me", "mo", "ya",
        "yu", "yo", "ra", "ri", "ru", "re", "ro", "wa", "ga", "gi", "gu", "ge", "go", "ji", "da", "ba", "kyo",
        "shou", "chuu", "ryo", "kan", "sen", "tou", "kai",
    ];
    let mut state = seed;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(count);
    while out.len() < count {
        let mut text = String::new();
        for _ in 0..(2 + next() % 4) {
            text.push_str(SYLLABLES[(next() % 60) as usize]);
        }
        if seen.insert(text.clone()) {
            out.push(text);
        }
    }
    out
}

/// 120,000 keys: 100,000 kana readings and 20,000 ASCII keys. Half of the ASCII keys are the
/// romaji of a kana key (both routes hit), half start with `q` (only the typed key hits).
fn synthetic_image() -> (Vec<u8>, Vec<String>) {
    let romaji = synthetic_romaji(100_000, 0x9E37_79B9_7F4A_7C15);
    let mut image = Image::default();
    let mut seen = std::collections::HashSet::new();
    let mut inputs = Vec::new();
    for (index, text) in romaji.iter().enumerate() {
        let mut kana = String::new();
        let result = to_kana(text, &mut kana);
        assert!(result.complete && result.strict);
        let mut key = String::new();
        normalise_key(&kana, &mut key);
        if !seen.insert(key.clone()) {
            continue;
        }
        inputs.push(text.clone());
        for copy in 0..(1 + index % 3) {
            let word = image.word(&format!("語{index}の{copy}"), 0);
            let flags = if copy == 0 { FLAG_PINYIN_OK } else { 0 };
            image.posting(&key, word, 6000 - copy as i16, KIND_READING, flags);
        }
        if index % 10 == 0 {
            let word = image.word(&format!("仮{index}"), FLAG_REVIEWED);
            image.posting(text, word, 8000, KIND_AUTHORED, FLAG_PINYIN_OK | FLAG_REVIEWED);
        } else if index % 10 == 1 {
            let word = image.word(&format!("別{index}"), 0);
            let alias = format!("q{text}");
            image.posting(&alias, word, 3000, KIND_ALIAS_KANJI, FLAG_PINYIN_OK);
            inputs.push(alias);
        }
    }
    (image.write(), inputs)
}

/// Lookup time on a synthetic image of 120,000 keys, or on the image named by `MIMO_JP_IMAGE`
/// (extra inputs, one per line, from the file named by `MIMO_JP_BENCH_INPUTS`).
/// `cargo test -p meowpuut-core --release --test japanese jp_lookup_bench -- --ignored --nocapture`
#[test]
#[ignore = "timing; run with --release"]
fn jp_lookup_bench() {
    let (bytes, mut inputs) = match std::env::var_os("MIMO_JP_IMAGE") {
        Some(path) => (std::fs::read(&path).expect("MIMO_JP_IMAGE"), synthetic_romaji(20_000, 7)),
        None => synthetic_image(),
    };
    if let Some(path) = std::env::var_os("MIMO_JP_BENCH_INPUTS") {
        inputs.extend(std::fs::read_to_string(&path).expect("MIMO_JP_BENCH_INPUTS").lines().map(str::to_string));
    }
    let size = bytes.len();
    let lexicon = open(bytes);
    assert!(lexicon.verify(), "image does not pass verify");
    println!("image: {} keys, {} words, {size} bytes", lexicon.key_count(), lexicon.word_count());
    // Hits, misses, and every prefix of a part of the inputs (what typing produces).
    let misses = synthetic_romaji(inputs.len() / 2, 99);
    let typed: Vec<String> =
        inputs.iter().step_by(20).flat_map(|text| (1..=text.len()).map(|end| text[..end].to_string())).collect();
    let mut query = JapaneseQuery::new();
    for (name, list) in [("stored keys", &inputs), ("other romaji", &misses), ("typed prefixes", &typed)] {
        for (mode, limit) in [(Japanese, 64), (Pinyin, 5)] {
            let mut found = 0usize;
            let mut best = f64::MAX;
            for _ in 0..5 {
                let start = std::time::Instant::now();
                for input in list {
                    found += std::hint::black_box(query.lookup(&lexicon, std::hint::black_box(input), mode, limit));
                }
                best = best.min(start.elapsed().as_secs_f64() * 1e6 / list.len() as f64);
            }
            println!("{name:15} {mode:?}: {best:.3} us per lookup ({} lookups, {} rows per pass)", list.len(), found / 5);
            assert!(best < 10.0, "{name} {mode:?}: {best:.3} us per lookup");
        }
    }
}

#[test]
fn synthetic_image_is_valid() {
    // A smaller image from the same generator as the bench, so that the generator stays correct.
    let romaji = synthetic_romaji(3000, 5);
    let mut image = Image::default();
    let mut seen = std::collections::HashSet::new();
    let mut kept = Vec::new();
    for (index, text) in romaji.iter().enumerate() {
        let mut kana = String::new();
        to_kana(text, &mut kana);
        let mut key = String::new();
        normalise_key(&kana, &mut key);
        if seen.insert(key.clone()) {
            let word = image.word(&format!("語{index}"), 0);
            image.posting(&key, word, 5000, KIND_READING, FLAG_PINYIN_OK);
            kept.push((text.clone(), format!("語{index}")));
        }
    }
    let lexicon = open(image.write());
    assert!(lexicon.verify());
    assert_eq!(lexicon.key_count(), kept.len());
    for (text, surface) in &kept {
        assert_eq!(words(&lexicon, text, Pinyin, 5), [surface.as_str()], "{text}");
    }
}
