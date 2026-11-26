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
        assert_eq!(session.input(), keys);
    };
    type_keys(&mut session, "74");
    assert_eq!(session.preedit(), "qi");
    assert_eq!(session.candidate(0), "起");
    assert_eq!(session.candidate(1), "皮");
    type_keys(&mut session, "6442696");
    assert_eq!(session.preedit(), "ni hao wo");
    let prefix = (0..session.candidate_count()).find(|&i| session.candidate(i) == "你好").unwrap();
    assert_eq!(session.candidate_consumed(prefix), 5);
    assert!(session.choose(prefix));
    assert_eq!((session.committed(), session.input()), ("你好", "96"));
    assert_eq!(session.candidate(0), "我");
    assert_eq!(session.preedit(), "wo");
    assert!(session.key(0xff08));
    assert_eq!(session.input(), "9");
    assert_eq!(session.preedit(), "9");
    assert!(session.key(0xff08));
    assert_eq!(session.candidate_count(), 0);

    type_keys(&mut session, "64426965");
    assert_eq!(session.preedit(), "ni hao wo 5");
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
    assert_eq!(session.preedit(), "ji nan");

    session.clear();
    keys(&mut session, "546'");
    assert_eq!((session.candidate(0), session.candidate_consumed(0)), ("今", 4));
    assert_eq!(session.preedit(), "jin'");
    assert!(!session.key(i32::from(b'\'')));
    assert!(session.key(0xff08));
    assert_eq!(session.preedit(), "jin");
    keys(&mut session, "'26");
    assert_eq!(session.candidate(0), "今安");
    assert_eq!(session.preedit(), "jin'an");
    assert_eq!(session.candidate_info(0).unwrap().kind, CandidateKind::Exact as u32);
    assert!((0..session.candidate_count()).all(|i| session.candidate(i) != "濟南"));
    let prefix = (0..session.candidate_count()).find(|&i| session.candidate(i) == "今").unwrap();
    assert_eq!(session.candidate_consumed(prefix), 4);
    assert!(session.choose(prefix));
    assert_eq!((session.committed(), session.input(), session.candidate(0)), ("今", "26", "安"));

    session.clear();
    keys(&mut session, "999'999");
    assert_eq!(session.candidate_count(), 0);
    assert_eq!(session.preedit(), "999'999");
    assert!(session.key(0xff0d));
    assert_eq!(session.committed(), "999999");
}

#[test]
fn nine_key_preedit_spells_the_leading_abbreviated_candidate() {
    let lexicon = Arc::new(Lexicon::load(concat!(
        "MEOWPUUT-PINYIN-1\n",
        "n\tn3\t你\t你\t9000\n",
        "o\to4\t哦\t哦\t3000\n",
    )).unwrap());
    let mut session = Session::new_nine_key(lexicon).unwrap();
    assert!(session.key(i32::from(b'6')));
    assert_eq!(session.candidate_info(0).unwrap().kind, CandidateKind::Correction as u32);
    assert_eq!((session.candidate(0), session.preedit()), ("你", "n"));
}

#[test]
fn nine_key_reading_reuses_the_preedit_buffer() {
    let lexicon = Arc::new(Lexicon::load(concat!(
        "MEOWPUUT-PINYIN-1\n",
        "hao\thao3\t好\t好\t4800\n",
        "ni\tni3\t你\t你\t5000\n",
        "nihao\tni3hao3\t你好\t你好\t5500\n",
        "qi\tqi3\t起\t起\t5500\n",
        "wo\two3\t我\t我\t5600\n",
    )).unwrap());
    let mut session = Session::new_nine_key(lexicon).unwrap();
    let typed = |session: &mut Session, keys: &str| {
        session.clear();
        for key in keys.bytes() {
            assert!(session.key(i32::from(key)));
        }
    };
    typed(&mut session, "64426965");
    assert_eq!(session.preedit(), "ni hao wo 5");
    let warm = session.capacity_signature();
    typed(&mut session, "74");
    assert_eq!(session.preedit(), "qi");
    assert_eq!(session.capacity_signature(), warm, "showing a shorter reading shrank the buffers");
}

fn typed(session: &mut Session, keys: &str) {
    session.clear();
    for key in keys.bytes() {
        assert!(session.key(i32::from(key)));
    }
}

fn candidates(session: &Session) -> Vec<&str> {
    (0..session.candidate_count()).map(|i| session.candidate(i)).collect()
}

