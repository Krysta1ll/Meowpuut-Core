#![forbid(unsafe_code)]
//! Binary script-aware cross-word association table (`MIMO-ASC-1\0\0`).
//!
//! Stores separate Traditional (`tc`) and Simplified (`sc`) CSR Structure-of-Arrays
//! tables read either in place (`heap_bytes() == 0`) or owned on the heap, enabling
//! zero-allocation cross-word next-word prediction in [`crate::predict`].
//!
//! # Examples
//!
//! ```
//! use meowpuut_core::assoc::{pack_image, Assoc};
//!
//! let tc: &[(&str, &[(&str, u16)])] = &[
//!     ("今天", &[("天氣", 9000), ("晚上", 7000)]),
//!     ("幫我", &[("看一下", 9500), ("查一下", 8000)]),
//! ];
//! let sc: &[(&str, &[(&str, u16)])] = &[
//!     ("帮我", &[("看一下", 9500), ("发一下", 7500)]),
//! ];
//! let bytes = pack_image(tc, sc).unwrap();
//! let assoc = Assoc::load(&bytes).unwrap();
//! let mut hits = Vec::new();
//! assoc.visit(false, "幫我".as_bytes(), |text, score| hits.push((text.to_owned(), score)));
//! assert_eq!(hits, vec![("看一下".to_owned(), 9500), ("查一下".to_owned(), 8000)]);
//! hits.clear();
//! assoc.visit(true, "帮我".as_bytes(), |text, score| hits.push((text.to_owned(), score)));
//! assert_eq!(hits, vec![("看一下".to_owned(), 9500), ("发一下".to_owned(), 7500)]);
//! ```

use std::collections::HashMap;

use crate::image::{Bytes, ImageOwner, U16Col, U32Col};

const MAGIC: &[u8; 12] = b"MIMO-ASC-1\0\0";
const VERSION: u32 = 1;
const HEADER_BYTES: usize = 64;

#[derive(Debug, Clone)]
struct ScriptTable {
    keys: usize,
    edges: usize,
    key_offsets: U32Col,
    key_lens: U16Col,
    edge_offsets: U32Col,
    val_offsets: U32Col,
    val_lens: U16Col,
    scores: U16Col,
}

impl ScriptTable {
    fn byte_len(keys: usize, edges: usize) -> Option<usize> {
        let k4 = keys.checked_mul(4)?;
        let k2 = keys.checked_mul(2)?;
        let kp1_4 = keys.checked_add(1)?.checked_mul(4)?;
        let e4 = edges.checked_mul(4)?;
        let e2 = edges.checked_mul(2)?;
        k4.checked_add(k2)?.checked_add(kp1_4)?.checked_add(e4)?.checked_add(e2)?.checked_add(e2)
    }

