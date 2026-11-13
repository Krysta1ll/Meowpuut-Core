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
    "nihao", "woxihuanni", "jintiantianqihenhao", "zhongguoren", "xiansheng",
    "pengyou", "shijieheping", "woxihuannidenekohenkeai", "nihaoshijie",
    "womenshihaopengyou", "zhuyinshurufa", "diannao", "chengxuyuan", "kaixin",
    "xiexieni", "mingtianjian", "bukeqi", "duibuqi", "meiguanxi", "zaoshanghao",
    "wanan", "woaini", "taibei", "dongjing", "watashixihuanneko", "sakurahenmei",
    "ni3hao3", "xian'an", "zhong1wen2", "ying1wen2", "ri4wen2", "fa3wen2",
    "nihaom", "woxihuanb", "womens", "jintianw", "pengyoum", "xianshengd",
    "nh", "wxhn", "jttqhh", "zgr", "xs", "py", "sjhp",
    "nihaonihao", "nihaoa", "shurufaceshi", "zheshiyigeceshi", "qingwen",
];

const ZHUYIN_POOL: [&str; 19] = [
    "su3cl3", "ji3vu3c0u3", "rupwu0wu0fu4cpcl3", "5j/eji6bp6", "v0g/ ",
    "q/7u.6", "g4r,4ck6qu/6", "su3cl3g4r,4", "ji3mp3g4cl3q/7u.6",
    "1j6e06", "vm,4vm,4su3", "au/6wu0ru04", "1j4d4f3", "ao6ej0uv7",
    "su3cl3a87", "5j/eji6", "t/6vu4m06", "d9vup ", "2u04sao3",
];

const CONTEXT_POOL: [&str; 10] = [
    "你", "我", "我們", "今天", "喜歡", "中國", "台灣", "世界", "朋友", "",
];

const GOLDEN_100_SEEDS: [u64; 100] = [
    0x38b636155f16b2b0, 0x887275dd90ac4068, 0x9cefb856c19af30f, 0xd547eedb594cec1e,
    0xe65b62617a82a55a, 0x3ee16dcdd19c66a8, 0x8c175530428ee1df, 0xe85401222b296676,
    0x8231690d09345b6e, 0x1a18b00b530e709b, 0x1c105e2cf332b002, 0x544017f374505a3f,
    0xf764d3879934d25f, 0xda7319c174affcdb, 0x1b4aa00ed21e186a, 0x1e468554884d7404,
    0x519efac699f0a83c, 0xedaadbf434934337, 0xafca846072a5d411, 0xb26c960419733843,
    0x939ce59673749909, 0x8f6986c853947055, 0x501e78f446aa7288, 0xb06f83de64dc3f3c,
    0x4109889558363d94, 0x0b9779e06b53d7b8, 0xa5abf1ebc862d483, 0xafbe855ddac764c9,
    0xb0d884afb9cd0fad, 0xa5ce35e48441f829, 0x8237fe6747e592d7, 0xc54036445e40b229,
    0x6c0d252b3d671984, 0x14251378b8b98f53, 0xae519ab1da7e5e62, 0x6307cc2bbc2b594b,
    0xbc20937f84344acf, 0x8e4a942d83e35522, 0x6606a3494fd083e3, 0x8c03d33e515ce078,
    0xb447aba04bf133a8, 0x0704a85d79df38c0, 0x5108ef5739d57433, 0x7fac119d1df981e7,
    0xd26cabcf170b812d, 0x4bbcbfebb6205b26, 0x684934bfde670d3e, 0x03c67096ea0ee52c,
    0x4d4518ea07bf4d8c, 0x811ef5028d5aec00, 0xf8e8d2e490451e13, 0xe96bab46a4b33f24,
    0xce7793d665f9016f, 0xa802bb45f65c60f4, 0x9b1312d3f2c5ec63, 0x7a90c8277ea9d72a,
    0xe844289981b3d724, 0xa4598eab229b8bef, 0x435e9f5ec7ea83d2, 0xcf0f4480565b43db,
    0x7c5f7f159019ce02, 0x4997cb55c58879a6, 0xd8e457ceab64329f, 0x870182d0fa116c37,
    0x73d100296fedbc82, 0xb0f69a9799f5ef5b, 0xe8b3f2116e7e1814, 0x6158cba65690c624,
    0x15e0ea50659a6cf1, 0x6be59e526cd309e8, 0x16c959cfe51e3b20, 0xe1a0893cbe1c90d0,
    0x34e29b07b793dae6, 0xfea5525570d7f793, 0x7bf44e180f9162a1, 0x1f00bae43f69fd1b,
    0xc7de21659f1f8f66, 0x9996a9ee99d6432e, 0x23043fa2fab6a801, 0x99dcd53244619f89,
    0x010d3028a435dc9f, 0xe3e7af497b211aa7, 0x4c31493883757d45, 0x7e2cf59c13ad66d7,
    0x733e2fa2ce9ab2ad, 0xb02f9a52c73a970a, 0x892b3ce132d454fb, 0x86c407dd309a682c,
    0x4c188deaf6e45063, 0x2eab57e33d3cbf69, 0xe2eabe5614200986, 0xb4a84dfb314d1581,
    0xe45a68fa526dce94, 0x43701dc676bc87fa, 0x16b6394cc71c75fa, 0x0c11c73eb960f044,
    0xf3bc245dc3a7a530, 0x8f158071914199fd, 0xfb23c9e7daa4d661, 0x69c6df161afdf48e,
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
        assert_eq!(
            actual_hash, expected_hash,
            "Mismatch at seed {seed}: actual={actual_hash:016x}, expected={expected_hash:016x}"
        );
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
