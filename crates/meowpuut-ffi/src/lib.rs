//! C ABI for the Meowpuut engine: a drop-in replacement for the former C++
//! `MeowpuutCore` (`include/meowpuut.h`).
//!
//! This is the **only** crate in the workspace where `unsafe` is allowed. It is
//! denied crate-wide and re-allowed per item; each unsafe block holds exactly
//! one operation and a `SAFETY` comment (enforced by clippy lints). All engine
//! logic lives in `#![forbid(unsafe_code)]` crates.
//!
//! Contract (unchanged from C++): a session is used on one thread; returned
//! strings stay valid until the next mutating call on the same session; NULL
//! handles are tolerated and yield empty results.
//!
//! # Examples
//!
//! ```
//! use meowpuut_ffi::*;
//! let tsv = "MEOWPUUT-PINYIN-1\nni\tni3\t你\t你\t5000\n";
//! # #[allow(unsafe_code)]
//! // SAFETY: valid pointers from live Rust values; handles destroyed once.
//! unsafe {
//!     let lex = mpn_lexicon_create(tsv.as_ptr().cast(), tsv.len());
//!     let s = mpn_session_create(lex);
//!     mpn_key(s, 'n' as i32);
//!     mpn_key(s, 'i' as i32);
//!     assert_eq!(mpn_candidate_count(s), 1);
//!     assert_eq!(std::ffi::CStr::from_ptr(mpn_candidate(s, 0)).to_str(), Ok("你"));
//!     mpn_session_destroy(s);
//!     mpn_lexicon_destroy(lex);
//! }
//! ```

pub mod api;
pub mod japanese;
pub mod text;

pub use api::*;
pub use japanese::*;

use std::ffi::{c_char, CStr};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

use meowpuut_core::predict::Predictor;
use meowpuut_core::{Lexicon, Session};

use crate::text::{CArena, CBuf};

/// Opaque immutable lexicon handle (shared by sessions through `Arc`).
pub struct MPNLexicon {
    pub(crate) value: Arc<Lexicon>,
}

/// Opaque session handle: the engine plus NUL-terminated mirrors of every
/// string the C ABI hands out.
pub struct MPNSession {
    pub(crate) core: Session,
    pub(crate) lexicon: Arc<Lexicon>,
    pub(crate) predictor: Predictor,
    pub(crate) input: CBuf,
    pub(crate) preedit: CBuf,
    pub(crate) committed: CBuf,
    pub(crate) context: CBuf,
    pub(crate) mixed: CBuf,
    pub(crate) candidates: CArena,
    pub(crate) predictions: CArena,
    pub(crate) simplified: bool,
}

impl MPNSession {
    pub(crate) fn new(lexicon: Arc<Lexicon>) -> Self {
        Self::from_session(Arc::clone(&lexicon), Session::new(lexicon))
    }

    pub(crate) fn from_session(lexicon: Arc<Lexicon>, core: Session) -> Self {
        let mut session = Self {
            core,
            lexicon,
            predictor: Predictor::new(),
            input: CBuf::with_capacity(512),
            preedit: CBuf::with_capacity(1024),
            committed: CBuf::with_capacity(1024),
            context: CBuf::with_capacity(128),
            mixed: CBuf::with_capacity(512),
            candidates: CArena::with_capacity(320, 16 * 1024),
            predictions: CArena::with_capacity(64, 1024),
            simplified: false,
        };
        session.sync();
        session
    }

    /// Refreshes every C mirror after a mutation (allocation-free once warm).
    pub(crate) fn sync(&mut self) {
        self.input.set(self.core.input());
        self.preedit.set(self.core.preedit());
        self.committed.set(self.core.committed());
        self.context.set(self.core.segment_context());
        self.candidates.clear();
        for i in 0..self.core.candidate_count() {
            self.candidates.push(self.core.candidate(i));
        }
    }

    /// Runs a mutation; on an (unexpected) engine panic the composition is
    /// reset instead of unwinding into C, mirroring the C++ `catch (...)`.
    pub(crate) fn mutate<R: Default>(&mut self, f: impl FnOnce(&mut Session) -> R) -> R {
        let result = catch_unwind(AssertUnwindSafe(|| f(&mut self.core)));
        let value = result.unwrap_or_else(|_| {
            self.core.reset();
            R::default()
        });
        self.sync();
        value
    }
}

/// Static empty C string returned for NULL handles.
pub(crate) const EMPTY: &CStr = c"";

/// Borrows a NUL-terminated UTF-8 argument; NULL or invalid UTF-8 → `None`.
///
/// # Safety
/// `ptr` must be NULL or point to a NUL-terminated string valid for `'a`.
#[allow(unsafe_code)]
pub(crate) unsafe fn arg_str<'a>(ptr: *const c_char) -> Option<&'a str> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: non-null and NUL-terminated per this function's contract.
    let cstr = unsafe { CStr::from_ptr(ptr) };
    cstr.to_str().ok()
}

/// Mutable session from a C handle.
///
/// # Safety
/// `ptr` must be NULL or a live handle from `mpn_session_create`, not aliased.
#[allow(unsafe_code)]
pub(crate) unsafe fn session_mut<'a>(ptr: *mut MPNSession) -> Option<&'a mut MPNSession> {
    // SAFETY: NULL or a unique live Box allocation per the contract.
    unsafe { ptr.as_mut() }
}

/// Shared session from a C handle.
///
/// # Safety
/// `ptr` must be NULL or a live handle from `mpn_session_create`.
#[allow(unsafe_code)]
pub(crate) unsafe fn session_ref<'a>(ptr: *const MPNSession) -> Option<&'a MPNSession> {
    // SAFETY: NULL or a live Box allocation per the contract.
    unsafe { ptr.as_ref() }
}
