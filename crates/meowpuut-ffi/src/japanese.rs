//! Exported `mpn_japanese_*` functions: the Japanese lexicon image and its lookup
//! (see the last block of `include/meowpuut.h`).
//!
//! An `MPNJapanese` is read-only and may be shared by any number of threads and queries. An
//! `MPNJapaneseQuery` holds the result of one lookup and is used on one thread. NULL handles
//! give 0, -1 or `""`.
//!
//! # Examples
//!
//! ```
//! use meowpuut_ffi::*;
//! use std::ffi::CStr;
//! // An image without any word: the 64-byte header alone.
//! let mut image = b"MIMO-JP-1\0\0\0".to_vec();
//! for field in [1u32, 0, 0, 0, 0, 64, 64, 64, 64, 64, 0, 64, 0] {
//!     image.extend_from_slice(&field.to_le_bytes());
//! }
//! # #[allow(unsafe_code)]
//! // SAFETY: valid pointers from live Rust values; handles destroyed once.
//! unsafe {
//!     let japanese = mpn_japanese_open(image.as_ptr().cast(), image.len(), 1);
//!     assert!(!japanese.is_null());
//!     let query = mpn_japanese_query_create();
//!     assert_eq!(mpn_japanese_lookup(query, japanese, c"neko".as_ptr(), MPN_JP_MODE_JAPANESE, 64), 0);
//!     assert_eq!(CStr::from_ptr(mpn_japanese_kana(query)).to_str(), Ok("ねこ"));
//!     assert_eq!(CStr::from_ptr(mpn_japanese_katakana(query)).to_str(), Ok("ネコ"));
//!     mpn_japanese_query_destroy(query);
//!     mpn_japanese_close(japanese);
//! }
//! ```

use std::ffi::{c_char, c_int, c_void};
use std::panic::{catch_unwind, AssertUnwindSafe};

use meowpuut_core::japanese::{JapaneseLexicon, JapaneseMode, JapaneseQuery, REVIEWED_POOL};

use crate::text::{CArena, CBuf};
use crate::{arg_str, session_mut, MPNSession, EMPTY};

/// `mode` of `mpn_japanese_lookup`: Japanese mode.
pub const MPN_JP_MODE_JAPANESE: c_int = 0;
/// `mode` of `mpn_japanese_lookup`: pinyin mode.
pub const MPN_JP_MODE_PINYIN: c_int = 1;

/// `MPNJapaneseInfo.kind`: kana reading.
pub const MPN_JP_KIND_READING: u32 = 0;
/// `MPNJapaneseInfo.kind`: kana of a sloppy spelling.
pub const MPN_JP_KIND_VARIANT: u32 = 1;
/// `MPNJapaneseInfo.kind`: pinyin of the kanji plus Hepburn of the kana.
pub const MPN_JP_KIND_ALIAS_KANA: u32 = 2;
/// `MPNJapaneseInfo.kind`: pinyin of the kanji.
pub const MPN_JP_KIND_ALIAS_KANJI: u32 = 3;
/// `MPNJapaneseInfo.kind`: key written by hand in the reviewed lists.
pub const MPN_JP_KIND_AUTHORED: u32 = 4;

/// `MPNJapaneseInfo.flags`: may be offered in pinyin mode.
pub const MPN_JP_FLAG_PINYIN_OK: u32 = 1;
/// `MPNJapaneseInfo.flags`: reviewed word.
pub const MPN_JP_FLAG_REVIEWED: u32 = 2;
/// `MPNJapaneseInfo.flags`: generated inflected form.
pub const MPN_JP_FLAG_FORM: u32 = 4;

/// Reviewed-row flag: the row is registered for mixed sentences.
pub const MPN_JP_REVIEWED_POOL: u32 = 1;

/// Opaque handle of an opened image.
pub struct MPNJapanese {
    pub(crate) value: JapaneseLexicon,
}

/// Opaque query handle: the lookup buffers plus NUL-terminated copies of the strings handed out.
pub struct MPNJapaneseQuery {
    core: JapaneseQuery,
    texts: CArena,
    kana: CBuf,
    katakana: CBuf,
}

impl MPNJapaneseQuery {
    fn clear(&mut self) {
        self.texts.clear();
        self.kana.set("");
        self.katakana.set("");
    }
}

/// Mirrors `MPNJapaneseInfo` in the C header (16 bytes, no padding).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MPNJapaneseInfo {
    /// `MPN_JP_KIND_*`.
    pub kind: u32,
    /// `MPN_JP_FLAG_*`.
    pub flags: u32,
    /// Posting weight, 1..=10000.
    pub weight: i32,
    /// Index of the word in the image; equal for the same word under two keys.
    pub word: u32,
}

