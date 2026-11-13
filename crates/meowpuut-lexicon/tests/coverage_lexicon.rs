#![forbid(unsafe_code)]
//! Comprehensive branch coverage tests for `meowpuut-lexicon`.

use meowpuut_lexicon::compiler::{compile_tsv, merge_catalogs, parse_japanese_tsv, JapaneseLexiconSoA};
use meowpuut_lexicon::evidence::{AssociationTableSoA, EnglishEvidenceSoA, WordEvidenceSoA};
use meowpuut_lexicon::phonetics::{
    encode_syllable, hiragana_to_katakana, numbered_pinyin, pinyin_kana, romaji_to_hiragana,
    strip_pinyin_tones,
};
use meowpuut_lexicon::soa_columns::{calibrate_weight, ContextColumnSoA, LexiconCatalogSoA};

#[test]
fn test_phonetics_all_branches() {
    assert_eq!(encode_syllable("ㄋㄧˇ"), "su3");
    assert_eq!(encode_syllable("ㄓㄨㄥ"), "5j/ ");
    assert_eq!(encode_syllable("˙ㄇㄚ"), "7a8");
    assert_eq!(encode_syllable("ㄅㄚˉ"), "18 ");
    assert_eq!(encode_syllable("ㄆㄚˊ"), "q86");
    assert_eq!(encode_syllable("ㄇㄚˋ"), "a84");

    assert_eq!(
        numbered_pinyin("ā á ǎ à ē é ě è ī í ǐ ì ō ó ǒ ò ū ú ǔ ù ǖ ǘ ǚ ǜ ü"),
        "a1a2a3a4e1e2e3e4i1i2i3i4o1o2o3o4u1u2u3u4v1v2v3v4v"
    );
    assert_eq!(numbered_pinyin("zhong1'guo2"), "zhong1guo2");
    assert_eq!(strip_pinyin_tones("ni3'hao3"), "nihao");

    for syl in [
        "a", "i", "u", "e", "o", "wo", "ka", "ki", "ku", "ke", "ko", "sa", "shi", "su", "se", "so",
        "ta", "chi", "tsu", "te", "to", "na", "ni", "nu", "ne", "no", "ha", "hi", "fu", "he", "ho",
        "ma", "mi", "mu", "me", "mo", "ya", "yu", "yo", "ra", "ri", "ru", "re", "ro", "wa", "ga",
        "gi", "gu", "ge", "go", "za", "ji", "zu", "ze", "zo", "da", "de", "do", "ba", "bi", "bu",
        "be", "bo", "pa", "pi", "pu", "pe", "po", "hao",
    ] {
        assert!(pinyin_kana(syl).is_some());
    }
    assert!(pinyin_kana("unknown").is_none());

    assert_eq!(romaji_to_hiragana("kyashachanyahyamyaryagyabyapyajya"), "きゃしゃちゃにゃひゃみゃりゃぎゃびゃぴゃじゃ");
    assert_eq!(romaji_to_hiragana("nn n' n kka tto ramen-"), "ん ん ん っか っと らめんー");
    assert_eq!(romaji_to_hiragana("xyz"), "xyz");
    assert_eq!(hiragana_to_katakana("わたしーabc"), "ワタシーabc");
}

