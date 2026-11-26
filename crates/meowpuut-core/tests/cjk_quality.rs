#![forbid(unsafe_code)]
//! Regression tests for Japanese long sentences & sentence-end completion,
//! long mixed CJK input, and Pinyin typo / omission recovery.

#[path = "support/mimo_jp_writer.rs"]
mod writer;

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use meowpuut_core::japanese::{
    JapaneseLexicon, JapaneseMode, JapaneseQuery, OPTION_COMPLETIONS, OPTION_FORMS, OPTION_SEGMENTS,
};
use meowpuut_core::pairs::Pairs;
use meowpuut_core::{Lexicon, Session};
use writer::Image;

const ALL_JP_OPTIONS: u32 = OPTION_FORMS | OPTION_COMPLETIONS | OPTION_SEGMENTS;

fn build_jp_fixture() -> JapaneseLexicon {
    let spec = include_str!("data/mimo_jp_forms_fixture.tsv");
    JapaneseLexicon::from_vec(Image::parse(spec).write()).unwrap()
}

#[test]
fn japanese_long_sentence_segments_and_polite_endings() {
    let jp = build_jp_fixture();
    let mut q = JapaneseQuery::new();

    // Long (>55 bytes) sentence must not exhaust MAX_SEGMENT_ATTEMPTS before reaching "わたしは".
    let long = "watashihakyouwotenkiwoonegaishimasutabemashitatabenakatta";
    let count = q.lookup_extended(&jp, long, JapaneseMode::Japanese, 16, ALL_JP_OPTIONS);
    assert!(count > 0, "long Japanese sentence returned 0 candidates");
    assert_eq!(q.text(0), "私は");
    assert_eq!(q.consumed(0), 9);

    // Polite / sentence-end completion from pending auxiliary stem.
    for (input, expected) in [
        ("tabemas", "食べます"),
        ("tabemash", "食べました"),
        ("tenkides", "天気です"),
        ("tenkidesun", "天気ですね"),
        ("onegaishim", "お願いします"),
    ] {
        let n = q.lookup_extended(&jp, input, JapaneseMode::Japanese, 16, ALL_JP_OPTIONS);
        let texts: Vec<_> = (0..n).map(|i| q.text(i)).collect();
        assert!(texts.contains(&expected), "input {input:?} missed {expected:?} in {texts:?}");
    }
}

#[test]
fn mixed_long_sentence_preserves_late_japanese_word() {
    let tsv = concat!(
        "MEOWPUUT-PINYIN-1\n",
        "de\tde5\t的\t的\t5500\n",
        "hao\thao3\t好\t好\t5400\n",
        "hen\then3\t很\t很\t5200\n",
        "keai\tke3ai4\t可愛\t可爱\t5400\n",
        "ni\tni3\t你\t你\t5500\n",
        "wo\two3\t我\t我\t5600\n",
        "xihuan\txi3huan1\t喜歡\t喜欢\t5500\n",
    );
    let lex = Arc::new(Lexicon::load(tsv).unwrap());
    let mut s = Session::new(lex);
    assert!(s.add_japanese_word("neko", "猫", 7000));
    assert!(s.add_japanese_word("suki", "好き", 7000));

    // Many repeated Chinese syllables (>50 bytes) followed by late Japanese word.
    let long_mixed = "woxihuanniwoxihuanniwoxihuanniwoxihuannidenekohenkeai";
    let out = s.mixed_pinyin(long_mixed, false);
    assert!(out.contains("猫"), "late Japanese word missing in {out:?}");
}

#[test]
fn shipped_pinyin_neighbor_and_omit_typos_and_nine_key_eval() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.build/lexicon");
    if !dir.join("mimo_base.bin").exists() {
        return;
    }
    let base: &'static [u8] = Box::leak(fs::read(dir.join("mimo_base.bin")).unwrap().into_boxed_slice());
    let pairs: &'static [u8] = Box::leak(fs::read(dir.join("mimo_pairs.bin")).unwrap().into_boxed_slice());
    let mut lex = Lexicon::from_base_in_place(base, true, None).unwrap();
    lex.set_pairs(Pairs::in_place(pairs, &lex.index().word_ids, None).unwrap());
    let arc_lex = Arc::new(lex);

    let mut s = Session::new(Arc::clone(&arc_lex));
    for (typed, expected) in [
        ("nangul", "難過"),
        ("texiap", "特效"),
        ("shijiabugou", "時間不夠"),
    ] {
        s.clear();
        for b in typed.bytes() {
            assert!(s.key(i32::from(b)));
        }
        let top5: Vec<_> = (0..s.candidate_count().min(5)).map(|i| s.candidate(i)).collect();
        assert!(top5.contains(&expected), "{typed} expected {expected} in top5 {top5:?}");
    }

    // Nine-key shipped check: trailing vowel trap + Stage-2 trigram + zero hot allocs.
    let mut nk = Session::new_nine_key(arc_lex).unwrap();
    nk.set_simplified(true);
    for b in b"325423683" {
        assert!(nk.key(i32::from(*b)));
    }
    let warm = nk.capacity_signature();
    assert_eq!(nk.candidate(0), "大家都", "trailing digit 3 should keep 大家都 ahead of 大家都饿");
    nk.clear();
    for b in b"4349652494926" {
        assert!(nk.key(i32::from(*b)));
    }
    assert_eq!(nk.capacity_signature(), warm);
}
