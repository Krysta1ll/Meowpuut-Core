#![forbid(unsafe_code)]
//! Comprehensive tests for the nine-key (`NineKeyState`) overhaul:
//! single-key common initials (`7` -> `是`), beam prefix diversity (`9264468482` -> `往後滑`),
//! mixed spelled-plus-initials (`74366` -> `什麼`, `93662` -> `怎麼辦`), syllable locking
//! (`WANG'468482`, `WANG'HOU'482`), zero-allocation SoA scratch across 100 random seeds,
//! and Pinyin syllable rail enumeration.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use meowpuut_core::english::{pack as pack_english, English};
use meowpuut_core::nine_key::syllables::first_unlocked_digits;
use meowpuut_core::nine_key::{digit, key_digit, key_letters, key_matches, KeyInputSoA};
use meowpuut_core::pairs::Pairs;
use meowpuut_core::{CandidateKind, Lexicon, Session, MAX_INPUT};

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn type_str(session: &mut Session, raw: &str) {
    session.clear();
    for b in raw.bytes() {
        assert!(session.key(i32::from(b)), "rejected byte {:?}", char::from(b));
    }
}

fn all_candidates(session: &Session) -> Vec<&str> {
    (0..session.candidate_count()).map(|i| session.candidate(i)).collect()
}

#[test]
fn nine_key_single_key_initials_and_trailing_initials() {
    let tsv = concat!(
        "MEOWPUUT-PINYIN-1\n",
        "a\ta5\t啊\t啊\t7983\n",
        "bu\tbu4\t不\t不\t9190\n",
        "de\tde5\t的\t的\t10000\n",
        "e\te4\t餓\t饿\t6144\n",
        "hao\thao3\t好\t好\t9000\n",
        "le\tle5\t了\t了\t10000\n",
        "ma\tma5\t嗎\t吗\t8488\n",
        "ni\tni3\t你\t你\t9000\n",
        "nihao\tni3hao3\t你好\t你好\t9200\n",
        "o\to1\t喔\t喔\t7168\n",
        "qu\tqu4\t去\t去\t9062\n",
        "ren\tren2\t人\t人\t9058\n",
        "shi\tshi4\t是\t是\t10000\n",
        "shuo\tshuo1\t說\t说\t9090\n",
        "ta\tta1\t他\t他\t8800\n",
        "wo\two3\t我\t我\t10000\n",
        "zai\tzai4\t在\t在\t9724\n",
        "zang\tzang1\t髒\t脏\t7000\tn\n",
    );
    let lex = Arc::new(Lexicon::load(tsv).unwrap());
    let mut s = Session::new_nine_key(lex).unwrap();
    s.set_simplified(true);

    // Key 7 -> 是 first, never-volunteer excluded, preedit stays "7".
    type_str(&mut s, "7");
    assert_eq!(s.candidate(0), "是");
    assert_eq!(s.preedit(), "7");
    assert_eq!(s.candidate_info(0).unwrap().kind, CandidateKind::Correction as u32);
    let c7 = all_candidates(&s);
    assert!(c7.contains(&"说") && c7.contains(&"去") && c7.contains(&"人"));

    // Key 9 -> 我 first, 在 second, FLAG_NO_SUGGEST (脏) never volunteered.
    type_str(&mut s, "9");
    assert_eq!(s.candidate(0), "我");
    assert!(all_candidates(&s).contains(&"在"));
    assert!(!all_candidates(&s).contains(&"脏"));

    // Keys 2, 3, 4, 5, 6, 8 all offer their top daily characters.
    type_str(&mut s, "2");
    assert_eq!(s.candidate(0), "不");
    assert!(all_candidates(&s).contains(&"啊"));

    type_str(&mut s, "3");
    assert_eq!(s.candidate(0), "的");
    assert!(all_candidates(&s).contains(&"饿"));

    type_str(&mut s, "4");
    assert_eq!(s.candidate(0), "好");
    type_str(&mut s, "5");
    assert_eq!(s.candidate(0), "了");
    type_str(&mut s, "6");
    assert_eq!(s.candidate(0), "你");
    type_str(&mut s, "8");
    assert_eq!(s.candidate(0), "他");

    // Trailing single-key initial after spelled syllables:
    // 967 (wo + s -> 我 prefix + 我是), 963 (wo + e/d -> 我饿 + 我的), 644266 (ni hao + m -> 你好吗).
    type_str(&mut s, "967");
    assert_eq!((s.candidate(0), s.preedit()), ("我", "wo 7"));
    assert!(all_candidates(&s).contains(&"我是"));

    type_str(&mut s, "963");
    assert!(all_candidates(&s).contains(&"我的"));

    type_str(&mut s, "644267");
    assert_eq!((s.candidate(0), s.preedit()), ("你好", "ni hao 7"));
    assert!(all_candidates(&s).contains(&"你好是"));

    type_str(&mut s, "644266");
    assert!(all_candidates(&s).contains(&"你好吗"));
}

