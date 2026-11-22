//! The Japanese next-word image: what is refused when opening, what a prediction reads from
//! the committed pieces, that a damaged image cannot make it panic, and the shipped table.

#[path = "support/mimo_jn_writer.rs"]
mod mimo_jn_writer;

use std::path::PathBuf;

use meowpuut_core::japanese_next::{JapaneseNext, NextQuery, FLAG_TAIL, MAX_RESULTS, MAX_TEXT_BYTES};
use mimo_jn_writer::{header_field, image, Key};

fn fixture() -> Vec<Key> {
    vec![
        Key::new("ありがとう", 0, &[("ございます", 228), ("ございました", 132), ("ね", 6)]),
        Key::new("ご飯", 0, &[("を", 40), ("は", 12)]),
        Key::new("ご飯を", 0, &[("食べる", 11), ("作る", 3), ("は", 1)]),
        Key::new("を", FLAG_TAIL, &[("した", 9), ("食べる", 5), ("見た", 4)]),
        Key::new("た", FLAG_TAIL, &[("の", 30), ("こと", 20)]),
        Key::new("ました", FLAG_TAIL, &[("か", 733), ("が", 66)]),
        // A content word that happens to end other words: never matched as an ending.
        Key::new("した", 0, &[("ところ", 3)]),
    ]
}

fn texts(query: &NextQuery) -> Vec<&str> {
    (0..query.len()).map(|index| query.text(index)).collect()
}

fn predict(next: &JapaneseNext, context: &str, limit: usize) -> Vec<String> {
    let mut query = NextQuery::new();
    let count = query.predict(next, context, limit);
    assert_eq!(count, query.len());
    assert_eq!(query.is_empty(), count == 0);
    texts(&query).into_iter().map(str::to_owned).collect()
}

#[test]
fn opens_only_a_well_formed_image() {
    let good = image(&fixture());
    let next = JapaneseNext::from_vec(good.clone()).expect("fixture opens");
    assert!(next.verify());
    assert_eq!((next.key_count(), next.next_count()), (7, 16));
    let leaked: &'static [u8] = Box::leak(good.clone().into_boxed_slice());
    assert!(JapaneseNext::from_static(leaked).is_some_and(|next| next.verify()));

    assert!(JapaneseNext::from_vec(Vec::new()).is_none());
    assert!(JapaneseNext::from_vec(good[..40].to_vec()).is_none());
    let damaged = |change: &dyn Fn(&mut Vec<u8>)| {
        let mut bytes = good.clone();
        change(&mut bytes);
        JapaneseNext::from_vec(bytes)
    };
    assert!(damaged(&|bytes| bytes[0] = b'X').is_none(), "magic");
    assert!(damaged(&|bytes| header_field(bytes, 12, 2)).is_none(), "version");
    assert!(damaged(&|bytes| bytes.push(0)).is_none(), "file_len differs from the size");
    assert!(damaged(&|bytes| header_field(bytes, 16, 1 << 28)).is_none(), "keys beyond the file");
    assert!(damaged(&|bytes| header_field(bytes, 24, 8)).is_none(), "keys inside the header");
    // The next records moved onto the keys.
    assert!(damaged(&|bytes| header_field(bytes, 28, 48)).is_none(), "overlapping sections");
    assert!(damaged(&|bytes| header_field(bytes, 36, u32::MAX)).is_none(), "blob length overflows");
}

