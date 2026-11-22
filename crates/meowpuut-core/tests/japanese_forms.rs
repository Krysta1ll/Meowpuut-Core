//! `JapaneseQuery::lookup_extended`: inflected forms and tails, completions, leading segments.
//!
//! `tests/data/mimo_jp_forms_fixture.tsv` is the source of the image most tests use; the test
//! writer in `tests/support/mimo_jp_writer.rs` builds it. The `#[ignore]` test at the end runs
//! the acceptance list on a real image:
//! `MIMO_JP_BIN=path/to/mimo_jp.bin cargo test -p meowpuut-core --release --test japanese_forms -- --ignored --nocapture`

#[path = "support/mimo_jp_writer.rs"]
mod writer;

use std::path::{Path, PathBuf};

use meowpuut_core::japanese::{
    JapaneseLexicon, JapaneseMode, JapaneseQuery, JapaneseRow, FLAG_COMPLETION, FLAG_FORM, FLAG_PINYIN_OK,
    FLAG_REVIEWED, KIND_READING, KIND_VARIANT, OPTION_COMPLETIONS, OPTION_FORMS, OPTION_SEGMENTS,
};
use writer::Image;

use JapaneseMode::{Japanese, Pinyin};

const FORMS: u32 = OPTION_FORMS;
const ALL: u32 = OPTION_FORMS | OPTION_COMPLETIONS | OPTION_SEGMENTS;

fn data_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(name)
}

fn source(name: &str) -> Image {
    let path = data_path(name);
    Image::parse(&std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display())))
}

fn open(bytes: Vec<u8>) -> JapaneseLexicon {
    JapaneseLexicon::from_vec(bytes).expect("image opens")
}

fn fixture() -> JapaneseLexicon {
    let lexicon = open(source("mimo_jp_forms_fixture.tsv").write());
    assert!(lexicon.verify());
    lexicon
}

/// One result: text, row, consumed bytes.
#[derive(Debug, Clone, PartialEq)]
struct Found {
    text: String,
    row: JapaneseRow,
    consumed: usize,
}

fn found(lexicon: &JapaneseLexicon, input: &str, mode: JapaneseMode, options: u32) -> Vec<Found> {
    let mut query = JapaneseQuery::new();
    let count = query.lookup_extended(lexicon, input, mode, 64, options);
    assert_eq!(count, query.len());
    assert!(query.row(count).is_none());
    assert_eq!((query.text(count), query.consumed(count)), ("", 0));
    (0..count)
        .map(|index| Found {
            text: query.text(index).to_string(),
            row: query.row(index).expect("row"),
            consumed: query.consumed(index),
        })
        .collect()
}

fn texts(lexicon: &JapaneseLexicon, input: &str, mode: JapaneseMode, options: u32) -> Vec<String> {
    found(lexicon, input, mode, options).into_iter().map(|found| found.text).collect()
}

fn first(lexicon: &JapaneseLexicon, input: &str, mode: JapaneseMode, options: u32) -> Found {
    found(lexicon, input, mode, options).into_iter().next().unwrap_or_else(|| panic!("nothing for {input:?}"))
}

/// Checks that each input gives `text` as its first form, with `weight`.
fn check_firsts(lexicon: &JapaneseLexicon, cases: &[(&str, &str, i32)]) {
    for &(input, text, weight) in cases {
        let got = first(lexicon, input, Japanese, FORMS);
        assert_eq!((got.text.as_str(), got.row.weight), (text, weight), "{input}: {got:?}");
        assert_ne!(got.row.flags & FLAG_FORM, 0, "{input}");
        assert_eq!(got.row.flags & FLAG_REVIEWED, 0, "{input}");
        assert_eq!(got.consumed, input.len(), "{input}");
    }
}