/// Shared image from a C handle.
///
/// # Safety
/// `ptr` must be NULL or a live handle from `mpn_japanese_open`.
#[allow(unsafe_code)]
unsafe fn japanese_ref<'a>(ptr: *const MPNJapanese) -> Option<&'a MPNJapanese> {
    // SAFETY: NULL or a live Box allocation per the contract.
    unsafe { ptr.as_ref() }
}

/// Mutable query from a C handle.
///
/// # Safety
/// `ptr` must be NULL or a live handle from `mpn_japanese_query_create`, not aliased.
#[allow(unsafe_code)]
unsafe fn query_mut<'a>(ptr: *mut MPNJapaneseQuery) -> Option<&'a mut MPNJapaneseQuery> {
    // SAFETY: NULL or a unique live Box allocation per the contract.
    unsafe { ptr.as_mut() }
}

/// Shared query from a C handle.
///
/// # Safety
/// `ptr` must be NULL or a live handle from `mpn_japanese_query_create`.
#[allow(unsafe_code)]
unsafe fn query_ref<'a>(ptr: *const MPNJapaneseQuery) -> Option<&'a MPNJapaneseQuery> {
    // SAFETY: NULL or a live Box allocation per the contract.
    unsafe { ptr.as_ref() }
}

/// Opens a `MIMO-JP-1` image. `copy == 0`: the bytes are used in place. `copy != 0`: the bytes
/// are copied. Returns NULL for NULL, a short buffer, a wrong magic or version, a `file_len`
/// other than `size`, or sections outside the buffer.
///
/// # Safety
/// `data` must be NULL or valid for reads of `size` bytes. With `copy == 0` the bytes must stay
/// mapped and unchanged until `mpn_japanese_close` has returned for this handle.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_japanese_open(data: *const c_void, size: usize, copy: c_int) -> *mut MPNJapanese {
    if data.is_null() || size == 0 || isize::try_from(size).is_err() {
        return std::ptr::null_mut();
    }
    // SAFETY: the caller guarantees `size` readable bytes at non-null `data`. With `copy == 0`
    // the caller also guarantees that they stay valid and unchanged until the handle is closed;
    // the handle is the only owner of this reference, so `'static` does not outlive that
    // promise. With `copy != 0` the reference is dropped before this function returns.
    let bytes: &'static [u8] = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), size) };
    let opened = catch_unwind(|| {
        if copy == 0 {
            JapaneseLexicon::from_static(bytes)
        } else {
            JapaneseLexicon::from_vec(bytes.to_vec())
        }
    });
    match opened.ok().flatten() {
        Some(value) => Box::into_raw(Box::new(MPNJapanese { value })),
        None => std::ptr::null_mut(),
    }
}

/// Closes an image handle. Queries keep nothing of the image; the mapped bytes of a
/// `copy == 0` handle may be released after this call.
///
/// # Safety
/// `japanese` must be NULL or a live handle, closed at most once, with no lookup running on it.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_japanese_close(japanese: *mut MPNJapanese) {
    if !japanese.is_null() {
        // SAFETY: allocated by `Box::into_raw` in `mpn_japanese_open`, freed once.
        drop(unsafe { Box::from_raw(japanese) });
    }
}

/// Number of words in the image (0 for NULL).
///
/// # Safety
/// `japanese` must be NULL or a live handle.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_japanese_word_count(japanese: *const MPNJapanese) -> usize {
    // SAFETY: forwarded caller contract.
    unsafe { japanese_ref(japanese) }.map_or(0, |j| j.value.word_count())
}

/// First four bytes of the sha256 of the Chinese lexicon the image was compiled with (0 for NULL).
///
/// # Safety
/// `japanese` must be NULL or a live handle.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_japanese_chinese_build(japanese: *const MPNJapanese) -> u32 {
    // SAFETY: forwarded caller contract.
    unsafe { japanese_ref(japanese) }.map_or(0, |j| j.value.chinese_build())
}

/// Number of reviewed rows (0 for NULL).
///
/// # Safety
/// `japanese` must be NULL or a live handle.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_japanese_reviewed_count(japanese: *const MPNJapanese) -> usize {
    // SAFETY: forwarded caller contract.
    unsafe { japanese_ref(japanese) }.map_or(0, |j| j.value.reviewed_count())
}

