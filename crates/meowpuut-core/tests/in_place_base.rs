//! The shipped `mimo_base.bin` read in place (version 2) gives the same index
//! and the same candidates as the copying loader with a heap-built index
//! (version 1), and a version 2 file written by `scripts/mimo_binary.py` is
//! byte-identical to `Lexicon::pack_base` over the same two images.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use meowpuut_core::lexicon::LexiconIndex;
use meowpuut_core::{Lexicon, Session};

fn read_u32(b: &[u8], at: usize) -> usize {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize
}

fn same_index(a: &LexiconIndex, b: &LexiconIndex) -> bool {
    a.bucket.columns() == b.bucket.columns()
        && a.context.columns() == b.context.columns()
        && a.traditional.columns() == b.traditional.columns()
        && a.simplified.columns() == b.simplified.columns()
        && a.initials.columns() == b.initials.columns()
}

fn candidates(session: &mut Session, keys: &str, simplified: bool) -> Vec<String> {
    session.clear();
    session.set_simplified(simplified);
    for k in keys.bytes() {
        session.key(i32::from(k));
    }
    (0..session.candidate_count()).map(|i| session.candidate(i).to_string()).collect()
}

#[test]
fn shipped_base_in_place_equals_copy() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let file = fs::read(root.join(".build/lexicon/mimo_base.bin"))
        .expect(".build/lexicon/mimo_base.bin missing; run scripts/build-lexicon.sh");
    let image = |off: usize, len: usize| &file[read_u32(&file, off)..read_u32(&file, off) + read_u32(&file, len)];
    let (py, zy) = (image(16, 20), image(24, 28));
    let v2 = Lexicon::pack_base(py, zy);
    if read_u32(&file, 12) == 2 {
        assert!(file == v2, "mimo_binary.py and Lexicon::pack_base write different bytes");
    }
    let v1: &'static [u8] = Box::leak(Lexicon::pack_base_v1(py, zy).into_boxed_slice());
    let v2: &'static [u8] = Box::leak(v2.into_boxed_slice());

    let inputs = [
        (true, "nihao"), (true, "zhongguoren"), (true, "woxiangqu"), (true, "shi"), (true, "zhonghuarenmingongheguo"),
        (true, "xian"), (true, "nhao"), (true, "zg"), (true, "jintiantianqihenhao"), (true, "lvse"),
        (false, "su3cl3"), (false, "5j/ cj86"), (false, "ji3"), (false, "1j4ji3"), (false, "z/6"),
    ];
    for pinyin in [true, false] {
        let copied = Lexicon::load_from_base(v1, pinyin).unwrap();
        let mapped = Lexicon::from_base_in_place(v2, pinyin, None).unwrap();
        let mapped_v1 = Lexicon::from_base_in_place(v1, pinyin, None).unwrap();
        assert!(same_index(copied.index(), mapped.index()));
        assert!(same_index(copied.index(), mapped_v1.index()));
        assert!(mapped.heap_bytes() == 0, "in place: {} heap bytes", mapped.heap_bytes());
        assert!(copied.heap_bytes() > 10 << 20);
        let mut a = Session::new(Arc::new(copied));
        let mut b = Session::new(Arc::new(mapped));
        for &(mode, keys) in inputs.iter().filter(|(m, _)| *m == pinyin) {
            for simplified in [false, true] {
                let expected = candidates(&mut a, keys, simplified && mode);
                assert!(!mode || !expected.is_empty(), "{keys}");
                assert_eq!(candidates(&mut b, keys, simplified && mode), expected, "{keys}");
            }
        }
    }
    // An index section is refused for an image that changed after it was written.
    let mut changed = v2.to_vec();
    let (py_at, py_len) = (read_u32(v2, 16), read_u32(v2, 20));
    changed[py_at + (py_len - 64) / 63] ^= 1;
    let changed: &'static [u8] = Box::leak(changed.into_boxed_slice());
    assert!(Lexicon::from_base_in_place(changed, true, None).is_none());
    assert!(Lexicon::from_base_in_place(changed, false, None).is_some());

    // The context table is the same in both images, so version 2 stores its index once.
    let (py_idx, zy_idx) = (read_u32(v2, 32), read_u32(v2, 40));
    assert_eq!(read_u32(v2, py_idx + 32), read_u32(v2, zy_idx + 32));
    assert_eq!(read_u32(v2, py_idx + 36), read_u32(v2, zy_idx + 36));
}