#[test]
fn options_zero_is_lookup() {
    let inputs = [
        "sushi", "sensei", "sinnbun", "ko-hi-", "koohii", "tokyo", "shin'ya", "sanbu", "shiji", "hon", "au", "renai",
        "tabeta", "kawaii", "yinyue", "taberu", "tabemashita", "itta", "kyou", "kyo", "arig", "watashiha", "konai",
        "kyouhaiitenkidesune", "zzz", "a", "", "'", "-", "Sushi", "su shi", "寿司", "nnn",
    ];
    let main_fixture = open(source("mimo_jp_fixture.tsv").write());
    for lexicon in [&fixture(), &main_fixture] {
        let (mut old, mut new) = (JapaneseQuery::new(), JapaneseQuery::new());
        for input in inputs {
            for mode in [Japanese, Pinyin] {
                for limit in [0, 1, 3, 64] {
                    for options in [0, 8, 0x100] {
                        let count = old.lookup(lexicon, input, mode, limit);
                        assert_eq!(new.lookup_extended(lexicon, input, mode, limit, options), count, "{input}");
                        assert_eq!((old.kana(), old.katakana()), (new.kana(), new.katakana()), "{input}");
                        for index in 0..=count {
                            assert_eq!(old.row(index), new.row(index), "{input} {index}");
                            assert_eq!(old.text(index), new.text(index), "{input} {index}");
                            let consumed = if index < count { input.len() } else { 0 };
                            assert_eq!((old.consumed(index), new.consumed(index)), (consumed, consumed), "{input}");
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn ichidan_forms() {
    let lexicon = fixture();
    check_firsts(
        &lexicon,
        &[
            ("tabemasu", "食べます", 6700),
            ("tabemashita", "食べました", 6700),
            ("tabemasen", "食べません", 6700),
            ("tabemasendeshita", "食べませんでした", 6700),
            ("tabemashou", "食べましょう", 6700),
            ("tabemashite", "食べまして", 6700),
            ("tabete", "食べて", 6700),
            ("tabenai", "食べない", 6700),
            ("tabenakatta", "食べなかった", 6400),
            ("tabenakute", "食べなくて", 6400),
            ("tabenaide", "食べないで", 6700),
            ("tabetai", "食べたい", 6700),
            ("tabetakatta", "食べたかった", 6400),
            ("tabetakunai", "食べたくない", 6400),
            ("tabeyou", "食べよう", 6700),
            ("tabereba", "食べれば", 6700),
            ("tabero", "食べろ", 4500),
            ("tabetara", "食べたら", 6700),
            ("tabetari", "食べたり", 6700),
            ("tabenagara", "食べながら", 6700),
            ("taberareru", "食べられる", 6700),
            ("tabesaseru", "食べさせる", 6700),
            ("tabesaserareru", "食べさせられる", 6400),
            ("tabesaseraremashita", "食べさせられました", 6100),
            ("tabeteiru", "食べている", 6700),
            ("tabeteru", "食べてる", 6700),
            ("tabeteita", "食べていた", 6400),
            ("tabeteta", "食べてた", 6400),
            ("tabeteimasu", "食べています", 6400),
            ("tabetemasu", "食べてます", 6400),
            ("tabeteimashita", "食べていました", 6400),
            ("tabeteinai", "食べていない", 6400),
            ("tabetekudasai", "食べてください", 6700),
            ("tabenaidekudasai", "食べないでください", 6400),
            ("tabechau", "食べちゃう", 6700),
            ("tabechatta", "食べちゃった", 6400),
            ("tabeteshimatta", "食べてしまった", 6400),
            ("tabetemiru", "食べてみる", 6700),
            ("tabeteoku", "食べておく", 6700),
            ("tabesou", "食べそう", 6700),
            ("tabesugiru", "食べすぎる", 6700),
            ("tabeyasui", "食べやすい", 6700),
            ("tabenikui", "食べにくい", 6700),
            ("tabenasai", "食べなさい", 6700),
        ],
    );
    let row = first(&lexicon, "tabemasu", Japanese, FORMS).row;
    assert_eq!((row.word, row.kind, row.flags), (0, KIND_READING, FLAG_FORM | FLAG_PINYIN_OK));
    // Four steps (ます, てる, られる, させる) are one too many.
    assert!(texts(&lexicon, "tabesaseraretemasu", Japanese, FORMS).iter().all(|text| !text.starts_with('食')));
}

#[test]
fn godan_forms_with_onbin() {
    let lexicon = fixture();
    check_firsts(
        &lexicon,
        &[
            // く: い音便; potential, conditional, imperative and volitional on the え and お rows.
            ("kakimasu", "書きます", 6500),
            ("kaite", "書いて", 6500),
            ("kaita", "書いた", 6500),
            ("kakanai", "書かない", 6500),
            ("kakeru", "書ける", 6200),
            ("kakemasu", "書けます", 5900),
            ("kakeba", "書けば", 6500),
            ("kake", "書け", 4300),
            ("kakou", "書こう", 6500),
            ("kakareru", "書かれる", 6500),
            ("kakaseru", "書かせる", 6500),
            // ぐ: いで.
            ("oyoide", "泳いで", 4500),
            ("oyoida", "泳いだ", 4500),
            ("oyoganai", "泳がない", 4500),
            ("oyogou", "泳ごう", 4500),
            // す: して.
            ("hanashite", "話して", 6700),
            ("hanashita", "話した", 6700),
            ("hanasanai", "話さない", 6700),
            ("hanasou", "話そう", 6700),
            ("hanaseba", "話せば", 6700),
            // つ: って.
            ("matte", "待って", 6700),
            ("matta", "待った", 6700),
            ("matanai", "待たない", 6700),
            ("mate", "待て", 4500),
            ("matou", "待とう", 6700),
            // ぬ, ぶ, む: んで.
            ("shinde", "死んで", 5700),
            ("shinda", "死んだ", 5700),
            ("shinanai", "死なない", 5700),
            ("shinou", "死のう", 5700),
            ("asonde", "遊んで", 6700),
            ("asonda", "遊んだ", 6700),
            ("asobanai", "遊ばない", 6700),
            ("asobou", "遊ぼう", 6700),
            ("yonde", "読んで", 6700),
            ("yonda", "読んだ", 6700),
            ("yomanai", "読まない", 6700),
            ("yomeba", "読めば", 6700),
            ("yomou", "読もう", 6700),
            // る: って; う: って and わ before ない.
            ("kaette", "帰って", 6700),
            ("kaeranai", "帰らない", 6700),
            ("kaereru", "帰れる", 6400),
            ("kaerou", "帰ろう", 6700),
            ("katte", "買って", 6700),
            ("katta", "買った", 6700),
            ("kawanai", "買わない", 6700),
            ("kaou", "買おう", 6700),
            ("kae", "買え", 4500),
            // 行く.
            ("ikimasu", "行きます", 6700),
            ("ikanai", "行かない", 6700),
            ("ikou", "行こう", 6700),
            ("ikeba", "行けば", 6700),
        ],
    );
}

#[test]
fn iku_takes_tte_never_ite() {
    let lexicon = fixture();
    // 行って and 言って, and いって from いる (class v?, read as 五段 ラ行 too).
    let itte = found(&lexicon, "itte", Japanese, FORMS);
    let texts_of = |list: &[Found]| list.iter().map(|found| found.text.clone()).collect::<Vec<_>>();
    assert_eq!(texts_of(&itte), ["行って", "いって", "言って", "胃って"]);
    assert_eq!(itte[0].row.word, 1);
    assert_eq!(itte[2].row.word, 2);
    // A one-kana noun before a function word costs more: 4500 - 500 - 800.
    assert_eq!(itte[3].row.weight, 3200);
    // The stored, reviewed 行った stands before the generated one, which is dropped.
    let itta = found(&lexicon, "itta", Japanese, FORMS);
    assert_eq!(texts_of(&itta), ["行った", "いった", "言った"]);
    assert_eq!(itta[0].row, JapaneseRow { word: 49, weight: 3000, kind: KIND_READING, flags: FLAG_REVIEWED | FLAG_FORM });
    // いいて would be 行く through いて: not a form of 行く.
    assert!(texts(&lexicon, "iite", Japanese, FORMS).is_empty());
}

#[test]
fn kuru_keeps_its_kanji() {
    let lexicon = fixture();
    check_firsts(
        &lexicon,
        &[
            ("kimasu", "来ます", 6700),
            ("kimashita", "来ました", 6700),
            ("kite", "来て", 6700),
            ("kita", "来た", 6700),
            ("konai", "来ない", 6700),
            ("konakatta", "来なかった", 6400),
            ("koyou", "来よう", 6700),
            ("kureba", "来れば", 6700),
            ("koi", "来い", 4500),
            ("korareru", "来られる", 6700),
            ("kosaseru", "来させる", 6700),
            ("kitai", "来たい", 6700),
        ],
    );
    // The form outranks a lighter word of the same reading.
    assert_eq!(texts(&lexicon, "konai", Japanese, FORMS), ["来ない", "庫内"]);
}

#[test]
fn suru_and_nouns_with_suru() {
    let lexicon = fixture();
    check_firsts(
        &lexicon,
        &[
            ("shimasu", "します", 8700),
            ("shimashita", "しました", 8700),
            ("shite", "して", 8700),
            ("shita", "した", 8700),
            ("shinai", "しない", 8700),
            ("shiyou", "しよう", 8700),
            ("sureba", "すれば", 8700),
            ("shiro", "しろ", 6500),
            ("sareru", "される", 8700),
            ("saseru", "させる", 8700),
            ("shiteiru", "している", 8700),
            // A noun with する: 1200 more.
            ("benkyoushimasu", "勉強します", 6500),
            ("benkyoushite", "勉強して", 6500),
            ("benkyousuru", "勉強する", 6800),
            ("benkyoushinai", "勉強しない", 6500),
            ("benkyousaseru", "勉強させる", 6500),
            ("benkyoudekiru", "勉強できる", 6200),
            ("benkyoudekimasu", "勉強できます", 5900),
            ("onegaishimasu", "お願いします", 6500),
            // Verbs of class vs.
            ("aishite", "愛して", 5400),
            ("aishimasu", "愛します", 5400),
            ("anjite", "案じて", 2300),
            ("anjinai", "案じない", 2300),
            ("anzureba", "案ずれば", 2300),
            ("anjiyou", "案じよう", 2300),
        ],
    );
    // A form of する stands before a lighter word of the same reading.
    assert_eq!(texts(&lexicon, "shita", Japanese, FORMS), ["した", "下"]);
    // A noun of one or two kana with する costs 600 more: 8000 - 300 - 1200 - 600.
    let hanashite = found(&lexicon, "hanashite", Japanese, FORMS);
    assert_eq!((hanashite[0].text.as_str(), hanashite[1].text.as_str(), hanashite[1].row.weight), ("話して", "花して", 5900));
    // できる alone is its own word; する is not looked up for it.
    assert_eq!(texts(&lexicon, "dekiru", Japanese, FORMS), ["出来る"]);
}

#[test]
fn verbs_of_unknown_class() {
    let lexicon = fixture();
    // 言い出す (v?) inflects as 五段 サ行; いる (v?) as 一段 (いて) and as 五段 ラ行 (いって).
    check_firsts(&lexicon, &[("iidashite", "言い出して", 3700), ("iidasanai", "言い出さない", 3700), ("ite", "いて", 6700)]);
    assert_eq!(texts(&lexicon, "imasu", Japanese, FORMS), ["います"]);
}

#[test]
fn adjectives() {
    let lexicon = fixture();
    check_firsts(
        &lexicon,
        &[
            ("ookiku", "大きく", 6700),
            ("ookikute", "大きくて", 6700),
            ("ookikatta", "大きかった", 6700),
            ("ookikunai", "大きくない", 6700),
            ("ookikunakatta", "大きくなかった", 6400),
            ("ookikereba", "大きければ", 6700),
            ("ookisa", "大きさ", 6700),
            ("ookisou", "大きそう", 6700),
            ("ookisugiru", "大きすぎる", 6700),
            ("ookikattara", "大きかったら", 6700),
            ("atatakakatta", "温かかった", 3700),
            ("yokatta", "良かった", 6900),
            ("yokunai", "良くない", 6900),
        ],
    );
    // いい inflects through よい only; as it is, it takes a copula.
    assert!(texts(&lexicon, "ikatta", Japanese, FORMS).is_empty());
    assert!(texts(&lexicon, "ikunai", Japanese, FORMS).is_empty());
    assert_eq!(texts(&lexicon, "iidesu", Japanese, FORMS), ["いいです", "良いです"]);
}

#[test]
fn copula_and_particles() {
    let lexicon = fixture();
    check_firsts(
        &lexicon,
        &[
            // Na-adjective and noun with a copula form, な and に.
            ("kireida", "綺麗だ", 7500),
            ("kireidesu", "綺麗です", 7500),
            ("kireideshita", "綺麗でした", 7500),
            ("kireidatta", "綺麗だった", 7500),
            ("kireijanai", "綺麗じゃない", 7500),
            ("kireidehanai", "綺麗ではない", 7500),
            ("kireijanakatta", "綺麗じゃなかった", 7500),
            ("kireidehaarimasen", "綺麗ではありません", 7500),
            ("kireideshou", "綺麗でしょう", 7500),
            ("kireidarou", "綺麗だろう", 7500),
            ("kireinara", "綺麗なら", 7500),
            ("kireina", "綺麗な", 7500),
            ("kireini", "綺麗に", 7500),
            ("kireinandesu", "綺麗なんです", 7500),
            ("shizukananode", "静かなので", 4500),
            // な after a word of class n costs 300 more.
            ("genkina", "元気な", 6700),
            // Particles after a noun.
            ("watashiha", "私は", 7500),
            ("watashiga", "私が", 7500),
            ("watashiwo", "私を", 7500),
            ("watashini", "私に", 7500),
            ("watashide", "私で", 7500),
            ("watashito", "私と", 7500),
            ("watashimo", "私も", 7500),
            ("watashino", "私の", 7500),
            ("watashihe", "私へ", 7500),
            ("watashiya", "私や", 7500),
            ("watashika", "私か", 7500),
            ("watashine", "私ね", 7500),
            ("watashiyo", "私よ", 7500),
            ("watashikara", "私から", 7500),
            ("watashimade", "私まで", 7500),
            ("watashiyori", "私より", 7500),
            ("watashidake", "私だけ", 7500),
            ("watashinado", "私など", 7500),
            ("watashitte", "私って", 7500),
            ("watashiyone", "私よね", 7500),
            ("watashikana", "私かな", 7500),
            // Two function words.
            ("watashiniha", "私には", 7000),
            ("watashikaraha", "私からは", 7000),
            ("watashinoha", "私のは", 7000),
            ("watashidakeha", "私だけは", 7000),
            ("tenkidesune", "天気ですね", 7000),
            ("gakkoudesuka", "学校ですか", 8000),
            ("gakkoudattakedo", "学校だったけど", 8000),
            ("matane", "またね", 6500),
            ("kyoumo", "今日も", 8500),
            // After a predicate.
            ("taberukara", "食べるから", 6500),
            ("taberunode", "食べるので", 6500),
            ("taberunoni", "食べるのに", 6500),
            ("taberushi", "食べるし", 6500),
            ("taberukedo", "食べるけど", 6500),
            ("taberuna", "食べるな", 6500),
            ("taberundesu", "食べるんです", 6500),
            ("taberudeshou", "食べるでしょう", 6500),
            ("taberunara", "食べるなら", 6500),
            ("tabetane", "食べたね", 6200),
            ("tabemashitane", "食べましたね", 6200),
            ("tabemasuka", "食べますか", 6200),
            ("tabetekara", "食べてから", 6200),
            ("tabetemo", "食べても", 6200),
            ("tabetene", "食べてね", 6200),
            ("tabenaidesu", "食べないです", 6200),
            ("ookiidesu", "大きいです", 6500),
            ("ookiine", "大きいね", 6500),
            ("ookikattadesu", "大きかったです", 6200),
        ],
    );
    // A one-kana noun before a function word costs 800 more.
    assert_eq!(first(&lexicon, "ini", Japanese, FORMS).row.weight, 3200);
    // A variant key is a base too.
    let row = first(&lexicon, "renaiha", Japanese, FORMS).row;
    assert_eq!((row.word, row.kind, row.weight), (53, KIND_VARIANT, 2800));
}

#[test]
fn duplicates_and_order() {
    let lexicon = fixture();
    // The stored 食べた (6000) gives way to the generated one (7000 - 300).
    let tabeta = found(&lexicon, "tabeta", Japanese, FORMS);
    assert_eq!(tabeta.len(), 1);
    assert_eq!(tabeta[0].row, JapaneseRow { word: 0, weight: 6700, kind: KIND_READING, flags: FLAG_FORM | FLAG_PINYIN_OK });
    // Forms of a reviewed word are not reviewed rows.
    let kirei = first(&lexicon, "kireidesu", Japanese, FORMS);
    assert_eq!(kirei.row.flags, FLAG_FORM);
    // A word of the list stays ahead of a heavier noun with a particle that reads the same
    // (うんと 2300, 運 4800): the particle form pays 500 and, beside a word, 2500 more.
    let unto = found(&lexicon, "unto", Japanese, FORMS);
    assert_eq!(unto.iter().map(|hit| hit.text.as_str()).collect::<Vec<_>>(), ["うんと", "運と"]);
    assert_eq!(unto[1].row.weight, 4800 - 500 - 2500);
    // With no word for the whole input the particle form pays its 500 only.
    assert_eq!(first(&lexicon, "unha", Japanese, FORMS).row.weight, 4800 - 500);
    // Without FORMS, the exact lookup only.
    assert_eq!(texts(&lexicon, "tabeta", Japanese, OPTION_COMPLETIONS), ["食べた"]);
    // The limit cuts the merged list.
    let mut query = JapaneseQuery::new();
    assert_eq!(query.lookup_extended(&lexicon, "itte", Japanese, 2, FORMS), 2);
    assert_eq!(query.lookup_extended(&lexicon, "itte", Japanese, 0, FORMS), 0);
    assert_eq!(query.kana(), "いって");
}

#[test]
fn pinyin_mode_stays_quiet() {
    let lexicon = fixture();
    // Hepburn input, a base with PINYIN_OK: inflected.
    assert_eq!(texts(&lexicon, "tabeta", Pinyin, ALL), ["食べた"]);
    assert_eq!(first(&lexicon, "tabeta", Pinyin, ALL).row.flags, FLAG_FORM | FLAG_PINYIN_OK);
    assert_eq!(texts(&lexicon, "ookikatta", Pinyin, ALL), ["大きかった"]);
    assert_eq!(texts(&lexicon, "shinai", Pinyin, ALL), ["しない"]);
    // A reviewed base counts as well: its permission sits on its own pinyin key, not on the kana posting.
    assert_eq!(texts(&lexicon, "asonda", Pinyin, ALL), ["遊んだ"]);
    assert_eq!(first(&lexicon, "asonda", Pinyin, ALL).row.flags, FLAG_FORM);
    // Kunrei spelling: no kana key in pinyin mode.
    assert!(texts(&lexicon, "sinai", Pinyin, ALL).is_empty());
    assert_eq!(texts(&lexicon, "sinai", Japanese, FORMS), ["しない"]);
    // No base without PINYIN_OK, no imperative or potential, no particle, copula or noun + する.
    for input in ["hanashita", "kake", "kakeru", "gakkouni", "watashiha", "kireidesu", "benkyoushimasu", "tabeteiruyo"] {
        assert!(texts(&lexicon, input, Pinyin, ALL).is_empty(), "{input}");
        assert!(!texts(&lexicon, input, Japanese, FORMS).is_empty(), "{input}");
    }
    // Completions and segments are Japanese mode only.
    assert!(texts(&lexicon, "arig", Pinyin, ALL).is_empty());
    assert!(texts(&lexicon, "kyouhaiitenkidesune", Pinyin, ALL).is_empty());
}

#[test]
fn completions() {
    let lexicon = fixture();
    let check = |input: &str, expected: &[(&str, i32)]| {
        let list = found(&lexicon, input, Japanese, OPTION_COMPLETIONS);
        let got: Vec<(&str, i32)> = list.iter().map(|found| (found.text.as_str(), found.row.weight)).collect();
        assert_eq!(got, expected, "{input}");
        for found in &list {
            if found.row.flags & FLAG_COMPLETION != 0 {
                assert_eq!(found.consumed, input.len());
                assert_eq!(found.row.flags & !(FLAG_COMPLETION | FLAG_PINYIN_OK), 0);
            }
        }
    };
    // Pending g: が, ぎ, ぐ … and っ. ありがち is too light (2000).
    check("arig", &[("ありがとう", 8700), ("有難う", 5000), ("ありがたい", 4900)]);
    check("ohay", &[("おはよう", 7850)]);
    check("kyo", &[("今日", 8850), ("京都", 8400), ("教室", 4550), ("協力", 2900), ("教", 2850)]);
    // Pending k as っ; pending ch as ち (ちゃ, ちょ … start with it).
    check("gak", &[("学校", 8700)]);
    check("kach", &[("価値", 6000)]);
    // A final n is pending: こん, こな … こ; the key こん itself would be the exact lookup.
    check("kon", &[("こんにちは", 7550), ("この", 7000), ("庫内", 2850)]);
    // Through a variant key.
    check("rena", &[("恋愛", 3150)]);
    // Merged behind the first word: word weight + 1500 against the completion score.
    check("kyou", &[("今日", 9000), ("京都", 8550), ("教室", 4700), ("教", 3000), ("協力", 3050)]);
    let kyou = found(&lexicon, "kyou", Japanese, OPTION_COMPLETIONS);
    assert_eq!(kyou.iter().map(|found| found.row.flags & FLAG_COMPLETION != 0).collect::<Vec<_>>(), [false, true, true, false, true]);
    // The key of the input is not its own completion.
    check("arigatou", &[("ありがとう", 9000), ("有難う", 5300)]);
    // Too little typed, or a tail that no syllable starts with.
    for input in ["sh", "a", "k", "kaxq", "kax-", "arigq"] {
        check(input, &[]);
    }
    check("ka", &[("蚊", 3000)]);
    // A completion with the surface of a form is dropped.
    let konai = texts(&lexicon, "konai", Japanese, ALL);
    assert_eq!(konai.iter().filter(|text| text.as_str() == "来ない").count(), 1);
}

#[test]
fn completion_limits() {
    // 40 keys under あい: the best 32 are kept, best first.
    let mut image = Image::default();
    for index in 0..40i16 {
        let word = image.word(&format!("語{index}"), 0);
        let key = format!("あい{}", ["か", "き", "く", "け", "こ", "さ", "し", "す", "せ", "そ"][index as usize % 10]);
        let key = format!("{key}{}", ["た", "ち", "つ", "て"][index as usize / 10]);
        image.posting(&key, word, 3000 + index * 10, KIND_READING, 0);
    }
    let lexicon = open(image.write());
    let list = found(&lexicon, "ai", Japanese, OPTION_COMPLETIONS);
    assert_eq!(list.len(), 32);
    assert_eq!(list[0].text, "語39");
    assert_eq!(list[31].text, "語8");
    assert!(list.windows(2).all(|pair| pair[0].row.weight >= pair[1].row.weight));

    // More keys than one lookup reads: the scan stops, so a word behind them is not offered.
    let mut image = Image::default();
    let kana = ["か", "き", "く", "け", "こ", "さ", "し", "す", "せ", "そ", "た", "ち", "つ", "て", "と", "な", "に", "ぬ", "ね", "の", "は"];
    let mut keys = Vec::new();
    for a in kana {
        for b in kana {
            for c in kana {
                keys.push(format!("かか{a}{b}{c}"));
            }
        }
    }
    keys.sort();
    keys.truncate(8300);
    for (index, key) in keys.iter().enumerate() {
        let word = image.word(&format!("語{index}"), 0);
        let weight = if index == keys.len() - 1 { 9000 } else { 3000 };
        image.posting(key, word, weight, KIND_READING, 0);
    }
    let lexicon = open(image.write());
    let list = texts(&lexicon, "kaka", Japanese, OPTION_COMPLETIONS);
    assert_eq!(list.len(), 32);
    assert!(!list.contains(&"語8299".to_string()));
}

#[test]
fn segments() {
    let lexicon = fixture();
    let summary = |input: &str, options: u32| -> Vec<(String, usize)> {
        found(&lexicon, input, Japanese, options).into_iter().map(|found| (found.text, found.consumed)).collect()
    };
    let pairs = |list: &[(&str, usize)]| -> Vec<(String, usize)> {
        list.iter().map(|&(text, consumed)| (text.to_string(), consumed)).collect()
    };
    // The longest leading part with words, then up to two shorter ones (three rows each).
    assert_eq!(
        summary("kyouhaiitenkidesune", ALL),
        pairs(&[("今日は", 6), ("教は", 6), ("今日", 4), ("教", 4)])
    );
    // A leading part is at least two kana: 胃 for the first い is not offered.
    assert_eq!(summary("iitenkidesune", ALL), pairs(&[("いい", 2), ("良い", 2)]));
    assert_eq!(summary("tenkidesune", ALL), pairs(&[("天気ですね", 11)]));
    // The cut never splits a kana: gakkou|niiku; kan|u is not かん and う, and か alone is one kana.
    assert_eq!(summary("gakkouniiku", ALL), pairs(&[("学校に", 8), ("学校", 6)]));
    assert!(summary("kanu", ALL).is_empty());
    assert!(summary("koko", ALL).is_empty());
    // A part ending in ' is the same kana as the one without it.
    assert_eq!(summary("kyou'kyou", ALL), pairs(&[("今日", 4), ("教", 4)]));
    // A word that continues the whole input comes before the words of its start.
    let ahead = found(&lexicon, "kyouryo", Japanese, ALL);
    assert_eq!(ahead.iter().map(|found| (found.text.as_str(), found.consumed)).collect::<Vec<_>>(), [("協力", 7), ("今日", 4), ("教", 4)]);
    assert_ne!(ahead[0].row.flags & FLAG_COMPLETION, 0);
    assert_eq!(ahead[1].row.flags & FLAG_COMPLETION, 0);
    let konnichi = found(&lexicon, "konnichi", Japanese, ALL);
    assert_eq!(konnichi.iter().map(|found| (found.text.as_str(), found.consumed)).collect::<Vec<_>>(), [("こんにちは", 8)]);
    // Without FORMS, the parts are looked up exactly.
    assert_eq!(summary("kyouhaiitenkidesune", OPTION_SEGMENTS), pairs(&[("今日", 4), ("教", 4)]));
    // Not when the whole input has a word or a form.
    assert_eq!(summary("tabemashita", ALL), pairs(&[("食べました", 11)]));
    // The input may end in half a syllable: the letters stay with the rest, and the candidates
    // do not come and go at every consonant of a sentence.
    assert_eq!(summary("kyouhag", ALL), pairs(&[("今日は", 6), ("教は", 6), ("今日", 4), ("教", 4)]));
    assert_eq!(summary("kyouhaiit", ALL), summary("kyouhaiitenkidesune", ALL));
    assert_eq!(summary("tabemashitak", ALL), pairs(&[("食べました", 11)]));
    assert_eq!(summary("gakkounikk", ALL), pairs(&[("学校に", 8), ("学校", 6)]));
    // With a completion for the half syllable, that comes first.
    assert_eq!(summary("kyoush", ALL), pairs(&[("教室", 6), ("今日", 4), ("教", 4)]));
    // Many leading parts without words: the search stops, nothing is found.
    assert!(summary(&"a".repeat(80), ALL).is_empty());
    assert!(summary(&"zo".repeat(40), ALL).is_empty());
}

/// Probes for damaged images and odd input: none of this may panic.
fn exercise(lexicon: &JapaneseLexicon) {
    let mut query = JapaneseQuery::new();
    let inputs = [
        "tabemashita", "itta", "konai", "benkyoushimasu", "tenkidesune", "kyouhaiitenkidesune", "arig", "kon",
        "kyo", "ookikunakatta", "kireidesu", "sushi", "a", "zzz",
    ];
    for input in inputs {
        for mode in [Japanese, Pinyin] {
            let count = query.lookup_extended(lexicon, input, mode, 64, ALL);
            assert!(count <= 64);
            for index in 0..count {
                assert!(query.row(index).is_some());
                assert!(query.consumed(index) <= input.len());
                let _ = query.text(index);
            }
        }
    }
}

#[test]
fn odd_input_never_panics() {
    let lexicon = fixture();
    let mut query = JapaneseQuery::new();
    let long = "kyouhaiitenkidesunewatashihagakkouniikimasutabesaseraremashitakedoyokattadesunen";
    assert_eq!(long.len(), 80);
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut inputs: Vec<String> = [
        "", "'", "-", "''''", "---", "n", "nn", "nnn", "n'", "xtsu", "ltsu", "q", "qqqq", "x", "xx", "y", "ww", "ch", "tch",
        "a-", "-a", "ka'", "'ka", "zzzzzzzzzzzz", "Tabemashita", "TABEMASHITA", "日本", "tabe\n", "tabe ta", long,
    ]
    .iter()
    .map(|text| text.to_string())
    .collect();
    inputs.push("a".repeat(80));
    inputs.push("n".repeat(80));
    inputs.push("tsu".repeat(26));
    inputs.push("de".repeat(40));
    inputs.push("ka".repeat(41));
    let letters = b"abcdefghijklmnopqrstuvwxyz'-";
    for _ in 0..300 {
        let len = 1 + (next() % 80) as usize;
        inputs.push((0..len).map(|_| char::from(letters[(next() % letters.len() as u64) as usize])).collect());
    }
    for input in &inputs {
        for mode in [Japanese, Pinyin] {
            let count = query.lookup_extended(&lexicon, input, mode, 64, ALL);
            for index in 0..count {
                assert!(query.consumed(index) >= 1 && query.consumed(index) <= input.len(), "{input}");
                assert!(!query.text(index).is_empty(), "{input}");
            }
            if input.len() > 80 {
                assert_eq!(count, 0);
            }
        }
    }
    assert_eq!(query.lookup_extended(&lexicon, &"a".repeat(81), Japanese, 64, ALL), 0);
    assert_eq!(query.kana(), "");
}

#[test]
fn damaged_images_never_panic() {
    let good = source("mimo_jp_forms_fixture.tsv").write();
    let put_u32 = |bytes: &mut [u8], at: usize, value: u32| bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    // Cut, with file_len set to the new length.
    for len in (64..good.len()).step_by(7) {
        let mut bytes = good[..len].to_vec();
        put_u32(&mut bytes, 56, len as u32);
        if let Some(lexicon) = JapaneseLexicon::from_vec(bytes) {
            exercise(&lexicon);
        }
    }
    // Bytes of the tables changed.
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for round in 0..1500 {
        let mut bytes = good.clone();
        for _ in 0..(1 + next() % 6) {
            let at = 16 + (next() as usize) % (bytes.len() - 16);
            bytes[at] = if round % 3 == 0 { 0xFF } else { next() as u8 };
        }
        if let Some(lexicon) = JapaneseLexicon::from_vec(bytes) {
            exercise(&lexicon);
        }
    }
}

#[test]
fn many_forms_keep_the_heaviest() {
    // たべには: 64 nouns read たべ (+ には, 1000 less) come first and fill the 64 places; of the
    // nouns read たべに (+ は, 500 less), the heavier ones push out the lightest, the lighter
    // ones are dropped.
    let mut image = Image::default();
    for index in 0..64i16 {
        let word = image.word(&format!("語{index}"), 0);
        image.posting("たべ", word, 3000 + index, KIND_READING, 0);
    }
    for index in 0..6i16 {
        let word = image.word(&format!("重{index}"), 0);
        image.posting("たべに", word, 4000 + index, KIND_READING, 0);
        let word = image.word(&format!("軽{index}"), 0);
        image.posting("たべに", word, 2000 + index, KIND_READING, 0);
    }
    let lexicon = open(image.write());
    let list = found(&lexicon, "tabeniha", Japanese, FORMS);
    assert_eq!(list.len(), 64);
    assert_eq!(list.iter().filter(|found| found.text.starts_with('重')).count(), 6);
    assert!(list.iter().all(|found| !found.text.starts_with('軽')));
    assert_eq!(list.iter().map(|found| found.row.weight).min(), Some(3006 - 1000));
    assert_eq!((list[0].text.as_str(), list[0].row.weight), ("重5は", 3505));
}

/// The acceptance list on a real image. `MIMO_JP_BIN` names it.
#[test]
#[ignore = "needs MIMO_JP_BIN"]
fn shipped_image_acceptance() {
    let path = std::env::var_os("MIMO_JP_BIN").expect("MIMO_JP_BIN");
    let lexicon = open(std::fs::read(path).expect("read MIMO_JP_BIN"));
    let mut failures = Vec::new();
    let mut check = |input: &str, wanted: &[&str], within: usize, all_of: bool| {
        let list = found(&lexicon, input, Japanese, ALL);
        let top: Vec<&str> = list.iter().take(within).map(|found| found.text.as_str()).collect();
        let ok = if all_of { wanted.iter().all(|w| top.contains(w)) } else { wanted.iter().any(|w| top.contains(w)) };
        let shown: Vec<String> =
            list.iter().take(6).map(|found| format!("{}[{} {}]", found.text, found.row.weight, found.consumed)).collect();
        println!("{} {input:22} {}", if ok { "pass" } else { "FAIL" }, shown.join("  "));
        if !ok {
            failures.push(input.to_string());
        }
    };
    for (input, wanted) in [
        ("tabemashita", &["食べました"][..]),
        ("ikimasu", &["行きます"]),
        ("nonde", &["飲んで"]),
        ("kaita", &["書いた"]),
        ("oyoide", &["泳いで"]),
        ("hanashite", &["話して"]),
        ("matte", &["待って"]),
        ("shinda", &["死んだ"]),
        ("asonde", &["遊んで"]),
        ("yonda", &["読んだ", "呼んだ"]),
        ("katta", &["買った", "勝った"]),
        ("konai", &["来ない"]),
        ("shimasu", &["します"]),
        ("shite", &["して"]),
        ("benkyoushimasu", &["勉強します"]),
        ("ookikatta", &["大きかった"]),
        ("oishikunai", &["美味しくない", "おいしくない"]),
        ("kireidesu", &["きれいです", "綺麗です"]),
        ("watashiha", &["私は"]),
        ("gakkouni", &["学校に"]),
        ("tenkidesune", &["天気ですね"]),
        ("onegaishimasu", &["お願いします"]),
        ("taberareru", &["食べられる"]),
        ("tabesaseraremashita", &["食べさせられました"]),
        ("mitai", &["見たい"]),
        ("yomitakunai", &["読みたくない"]),
    ] {
        check(input, wanted, 3, false);
    }
    check("itta", &["行った", "言った"], 4, true);
    check("arig", &["ありがとう"], 1, false);
    check("ohay", &["おはよう"], 3, false);
    check("kyo", &["今日"], 5, false);
    let first_of = |input: &str| {
        let list = found(&lexicon, input, Japanese, ALL);
        list.first().map(|found| (found.text.clone(), found.consumed))
    };
    let segments = [
        ("kyouhaiitenkidesune", Some(("今日は".to_string(), 6))),
        ("iitenkidesune", first_of("iitenkidesune").filter(|(_, consumed)| *consumed == 2)),
        ("tenkidesune", Some(("天気ですね".to_string(), 11))),
    ];
    for (input, wanted) in segments {
        let got = first_of(input);
        let ok = wanted.is_some() && got == wanted;
        println!("{} {input:22} first {got:?}", if ok { "pass" } else { "FAIL" });
        if !ok {
            failures.push(input.to_string());
        }
    }
    assert!(failures.is_empty(), "failed: {failures:?}");
}
