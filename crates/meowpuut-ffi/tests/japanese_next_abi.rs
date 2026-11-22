//! C ABI of the Japanese next-word image (`mpn_japanese_next_*`), over an image written by the
//! test writer of the core tests.
#![allow(unsafe_code)]
#![allow(clippy::undocumented_unsafe_blocks, clippy::multiple_unsafe_ops_per_block)]

#[path = "../../meowpuut-core/tests/support/mimo_jn_writer.rs"]
mod mimo_jn_writer;

use std::ffi::{c_char, CStr};
use std::ptr::{null, null_mut};

use meowpuut_ffi::*;
use mimo_jn_writer::{image, Key};

const HEADER: &str = include_str!("../include/meowpuut.h");

fn s(p: *const c_char) -> String {
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

fn fixture() -> Vec<u8> {
    image(&[
        Key::new("ありがとう", 0, &[("ございます", 228), ("ございました", 132)]),
        Key::new("ご飯を", 0, &[("食べる", 11)]),
        Key::new("ました", 1, &[("か", 733), ("が", 66)]),
    ])
}

fn predict(query: *mut MPNJapaneseNextQuery, next: *const MPNJapaneseNext, context: &CStr, limit: usize) -> Vec<String> {
    unsafe {
        let count = mpn_japanese_next_predict(query, next, context.as_ptr(), limit);
        assert_eq!(s(mpn_japanese_next_text(query, count)), "");
        (0..count).map(|index| s(mpn_japanese_next_text(query, index))).collect()
    }
}

#[test]
fn header_declares_every_function() {
    for name in [
        "mpn_japanese_next_open", "mpn_japanese_next_close", "mpn_japanese_next_key_count",
        "mpn_japanese_next_query_create", "mpn_japanese_next_query_destroy", "mpn_japanese_next_predict",
        "mpn_japanese_next_text",
    ] {
        assert!(HEADER.contains(&format!("{name}(")), "{name} is not in meowpuut.h");
    }
}

#[test]
fn opens_in_place_and_copied_and_predicts() {
    let bytes = fixture();
    unsafe {
        for copy in [0, 1] {
            let next = mpn_japanese_next_open(bytes.as_ptr().cast(), bytes.len(), copy);
            assert!(!next.is_null());
            assert_eq!(mpn_japanese_next_key_count(next), 3);
            let query = mpn_japanese_next_query_create();
            assert!(!query.is_null());
            assert_eq!(predict(query, next, c"ありがとう", 8), ["ございます", "ございました"]);
            assert_eq!(predict(query, next, c"ありがとう", 1), ["ございます"]);
            assert_eq!(predict(query, next, c"ご飯\x1fを", 8), ["食べる"]);
            assert_eq!(predict(query, next, c"食べました", 8), ["か", "が"]);
            // The previous result is replaced, also by an empty one.
            assert!(predict(query, next, c"猫", 8).is_empty());
            assert_eq!(s(mpn_japanese_next_text(query, 0)), "");
            mpn_japanese_next_query_destroy(query);
            mpn_japanese_next_close(next);
        }
    }
}

#[test]
fn invalid_arguments_give_nothing() {
    let bytes = fixture();
    unsafe {
        assert!(mpn_japanese_next_open(null(), 16, 1).is_null());
        assert!(mpn_japanese_next_open(bytes.as_ptr().cast(), 0, 1).is_null());
        assert!(mpn_japanese_next_open(bytes.as_ptr().cast(), usize::MAX, 1).is_null());
        assert!(mpn_japanese_next_open(bytes.as_ptr().cast(), bytes.len() - 8, 1).is_null());
        let mut wrong = bytes.clone();
        wrong[0] = b'X';
        assert!(mpn_japanese_next_open(wrong.as_ptr().cast(), wrong.len(), 0).is_null());
        assert_eq!(mpn_japanese_next_key_count(null()), 0);
        mpn_japanese_next_close(null_mut());
        mpn_japanese_next_query_destroy(null_mut());
        assert_eq!(s(mpn_japanese_next_text(null(), 0)), "");

        let next = mpn_japanese_next_open(bytes.as_ptr().cast(), bytes.len(), 1);
        let query = mpn_japanese_next_query_create();
        assert_eq!(mpn_japanese_next_predict(null_mut(), next, c"ありがとう".as_ptr(), 8), 0);
        assert_eq!(mpn_japanese_next_predict(query, next, c"ありがとう".as_ptr(), 8), 2);
        // A NULL image, a NULL context and a context that is not UTF-8 clear the result.
        assert_eq!(mpn_japanese_next_predict(query, null(), c"ありがとう".as_ptr(), 8), 0);
        assert_eq!(s(mpn_japanese_next_text(query, 0)), "");
        assert_eq!(mpn_japanese_next_predict(query, next, null(), 8), 0);
        let not_utf8 = [0xffu8, 0xfe, 0];
        assert_eq!(mpn_japanese_next_predict(query, next, not_utf8.as_ptr().cast(), 8), 0);
        mpn_japanese_next_query_destroy(query);
        mpn_japanese_next_close(next);
    }
}
