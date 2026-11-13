#![forbid(unsafe_code)]
//! Comprehensive branch coverage tests for `meowpuut-core` covering edge cases
//! in `soa`, `reading`, `workspace`, `lexicon`, `fallback`, `decoder`, `segments`,
//! `mixed`, `mixed_prefix`, `session`, and `refresh`.

use std::sync::Arc;

use meowpuut_core::decoder::{decode_into, has_exact_match};
use meowpuut_core::fallback::{fallback_into, unfinished_phrase_into};
use meowpuut_core::mixed::{mixed_pinyin_into, mixed_pinyin_prefix_into, unfinished_japanese_prefix_into};
use meowpuut_core::reading::{
    han_characters_stack, is_tone, strict_han_count, utf8_char_count,
    zhuyin_symbol_for_key, Reading, MAX_INPUT,
};
use meowpuut_core::segments::{
    incomplete_pinyin_syllable, pinyin_segments_into, predictive_segments_into,
    unfinished_sentence_into, zhuyin_segments_into,
};
use meowpuut_core::soa::{BucketTableSoA, CandidatePoolSoA, ContextTableSoA, JapaneseTableSoA, WordTableSoA};
use meowpuut_core::workspace::{DecodeOutputSoA, DecoderScratchSoA, NasalVariantsStack};
use meowpuut_core::{max_input_bytes, CandidateInfo, CandidateKind, CandidateScoreKind, Lexicon, Session};

const SAMPLE_PINYIN_TSV: &str = concat!(
    "MEOWPUUT-PINYIN-1\n",
    "ai\tai4\t愛\t爱\t5200\n",
    "an\tan1\t安\t安\t4900\n",
    "ban\tban4\t辦\t办\t4700\n",
    "bang\tbang1\t幫\t帮\t4800\n",
    "de\tde5\t的\t的\t5600\n",
    "dian\tdian4\t電\t电\t5000\n",
    "diannao\tdian4nao3\t電腦\t电脑\t5400\n",
    "dong\tdong1\t東\t东\t5000\n",
    "dongjing\tdong1jing1\t東京\t东京\t5300\n",
    "en\ten1\t恩\t恩\t4500\n",
    "eng\teng1\t鞥\t鞥\t4100\n",
    "er\ter2\t兒\t儿\t4800\n",
    "guo\tguo2\t國\t国\t5100\n",
    "guoren\tguo2ren2\t國人\t国人\t5000\n",
    "hao\thao3\t好\t好\t5300\n",
    "hen\then3\t很\t很\t5200\n",
    "in\tin1\t音\t音\t4200\n",
    "ing\ting1\t英\t英\t4300\n",
    "jia\tjia1\t家\t家\t5000\n",
    "jin\tjin1\t今\t今\t5000\n",
    "jing\tjing1\t京\t京\t4900\n",
    "jintian\tjin1tian1\t今天\t今天\t5400\n",
    "keai\tke3ai4\t可愛\t可爱\t5400\n",
    "men\tmen5\t們\t们\t5100\n",
    "nao\tnao3\t腦\t脑\t4800\n",
    "nh\tn3h3\t你好\t你好\t4400\n",
    "ni\tni3\t你\t你\t5500\n",
    "nide\tni3de5\t你的\t你的\t5400\n",
    "nihao\tni3hao3\t你好\t你好\t5600\n",
    "nihaoma\tni3hao3ma5\t你好嗎\t你好吗\t5300\n",
    "ren\tren2\t人\t人\t5200\n",
    "shi\tshi4\t是\t是\t5500\n",
    "shijie\tshi4jie4\t世界\t世界\t5400\n",
    "tian\ttian1\t天\t天\t5100\n",
    "tianqi\ttian1qi4\t天氣\t天气\t5300\n",
    "wo\two3\t我\t我\t5600\n",
    "women\two3men5\t我們\t我们\t5500\n",
    "woxihuan\two3xi3huan1\t我喜歡\t我喜欢\t5600\n",
    "xihuan\txi3huan1\t喜歡\t喜欢\t5500\n",
    "xin\txin1\t心\t心\t5000\n",
    "xing\txing1\t星\t星\t5000\n",
    "zhong\tzhong1\t中\t中\t5300\n",
    "zhongguo\tzhong1guo2\t中國\t中国\t5500\n",
    "zhongguoren\tzhong1guo2ren2\t中國人\t中国人\t5600\n",
    "# context\n",
    "今天天氣\t800\n",
    "你好\t600\n",
    "喜歡你\t700\n",
    "我們喜歡\t750\n",
);

