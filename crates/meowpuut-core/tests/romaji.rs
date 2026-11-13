//! Romaji conversion against the Swift table, the strict flag, and key normalisation.
//!
//! Data files in `tests/data`:
//! - `romaji_golden.tsv` (`input<TAB>kana`): output of `Romaji.reading` in
//!   `Sources/JapaneseBackend.swift` (made with `romaji_golden_main.swift`). No input holds `dhu`
//!   or `thu`: the Rust table converts those two and the Swift table does not.
//! - `romaji_strict_cases.tsv` (`input<TAB>0 or 1`): hand-written.
//! - `normalise_key_cases.tsv` (`kana<TAB>key`): hand-written.
//! - `mimo_jp_snapshot_sample.tsv`: rows in the snapshot shape for the round-trip property.
//!
//! The Python port (`scripts/japanese_readings.py`) is checked against the same files.

use std::path::{Path, PathBuf};

use meowpuut_core::romaji::{normalise_key, to_kana, to_katakana, KanaResult};

fn data(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn kana(input: &str) -> (String, KanaResult) {
    let mut out = String::new();
    let result = to_kana(input, &mut out);
    (out, result)
}

fn key(text: &str) -> String {
    let mut out = String::new();
    normalise_key(text, &mut out);
    out
}

#[test]
fn golden_matches_the_swift_table() {
    let golden = data("romaji_golden.tsv");
    let mut rows = 0;
    for line in golden.lines() {
        let (input, expected) = line.split_once('\t').expect("input<TAB>kana");
        let (out, result) = kana(input);
        assert_eq!(out, expected, "input {input:?}");
        let letters_left = out.bytes().any(|byte| byte.is_ascii_alphabetic());
        assert_eq!(result.complete, !letters_left, "complete flag for {input:?}");
        rows += 1;
    }
    assert!(rows >= 40_000, "golden file has only {rows} rows");
}

#[test]
fn strict_cases() {
    let cases = data("romaji_strict_cases.tsv");
    let mut rows = 0;
    for line in cases.lines() {
        let (input, expected) = line.split_once('\t').expect("input<TAB>0 or 1");
        let (out, result) = kana(input);
        assert_eq!(result.strict, expected == "1", "strict flag for {input:?} ({out})");
        rows += 1;
    }
    assert!(rows >= 80, "strict case file has only {rows} rows");
}

#[test]
fn strict_implies_complete() {
    for line in data("romaji_golden.tsv").lines() {
        let (input, _) = line.split_once('\t').expect("input<TAB>kana");
        let (out, result) = kana(input);
        assert!(!result.strict || result.complete, "{input:?} gives {out}: strict but not complete");
    }
}

#[test]
fn added_tokens() {
    assert_eq!(kana("dhu"), ("でゅ".to_string(), KanaResult { complete: true, strict: true }));
    assert_eq!(kana("thu"), ("てゅ".to_string(), KanaResult { complete: true, strict: true }));
    assert_eq!(kana("dhuo").0, "でゅお");
}

#[test]
fn edges() {
    assert_eq!(kana(""), (String::new(), KanaResult { complete: true, strict: true }));
    // Appends; does not clear.
    let mut out = String::from("x");
    to_kana("ka", &mut out);
    assert_eq!(out, "xか");
    // Capitals are read as lower case.
    assert_eq!(kana("KaWaIi").0, "かわいい");
    // Text that is not romaji is copied and is never strict; letters left make it incomplete.
    assert_eq!(kana("猫ka"), ("猫か".to_string(), KanaResult { complete: true, strict: false }));
    assert_eq!(kana("kak"), ("かk".to_string(), KanaResult { complete: false, strict: false }));
    assert_eq!(kana("a1"), ("あ1".to_string(), KanaResult { complete: true, strict: false }));
}

#[test]
fn katakana() {
    let mut out = String::new();
    to_katakana("こーひー と ゔぁいおりん、ぁゖ 猫 abc", &mut out);
    assert_eq!(out, "コーヒー ト ヴァイオリン、ァヶ 猫 abc");
}

#[test]
fn normalise_cases() {
    let cases = data("normalise_key_cases.tsv");
    let mut rows = 0;
    for line in cases.lines() {
        let (input, expected) = line.split_once('\t').expect("kana<TAB>key");
        assert_eq!(key(input), expected, "key of {input:?}");
        assert_eq!(key(expected), expected, "the key of a key is the key ({input:?})");
        rows += 1;
    }
    assert!(rows >= 40, "normalise case file has only {rows} rows");
}

/// Rows of a snapshot file (design 6.6): `kana|surface|romaji|…`, eight columns.
/// Returns the number of rows with a romaji value. The old five-column file is skipped.
fn check_round_trip(path: &Path) -> usize {
    let text = std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let mut checked = 0;
    for line in text.lines().filter(|line| !line.starts_with('#') && !line.is_empty()) {
        let fields: Vec<&str> = line.split('|').collect();
        if fields.len() != 8 || fields[2].is_empty() {
            continue;
        }
        let (out, result) = kana(fields[2]);
        assert!(result.complete && result.strict, "{line}: romaji gives {out} ({result:?})");
        assert_eq!(key(&out), key(fields[0]), "{line}: romaji gives {out}");
        checked += 1;
    }
    checked
}

#[test]
fn snapshot_romaji_round_trips() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let sample = check_round_trip(&dir.join("tests/data/mimo_jp_snapshot_sample.tsv"));
    assert!(sample >= 300, "sample has only {sample} rows with romaji");
    // The snapshot of the repo, or the file named by MIMO_JP_SNAPSHOT.
    let snapshot = std::env::var_os("MIMO_JP_SNAPSHOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| dir.join("../../DataSources/mimo-jp/mimo-jp.tsv"));
    if snapshot.exists() {
        let rows = check_round_trip(&snapshot);
        println!("{}: {rows} rows with romaji checked", snapshot.display());
    }
}
