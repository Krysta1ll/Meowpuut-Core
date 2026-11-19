//! Branch-coverage, 100-seed parity, and zero-allocation decoder tests for `crate::trigrams`.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use meowpuut_core::image::{Bytes, U32Col};
use meowpuut_core::lexicon::WordIds;
use meowpuut_core::pairs::Pairs;
use meowpuut_core::trigrams::{pack_image, Trigrams, MAX_BRIDGES};
use meowpuut_core::{Lexicon, Session};

struct SplitMix64(u64);

impl SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn next_usize(&mut self, bound: usize) -> usize {
        (self.next_u64() % bound as u64) as usize
    }
    fn next_range_i32(&mut self, lo: i32, hi_inclusive: i32) -> i32 {
        let span = (hi_inclusive - lo + 1) as u64;
        lo + (self.next_u64() % span) as i32
    }
}

fn synth_ids(count: u32, fp: u64) -> WordIds {
    let mut raw = Vec::with_capacity(count as usize * 4);
    for id in 0..count {
        raw.extend_from_slice(&id.to_le_bytes());
    }
    WordIds { count, fingerprint: fp, ids: U32Col::new(Bytes::owned(raw)) }
}

#[test]
fn trigrams_parse_and_pack_error_branches() {
    let ids = synth_ids(16, 0xCAFE_BABE);
    assert!(Trigrams::load(&[], &ids).is_none());
    let valid = pack_image(&ids, &[2, 5], &[(1, 0, 3, 200), (4, 1, 6, -50)], &[(1, 3, 40), (7, 8, -2048), (9, 10, 2047)], 128, 64).unwrap();
    let loaded = Trigrams::load(&valid, &ids).unwrap();
    assert_eq!((loaded.bridge_count(), loaded.tri_len(), loaded.skip_len(), loaded.weights_q8()), (2, 2, 3, (128, 64)));
    assert_eq!(loaded.heap_bytes(), (2 + 3) * 8);

    // Corrupt header and bridge table fields one by one.
    for (off, xor) in [(0, 1), (12, 1), (16, 1), (20, 99), (24, 1), (64, 0xF0)] {
        let mut bad = valid.clone();
        bad[off] ^= xor;
        assert!(Trigrams::load(&bad, &ids).is_none(), "expected failure when byte {off} xor {xor}");
    }
    let mut bad_dup_bridge = pack_image(&ids, &[2, 3], &[(1, 0, 4, 10)], &[], 256, 256).unwrap();
    bad_dup_bridge[68..72].copy_from_slice(&2u32.to_le_bytes());
    assert!(Trigrams::load(&bad_dup_bridge, &ids).is_none());

    let mut wrong_len = valid.clone();
    wrong_len.pop();
    assert!(Trigrams::load(&wrong_len, &ids).is_none());
    wrong_len.extend_from_slice(&[0u8; 9]);
    assert!(Trigrams::load(&wrong_len, &ids).is_none());

    // Corrupt sorted order, reserved entry bits, and entry ID bounds.
    let mut bad_order = pack_image(&ids, &[2], &[(1, 0, 3, 10), (4, 0, 6, 20)], &[], 256, 256).unwrap();
    let e0 = bad_order[320..328].to_vec();
    let e1 = bad_order[328..336].to_vec();
    bad_order[320..328].copy_from_slice(&e1);
    bad_order[328..336].copy_from_slice(&e0);
    assert!(Trigrams::load(&bad_order, &ids).is_none());

    let mut bad_top_tri = pack_image(&ids, &[2], &[(1, 0, 3, 10)], &[], 256, 256).unwrap();
    bad_top_tri[327] |= 0x80;
    assert!(Trigrams::load(&bad_top_tri, &ids).is_none());

    let mut bad_tri_bounds = pack_image(&ids, &[2], &[(1, 0, 3, 10)], &[], 256, 256).unwrap();
    let bogus_tri = ((15u64 << 28) | (5u64 << 20) | 3u64) << 12;
    bad_tri_bounds[320..328].copy_from_slice(&bogus_tri.to_le_bytes());
    assert!(Trigrams::load(&bad_tri_bounds, &ids).is_none());

    let mut bad_skip_order = pack_image(&ids, &[2], &[], &[(1, 3, 10), (4, 6, 20)], 256, 256).unwrap();
    let s0 = bad_skip_order[320..328].to_vec();
    let s1 = bad_skip_order[328..336].to_vec();
    bad_skip_order[320..328].copy_from_slice(&s1);
    bad_skip_order[328..336].copy_from_slice(&s0);
    assert!(Trigrams::load(&bad_skip_order, &ids).is_none());

    let mut bad_skip_top = pack_image(&ids, &[2], &[], &[(1, 3, 10)], 256, 256).unwrap();
    bad_skip_top[327] |= 0x40;
    assert!(Trigrams::load(&bad_skip_top, &ids).is_none());

    let mut bad_skip_bounds = pack_image(&ids, &[2], &[], &[(1, 3, 10)], 256, 256).unwrap();
    let bogus_skip = ((99u64 << 20) | 3u64) << 12;
    bad_skip_bounds[320..328].copy_from_slice(&bogus_skip.to_le_bytes());
    assert!(Trigrams::load(&bad_skip_bounds, &ids).is_none());

    // pack_image argument validation branches.
    assert!(pack_image(&synth_ids(0, 1), &[], &[], &[], 256, 256).is_none());
    let too_many_bridges: Vec<u32> = (0..=(MAX_BRIDGES as u32)).collect();
    assert!(pack_image(&synth_ids(100, 1), &too_many_bridges, &[], &[], 256, 256).is_none());
    assert!(pack_image(&ids, &[2, 2], &[], &[], 256, 256).is_none());
    assert!(pack_image(&ids, &[16], &[], &[], 256, 256).is_none());
    assert!(pack_image(&ids, &[2], &[(1, 0, 3, 2048)], &[], 256, 256).is_none());
    assert!(pack_image(&ids, &[2], &[], &[(1, 3, -2049)], 256, 256).is_none());
    assert!(pack_image(&ids, &[2], &[(1, 1, 3, 10)], &[], 256, 256).is_none());
    assert!(pack_image(&ids, &[2], &[], &[(16, 3, 10)], 256, 256).is_none());
}

