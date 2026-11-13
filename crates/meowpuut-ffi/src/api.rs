//! Exported `mpn_*` functions (see `include/meowpuut.h`).
//!
//! Every function tolerates NULL handles. Functions taking pointers are
//! `unsafe` for the caller: handles must come from the matching `*_create`
//! and must not be used after `*_destroy`.
//!
//! # Examples
//!
//! ```
//! use meowpuut_ffi::*;
//! use std::ffi::CStr;
//! let tsv = "MEOWPUUT-PINYIN-1\nmen\tmen5\t們\t们\t5000\nwo\two3\t我\t我\t5000\n# context\n我們\t9000\n";
//! # #[allow(unsafe_code)]
//! // SAFETY: valid pointers from live Rust values; handles destroyed once.
//! unsafe {
//!     let lex = mpn_lexicon_create(tsv.as_ptr().cast(), tsv.len());
//!     let s = mpn_session_create(lex);
//!     assert_eq!(mpn_predict(s, c"我".as_ptr(), 4), 1);
//!     assert_eq!(CStr::from_ptr(mpn_prediction(s, 0)).to_str(), Ok("們"));
//!     mpn_set_simplified(s, 1);
//!     mpn_predict(s, c"我".as_ptr(), 4);
//!     assert_eq!(CStr::from_ptr(mpn_prediction(s, 0)).to_str(), Ok("们"));
//!     assert_eq!(mpn_key(std::ptr::null_mut(), 'a' as i32), 0);
//!     mpn_session_destroy(s);
//!     mpn_lexicon_destroy(lex);
//! }
//! ```

use std::ffi::{c_char, c_int, c_void};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use meowpuut_core::image::ImageOwner;
use meowpuut_core::pairs::Pairs;
use meowpuut_core::{Lexicon, Session};

use crate::{arg_str, session_mut, session_ref, MPNLexicon, MPNSession, EMPTY};

/// Mirrors `MPNCandidateInfo` in the C header (32 bytes, no padding).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MPNCandidateInfo {
    /// `MPNCandidateKind`.
    pub kind: u32,
    /// `MPNCandidateScoreKind`.
    pub score_kind: u32,
    /// Raw input bytes consumed by selecting the candidate.
    pub consumed_bytes: u64,
    /// First raw input byte retained after selection.
    pub remaining_offset_bytes: u64,
    /// Generation score (not a probability).
    pub score: f64,
}

/// Parses a `MIMO-BIN-1` binary image or UTF-8 TSV lexicon; returns NULL for NULL/empty/invalid data.
///
/// # Safety
/// `data` must be NULL or valid for reads of `size` bytes.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_lexicon_create(data: *const c_char, size: usize) -> *mut MPNLexicon {
    if data.is_null() || size == 0 {
        return std::ptr::null_mut();
    }
    // SAFETY: caller guarantees `size` readable bytes at non-null `data`.
    let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), size) };
    let loaded = std::panic::catch_unwind(|| Lexicon::load_bytes(bytes)).ok().flatten();
    match loaded {
        Some(lexicon) => Box::into_raw(Box::new(MPNLexicon { value: Arc::new(lexicon) })),
        None => std::ptr::null_mut(),
    }
}

/// Loads either the Pinyin (`pinyin != 0`) or Zhuyin (`pinyin == 0`) lexicon slice
/// from a mapped `MIMO-BASE-1` (`mimo_base.bin`) container; returns NULL on failure.
///
/// # Safety
/// `data` must be NULL or valid for reads of `size` bytes.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_lexicon_create_from_base(
    data: *const c_char,
    size: usize,
    pinyin: c_int,
) -> *mut MPNLexicon {
    if data.is_null() || size == 0 {
        return std::ptr::null_mut();
    }
    // SAFETY: caller guarantees `size` readable bytes at non-null `data`.
    let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), size) };
    let loaded = std::panic::catch_unwind(|| Lexicon::load_from_base(bytes, pinyin != 0))
        .ok()
        .flatten();
    match loaded {
        Some(lexicon) => Box::into_raw(Box::new(MPNLexicon { value: Arc::new(lexicon) })),
        None => std::ptr::null_mut(),
    }
}

