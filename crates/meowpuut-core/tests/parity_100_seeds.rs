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
    0x7f019cc22daa0f34, 0x973f1fb12f71c1cb, 0xa6a4946ec8aad074, 0x1d55e678f4d05783,
    0xd663c84ab60fe6f2, 0x424bda1b65d34101, 0xb3e61e698ae04987, 0x4d4577a917748d02,
    0xc974d9ebb8b9269f, 0x6d4610fd4ed4f366, 0x8e1876791cf10a5c, 0x6440e5e6598eb25d,
    0xb6ce27e96b2186e4, 0xac4bc4445774014b, 0x6231c3b6e93c7bdc, 0xb42ffe4c47748d4a,
    0xa6f273d58d4d81b2, 0x801894ea064e164d, 0x8779db56e85f16e6, 0x52f452734831dcc1,
    0x4adbc0c3c75cd768, 0xe12143bb5c83a82b, 0x5f743c0a54ae66f9, 0xb0300399e681346d,
    0x1075dd5e977b0092, 0x5b7e6b4cb504f67e, 0x33382b0afa90fc33, 0x5bc7bdf19dea0ec1,
    0x02e377f2b7f3a662, 0x32e52646a0c3bb63, 0xe5549a8f70a2b39d, 0x2d02de6bb667e3d6,
    0x5fd49430e920337b, 0x25cd179bee15f392, 0xe4a43596a96fbb23, 0x567ac20044a6bf70,
    0xa4eae40d951921fd, 0x02d66ea025611486, 0xf71dc97de8322494, 0x55ed42e946c374e3,
    0x6157d7ea3e4ed409, 0xfc80e07a602fa88b, 0xba656b120e520a4e, 0xc2dd2cf1c8656737,
    0x9788ee4900400f36, 0xbf3bafa924785469, 0x45bc5dd3d4ddbc97, 0x23ed8acebf75dabb,
    0x658d1e696cb1cb84, 0x07a88471a0811f70, 0x1f0c3266984954e0, 0x2ba2f73465897650,
    0x8d9353f637aea34c, 0xd596f9fc6e54a3ca, 0x34e67f384a91c565, 0x9ada80000cc04f12,
    0x3463439e02a4bab6, 0x2108e69614808549, 0x0576ef2d64abc189, 0xa856ed5cff0111df,
    0xfcc7f44d197d9182, 0xa8e2c99f718805cb, 0x31e88e72442bb4a7, 0x0662e3d10c4b07e7,
    0xdc7fc88ad2711baf, 0xa2092d288c55f6a5, 0xb639929533063480, 0x4f24b48d7a25c1c9,
    0xed927a26ec6a963f, 0x055e86cb6f811cd9, 0x9bb9663548998f8d, 0x587936aabb04f667,
    0x57a0a614dbd8a2e2, 0xda4c7d1cebf6e586, 0x3821e8c42fef7e7e, 0x71ff69bc8abf4a33,
    0x97133b61677627ea, 0x5eaa882a2384d3e9, 0xf0218cb8a693cc72, 0xfc31ae86c5b1e0b2,
    0x5943f07eabb44587, 0x2b6cae75da04fa63, 0xfd2560c54f4c763d, 0x37b6e8b0663bee19,
    0x80ec13088cbf5db5, 0x2c92adf4aec440ba, 0x76b4e6e77d511808, 0x4bc80a0e264a8330,
    0x6adaf6651b01736b, 0xf1d0efbd9829fce5, 0x7b9f334b22a7a80e, 0xfdbf0e0c4f269736,
    0x005bc6c7092bf8e6, 0x11caf37ef45067b5, 0x620ea4960ef4e873, 0x691e06631388b1a1,
    0x75110ada1d69bfaa, 0xc09961ecf45cbc04, 0xed3272c3670445cc, 0x4cbe496a71a2028b,
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
