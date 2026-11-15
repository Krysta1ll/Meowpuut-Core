//! Abbreviated input (簡拼) with the shipped lexicon: Pinyin initials and
//! full syllables mixed with initials, Zhuyin first symbols, with and without
//! the pair table, and no buffer growth after a warm-up.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use meowpuut_core::pairs::Pairs;
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

/// The first `n` candidates after typing `keys` (Zhuyin as Da-Chien keys).
fn first(session: &mut Session, keys: &str, n: usize) -> Vec<String> {
    session.clear();
    for k in keys.bytes() {
        session.key(i32::from(k));
    }
    (0..session.candidate_count().min(n)).map(|i| session.candidate(i).to_owned()).collect()
}

#[test]
fn pinyin_abbreviations() {
    for pairs in [true, false] {
        let mut s = session(true, pairs);
        // wode: 我的 was below 握得, whose weight the word counts had raised
        // while 我的 (always split as 我 + 的 there) counted as rare.
        for (keys, expected) in [("wode", "我的"), ("wod", "我的"), ("woxhn", "我喜歡你"), ("zmb", "怎麼辦"), ("jtqkdy", "今天去看電影")] {
            // Without a pair table 我喜歡你 is a composed path, which a
            // one-key fix (woxun: 我訊) may lead; it must stay second.
            if !pairs && keys == "woxhn" {
                assert!(first(&mut s, keys, 2).iter().any(|t| t == expected), "{keys}, pairs {pairs}");
                continue;
            }
            assert_eq!(first(&mut s, keys, 1), [expected], "{keys}, pairs {pairs}");
        }
    }
}

#[test]
fn zhuyin_first_symbols() {
    for pairs in [true, false] {
        let mut s = session(false, pairs);
        // ㄋㄏ, ㄐㄊ, ㄨㄛㄉ, ㄋㄏㄇ.
        for (keys, expected) in [("sc", "你好"), ("rw", "今天"), ("ji2", "我的"), ("sca", "你好嗎")] {
            assert_eq!(first(&mut s, keys, 1), [expected], "{keys}, pairs {pairs}");
        }
        // ㄨㄛˇㄒㄏ: 我喜歡 among the first.
        assert!(first(&mut s, "ji3vc", 3).iter().any(|c| c == "我喜歡"), "ji3vc, pairs {pairs}");
        // One initial keeps its single characters.
        assert!(first(&mut s, "s", 20).iter().all(|c| c.chars().count() == 1));
        // Typed in full: unchanged.
        assert_eq!(first(&mut s, "su3cl3", 1), ["你好"]);
        assert_eq!(first(&mut s, "ji32k7", 1), ["我的"]);
    }
}

#[test]
fn abbreviations_do_not_grow_buffers() {
    // Long runs of initials reach the scan budget of abbrev.rs.
    for (pinyin, keys) in [(false, "rwfd2urwfd2urwfd2u"), (false, "ji3vcsca"), (true, "jtqkdywocgmbchl")] {
        let mut s = session(pinyin, true);
        first(&mut s, keys, 1);
        let warm = s.capacity_signature();
        first(&mut s, keys, 1);
        assert_eq!(s.capacity_signature(), warm, "{keys} grew the buffers");
    }
}

#[test]
fn pinyin_bare_initials_keep_tones_and_english() {
    for pairs in [true, false] {
        let mut s = session(true, pairs);
        // A tone after the bare initial picks the characters with that tone: 的 is de5.
        let toned = first(&mut s, "wod4", 5);
        assert!(!toned.iter().any(|c| c == "我的"), "wod4 gave {toned:?}, pairs {pairs}");
        assert!(toned.iter().any(|c| c == "我對" || c == "我到"), "wod4 gave {toned:?}, pairs {pairs}");
        // An English word keeps its letters: no 不乾 for bug, no 哦看 for ok.
        for (keys, english) in [("bug", "bug"), ("ok", "OK")] {
            let list = first(&mut s, keys, 8);
            assert_eq!(list.first().map(String::as_str), Some(english), "{keys}, pairs {pairs}");
            assert!(list.iter().all(|c| c.is_ascii()), "{keys} gave {list:?}, pairs {pairs}");
        }
        assert!(!first(&mut s, "zhegebug", 8).iter().any(|c| c == "這個不乾"), "zhegebug, pairs {pairs}");
        // An English word inside a syllable does not count: the id of nid.
        assert_eq!(first(&mut s, "nid", 1), ["你的"], "nid, pairs {pairs}");
    }
}

#[test]
fn zhuyin_abbreviations_stay_near_the_end() {
    let mut s = session(false, true);
    // Full syllables before the window keep their reading; the last
    // first symbols are still read (ㄨㄛˇㄒㄧˇㄏㄨㄢㄋㄧˇ x2, then ㄨㄛㄉ).
    let keys = "ji3vu3cj0su3ji3vu3cj0su3ji2";
    assert_eq!(first(&mut s, keys, 1), ["我喜歡你我喜歡你我的"]);
    // Twenty bare symbols: those before the last sixteen are not read as
    // words, so the decode stays bounded (no whole-input candidate).
    let list = first(&mut s, "scscscscscscscscscsc", 5);
    assert!(list.iter().all(|c| c.chars().count() < 20), "{list:?}");
}