#[test]
fn reads_the_last_two_pieces_then_the_last_then_its_ending() {
    let next = JapaneseNext::from_vec(image(&fixture())).unwrap();
    assert_eq!(predict(&next, "ありがとう", 8), ["ございます", "ございました", "ね"]);
    // Both pieces together come first; what the last piece alone adds follows, each word once.
    assert_eq!(predict(&next, "ご飯\u{1f}を", 8), ["食べる", "作る", "は", "した", "見た"]);
    // Only the last two pieces are read.
    assert_eq!(predict(&next, "今日\u{1f}ご飯\u{1f}を", 8), ["食べる", "作る", "は", "した", "見た"]);
    // A piece committed whole is found as a key, then its ending adds the rest.
    assert_eq!(predict(&next, "ご飯を", 8), ["食べる", "作る", "は", "した", "見た"]);
    // An unknown word gets the continuations of its longest listed ending: ました, not た.
    assert_eq!(predict(&next, "食べました", 8), ["か", "が"]);
    assert_eq!(predict(&next, "書いた", 8), ["の", "こと"]);
    // した is a key without the ending flag, so 勉強した falls through to た.
    assert_eq!(predict(&next, "勉強した", 8), ["の", "こと"]);
    // The piece itself is a key and is never read as its own ending twice.
    assert_eq!(predict(&next, "した", 8), ["ところ", "の", "こと"]);
    assert_eq!(predict(&next, "猫", 8), Vec::<String>::new());
}

#[test]
fn limits_and_empty_contexts() {
    let next = JapaneseNext::from_vec(image(&fixture())).unwrap();
    assert_eq!(predict(&next, "ありがとう", 2), ["ございます", "ございました"]);
    assert_eq!(predict(&next, "ご飯\u{1f}を", 4), ["食べる", "作る", "は", "した"]);
    assert!(predict(&next, "ありがとう", 0).is_empty());
    assert!(predict(&next, "", 8).is_empty());
    assert!(predict(&next, "\u{1f}\u{1f}", 8).is_empty());
    // Empty pieces are skipped: the piece before the separator is the last one.
    assert_eq!(predict(&next, "ありがとう\u{1f}", 1), ["ございます"]);
    // A piece longer than any key still gets its ending.
    let long = "あ".repeat(MAX_TEXT_BYTES) + "ました";
    assert_eq!(predict(&next, &long, 8), ["か", "が"]);
    assert_eq!(predict(&next, &format!("{long}\u{1f}{long}"), 8), ["か", "が"]);
    // More rows than a query holds.
    let many: Vec<(String, u16)> = (0..12).map(|index| (format!("語{index}"), 20 - index)).collect();
    let rows: Vec<(&str, u16)> = many.iter().map(|(text, score)| (text.as_str(), *score)).collect();
    let wide = vec![Key::new("あ", 0, &rows), Key::new("い", 0, &rows), Key::new("あい", 0, &rows)];
    let wide = JapaneseNext::from_vec(image(&wide)).unwrap();
    assert_eq!(predict(&wide, "あ\u{1f}い", usize::MAX).len(), 12.min(MAX_RESULTS));
    let mut query = NextQuery::default();
    query.predict(&wide, "あ", 3);
    assert_eq!((query.score(0), query.score(2), query.score(3), query.text(3)), (20, 18, 0, ""));
}

#[test]
fn a_damaged_image_gives_fewer_rows_and_never_panics() {
    let good = image(&fixture());
    let reference = JapaneseNext::from_vec(good.clone()).unwrap();
    let contexts = ["ありがとう", "ご飯\u{1f}を", "食べました", "勉強した", "猫"];
    // Every single byte of the tables and the blob flipped, one at a time.
    for at in 48..good.len() {
        let mut bytes = good.clone();
        bytes[at] ^= 0xff;
        let Some(next) = JapaneseNext::from_vec(bytes) else { continue };
        let _ = next.verify();
        for context in contexts {
            assert!(predict(&next, context, 8).len() <= MAX_RESULTS);
        }
    }
    // Header counts that still fit the file but disagree with the records.
    for (field, value) in [(16u32, 3u32), (20, 2), (36, 4)] {
        let mut bytes = good.clone();
        header_field(&mut bytes, field as usize, value);
        if let Some(next) = JapaneseNext::from_vec(bytes) {
            assert!(!next.verify());
            for context in contexts {
                let _ = predict(&next, context, 8);
            }
        }
    }
    assert!(reference.verify());
}