/// Copies `text` and a NUL into `buffer`.
///
/// # Safety
/// `buffer` must be writable for `text.len() + 1` bytes.
#[allow(unsafe_code)]
unsafe fn write_c_string(buffer: *mut c_char, text: &str) {
    let out = buffer.cast::<u8>();
    // SAFETY: `text.len()` bytes are writable per the contract; a Rust string and a caller
    // buffer do not overlap.
    unsafe { std::ptr::copy_nonoverlapping(text.as_ptr(), out, text.len()) };
    // SAFETY: `text.len()` is inside the buffer per the contract.
    let end = unsafe { out.add(text.len()) };
    // SAFETY: the byte behind the text is writable per the contract.
    unsafe { end.write(0) };
}

/// Copies reviewed row `index` into the caller's buffers (NUL-terminated). Returns 0 and writes
/// nothing when the index is out of range, a buffer is NULL or a buffer is too small.
/// `weight` and `flags` may be NULL.
///
/// # Safety
/// `japanese` must be NULL or a live handle. `reading` and `text` must be NULL or writable for
/// `reading_size` and `text_size` bytes. `weight` and `flags` must be NULL or writable.
#[no_mangle]
#[allow(unsafe_code)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn mpn_japanese_reviewed_get(
    japanese: *const MPNJapanese,
    index: usize,
    reading: *mut c_char,
    reading_size: usize,
    text: *mut c_char,
    text_size: usize,
    weight: *mut i32,
    flags: *mut u32,
) -> c_int {
    // SAFETY: forwarded caller contract.
    let Some(japanese) = (unsafe { japanese_ref(japanese) }) else { return 0 };
    if reading.is_null() || text.is_null() {
        return 0;
    }
    let Some((row_reading, row_text, row_weight, row_flags)) = japanese.value.reviewed_row(index) else { return 0 };
    if row_reading.len() >= reading_size || row_text.len() >= text_size {
        return 0;
    }
    // SAFETY: `reading` is writable for `reading_size` bytes, and the reading plus its NUL fit.
    unsafe { write_c_string(reading, row_reading) };
    // SAFETY: `text` is writable for `text_size` bytes, and the text plus its NUL fit.
    unsafe { write_c_string(text, row_text) };
    if !weight.is_null() {
        // SAFETY: the caller passes NULL or a writable `int32_t`.
        unsafe { weight.write(row_weight) };
    }
    if !flags.is_null() {
        // SAFETY: the caller passes NULL or a writable `uint32_t`.
        unsafe { flags.write(row_flags) };
    }
    1
}

/// Index of the reviewed row with exactly this reading, or -1.
///
/// # Safety
/// `japanese` must be NULL or a live handle; `reading` must be NULL or a NUL-terminated string.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_japanese_reviewed_find(japanese: *const MPNJapanese, reading: *const c_char) -> i32 {
    // SAFETY: forwarded caller contract.
    let Some(japanese) = (unsafe { japanese_ref(japanese) }) else { return -1 };
    // SAFETY: forwarded caller contract for `reading`.
    let Some(reading) = (unsafe { arg_str(reading) }) else { return -1 };
    japanese.value.reviewed_find(reading).and_then(|index| i32::try_from(index).ok()).unwrap_or(-1)
}

/// Registers every `MPN_JP_REVIEWED_POOL` row with the session through the rule of
/// `mpn_add_japanese_word`. Returns the number accepted.
///
/// # Safety
/// `session` and `japanese` must each be NULL or a live handle.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_session_add_japanese_mixed(
    session: *mut MPNSession,
    japanese: *const MPNJapanese,
) -> usize {
    // SAFETY: forwarded caller contract.
    let Some(session) = (unsafe { session_mut(session) }) else { return 0 };
    // SAFETY: forwarded caller contract.
    let Some(japanese) = (unsafe { japanese_ref(japanese) }) else { return 0 };
    let lexicon = &japanese.value;
    session.mutate(|core| {
        let mut accepted = 0;
        for index in 0..lexicon.reviewed_count() {
            let Some((reading, text, weight, flags)) = lexicon.reviewed_row(index) else { continue };
            if flags & REVIEWED_POOL != 0 && core.add_japanese_word(reading, text, weight) {
                accepted += 1;
            }
        }
        accepted
    })
}

/// Creates a query.
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn mpn_japanese_query_create() -> *mut MPNJapaneseQuery {
    Box::into_raw(Box::new(MPNJapaneseQuery {
        core: JapaneseQuery::new(),
        texts: CArena::with_capacity(64, 2048),
        kana: CBuf::with_capacity(256),
        katakana: CBuf::with_capacity(256),
    }))
}

