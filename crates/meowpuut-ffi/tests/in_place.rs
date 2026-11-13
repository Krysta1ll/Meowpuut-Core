//! `mpn_lexicon_create_from_base_in_place`: same candidates as the copying
//! loader, nothing large on the heap, and the release callback runs once,
//! after the last session is gone.
#![allow(unsafe_code)]
#![allow(clippy::multiple_unsafe_ops_per_block)]

use std::ffi::{c_void, CStr};
use std::sync::atomic::{AtomicUsize, Ordering};

use meowpuut_core::Lexicon;
use meowpuut_ffi::*;

const PINYIN: &str = "MEOWPUUT-PINYIN-1\nhao\thao3\t好\t好\t4800\nni\tni3\t你\t你\t5000\nnihao\tni3hao3\t你好\t你好\t5400\nshi\tshi4\t是\t是\t5200\nshijie\tshi4jie4\t世界\t世界\t5300\n# context\n你好\t600\n";
const ZHUYIN: &str = "MEOWPUUT-ZHUYIN-1\ncl\tcl3\t好\t好\t4800\nsu\tsu3\t你\t你\t5000\nsucl\tsu3cl3\t你好\t你好\t5400\n";

/// Owns a copy of the container and counts releases.
struct Image {
    bytes: Vec<u8>,
    released: AtomicUsize,
}

unsafe extern "C" fn release(context: *mut c_void) {
    // SAFETY: `context` points to an `Image` that outlives every lexicon in these tests.
    let image = unsafe { &*context.cast::<Image>() };
    image.released.fetch_add(1, Ordering::SeqCst);
}

fn candidates(session: *mut MPNSession, keys: &str) -> Vec<String> {
    // SAFETY: live session handle from the caller.
    unsafe {
        mpn_clear(session);
        for k in keys.bytes() {
            mpn_key(session, i32::from(k));
        }
        (0..mpn_candidate_count(session))
            .map(|i| CStr::from_ptr(mpn_candidate(session, i)).to_string_lossy().into_owned())
            .collect()
    }
}

fn check(base: Vec<u8>, version: u32) {
    assert_eq!(u32::from_le_bytes(base[12..16].try_into().unwrap()), version);
    let image = Image { bytes: base, released: AtomicUsize::new(0) };
    let context = std::ptr::from_ref(&image).cast_mut().cast::<c_void>();
    for (pinyin, keys) in [(1, "nihaoshijie"), (0, "su3cl3")] {
        // SAFETY: `image` outlives the lexicon and its session; release only counts.
        unsafe {
            let copied = mpn_lexicon_create_from_base(image.bytes.as_ptr().cast(), image.bytes.len(), pinyin);
            let before = image.released.load(Ordering::SeqCst);
            let mapped = mpn_lexicon_create_from_base_in_place(
                image.bytes.as_ptr().cast(), image.bytes.len(), pinyin, Some(release), context);
            assert!(!copied.is_null() && !mapped.is_null());
            if version == 2 {
                assert_eq!(mpn_lexicon_heap_bytes(mapped), 0);
            }
            assert!(mpn_lexicon_heap_bytes(copied) > 0);
            let (a, b) = (mpn_session_create(copied), mpn_session_create(mapped));
            let expected = candidates(a, keys);
            assert!(!expected.is_empty());
            assert_eq!(candidates(b, keys), expected);
            mpn_lexicon_destroy(mapped);
            assert_eq!(image.released.load(Ordering::SeqCst), before, "session still holds the lexicon");
            assert_eq!(candidates(b, keys), expected);
            mpn_session_destroy(b);
            assert_eq!(image.released.load(Ordering::SeqCst), before + 1);
            mpn_session_destroy(a);
            mpn_lexicon_destroy(copied);
        }
    }
}

#[test]
fn in_place_matches_copy_and_releases_once() {
    let py = Lexicon::load(PINYIN).unwrap().to_binary();
    let zy = Lexicon::load(ZHUYIN).unwrap().to_binary();
    check(Lexicon::pack_base(&py, &zy), 2);
    check(Lexicon::pack_base_v1(&py, &zy), 1);
}