#[test]
fn trigrams_scoring_rules_and_in_place_heap_bytes() {
    let ids = synth_ids(32, 0x9988);
    let raw = pack_image(&ids, &[4, 8], &[(2, 0, 6, 128), (2, 1, 6, -64)], &[(2, 6, 64), (10, 12, 32)], 256, 128).unwrap();
    let leaked: &'static [u8] = Box::leak(raw.into_boxed_slice());
    let mut tri = Trigrams::in_place(leaked, &ids, None).unwrap();
    assert_eq!(tri.heap_bytes(), 0);
    assert!((tri.score_path(&[2, 4])).abs() < 1e-12);
    // Bridge hit suppresses skip1 on (2, 4, 6), while non-bridge 5 falls back to skip1 (2, 6).
    assert!((tri.score_path(&[2, 4, 6]) - 2.0).abs() < 1e-12);
    assert!((tri.score_path(&[2, 5, 6]) - 0.5).abs() < 1e-12);
    assert!((tri.score_path(&[2, u32::MAX, 6])).abs() < 1e-12);
    assert_eq!(tri.score_trigram(2, MAX_BRIDGES as u32, 6), None);
    assert_eq!(tri.score_skip1(1 << 20, 6), None);

    tri.set_weights_q8(512, 256);
    assert!((tri.score_path(&[2, 4, 6, 2, 8, 6, 10, 5, 12]) - 2.5).abs() < 1e-12);
}

#[test]
fn trigrams_100_seeds_deterministic_parity() {
    let mut rng = SplitMix64(0x2026_1119_55AA_33CC);
    for seed in 0..100u64 {
        let word_count = 32 + rng.next_usize(128);
        let ids = synth_ids(word_count as u32, seed ^ 0xA5A5_5A5A);
        let bridge_target = 1 + rng.next_usize(MAX_BRIDGES.min(word_count / 2));
        let mut bridges = Vec::new();
        while bridges.len() < bridge_target {
            let b = (2 + rng.next_usize(word_count - 2)) as u32;
            if !bridges.contains(&b) {
                bridges.push(b);
            }
        }
        let mut tri_map: HashMap<(u32, u32, u32), i32> = HashMap::new();
        for _ in 0..80 {
            let w1 = rng.next_usize(word_count) as u32;
            let b_idx = rng.next_usize(bridges.len()) as u32;
            let w3 = rng.next_usize(word_count) as u32;
            tri_map.insert((w1, b_idx, w3), rng.next_range_i32(-2048, 2047));
        }
        let mut skip_map: HashMap<(u32, u32), i32> = HashMap::new();
        for _ in 0..80 {
            let w1 = rng.next_usize(word_count) as u32;
            let w3 = rng.next_usize(word_count) as u32;
            skip_map.insert((w1, w3), rng.next_range_i32(-2048, 2047));
        }
        let tri_rows: Vec<_> = tri_map.iter().map(|(&(w1, b, w3), &s)| (w1, b, w3, s)).collect();
        let skip_rows: Vec<_> = skip_map.iter().map(|(&(w1, w3), &s)| (w1, w3, s)).collect();
        let w_tri = rng.next_range_i32(0, 512);
        let w_skip = rng.next_range_i32(0, 512);
        let bytes = pack_image(&ids, &bridges, &tri_rows, &skip_rows, w_tri, w_skip).unwrap();
        let tri = Trigrams::load(&bytes, &ids).unwrap();

        let path_len = 1 + rng.next_usize(28);
        let path: Vec<u32> = (0..path_len)
            .map(|_| if rng.next_usize(12) == 0 { u32::MAX } else { rng.next_usize(word_count) as u32 })
            .collect();
        let mut ref_tri = 0i64;
        let mut ref_skip = 0i64;
        for w in path.windows(3) {
            if w[0] < word_count as u32 && w[1] < word_count as u32 && w[2] < word_count as u32 {
                if let Some(b_idx) = bridges.iter().position(|&b| b == w[1]) {
                    if let Some(&s) = tri_map.get(&(w[0], b_idx as u32, w[2])) {
                        ref_tri += i64::from(s);
                        continue;
                    }
                }
                if let Some(&s) = skip_map.get(&(w[0], w[2])) {
                    ref_skip += i64::from(s);
                }
            }
        }
        let ref_score = (ref_tri as f64) * (f64::from(w_tri) / (64.0 * 256.0))
            + (ref_skip as f64) * (f64::from(w_skip) / (64.0 * 256.0));
        let got = tri.score_path(&path);
        assert!((got - ref_score).abs() < 1e-12, "seed {seed}: got {got}, expected {ref_score}");
    }
}

