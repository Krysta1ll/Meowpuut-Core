//! Edge-case and memory-safety tests for the C ABI. Small inputs so the whole
//! file also runs under Miri: `cargo +nightly miri test -p meowpuut-ffi --test abi_edges`.
#![allow(unsafe_code)]
#![allow(clippy::undocumented_unsafe_blocks, clippy::multiple_unsafe_ops_per_block)]

use std::ffi::{CStr, CString};
use std::ptr::{null, null_mut};

use meowpuut_ffi::*;

const TSV: &str = concat!(
    "MEOWPUUT-PINYIN-1\n",
    "hao\thao3\t好\t好\t4800\n",
    "men\tmen5\t們\t们\t5000\n",
    "ni\tni3\t你\t你\t5000\n",
    "nihao\tni3hao3\t你好\t你好\t5400\n",
    "wo\two3\t我\t我\t5000\n",
    "# context\n",
    "你好\t600\n",
    "我們\t9000\n",
);

fn s(p: *const std::ffi::c_char) -> String {
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

#[test]
fn null_handles_are_harmless() {
    unsafe {
        assert!(mpn_lexicon_create(null(), 10).is_null());
        assert!(mpn_lexicon_create(TSV.as_ptr().cast(), 0).is_null());
        assert!(mpn_lexicon_create_from_base(null(), 10, 1).is_null());
        assert!(mpn_lexicon_create_from_base(TSV.as_ptr().cast(), 0, 1).is_null());
        assert!(mpn_lexicon_create_from_base(TSV.as_ptr().cast(), TSV.len(), 1).is_null());
        let bad = [0xffu8, 0xfe, 0x00];
        assert!(mpn_lexicon_create(bad.as_ptr().cast(), bad.len()).is_null());
        let wrong = "NOT-A-LEXICON\n";
        assert!(mpn_lexicon_create(wrong.as_ptr().cast(), wrong.len()).is_null());
        assert!(mpn_session_create(null()).is_null());
        assert_eq!(mpn_lexicon_heap_bytes(null()), 0);
        mpn_lexicon_destroy(null_mut());
        mpn_session_destroy(null_mut());
        let n = null_mut();
        assert_eq!(mpn_key(n, 'a' as i32), 0);
        assert_eq!(mpn_choose(n, 0), 0);
        mpn_set_simplified(n, 1);
        mpn_clear(n);
        mpn_commit(n);
        mpn_clear_commit(n);
        mpn_set_segment_context(n, c"你".as_ptr());
        assert_eq!(mpn_add_japanese_word(n, c"neko".as_ptr(), c"猫".as_ptr(), 5000), 0);
        assert_eq!(s(mpn_mixed_pinyin(n, c"nekoxihuan".as_ptr(), 0)), "");
        let mut used = 7usize;
        assert_eq!(s(mpn_mixed_pinyin_prefix(n, c"neko".as_ptr(), 0, &mut used)), "");
        assert_eq!(used, 0);
        assert_eq!(s(mpn_mixed_pinyin_prefix(n, c"neko".as_ptr(), 0, null_mut())), "");
        assert_eq!(mpn_predict(n, c"我".as_ptr(), 3), 0);
        for p in [mpn_input(n), mpn_preedit(n), mpn_committed(n), mpn_segment_context(n), mpn_candidate(n, 0), mpn_prediction(n, 0)] {
            assert_eq!(s(p), "");
        }
        assert_eq!(mpn_candidate_count(n), 0);
        assert_eq!(mpn_candidate_consumed(n, 0), 0);
        assert_eq!(mpn_page(n), 0);
        let mut info = MPNCandidateInfo::default();
        assert_eq!(mpn_candidate_info(n, 0, &mut info, 32), 0);
        assert_eq!(mpn_max_input_bytes(), 256);
    }
}

#[test]
fn typing_choosing_predicting_and_partial_info() {
    unsafe {
        let lex = mpn_lexicon_create(TSV.as_ptr().cast(), TSV.len());
        assert!(mpn_lexicon_heap_bytes(lex) > 0);
        let ss = mpn_session_create(lex);
        mpn_lexicon_destroy(lex); // session keeps its own reference
        for b in b"nihao" {
            assert_eq!(mpn_key(ss, i32::from(*b)), 1);
        }
        assert_eq!(s(mpn_input(ss)), "nihao");
        assert_eq!(s(mpn_preedit(ss)), "nihao");
        assert_eq!(s(mpn_candidate(ss, 0)), "你好");
        assert_eq!(s(mpn_candidate(ss, 999)), "");
        assert_eq!(mpn_candidate_consumed(ss, 0), 5);

        // Partial writes honour info_size (C ABI versioning).
        let mut info = MPNCandidateInfo { kind: 99, score_kind: 99, consumed_bytes: 99, remaining_offset_bytes: 99, score: 9.0 };
        assert_eq!(mpn_candidate_info(ss, 0, &mut info, 4), 1);
        assert_eq!((info.kind, info.score_kind), (1, 99));
        assert_eq!(mpn_candidate_info(ss, 0, &mut info, 32), 1);
        assert_eq!((info.consumed_bytes, info.remaining_offset_bytes, info.score), (5, 5, 5400.0));
        assert_eq!(mpn_candidate_info(ss, 0, null_mut(), 32), 0);
        assert_eq!(mpn_candidate_info(ss, 0, &mut info, 0), 0);
        assert_eq!(mpn_candidate_info(ss, 999, &mut info, 32), 0);

        assert_eq!(mpn_choose(ss, 0), 1);
        assert_eq!(s(mpn_committed(ss)), "你好");
        mpn_clear_commit(ss);
        assert_eq!(s(mpn_committed(ss)), "");
        assert_eq!(mpn_choose(ss, 0), 0);

        assert_eq!(mpn_predict(ss, c"我".as_ptr(), 8), 1);
        assert_eq!(s(mpn_prediction(ss, 0)), "們");
        mpn_set_simplified(ss, 1);
        assert_eq!(mpn_predict(ss, c"我".as_ptr(), 8), 1);
        assert_eq!(s(mpn_prediction(ss, 0)), "们");
        assert_eq!(s(mpn_prediction(ss, 5)), "");
        assert_eq!(mpn_predict(ss, null(), 8), 0);

        // Stored even while idle (C++ contract); only refresh needs input.
        mpn_set_segment_context(ss, c"你".as_ptr());
        assert_eq!(s(mpn_segment_context(ss)), "你");
        mpn_key(ss, 'w' as i32);
        mpn_key(ss, 'o' as i32);
        mpn_set_segment_context(ss, c"你".as_ptr());
        assert_eq!(s(mpn_segment_context(ss)), "你");
        mpn_set_segment_context(ss, null());
        assert_eq!(s(mpn_segment_context(ss)), "");
        mpn_commit(ss);
        assert_eq!(s(mpn_committed(ss)), "我");
        mpn_clear(ss);
        assert_eq!(s(mpn_input(ss)), "");

        let (r, t) = (CString::new("neko").unwrap(), CString::new("猫").unwrap());
        assert_eq!(mpn_add_japanese_word(ss, r.as_ptr(), t.as_ptr(), 5000), 1);
        assert_eq!(mpn_add_japanese_word(ss, r.as_ptr(), null(), 5000), 0);
        let _ = s(mpn_mixed_pinyin(ss, c"nihaoneko".as_ptr(), 0));
        let mut used = 0usize;
        let _ = s(mpn_mixed_pinyin_prefix(ss, c"nekonihao".as_ptr(), 0, &mut used));
        assert_eq!(s(mpn_mixed_pinyin(ss, null(), 0)), "");
        assert_eq!(mpn_key(ss, 0xff55), 0);
        mpn_session_destroy(ss);
    }
}

#[test]
fn candidate_metadata_and_input_limit_contracts() {
    const PINYIN: &str = concat!(
        "MEOWPUUT-PINYIN-1\n",
        "hao\thao3\t好\t好\t15000\n",
        "ni\tni3\t你\t你\t15000\n",
        "ni\tni3\t𠮷\t𠮷\t12000\n",
        "nihao\tni3hao3\t你好\t你好\t16000\n",
        "nixua\tni3xua1\t你花\t你花\t16000\n",
        "wo\two3\t我\t我\t15000\n",
        "wox\two3x1\t我想\t我想\t16000\n",
        "xihuan\txi3huan1\t喜歡\t喜欢\t15000\n",
    );
    const ZHUYIN: &str = concat!(
        "MEOWPUUT-ZHUYIN-1\n",
        "sucl\tsu3cl3\t你好\t你好\t15000\n",
        "suclv\tsu3cl3v \t你好\t你好\t16000\n",
    );
    unsafe {
        let find = |ss: *mut MPNSession, text: &str, kind: u32, consumed: u64| -> usize {
            for i in 0..mpn_candidate_count(ss) {
                let mut info = MPNCandidateInfo::default();
                assert_eq!(mpn_candidate_info(ss, i, &mut info, size_of::<MPNCandidateInfo>()), 1);
                assert_eq!(info.consumed_bytes, mpn_candidate_consumed(ss, i) as u64);
                assert_eq!(info.remaining_offset_bytes, info.consumed_bytes);
                if s(mpn_candidate(ss, i)) == text && info.kind == kind && info.consumed_bytes == consumed {
                    return i;
                }
            }
            panic!("missing candidate {text} kind={kind} consumed={consumed}");
        };
        let type_str = |ss: *mut MPNSession, keys: &str| {
            for b in keys.bytes() {
                assert_eq!(mpn_key(ss, i32::from(b)), 1);
            }
        };

        let lex = mpn_lexicon_create(PINYIN.as_ptr().cast(), PINYIN.len());
        let ss = mpn_session_create(lex);
        type_str(ss, "nihao");
        let idx = find(ss, "你好", 1, 5);
        let mut info = MPNCandidateInfo::default();
        assert_eq!(mpn_candidate_info(ss, idx, &mut info, size_of::<MPNCandidateInfo>()), 1);
        assert_eq!((info.score_kind, info.score), (1, 16000.0));

        mpn_clear(ss);
        type_str(ss, "nihaoni");
        let _ = find(ss, "你好你", 2, 7);
        mpn_clear(ss);
        type_str(ss, "wox");
        let _ = find(ss, "我想", 4, 3);
        mpn_clear(ss);
        type_str(ss, "nixua");
        let _ = find(ss, "你花", 0, 5);
        mpn_clear(ss);
        type_str(ss, "niaho");
        let _ = find(ss, "你好", 4, 5);
        mpn_clear(ss);
        type_str(ss, "ni3hao3x");
        let partial = find(ss, "你好", 3, 7);
        assert_eq!(mpn_choose(ss, partial), 1);
        assert_eq!((s(mpn_committed(ss)), s(mpn_input(ss))), ("你好".to_string(), "x".to_string()));
        mpn_clear(ss);
        type_str(ss, "ni'hao");
        let _ = find(ss, "你好", 1, 6);
        mpn_clear(ss);
        type_str(ss, "ni");
        let _ = find(ss, "𠮷", 1, 2);
        mpn_session_destroy(ss);
        mpn_lexicon_destroy(lex);

        let zlex = mpn_lexicon_create(ZHUYIN.as_ptr().cast(), ZHUYIN.len());
        let zss = mpn_session_create(zlex);
        type_str(zss, "su3cl3v");
        let full = find(zss, "你好", 1, 7);
        let part = find(zss, "你好", 3, 6);
        assert_ne!(full, part);
        assert_eq!(mpn_choose(zss, part), 1);
        assert_eq!((s(mpn_committed(zss)), s(mpn_input(zss))), ("你好".to_string(), "v".to_string()));
        mpn_session_destroy(zss);
        mpn_lexicon_destroy(zlex);

        let py_bin = meowpuut_core::Lexicon::load(PINYIN).unwrap().to_binary();
        let zy_bin = meowpuut_core::Lexicon::load(ZHUYIN).unwrap().to_binary();
        let base = meowpuut_core::Lexicon::pack_base(&py_bin, &zy_bin);
        let blex = mpn_lexicon_create_from_base(base.as_ptr().cast(), base.len(), 1);
        assert!(!blex.is_null());
        mpn_lexicon_destroy(blex);
    }
}