/// Called once with its context when the engine no longer reads the bytes
/// passed to `mpn_lexicon_create_from_base_in_place`.
pub type MPNReleaseFn = Option<unsafe extern "C" fn(context: *mut c_void)>;

/// Calls the release callback when dropped, which happens when the last
/// `Arc<Lexicon>` holding the in-place bytes goes away (or a load fails).
struct Release {
    release: MPNReleaseFn,
    context: *mut c_void,
}

// SAFETY: the header requires the release callback to accept a call from any
// thread; the context pointer is only handed back to it, never dereferenced.
#[allow(unsafe_code)]
unsafe impl Send for Release {}
// SAFETY: as above; `Release` has no methods that use the pointer through `&self`.
#[allow(unsafe_code)]
unsafe impl Sync for Release {}

impl Drop for Release {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        if let Some(release) = self.release {
            // SAFETY: the caller's function and context, called exactly once (drop runs once).
            unsafe { release(self.context) };
        }
    }
}

/// Loads Pinyin (`pinyin != 0`) or Zhuyin from a `MIMO-BASE-1` container
/// without copying it: the lexicon reads `data` in place. Returns NULL on failure.
///
/// The engine takes the bytes over on every call: `release(context)` runs
/// exactly once, when the lexicon handle and every session created from it
/// are destroyed, or before this function returns NULL. `release` may be NULL.
///
/// # Safety
/// `data` must be NULL or valid for reads of `size` bytes, and stay valid and
/// unchanged until `release(context)` is called (with a NULL `release`: until
/// the lexicon and all its sessions are destroyed). `release` must be safe to
/// call from any thread.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_lexicon_create_from_base_in_place(
    data: *const c_char,
    size: usize,
    pinyin: c_int,
    release: MPNReleaseFn,
    context: *mut c_void,
) -> *mut MPNLexicon {
    let guard = Release { release, context };
    if data.is_null() || size == 0 {
        return std::ptr::null_mut();
    }
    // SAFETY: the caller keeps `size` bytes at `data` valid and unchanged until
    // `release` runs. `guard` is stored in the lexicon as its owner, so release
    // runs only after the last reference to these bytes is dropped.
    let bytes: &'static [u8] = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), size) };
    let owner = ImageOwner::new(Arc::new(guard));
    let loaded = std::panic::catch_unwind(AssertUnwindSafe(|| Lexicon::from_base_in_place(bytes, pinyin != 0, Some(owner))))
        .ok()
        .flatten();
    match loaded {
        Some(lexicon) => Box::into_raw(Box::new(MPNLexicon { value: Arc::new(lexicon) })),
        None => std::ptr::null_mut(),
    }
}

/// Attaches a word-pair table (`mimo_pairs.bin`) to `lexicon`, read in place.
/// Returns 1 when attached; 0 when `data` is not a pair table for this
/// lexicon (another word list), or sessions already use the lexicon.
///
/// Ownership as for `mpn_lexicon_create_from_base_in_place`: `release(context)`
/// runs exactly once, when the lexicon and every session created from it are
/// destroyed, or before this function returns 0. `release` may be NULL.
///
/// # Safety
/// `lexicon` must be NULL or a live handle; `data` must be NULL or valid for
/// reads of `size` bytes and stay valid and unchanged until `release(context)`
/// is called. `release` must be safe to call from any thread.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_lexicon_attach_pairs(
    lexicon: *mut MPNLexicon,
    data: *const c_char,
    size: usize,
    release: MPNReleaseFn,
    context: *mut c_void,
) -> c_int {
    let guard = Release { release, context };
    // SAFETY: NULL or a live handle per the contract.
    let Some(handle) = (unsafe { lexicon.as_mut() }) else { return 0 };
    let Some(lex) = Arc::get_mut(&mut handle.value) else { return 0 };
    if data.is_null() || size == 0 {
        return 0;
    }
    // SAFETY: the caller keeps `size` bytes at `data` valid and unchanged until
    // `release` runs; `guard` is stored with the table, so release runs only
    // after the last reference to these bytes is dropped.
    let bytes: &'static [u8] = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), size) };
    let owner = ImageOwner::new(Arc::new(guard));
    let ids = &lex.index().word_ids;
    match std::panic::catch_unwind(AssertUnwindSafe(|| Pairs::in_place(bytes, ids, Some(owner)))).ok().flatten() {
        Some(pairs) => {
            lex.set_pairs(pairs);
            1
        }
        None => 0,
    }
}

