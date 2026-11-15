//! `mpn_input_text`: the uncommitted input for committing it raw.
#![allow(unsafe_code)]
#![allow(clippy::undocumented_unsafe_blocks, clippy::multiple_unsafe_ops_per_block)]

use std::ffi::CStr;
use std::ptr::null;

use meowpuut_ffi::*;

const ZHUYIN: &str = concat!(
    "MEOWPUUT-ZHUYIN-1\n",
    "cl\tcl3\t好\t好\t5200\n",
    "su\tsu3\t你\t你\t5400\n",
    "sucl\tsu3cl3\t你好\t你好\t5600\n",
    "vu\tvu \t西\t西\t5000\n",
);

const PINYIN: &str = concat!(
    "MEOWPUUT-PINYIN-1\n",
    "hao\thao3\t好\t好\t4800\n",
    "ni\tni3\t你\t你\t5000\n",
    "nihao\tni3hao3\t你好\t你好\t5400\n",
);

fn s(p: *const std::ffi::c_char) -> String {
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

unsafe fn type_keys(session: *mut MPNSession, keys: &str) {
    for b in keys.bytes() {
        assert_eq!(unsafe { mpn_key(session, i32::from(b)) }, 1);
    }
}

#[test]
fn zhuyin_input_text_is_bopomofo_and_follows_choose() {
    unsafe {
        assert_eq!(s(mpn_input_text(null())), "");
        let lex = mpn_lexicon_create(ZHUYIN.as_ptr().cast(), ZHUYIN.len());
        let ss = mpn_session_create(lex);
        assert_eq!(s(mpn_input_text(ss)), "");
        type_keys(ss, "su3cl3");
        assert_eq!(s(mpn_input(ss)), "su3cl3");
        assert_eq!(s(mpn_input_text(ss)), "ㄋㄧˇㄏㄠˇ");
        assert_eq!(s(mpn_preedit(ss)), "ㄋㄧˇㄏㄠˇ");
        type_keys(ss, "vu");

        let index = (0..mpn_candidate_count(ss))
            .find(|&i| s(mpn_candidate(ss, i)) == "你好" && mpn_candidate_consumed(ss, i) == 6)
            .expect("你好 for su3cl3");
        assert_eq!(mpn_choose(ss, index), 1);
        assert_eq!(s(mpn_committed(ss)), "你好");
        assert_eq!(s(mpn_input_text(ss)), "ㄒㄧ");

        // Backspace (X11 BackSpace) keeps the getter in step.
        assert_eq!(mpn_key(ss, 0xff08), 1);
        assert_eq!(s(mpn_input_text(ss)), "ㄒ");
        mpn_clear(ss);
        assert_eq!(s(mpn_input_text(ss)), "");
        mpn_session_destroy(ss);
        mpn_lexicon_destroy(lex);
    }
}

#[test]
fn pinyin_input_text_is_the_letters_typed() {
    unsafe {
        let lex = mpn_lexicon_create(PINYIN.as_ptr().cast(), PINYIN.len());
        let ss = mpn_session_create(lex);
        type_keys(ss, "ni3hao");
        assert_eq!(s(mpn_input_text(ss)), "ni3hao");
        assert_eq!(s(mpn_input_text(ss)), s(mpn_input(ss)));
        mpn_session_destroy(ss);
        mpn_lexicon_destroy(lex);
    }
}