#[test]
fn nine_key_initials_offer_abbreviated_words() {
    let lexicon = Arc::new(Lexicon::load(concat!(
        "MEOWPUUT-PINYIN-1\n",
        "hao\thao3\t好\t好\t4800\n",
        "nh\tn3h3\t你好\t你好\t900\n",
        "ni\tni3\t你\t你\t5000\n",
        "nihao\tni3hao3\t你好\t你好\t7200\n",
        "xian\txian4\t現\t现\t6000\n",
        "xianzai\txian4zai4\t現在\t现在\t9000\n",
        "xiazai\txia4zai3\t下載\t下载\t6000\n",
        "xz\tx4z4\t現在\t现在\t1125\n",
        "xz\tx4z3\t下載\t下载\t750\n",
        "xz\tx4z4\t陷在\t陷在\t265\n",
        "zai\tzai4\t在\t在\t6000\n",
        "zz\tz4z4\t自在\t自在\t600\n",
    )).unwrap());
    let mut session = Session::new_nine_key(lexicon).unwrap();
    session.set_simplified(true);

    // Every key an initial: x z, z z.
    typed(&mut session, "99");
    assert_eq!((session.candidate(0), session.preedit()), ("现在", "x z"));
    assert_eq!(session.candidate_info(0).unwrap().kind, CandidateKind::Correction as u32);
    assert_eq!(candidates(&session), ["现在", "下载", "自在"], "陷在 is below the weight floor");
    assert!(session.choose(1));
    assert_eq!((session.committed(), session.input()), ("下载", ""));

    // A spelled syllable, then the initial of the next one: the spelled
    // prefix stays first while the next syllable is typed, the word
    // completed from it follows.
    typed(&mut session, "94269");
    assert_eq!((session.candidate(0), session.preedit()), ("现", "xian 9"));
    assert_eq!(session.candidate_info(0).unwrap().kind, CandidateKind::Partial as u32);
    assert_eq!((session.candidate(1), session.candidate_consumed(1)), ("现在", 5));
    typed(&mut session, "99'");
    assert_eq!(session.candidate(0), "现在");
    // A word typed as initials after a spelled word.
    typed(&mut session, "6442699");
    assert_eq!((session.candidate(0), session.preedit()), ("你好", "ni hao 99"));
    assert!(candidates(&session).contains(&"你好现在"));

    // Spelled words stay first.
    typed(&mut session, "9426924");
    assert_eq!((session.candidate(0), session.preedit()), ("现在", "xian zai"));
    assert_eq!(session.candidate_info(0).unwrap().kind, CandidateKind::Exact as u32);
    typed(&mut session, "64");
    assert_eq!(session.candidate(0), "你");
    assert_eq!(candidates(&session), ["你", "你好"]);
}

#[test]
fn nine_key_two_abbreviated_words_in_a_row() {
    let lexicon = Arc::new(Lexicon::load(concat!(
        "MEOWPUUT-PINYIN-1\n",
        "hh\th3h3\t好好\t好好\t900\n",
        "ss\ts1s3\t搜索\t搜索\t800\n",
        "xz\tx4z4\t現在\t现在\t1125\n",
    )).unwrap());
    let mut session = Session::new_nine_key(lexicon).unwrap();
    session.set_simplified(true);
    typed(&mut session, "7744");
    assert_eq!((session.candidate(0), session.preedit()), ("搜索好好", "s s h h"));
    typed(&mut session, "77'44");
    assert_eq!((session.candidate(0), session.preedit()), ("搜索好好", "s s'h h"));
    typed(&mut session, "7'744");
    assert_eq!(session.candidate_count(), 0);
    // Not when one abbreviated word reads all of the keys.
    typed(&mut session, "99");
    assert_eq!(candidates(&session), ["现在"]);
}

#[test]
fn nine_key_spelled_input_keeps_its_place() {
    let lexicon = Arc::new(Lexicon::load(concat!(
        "MEOWPUUT-PINYIN-1\n",
        "BUG\tBUG5\tbug\tbug\t9000\n",
        "TOMATO\tTOMATO5\ttomato\ttomato\t9000\n",
        "cui\tcui1\t催\t催\t3000\n",
        "men\tmen5\t們\t们\t7000\n",
        "ta\tta1\t他\t他\t8000\n",
        "ta\tta1\t它\t它\t7000\n",
        "tamen\tta1men5\t他們\t他们\t8000\n",
        "tmx\tt2m4x2\t談模型\t谈模型\t900\n",
        "tw\tt1w4\t他們\t他们\t900\n",
        "wo\two3\t我\t我\t9000\n",
        "wt\tw4t2\t問題\t问题\t1200\n",
        "wu\twu2\t無\t无\t7000\n",
    )).unwrap());
    let mut session = Session::new_nine_key(lexicon).unwrap();
    session.set_simplified(true);

    // The first key of the next syllable keeps the spelled prefix first.
    typed(&mut session, "826369");
    assert_eq!((session.candidate(0), session.preedit()), ("他们", "ta men 9"));
    // A spelled syllable keeps a full page of its own before abbreviated words.
    typed(&mut session, "98");
    assert_eq!(session.candidate(0), "无");
    assert!(candidates(&session).contains(&"问题"));
    // English follows a spelled syllable, even a heavy word.
    typed(&mut session, "284");
    assert_eq!((session.candidate(0), session.candidate(1)), ("催", "bug"));
    // It leads keys that spell no pinyin.
    typed(&mut session, "866286");
    assert_eq!((session.candidate(0), session.preedit()), ("tomato", "tomato"));
}

