#![forbid(unsafe_code)]
//! Unified cross-platform client `Engine`, `InputMode`, `PhraseModuleSoA`,
//! `ShiftTapTracker`, and `Snapshot` for macOS, iOS, and Windows clients.
//!
//! # Examples
//!
//! ```
//! use std::sync::Arc;
//! use meowpuut_core::Lexicon;
//! use meowpuut_lexicon::parse_japanese_tsv;
//! use meowpuut_client::engine::{Engine, InputMode};
//!
//! let py_tsv = concat!(
//!     "MEOWPUUT-PINYIN-1\n",
//!     "hao\thao3\t好\t好\t4800\n",
//!     "ni\tni3\t你\t你\t5000\n",
//!     "nihao\tni3hao3\t你好\t你好\t5400\n",
//! );
//! let jp_tsv = "MEOWPUUT-JAPANESE-1\nwatashi\t私\t5500\n";
//! let py_lex = Arc::new(Lexicon::load(py_tsv).unwrap());
//! let jp_lex = parse_japanese_tsv(jp_tsv).unwrap();
//! let mut engine = Engine::new(py_lex, jp_lex);
//! engine.phrases_mut().add("addr", "東京都千代田區");
//!
//! for ch in "nihao".chars() {
//!     engine.handle_char(ch);
//! }
//! let snap = engine.snapshot();
//! assert_eq!(snap.mode, InputMode::Chinese);
//! assert_eq!(snap.candidates[0], "你好");
//! let committed = engine.space().unwrap();
//! assert_eq!(committed, "你好");
//! ```

use std::sync::Arc;

use meowpuut_core::{Lexicon, PAGE_SIZE};
use meowpuut_lexicon::{EnglishEvidenceSoA, JapaneseLexiconSoA};

use crate::backend::ChineseWithJapaneseBackend;

/// Active input mode of the client engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Chinese,
    English,
    Japanese,
}

/// Structure-of-Arrays user phrase shortcut table (`shortcut -> phrase`).
#[derive(Debug, Clone, Default)]
pub struct PhraseModuleSoA {
    pub arena: Vec<u8>,
    pub key_offset: Vec<u32>,
    pub key_len: Vec<u16>,
    pub phrase_offset: Vec<u32>,
    pub phrase_len: Vec<u16>,
}

impl PhraseModuleSoA {
    pub fn add(&mut self, shortcut: &str, phrase: &str) {
        if shortcut.is_empty() || phrase.is_empty() {
            return;
        }
        let k_off = self.arena.len() as u32;
        self.arena.extend_from_slice(shortcut.as_bytes());
        let p_off = self.arena.len() as u32;
        self.arena.extend_from_slice(phrase.as_bytes());
        self.key_offset.push(k_off);
        self.key_len.push(shortcut.len() as u16);
        self.phrase_offset.push(p_off);
        self.phrase_len.push(phrase.len() as u16);
    }

    pub fn lookup(&self, input: &str) -> Vec<String> {
        if input.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for i in 0..self.key_offset.len() {
            let ks = self.key_offset[i] as usize;
            let ke = ks + self.key_len[i] as usize;
            let key = std::str::from_utf8(&self.arena[ks..ke]).unwrap_or("");
            if key.starts_with(input) {
                let ps = self.phrase_offset[i] as usize;
                let pe = ps + self.phrase_len[i] as usize;
                if let Ok(phrase) = std::str::from_utf8(&self.arena[ps..pe]) {
                    out.push(String::from(phrase));
                }
            }
        }
        out
    }
}

/// Tracks clean single Shift taps (`<= 250ms` with no intervening key) to toggle input modes.
#[derive(Debug, Clone, Default)]
pub struct ShiftTapTracker {
    shift_down_ms: Option<u64>,
    interrupted: bool,
}

impl ShiftTapTracker {
    pub fn on_shift_down(&mut self, now_ms: u64) {
        self.shift_down_ms = Some(now_ms);
        self.interrupted = false;
    }

    pub fn on_other_key(&mut self) {
        if self.shift_down_ms.is_some() {
            self.interrupted = true;
        }
    }

    /// Returns `true` if a clean Shift tap completed within `250ms`.
    pub fn on_shift_up(&mut self, now_ms: u64) -> bool {
        let Some(down_ms) = self.shift_down_ms.take() else {
            return false;
        };
        !self.interrupted && now_ms.saturating_sub(down_ms) <= 250
    }
}

/// Immutable UI state snapshot emitted by [`Engine::snapshot`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub mode: InputMode,
    pub simplified: bool,
    pub composing: bool,
    pub raw_input: String,
    pub preedit: String,
    pub candidates: Vec<String>,
    pub highlighted_index: usize,
    pub page: usize,
    pub committed: String,
}

/// Cross-platform client input engine coordinating Chinese, Japanese, and English modes.
#[derive(Debug, Clone)]
pub struct Engine {
    backend: ChineseWithJapaneseBackend,
    english: EnglishEvidenceSoA,
    phrases: PhraseModuleSoA,
    shift_tracker: ShiftTapTracker,
    mode: InputMode,
    english_raw: String,
    post_commit_associations: Vec<String>,
    highlighted_index: usize,
    last_committed: String,
}

impl Engine {
    pub fn new(chinese_lexicon: Arc<Lexicon>, japanese_lexicon: JapaneseLexiconSoA) -> Self {
        Self {
            backend: ChineseWithJapaneseBackend::new(chinese_lexicon, japanese_lexicon),
            english: EnglishEvidenceSoA::default_builtin(),
            phrases: PhraseModuleSoA::default(),
            shift_tracker: ShiftTapTracker::default(),
            mode: InputMode::Chinese,
            english_raw: String::with_capacity(64),
            post_commit_associations: Vec::new(),
            highlighted_index: 0,
            last_committed: String::new(),
        }
    }