fn find_lexicon_dir() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for rel in ["../../.build/lexicon-v2b", "../../.build/lexicon", "../../Meowpuut-iOS/iOS/Generated"] {
        let p = manifest.join(rel);
        if p.join("mimo_base.bin").exists() && p.join("mimo_pairs.bin").exists() {
            return p;
        }
    }
    panic!("no lexicon directory with mimo_base.bin and mimo_pairs.bin found");
}

#[test]
fn decoder_trigrams_rerank_and_zero_hot_heap_alloc() {
    let dir = find_lexicon_dir();
    let base: &'static [u8] = Box::leak(fs::read(dir.join("mimo_base.bin")).unwrap().into_boxed_slice());
    let pairs_bytes: &'static [u8] = Box::leak(fs::read(dir.join("mimo_pairs.bin")).unwrap().into_boxed_slice());
    let mut lex = Lexicon::from_base_in_place(base, true, None).unwrap();
    let id_of = |w: &str| {
        let idx = (0..lex.words.len()).find(|&i| lex.words.text(&lex.storage, i, true) == w).unwrap();
        lex.index().word_ids.ids.get(idx)
    };
    let (wo, zai, che) = (id_of("我"), id_of("在"), id_of("撤"));

    let mut base_lex = Lexicon::from_base_in_place(base, true, None).unwrap();
    base_lex.set_pairs(Pairs::in_place(pairs_bytes, &base_lex.index().word_ids, None).unwrap());
    let mut base_session = Session::new(Arc::new(base_lex));
    for &k in b"wozaiche" {
        base_session.key(i32::from(k));
    }
    let baseline_top = base_session.candidate(0);
    assert_ne!(baseline_top, "我在撤");

    let tri_bytes: &'static [u8] = Box::leak(
        pack_image(&lex.index().word_ids, &[zai], &[(wo, 0, che, 2047)], &[], 4096, 256)
            .unwrap()
            .into_boxed_slice(),
    );
    let tri = Trigrams::in_place(tri_bytes, &lex.index().word_ids, None).unwrap();
    // Guard branch: without pairs, attaching trigrams alone keeps heap zero and leaves ranking unaffected.
    let mut no_pairs_lex = Lexicon::from_base_in_place(base, true, None).unwrap();
    no_pairs_lex.set_trigrams(tri.clone());
    assert!(no_pairs_lex.trigrams().is_some());
    let mut guard_session = Session::new(Arc::new(no_pairs_lex));
    for &k in b"wozaiche" {
        guard_session.key(i32::from(k));
    }
    assert!(guard_session.candidate_count() > 0);

    // Pairs + trigrams: verify boosted candidate wins top-1 and hot loop allocates 0 heap across 200 decodes.
    lex.set_pairs(Pairs::in_place(pairs_bytes, &lex.index().word_ids, None).unwrap());
    lex.set_trigrams(tri);
    assert_eq!(lex.heap_bytes(), 0);
    let mut session = Session::new(Arc::new(lex));
    let type_keys = |s: &mut Session, keys: &[u8]| {
        s.clear();
        for &k in keys {
            s.key(i32::from(k));
        }
    };
    type_keys(&mut session, b"wozaiche");
    assert_eq!(session.candidate(0), "我在撤");
    let warm_sig = session.capacity_signature();
    for i in 0..200 {
        let keys: &[u8] = if i % 2 == 0 { b"wodeqiangou" } else { b"wozaiche" };
        type_keys(&mut session, keys);
        assert_eq!(session.capacity_signature(), warm_sig, "hot loop grew heap on iter {i}");
    }
}
