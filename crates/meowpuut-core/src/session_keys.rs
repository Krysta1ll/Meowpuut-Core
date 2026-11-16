#![forbid(unsafe_code)]
//! Keystroke dispatch (`Session::key`) across nine-key, Pinyin, and Zhuyin modes.
//!
//! # Examples
//!
//! ```
//! use std::sync::Arc;
//! use meowpuut_core::{Lexicon, Session};
//!
//! let tsv = concat!(
//!     "MEOWPUUT-PINYIN-1\n",
//!     "wang\twang3\t往\t往\t7000\n",
//!     "yang\tyang2\t陽\t阳\t8000\n",
//! );
//! let lex = Arc::new(Lexicon::load(tsv).unwrap());
//! let mut s = Session::new_nine_key(lex).unwrap();
//! for b in b"WANG" {
//!     assert!(s.key(i32::from(*b)));
//! }
//! assert_eq!(s.candidate(0), "往");
//! ```

use crate::nine_key::SEPARATOR as NINE_KEY_SEPARATOR;
use crate::reading::{is_tone, zhuyin_symbol_for_key, KEYS, MAX_INPUT, PAGE_SIZE};
use crate::session::Session;

impl Session {
    /// Processes a single key event (ASCII `32..=126` or X11 editing keys `0xff08`, `0xff0d`, `0xff1b`, `0xff55`, `0xff56`).
    pub fn key(&mut self, key: i32) -> bool {
        if key == 0xff55 || key == 0xff56 {
            if self.input.is_empty() {
                return false;
            }
            if key == 0xff55 && self.page > 0 {
                self.page -= 1;
            }
            if key == 0xff56 && (self.page + 1) * PAGE_SIZE < self.candidates.len() {
                self.page += 1;
            }
            return true;
        }
        if key == 0xff1b && !self.input.is_empty() {
            self.clear();
            return true;
        }
        if key == 0xff08 && !self.input.is_empty() {
            self.input.pop();
            if self.input.is_empty() {
                self.segment_context.clear();
            }
            self.refresh();
            return true;
        }
        if key == 0xff0d && !self.input.is_empty() {
            self.commit();
            return true;
        }
        if !(32..=126).contains(&key) {
            return false;
        }
        let ch = key as u8;
        if self.nine_key.is_some() {
            if ch == b' ' && !self.input.is_empty() {
                let literal = self.candidates.is_empty();
                self.commit();
                if literal {
                    self.committed.push(' ');
                }
                return true;
            }
            if ch == NINE_KEY_SEPARATOR {
                if !self
                    .input
                    .as_bytes()
                    .last()
                    .is_some_and(|b| (b'2'..=b'9').contains(b) || b.is_ascii_uppercase())
                {
                    return false;
                }
                if self.input.len() == MAX_INPUT {
                    return true;
                }
            } else if !(b'2'..=b'9').contains(&ch) && !ch.is_ascii_uppercase() {
                return false;
            }
            if self.input.len() == MAX_INPUT {
                self.append_literal_input();
                self.reset();
            }
            self.input.push(char::from(ch));
            self.refresh();
            return true;
        }
        if self.lexicon.pinyin {
            if ch == b' ' && self.input.len() == MAX_INPUT {
                self.committed.push_str(&self.preedit);
                self.committed.push(' ');
                self.reset();
                return true;
            }
            if ch == b' ' && !self.input.is_empty() {
                self.commit();
                return true;
            }
            let boundary = is_tone(ch, true) || ch == b'\'';
            if !(b'a'..=b'z').contains(&ch) && !boundary {
                return false;
            }
            if boundary {
                let last = self.input.as_bytes().last().copied();
                if last.map_or(true, |l| is_tone(l, true) || l == b'\'') {
                    return false;
                }
            }
            if self.input.len() == MAX_INPUT {
                self.committed.push_str(&self.preedit);
                if boundary {
                    self.committed.push(ch as char);
                }
                self.reset();
                if boundary {
                    return true;
                }
            }
            self.input.push(ch as char);
            self.refresh();
            return true;
        }

        let boundary = is_tone(ch, false) || ch == b'\'';
        if !KEYS.contains(&ch) || (boundary && self.input.is_empty()) {
            return false;
        }
        if boundary {
            let last = *self.input.as_bytes().last().unwrap();
            if is_tone(last, false) || last == b'\'' {
                if ch == b' ' {
                    self.commit();
                }
                return true;
            }
        }
        if self.input.len() == MAX_INPUT {
            self.committed.push_str(&self.preedit);
            if boundary {
                if let Some(sym) = zhuyin_symbol_for_key(ch) {
                    self.committed.push_str(sym);
                }
            }
            self.reset();
            if boundary {
                return true;
            }
        }
        self.input.push(ch as char);
        self.refresh();
        true
    }
}