const SAMPLE_ZHUYIN_TSV: &str = concat!(
    "MEOWPUUT-ZHUYIN-1\n",
    "cl\tcl3\t好\t好\t5200\n",
    "g\tg4\t是\t是\t5300\n",
    "gr,\tg4r,4\t世界\t世界\t5400\n",
    "ji\tji3\t我\t我\t5500\n",
    "jimp\tji3mp3\t我們\t我们\t5400\n",
    "mp\tmp3\t們\t们\t5000\n",
    "r,\tr,4\t界\t界\t4800\n",
    "su\tsu3\t你\t你\t5400\n",
    "sucl\tsu3cl3\t你好\t你好\t5600\n",
    "# context\n",
    "你好\t600\n",
    "我們是\t700\n",
);

#[test]
fn test_reading_and_soa_primitives() {
    assert_eq!(max_input_bytes(), MAX_INPUT);
    assert!(is_tone(b'1', true));
    assert!(!is_tone(b'a', true));
    assert!(is_tone(b' ', false));
    assert!(is_tone(b'6', false));
    assert_eq!(zhuyin_symbol_for_key(b'1'), Some("ㄅ"));
    assert_eq!(zhuyin_symbol_for_key(b'~'), None);
    assert_eq!(strict_han_count(""), 0);
    assert_eq!(strict_han_count("你好"), 2);
    assert_eq!(strict_han_count("你a"), 0);
    assert_eq!(utf8_char_count("你好a"), 3);

    let h = han_characters_stack("你好世界").unwrap();
    assert_eq!(h.len(), 4);
    assert_eq!(h.slice("你好世界", 1), "好");
    assert!(han_characters_stack("hello").is_none());

    let r_py = Reading::new("ni3'hao3", true);
    assert!(!r_py.is_empty());
    assert!(!r_py.unambiguous);
    assert!(r_py.matches(b"ni3hao3", 0, r_py.len()));
    let r_py_toned = Reading::new("ni3hao3", true);
    assert!(r_py_toned.unambiguous);
    assert!(!r_py_toned.matches(b"ni2hao3", 0, r_py_toned.len()));

    let r_zy = Reading::new("su3'cl3", false);
    assert!(!r_zy.unambiguous);
    assert!(r_zy.matches(b"su3cl3", 0, r_zy.len()));

    let mut wt = WordTableSoA::with_capacity(2);
    assert!(wt.is_empty());
    wt.push(0, 3, 0, 3, 0, 3, 5000, 15);
    assert_eq!(wt.len(), 1);
    assert!(wt.is_fully_spelled(0));
    assert!(wt.is_single_han_hant(0));
    assert!(wt.is_verified_spelling(0));

    let bt = BucketTableSoA::default();
    assert!(bt.is_empty());
    let ct = ContextTableSoA::default();
    assert!(ct.is_empty());
    let mut jp = JapaneseTableSoA::with_capacity(2, 16);
    assert!(jp.is_empty());
    jp.push("neko", "猫", 5000, true);
    assert_eq!(jp.len(), 1);

    let mut cp = CandidatePoolSoA::with_capacity(4);
    assert!(cp.is_empty());
    cp.push("你好", 5, CandidateKind::Exact as u32, CandidateScoreKind::Lexicon as u32, 5000.0);
    assert_eq!(cp.text(0), "你好");

    assert!(Reading::new("", true).is_empty());
    assert!(NasalVariantsStack::from_sounds("abc").is_empty());
    let _ = meowpuut_core::workspace::PathLatticeSoA::default();
    let _ = meowpuut_core::workspace::StepPathsSoA::default();
    let mut sc = DecoderScratchSoA::default();
    sc.clear_decode();
    let mut do_soa = DecodeOutputSoA::with_capacity(4);
    do_soa.push("甲", 1, 0, 0, 1.0);
    do_soa.push("乙", 1, 0, 0, 2.0);
    do_soa.push("丙", 1, 0, 0, 3.0);
    do_soa.remove_at(1);
    do_soa.truncate(1);
    assert_eq!(do_soa.len(), 1);
}

