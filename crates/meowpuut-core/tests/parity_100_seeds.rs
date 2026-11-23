#![forbid(unsafe_code)]
//! 100-seed deterministic simulation parity test & zero-heap-allocation verification
//! against shipped rankings and their TSV/binary representations. The
//! Zhuyin baseline remains the original C++ reference.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use meowpuut_core::{Lexicon, Session};

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e3779b97f4a7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
    z ^ (z >> 31)
}

const PINYIN_POOL: [&str; 50] = [
    "nihao", "woxihuanni", "jintiantianqihenhao", "xuexiaoshitang", "xiansheng",
    "pengyou", "shijieheping", "woxihuannidenekohenkeai", "nihaoshijie",
    "womenshihaopengyou", "zhuyinshurufa", "diannao", "chengxuyuan", "kaixin",
    "xiexieni", "mingtianjian", "bukeqi", "duibuqi", "meiguanxi", "zaoshanghao",
    "wanan", "woaini", "kafeiguan", "shuiguo", "watashixihuanneko", "sakurahenmei",
    "ni3hao3", "xian'an", "zhong1wen2", "ying1wen2", "ri4wen2", "fa3wen2",
    "nihaom", "woxihuanb", "womens", "jintianw", "pengyoum", "xianshengd",
    "nh", "wxhn", "jttqhh", "xxst", "xs", "py", "sjhp",
    "nihaonihao", "nihaoa", "shurufaceshi", "zheshiyigeceshi", "qingwen",
];

const ZHUYIN_POOL: [&str; 19] = [
    "su3cl3", "ji3vu3c0u3", "rupwu0wu0fu4cpcl3", "vm,6vul4", "v0g/ ",
    "q/7u.6", "g4r,4ck6qu/6", "su3cl3g4r,4", "ji3mp3g4cl3q/7u.6",
    "1j6e06", "vm,4vm,4su3", "au/6wu0ru04", "1j4d4f3", "ao6ej0uv7",
    "su3cl3a87", "2u04u/3", "t/6vu4m06", "d9vup ", "2u04sao3",
];

const CONTEXT_POOL: [&str; 10] = [
    "你", "我", "我們", "今天", "喜歡", "學校", "電影", "世界", "朋友", "",
];

