//! English lexicon (mimo_en.bin): the Rust packer writes the bytes
//! scripts/mimo_en_binary.py writes, the reader answers the lookups the
//! Python reader answers, and the English keyboard rules hold.

use std::path::{Path, PathBuf};

use meowpuut_core::english::{pack, English, Kind, Suggestions, NO_SUGGEST, PACK_LIMIT, SHIFT};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn mini() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/english-mini")
}

/// The shipped image, when scripts/build-lexicon.sh (or mimo_en_binary.py) has made it.
fn shipped() -> Option<English> {
    let bytes = std::fs::read(root().join(".build/lexicon/mimo_en.bin")).ok()?;
    English::load(&bytes)
}

fn texts(out: &Suggestions) -> Vec<String> {
    (0..out.len()).map(|i| out.text(i).to_string()).collect()
}

#[test]
fn rust_packer_writes_the_python_bytes() {
    let dir = mini();
    let english = std::fs::read_to_string(dir.join("english.tsv")).unwrap();
    let pairs = std::fs::read_to_string(dir.join("pairs.tsv")).unwrap();
    let python = std::fs::read(dir.join("mimo_en.bin")).unwrap();
    let rust = pack(&english, &pairs, 300).unwrap();
    assert_eq!(rust.len(), python.len());
    assert!(rust == python, "first difference at byte {:?}", rust.iter().zip(&python).position(|(a, b)| a != b));
}

#[test]
fn rust_packer_writes_the_shipped_bytes() {
    // The full lists are .xz files; this test runs where xz and the built image exist.
    let built = root().join(".build/lexicon/mimo_en.bin");
    let Ok(python) = std::fs::read(&built) else {
        eprintln!("skipped: {} not built", built.display());
        return;
    };
    let unxz = |name: &str| {
        let out = std::process::Command::new("xz").arg("-dc").arg(root().join("DataSources/mimo-english").join(name)).output();
        out.ok().filter(|o| o.status.success()).map(|o| String::from_utf8(o.stdout).unwrap())
    };
    let (Some(english), Some(pairs)) = (unxz("english.tsv.xz"), unxz("pairs.tsv.xz")) else {
        eprintln!("skipped: xz not available");
        return;
    };
    let rust = pack(&english, &pairs, PACK_LIMIT).unwrap();
    assert_eq!(rust.len(), python.len());
    assert!(rust == python, "first difference at byte {:?}", rust.iter().zip(&python).position(|(a, b)| a != b));
}

#[test]
fn reader_answers_the_python_lookups() {
    let dir = mini();
    let english = English::load(&std::fs::read(dir.join("mimo_en.bin")).unwrap()).unwrap();
    let dump = std::fs::read_to_string(dir.join("dump.tsv")).unwrap();
    let mut checked = 0;
    for line in dump.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        let ids = |s: &str| -> Vec<usize> { s.split(',').filter(|x| !x.is_empty()).map(|x| x.parse().unwrap()).collect() };
        match f[0] {
            "n" => assert_eq!(english.len(), f[1].parse::<usize>().unwrap()),
            "row" => {
                let row: usize = f[1].parse().unwrap();
                assert_eq!(english.key(row), f[2].as_bytes());
                assert_eq!(english.spelling(row), f[3]);
                assert_eq!(english.weight(row), f[4].parse::<u16>().unwrap());
            }
            "next" => {
                let got: Vec<usize> = english.next_words(f[1].parse().unwrap()).collect();
                assert_eq!(got, ids(f[2]), "{line}");
            }
            "pair" => assert_eq!(english.pair(f[1].parse().unwrap(), f[2].parse().unwrap()), f[3].parse::<i32>().unwrap(), "{line}"),
            "t9" => {
                let (lo, hi) = english.nine_key_range(f[1].as_bytes());
                let got: Vec<usize> = (lo..hi).map(|p| english.nine_key_row(p)).take(20).collect();
                assert_eq!(got, ids(f[2]), "{line}");
            }
            _ => continue,
        }
        checked += 1;
    }
    assert!(checked > 1000, "{checked}");
}