#[test]
fn nine_key_beam_diversity_and_syllable_locking_preserve_competing_phrases() {
    let tsv = concat!(
        "MEOWPUUT-PINYIN-1\n",
        "gou\tgou4\t購\t购\t6200\n",
        "gua\tgua4\t掛\t挂\t6390\n",
        "hou\thou4\t後\t后\t6000\n",
        "hua\thua4\t話\t话\t7368\n",
        "hua\thua1\t花\t花\t7272\n",
        "hua\thua4\t畫\t画\t7048\n",
        "hua\thua2\t滑\t滑\t6372\n",
        "hua\thua4\t化\t化\t6372\n",
        "hua\thua2\t華\t华\t6098\n",
        "wa\twa1\t哇\t哇\t5861\n",
        "wan\twan2\t完\t完\t7262\n",
        "wang\twang2\t王\t王\t7400\n",
        "wang\twang3\t網\t网\t7200\n",
        "wang\twang3\t往\t往\t7057\n",
        "wanggou\twang3gou4\t網購\t网购\t6400\n",
        "wanghou\twang3hou4\t往後\t往后\t4818\n",
        "ya\tya1\t呀\t呀\t7128\n",
        "yan\tyan2\t鹽\t盐\t6352\n",
        "yang\tyang4\t樣\t样\t7500\n",
    );
    let lex = Arc::new(Lexicon::load(tsv).unwrap());
    let mut s = Session::new_nine_key(lex).unwrap();
    s.set_simplified(true);

    // Without locking, 9264468482 must retain 往后滑 in the first page of the expanded panel instead of evicting it.
    type_str(&mut s, "9264468482");
    let cands = all_candidates(&s);
    let pos = cands.iter().position(|&c| c == "往后滑").expect("往后滑 must be offered");
    assert!(pos < 16, "往后滑 should appear in top 16, got index {pos}: {cands:?}");
    assert!(cands.contains(&"往后") && cands.contains(&"往"));

    // Syllable rail options for 9264468482: longest common syllables first.
    let mut sylls = [""; 32];
    let n = s.nine_key_syllables(&mut sylls);
    assert_eq!(&sylls[..n], &["yang", "wang", "wan", "yan", "ya", "wa"]);

    // Locking 1st syllable "WANG'" eliminates "yang" and advances syllable rail to 468482 ("hou", "gou").
    type_str(&mut s, "WANG'468482");
    assert!(!all_candidates(&s).contains(&"样"));
    let mut sylls2 = [""; 32];
    let n2 = s.nine_key_syllables(&mut sylls2);
    assert_eq!(&sylls2[..n2], &["gou", "hou"]);

    // Locking 2nd syllable "WANG'HOU'482" eliminates 网购* completely!
    type_str(&mut s, "WANG'HOU'482");
    let locked = all_candidates(&s);
    assert!(!locked.iter().any(|c| c.starts_with("网购")));
    assert!(locked.contains(&"往后滑"));
    assert_eq!(s.preedit(), "wang'hou'hua");

    let mut raw_text = String::new();
    s.input_text_into(&mut raw_text);
    assert_eq!(raw_text, "wang'hou'482");
}