/// Releases a lexicon handle (sessions keep their own reference).
///
/// # Safety
/// `lexicon` must be NULL or a live handle, destroyed at most once.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_lexicon_destroy(lexicon: *mut MPNLexicon) {
    if !lexicon.is_null() {
        // SAFETY: allocated by `Box::into_raw` in `mpn_lexicon_create`, freed once.
        drop(unsafe { Box::from_raw(lexicon) });
    }
}

/// Heap bytes held by a lexicon (0 for NULL); for memory diagnostics.
///
/// # Safety
/// `lexicon` must be NULL or a live handle.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_lexicon_heap_bytes(lexicon: *const MPNLexicon) -> usize {
    // SAFETY: NULL or a live handle per the contract.
    unsafe { lexicon.as_ref() }.map_or(0, |l| l.value.heap_bytes())
}

/// Creates a session over a lexicon; NULL for a NULL lexicon.
///
/// # Safety
/// `lexicon` must be NULL or a live handle.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_session_create(lexicon: *const MPNLexicon) -> *mut MPNSession {
    // SAFETY: NULL or a live handle per the contract.
    match unsafe { lexicon.as_ref() } {
        Some(l) => Box::into_raw(Box::new(MPNSession::new(Arc::clone(&l.value)))),
        None => std::ptr::null_mut(),
    }
}

/// Creates a nine-key session on a Pinyin lexicon, or returns NULL.
///
/// # Safety
/// `lexicon` must be NULL or a live handle.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_nine_key_session_create(lexicon: *const MPNLexicon) -> *mut MPNSession {
    // SAFETY: NULL or a live handle per the contract.
    unsafe { lexicon.as_ref() }
        .and_then(|l| {
            Session::new_nine_key(Arc::clone(&l.value))
                .map(|core| MPNSession::from_session(Arc::clone(&l.value), core))
        })
        .map_or(std::ptr::null_mut(), |session| Box::into_raw(Box::new(session)))
}

/// Destroys a session.
///
/// # Safety
/// `session` must be NULL or a live handle, destroyed at most once.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_session_destroy(session: *mut MPNSession) {
    if !session.is_null() {
        // SAFETY: allocated by `Box::into_raw` in `mpn_session_create`, freed once.
        drop(unsafe { Box::from_raw(session) });
    }
}

/// Maximum ASCII bytes in one composition.
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn mpn_max_input_bytes() -> usize {
    meowpuut_core::max_input_bytes()
}