#[test]
fn test_lexicon_load_errors_and_traditionalize() {
    assert!(Lexicon::load("").is_none());
    assert!(Lexicon::load("INVALID-HEADER\n").is_none());
    assert!(Lexicon::load("MEOWPUUT-PINYIN-1\nbad_line\n").is_none());
    assert!(Lexicon::load("MEOWPUUT-PINYIN-1\n# context\nbad_ctx\n").is_none());

    let lex = Lexicon::load(SAMPLE_PINYIN_TSV).unwrap();
    assert!(lex.pinyin);
    assert!(lex.heap_bytes() > 0);
    assert_eq!(lex.traditionalize("东京", true), "東京");
    assert_eq!(lex.traditionalize("hello", true), "");
    assert!(lex.join_bonus("我們", "喜歡", true) > 0.0);
    assert_eq!(lex.join_bonus("不存在", "詞", false), 0.0);

    let mut predictor = meowpuut_core::predict::Predictor::default();
    assert!(predictor.is_empty());
    assert_eq!(predictor.len(), 0);
    let sig_before = predictor.capacity_signature();
    assert_eq!(predictor.predict(&lex, "", false, 8), 0);
    assert_eq!(predictor.predict(&lex, "hello", false, 8), 0);
    assert_eq!(predictor.predict(&lex, "我們", false, 0), 0);
    assert!(predictor.predict(&lex, "我們", false, 8) > 0);
    assert!(!predictor.is_empty());
    assert_eq!(predictor.text(0), "喜歡");
    assert_eq!(predictor.text(99), "");
    assert!(predictor.predict(&lex, "我们", true, 8) > 0);
    assert!(!predictor.text(0).is_empty());
    assert_eq!(predictor.capacity_signature(), sig_before);
}

#[test]
fn test_fallback_variants_and_segments() {
    let nv = NasalVariantsStack::from_sounds("jintianxing");
    assert!(!nv.is_empty());

    let lex = Lexicon::load(SAMPLE_PINYIN_TSV).unwrap();
    assert!(has_exact_match(&lex, "nihao"));
    assert!(!has_exact_match(&lex, "zzzz"));

    let mut out = DecodeOutputSoA::with_capacity(16);
    unfinished_phrase_into(&lex, "nihaox", false, &mut out);
    assert!(!out.is_empty());

    let mut raw = DecodeOutputSoA::with_capacity(32);
    let mut order = Vec::with_capacity(32);
    // Single-letter initial fallback
    fallback_into(&lex, "n", false, true, &mut raw, &mut order, &mut out);
    assert!(!out.is_empty());
    // Prefix & QWERTY neighbor & duplicate char & nasal variant fallback
    fallback_into(&lex, "nihaoo", false, true, &mut raw, &mut order, &mut out);
    assert!(!out.is_empty());
    fallback_into(&lex, "xing", false, true, &mut raw, &mut order, &mut out);
    assert!(!out.is_empty());
    // Single char fallback when nothing else matches
    fallback_into(&lex, "ni9", false, true, &mut raw, &mut order, &mut out);
    assert!(!out.is_empty());

    let mut scratch = DecoderScratchSoA::new();
    decode_into(&lex, "xin", false, false, true, &mut scratch);
    assert!(!scratch.decoded.is_empty());

    predictive_segments_into(&lex, "zhongguor", false, &mut scratch, &mut out);
    assert!(!out.is_empty());

    pinyin_segments_into(&lex, "nihaoshijie", false, &mut scratch, &mut out);
    assert!(!out.is_empty());

    assert!(incomplete_pinyin_syllable(&lex, "don"));
    assert!(!incomplete_pinyin_syllable(&lex, "dong"));
    assert!(!incomplete_pinyin_syllable(&lex, ""));

    unfinished_sentence_into(&lex, "nihaoshijieb", false, true, false, &mut scratch, &mut out);
    assert!(!out.is_empty());
    unfinished_sentence_into(&lex, "nihaodon", false, false, true, &mut scratch, &mut out);
    assert!(!out.is_empty());

    let zy_lex = Lexicon::load(SAMPLE_ZHUYIN_TSV).unwrap();
    zhuyin_segments_into(&zy_lex, "su3cl3g4r,4", false, &mut scratch, &mut out);
    assert!(!out.is_empty());
}