#[test]
fn verify_names_what_the_writer_guarantees() {
    let broken = |rows: &[Key], change: &dyn Fn(&mut Vec<u8>)| {
        let mut bytes = image(rows);
        change(&mut bytes);
        !JapaneseNext::from_vec(bytes).expect("header still valid").verify()
    };
    let rows = fixture();
    let keys_at = 48;
    // Key record: u32 offset, u16 len, u8 count, u8 flags, u32 first.
    assert!(broken(&rows, &|bytes| bytes[keys_at + 7] = 2), "unknown flag");
    assert!(broken(&rows, &|bytes| bytes[keys_at + 6] = 0), "a key without next words");
    assert!(broken(&rows, &|bytes| bytes[keys_at + 6] = 13), "too many next words");
    assert!(broken(&rows, &|bytes| bytes[keys_at + 8] = 1), "first_next out of step");
    assert!(broken(&rows, &|bytes| bytes[keys_at + 4] = 0), "empty key");
    // The first two key records swapped: no longer sorted.
    assert!(broken(&rows, &|bytes| {
        let (first, second) = (bytes[keys_at..keys_at + 8].to_vec(), bytes[keys_at + 12..keys_at + 20].to_vec());
        bytes[keys_at..keys_at + 8].copy_from_slice(&second);
        bytes[keys_at + 12..keys_at + 20].copy_from_slice(&first);
    }), "unsorted keys");
    let nexts_at = u32::from_le_bytes(image(&rows)[28..32].try_into().unwrap()) as usize;
    assert!(broken(&rows, &|bytes| bytes[nexts_at + 6..nexts_at + 8].copy_from_slice(&[0, 0])), "score 0");
    assert!(broken(&rows, &|bytes| bytes[nexts_at + 4..nexts_at + 6].copy_from_slice(&[0, 0])), "empty next text");
    // One next record more in the header than the keys account for.
    let mut spare = image(&rows);
    let count = u32::from_le_bytes(spare[20..24].try_into().unwrap());
    let blob_at = u32::from_le_bytes(spare[32..36].try_into().unwrap());
    if nexts_at as u32 + (count + 1) * 8 <= blob_at {
        header_field(&mut spare, 20, count + 1);
        assert!(!JapaneseNext::from_vec(spare).unwrap().verify(), "unowned next record");
    }
    let long_key = "あ".repeat(MAX_TEXT_BYTES / 3 + 1);
    assert!(broken(&[Key::new(&long_key, 0, &[("い", 1)])], &|_| {}), "key over the limit");
    assert!(broken(&[Key::new("あ", 0, &[("い", 1)])], &|bytes| {
        let blob_at = u32::from_le_bytes(bytes[32..36].try_into().unwrap()) as usize;
        bytes[blob_at] = 0xff;
    }), "key that is not UTF-8");
}

/// The table the build ships (`scripts/build-lexicon.sh`, or `scripts/mimo_jp_next.py pack`).
fn shipped() -> Option<JapaneseNext> {
    let path = std::env::var_os("MIMO_JP_NEXT_BIN").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.build/lexicon/mimo_jp_next.bin")
    });
    let bytes = std::fs::read(&path).unwrap_or_else(|_| panic!("{} missing; run scripts/build-lexicon.sh", path.display()));
    JapaneseNext::from_vec(bytes)
}

#[test]
fn shipped_table_offers_what_people_write_next() {
    let next = shipped().expect("the shipped table opens");
    assert!(next.verify());
    assert!(next.key_count() > 10_000);
    let first = |context: &str| predict(&next, context, 8).into_iter().next().unwrap_or_default();
    assert_eq!(first("ありがとう"), "ございます");
    assert_eq!(first("おはよう"), "ございます");
    assert_eq!(first("よろしく"), "お願いします");
    assert_eq!(first("今日"), "は");
    assert_eq!(first("お疲れ様"), "です");
    assert_eq!(first("ご飯\u{1f}を"), "食べる");
    // 召し上がりました is not a key; its ending is.
    assert_eq!(first("召し上がりました"), "か");
    assert!(predict(&next, "了解", 8).contains(&"しました".to_owned()));
    // No name of the example sentences is offered anywhere.
    for context in ["は", "が", "と", "私", "昨日"] {
        for word in predict(&next, context, MAX_RESULTS) {
            assert!(!word.starts_with("トム") && !word.starts_with("メアリー"), "{context} offers {word}");
        }
    }
}
