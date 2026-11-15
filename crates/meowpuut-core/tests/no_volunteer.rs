//! The never-volunteer list (`FLAG_NO_SUGGEST`, Resources/Lexicon/blocked-terms.txt): a marked word
//! is offered for its own full spelling and nowhere else. A small lexicon with a neutral word marked
//! shows each path; the shipped lexicon is checked for next-word associations and initials (the
//! words come from the image's flags, so none is written here). scripts/check-no-volunteer.py
//! checks the shipped data through the C interface in more ways.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use meowpuut_core::pairs::Pairs;
use meowpuut_core::soa::FLAG_NO_SUGGEST;
use meowpuut_core::workspace::DecodeOutputSoA;
use meowpuut_core::{Lexicon, Session};

/// 咖啡館 stands in for a marked word: it has the sixth field `n`, an initials row (the build
/// writes none for a marked word; the engine must not offer it either) and a context row.
const TSV: &str = concat!(
    "MEOWPUUT-PINYIN-1\n",
    "fei\tfei1\t啡\t啡\t3000\n",
    "guan\tguan3\t館\t馆\t4000\n",
    "guang\tguang1\t光\t光\t4500\n",
    "ka\tka1\t咖\t咖\t3000\n",
    "kafei\tka1fei1\t咖啡\t咖啡\t5000\n",
    "kafeiguan\tka1fei1guan3\t咖啡館\t咖啡馆\t4800\tn\n",
    "kfg\tk1f1g3\t咖啡館\t咖啡馆\t600\tn\n",
    "# context\n",
    "咖啡館\t3000\n",
    "咖啡館裡\t2000\n",
);

fn typed(session: &mut Session, keys: &str) -> Vec<String> {
    session.clear();
    for key in keys.bytes() {
        session.key(i32::from(key));
    }
    (0..session.candidate_count()).map(|i| session.candidate(i).to_string()).collect()
}

#[test]
fn a_marked_word_is_offered_only_for_its_full_spelling() {
    let lex = Arc::new(Lexicon::load(TSV).unwrap());
    assert!(lex.is_no_suggest_text("咖啡館") && lex.is_no_suggest_text("咖啡馆"));
    assert!(lex.words.is_no_suggest(lex.range("kafeiguan").0));
    assert!(lex.spells_no_suggest("kafeiguan", "咖啡館") && lex.spells_no_suggest("ka'fei'guan", "咖啡館"));
    assert!(!lex.spells_no_suggest("kfg", "咖啡館") && !lex.spells_no_suggest("kafeigua", "咖啡館"));
    let mut session = Session::new(Arc::clone(&lex));
    assert_eq!(typed(&mut session, "kafeiguan")[0], "咖啡館");
    for keys in ["kfg", "kafeigua", "kafeigu", "kafeiguang", "kafeiguam", "kafeiugan", "kafeig"] {
        let row = typed(&mut session, keys);
        assert!(!row.iter().any(|c| c.contains("咖啡館")), "{keys}: {row:?}");
    }
}

#[test]
fn a_marked_word_is_never_a_next_word() {
    let lex = Lexicon::load(TSV).unwrap();
    let (mut raw, mut out) = (DecodeOutputSoA::with_capacity(64), DecodeOutputSoA::with_capacity(16));
    for context in ["咖", "咖啡", "我們的咖啡"] {
        lex.predict_into(context, false, 8, &mut raw, &mut out);
        assert!(!(0..out.len()).any(|i| out.text(i) == "館" || out.text(i).starts_with("館")), "{context}");
    }
    assert!(lex.completes_no_suggest("咖啡", "館") && !lex.completes_no_suggest("咖啡", "店"));
}

#[test]
fn volunteers_reads_the_consumed_keys() {
    let lex = Lexicon::load(TSV).unwrap();
    assert!(!lex.volunteers("kafeiguan", "咖啡館"));
    assert!(lex.volunteers("kfg", "咖啡館") && lex.volunteers("kafeigua", "咖啡馆"));
    // Inside a longer text: spelled in the keys or not.
    assert!(!lex.volunteers("wodekafeiguan", "我的咖啡館"));
    assert!(lex.volunteers("wodekfg", "我的咖啡館"));
    assert!(lex.mentions_no_suggest("我的咖啡館") && !lex.mentions_no_suggest("我的咖啡"));
}

fn generated(name: &str) -> Option<&'static [u8]> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.build/lexicon").join(name);
    fs::read(path).ok().map(|bytes| &*Box::leak(bytes.into_boxed_slice()))
}

