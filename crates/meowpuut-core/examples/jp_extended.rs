//! `lookup_extended` on a real image: top results for a phrase list, the regression check
//! against `lookup`, and timing.
//!
//! ```text
//! MIMO_JP_BIN=path/to/mimo_jp.bin cargo run -p meowpuut-core --release --example jp_extended [-- phrases|regression|bench|<romaji>...]
//! ```

use meowpuut_core::japanese::{
    JapaneseLexicon, JapaneseMode, JapaneseQuery, FLAG_COMPLETION, FLAG_FORM, OPTION_COMPLETIONS, OPTION_FORMS,
    OPTION_SEGMENTS,
};
use meowpuut_core::romaji::{normalise_key, to_kana};

const ALL: u32 = OPTION_FORMS | OPTION_COMPLETIONS | OPTION_SEGMENTS;

const PHRASES: &[&str] = &[
    "tabemashita", "ikimasu", "itta", "nonde", "kaita", "oyoide", "hanashite", "matte", "shinda", "asonde",
    "yonda", "katta", "konai", "shimasu", "shite", "benkyoushimasu", "ookikatta", "oishikunai", "kireidesu",
    "watashiha", "gakkouni", "tenkidesune", "onegaishimasu", "taberareru", "tabesaseraremashita", "mitai",
    "yomitakunai", "arig", "ohay", "kyo", "kyouhaiitenkidesune", "iitenkidesune", "tenkidesune",
];

fn show(query: &mut JapaneseQuery, lexicon: &JapaneseLexicon, input: &str, mode: JapaneseMode) {
    let count = query.lookup_extended(lexicon, input, mode, 6, ALL);
    let mut line = format!("{input:24} {:?}", mode);
    for index in 0..count {
        let row = query.row(index).expect("row");
        let mark = if row.flags & FLAG_FORM != 0 {
            "f"
        } else if row.flags & FLAG_COMPLETION != 0 {
            "c"
        } else {
            "w"
        };
        line.push_str(&format!("  {}[{mark}{} {}]", query.text(index), row.weight, query.consumed(index)));
    }
    println!("{line}");
}

/// Hepburn romaji of a hiragana key, as `to_kana` reads it back; `None` for other characters.
fn romaji_of(key: &str) -> Option<String> {
    const BASE: &[(&str, &str)] = &[
        ("きゃ", "kya"), ("きゅ", "kyu"), ("きょ", "kyo"), ("しゃ", "sha"), ("しゅ", "shu"), ("しょ", "sho"),
        ("ちゃ", "cha"), ("ちゅ", "chu"), ("ちょ", "cho"), ("にゃ", "nya"), ("にゅ", "nyu"), ("にょ", "nyo"),
        ("ひゃ", "hya"), ("ひゅ", "hyu"), ("ひょ", "hyo"), ("みゃ", "mya"), ("みゅ", "myu"), ("みょ", "myo"),
        ("りゃ", "rya"), ("りゅ", "ryu"), ("りょ", "ryo"), ("ぎゃ", "gya"), ("ぎゅ", "gyu"), ("ぎょ", "gyo"),
        ("じゃ", "ja"), ("じゅ", "ju"), ("じょ", "jo"), ("びゃ", "bya"), ("びゅ", "byu"), ("びょ", "byo"),
        ("ぴゃ", "pya"), ("ぴゅ", "pyu"), ("ぴょ", "pyo"), ("しぇ", "she"), ("ちぇ", "che"), ("じぇ", "je"),
        ("ふぁ", "fa"), ("ふぃ", "fi"), ("ふぇ", "fe"), ("ふぉ", "fo"), ("てぃ", "thi"), ("でぃ", "dhi"),
        ("あ", "a"), ("い", "i"), ("う", "u"), ("え", "e"), ("お", "o"), ("か", "ka"), ("き", "ki"), ("く", "ku"),
        ("け", "ke"), ("こ", "ko"), ("さ", "sa"), ("し", "shi"), ("す", "su"), ("せ", "se"), ("そ", "so"),
        ("た", "ta"), ("ち", "chi"), ("つ", "tsu"), ("て", "te"), ("と", "to"), ("な", "na"), ("に", "ni"),
        ("ぬ", "nu"), ("ね", "ne"), ("の", "no"), ("は", "ha"), ("ひ", "hi"), ("ふ", "fu"), ("へ", "he"),
        ("ほ", "ho"), ("ま", "ma"), ("み", "mi"), ("む", "mu"), ("め", "me"), ("も", "mo"), ("や", "ya"),
        ("ゆ", "yu"), ("よ", "yo"), ("ら", "ra"), ("り", "ri"), ("る", "ru"), ("れ", "re"), ("ろ", "ro"),
        ("わ", "wa"), ("を", "wo"), ("が", "ga"), ("ぎ", "gi"), ("ぐ", "gu"), ("げ", "ge"), ("ご", "go"),
        ("ざ", "za"), ("じ", "ji"), ("ず", "zu"), ("ぜ", "ze"), ("ぞ", "zo"), ("だ", "da"), ("ぢ", "di"),
        ("づ", "du"), ("で", "de"), ("ど", "do"), ("ば", "ba"), ("び", "bi"), ("ぶ", "bu"), ("べ", "be"),
        ("ぼ", "bo"), ("ぱ", "pa"), ("ぴ", "pi"), ("ぷ", "pu"), ("ぺ", "pe"), ("ぽ", "po"), ("ゔ", "vu"),
        ("ぁ", "xa"), ("ぃ", "xi"), ("ぅ", "xu"), ("ぇ", "xe"), ("ぉ", "xo"), ("ゃ", "xya"), ("ゅ", "xyu"),
        ("ょ", "xyo"),
    ];
    let mut out = String::new();
    let mut rest = key;
    let mut double = false;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('っ') {
            if double {
                out.push_str("xtsu");
            }
            double = true;
            rest = after;
            continue;
        }
        if let Some(after) = rest.strip_prefix('ん') {
            let next = BASE.iter().find(|(kana, _)| after.starts_with(kana)).map(|(_, romaji)| *romaji);
            match next {
                Some(romaji) if romaji.starts_with(['a', 'i', 'u', 'e', 'o', 'y', 'n']) => out.push_str("n'"),
                _ => out.push('n'),
            }
            rest = after;
            continue;
        }
        let (kana, romaji) = BASE.iter().find(|(kana, _)| rest.starts_with(kana))?;
        if double {
            if romaji.starts_with(['a', 'i', 'u', 'e', 'o', 'n']) {
                out.push_str("xtsu");
            } else {
                out.push_str(&romaji[..1]);
            }
            double = false;
        }
        out.push_str(romaji);
        rest = &rest[kana.len()..];
    }
    if double {
        out.push_str("xtsu");
    }
    Some(out)
}

