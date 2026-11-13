//! The shipped word-pair table belongs to the shipped lexicon, is read in
//! place without heap, decoding with it does not grow the session's buffers
//! after a warm-up, and a table for another word list is refused.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use meowpuut_core::pairs::{Pairs, END, START};
use meowpuut_core::{CandidateKind, Lexicon, Session};

fn generated(name: &str) -> &'static [u8] {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.build/lexicon").join(name);
    let bytes = fs::read(&path).unwrap_or_else(|_| panic!("{} missing; run scripts/build-lexicon.sh", path.display()));
    Box::leak(bytes.into_boxed_slice())
}

#[test]
fn shipped_pairs_attach_in_place() {
    let (base, pairs) = (generated("mimo_base.bin"), generated("mimo_pairs.bin"));
    for pinyin in [true, false] {
        let mut lex = Lexicon::from_base_in_place(base, pinyin, None).unwrap();
        let table = Pairs::in_place(pairs, &lex.index().word_ids, None).expect("pair table for this lexicon");
        // A clause more often starts with 我 than ends with it.
        let wo = (0..lex.words.len()).find(|&i| lex.words.text(&lex.storage, i, true) == "我").unwrap();
        let id = lex.index().word_ids.ids.get(wo);
        assert!(table.score(START, id) > table.score(id, END));
        lex.set_pairs(table);
        assert_eq!(lex.heap_bytes(), 0);
        let mut session = Session::new(Arc::new(lex));
        let keys = if pinyin { "wozuotianwanshangqulejiajinfandiancheferanhoumaileyixieshuiguo" } else { "ji3y94wu0vu/6 xu.6vul3" };
        let typed = |session: &mut Session| {
            session.clear();
            for k in keys.bytes() {
                session.key(i32::from(k));
            }
        };
        typed(&mut session);
        let warm = session.capacity_signature();
        typed(&mut session);
        assert_eq!(session.capacity_signature(), warm, "decoding with pairs grew the buffers");
    }
    let lex = Lexicon::from_base_in_place(base, true, None).unwrap();
    let mut other = pairs.to_vec();
    other[24] ^= 1;
    assert!(Pairs::load(&other, &lex.index().word_ids).is_none());
    let copied = Lexicon::load_binary(&base[64..64 + u32::from_le_bytes(base[20..24].try_into().unwrap()) as usize]).unwrap();
    assert!(Pairs::load(pairs, &copied.index().word_ids).is_none(), "a lexicon without word ids takes no table");
}

/// With the pair table, a complete syllable gives its word first, not an
/// abbreviation of two words (ma: 嗎, not 默哀 = m + 哀), and letters that
/// spell no word still give their abbreviation. Android and Windows show the
/// core's order as it is.
#[test]
fn shipped_pairs_first_candidates() {
    let mut lex = Lexicon::from_base_in_place(generated("mimo_base.bin"), true, None).unwrap();
    let table = Pairs::in_place(generated("mimo_pairs.bin"), &lex.index().word_ids, None).unwrap();
    lex.set_pairs(table);
    let mut session = Session::new(Arc::new(lex));
    let cases = [
        ("ma", "嗎", "吗"),
        ("ta", "他", "他"),
        ("de", "的", "的"),
        ("ba", "吧", "吧"),
        ("ha", "哈", "哈"),
        ("la", "啦", "啦"),
        ("ne", "呢", "呢"),
        ("wo", "我", "我"),
        ("ni", "你", "你"),
        ("shi", "是", "是"),
        ("nihao", "你好", "你好"),
        ("xiexie", "謝謝", "谢谢"),
        ("women", "我們", "我们"),
        ("xkl", "辛苦了", "辛苦了"),
        ("zenmb", "怎麼辦", "怎么办"),
        ("nh", "你好", "你好"),
    ];
    for (keys, hant, hans) in cases {
        for (simplified, expected) in [(false, hant), (true, hans)] {
            session.clear();
            session.set_simplified(simplified);
            for k in keys.bytes() {
                session.key(i32::from(k));
            }
            assert_eq!(session.candidate(0), expected, "{keys}, simplified {simplified}");
        }
    }
}