macro_rules! session_fn {
    ($(#[$doc:meta])* fn $name:ident($s:ident $(, $arg:ident: $ty:ty)*) -> $ret:ty, $none:expr, $body:expr) => {
        $(#[$doc])*
        ///
        /// # Safety
        /// `session` must be NULL or a live handle; pointer arguments must be
        /// NULL or valid NUL-terminated strings.
        #[no_mangle]
        #[allow(unsafe_code)]
        pub unsafe extern "C" fn $name(session: *mut MPNSession $(, $arg: $ty)*) -> $ret {
            // SAFETY: forwarded caller contract.
            match unsafe { session_mut(session) } {
                Some($s) => $body,
                None => $none,
            }
        }
    };
}

macro_rules! getter_fn {
    ($(#[$doc:meta])* fn $name:ident($s:ident $(, $arg:ident: $ty:ty)*) -> $ret:ty, $none:expr, $body:expr) => {
        $(#[$doc])*
        ///
        /// # Safety
        /// `session` must be NULL or a live handle.
        #[no_mangle]
        #[allow(unsafe_code)]
        pub unsafe extern "C" fn $name(session: *const MPNSession $(, $arg: $ty)*) -> $ret {
            // SAFETY: forwarded caller contract.
            match unsafe { session_ref(session) } {
                Some($s) => $body,
                None => $none,
            }
        }
    };
}

session_fn!(/// Feeds one key (printable ASCII or X11 editing key); 1 if handled.
    fn mpn_key(s, key: i32) -> c_int, 0, c_int::from(s.mutate(|core| core.key(key))));
session_fn!(/// Switches simplified (non-zero) / traditional output.
    fn mpn_set_simplified(s, simplified: c_int) -> (), (), {
        s.simplified = simplified != 0;
        s.mutate(|core| core.set_simplified(simplified != 0))
    });
session_fn!(/// Clears composition and pending commit.
    fn mpn_clear(s) -> (), (), s.mutate(|core| core.clear()));
session_fn!(/// Commits the highlighted candidate (or raw input).
    fn mpn_commit(s) -> (), (), s.mutate(|core| core.commit()));
session_fn!(/// Selects candidate `index`; 1 on success.
    fn mpn_choose(s, index: usize) -> c_int, 0, c_int::from(s.mutate(|core| core.choose(index))));
session_fn!(/// Drops the pending committed text.
    fn mpn_clear_commit(s) -> (), (), s.mutate(|core| core.clear_commit()));
session_fn!(/// Sets the Chinese segment context (NULL/empty/invalid clears it).
    fn mpn_set_segment_context(s, text: *const c_char) -> (), (), {
        // SAFETY: forwarded caller contract for `text`.
        let text = unsafe { arg_str(text) }.unwrap_or("");
        s.mutate(|core| core.set_segment_context(text))
    });
session_fn!(/// Registers a Japanese word for mixed input; 1 if accepted.
    fn mpn_add_japanese_word(s, reading: *const c_char, text: *const c_char, weight: c_int) -> c_int, 0, {
        // SAFETY: forwarded caller contract for `reading`.
        let reading = unsafe { arg_str(reading) };
        // SAFETY: forwarded caller contract for `text`.
        let text = unsafe { arg_str(text) };
        match (reading, text) {
            (Some(r), Some(t)) => c_int::from(s.mutate(|core| core.add_japanese_word(r, t, weight))),
            _ => 0,
        }
    });
session_fn!(/// Mixed pinyin + Japanese sentence ("" if none); valid until the next call.
    fn mpn_mixed_pinyin(s, reading: *const c_char, simplified: c_int) -> *const c_char, EMPTY.as_ptr(), {
        // SAFETY: forwarded caller contract for `reading`.
        let reading = unsafe { arg_str(reading) };
        // A panic in the decoder yields "" instead of unwinding into the caller.
        let decoded = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let text = reading.map_or("", |r| s.core.mixed_pinyin(r, simplified != 0));
            s.mixed.set(text);
        }));
        if decoded.is_err() {
            s.mixed.set("");
        }
        s.mixed.as_cstr().as_ptr()
    });
session_fn!(/// Leading mixed segment and the ASCII bytes it consumes.
    fn mpn_mixed_pinyin_prefix(s, reading: *const c_char, simplified: c_int, consumed: *mut usize) -> *const c_char, {
        if !consumed.is_null() {
            // SAFETY: caller passes NULL or a writable `size_t`.
            unsafe { consumed.write(0) };
        }
        EMPTY.as_ptr()
    }, {
        // SAFETY: forwarded caller contract for `reading`.
        let reading = unsafe { arg_str(reading) };
        // A panic in the decoder yields "" and 0 instead of unwinding into the caller.
        let decoded = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let (text, used) = reading.map_or(("", 0), |r| s.core.mixed_pinyin_prefix(r, simplified != 0));
            s.mixed.set(text);
            used
        }));
        let used = decoded.unwrap_or_else(|_| {
            s.mixed.set("");
            0
        });
        if !consumed.is_null() {
            // SAFETY: caller passes NULL or a writable `size_t`.
            unsafe { consumed.write(used) };
        }
        s.mixed.as_cstr().as_ptr()
    });
