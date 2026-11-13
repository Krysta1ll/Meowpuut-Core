//! Replays the 100-seed simulation through the exported C ABI and compares
//! results with the core's rankings. Zhuyin keeps the original C++ baseline;
//! Pinyin includes the current correction rules. Also exercises the ABI's
//! NUL-terminated strings and `MPNCandidateInfo` layout.
#![allow(unsafe_code)]
#![allow(clippy::undocumented_unsafe_blocks, clippy::multiple_unsafe_ops_per_block)]

use std::ffi::{c_char, CStr, CString};
use std::fs;
use std::path::PathBuf;

use meowpuut_ffi::*;

const PY: [&str; 50] = [
    "nihao", "woxihuanni", "jintiantianqihenhao", "zhongguoren", "xiansheng", "pengyou", "shijieheping",
    "woxihuannidenekohenkeai", "nihaoshijie", "womenshihaopengyou", "zhuyinshurufa", "diannao", "chengxuyuan",
    "kaixin", "xiexieni", "mingtianjian", "bukeqi", "duibuqi", "meiguanxi", "zaoshanghao", "wanan", "woaini",
    "taibei", "dongjing", "watashixihuanneko", "sakurahenmei", "ni3hao3", "xian'an", "zhong1wen2", "ying1wen2",
    "ri4wen2", "fa3wen2", "nihaom", "woxihuanb", "womens", "jintianw", "pengyoum", "xianshengd", "nh", "wxhn",
    "jttqhh", "zgr", "xs", "py", "sjhp", "nihaonihao", "nihaoa", "shurufaceshi", "zheshiyigeceshi", "qingwen",
];
const ZY: [&str; 19] = [
    "su3cl3", "ji3vu3c0u3", "rupwu0wu0fu4cpcl3", "5j/eji6bp6", "v0g/ ", "q/7u.6", "g4r,4ck6qu/6",
    "su3cl3g4r,4", "ji3mp3g4cl3q/7u.6", "1j6e06", "vm,4vm,4su3", "au/6wu0ru04", "1j4d4f3", "ao6ej0uv7",
    "su3cl3a87", "5j/eji6", "t/6vu4m06", "d9vup ", "2u04sao3",
];
const CTX: [&str; 10] = ["你", "我", "我們", "今天", "喜歡", "中國", "台灣", "世界", "朋友", ""];

fn splitmix64(s: &mut u64) -> u64 {
    *s = s.wrapping_add(0x9e3779b97f4a7c15);
    let mut z = *s;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
    z ^ (z >> 31)
}
fn mixb(h: &mut u64, d: &[u8]) {
    for &b in d {
        *h ^= u64::from(b);
        *h = h.wrapping_mul(1099511628211);
    }
}
fn mixs(h: &mut u64, p: *const c_char) {
    let bytes = unsafe { CStr::from_ptr(p) }.to_bytes();
    mixb(h, bytes);
    mixb(h, &[0xff]);
}
fn mixu(h: &mut u64, v: u64) {
    mixb(h, &v.to_le_bytes());
}

