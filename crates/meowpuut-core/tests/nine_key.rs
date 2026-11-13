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

    type_keys(&mut session, "64426965");
    assert_eq!((session.candidate(0), session.candidate_consumed(0)), ("你好我", 7));
    assert_eq!(session.candidate_info(0).unwrap().kind, CandidateKind::Partial as u32);
    assert!(session.choose(0));
    assert_eq!((session.committed(), session.input()), ("你好我", "5"));

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

#[test]
fn nine_key_word_break_constrains_matches_and_stays_in_preedit() {
    let lexicon = Arc::new(Lexicon::load(concat!(
        "MEOWPUUT-PINYIN-1\n",
        "an\tan1\t安\t安\t5500\n",
        "jin\tjin1\t今\t今\t5200\n",
        "jinan\tji3nan2\t濟南\t济南\t13000\n",
        "jinan\tjin1an1\t今安\t今安\t9000\n",
    )).unwrap());
    let mut session = Session::new_nine_key(lexicon).unwrap();
    let keys = |session: &mut Session, reading: &str| {
        for key in reading.bytes() {
            assert!(session.key(i32::from(key)));
        }
    };
    assert!(!session.key(i32::from(b'\'')));
    keys(&mut session, "54626");
    assert_eq!(session.candidate(0), "濟南");

    session.clear();
    keys(&mut session, "546'");
    assert_eq!((session.candidate(0), session.candidate_consumed(0)), ("今", 4));
    assert!(!session.key(i32::from(b'\'')));
    assert!(session.key(0xff08));
    assert_eq!(session.preedit(), "546");
    keys(&mut session, "'26");
    assert_eq!(session.candidate(0), "今安");
    assert_eq!(session.candidate_info(0).unwrap().kind, CandidateKind::Exact as u32);
    assert!((0..session.candidate_count()).all(|i| session.candidate(i) != "濟南"));
    let prefix = (0..session.candidate_count()).find(|&i| session.candidate(i) == "今").unwrap();
    assert_eq!(session.candidate_consumed(prefix), 4);
    assert!(session.choose(prefix));
    assert_eq!((session.committed(), session.input(), session.candidate(0)), ("今", "26", "安"));

    session.clear();
    keys(&mut session, "999'999");
    assert_eq!(session.candidate_count(), 0);
    assert!(session.key(0xff0d));
    assert_eq!(session.committed(), "999999");
}
