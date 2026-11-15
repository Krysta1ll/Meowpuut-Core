//! Exported `mpn_english_*` functions: the English lexicon image (`mimo_en.bin`,
//! MIMO-EN-1) and the English keyboard's completions, corrections, next words
//! and keypad words (see the English block of `include/meowpuut.h`).
//!
//! An `MPNEnglish` is read-only and may be shared by any number of threads and
//! queries. An `MPNEnglishQuery` holds the result of one call and is used on one
//! thread. NULL handles give 0 or `""`. The text before the cursor passed in
//! `before` is only read during the call; nothing is kept or written.
//!
//! # Examples
//!
//! ```
//! use meowpuut_ffi::*;
//! use std::ffi::CStr;
//! let words = "the\tthe\t1000\t-\nthey\tthey\t300\t-\nok\tOK\t50\tacronym,in-chinese\n";
//! let image = meowpuut_core::english::pack(words, "<s>\tthe\t40\n", 10).unwrap();
//! # #[allow(unsafe_code)]
//! // SAFETY: valid pointers from live Rust values; handles destroyed once.
//! unsafe {
//!     let english = mpn_english_open(image.as_ptr().cast(), image.len(), 1);
//!     let query = mpn_english_query_create();
//!     assert_eq!(mpn_english_complete(query, english, c"th".as_ptr(), c"".as_ptr(), 0), 2);
//!     assert_eq!(CStr::from_ptr(mpn_english_text(query, 0)).to_str(), Ok("the"));
//!     assert_eq!(mpn_english_kind(query, 1), MPN_ENGLISH_KIND_COMPLETION);
//!     assert_eq!(mpn_english_nine_key(query, english, c"65".as_ptr(), c"I said".as_ptr(), 0), 1);
//!     assert_eq!(CStr::from_ptr(mpn_english_text(query, 0)).to_str(), Ok("OK"));
//!     mpn_english_query_destroy(query);
//!     mpn_english_close(english);
//! }
//! ```

use std::ffi::{c_char, c_int, c_void};
use std::panic::{catch_unwind, AssertUnwindSafe};

use meowpuut_core::english::{English, Suggestions};

use crate::text::CArena;
use crate::{arg_str, EMPTY};

/// `flags`: the keyboard's shift is on (capital first letter).
pub const MPN_ENGLISH_SHIFT: u32 = 1;
/// `flags`: caps lock (all capitals).
pub const MPN_ENGLISH_CAPS_LOCK: u32 = 2;
/// `mpn_english_kind`: the typed letters or keys spell the word.
pub const MPN_ENGLISH_KIND_EXACT: c_int = 1;
/// `mpn_english_kind`: the word starts with the typed letters or keys.
pub const MPN_ENGLISH_KIND_COMPLETION: c_int = 2;
/// `mpn_english_kind`: the word is one or two edits away from the typed word.
pub const MPN_ENGLISH_KIND_CORRECTION: c_int = 3;
/// `mpn_english_kind`: a next word.
pub const MPN_ENGLISH_KIND_PREDICTION: c_int = 4;
/// `mpn_english_kind`: keypad only, the first letter of each key when they spell no word that may be offered.
pub const MPN_ENGLISH_KIND_LETTERS: c_int = 5;

/// Opaque English lexicon handle.
pub struct MPNEnglish {
    value: English,
}

/// Opaque result of one English call.
pub struct MPNEnglishQuery {
    core: Suggestions,
    texts: CArena,
}

/// Opens a MIMO-EN-1 image. `copy == 0`: the bytes are used in place. `copy != 0`: they are
/// copied. Returns NULL for NULL, a short or damaged buffer, or a wrong magic or version.
///
/// # Safety
/// `data` must be NULL or valid for reads of `size` bytes. With `copy == 0` the bytes must stay
/// mapped and unchanged until `mpn_english_close` has returned for this handle.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_english_open(data: *const c_void, size: usize, copy: c_int) -> *mut MPNEnglish {
    if data.is_null() || size == 0 || isize::try_from(size).is_err() {
        return std::ptr::null_mut();
    }
    // SAFETY: the caller guarantees `size` readable bytes at non-null `data`. With `copy == 0`
    // they stay valid and unchanged until the handle is closed, and the handle is the only owner
    // of this reference; with `copy != 0` the reference is dropped before this function returns.
    let bytes: &'static [u8] = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), size) };
    let opened = catch_unwind(|| if copy == 0 { English::in_place(bytes, None) } else { English::load(bytes) });
    match opened.ok().flatten() {
        Some(value) => Box::into_raw(Box::new(MPNEnglish { value })),
        None => std::ptr::null_mut(),
    }
}

/// Closes an image handle.
///
/// # Safety
/// `english` must be NULL or a live handle, closed at most once, with no call running on it.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_english_close(english: *mut MPNEnglish) {
    if !english.is_null() {
        // SAFETY: allocated by `Box::into_raw` in `mpn_english_open`, freed once.
        drop(unsafe { Box::from_raw(english) });
    }
}

/// Number of words in the image (0 for NULL).
///
/// # Safety
/// `english` must be NULL or a live handle.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_english_word_count(english: *const MPNEnglish) -> usize {
    // SAFETY: NULL or a live handle per the contract.
    unsafe { english.as_ref() }.map_or(0, |e| e.value.len())
}