unsafe fn run_seed(s: *mut MPNSession, seed: u64, py: bool) -> u64 {
    unsafe { run_seed_inner(s, seed, py) }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn run_seed_inner(s: *mut MPNSession, seed: u64, py: bool) -> u64 {
    let (mut rng, mut h) = (seed, 14695981039346656037u64);
    for _ in 0..50 {
        let r = splitmix64(&mut rng);
        let op = r % 100;
        let mut handled = false;
        if op < 55 {
            let ph = if py { PY[((r >> 8) as usize) % PY.len()] } else { ZY[((r >> 8) as usize) % ZY.len()] };
            let take = 1 + ((r >> 16) as usize % ph.len());
            for &b in &ph.as_bytes()[..take] {
                handled = mpn_key(s, i32::from(b)) != 0;
            }
        } else if op < 65 {
            handled = mpn_key(s, 0xff08) != 0;
        } else if op < 75 {
            let c = mpn_candidate_count(s);
            if c > 0 {
                handled = mpn_choose(s, ((r >> 8) as usize) % c) != 0;
            }
        } else if op < 82 {
            handled = mpn_key(s, if r & 1 != 0 { 0xff56 } else { 0xff55 }) != 0;
        } else if op < 88 {
            mpn_set_simplified(s, ((r >> 8) & 1) as i32);
            handled = true;
        } else if op < 94 {
            let ctx = CString::new(CTX[((r >> 8) as usize) % CTX.len()]).unwrap();
            mpn_set_segment_context(s, ctx.as_ptr());
            handled = true;
        } else if op < 97 {
            handled = mpn_key(s, 0xff0d) != 0;
        } else {
            handled = mpn_key(s, 0xff1b) != 0;
        }
        mixu(&mut h, u64::from(handled));
        mixs(&mut h, mpn_input(s));
        mixs(&mut h, mpn_preedit(s));
        mixs(&mut h, mpn_committed(s));
        mixs(&mut h, mpn_segment_context(s));
        mixu(&mut h, mpn_page(s) as u64);
        let c = mpn_candidate_count(s);
        mixu(&mut h, c as u64);
        for i in 0..c {
            mixs(&mut h, mpn_candidate(s, i));
            mixu(&mut h, mpn_candidate_consumed(s, i) as u64);
            let mut info = MPNCandidateInfo::default();
            let ok = mpn_candidate_info(s, i, &mut info, std::mem::size_of::<MPNCandidateInfo>());
            mixu(&mut h, u64::from(ok != 0));
            mixu(&mut h, u64::from(info.kind));
            mixu(&mut h, u64::from(info.score_kind));
            mixu(&mut h, info.consumed_bytes);
            mixu(&mut h, info.remaining_offset_bytes);
            let score = CString::new(format!("{:.9}", info.score)).unwrap();
            mixs(&mut h, score.as_ptr());
        }
        if py {
            let q = CString::new(PY[((r >> 24) as usize) % PY.len()]).unwrap();
            let simp = ((r >> 32) & 1) as i32;
            mixs(&mut h, mpn_mixed_pinyin(s, q.as_ptr(), simp));
            let mut used = 0usize;
            let p = mpn_mixed_pinyin_prefix(s, q.as_ptr(), simp, &mut used);
            mixs(&mut h, p);
            mixu(&mut h, used as u64);
        }
        mpn_clear_commit(s);
    }
    h
}

fn golden() -> Vec<u64> {
    let src = include_str!("../../meowpuut-core/tests/parity_100_seeds.rs");
    let start = src.find("GOLDEN_100_SEEDS").unwrap();
    src[start..]
        .split("0x")
        .skip(1)
        .take(100)
        .map(|t| u64::from_str_radix(&t[..16], 16).unwrap())
        .collect()
}

#[test]
fn c_abi_matches_core_goldens_for_100_seeds() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.build/lexicon");
    let pyd = fs::read(root.join("native-pinyin.tsv")).unwrap();
    let zyd = fs::read(root.join("native-zhuyin.tsv")).unwrap();
    let golden = golden();
    assert_eq!(golden.len(), 100);
    unsafe {
        let pyl = mpn_lexicon_create(pyd.as_ptr().cast(), pyd.len());
        let zyl = mpn_lexicon_create(zyd.as_ptr().cast(), zyd.len());
        assert!(!pyl.is_null() && !zyl.is_null());
        let (ps, zs) = (mpn_session_create(pyl), mpn_session_create(zyl));
        for (r, t, w) in [
            ("watashi", "私", 5500), ("neko", "猫", 5200), ("kawaii", "可愛い", 5300), ("arigatou", "ありがとう", 5600),
            ("konnichiwa", "こんにちは", 5400), ("sakura", "桜", 4800), ("tokyo", "東京", 5100), ("sushi", "寿司", 4900),
            ("au", "会う", 4200), ("ai", "愛", 3500),
        ] {
            let (r, t) = (CString::new(r).unwrap(), CString::new(t).unwrap());
            mpn_add_japanese_word(ps, r.as_ptr(), t.as_ptr(), w);
        }
        for (seed, expected) in golden.iter().enumerate() {
            let py = seed % 2 == 0;
            let s = if py { ps } else { zs };
            mpn_clear(s);
            mpn_set_simplified(s, 0);
            let actual = run_seed(s, seed as u64, py);
            assert_eq!(actual, *expected, "seed {seed}: {actual:016x} != {expected:016x}");
        }
        mpn_session_destroy(ps);
        mpn_session_destroy(zs);
        // Sessions own an Arc; destroying the lexicon handles afterwards is valid.
        mpn_lexicon_destroy(pyl);
        mpn_lexicon_destroy(zyl);
    }
}