    #[inline]
    pub fn mode(&self) -> InputMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: InputMode) {
        if self.mode != mode {
            self.clear();
            self.mode = mode;
        }
    }

    /// Toggles between `Chinese` and `English` modes on a clean Shift tap.
    pub fn toggle_chinese_english(&mut self) -> InputMode {
        let next = match self.mode {
            InputMode::Chinese | InputMode::Japanese => InputMode::English,
            InputMode::English => InputMode::Chinese,
        };
        self.set_mode(next);
        next
    }

    pub fn set_simplified(&mut self, simplified: bool) {
        self.backend.set_simplified(simplified);
    }

    #[inline]
    pub fn phrases_mut(&mut self) -> &mut PhraseModuleSoA {
        &mut self.phrases
    }

    #[inline]
    pub fn shift_tracker_mut(&mut self) -> &mut ShiftTapTracker {
        &mut self.shift_tracker
    }

    pub fn clear(&mut self) {
        self.backend.clear();
        self.english_raw.clear();
        self.post_commit_associations.clear();
        self.highlighted_index = 0;
    }

    /// Processes a printable character in the active `InputMode`.
    pub fn handle_char(&mut self, ch: char) -> bool {
        self.shift_tracker.on_other_key();
        self.post_commit_associations.clear();
        self.highlighted_index = 0;
        match self.mode {
            InputMode::English => {
                if ch.is_ascii_alphabetic() || ch == '\'' {
                    self.english_raw.push(ch);
                    true
                } else {
                    false
                }
            }
            InputMode::Chinese | InputMode::Japanese => {
                if ch.is_ascii() {
                    self.backend.type_key(ch as u8)
                } else {
                    false
                }
            }
        }
    }

    pub fn backspace(&mut self) -> bool {
        self.post_commit_associations.clear();
        self.highlighted_index = 0;
        match self.mode {
            InputMode::English => {
                if self.english_raw.pop().is_some() {
                    true
                } else {
                    false
                }
            }
            InputMode::Chinese | InputMode::Japanese => self.backend.backspace(),
        }
    }

    /// Moves the highlighted candidate selection by `delta` (`+1` or `-1`).
    pub fn browse(&mut self, delta: isize) {
        let total = self.current_candidates().len();
        if total == 0 {
            self.highlighted_index = 0;
            return;
        }
        let cur = self.highlighted_index as isize;
        self.highlighted_index = (cur + delta).rem_euclid(total as isize) as usize;
    }

    /// Commits the currently highlighted candidate on Space.
    pub fn space(&mut self) -> Option<String> {
        let idx = self.highlighted_index;
        self.select_candidate(idx)
    }

    /// Commits the raw input string on Enter/Return.
    pub fn enter(&mut self) -> Option<String> {
        let raw = match self.mode {
            InputMode::English => std::mem::take(&mut self.english_raw),
            InputMode::Chinese | InputMode::Japanese => {
                let s = String::from(self.backend.input());
                self.backend.clear();
                s
            }
        };
        self.post_commit_associations.clear();
        self.highlighted_index = 0;
        if raw.is_empty() {
            None
        } else {
            self.last_committed = raw.clone();
            Some(raw)
        }
    }

    /// Selects the candidate at `index` and updates post-commit associations if composition finishes.
    pub fn select_candidate(&mut self, index: usize) -> Option<String> {
        let cands = self.current_candidates();
        if index >= cands.len() {
            return None;
        }
        self.highlighted_index = 0;
        if !self.post_commit_associations.is_empty() {
            let chosen = self.post_commit_associations[index].clone();
            self.post_commit_associations = self.backend.associations_for(&chosen);
            self.last_committed = chosen.clone();
            return Some(chosen);
        }
        match self.mode {
            InputMode::English => {
                let chosen = cands[index].clone();
                self.english_raw.clear();
                self.last_committed = chosen.clone();
                Some(chosen)
            }
            InputMode::Chinese | InputMode::Japanese => {
                let chosen = self.backend.select(index).or_else(|| {
                    let fallback = cands[index].clone();
                    self.backend.clear();
                    Some(fallback)
                })?;
                if self.backend.input().is_empty() {
                    self.post_commit_associations = self.backend.associations_for(&chosen);
                }
                self.last_committed = chosen.clone();
                Some(chosen)
            }
        }
    }

    pub fn current_candidates(&self) -> Vec<String> {
        if !self.post_commit_associations.is_empty() {
            return self.post_commit_associations.clone();
        }
        match self.mode {
            InputMode::English => self.english.candidates(&self.english_raw, 9),
            InputMode::Chinese | InputMode::Japanese => {
                let raw = self.backend.input();
                let mut list = self.phrases.lookup(raw);
                for cand in self.backend.candidates() {
                    if !list.contains(cand) {
                        list.push(cand.clone());
                    }
                }
                list
            }
        }
    }

    /// Captures an immutable UI [`Snapshot`] of the engine state.
    pub fn snapshot(&self) -> Snapshot {
        let raw_input = match self.mode {
            InputMode::English => self.english_raw.clone(),
            InputMode::Chinese | InputMode::Japanese => String::from(self.backend.input()),
        };
        let preedit = match self.mode {
            InputMode::English => self.english_raw.clone(),
            InputMode::Chinese | InputMode::Japanese => String::from(self.backend.preedit()),
        };
        let candidates = self.current_candidates();
        let page = self.highlighted_index / PAGE_SIZE;
        Snapshot {
            mode: self.mode,
            simplified: self.backend.simplified,
            composing: !raw_input.is_empty(),
            raw_input,
            preedit,
            candidates,
            highlighted_index: self.highlighted_index,
            page,
            committed: self.last_committed.clone(),
        }
    }
}