#[test]
fn nine_key_mixed_spelled_and_multi_initials_and_english_tier() {
    let mut lex = Lexicon::load(concat!(
        "MEOWPUUT-PINYIN-1\n",
        "ban\tban4\t辦\t办\t7000\n",
        "bu\tbu4\t不\t不\t9190\n",
        "buzhidao\tbu4zhi1dao4\t不知道\t不知道\t7421\n",
        "bzd\tb4z1d4\t不知道\t不知道\t928\n",
        "qian\tqian2\t錢\t钱\t8500\n",
        "shen\tshen2\t什\t什\t4020\n",
        "shenme\tshen2me5\t什麼\t什么\t9000\n",
        "sm\ts2m5\t什麼\t什么\t1125\n",
        "zen\tzen3\t怎\t怎\t5042\n",
        "zenmeban\tzen3me5ban4\t怎麼辦\t怎么办\t6891\n",
        "zmb\tz3m5b4\t怎麼辦\t怎么办\t861\n",
    )).unwrap();
    let en_img = pack_english(
        "home\thome\t900\t-\ngood\tgood\t950\t-\nok\tOK\t800\tacronym,in-chinese\n",
        "",
        10,
    ).unwrap();
    lex.set_english(English::load(&en_img).unwrap());
    let mut s = Session::new_nine_key(Arc::new(lex)).unwrap();
    s.set_simplified(true);

    type_str(&mut s, "74366");
    assert!(all_candidates(&s).contains(&"什么"));

    type_str(&mut s, "93662");
    assert!(all_candidates(&s).contains(&"怎么办"));

    type_str(&mut s, "2893");
    assert!(all_candidates(&s).contains(&"不知道"));

    // Second-tier English from mimo_en.bin appears on 4663 ("good", "home").
    type_str(&mut s, "4663");
    let c4663 = all_candidates(&s);
    assert!(c4663.contains(&"good") && c4663.contains(&"home"));
}

