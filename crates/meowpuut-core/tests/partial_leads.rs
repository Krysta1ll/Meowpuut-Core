//! The first candidate while a word is still being typed, with the shipped
//! lexicon, with and without the pair table: Zhuyin syllables before a first
//! symbol typed after a tone, Pinyin phrases before a trailing initial, English
//! words before a lone initial, and key-edit fixes of input valid as typed.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use meowpuut_core::pairs::Pairs;
use meowpuut_core::reading::{KEYS, SYMBOLS};
use meowpuut_core::{Lexicon, Session};

fn generated(name: &str) -> &'static [u8] {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.build/lexicon").join(name);
    let bytes = fs::read(&path).unwrap_or_else(|_| panic!("{} missing; run scripts/build-lexicon.sh", path.display()));
    Box::leak(bytes.into_boxed_slice())
}

fn session(pinyin: bool, pairs: bool) -> Session {
    let mut lex = Lexicon::from_base_in_place(generated("mimo_base.bin"), pinyin, None).unwrap();
    if pairs {
        let table = Pairs::in_place(generated("mimo_pairs.bin"), &lex.index().word_ids, None).unwrap();
        lex.set_pairs(table);
    }
    Session::new(Arc::new(lex))
}

/// Da-Chien keys for Bopomofo text (ˉ is the space key).
fn zhuyin_keys(symbols: &str) -> String {
    symbols
        .chars()
        .map(|c| {
            let at = SYMBOLS.iter().position(|s| s.starts_with(c)).unwrap_or_else(|| panic!("symbol {c}"));
            char::from(KEYS[at])
        })
        .collect()
}

fn first(session: &mut Session, keys: &str, n: usize) -> Vec<String> {
    session.clear();
    for k in keys.bytes() {
        session.key(i32::from(k));
    }
    (0..session.candidate_count().min(n)).map(|i| session.candidate(i).to_owned()).collect()
}

#[test]
fn zhuyin_syllables_before_a_first_symbol_read_as_the_whole_input_reads_them() {
    for pairs in [true, false] {
        let mut s = session(false, pairs);
        for (symbols, expected) in [
            // The whole reading's characters lead, not a decode of the
            // syllables alone (你是神, 隊部, 我們金, 打店).
            ("ㄋㄧˇㄕˋㄕㄣˊㄇ", "你是什"),
            ("ㄉㄨㄟˋㄅㄨˋㄑ", "對不"),
            ("ㄨㄛˇㄇㄣ˙ㄐㄧㄣˉㄊ", "我們今"),
            ("ㄉㄚˇㄉㄧㄢˋㄏ", "打電"),
            // A whole reading that splits a syllable (該是為把) is skipped.
            ("ㄍㄞˉㄕㄨㄟˋㄅ", "該睡"),
            // One within a nat of a reading that keeps the syllables' own
            // decode does not take the lead (等一下藥球 against 等一下要錢).
            ("ㄉㄥˇㄧˉㄒㄧㄚˋㄧㄠˋㄑ", "等一下要"),
            // Kept as before.
            ("ㄏㄠˇㄐㄧㄡˇㄅ", "好久"),
            ("ㄇㄧㄥˊㄊㄧㄢˉㄐ", "明天"),
            ("ㄉㄚˋㄐㄧㄚˉㄉ", "大家"),
            ("ㄓˉㄉㄠˋㄌ", "知道"),
            ("ㄋㄧˇㄏㄠˇㄒ", "你好"),
            ("ㄋㄧˇㄒ", "你"),
            ("ㄋㄏ", "你好"),
        ] {
            assert_eq!(first(&mut s, &zhuyin_keys(symbols), 1), [expected], "{symbols}, pairs {pairs}");
        }
    }
    // Two first symbols still lead with the abbreviation.
    let mut s = session(false, true);
    assert_eq!(first(&mut s, &zhuyin_keys("ㄨㄛˇㄒㄏ"), 1), ["我喜歡"]);
}

#[test]
fn pinyin_phrase_before_a_trailing_initial_keeps_its_segmentation() {
    for pairs in [true, false] {
        let mut s = session(true, pairs);
        for (keys, expected) in [
            // Not re-segmented for a word the initial might start (因子, 簡直,
            // 盲目, 取件, 分為).
            ("bangwodayinz", "幫我打印"),
            ("qingwenxishoujianz", "請問洗手間"),
            ("keyibangwoyigemangm", "可以幫我一個忙"),
            ("woyaoquj", "我要去"),
            ("bangwodayinzhefenw", "幫我打印這份"),
            // A continuation word still settles a close call (哪裡, 八點).
            ("nizainal", "你在哪"),
            ("mingtianzaoshangbad", "明天早上八"),
        ] {
            assert_eq!(first(&mut s, keys, 1), [expected], "{keys}, pairs {pairs}");
        }
    }
}

#[test]
fn english_word_before_a_lone_initial_leads() {
    for pairs in [true, false] {
        let mut s = session(true, pairs);
        for (keys, expected) in [
            ("bangwochecky", "幫我check"),
            ("yongpythonx", "用Python"),
            // Abbreviations without English keep their whole read.
            ("wod", "我的"),
        ] {
            assert_eq!(first(&mut s, keys, 1), [expected], "{keys}, pairs {pairs}");
            if keys != "wod" {
                // The guess for the initial stays on the first page.
                assert!(first(&mut s, keys, 5).len() > 1, "{keys}, pairs {pairs}");
            }
        }
    }
    // A lighter word the initial goes on (不吃 under 不錯) is a guess.
    for pairs in [true, false] {
        let mut s = session(true, pairs);
        assert_ne!(first(&mut s, "wozaistarbuc", 1), ["我在star不吃"], "pairs {pairs}");
    }
    // The heaviest word the initial goes on is read whole (會議, 一下).
    let mut s = session(true, true);
    assert_eq!(first(&mut s, "kaizoomhuiy", 1), ["開Zoom會議"]);
    assert_eq!(first(&mut s, "yonggooglechayix", 1), ["用Google查一下"]);
}

#[test]
fn key_edits_do_not_lead_input_valid_as_typed() {
    for pairs in [true, false] {
        let mut s = session(true, pairs);
        // Spelled as typed: the path leads, not a one-key guess (我下去, 我們做了).
        assert_eq!(first(&mut s, "woxiawu", 1), ["我下午"], "pairs {pairs}");
        assert_eq!(first(&mut s, "womenzoule", 1), ["我們走了"], "pairs {pairs}");
        // A Japanese word of the session is valid as typed: no fix (他和如) first.
        s.add_japanese_word("taberu", "食べる", 7000);
        let row = first(&mut s, "taberu", 2);
        assert!(!row.is_empty() && row[0] != "他和如", "taberu, pairs {pairs}: {row:?}");
        // A real typo is still fixed first (錯字: no word spells the letters).
        assert_eq!(first(&mut s, "nidaoleema", 1), ["你到了嗎"], "pairs {pairs}");
    }
}
