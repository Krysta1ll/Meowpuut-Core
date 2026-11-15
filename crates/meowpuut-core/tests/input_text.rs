//! `Session::input_text_into`: the uncommitted input as the user would write it.

use std::sync::Arc;

use meowpuut_core::reading::zhuyin_text_into;
use meowpuut_core::{Lexicon, Session};

const ZHUYIN: &str = concat!(
    "MEOWPUUT-ZHUYIN-1\n",
    "2k\t2k7\t的\t的\t6000\n",
    "al\tal \t貓\t猫\t5000\n",
    "cl\tcl3\t好\t好\t5200\n",
    "su\tsu3\t你\t你\t5400\n",
    "sucl\tsu3cl3\t你好\t你好\t5600\n",
    "vu\tvu \t西\t西\t5000\n",
);

const PINYIN: &str = concat!(
    "MEOWPUUT-PINYIN-1\n",
    "hao\thao3\t好\t好\t4800\n",
    "ni\tni3\t你\t你\t5000\n",
    "nihao\tni3hao3\t你好\t你好\t5400\n",
);

fn typed(lexicon: &str, keys: &str) -> Session {
    let mut session = Session::new(Arc::new(Lexicon::load(lexicon).unwrap()));
    for key in keys.bytes() {
        assert!(session.key(i32::from(key)), "key {}", key as char);
    }
    session
}

fn text(session: &Session) -> String {
    let mut out = String::new();
    session.input_text_into(&mut out);
    out
}

#[test]
fn zhuyin_keys_map_to_bopomofo_without_first_tone() {
    let mut out = String::new();
    for (keys, want) in [
        ("su3cl3", "ㄋㄧˇㄏㄠˇ"),
        ("al", "ㄇㄠ"),
        ("al ", "ㄇㄠ"),
        ("2k7", "ㄉㄜ˙"),
        ("vu'0", "ㄒㄧㄢ"),
        ("1qaz-6", "ㄅㄆㄇㄈㄦˊ"),
        ("", ""),
    ] {
        out.clear();
        zhuyin_text_into(keys.as_bytes(), &mut out);
        assert_eq!(out, want, "{keys}");
    }
}

#[test]
fn zhuyin_session_gives_symbols_and_pinyin_gives_letters() {
    let session = typed(ZHUYIN, "su3cl3");
    assert_eq!(session.input(), "su3cl3");
    assert_eq!(text(&session), "ㄋㄧˇㄏㄠˇ");
    assert_eq!(text(&typed(ZHUYIN, "2k7")), "ㄉㄜ˙");
    assert_eq!(text(&typed(ZHUYIN, "al")), "ㄇㄠ");
    assert_eq!(text(&typed(PINYIN, "ni3hao")), "ni3hao");
}

#[test]
fn only_the_uncommitted_rest_after_a_partial_choose() {
    let mut session = typed(ZHUYIN, "su3cl3vu");
    let index = (0..session.candidate_count())
        .find(|&i| session.candidate(i) == "你好" && session.candidate_consumed(i) == 6)
        .expect("你好 for su3cl3");
    assert!(session.choose(index));
    assert_eq!(session.committed(), "你好");
    assert_eq!(session.input(), "vu");
    assert_eq!(text(&session), "ㄒㄧ");
    session.commit();
    assert_eq!(text(&session), "");
}

#[test]
fn reuses_the_callers_buffer() {
    let session = typed(ZHUYIN, "su3cl3");
    let mut out = String::with_capacity(1024);
    let capacity = out.capacity();
    for _ in 0..8 {
        session.input_text_into(&mut out);
    }
    assert_eq!(out, "ㄋㄧˇㄏㄠˇ");
    assert_eq!(out.capacity(), capacity);
}

#[test]
fn nine_key_gives_the_digits_typed() {
    let mut session = Session::new_nine_key(Arc::new(Lexicon::load(PINYIN).unwrap())).unwrap();
    for key in b"64426" {
        assert!(session.key(i32::from(*key)));
    }
    assert_eq!(text(&session), "64426");
}
