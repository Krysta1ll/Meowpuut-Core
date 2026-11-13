#![forbid(unsafe_code)]
//! `meowpuut-client` — Cross-platform Rust client layer (macOS IMKit, iOS Keyboard
//! Extension, and Windows TSF) built on Structure-of-Arrays (SoA) candidate pools
//! and zero-unsafe state machines.
//!
//! # Examples
//!
//! ```
//! use std::sync::Arc;
//! use meowpuut_core::Lexicon;
//! use meowpuut_lexicon::parse_japanese_tsv;
//! use meowpuut_client::{Engine, InputMode};
//!
//! let py_tsv = concat!(
//!     "MEOWPUUT-PINYIN-1\n",
//!     "hao\thao3\t好\t好\t4800\n",
//!     "ni\tni3\t你\t你\t5000\n",
//!     "nihao\tni3hao3\t你好\t你好\t5400\n",
//! );
//! let jp_tsv = "MEOWPUUT-JAPANESE-1\nneko\t猫\t5200\n";
//! let py = Arc::new(Lexicon::load(py_tsv).unwrap());
//! let jp = parse_japanese_tsv(jp_tsv).unwrap();
//! let mut engine = Engine::new(py, jp);
//! for c in "nihao".chars() {
//!     engine.handle_char(c);
//! }
//! assert_eq!(engine.snapshot().mode, InputMode::Chinese);
//! assert_eq!(engine.space().as_deref(), Some("你好"));
//! ```

pub mod backend;
pub mod engine;
pub mod layout;
pub mod memory;
pub mod windows_adapter;

pub use backend::{compact_japanese, ChineseWithJapaneseBackend, ChoicePoolSoA};
pub use engine::{Engine, InputMode, PhraseModuleSoA, ShiftTapTracker, Snapshot};
pub use layout::{
    candidate_frame, estimate_pill_width, expanded_candidate_rows, CandidateGridCell,
    CollapsedCandidateLayout, KeyboardLayoutMetrics, Rect,
};
pub use memory::ChoiceMemorySoA;
pub use windows_adapter::{WindowsKeyAction, WindowsSession};