/// 1 when `word` is a blocked (`NO_SUGGEST`) word of the list, in any casing;
/// 0 otherwise or for NULL. For filtering a platform's own spelling suggestions.
///
/// # Safety
/// `english` must be NULL or a live handle; `word` NULL or a NUL-terminated string.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_english_is_blocked(english: *const MPNEnglish, word: *const c_char) -> c_int {
    // SAFETY: NULL or a live handle per the contract.
    let Some(english) = (unsafe { english.as_ref() }) else { return 0 };
    // SAFETY: NULL or a NUL-terminated string per the contract.
    let Some(word) = (unsafe { arg_str(word) }) else { return 0 };
    catch_unwind(AssertUnwindSafe(|| english.value.is_blocked(word))).map_or(0, c_int::from)
}

/// Creates a query.
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn mpn_english_query_create() -> *mut MPNEnglishQuery {
    Box::into_raw(Box::new(MPNEnglishQuery { core: Suggestions::new(), texts: CArena::with_capacity(16, 2048) }))
}

/// Destroys a query.
///
/// # Safety
/// `query` must be NULL or a live handle, destroyed at most once.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_english_query_destroy(query: *mut MPNEnglishQuery) {
    if !query.is_null() {
        // SAFETY: allocated by `Box::into_raw` in `mpn_english_query_create`, freed once.
        drop(unsafe { Box::from_raw(query) });
    }
}

#[allow(unsafe_code)]
unsafe fn run(
    query: *mut MPNEnglishQuery,
    english: *const MPNEnglish,
    typed: *const c_char,
    before: *const c_char,
    call: impl FnOnce(&English, &str, &str, &mut Suggestions),
) -> usize {
    // SAFETY: NULL or a live handle used by this thread only, per the caller's contract.
    let Some(query) = (unsafe { query.as_mut() }) else { return 0 };
    query.texts.clear();
    // SAFETY: NULL or a live handle per the caller's contract.
    let Some(english) = (unsafe { english.as_ref() }) else { return 0 };
    // SAFETY: NULL or NUL-terminated strings per the caller's contract.
    let typed = unsafe { arg_str(typed) }.unwrap_or("");
    // SAFETY: as above.
    let before = unsafe { arg_str(before) }.unwrap_or("");
    let core = &mut query.core;
    if catch_unwind(AssertUnwindSafe(|| call(&english.value, typed, before, core))).is_err() {
        query.core = Suggestions::new();
        return 0;
    }
    for index in 0..query.core.len() {
        query.texts.push(query.core.text(index));
    }
    query.core.len()
}

/// Completions of the word being typed (`prefix`, as typed: its capitals set the casing).
/// `before` is the text before that word (NULL for none). Returns the count (at most 8).
///
/// # Safety
/// `query` must be NULL or a live handle used by this thread only; `english` NULL or live;
/// `prefix` and `before` NULL or NUL-terminated.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_english_complete(
    query: *mut MPNEnglishQuery,
    english: *const MPNEnglish,
    prefix: *const c_char,
    before: *const c_char,
    flags: u32,
) -> usize {
    // SAFETY: forwarded caller contract.
    unsafe { run(query, english, prefix, before, |e, t, b, out| e.complete(t, b, flags, out)) }
}

/// Corrections of a typed word that is not in the lexicon (one or two edits, QWERTY neighbours
/// first); 0 for a known word. Only offered: the keyboard replaces the word when one is tapped.
///
/// # Safety
/// As `mpn_english_complete`.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_english_correct(
    query: *mut MPNEnglishQuery,
    english: *const MPNEnglish,
    word: *const c_char,
    before: *const c_char,
    flags: u32,
) -> usize {
    // SAFETY: forwarded caller contract.
    unsafe { run(query, english, word, before, |e, t, b, out| e.correct(t, b, flags, out)) }
}

/// Next words for the text before the cursor (`before`, after a space): the followers of its
/// last word, the sentence starters (capitalised) at the start or after . ! ?.
///
/// # Safety
/// As `mpn_english_complete`.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_english_predict(
    query: *mut MPNEnglishQuery,
    english: *const MPNEnglish,
    before: *const c_char,
    flags: u32,
) -> usize {
    // SAFETY: forwarded caller contract.
    unsafe { run(query, english, before, std::ptr::null(), |e, t, _, out| e.predict(t, flags, out)) }
}

/// English words for keypad digits (`digits`: 2-9 only; 65 -> OK, 4663 -> good, home): the
/// words they spell, then up to three longer ones; a blocked word only when spelled, last.
///
/// # Safety
/// As `mpn_english_complete`.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_english_nine_key(
    query: *mut MPNEnglishQuery,
    english: *const MPNEnglish,
    digits: *const c_char,
    before: *const c_char,
    flags: u32,
) -> usize {
    // SAFETY: forwarded caller contract.
    unsafe { run(query, english, digits, before, |e, t, b, out| e.nine_key(t, b, flags, out)) }
}

/// Text of result `index` (`""` past the end); valid until the next call on this query.
///
/// # Safety
/// `query` must be NULL or a live handle.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_english_text(query: *const MPNEnglishQuery, index: usize) -> *const c_char {
    // SAFETY: NULL or a live handle per the contract.
    unsafe { query.as_ref() }.map_or(EMPTY.as_ptr(), |q| q.texts.get(index).as_ptr())
}

/// Kind of result `index` (`MPN_ENGLISH_KIND_*`; 0 past the end).
///
/// # Safety
/// `query` must be NULL or a live handle.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_english_kind(query: *const MPNEnglishQuery, index: usize) -> c_int {
    // SAFETY: NULL or a live handle per the contract.
    unsafe { query.as_ref() }.and_then(|q| q.core.kind(index)).map_or(0, |k| k as c_int)
}