fn regression(lexicon: &JapaneseLexicon, bytes: &[u8]) {
    // Kana keys from the key table (header field 32 is keys_offset, 16 the key count, 48 the blob).
    let field = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().expect("u32")) as usize;
    let (key_count, keys_at, blob_at) = (field(16), field(32), field(48));
    let key = |index: usize| {
        let record = keys_at + 12 * index;
        let offset = field(record);
        let len = u16::from_le_bytes(bytes[record + 4..record + 6].try_into().expect("u16")) as usize;
        std::str::from_utf8(&bytes[blob_at + offset..blob_at + offset + len]).expect("utf-8")
    };
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut query = JapaneseQuery::new();
    let (mut tried, mut same) = (0, 0);
    let mut exceptions = Vec::new();
    while tried < 200 {
        let text = key((next() % key_count as u64) as usize);
        if text.as_bytes()[0] < 0x80 {
            continue;
        }
        let Some(romaji) = romaji_of(text) else { continue };
        let mut kana = String::new();
        let mut back = String::new();
        to_kana(&romaji, &mut kana);
        normalise_key(&kana, &mut back);
        if back != text || romaji.len() > 80 {
            continue;
        }
        tried += 1;
        query.lookup(lexicon, &romaji, JapaneseMode::Japanese, 64);
        let old = query.text(0).to_string();
        query.lookup_extended(lexicon, &romaji, JapaneseMode::Japanese, 64, ALL);
        let new = query.text(0).to_string();
        if old == new {
            same += 1;
        } else {
            let row = query.row(0).expect("row");
            let mut old_query = JapaneseQuery::new();
            old_query.lookup(lexicon, &romaji, JapaneseMode::Japanese, 1);
            let old_weight = old_query.row(0).map_or(0, |row| row.weight);
            exceptions.push(format!("{romaji} ({text}): {old} [{old_weight}] -> {new} [{} flags {}]", row.weight, row.flags));
        }
    }
    println!("regression: {same}/{tried} keep the first result");
    for line in exceptions {
        println!("  {line}");
    }
}

fn bench(lexicon: &JapaneseLexicon) {
    let typical = [
        "kyo", "arig", "ohay", "neko", "taberu", "tabeta", "ikimasu", "watashiha", "tenkidesune", "gakkouni",
        "konnichiha", "onegaishimasu", "kireidesu", "shimasu", "itta", "kaita", "sushi", "benkyou", "tabemashita",
        "yomitakunai",
    ];
    let long = [
        "kyouhaiitenkidesunewatashihagakkouniikimasu", // 43 letters
        "watashihaashitatomodachitoeigawominiikimasu", // 43 letters
        "kinouhaamegafuttanodeieniimashitayo",
        "tabesaseraremashitakedoyokattadesu",
    ];
    let mut query = JapaneseQuery::new();
    for (name, list) in [("typical 3-13 letters", &typical[..]), ("long 34-43 letters", &long[..])] {
        for (label, options) in [("lookup", 0), ("all options", ALL)] {
            let mut best = f64::MAX;
            for _ in 0..20 {
                let start = std::time::Instant::now();
                for input in list {
                    std::hint::black_box(query.lookup_extended(lexicon, std::hint::black_box(input), JapaneseMode::Japanese, 64, options));
                }
                best = best.min(start.elapsed().as_secs_f64() * 1e6 / list.len() as f64);
            }
            println!("{name:22} {label:12}: {best:8.1} us per call");
        }
        for input in list {
            let mut best = f64::MAX;
            for _ in 0..20 {
                let start = std::time::Instant::now();
                std::hint::black_box(query.lookup_extended(lexicon, std::hint::black_box(input), JapaneseMode::Japanese, 64, ALL));
                best = best.min(start.elapsed().as_secs_f64() * 1e6);
            }
            println!("    {input:46} {best:8.1} us");
        }
    }
}

fn main() {
    let path = std::env::var_os("MIMO_JP_BIN").expect("set MIMO_JP_BIN to a mimo_jp.bin");
    let bytes = std::fs::read(&path).expect("read MIMO_JP_BIN");
    let lexicon = JapaneseLexicon::from_vec(bytes.clone()).expect("image opens");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut query = JapaneseQuery::new();
    match args.first().map(String::as_str) {
        None | Some("phrases") => {
            for phrase in PHRASES {
                show(&mut query, &lexicon, phrase, JapaneseMode::Japanese);
            }
        }
        Some("regression") => regression(&lexicon, &bytes),
        Some("bench") => bench(&lexicon),
        Some(_) => {
            for input in &args {
                show(&mut query, &lexicon, input, JapaneseMode::Japanese);
                show(&mut query, &lexicon, input, JapaneseMode::Pinyin);
            }
        }
    }
}