    fn parse<'a>(
        bytes: &'a [u8],
        keys: usize,
        edges: usize,
        strings: &[u8],
        col: &dyn Fn(&'a [u8]) -> Bytes,
    ) -> Option<Self> {
        let mut pos = 0usize;
        let mut take = |len: usize| -> Option<&'a [u8]> {
            let end = pos.checked_add(len)?;
            let slice = bytes.get(pos..end)?;
            pos = end;
            Some(slice)
        };
        let key_offsets = U32Col::new(col(take(keys.checked_mul(4)?)?));
        let key_lens = U16Col::new(col(take(keys.checked_mul(2)?)?));
        let edge_offsets = U32Col::new(col(take(keys.checked_add(1)?.checked_mul(4)?)?));
        let val_offsets = U32Col::new(col(take(edges.checked_mul(4)?)?));
        let val_lens = U16Col::new(col(take(edges.checked_mul(2)?)?));
        let scores = U16Col::new(col(take(edges.checked_mul(2)?)?));
        if pos != bytes.len() {
            return None;
        }
        let table = Self { keys, edges, key_offsets, key_lens, edge_offsets, val_offsets, val_lens, scores };
        table.validate(strings).then_some(table)
    }

    fn validate(&self, strings: &[u8]) -> bool {
        if self.edge_offsets.get(0) != 0 || self.edge_offsets.get(self.keys) as usize != self.edges {
            return false;
        }
        let s_len = strings.len();
        let mut prev_key: Option<&[u8]> = None;
        let mut prev_edge = 0u32;
        for i in 0..self.keys {
            let off = self.key_offsets.get(i) as usize;
            let len = usize::from(self.key_lens.get(i));
            if len == 0 || off.checked_add(len).is_none_or(|e| e > s_len) {
                return false;
            }
            let key = &strings[off..off + len];
            if std::str::from_utf8(key).is_err() {
                return false;
            }
            if let Some(prev) = prev_key {
                if key <= prev {
                    return false;
                }
            }
            prev_key = Some(key);
            let e_end = self.edge_offsets.get(i + 1);
            if e_end < prev_edge {
                return false;
            }
            prev_edge = e_end;
        }
        for e in 0..self.edges {
            let off = self.val_offsets.get(e) as usize;
            let len = usize::from(self.val_lens.get(e));
            if len == 0 || off.checked_add(len).is_none_or(|end| end > s_len) {
                return false;
            }
            if std::str::from_utf8(&strings[off..off + len]).is_err() {
                return false;
            }
        }
        true
    }

    #[inline]
    fn key_bytes<'a>(&self, strings: &'a [u8], idx: usize) -> &'a [u8] {
        let off = self.key_offsets.get(idx) as usize;
        let len = usize::from(self.key_lens.get(idx));
        &strings[off..off + len]
    }

    fn find_key(&self, strings: &[u8], target: &[u8]) -> Option<usize> {
        let (mut lo, mut hi) = (0usize, self.keys);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            match self.key_bytes(strings, mid).cmp(target) {
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
                std::cmp::Ordering::Equal => return Some(mid),
            }
        }
        None
    }

    fn visit<'a>(&self, strings: &'a [u8], key: &[u8], mut f: impl FnMut(&'a str, u16)) {
        let Some(k_idx) = self.find_key(strings, key) else { return };
        let start = self.edge_offsets.get(k_idx) as usize;
        let end = self.edge_offsets.get(k_idx + 1) as usize;
        for e in start..end {
            let off = self.val_offsets.get(e) as usize;
            let len = usize::from(self.val_lens.get(e));
            if let Ok(text) = std::str::from_utf8(&strings[off..off + len]) {
                f(text, self.scores.get(e));
            }
        }
    }

    fn heap_bytes(&self) -> usize {
        self.key_offsets.heap_bytes()
            + self.key_lens.heap_bytes()
            + self.edge_offsets.heap_bytes()
            + self.val_offsets.heap_bytes()
            + self.val_lens.heap_bytes()
            + self.scores.heap_bytes()
    }
}

/// Immutable script-aware cross-word association table (`MIMO-ASC-1\0\0`).
#[derive(Debug, Clone)]
pub struct Assoc {
    tc: ScriptTable,
    sc: ScriptTable,
    strings: Bytes,
    _owner: Option<ImageOwner>,
}

impl Assoc {
    /// Loads a `MIMO-ASC-1` image by copying its columns onto the heap.
    pub fn load(bytes: &[u8]) -> Option<Self> {
        Self::parse(bytes, &|b| Bytes::owned(b.to_vec()), None)
    }

    /// Reads a `MIMO-ASC-1` image directly from caller-owned `'static` bytes (`heap_bytes() == 0`).
    pub fn in_place(bytes: &'static [u8], owner: Option<ImageOwner>) -> Option<Self> {
        Self::parse(bytes, &Bytes::mapped, owner)
    }

