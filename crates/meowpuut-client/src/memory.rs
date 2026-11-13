#![forbid(unsafe_code)]
//! Structure-of-Arrays personal learning memory (`ChoiceMemorySoA`) tracking
//! reading-to-word pins, word selection boosts, and sequential context bigrams.
//!
//! # Examples
//!
//! ```
//! use meowpuut_client::memory::ChoiceMemorySoA;
//!
//! let mut mem = ChoiceMemorySoA::new();
//! mem.learn("nihao", "你好");
//! assert!(mem.is_pinned("nihao", "你好"));
//! assert!(mem.boost_for("你好") > 0);
//! mem.learn("shijie", "世界");
//! mem.learn("nihao", "你好");
//! assert!(mem.bigram_bonus_for("世界") > 0);
//!
//! let dumped = mem.serialize();
//! let mut restored = ChoiceMemorySoA::deserialize(&dumped);
//! assert!(restored.is_pinned("nihao", "你好"));
//! let wal = ChoiceMemorySoA::format_wal_event("zaoan", "早安");
//! assert_eq!(restored.replay_wal(&wal), 1);
//! assert!(restored.is_pinned("zaoan", "早安"));
//! ```

/// Structure-of-Arrays personal candidate memory.
#[derive(Debug, Clone, Default)]
pub struct ChoiceMemorySoA {
    pub pin_arena: Vec<u8>,
    pub pin_reading_off: Vec<u32>,
    pub pin_reading_len: Vec<u16>,
    pub pin_text_off: Vec<u32>,
    pub pin_text_len: Vec<u16>,

    pub boost_arena: Vec<u8>,
    pub boost_text_off: Vec<u32>,
    pub boost_text_len: Vec<u16>,
    pub boost_count: Vec<u16>,

    pub bigram_arena: Vec<u8>,
    pub prev_off: Vec<u32>,
    pub prev_len: Vec<u16>,
    pub next_off: Vec<u32>,
    pub next_len: Vec<u16>,
    pub bigram_count: Vec<u16>,

    last_committed: String,
}

impl ChoiceMemorySoA {
    /// Creates an empty preallocated `ChoiceMemorySoA`.
    pub fn new() -> Self {
        Self {
            pin_arena: Vec::with_capacity(512),
            pin_reading_off: Vec::with_capacity(32),
            pin_reading_len: Vec::with_capacity(32),
            pin_text_off: Vec::with_capacity(32),
            pin_text_len: Vec::with_capacity(32),
            boost_arena: Vec::with_capacity(512),
            boost_text_off: Vec::with_capacity(32),
            boost_text_len: Vec::with_capacity(32),
            boost_count: Vec::with_capacity(32),
            bigram_arena: Vec::with_capacity(512),
            prev_off: Vec::with_capacity(32),
            prev_len: Vec::with_capacity(32),
            next_off: Vec::with_capacity(32),
            next_len: Vec::with_capacity(32),
            bigram_count: Vec::with_capacity(32),
            last_committed: String::with_capacity(32),
        }
    }

    #[inline]
    fn slice(arena: &[u8], off: u32, len: u16) -> &str {
        let s = off as usize;
        let e = s + len as usize;
        std::str::from_utf8(&arena[s..e]).unwrap_or("")
    }

    /// Records a user candidate selection for `reading` -> `chosen`.
    pub fn learn(&mut self, reading: &str, chosen: &str) {
        if chosen.is_empty() {
            return;
        }
        if !reading.is_empty() {
            self.set_pin(reading, chosen);
        }
        self.increment_boost(chosen, 1);
        if !self.last_committed.is_empty() {
            let prev = self.last_committed.clone();
            self.increment_bigram(&prev, chosen, 1);
        }
        self.last_committed.clear();
        self.last_committed.push_str(chosen);
    }

    /// Resets the sequential context chain (e.g. when clearing or switching fields).
    pub fn clear_context(&mut self) {
        self.last_committed.clear();
    }

    #[inline]
    pub fn last_committed(&self) -> &str {
        &self.last_committed
    }

    fn set_pin(&mut self, reading: &str, chosen: &str) {
        for i in 0..self.pin_reading_off.len() {
            if Self::slice(&self.pin_arena, self.pin_reading_off[i], self.pin_reading_len[i]) == reading {
                let t_off = self.pin_arena.len() as u32;
                self.pin_arena.extend_from_slice(chosen.as_bytes());
                self.pin_text_off[i] = t_off;
                self.pin_text_len[i] = chosen.len() as u16;
                return;
            }
        }
        let r_off = self.pin_arena.len() as u32;
        self.pin_arena.extend_from_slice(reading.as_bytes());
        let t_off = self.pin_arena.len() as u32;
        self.pin_arena.extend_from_slice(chosen.as_bytes());
        self.pin_reading_off.push(r_off);
        self.pin_reading_len.push(reading.len() as u16);
        self.pin_text_off.push(t_off);
        self.pin_text_len.push(chosen.len() as u16);
    }

    fn increment_boost(&mut self, chosen: &str, delta: u16) {
        for i in 0..self.boost_text_off.len() {
            if Self::slice(&self.boost_arena, self.boost_text_off[i], self.boost_text_len[i]) == chosen {
                self.boost_count[i] = self.boost_count[i].saturating_add(delta).min(50);
                return;
            }
        }
        let off = self.boost_arena.len() as u32;
        self.boost_arena.extend_from_slice(chosen.as_bytes());
        self.boost_text_off.push(off);
        self.boost_text_len.push(chosen.len() as u16);
        self.boost_count.push(delta.min(50));
    }