#[test]
fn test_soa_columns_and_compiler_all_branches() {
    for len in 0..=6 {
        let w = calibrate_weight(500, 3000, len);
        assert!((1200..=6200).contains(&w));
    }

    let mut cat = LexiconCatalogSoA::with_capacity(8, 128);
    assert!(cat.is_empty());
    cat.push_entry("ni", "ni3", "你", "你", 5200, 1);
    cat.push_entry("ni", "ni2", "泥", "泥", 5150, 1);
    cat.push_entry("ni", "ni3", "你", "你", 5400, 1);
    cat.push_entry("nihao", "ni3hao3", "你好", "你好", 5500, 1);
    cat.push_entry("he", "han4", "和", "和", 4000, 1);
    cat.push_entry("he", "he2", "和", "和", 4000, 1);
    cat.push_entry("zhe", "zhe5", "著", "着", 4000, 1);
    cat.push_entry("zhu", "zhu4", "著", "着", 4000, 1);
    cat.push_entry("huo", "huo4", "和", "和", 4000, 1);
    cat.apply_tiered_heterophony(
        &[("和", "han4"), ("著", "zhe5")],
        &[("和", "he2")],
        &[("著", "zhu4")],
    );
    cat.apply_polyphony_discount();
    cat.sort_and_dedup();
    assert_eq!(cat.len(), 8);

    let mut overlay = LexiconCatalogSoA::with_capacity(4, 64);
    overlay.push_entry("nihao", "ni3hao3", "你好", "你好", 5800, 2);
    overlay.push_entry("ni", "ni3", "你", "你", 4000, 2);
    overlay.push_entry("shijie", "shi4jie4", "世界", "世界", 5300, 2);
    let rep = merge_catalogs(&mut cat, &overlay);
    assert_eq!(rep.added, 1);
    assert_eq!(rep.updated, 1);
    assert_eq!(rep.unchanged, 1);

    let mut ctx = ContextColumnSoA::with_capacity(4, 64);
    assert!(ctx.is_empty());
    ctx.push("世界", 500);
    ctx.push("你好", 600);
    assert_eq!(ctx.len(), 2);

    let py_tsv = compile_tsv(true, &mut cat, &ctx);
    assert!(py_tsv.starts_with("MEOWPUUT-PINYIN-1\n"));
    let zy_tsv = compile_tsv(false, &mut cat, &ContextColumnSoA::default());
    assert!(zy_tsv.starts_with("MEOWPUUT-ZHUYIN-1\n"));
    let py_bin = meowpuut_lexicon::compile_binary(true, &mut overlay, &ctx).unwrap();
    let loaded_bin = meowpuut_core::Lexicon::load_binary(&py_bin).unwrap();
    assert!(loaded_bin.pinyin);

    assert!(parse_japanese_tsv("BAD\n").is_none());
    let jp_tsv = "MEOWPUUT-JAPANESE-1\n# comment\nwatashi\t私\t5500\nわたし\t私\t5500\nneko\t猫\t5200\n";
    let jp = parse_japanese_tsv(jp_tsv).unwrap();
    assert!(!jp.is_empty());
    assert!(jp.lookup("").is_empty());
    let res = jp.lookup("watashineko");
    assert!(!res.is_empty());
    let empty_jp = JapaneseLexiconSoA::with_capacity(0, 0);
    assert!(empty_jp.is_empty());
}

#[test]
fn test_evidence_and_associations_all_branches() {
    let ev = WordEvidenceSoA::from_pairs(&[
        ("", 100),
        ("你好", 5000),
        ("你好", 5400),
        ("世界", 5200),
        ("早安", 4900),
    ]);
    assert_eq!(ev.len(), 3);
    assert!(!ev.is_empty());
    assert_eq!(ev.weight("你好"), Some(5400));
    assert_eq!(ev.weight("早安"), Some(4900));
    assert_eq!(ev.weight("不存在"), None);
    assert!(ev.contains("世界"));

    let eng = EnglishEvidenceSoA::default_builtin();
    assert!(!eng.is_empty());
    assert!(eng.candidates("", 5).is_empty());
    assert!(eng.candidates("the", 0).is_empty());
    let lower_cands = eng.candidates("teh", 5);
    assert!(lower_cands.contains(&String::from("the")));
    let pref_cands = eng.candidates("th", 2);
    assert_eq!(pref_cands.len(), 2);
    let title_cands = eng.candidates("Teh", 5);
    assert!(title_cands.contains(&String::from("The")));
    let upper_cands = eng.candidates("TEH", 5);
    assert!(upper_cands.contains(&String::from("THE")));

    let assoc = AssociationTableSoA::default_builtin();
    assert!(!assoc.lookup("你好", false).is_empty());
    assert!(!assoc.lookup("你好", true).is_empty());
    assert!(assoc.lookup("無關", false).is_empty());
}