#[test]
fn in_place_failure_releases_and_returns_null() {
    let image = Image { bytes: b"MIMO-BASE-1\0not a lexicon at all, just bytes".to_vec(), released: AtomicUsize::new(0) };
    let context = std::ptr::from_ref(&image).cast_mut().cast::<c_void>();
    // SAFETY: valid bytes; release only counts.
    unsafe {
        let lex = mpn_lexicon_create_from_base_in_place(image.bytes.as_ptr().cast(), image.bytes.len(), 1, Some(release), context);
        assert!(lex.is_null());
        assert_eq!(image.released.load(Ordering::SeqCst), 1);
        let lex = mpn_lexicon_create_from_base_in_place(std::ptr::null(), 0, 1, Some(release), context);
        assert!(lex.is_null());
        assert_eq!(image.released.load(Ordering::SeqCst), 2);
        assert!(mpn_lexicon_create_from_base_in_place(std::ptr::null(), 0, 1, None, std::ptr::null_mut()).is_null());
    }
}

/// An empty pair table (no pairs, zero backoff) for the word list of `base`.
fn empty_pairs(base: &[u8]) -> Vec<u8> {
    let lex = Lexicon::from_base_in_place(Box::leak(base.to_vec().into_boxed_slice()), true, None).unwrap();
    let ids = &lex.index().word_ids;
    let mut out = b"MIMO-PAIR-1\0".to_vec();
    for v in [ids.count, 0, 0] {
        out.extend(v.to_le_bytes());
    }
    out.extend(ids.fingerprint.to_le_bytes());
    out.resize(64 + 4 * (ids.count as usize + 1) + 2 * ids.count as usize, 0);
    out.resize(out.len().div_ceil(4) * 4, 0);
    out
}

#[test]
fn attach_pairs_releases_once() {
    let py = Lexicon::load(PINYIN).unwrap().to_binary();
    let zy = Lexicon::load(ZHUYIN).unwrap().to_binary();
    let base = Lexicon::pack_base(&py, &zy);
    let table = Image { bytes: empty_pairs(&base), released: AtomicUsize::new(0) };
    let context = std::ptr::from_ref(&table).cast_mut().cast::<c_void>();
    let mut other = table.bytes.clone();
    other[24] ^= 1;
    // SAFETY: `base`, `table` and `other` outlive the lexicons and sessions; release only counts.
    unsafe {
        let lex = mpn_lexicon_create_from_base(base.as_ptr().cast(), base.len(), 1);
        assert_eq!(mpn_lexicon_attach_pairs(lex, other.as_ptr().cast(), other.len(), Some(release), context), 0);
        assert_eq!(table.released.load(Ordering::SeqCst), 1, "a table for another word list is released at once");
        assert_eq!(mpn_lexicon_attach_pairs(lex, table.bytes.as_ptr().cast(), table.bytes.len(), Some(release), context), 1);
        let session = mpn_session_create(lex);
        assert_eq!(candidates(session, "nihao").first().map(String::as_str), Some("你好"));
        assert_eq!(mpn_lexicon_attach_pairs(lex, table.bytes.as_ptr().cast(), table.bytes.len(), Some(release), context), 0);
        assert_eq!(table.released.load(Ordering::SeqCst), 2, "no table once sessions exist");
        mpn_lexicon_destroy(lex);
        assert_eq!(table.released.load(Ordering::SeqCst), 2);
        mpn_session_destroy(session);
        assert_eq!(table.released.load(Ordering::SeqCst), 3);
        let tsv = mpn_lexicon_create(PINYIN.as_ptr().cast(), PINYIN.len());
        assert_eq!(mpn_lexicon_attach_pairs(tsv, table.bytes.as_ptr().cast(), table.bytes.len(), Some(release), context), 0);
        assert_eq!(table.released.load(Ordering::SeqCst), 4, "a lexicon without word ids takes no table");
        mpn_lexicon_destroy(tsv);
    }
}