    fn parse<'a>(bytes: &'a [u8], col: &dyn Fn(&'a [u8]) -> Bytes, owner: Option<ImageOwner>) -> Option<Self> {
        if bytes.len() < HEADER_BYTES || &bytes[0..12] != MAGIC {
            return None;
        }
        if read_u32(bytes, 12)? != VERSION {
            return None;
        }
        let tc_keys = read_u32(bytes, 16)? as usize;
        let tc_edges = read_u32(bytes, 20)? as usize;
        let sc_keys = read_u32(bytes, 24)? as usize;
        let sc_edges = read_u32(bytes, 28)? as usize;
        let strings_bytes = read_u32(bytes, 32)? as usize;

        let tc_len = ScriptTable::byte_len(tc_keys, tc_edges)?;
        let sc_len = ScriptTable::byte_len(sc_keys, sc_edges)?;
        let tc_end = HEADER_BYTES.checked_add(tc_len)?;
        let sc_end = tc_end.checked_add(sc_len)?;
        if bytes.len() != sc_end.checked_add(strings_bytes)? {
            return None;
        }
        let strings_slice = &bytes[sc_end..];
        let tc = ScriptTable::parse(&bytes[HEADER_BYTES..tc_end], tc_keys, tc_edges, strings_slice, col)?;
        let sc = ScriptTable::parse(&bytes[tc_end..sc_end], sc_keys, sc_edges, strings_slice, col)?;
        Some(Self { tc, sc, strings: col(strings_slice), _owner: owner })
    }

    /// Visits all stored continuations for `key` in the chosen script without allocating.
    #[inline]
    pub fn visit<'a>(&'a self, simplified: bool, key: &[u8], f: impl FnMut(&'a str, u16)) {
        let table = if simplified { &self.sc } else { &self.tc };
        table.visit(&self.strings, key, f);
    }

    /// Number of context keys `(traditional, simplified)`.
    #[inline]
    pub fn key_counts(&self) -> (usize, usize) {
        (self.tc.keys, self.sc.keys)
    }

    /// Number of continuation edges `(traditional, simplified)`.
    #[inline]
    pub fn edge_counts(&self) -> (usize, usize) {
        (self.tc.edges, self.sc.edges)
    }

    /// Heap bytes owned by this table (`0` when loaded via [`Assoc::in_place`]).
    pub fn heap_bytes(&self) -> usize {
        self.tc.heap_bytes() + self.sc.heap_bytes() + self.strings.heap_bytes()
    }
}

/// Packs `(key, [(continuation, score)])` lists for Traditional and Simplified into `MIMO-ASC-1`.
pub fn pack_image<'a>(
    tc_rows: &[(&'a str, &[(&'a str, u16)])],
    sc_rows: &[(&'a str, &[(&'a str, u16)])],
) -> Option<Vec<u8>> {
    let mut pool = Vec::<u8>::new();
    let mut intern_map = HashMap::<&'a str, (u32, u16)>::new();
    let mut intern = |s: &'a str| -> Option<(u32, u16)> {
        if s.is_empty() || s.len() > usize::from(u16::MAX) {
            return None;
        }
        if let Some(&pair) = intern_map.get(s) {
            return Some(pair);
        }
        let off = u32::try_from(pool.len()).ok()?;
        let len = u16::try_from(s.len()).ok()?;
        pool.extend_from_slice(s.as_bytes());
        intern_map.insert(s, (off, len));
        Some((off, len))
    };

    let build = |rows: &[(&'a str, &[(&'a str, u16)])], intern: &mut dyn FnMut(&'a str) -> Option<(u32, u16)>| {
        let mut sorted: Vec<(&'a str, &[(&'a str, u16)])> = rows.to_vec();
        sorted.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        let (mut k_off, mut k_len, mut e_off) = (Vec::new(), Vec::new(), Vec::with_capacity(sorted.len() + 1));
        let (mut v_off, mut v_len, mut scores) = (Vec::new(), Vec::new(), Vec::new());
        let mut prev: Option<&[u8]> = None;
        e_off.push(0u32);
        for (key, vals) in sorted {
            if prev.is_some_and(|p| key.as_bytes() <= p) {
                return None;
            }
            prev = Some(key.as_bytes());
            let (ko, kl) = intern(key)?;
            k_off.push(ko);
            k_len.push(kl);
            for &(val, sc) in vals {
                let (vo, vl) = intern(val)?;
                v_off.push(vo);
                v_len.push(vl);
                scores.push(sc);
            }
            e_off.push(u32::try_from(v_off.len()).ok()?);
        }
        Some((k_off, k_len, e_off, v_off, v_len, scores))
    };

    let tc = build(tc_rows, &mut intern)?;
    let sc = build(sc_rows, &mut intern)?;
    let mut out = Vec::with_capacity(HEADER_BYTES + pool.len() + (tc.0.len() + sc.0.len()) * 10 + (tc.3.len() + sc.3.len()) * 8);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(tc.0.len() as u32).to_le_bytes());
    out.extend_from_slice(&(tc.3.len() as u32).to_le_bytes());
    out.extend_from_slice(&(sc.0.len() as u32).to_le_bytes());
    out.extend_from_slice(&(sc.3.len() as u32).to_le_bytes());
    out.extend_from_slice(&(pool.len() as u32).to_le_bytes());
    out.resize(HEADER_BYTES, 0);
    for tbl in [&tc, &sc] {
        for &v in &tbl.0 { out.extend_from_slice(&v.to_le_bytes()); }
        for &v in &tbl.1 { out.extend_from_slice(&v.to_le_bytes()); }
        for &v in &tbl.2 { out.extend_from_slice(&v.to_le_bytes()); }
        for &v in &tbl.3 { out.extend_from_slice(&v.to_le_bytes()); }
        for &v in &tbl.4 { out.extend_from_slice(&v.to_le_bytes()); }
        for &v in &tbl.5 { out.extend_from_slice(&v.to_le_bytes()); }
    }
    out.extend_from_slice(&pool);
    Some(out)
}