#[test]
fn shipped_pinyin_corrections_keep_tones_and_original_byte_offsets() {
    let mut lex = Lexicon::from_base_in_place(generated("mimo_base.bin"), true, None).unwrap();
    let table = Pairs::in_place(generated("mimo_pairs.bin"), &lex.index().word_ids, None).unwrap();
    lex.set_pairs(table);
    let mut session = Session::new(Arc::new(lex));
    let cases = [
        ("fanfa", "方法", "方法"), ("fangqie", "番茄", "番茄"),
        ("penyou", "朋友", "朋友"), ("zhengde", "真的", "真的"),
        ("jingzhang", "緊張", "紧张"), ("qinchu", "清楚", "清楚"),
        ("tian'ang'men", "天安門", "天安门"), ("qin1chu3", "清楚", "清楚"),
        ("jingtianwoqukandianying", "今天我去看電影", "今天我去看电影"),
        ("mingtianyaogenpenyoujianmian", "明天要跟朋友見面", "明天要跟朋友见面"),
        ("woxihuanfangdian", "我喜歡飯店", "我喜欢饭店"),
        ("wodexingwen", "我的新聞", "我的新闻"),
        ("gognzuo", "工作", "工作"), ("gonggzuo", "工作", "工作"),
        ("gogzuo", "工作", "工作"), ("zhogguo", "中國", "中国"),
        ("xihun", "喜歡", "喜欢"),
    ];
    for (keys, hant, hans) in cases {
        for (simplified, expected) in [(false, hant), (true, hans)] {
            session.clear();
            session.set_simplified(simplified);
            for k in keys.bytes() { assert!(session.key(i32::from(k))); }
            let index = (0..session.candidate_count().min(5)).find(|&i| session.candidate(i) == expected)
                .unwrap_or_else(|| panic!("missing {expected} from first five for {keys}, simplified {simplified}"));
            let info = session.candidate_info(index).unwrap();
            assert_eq!(info.kind, CandidateKind::Correction as u32);
            assert_eq!(info.consumed_bytes, keys.len() as u64);
            assert_eq!(info.remaining_offset_bytes, keys.len() as u64);
            if keys == "qinchu" {
                assert_ne!(index, 0);
                assert_eq!(session.candidate_info(0).unwrap().kind, CandidateKind::Exact as u32);
            }
            assert!(session.choose(index));
            assert_eq!(session.committed(), expected);
            assert!(session.input().is_empty());
        }
    }
    for k in "qin3chu3".bytes() { assert!(session.key(i32::from(k))); }
    assert!((0..session.candidate_count()).all(|i| session.candidate(i) != "清楚"));
}

#[test]
fn nasal_correction_does_not_drop_a_following_syllables_initial() {
    let lex = Lexicon::load(concat!(
        "MEOWPUUT-PINYIN-1\n",
        "guo\tguo4\t過\t过\t4800\n",
        "ji\tji1\t基\t基\t4800\n",
        "jin\tjin1\t金\t金\t4800\n",
        "jing\tjing1\t經\t经\t4800\n",
        "jingguo\tjing1guo4\t經過\t经过\t5500\n",
        "jinguo\tjin1guo4\t金過\t金过\t4900\n",
        "jinuo\tji1nuo4\t基諾\t基诺\t5400\n",
        "nuo\tnuo4\t諾\t诺\t4800\n",
    )).unwrap();
    let mut session = Session::new(Arc::new(lex));
    for k in "jinguo".bytes() { assert!(session.key(i32::from(k))); }
    assert_eq!(session.candidate(0), "金過");
    assert!((0..session.candidate_count()).any(|i| session.candidate(i) == "經過"));
    assert!((0..session.candidate_count()).all(|i| session.candidate(i) != "基諾"));
}

#[test]
fn dropped_letter_guess_keeps_a_complete_reading_ahead() {
    let lex = Lexicon::load(concat!(
        "MEOWPUUT-PINYIN-1\n",
        "ciku\tci2ku4\t詞庫\t词库\t5500\n",
        "hkn\th1k3n2\t還可能\t还可能\t5000\n",
        "hong\thong2\t紅\t红\t4800\n",
        "po\tpo4\t破\t破\t4800\n",
        "san\tsan1\t三\t三\t4800\n",
        "shanpo\tshan1po1\t山坡\t山坡\t5500\n",
    )).unwrap();
    let mut session = Session::new(Arc::new(lex));
    for key in b"sanpo" { assert!(session.key(i32::from(*key))); }
    assert_eq!(session.candidate(0), "三破");
    assert!((1..session.candidate_count()).any(|i| session.candidate(i) == "山坡"));

    for (input, unrelated) in [(b"hon", "還可能"), (b"iku", "詞庫")] {
        session.clear();
        for key in input { assert!(session.key(i32::from(*key))); }
        assert!((0..session.candidate_count()).all(|i| session.candidate(i) != unrelated));
    }
}

#[test]
fn unverified_complete_paths_stay_ahead_of_nasal_guesses() {
    let lex = Lexicon::from_base_in_place(generated("mimo_base.bin"), true, None).unwrap();
    let mut session = Session::new(Arc::new(lex));
    let cases = [
        ("mafangeiwocaidan", "麻煩給我菜單", "麻烦给我菜单"),
        ("niranguotoufa", "你染過頭髮", "你染过头发"),
    ];
    for (keys, hant, hans) in cases {
        for (simplified, expected) in [(false, hant), (true, hans)] {
            session.clear();
            session.set_simplified(simplified);
            for key in keys.bytes() { assert!(session.key(i32::from(key))); }
            assert_eq!(session.candidate(0), expected);
            let info = session.candidate_info(0).unwrap();
            assert_eq!(info.kind, CandidateKind::Unknown as u32);
            assert_eq!(info.consumed_bytes, keys.len() as u64);
        }
    }
}
