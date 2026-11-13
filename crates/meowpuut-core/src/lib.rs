#![forbid(unsafe_code)]
//! `meowpuut-core` — Cache-friendly Structure-of-Arrays (SoA) decoding engine
//! with zero heap allocations in the hot simulation/decoding loop.
//!
//! # Examples
//!
//! ```
//! use std::sync::Arc;
//! use meowpuut_core::{Lexicon, Session, CandidateKind, CandidateScoreKind};
//!
//! let tsv = concat!(
//!     "MEOWPUUT-PINYIN-1\n",
//!     "hao\thao3\t好\t好\t4700\n",
//!     "ni\tni3\t你\t你\t4800\n",
//!     "nihao\tni3hao3\t你好\t你好\t5200\n",
//!     "# context\n",
//!     "你好\t500\n",
//! );
//! let lexicon = Arc::new(Lexicon::load(tsv).expect("valid lexicon"));
//! let mut session = Session::new(lexicon);
//! for b in b"nihao" {
//!     assert!(session.key(i32::from(*b)));
//! }
//! assert_eq!(session.candidate(0), "你好");
//! let info = session.candidate_info(0).expect("metadata");
//! assert_eq!(info.kind, CandidateKind::Exact as u32);
//! assert_eq!(info.score_kind, CandidateScoreKind::Lexicon as u32);
//! assert_eq!(info.score, 5200.0);
//! assert!(session.choose(0));
//! assert_eq!(session.committed(), "你好");
//! ```

pub mod bin_format;
pub mod context_index;
pub mod decoder;
pub mod fallback;
pub mod image;
pub mod japanese;
pub mod latin;
pub mod lexicon;
pub mod merge;
pub mod mixed;
pub mod mixed_prefix;
pub mod pairs;
pub mod predict;
pub mod reading;
pub mod refresh;
pub mod row;
pub mod romaji;
pub mod segments;
pub mod session;
pub mod soa;
pub mod workspace;

pub use lexicon::Lexicon;
pub use reading::{is_tone, Reading, CANDIDATE_LIMIT, MAX_INPUT, PAGE_SIZE};
pub use session::Session;
pub use soa::{BucketTableSoA, CandidatePoolSoA, ContextTableSoA, JapaneseTableSoA, WordTableSoA};
pub use workspace::DecoderScratchSoA;

/// Provenance describing how a candidate was generated.
///
/// # Examples
///
/// ```
/// use meowpuut_core::CandidateKind;
/// assert_eq!(CandidateKind::Exact as u32, 1);
/// assert_eq!(CandidateKind::Path as u32, 2);
/// assert_eq!(CandidateKind::Partial as u32, 3);
/// assert_eq!(CandidateKind::Correction as u32, 4);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum CandidateKind {
    Unknown = 0,
    Exact = 1,
    Path = 2,
    Partial = 3,
    Correction = 4,
}

/// Score provenance category for a generated candidate.
///
/// # Examples
///
/// ```
/// use meowpuut_core::CandidateScoreKind;
/// assert_eq!(CandidateScoreKind::None as u32, 0);
/// assert_eq!(CandidateScoreKind::Lexicon as u32, 1);
/// assert_eq!(CandidateScoreKind::Path as u32, 2);
/// assert_eq!(CandidateScoreKind::Fallback as u32, 3);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum CandidateScoreKind {
    None = 0,
    Lexicon = 1,
    Path = 2,
    Fallback = 3,
}

/// Candidate metadata matching C `MPNCandidateInfo` bit-for-bit.
///
/// # Examples
///
/// ```
/// use meowpuut_core::{CandidateInfo, CandidateKind, CandidateScoreKind};
/// let info = CandidateInfo {
///     kind: CandidateKind::Exact as u32,
///     score_kind: CandidateScoreKind::Lexicon as u32,
///     consumed_bytes: 5,
///     remaining_offset_bytes: 5,
///     score: 5000.0,
/// };
/// assert_eq!(info.consumed_bytes, 5);
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CandidateInfo {
    pub kind: u32,
    pub score_kind: u32,
    pub consumed_bytes: u64,
    pub remaining_offset_bytes: u64,
    pub score: f64,
}

/// Returns the maximum ASCII input bytes supported in a single composition.
///
/// # Examples
///
/// ```
/// assert_eq!(meowpuut_core::max_input_bytes(), 256);
/// ```
#[inline]
pub const fn max_input_bytes() -> usize {
    MAX_INPUT
}