session_fn!(/// Computes next-word associations for committed `context`; returns the count.
    fn mpn_predict(s, context: *const c_char, limit: usize) -> usize, 0, {
        // SAFETY: forwarded caller contract for `context`.
        let context = unsafe { arg_str(context) }.unwrap_or("");
        // A panic yields no associations instead of unwinding into the caller.
        let n = std::panic::catch_unwind(AssertUnwindSafe(|| s.predictor.predict(&s.lexicon, context, s.simplified, limit)))
            .unwrap_or(0);
        s.predictions.clear();
        for i in 0..n {
            s.predictions.push(s.predictor.text(i));
        }
        n
    });

getter_fn!(/// Raw ASCII input.
    fn mpn_input(s) -> *const c_char, EMPTY.as_ptr(), s.input.as_cstr().as_ptr());
getter_fn!(/// Display preedit (Zhuyin symbols or pinyin letters).
    fn mpn_preedit(s) -> *const c_char, EMPTY.as_ptr(), s.preedit.as_cstr().as_ptr());
getter_fn!(/// Text committed by the last action.
    fn mpn_committed(s) -> *const c_char, EMPTY.as_ptr(), s.committed.as_cstr().as_ptr());
getter_fn!(/// Current segment context.
    fn mpn_segment_context(s) -> *const c_char, EMPTY.as_ptr(), s.context.as_cstr().as_ptr());
getter_fn!(/// Number of candidates.
    fn mpn_candidate_count(s) -> usize, 0, s.candidates.len());
getter_fn!(/// Candidate text ("" when out of range).
    fn mpn_candidate(s, index: usize) -> *const c_char, EMPTY.as_ptr(), s.candidates.get(index).as_ptr());
getter_fn!(/// Raw bytes consumed by selecting candidate `index` (0 when invalid).
    fn mpn_candidate_consumed(s, index: usize) -> usize, 0, s.core.candidate_consumed(index));
getter_fn!(/// Current candidate page.
    fn mpn_page(s) -> usize, 0, s.core.page());
getter_fn!(/// Association `index` from the last `mpn_predict` ("" when out of range).
    fn mpn_prediction(s, index: usize) -> *const c_char, EMPTY.as_ptr(), s.predictions.get(index).as_ptr());

/// Writes up to `info_size` bytes of candidate metadata; 0 for invalid arguments.
///
/// # Safety
/// `session` must be NULL or live; `info` must be NULL or writable for `info_size` bytes.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mpn_candidate_info(
    session: *const MPNSession,
    index: usize,
    info: *mut MPNCandidateInfo,
    info_size: usize,
) -> c_int {
    // SAFETY: forwarded caller contract.
    let Some(s) = (unsafe { session_ref(session) }) else { return 0 };
    if info.is_null() || info_size == 0 {
        return 0;
    }
    let Some(c) = s.core.candidate_info(index) else { return 0 };
    let value = MPNCandidateInfo {
        kind: c.kind,
        score_kind: c.score_kind,
        consumed_bytes: c.consumed_bytes,
        remaining_offset_bytes: c.remaining_offset_bytes,
        score: c.score,
    };
    let mut bytes = [0u8; std::mem::size_of::<MPNCandidateInfo>()];
    bytes[0..4].copy_from_slice(&value.kind.to_ne_bytes());
    bytes[4..8].copy_from_slice(&value.score_kind.to_ne_bytes());
    bytes[8..16].copy_from_slice(&value.consumed_bytes.to_ne_bytes());
    bytes[16..24].copy_from_slice(&value.remaining_offset_bytes.to_ne_bytes());
    bytes[24..32].copy_from_slice(&value.score.to_ne_bytes());
    let n = info_size.min(bytes.len());
    // SAFETY: caller guarantees `info_size` writable bytes; `n <= info_size`, no overlap.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), info.cast::<u8>(), n) };
    1
}