#[test]
fn nine_key_zero_allocation_and_100_seed_determinism() {
    let tsv = concat!(
        "MEOWPUUT-PINYIN-1\n",
        "OK\tOK5\tOK\tOK\t5200\n",
        "bu\tbu4\t不\t不\t9000\n",
        "de\tde5\t的\t的\t9800\n",
        "gou\tgou4\t購\t购\t6200\n",
        "hao\thao3\t好\t好\t8800\n",
        "hou\thou4\t後\t后\t6000\n",
        "hua\thua4\t話\t话\t7368\n",
        "hua\thua2\t滑\t滑\t6372\n",
        "nh\tn3h3\t你好\t你好\t900\n",
        "ni\tni3\t你\t你\t9000\n",
        "nihao\tni3hao3\t你好\t你好\t9200\n",
        "shi\tshi4\t是\t是\t9900\n",
        "wang\twang3\t往\t往\t7057\n",
        "wanggou\twang3gou4\t網購\t网购\t6400\n",
        "wanghou\twang3hou4\t往後\t往后\t4818\n",
        "wo\two3\t我\t我\t9900\n",
        "xz\tx4z4\t現在\t现在\t1125\n",
    );
    let lex = Arc::new(Lexicon::load(tsv).unwrap());
    let mut s1 = Session::new_nine_key(Arc::clone(&lex)).unwrap();
    let mut s2 = Session::new_nine_key(lex).unwrap();
    s1.add_japanese_word("sushi", "寿司", 9000);
    s2.add_japanese_word("sushi", "寿司", 9000);

    let pool = [
        "7", "9", "64426", "9264468482", "WANG'468482", "99", "65", "78744",
        "963", "546'26", "999'999", "64426965",
    ];

    // Warmup pass.
    for p in pool {
        type_str(&mut s1, p);
        type_str(&mut s2, p);
    }
    let warm_sig = s1.capacity_signature();

    let run_one = |s: &mut Session, seed: u64| -> u64 {
        let mut rng = seed;
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        s.clear();
        for _ in 0..30 {
            let r = splitmix64(&mut rng);
            match r % 5 {
                0..=2 => {
                    let seq = pool[((r >> 8) as usize) % pool.len()];
                    for &b in seq.as_bytes() {
                        let _ = s.key(i32::from(b));
                    }
                }
                3 => { let _ = s.key(0xff08); }
                _ => {
                    let cnt = s.candidate_count();
                    if cnt > 0 { let _ = s.choose(((r >> 16) as usize) % cnt); }
                }
            }
            for b in s.preedit().bytes().chain(s.committed().bytes()) {
                h = (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
            }
            let n_syl = {
                let mut sylls = [""; 32];
                s.nine_key_syllables(&mut sylls)
            };
            h ^= n_syl as u64;
        }
        h
    };

    for seed in 0..100u64 {
        let h1 = run_one(&mut s1, seed);
        let h2 = run_one(&mut s2, seed);
        assert_eq!(h1, h2, "non-deterministic nine-key output at seed {seed}");
        assert_eq!(s1.capacity_signature(), warm_sig, "nine-key hot loop grew heap at seed {seed}");
    }
}

#[test]
fn nine_key_primitives_and_edge_cases() {
    assert_eq!(digit(b'w'), b'9');
    assert_eq!(key_digit(b'W'), b'9');
    assert_eq!(key_digit(b'7'), b'7');
    assert!(key_matches(b'w', b'W') && !key_matches(b'y', b'W') && key_matches(b'y', b'9'));
    assert_eq!(key_letters(b'7'), b"pqrs");
    assert_eq!(key_letters(b'S'), b"s");
    assert_eq!(key_letters(b'1'), b"");
    assert_eq!(first_unlocked_digits(b"WANG'468'2"), b"468");
    assert_eq!(first_unlocked_digits(b"WANG'"), b"");
    let parsed = KeyInputSoA::from_raw(b"92'64");
    assert_eq!((parsed.slice(), parsed.breaks_slice()), (&b"9264"[..], &[2u16][..]));

    // MAX_INPUT rollover with locked letters and separator.
    let lex = Arc::new(Lexicon::load("MEOWPUUT-PINYIN-1\nni\tni3\t你\t你\t5000\n").unwrap());
    let mut s = Session::new_nine_key(lex).unwrap();
    for _ in 0..MAX_INPUT {
        assert!(s.key(i32::from(b'W')));
    }
    assert!(s.key(i32::from(b'\'')));
    assert!(s.key(i32::from(b'6')));
    assert_eq!(s.committed().len(), MAX_INPUT);

    // Optional live check when MEOWPUUT_GENERATED_DIR is provided.
    if let Some(dir) = std::env::var_os("MEOWPUUT_GENERATED_DIR") {
        let dir = Path::new(&dir);
        let base = fs::read(dir.join("mimo_base.bin")).unwrap();
        let pairs_bytes = fs::read(dir.join("mimo_pairs.bin")).unwrap();
        let mut live = Lexicon::load_from_base(&base, true).unwrap();
        let pairs = Pairs::in_place(Box::leak(pairs_bytes.into_boxed_slice()), &live.index().word_ids, None).unwrap();
        live.set_pairs(pairs);
        let mut ls = Session::new_nine_key(Arc::new(live)).unwrap();
        ls.set_simplified(true);
        for (k, want) in [("7", "是"), ("9", "我"), ("3", "的"), ("6", "你"), ("5", "了"), ("2", "不")] {
            type_str(&mut ls, k);
            assert_eq!(ls.candidate(0), want, "single key {k} top candidate");
        }
        type_str(&mut ls, "9264468482");
        let top20: Vec<_> = (0..ls.candidate_count().min(20)).map(|i| ls.candidate(i)).collect();
        println!("LIVE 9264468482 top20: {top20:?}");
        assert!(top20.contains(&"往后滑"), "往后滑 missing from live top20: {top20:?}");
        type_str(&mut ls, "WANG'HOU'482");
        let top5: Vec<_> = (0..ls.candidate_count().min(5)).map(|i| ls.candidate(i)).collect();
        println!("LIVE WANG'HOU'482 top5: {top5:?}");
        assert!(top5.contains(&"往后滑"), "往后滑 missing from live locked top5: {top5:?}");
        type_str(&mut ls, "74366");
        let c74366 = all_candidates(&ls);
        println!("LIVE 74366 top6: {:?}", &c74366[..c74366.len().min(6)]);
        assert!(c74366.contains(&"什么"));
    }
}