/// Destroys a query.
///
/// # Safety
/// `query` must be NULL or a live handle, destroyed at most once.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_japanese_query_destroy(query: *mut MPNJapaneseQuery) {
    if !query.is_null() {
        // SAFETY: allocated by `Box::into_raw` in `mpn_japanese_query_create`, freed once.
        drop(unsafe { Box::from_raw(query) });
    }
}

/// Looks up the whole input. Returns the number of words (at most `limit`); 0 for a NULL
/// handle, NULL or non-UTF-8 input, or an unknown mode. Replaces the previous result of the query.
///
/// # Safety
/// `query` must be NULL or a live handle used by this thread only; `japanese` must be NULL or a
/// live handle; `input` must be NULL or a NUL-terminated string.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_japanese_lookup(
    query: *mut MPNJapaneseQuery,
    japanese: *const MPNJapanese,
    input: *const c_char,
    mode: c_int,
    limit: usize,
) -> usize {
    // SAFETY: forwarded caller contract.
    let Some(query) = (unsafe { query_mut(query) }) else { return 0 };
    query.clear();
    // SAFETY: forwarded caller contract.
    let Some(japanese) = (unsafe { japanese_ref(japanese) }) else { return 0 };
    // SAFETY: forwarded caller contract for `input`.
    let Some(input) = (unsafe { arg_str(input) }) else { return 0 };
    let mode = match mode {
        MPN_JP_MODE_JAPANESE => JapaneseMode::Japanese,
        MPN_JP_MODE_PINYIN => JapaneseMode::Pinyin,
        _ => return 0,
    };
    let core = &mut query.core;
    let found = catch_unwind(AssertUnwindSafe(|| core.lookup(&japanese.value, input, mode, limit)));
    let Ok(count) = found else {
        // Not expected. The query buffers may be half written, so start from new ones.
        query.core = JapaneseQuery::new();
        return 0;
    };
    for index in 0..count {
        query.texts.push(query.core.text(index));
    }
    query.kana.set(query.core.kana());
    query.katakana.set(query.core.katakana());
    count
}

/// Written form of result `index` (`""` when out of range); valid until the next lookup on
/// this query.
///
/// # Safety
/// `query` must be NULL or a live handle.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_japanese_text(query: *const MPNJapaneseQuery, index: usize) -> *const c_char {
    // SAFETY: forwarded caller contract.
    unsafe { query_ref(query) }.map_or(EMPTY.as_ptr(), |q| q.texts.get(index).as_ptr())
}

/// Writes up to `info_size` bytes of the metadata of result `index`; 0 for invalid arguments.
///
/// # Safety
/// `query` must be NULL or live; `info` must be NULL or writable for `info_size` bytes.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_japanese_info(
    query: *const MPNJapaneseQuery,
    index: usize,
    info: *mut MPNJapaneseInfo,
    info_size: usize,
) -> c_int {
    // SAFETY: forwarded caller contract.
    let Some(query) = (unsafe { query_ref(query) }) else { return 0 };
    if info.is_null() || info_size == 0 || index >= query.texts.len() {
        return 0;
    }
    let Some(row) = query.core.row(index) else { return 0 };
    let mut bytes = [0u8; std::mem::size_of::<MPNJapaneseInfo>()];
    bytes[0..4].copy_from_slice(&u32::from(row.kind).to_ne_bytes());
    bytes[4..8].copy_from_slice(&u32::from(row.flags).to_ne_bytes());
    bytes[8..12].copy_from_slice(&row.weight.to_ne_bytes());
    bytes[12..16].copy_from_slice(&row.word.to_ne_bytes());
    let n = info_size.min(bytes.len());
    // SAFETY: caller guarantees `info_size` writable bytes; `n <= info_size`, no overlap.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), info.cast::<u8>(), n) };
    1
}

/// Hiragana of the whole input of the last lookup; `""` when the input is not a complete
/// romaji string. Valid until the next lookup on this query.
///
/// # Safety
/// `query` must be NULL or a live handle.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_japanese_kana(query: *const MPNJapaneseQuery) -> *const c_char {
    // SAFETY: forwarded caller contract.
    unsafe { query_ref(query) }.map_or(EMPTY.as_ptr(), |q| q.kana.as_cstr().as_ptr())
}

/// Katakana of the whole input of the last lookup; `""` when the input is not a complete
/// romaji string. Valid until the next lookup on this query.
///
/// # Safety
/// `query` must be NULL or a live handle.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_japanese_katakana(query: *const MPNJapaneseQuery) -> *const c_char {
    // SAFETY: forwarded caller contract.
    unsafe { query_ref(query) }.map_or(EMPTY.as_ptr(), |q| q.katakana.as_cstr().as_ptr())
}