#[test]
fn test_mixed_and_session_edge_cases() {
    let lex = Arc::new(Lexicon::load(SAMPLE_PINYIN_TSV).unwrap());
    let mut session = Session::new(Arc::clone(&lex));

    // Reject invalid Japanese words
    assert!(!session.add_japanese_word("neko", "猫", 1000));
    assert!(!session.add_japanese_word("a", "あ", 5000));
    assert!(!session.add_japanese_word("ai", "愛", 3500));
    assert!(session.add_japanese_word("au", "会う", 4500));
    assert!(session.add_japanese_word("koi", "恋", 3500));
    assert!(session.add_japanese_word("neko", "猫", 5500));
    assert!(session.add_japanese_word("watashi", "私", 5600));
    assert!(session.add_japanese_word("dong", "丼", 4500));
    assert!(session.add_japanese_word("wat", "ワ", 4200));

    assert_eq!(session.mixed_pinyin("woxihuannidenekohenkeai", false), "我喜歡你的猫很可愛");
    assert_eq!(session.mixed_pinyin("woxihuannidenekohenkeai", true), "我喜欢你的猫很可爱");
    // 3-match combination, pure-JP rejection, 3500-only rejection, short reading ("au"), ambig_before, ambig_after
    let _ = session.mixed_pinyin("woxihuanwatashinekokoishijie", false);
    assert_eq!(session.mixed_pinyin("watashineko", false), "");
    assert_eq!(session.mixed_pinyin("woxihuankoi", false), "");
    let _ = session.mixed_pinyin("auwoxihuan", false);
    let _ = session.mixed_pinyin("aunih", false);
    let _ = session.mixed_pinyin("nihaodongwatashi", false);
    let _ = session.mixed_pinyin("watashinihaodong", false);

    let (pref, cons) = session.mixed_pinyin_prefix("woxihuannekob", false);
    assert_eq!(pref, "我喜歡猫");
    assert_eq!(cons, 12);
    let _ = session.mixed_pinyin_prefix("woxihuannekodon", false);
    let _ = session.mixed_pinyin_prefix("woxihuankoidon", false);

    // Key handling edge cases: empty navigation, invalid keys, boundary keys, max length
    assert!(!session.key(0xff55));
    assert!(!session.key(0xff56));
    assert!(!session.key(10));
    assert!(!session.key(i32::from(b'1')));

    for b in b"nihaowat" {
        assert!(session.key(i32::from(*b)));
    }
    assert!(session.candidate_count() > 0);
    assert_eq!(session.candidate(999), "");
    assert_eq!(session.candidate_consumed(999), 0);
    assert!(session.candidate_info(999).is_none());
    assert!(!session.choose(999));

    session.set_simplified(true);
    session.set_simplified(true);
    session.set_segment_context("我們");
    assert_eq!(session.segment_context(), "我們");
    session.set_segment_context(&"a".repeat(100));
    assert_eq!(session.segment_context(), "");

    // Commit when candidates exist vs empty
    session.commit();
    assert!(!session.committed().is_empty());
    session.clear();
    session.commit();

    // Test MAX_INPUT boundary rollover for Pinyin (tone boundary and space)
    session.clear();
    for _ in 0..(MAX_INPUT / 2) {
        session.key(i32::from(b'n'));
        session.key(i32::from(b'i'));
    }
    assert_eq!(session.input().len(), MAX_INPUT);
    assert!(session.key(i32::from(b'3')));
    assert!(!session.committed().is_empty());

    session.clear();
    for _ in 0..(MAX_INPUT / 2) {
        session.key(i32::from(b'n'));
        session.key(i32::from(b'i'));
    }
    assert!(session.key(i32::from(b' ')));
    assert!(!session.committed().is_empty());

    // Zhuyin session edge cases
    let zy_lex = Arc::new(Lexicon::load(SAMPLE_ZHUYIN_TSV).unwrap());
    let mut zy_session = Session::new(zy_lex);
    assert!(!zy_session.key(i32::from(b'3')));
    for b in b"su3cl3" {
        assert!(zy_session.key(i32::from(*b)));
    }
    // Repeated tone key or space after tone in Zhuyin commits
    assert!(zy_session.key(i32::from(b' ')));
    assert_eq!(zy_session.committed(), "你好");

    zy_session.clear();
    for _ in 0..(MAX_INPUT / 2) {
        zy_session.key(i32::from(b's'));
        zy_session.key(i32::from(b'u'));
    }
    assert_eq!(zy_session.input().len(), MAX_INPUT);
    assert!(zy_session.key(i32::from(b'3')));
    assert!(!zy_session.committed().is_empty());

    let mut scratch = DecoderScratchSoA::new();
    let mut out = DecodeOutputSoA::with_capacity(8);
    let mut jp = JapaneseTableSoA::with_capacity(4, 32);
    jp.push("watashi", "私", 5500, true);
    unfinished_japanese_prefix_into(&lex, &jp, "nihaowat", false, true, &mut scratch, &mut out);
    assert!(!out.is_empty());
    let mut str_out = String::new();
    mixed_pinyin_into(&lex, &jp, "short", false, &mut scratch, &mut str_out);
    let _ = mixed_pinyin_prefix_into(&lex, &jp, "watashinihao", false, &mut scratch, &mut str_out);
    let _ = CandidateInfo {
        kind: 1,
        score_kind: 1,
        consumed_bytes: 2,
        remaining_offset_bytes: 2,
        score: 100.0,
    };

    // Binary format (MIMO-BIN-1 & MIMO-BASE-1) round-trip and validation branches
    let py_bin = lex.to_binary();
    let zy_bin = Lexicon::load(SAMPLE_ZHUYIN_TSV).unwrap().to_binary();
    let py_from_bin = Lexicon::load_bytes(&py_bin).unwrap();
    assert_eq!(py_from_bin.words.len(), lex.words.len());
    assert_eq!(py_from_bin.buckets.len(), lex.buckets.len());
    assert!(Lexicon::load_bytes(SAMPLE_PINYIN_TSV.as_bytes()).is_some());
    assert!(Lexicon::load_binary(b"SHORT").is_none());
    let mut bad_words = py_bin.clone();
    bad_words[20..24].copy_from_slice(&0u32.to_le_bytes());
    assert!(Lexicon::load_binary(&bad_words).is_none());
    let mut bad_size = py_bin.clone();
    bad_size.push(0);
    assert!(Lexicon::load_binary(&bad_size).is_none());
    let base = Lexicon::pack_base(&py_bin, &zy_bin);
    assert!(Lexicon::load_from_base(&base, true).is_some());
    assert!(Lexicon::load_from_base(&base, false).is_some());
    assert!(Lexicon::load_from_base(b"SHORT", true).is_none());
    let mut bad_ver = base.clone();
    bad_ver[12..16].copy_from_slice(&99u32.to_le_bytes());
    assert!(Lexicon::load_from_base(&bad_ver, true).is_none());
}
