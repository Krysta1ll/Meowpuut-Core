//! The shipped word-pair table belongs to the shipped lexicon, is read in
//! place without heap, decoding with it does not grow the session's buffers
//! after a warm-up, and a table for another word list is refused.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use meowpuut_core::pairs::{Pairs, END, START};
use meowpuut_core::{Lexicon, Session};

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