const GOLDEN_100_SEEDS: [u64; 100] = [
    0xc6ff2e38855aa534, 0x48b0443aca651576, 0x447ad826f36c4946, 0x6fe167b280901788,
    0x9cecb054d4fbe665, 0xf3744c39412abb99, 0x2ebc2eef110b5592, 0x52e83c250848ffcb,
    0x006748762dfb3818, 0x57ef5d4a3b69d8d2, 0x00246d3c3359e779, 0xae49a729e352c0bb,
    0x5213f9e5663c2aa8, 0xd2f92266ddaf625e, 0xa1731bdaea10d2ff, 0x47777f0e8769d506,
    0x9eb6a071ef8fd173, 0xc973dc961b08a925, 0xcee01ef183498ef9, 0xf4f2c64d9e58e53b,
    0xaa71cea0433f10a2, 0xe5db2b008f00be9b, 0x90ea8200a0391fb5, 0xdba40eb6239c5b85,
    0xb66ecf3e31a5478b, 0x33b62fbbca318e32, 0x2ef283700758c8bb, 0x9aeda76a09c83488,
    0xed9f5e390959c458, 0x9b7d7f8db7585dcd, 0x65fe3e2f118689ea, 0x640f1eddc3435f0a,
    0x9bb2188a1e13fac2, 0xf57bba6a5f70b54b, 0x9650526f9f5d0701, 0x2a860f93237682c0,
    0x3f54cdb65cbdbb65, 0x6b321e18887b2e53, 0xbb9fe071e1daa66c, 0xdaec66b05b989ea2,
    0x95692cf0804be69d, 0xe0ce39824f97805f, 0x169a1cda85e007b1, 0xf7168688fc2773e6,
    0x672b74fc74783a07, 0xb66c2745ab58b318, 0xbe1fa129150dbd0c, 0x27b4d360d9a79773,
    0xcf944e33760f79dc, 0xb467b20e3ae52be1, 0xd49fec41feabd808, 0x8d6e5162c5b10cf0,
    0xb8c93f32da8ff536, 0xffe3367ff03f5eda, 0x1a605e08b201ff03, 0xc82f83fe5efba29d,
    0x9957c14ac1847dd3, 0x8022d8b00ff0f0ba, 0x04b54c10452830ce, 0xbf4adb0f1a09747c,
    0x0b2d1aaf1cc9f784, 0xe72934e610854911, 0xda3bdea3d7670f3b, 0xaea06868eb26d2b2,
    0x5ad11a52b122e18d, 0x28e7a13846b9707e, 0x35f09e30ea50d2c7, 0xb3398d6be75bdcb7,
    0xcb1dbae8217abfed, 0x3c14ad018a4bfcf8, 0x3231cc3975866e70, 0x2a967f0298d22a57,
    0x2e8592897532d75e, 0x91f9b700c020ab5e, 0xbd86395664c438c8, 0xf55cff502fac825b,
    0xe6a4b633e2cf7ff4, 0xe385d68bc92a66f1, 0xf0f224ac45eb792f, 0x29ed93a2a7c58d8b,
    0xa664f199db8320d2, 0xcf2d1e14ce1cb459, 0x92c5ab3af468059b, 0x6636f8be5755037e,
    0x1e84da918f1db5c5, 0x64878e96c03c0eb0, 0xb3ab335850c8a45f, 0xa59814be0944a442,
    0x1e731cadfadc4132, 0x60a1ea932ddfeeb9, 0x2bd3a30403b12c01, 0xb8f2e402e4221899,
    0xf75ca9e7728f9626, 0xbfdf8f64e69d4c31, 0x90ec74a6a970e7a9, 0x44ba042321036d88,
    0xa7a0bb17845bc870, 0xdddaeb3841a5ee08, 0x117492e89b004b80, 0x96e408e247c559ca,
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn mix_bytes(hash: &mut u64, data: &[u8]) {
    for &b in data {
        *hash ^= u64::from(b);
        *hash = hash.wrapping_mul(1099511628211);
    }
}

fn mix_str(hash: &mut u64, s: &str) {
    mix_bytes(hash, s.as_bytes());
    mix_bytes(hash, &[0xff]);
}

fn mix_u64(hash: &mut u64, v: u64) {
    mix_bytes(hash, &v.to_le_bytes());
}

fn run_seed(session: &mut Session, seed: usize, is_pinyin: bool) -> u64 {
    let mut rng = seed as u64;
    let mut hash = 14695981039346656037u64;

    for _step in 0..50 {
        let r = splitmix64(&mut rng);
        let op = r % 100;
        let mut handled = false;

        if op < 55 {
            let phrase = if is_pinyin {
                PINYIN_POOL[((r >> 8) as usize) % PINYIN_POOL.len()]
            } else {
                ZHUYIN_POOL[((r >> 8) as usize) % ZHUYIN_POOL.len()]
            };
            let take = 1 + (((r >> 16) as usize) % phrase.len());
            for &b in &phrase.as_bytes()[..take] {
                handled = session.key(i32::from(b));
            }
        } else if op < 65 {
            handled = session.key(0xff08);
        } else if op < 75 {
            let cnt = session.candidate_count();
            if cnt > 0 {
                let idx = ((r >> 8) as usize) % cnt;
                handled = session.choose(idx);
            }
        } else if op < 82 {
            handled = session.key(if (r & 1) != 0 { 0xff56 } else { 0xff55 });
        } else if op < 88 {
            session.set_simplified(((r >> 8) & 1) != 0);
            handled = true;
        } else if op < 94 {
            let ctx = CONTEXT_POOL[((r >> 8) as usize) % CONTEXT_POOL.len()];
            session.set_segment_context(ctx);
            handled = true;
        } else if op < 97 {
            handled = session.key(0xff0d);
        } else {
            handled = session.key(0xff1b);
        }

        mix_u64(&mut hash, u64::from(handled));
        mix_str(&mut hash, session.input());
        mix_str(&mut hash, session.preedit());
        mix_str(&mut hash, session.committed());
        mix_str(&mut hash, session.segment_context());
        mix_u64(&mut hash, session.page() as u64);

        let cnt = session.candidate_count();
        mix_u64(&mut hash, cnt as u64);
        for i in 0..cnt {
            mix_str(&mut hash, session.candidate(i));
            mix_u64(&mut hash, session.candidate_consumed(i) as u64);
            let info_opt = session.candidate_info(i);
            mix_u64(&mut hash, u64::from(info_opt.is_some()));
            let info = info_opt.unwrap();
            mix_u64(&mut hash, u64::from(info.kind));
            mix_u64(&mut hash, u64::from(info.score_kind));
            mix_u64(&mut hash, info.consumed_bytes);
            mix_u64(&mut hash, info.remaining_offset_bytes);
            let score_str = format!("{:.9}", info.score);
            mix_str(&mut hash, &score_str);
        }

        if is_pinyin {
            let q = PINYIN_POOL[((r >> 24) as usize) % PINYIN_POOL.len()];
            let simp = ((r >> 32) & 1) != 0;
            let mp = session.mixed_pinyin(q, simp);
            mix_str(&mut hash, mp);
            let (pref, pref_cons) = session.mixed_pinyin_prefix(q, simp);
            mix_str(&mut hash, pref);
            mix_u64(&mut hash, pref_cons as u64);
        }
        session.clear_commit();
    }
    hash
}

#[test]
fn parity_100_random_seeds_zero_diff_and_zero_hot_allocs() {
    let root = repo_root();
    let py_tsv = fs::read_to_string(root.join(".build/lexicon/native-pinyin.tsv"))
        .expect("read native-pinyin.tsv");
    let zy_tsv = fs::read_to_string(root.join(".build/lexicon/native-zhuyin.tsv"))
        .expect("read native-zhuyin.tsv");

    let py_lex = Arc::new(Lexicon::load(&py_tsv).expect("load pinyin lexicon"));
    let zy_lex = Arc::new(Lexicon::load(&zy_tsv).expect("load zhuyin lexicon"));
    let base_bytes = fs::read(root.join(".build/lexicon/mimo_base.bin")).expect("read mimo_base.bin");
    let py_base_lex = Arc::new(Lexicon::load_from_base(&base_bytes, true).expect("load pinyin from mimo_base.bin"));
    let zy_base_lex = Arc::new(Lexicon::load_from_base(&base_bytes, false).expect("load zhuyin from mimo_base.bin"));
    assert_eq!(py_base_lex.words.len(), py_lex.words.len());
    assert_eq!(py_base_lex.words.flags, py_lex.words.flags);
    assert_eq!(zy_base_lex.words.len(), zy_lex.words.len());
    assert_eq!(zy_base_lex.words.flags, zy_lex.words.flags);

    let mut py_reused = Session::new(Arc::clone(&py_lex));
    let mut py_bin_session = Session::new(Arc::clone(&py_base_lex));
    for sess in [&mut py_reused, &mut py_bin_session] {
        sess.add_japanese_word("watashi", "私", 5500);
        sess.add_japanese_word("neko", "猫", 5200);
        sess.add_japanese_word("kawaii", "可愛い", 5300);
        sess.add_japanese_word("arigatou", "ありがとう", 5600);
        sess.add_japanese_word("konnichiwa", "こんにちは", 5400);
        sess.add_japanese_word("sakura", "桜", 4800);
        sess.add_japanese_word("tokyo", "東京", 5100);
        sess.add_japanese_word("sushi", "寿司", 4900);
        sess.add_japanese_word("au", "会う", 4200);
        sess.add_japanese_word("ai", "愛", 3500);
    }

    let mut zy_reused = Session::new(Arc::clone(&zy_lex));
    let mut zy_bin_session = Session::new(Arc::clone(&zy_base_lex));

    // MEOWPUUT_PARITY_PRINT=1 prints the table for GOLDEN_100_SEEDS instead of
    // comparing with it (after an intended lexicon or ranking change).
    let print = std::env::var_os("MEOWPUUT_PARITY_PRINT").is_some();
    let mut printed = Vec::with_capacity(GOLDEN_100_SEEDS.len());
    for (seed, &expected_hash) in GOLDEN_100_SEEDS.iter().enumerate() {
        let is_pinyin = seed % 2 == 0;
        let (actual_hash, bin_hash) = if is_pinyin {
            py_reused.clear();
            py_reused.set_simplified(false);
            py_bin_session.clear();
            py_bin_session.set_simplified(false);
            (run_seed(&mut py_reused, seed, true), run_seed(&mut py_bin_session, seed, true))
        } else {
            zy_reused.clear();
            zy_reused.set_simplified(false);
            zy_bin_session.clear();
            zy_bin_session.set_simplified(false);
            (run_seed(&mut zy_reused, seed, false), run_seed(&mut zy_bin_session, seed, false))
        };

        assert_eq!(
            actual_hash, bin_hash,
            "Binary vs TSV mismatch at seed {seed}: tsv={actual_hash:016x}, bin={bin_hash:016x}"
        );
        if print {
            printed.push(actual_hash);
            continue;
        }
        assert_eq!(
            actual_hash, expected_hash,
            "Mismatch at seed {seed}: actual={actual_hash:016x}, expected={expected_hash:016x}"
        );
    }
    if print {
        println!("GOLDEN_100_SEEDS_BEGIN");
        for row in printed.chunks(4) {
            let cells: Vec<String> = row.iter().map(|h| format!("0x{h:016x}")).collect();
            println!("    {},", cells.join(", "));
        }
        println!("GOLDEN_100_SEEDS_END");
    }

    // Verify zero heap allocations across hot simulation loops once warmed up:
    let py_cap_before = py_reused.capacity_signature();
    let zy_cap_before = zy_reused.capacity_signature();
    for seed in 0..20 {
        py_reused.clear();
        py_reused.set_simplified(false);
        let _ = run_seed(&mut py_reused, seed * 2, true);
        zy_reused.clear();
        zy_reused.set_simplified(false);
        let _ = run_seed(&mut zy_reused, seed * 2 + 1, false);
    }
    assert_eq!(
        py_reused.capacity_signature(),
        py_cap_before,
        "Pinyin hot simulation loop grew heap capacity"
    );
    assert_eq!(
        zy_reused.capacity_signature(),
        zy_cap_before,
        "Zhuyin hot simulation loop grew heap capacity"
    );
}