    fn increment_bigram(&mut self, prev: &str, next: &str, delta: u16) {
        for i in 0..self.prev_off.len() {
            let p = Self::slice(&self.bigram_arena, self.prev_off[i], self.prev_len[i]);
            let n = Self::slice(&self.bigram_arena, self.next_off[i], self.next_len[i]);
            if p == prev && n == next {
                self.bigram_count[i] = self.bigram_count[i].saturating_add(delta).min(50);
                return;
            }
        }
        let p_off = self.bigram_arena.len() as u32;
        self.bigram_arena.extend_from_slice(prev.as_bytes());
        let n_off = self.bigram_arena.len() as u32;
        self.bigram_arena.extend_from_slice(next.as_bytes());
        self.prev_off.push(p_off);
        self.prev_len.push(prev.len() as u16);
        self.next_off.push(n_off);
        self.next_len.push(next.len() as u16);
        self.bigram_count.push(delta.min(50));
    }

    /// Returns `true` if `text` is the most recently pinned choice for `reading`.
    pub fn is_pinned(&self, reading: &str, text: &str) -> bool {
        for i in 0..self.pin_reading_off.len() {
            let r = Self::slice(&self.pin_arena, self.pin_reading_off[i], self.pin_reading_len[i]);
            if r == reading {
                let t = Self::slice(&self.pin_arena, self.pin_text_off[i], self.pin_text_len[i]);
                return t == text;
            }
        }
        false
    }

    /// Returns a frequency boost score (`0..=2500`) for `text`.
    pub fn boost_for(&self, text: &str) -> i32 {
        for i in 0..self.boost_text_off.len() {
            if Self::slice(&self.boost_arena, self.boost_text_off[i], self.boost_text_len[i]) == text {
                return i32::from(self.boost_count[i]) * 180;
            }
        }
        0
    }

    /// Returns a sequential bigram bonus (`0..=2400`) when `text` follows `last_committed`.
    pub fn bigram_bonus_for(&self, text: &str) -> i32 {
        if self.last_committed.is_empty() {
            return 0;
        }
        for i in 0..self.prev_off.len() {
            let p = Self::slice(&self.bigram_arena, self.prev_off[i], self.prev_len[i]);
            let n = Self::slice(&self.bigram_arena, self.next_off[i], self.next_len[i]);
            if p == self.last_committed && n == text {
                return i32::from(self.bigram_count[i]) * 240;
            }
        }
        0
    }

    /// Serializes the memory state to a compact TSV string.
    pub fn serialize(&self) -> String {
        let mut out = String::new();
        for i in 0..self.pin_reading_off.len() {
            let r = Self::slice(&self.pin_arena, self.pin_reading_off[i], self.pin_reading_len[i]);
            let t = Self::slice(&self.pin_arena, self.pin_text_off[i], self.pin_text_len[i]);
            out.push_str(&format!("P\t{r}\t{t}\n"));
        }
        for i in 0..self.boost_text_off.len() {
            let t = Self::slice(&self.boost_arena, self.boost_text_off[i], self.boost_text_len[i]);
            let c = self.boost_count[i];
            out.push_str(&format!("B\t{t}\t{c}\n"));
        }
        for i in 0..self.prev_off.len() {
            let p = Self::slice(&self.bigram_arena, self.prev_off[i], self.prev_len[i]);
            let n = Self::slice(&self.bigram_arena, self.next_off[i], self.next_len[i]);
            let c = self.bigram_count[i];
            out.push_str(&format!("G\t{p}\t{n}\t{c}\n"));
        }
        out
    }

    /// Deserializes a `ChoiceMemorySoA` from `serialize()` output.
    pub fn deserialize(data: &str) -> Self {
        let mut mem = Self::new();
        for line in data.lines() {
            let mut parts = line.split('\t');
            match parts.next() {
                Some("P") => {
                    if let (Some(r), Some(t)) = (parts.next(), parts.next()) {
                        mem.set_pin(r, t);
                    }
                }
                Some("B") => {
                    if let (Some(t), Some(c_str)) = (parts.next(), parts.next()) {
                        if let Ok(c) = c_str.parse::<u16>() {
                            mem.increment_boost(t, c);
                        }
                    }
                }
                Some("G") => {
                    if let (Some(p), Some(n), Some(c_str)) = (parts.next(), parts.next(), parts.next()) {
                        if let Ok(c) = c_str.parse::<u16>() {
                            mem.increment_bigram(p, n, c);
                        }
                    }
                }
                _ => {}
            }
        }
        mem
    }

    /// Formats a single candidate selection as an append-only WAL line (`mimo_user_wal.log`).
    pub fn format_wal_event(reading: &str, chosen: &str) -> String {
        if chosen.is_empty() || reading.contains('\n') || chosen.contains('\n') {
            return String::new();
        }
        format!("W\t{reading}\t{chosen}\n")
    }

    /// Replays append-only WAL lines (`W\t<reading>\t<chosen>`) into this memory instance,
    /// returning the number of applied events.
    pub fn replay_wal(&mut self, wal_data: &str) -> usize {
        let mut count = 0usize;
        for line in wal_data.lines() {
            let mut parts = line.split('\t');
            if parts.next() == Some("W") {
                if let (Some(reading), Some(chosen)) = (parts.next(), parts.next()) {
                    if !chosen.is_empty() {
                        self.learn(reading, chosen);
                        count += 1;
                    }
                }
            }
        }
        count
    }

    /// Compacts `wal_data` into this memory instance and returns the serialized
    /// base snapshot (`mimo_user.db`) so the caller can truncate `mimo_user_wal.log`.
    pub fn compact_with_wal(&mut self, wal_data: &str) -> String {
        self.replay_wal(wal_data);
        self.serialize()
    }
}
