//! C interface of the Japanese next-word image `mimo_jp_next.bin` (format `MIMO-JN-1`) and its
//! prediction: what may follow the text that was just committed.
//!
//! The byte layout is described in `meowpuut_core::japanese_next`. Every function checks its
//! arguments: a NULL handle, a NULL or non-UTF-8 string and an index out of range give 0, NULL
//! or `""`, and no function unwinds into the caller.
//!
//! # Examples
//!
//! ```
//! use meowpuut_ffi::*;
//! use std::ffi::CStr;
//!
//! // An image without any key: the header alone.
//! let mut image = Vec::new();
//! image.extend_from_slice(b"MIMO-JN-1\0\0\0");
//! for field in [1u32, 0, 0, 48, 48, 48, 0, 48, 0] {
//!     image.extend_from_slice(&field.to_le_bytes());
//! }
//! unsafe {
//!     let next = mpn_japanese_next_open(image.as_ptr().cast(), image.len(), 1);
//!     assert!(!next.is_null());
//!     let query = mpn_japanese_next_query_create();
//!     assert_eq!(mpn_japanese_next_predict(query, next, c"今日".as_ptr(), 8), 0);
//!     assert_eq!(CStr::from_ptr(mpn_japanese_next_text(query, 0)).to_bytes(), b"");
//!     mpn_japanese_next_query_destroy(query);
//!     mpn_japanese_next_close(next);
//! }
//! ```

use std::ffi::{c_char, c_int, c_void};
use std::panic::{catch_unwind, AssertUnwindSafe};

use meowpuut_core::japanese_next::{JapaneseNext, NextQuery};

use crate::text::CArena;
use crate::{arg_str, EMPTY};

/// Opaque handle of an opened next-word image.
pub struct MPNJapaneseNext {
    value: JapaneseNext,
}

/// Opaque handle: the buffers of one prediction plus NUL-terminated copies of its words.
pub struct MPNJapaneseNextQuery {
    core: NextQuery,
    texts: CArena,
}

/// Opens a `MIMO-JN-1` image. `copy == 0`: the bytes are used in place. `copy != 0`: the bytes
/// are copied. Returns NULL for NULL, a short buffer, a wrong magic or version, a `file_len`
/// other than `size`, or sections outside the buffer.
///
/// # Safety
/// `data` must be NULL or valid for reads of `size` bytes. With `copy == 0` the bytes must stay
/// mapped and unchanged until `mpn_japanese_next_close` has returned for this handle.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_japanese_next_open(data: *const c_void, size: usize, copy: c_int) -> *mut MPNJapaneseNext {
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
            JapaneseNext::from_static(bytes)
        } else {
            JapaneseNext::from_vec(bytes.to_vec())
        }
    });
    match opened.ok().flatten() {
        Some(value) => Box::into_raw(Box::new(MPNJapaneseNext { value })),
        None => std::ptr::null_mut(),
    }
}

/// Closes a handle from `mpn_japanese_next_open`. NULL is a no-op.
///
/// # Safety
/// `next` must be NULL or a live handle that no other thread is using; it is invalid afterwards.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_japanese_next_close(next: *mut MPNJapaneseNext) {
    if !next.is_null() {
        // SAFETY: allocated by `Box::into_raw` in `mpn_japanese_next_open`, freed once.
        drop(unsafe { Box::from_raw(next) });
    }
}

/// Number of keys of the image; 0 for NULL.
///
/// # Safety
/// `next` must be NULL or a live handle.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_japanese_next_key_count(next: *const MPNJapaneseNext) -> usize {
    // SAFETY: NULL or a live Box allocation per the contract.
    unsafe { next.as_ref() }.map_or(0, |next| next.value.key_count())
}

/// Makes a query. Use one query on one thread.
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn mpn_japanese_next_query_create() -> *mut MPNJapaneseNextQuery {
    catch_unwind(|| {
        Box::into_raw(Box::new(MPNJapaneseNextQuery { core: NextQuery::new(), texts: CArena::with_capacity(32, 1024) }))
    })
    .unwrap_or(std::ptr::null_mut())
}

/// Frees a query. NULL is a no-op.
///
/// # Safety
/// `query` must be NULL or a live handle from `mpn_japanese_next_query_create`; it is invalid afterwards.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_japanese_next_query_destroy(query: *mut MPNJapaneseNextQuery) {
    if !query.is_null() {
        // SAFETY: allocated by `Box::into_raw` in `mpn_japanese_next_query_create`, freed once.
        drop(unsafe { Box::from_raw(query) });
    }
}

/// Finds up to `limit` (at most 32) words that may follow `context` and returns their number;
/// 0 for a NULL handle or a NULL or non-UTF-8 context. `context` is the committed pieces, oldest
/// first, separated by U+001F; only the last two are read. Replaces the previous result of the query.
///
/// # Safety
/// `query` must be NULL or a live handle used by this thread only; `next` must be NULL or a live
/// handle; `context` must be NULL or a NUL-terminated string.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_japanese_next_predict(
    query: *mut MPNJapaneseNextQuery,
    next: *const MPNJapaneseNext,
    context: *const c_char,
    limit: usize,
) -> usize {
    // SAFETY: NULL or a unique live Box allocation per the contract.
    let Some(query) = (unsafe { query.as_mut() }) else { return 0 };
    query.texts.clear();
    // SAFETY: NULL or a live Box allocation per the contract.
    let Some(next) = (unsafe { next.as_ref() }) else { return 0 };
    // SAFETY: forwarded caller contract for `context`.
    let Some(context) = (unsafe { arg_str(context) }) else { return 0 };
    let core = &mut query.core;
    let found = catch_unwind(AssertUnwindSafe(|| core.predict(&next.value, context, limit)));
    let Ok(count) = found else {
        // Not expected. The buffers may be half written, so start from new ones.
        query.core = NextQuery::new();
        return 0;
    };
    for index in 0..count {
        query.texts.push(query.core.text(index));
    }
    count
}

/// Word `index` of the last prediction (`""` when out of range); valid until the next
/// prediction on this query.
///
/// # Safety
/// `query` must be NULL or a live handle.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_japanese_next_text(query: *const MPNJapaneseNextQuery, index: usize) -> *const c_char {
    // SAFETY: NULL or a live Box allocation per the contract.
    unsafe { query.as_ref() }.map_or(EMPTY.as_ptr(), |q| q.texts.get(index).as_ptr())
}