#[inline]
fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice: [u8; 4] = bytes.get(offset..offset + 4)?.try_into().ok()?;
    Some(u32::from_le_bytes(slice))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_zero_heap_in_place() {
        let tc: &[(&str, &[(&str, u16)])] = &[
            ("今天天氣", &[("真好", 9200), ("好冷", 8100)]),
            ("幫我", &[("看一下", 9500), ("查一下", 8400)]),
        ];
        let sc: &[(&str, &[(&str, u16)])] = &[
            ("帮我", &[("看一下", 9500), ("发一下", 8000)]),
            ("今天天气", &[("真好", 9100)]),
        ];
        let bytes = pack_image(tc, sc).unwrap();
        let owned = Assoc::load(&bytes).unwrap();
        assert_eq!(owned.key_counts(), (2, 2));
        assert_eq!(owned.edge_counts(), (4, 3));
        assert!(owned.heap_bytes() > 0);

        let leaked: &'static [u8] = Box::leak(bytes.into_boxed_slice());
        let mapped = Assoc::in_place(leaked, None).unwrap();
        assert_eq!(mapped.heap_bytes(), 0);
        let mut res = Vec::new();
        mapped.visit(false, "今天天氣".as_bytes(), |t, s| res.push((t, s)));
        assert_eq!(res, vec![("真好", 9200), ("好冷", 8100)]);
        res.clear();
        mapped.visit(true, "今天天气".as_bytes(), |t, s| res.push((t, s)));
        assert_eq!(res, vec![("真好", 9100)]);
        res.clear();
        mapped.visit(false, "不存在".as_bytes(), |t, s| res.push((t, s)));
        assert!(res.is_empty());
    }

    #[test]
    fn rejects_corrupt_and_invalid_images() {
        assert!(Assoc::load(b"too short").is_none());
        let mut bad = pack_image(&[("我", &[("們", 100)])], &[]).unwrap();
        bad[0] = b'X';
        assert!(Assoc::load(&bad).is_none());
        let mut bad_ver = pack_image(&[("我", &[("們", 100)])], &[]).unwrap();
        bad_ver[12] = 9;
        assert!(Assoc::load(&bad_ver).is_none());
        let mut truncated = pack_image(&[("我", &[("們", 100)])], &[]).unwrap();
        truncated.pop();
        assert!(Assoc::load(&truncated).is_none());
        assert!(pack_image(&[("", &[("們", 1)])], &[]).is_none());
        assert!(pack_image(&[("我", &[("們", 1)]), ("我", &[("的", 2)])], &[]).is_none());
    }
}
