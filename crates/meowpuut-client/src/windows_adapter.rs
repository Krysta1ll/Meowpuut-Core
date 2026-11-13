#![forbid(unsafe_code)]
//! Windows TSF (Text Services Framework) session adapter (`WindowsSession`),
//! virtual-key routing (`can_handle_key`, `handle_key`), and 9-key candidate pagination.
//!
//! # Examples
//!
//! ```
//! use std::sync::Arc;
//! use meowpuut_core::Lexicon;
//! use meowpuut_client::windows_adapter::{WindowsKeyAction, WindowsSession, VK_RETURN, VK_SPACE};
//!
//! let tsv = concat!(
//!     "MEOWPUUT-PINYIN-1\n",
//!     "hao\thao3\t好\t好\t4800\n",
//!     "ni\tni3\t你\t你\t5000\n",
//!     "nihao\tni3hao3\t你好\t你好\t5400\n",
//! );
//! let lex = Arc::new(Lexicon::load(tsv).unwrap());
//! let mut win = WindowsSession::new(lex);
//! for b in b"nihao" {
//!     assert!(win.can_handle_key(u32::from(b.to_ascii_uppercase()), *b, false, false));
//!     let act = win.handle_key(u32::from(b.to_ascii_uppercase()), *b, false, false);
//!     assert_eq!(act, WindowsKeyAction::UpdateComposition);
//! }
//! assert_eq!(win.candidate_page(), vec!["你好".to_string()]);
//! let commit_act = win.handle_key(VK_SPACE, b' ', false, false);
//! assert_eq!(commit_act, WindowsKeyAction::Commit(String::from("你好")));
//! assert_eq!(win.handle_key(VK_RETURN, b'\r', false, false), WindowsKeyAction::PassThrough);
//! ```

use std::sync::Arc;

use meowpuut_core::{Lexicon, Session, PAGE_SIZE};

pub const VK_BACK: u32 = 0x08;
pub const VK_RETURN: u32 = 0x0D;
pub const VK_ESCAPE: u32 = 0x1B;
pub const VK_SPACE: u32 = 0x20;
pub const VK_PRIOR: u32 = 0x21;
pub const VK_NEXT: u32 = 0x22;
pub const VK_UP: u32 = 0x26;
pub const VK_DOWN: u32 = 0x28;

/// Result of processing a Windows TSF key event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowsKeyAction {
    PassThrough,
    UpdateComposition,
    Commit(String),
    ClearComposition,
}

/// Stateful Windows TSF IME session adapter wrapping [`meowpuut_core::Session`].
#[derive(Debug, Clone)]
pub struct WindowsSession {
    session: Session,
    highlighted_in_page: usize,
}

impl WindowsSession {
    pub fn new(lexicon: Arc<Lexicon>) -> Self {
        Self {
            session: Session::new(lexicon),
            highlighted_in_page: 0,
        }
    }

    #[inline]
    pub fn is_composing(&self) -> bool {
        !self.session.input().is_empty()
    }

    #[inline]
    pub fn preedit(&self) -> &str {
        self.session.preedit()
    }

    #[inline]
    pub fn page(&self) -> usize {
        self.session.page()
    }

    #[inline]
    pub fn highlighted_in_page(&self) -> usize {
        self.highlighted_in_page
    }

    pub fn set_simplified(&mut self, simplified: bool) {
        self.session.set_simplified(simplified);
    }

    /// Returns the up-to-9 candidates visible on the current page.
    pub fn candidate_page(&self) -> Vec<String> {
        let start = self.session.page() * PAGE_SIZE;
        let end = (start + PAGE_SIZE).min(self.session.candidate_count());
        let mut out = Vec::with_capacity(end.saturating_sub(start));
        for i in start..end {
            out.push(String::from(self.session.candidate(i)));
        }
        out
    }

    /// Fast pre-filter corresponding to `ITfKeyEventSink::OnTestKeyDown`.
    pub fn can_handle_key(&self, vk: u32, ascii: u8, ctrl_down: bool, alt_down: bool) -> bool {
        if ctrl_down || alt_down {
            return false;
        }
        if self.is_composing() {
            if matches!(
                vk,
                VK_BACK | VK_RETURN | VK_ESCAPE | VK_SPACE | VK_PRIOR | VK_NEXT | VK_UP | VK_DOWN
            ) {
                return true;
            }
            if (b'1'..=b'9').contains(&ascii) {
                return true;
            }
        }
        (b'a'..=b'z').contains(&ascii)
    }