#[test]
fn damaged_images_are_refused() {
    let image = std::fs::read(mini().join("mimo_en.bin")).unwrap();
    assert!(English::load(&image).is_some());
    assert!(English::load(&[]).is_none());
    for cut in [10, 100, 160, 1000, image.len() - 1] {
        assert!(English::load(&image[..cut]).is_none(), "{cut}");
    }
    let mut version = image.clone();
    version[12] = 2;
    assert!(English::load(&version).is_none());
    let mut offsets = image.clone();
    // The first key offset section starts right after the header; make it go backwards.
    offsets[160 + 4..160 + 8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(English::load(&offsets).is_none());
}

#[test]
fn keypad_spelling_casing_and_blocked_words() {
    let Some(english) = shipped() else { return eprintln!("skipped: mimo_en.bin not built") };
    let mut out = Suggestions::new();
    english.nine_key("65", "", 0, &mut out);
    assert_eq!(out.text(0), "OK");
    english.nine_key("474663", "", 0, &mut out);
    assert_eq!(out.text(0), "iPhone");
    english.nine_key("3668", "I", 0, &mut out);
    assert!(texts(&out).contains(&"don't".to_string()), "{:?}", texts(&out));
    english.nine_key("4663", "I have a", 0, &mut out);
    assert_eq!(out.kind(0), Some(Kind::Exact));
    assert!(texts(&out)[..4].iter().any(|t| t == "good"), "{:?}", texts(&out));
    // Keys that spell nothing: their first letters, no guess (spec 6.2 item 3).
    english.nine_key("7777777777", "I said", 0, &mut out);
    assert_eq!((out.len(), out.text(0), out.kind(0), out.row(0)), (1, "pppppppppp", Some(Kind::Letters), None));
    english.nine_key("99999999", "", 0, &mut out);
    assert_eq!(out.kind(0), Some(Kind::Letters));
    assert_eq!(out.text(0), "Wwwwwwww");

    // Sentence start capitalises; iPhone keeps its capitals; caps lock writes capitals.
    english.complete("iphon", "", 0, &mut out);
    assert_eq!(out.text(0), "iPhone");
    english.complete("Iphon", "", SHIFT, &mut out);
    assert_eq!(out.text(0), "iPhone");
    english.complete("Lond", "", 0, &mut out);
    assert_eq!(out.text(0), "London");
    english.complete("lond", "", 0, &mut out);
    assert_eq!(out.text(0), "London");
    english.complete("TH", "", 0, &mut out);
    assert_eq!(out.text(0), "THE");
    english.predict("", 0, &mut out);
    assert_eq!(out.text(0), "I");
    assert!(texts(&out).contains(&"The".to_string()), "{:?}", texts(&out));
    english.predict("Thank", 0, &mut out);
    assert_eq!(out.text(0), "you");

    english.correct("teh", "", 0, &mut out);
    assert_eq!(out.text(0), "the");
    english.correct("dont", "I", 0, &mut out);
    assert_eq!(out.text(0), "don't");
    english.correct("recieve", "", 0, &mut out);
    assert!(texts(&out).iter().take(3).any(|t| t == "receive"), "{:?}", texts(&out));
    english.correct("the", "", 0, &mut out);
    assert!(out.is_empty());

    // A blocked word: typed in full it stays, never offered otherwise.
    let blocked: Vec<usize> = (0..english.len()).filter(|&r| english.flags(r) & NO_SUGGEST != 0).collect();
    assert!(!blocked.is_empty());
    assert!(blocked.iter().all(|&r| english.is_blocked(&english.spelling(r).to_uppercase())));
    assert!(!english.is_blocked("the") && !english.is_blocked("qzxv") && !english.is_blocked(""));
    for &row in &blocked {
        let key = std::str::from_utf8(english.key(row)).unwrap().to_string();
        for cut in 1..key.len() {
            english.complete(&key[..cut], "", 0, &mut out);
            assert!((0..out.len()).all(|i| out.row(i) != Some(row)), "{key}");
        }
        english.correct(&format!("{key}x"), "", 0, &mut out);
        assert!((0..out.len()).all(|i| out.row(i) != Some(row)), "{key}");
        let digits: String = key.bytes().map(|b| char::from(meowpuut_core::english::digit(b))).collect();
        english.nine_key(&digits, "", 0, &mut out);
        // Spelled in full it is there, but never first (spec 6.2 item 4).
        assert!(out.len() >= 8 || (0..out.len()).any(|i| out.row(i).is_some_and(|r| english.key(r) == english.key(row))), "{key}");
        match out.row(0) {
            Some(first) => assert_eq!(english.flags(first) & NO_SUGGEST, 0, "{key}"),
            None => assert_eq!(out.kind(0), Some(Kind::Letters), "{key}"),
        }
    }
    for row in (0..english.len()).step_by(97) {
        english.predict(english.spelling(row), 0, &mut out);
        assert!((0..out.len()).all(|i| out.row(i).is_some_and(|r| english.flags(r) & NO_SUGGEST == 0)));
    }
}

#[test]
fn lookups_do_not_allocate() {
    let Some(english) = shipped() else { return eprintln!("skipped: mimo_en.bin not built") };
    let mut out = Suggestions::new();
    english.complete("warm", "", 0, &mut out);
    let before = out.capacity_signature();
    for word in ["t", "th", "the", "Wh", "inter", "recieve", "teh", "helllo", "zzzzq", "Supercalifragilisticexpialidocious"] {
        english.complete(word, "I think", 0, &mut out);
        english.correct(word, "I think", 0, &mut out);
        english.predict(word, 0, &mut out);
        english.nine_key("4663", word, 0, &mut out);
    }
    // The lookups keep their candidates on the stack; only these buffers hold text.
    assert_eq!(out.capacity_signature(), before);
    assert_eq!(english.heap_bytes(), english.heap_bytes());
}
