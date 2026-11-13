use std::sync::Arc;

use meowpuut_core::{CandidateKind, Lexicon, Session};

#[test]
fn nine_key_converts_and_commits_only_the_selected_digits() {
    let lexicon = Arc::new(Lexicon::load(concat!(
        "MEOWPUUT-PINYIN-1\n",
        "hao\thao3\t好\t好\t4800\n",
        "momo\tmo4mo4\t莫莫\t莫莫\t8000\n",
        "ni\tni3\t你\t你\t5000\n",
        "nihao\tni3hao3\t你好\t你好\t5500\n",
        "pi\tpi2\t皮\t皮\t5400\n",
        "qi\tqi3\t起\t起\t5500\n",
        "sushi\tsu1shi4\t蘇軾\t苏轼\t10000\n",
        "wo\two3\t我\t我\t5600\n",
    )).unwrap());
    let mut session = Session::new_nine_key(Arc::clone(&lexicon)).unwrap();
    assert!(session.add_japanese_word("sushi", "寿司", 10000));
    assert!(session.add_japanese_word("kawaii", "可愛い", 8000));
    assert!(!session.key(i32::from(b'n')));
    assert!(!session.key(i32::from(b'1')));

    let type_keys = |session: &mut Session, keys: &str| {
        session.clear();
        for key in keys.bytes() {
            assert!(session.key(i32::from(key)));
        }
        assert_eq!(session.preedit(), keys);
    };
    type_keys(&mut session, "74");
    assert_eq!(session.candidate(0), "起");
    assert_eq!(session.candidate(1), "皮");
    type_keys(&mut session, "6442696");
    let prefix = (0..session.candidate_count()).find(|&i| session.candidate(i) == "你好").unwrap();
    assert_eq!(session.candidate_consumed(prefix), 5);
    assert!(session.choose(prefix));
    assert_eq!((session.committed(), session.input()), ("你好", "96"));
    assert_eq!(session.candidate(0), "我");
    assert!(session.key(0xff08));
    assert_eq!(session.input(), "9");
    assert!(session.key(0xff08));
    assert_eq!(session.candidate_count(), 0);

    type_keys(&mut session, "78744");
    assert_eq!(session.candidate(0), "蘇軾");
    assert_eq!(session.candidate(1), "寿司");
    session.set_simplified(true);
    assert_eq!(session.candidate(0), "苏轼");
    assert!(session.choose(1));
    assert_eq!((session.committed(), session.input()), ("寿司", ""));
    type_keys(&mut session, "6666529244");
    assert_eq!(session.candidate(0), "莫莫可愛い");
    assert_eq!(session.candidate_info(0).unwrap().kind, CandidateKind::Japanese as u32);
    assert!(session.key(32));
    assert_eq!((session.committed(), session.input()), ("莫莫可愛い", ""));

    // A Pinyin session on the same dictionary still accepts letters.
    let mut pinyin = Session::new(lexicon);
    assert!(pinyin.key(i32::from(b'n')));
    assert!(pinyin.key(i32::from(b'i')));
    assert_eq!(pinyin.candidate(0), "你");
}