    /// Processes a Windows virtual-key / ASCII event (`ITfKeyEventSink::OnKeyDown`).
    pub fn handle_key(
        &mut self,
        vk: u32,
        ascii: u8,
        ctrl_down: bool,
        alt_down: bool,
    ) -> WindowsKeyAction {
        if !self.can_handle_key(vk, ascii, ctrl_down, alt_down) {
            return WindowsKeyAction::PassThrough;
        }

        if self.is_composing() {
            match vk {
                VK_ESCAPE => {
                    self.session.clear();
                    self.highlighted_in_page = 0;
                    return WindowsKeyAction::ClearComposition;
                }
                VK_BACK => {
                    let _ = self.session.key(0xff08);
                    self.highlighted_in_page = 0;
                    return if self.is_composing() {
                        WindowsKeyAction::UpdateComposition
                    } else {
                        WindowsKeyAction::ClearComposition
                    };
                }
                VK_RETURN => {
                    let raw = String::from(self.session.input());
                    self.session.clear();
                    self.highlighted_in_page = 0;
                    return WindowsKeyAction::Commit(raw);
                }
                VK_PRIOR => {
                    let _ = self.session.key(0xff55);
                    self.highlighted_in_page = 0;
                    return WindowsKeyAction::UpdateComposition;
                }
                VK_NEXT => {
                    let _ = self.session.key(0xff56);
                    self.highlighted_in_page = 0;
                    return WindowsKeyAction::UpdateComposition;
                }
                VK_UP => {
                    let page_len = self.candidate_page().len();
                    if page_len > 0 {
                        if self.highlighted_in_page == 0 {
                            let prev_page = self.session.page();
                            let _ = self.session.key(0xff55);
                            if self.session.page() < prev_page {
                                self.highlighted_in_page = self.candidate_page().len().saturating_sub(1);
                            }
                        } else {
                            self.highlighted_in_page -= 1;
                        }
                    }
                    return WindowsKeyAction::UpdateComposition;
                }
                VK_DOWN => {
                    let page_len = self.candidate_page().len();
                    if page_len > 0 {
                        if self.highlighted_in_page + 1 < page_len {
                            self.highlighted_in_page += 1;
                        } else {
                            let prev_page = self.session.page();
                            let _ = self.session.key(0xff56);
                            if self.session.page() > prev_page {
                                self.highlighted_in_page = 0;
                            }
                        }
                    }
                    return WindowsKeyAction::UpdateComposition;
                }
                VK_SPACE => {
                    return self.select_in_current_page(self.highlighted_in_page);
                }
                _ => {}
            }

            if (b'1'..=b'9').contains(&ascii) {
                let slot = (ascii - b'1') as usize;
                return self.select_in_current_page(slot);
            }
        }

        if (b'a'..=b'z').contains(&ascii) && self.session.key(i32::from(ascii)) {
            self.highlighted_in_page = 0;
            let committed = String::from(self.session.committed());
            if !committed.is_empty() {
                self.session.clear_commit();
                return WindowsKeyAction::Commit(committed);
            }
            return WindowsKeyAction::UpdateComposition;
        }

        WindowsKeyAction::PassThrough
    }

    /// Selects a 0-based slot (`0..9`) on the active candidate page.
    pub fn select_in_current_page(&mut self, slot: usize) -> WindowsKeyAction {
        let abs_idx = self.session.page() * PAGE_SIZE + slot;
        if abs_idx >= self.session.candidate_count() {
            return WindowsKeyAction::UpdateComposition;
        }
        let mut accumulated = String::new();
        if self.session.choose(abs_idx) {
            accumulated.push_str(self.session.committed());
            self.session.clear_commit();
        }
        self.highlighted_in_page = 0;
        if self.is_composing() {
            WindowsKeyAction::UpdateComposition
        } else if !accumulated.is_empty() {
            WindowsKeyAction::Commit(accumulated)
        } else {
            WindowsKeyAction::ClearComposition
        }
    }
}