#[test]
fn shipped_marked_words_are_not_volunteered() {
    let (Some(base), Some(pairs)) = (generated("mimo_base.bin"), generated("mimo_pairs.bin")) else {
        eprintln!("skipped: .build/lexicon not built (scripts/build-lexicon.sh)");
        return;
    };
    let mut lex = Lexicon::from_base_in_place(base, true, None).unwrap();
    let table = Pairs::in_place(pairs, &lex.index().word_ids, None).unwrap();
    lex.set_pairs(table);
    let lex = Arc::new(lex);
    // Marked words with a full Pinyin reading: (Simplified text, reading). A word that is on the
    // list only as a whole word (=term, FLAG_NO_SUGGEST_WHOLE: 靖国) is not looked for inside a
    // longer text (靖国安, 默克尔细胞), so for it only the word itself counts.
    let mut marked: Vec<(String, String)> = Vec::new();
    let mut whole_only = std::collections::HashSet::new();
    for i in 0..lex.words.len() {
        let reading = lex.words.reading(&lex.storage, i);
        if lex.words.flags(i) & FLAG_NO_SUGGEST != 0 && lex.words.is_fully_spelled(i) && reading.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()) {
            marked.push((lex.words.text(&lex.storage, i, true).to_string(), reading.to_string()));
            if lex.words.is_no_suggest_whole(i) {
                whole_only.insert(lex.words.text(&lex.storage, i, true).to_string());
            }
        }
    }
    marked.sort();
    marked.dedup_by(|a, b| a.0 == b.0);
    assert!(marked.len() > 500, "only {} marked words: is the list applied?", marked.len());
    // Next-word associations after each first part of a marked word (the predictor reads the
    // last three characters, so a part of up to three).
    let (mut raw, mut out) = (DecodeOutputSoA::with_capacity(128), DecodeOutputSoA::with_capacity(16));
    let mut checked = 0;
    for (text, _) in &marked {
        let chars: Vec<char> = text.chars().collect();
        for cut in 1..chars.len().min(4) {
            let context: String = chars[..cut].iter().collect();
            lex.predict_into(&context, true, 16, &mut raw, &mut out);
            for i in 0..out.len() {
                let joined = format!("{context}{}", out.text(i));
                let completes = if whole_only.contains(text) { joined == *text } else { joined.contains(text.as_str()) };
                assert!(!completes, "a next word completes a marked word: {context} -> {}", out.text(i));
                assert!(!lex.is_no_suggest_text(out.text(i)), "a marked word as the next word");
            }
            checked += 1;
        }
    }
    assert!(checked > 1000);
    // Initials of each marked word of two syllables or more.
    let mut session = Session::new(Arc::clone(&lex));
    session.set_simplified(true);
    for (text, reading) in &marked {
        let initials: String = reading.split(|c: char| c.is_ascii_digit()).filter(|s| !s.is_empty()).map(|s| &s[..1]).collect();
        if initials.len() < 2 {
            continue;
        }
        let row = typed(&mut session, &initials);
        assert!(!row.iter().take(10).any(|c| c == text), "{initials}: a marked word for its initials");
    }
}

#[test]
fn an_everyday_word_that_holds_a_marked_word_is_offered() {
    // 咖啡館 is marked; 咖啡館街 stands in for an everyday word that holds it ([zh-keep], `k`).
    let tsv = TSV
        .replace("kfg\t", "kafeiguanjie\tka1fei1guan3jie1\t咖啡館街\t咖啡馆街\t3900\tk\nkfg\t")
        .replace("# context\n", "kfgj\tk1f1g3j1\t咖啡館街\t咖啡馆街\t500\tk\n# context\n");
    let lex = Lexicon::load(&tsv).unwrap();
    assert!(lex.is_listed_keep_text("咖啡館街") && !lex.is_no_suggest_text("咖啡館街"));
    assert!(!lex.volunteers("kfgj", "咖啡館街") && !lex.volunteers("kfgjk", "咖啡館街咖"));
    assert!(lex.volunteers("kfgk", "咖啡館咖"));
    // mpn_text_no_suggest: a text that is or holds a marked word, not one inside the everyday word.
    assert!(lex.holds_no_suggest("咖啡館") && lex.holds_no_suggest("我的咖啡館"));
    assert!(!lex.holds_no_suggest("咖啡館街") && !lex.holds_no_suggest("我的咖啡"));
}

#[test]
fn zhuyin_with_the_last_tone_only_spells_a_marked_word() {
    // ㄓㄨㄥ ㄍㄨㄥˋ: typed 5j/ ej/4 (every tone) or 5j/ej/4 (the last tone only, as for any word).
    let tsv = "MEOWPUUT-ZHUYIN-1\n5j/\t5j/ \t中\t中\t5000\n5j/ej/\t5j/ ej/ \t中工\t中工\t3500\n5j/ej/\t5j/ ej/4\t中共\t中共\t3000\tn\n";
    let lex = Lexicon::load(tsv).unwrap();
    assert!(lex.spells_no_suggest("5j/ ej/4", "中共") && lex.spells_no_suggest("5j/ej/4", "中共"));
    assert!(!lex.spells_no_suggest("5j/e", "中共"));
    // ㄕㄨㄟˇ is 水 as well as ㄕ(ˋ)ㄨㄟˇ for 市委: without the first tone it is not 市委's spelling.
    let tsv = "MEOWPUUT-ZHUYIN-1\ngjo\tgjo3\t水\t水\t5000\ngjo\tg4jo3\t市委\t市委\t3000\tn\n";
    let lex = Lexicon::load(tsv).unwrap();
    assert!(lex.spells_no_suggest("g4jo3", "市委") && !lex.spells_no_suggest("gjo3", "市委"));
}