#[test]
fn nine_key_offers_english_words_by_their_letters() {
    let lexicon = Arc::new(Lexicon::load(concat!(
        "MEOWPUUT-PINYIN-1\n",
        "AN\tAN5\tAN\tAN\t5000\n",
        "IPHONE\tIPHONE5\tiPhone\tiPhone\t4553\n",
        "OB\tOB5\tOB\tOB\t5000\n",
        "OK\tOK5\tOK\tOK\t5211\n",
        "WIFI\tWIFI5\tWi-Fi\tWi-Fi\t6030\n",
        "an\tan1\t安\t安\t5500\n",
        "ma\tma1\t媽\t妈\t8000\n",
        "o\to4\t哦\t哦\t3000\n",
    )).unwrap());
    let mut session = Session::new_nine_key(lexicon).unwrap();

    // 65 spells no pinyin: OK leads and the preedit shows its letters.
    typed(&mut session, "65");
    assert_eq!((session.candidate(0), session.preedit()), ("OK", "ok"));
    assert!(candidates(&session).contains(&"哦"));
    assert!(session.choose(0));
    assert_eq!((session.committed(), session.input()), ("OK", ""));
    typed(&mut session, "474663");
    assert_eq!((session.candidate(0), session.preedit()), ("iPhone", "iphone"));
    typed(&mut session, "9434");
    assert_eq!(session.candidate(0), "Wi-Fi");
    let warm = session.capacity_signature();
    typed(&mut session, "65");
    assert_eq!(session.capacity_signature(), warm, "an English word changed the session buffers");

    // A common spelled Chinese word stays first.
    typed(&mut session, "62");
    assert_eq!((session.candidate(0), session.preedit()), ("媽", "ma"));
    assert!(candidates(&session).contains(&"OB"));
    // Letters that are one pinyin syllable get no English word.
    typed(&mut session, "26");
    assert_eq!(candidates(&session), ["安"]);

    // A word break is not inside an English word.
    typed(&mut session, "6'5");
    assert!(!candidates(&session).contains(&"OK"));
}

#[test]
fn nine_key_space_keeps_unmatched_digits_and_trailing_vowel_stays_prefix() {
    let lexicon = Arc::new(Lexicon::load(concat!(
        "MEOWPUUT-PINYIN-1\n",
        "a\ta5\t啊\t啊\t8500\n",
        "e\te4\t餓\t饿\t8500\n",
        "hao\thao3\t好\t好\t9000\n",
        "haode\thao3de5\t好的\t好的\t9100\n",
        "o\to1\t喔\t喔\t8500\n",
        "qi\tqi3\t起\t起\t5000\n",
        "qilai\tqi3lai2\t起來\t起来\t6000\n",
        "ql\tq3l2\t起來\t起来\t750\n",
        "wo\two3\t我\t我\t9500\n",
        "xz\tx4z4\t現在\t现在\t1125\n",
    )).unwrap());
    let mut session = Session::new_nine_key(lexicon).unwrap();
    session.set_simplified(true);

    // #59: Space on unmatched 9-key digits must NOT silently commit raw digits + space.
    typed(&mut session, "999");
    assert_eq!(session.candidate_count(), 0);
    assert!(!session.key(i32::from(b' ')));
    assert_eq!((session.input(), session.preedit(), session.committed()), ("999", "999", ""));
    // Explicit Enter still commits raw digits when desired.
    assert!(session.key(0xff0d));
    assert_eq!((session.committed(), session.input()), ("999", ""));

    // Trailing single-digit vowel trap (4263 = hao + d/e): prefix "好" must stay ahead of "好饿".
    typed(&mut session, "4263");
    assert_eq!(session.candidate(0), "好");
    assert_eq!(session.preedit(), "hao 3");

    // Two-word initials (ql + xz = 7599) compose even when 74/75 prefix has lattice activity.
    typed(&mut session, "7599");
    assert!(candidates(&session).contains(&"起来现在"));
}
