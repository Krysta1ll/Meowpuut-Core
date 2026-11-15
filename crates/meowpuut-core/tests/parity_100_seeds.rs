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
    0xc061344db935d70a, 0x1862cf597f9424ed, 0xe68af99d5f0632f3, 0xefd56d00edd2c843,
    0xc013dc417ead5064, 0x1f8549a89825ff0a, 0x985439c52b4f29dc, 0xacedca344a7f1e9b,
    0xc293306ad8fa5a63, 0x0b0834c7f7633406, 0xd24e4ba2dec2c2f2, 0x2593dc5925d76c0a,
    0xd94d0287cdc1d0c0, 0xff5ffb71adbc9e38, 0xc570b5d559ebfa99, 0x9833a6a65d36e884,
    0xa36f81882f25602f, 0xb759eedb8b114777, 0x434a9765e6e39cdc, 0x95da5504bdcd73f5,
    0xfa1165cb3651aaee, 0x6fab06a28543f54f, 0xdf2f17975e04cbad, 0x90af6b4e1dd98243,
    0x554091923d2db868, 0x811fd50d4c22ccac, 0xc92b3e5dc0cc7907, 0xd67595f0c78a2adc,
    0xe803129be8f56bbd, 0x4d17e6d8974539b6, 0x728aeb4255d718f2, 0xdf55fc02fadee6fb,
    0x804696586b307c2d, 0xd6d0b61d35cf3a8f, 0xf695651546e664ad, 0x6733e61c1876521b,
    0xee670cfb22231fa2, 0x98aee0f1cf93d635, 0x8d917804e6fa06e0, 0x190231b252d11bbd,
    0x79322bd2374087bb, 0x25648216dc2eb2fb, 0x270e17f300a4bad3, 0xd3cefdefc2aaacf4,
    0xe74149effc53e36b, 0x9c20fa511786bbdb, 0x3cc52d7180a6c8b9, 0x6fda6bc67f142bba,
    0x3ab02ef2f1254481, 0x535148b2cb32dfc3, 0x52ce9bf3612f0481, 0x314c84d57c1cabbe,
    0xd23818b4d35a1807, 0x1b35cd44494cce47, 0x9be3894d091d7a7d, 0xd9f75af5616fdf15,
    0x187d777c8e94d1a5, 0x90f1edce7afb87fa, 0x564e0ad33e0a9b38, 0xc4ec729db196a960,
    0xaab17e62346ff70a, 0xf488bfe453e3de32, 0xdd78dd077709b4f7, 0x23d4f4990a45aeb7,
    0x6f124163247c533c, 0x80f9a35ce68db024, 0x9f569a596b0a14fb, 0x7a3c927038f04452,
    0x42646db1effc9757, 0x8093f205287fb113, 0x53f6ddf4b0e838b8, 0x31e9eafafcb191dd,
    0x804fc7a32729a50f, 0x5f7ea2982442beab, 0xe66a4bd6a76e0524, 0x21ea254353e871a7,
    0x4dcc4abe01e6d035, 0xa4ca473d18391cfb, 0xa3f367f1112fa61f, 0xd36e89380b132a5b,
    0x1ec20b86967118a5, 0x45bb6dad7ce9d065, 0x6e0e2bb8a76edd68, 0xc88f2b821a9be10d,
    0x489fc75d95972cfc, 0xaebb663c1c18acf5, 0xcb3bb7a9e571603c, 0x4d860ea3b7139b58,
    0xc45f8fd45dcb929a, 0xe17b604e788d1dfa, 0xedbe67eeac34b0f2, 0xcdf2feee520bec9d,
    0x4bb154cdb9c9e4ef, 0xbf9f95c0b46ae82b, 0x1aa03ca719b18b4f, 0xeee981fe401cc8e2,
    0x75c20d04dee7f560, 0x9dd59cfe62c40838, 0x41033f7d6557efac, 0x3f4364bfed7b3fd6,
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
