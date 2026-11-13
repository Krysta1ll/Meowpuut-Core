#![forbid(unsafe_code)]
//! Byte columns that are either read in place from a lexicon image or owned
//! on the heap.
//!
//! A lexicon read in place holds `&'static` slices of the image. This crate
//! never makes such a slice itself: it comes from the caller (in practice
//! `meowpuut-ffi`, which maps the file and keeps it alive until the lexicon
//! and its [`ImageOwner`] are dropped). The variants are private, so a
//! `'static` reference never leaves this module; readers only get slices
//! borrowed from the column.
//!
//! All numbers are little-endian and need no alignment.
//!
//! # Examples
//!
//! ```
//! use meowpuut_core::image::{Bytes, U32Col};
//! let mut col = U32Col::default();
//! col.push(7);
//! col.push(70_000);
//! assert_eq!((col.len(), col.get(1)), (2, 70_000));
//! static IMAGE: [u8; 8] = [1, 0, 0, 0, 2, 0, 0, 0];
//! let mapped = U32Col::new(Bytes::mapped(&IMAGE));
//! assert_eq!((mapped.get(0), mapped.get(1), mapped.heap_bytes()), (1, 2, 0));
//! ```

use std::fmt;
use std::ops::Deref;
use std::sync::Arc;

/// Keeps the bytes of a lexicon read in place alive; dropped together with
/// the last clone of the lexicon.
#[derive(Clone)]
pub struct ImageOwner {
    _keep: Arc<dyn Send + Sync>,
}

impl ImageOwner {
    /// Wraps any value whose drop releases the image (an unmap, a free).
    pub fn new(value: Arc<dyn Send + Sync>) -> Self {
        Self { _keep: value }
    }
}

impl fmt::Debug for ImageOwner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ImageOwner")
    }
}

#[derive(Clone)]
enum Store {
    Mapped(&'static [u8]),
    Owned(Vec<u8>),
}

/// Bytes read in place from an image, or an owned heap copy.
#[derive(Clone)]
pub struct Bytes(Store);

impl Bytes {
    /// Bytes read in place. They must stay valid as long as the value lives,
    /// which `'static` states for this crate; the caller who made the slice
    /// keeps that promise.
    pub fn mapped(bytes: &'static [u8]) -> Self {
        Self(Store::Mapped(bytes))
    }

    /// Owned heap bytes.
    pub fn owned(bytes: Vec<u8>) -> Self {
        Self(Store::Owned(bytes))
    }

    /// Heap bytes held (0 when read in place).
    pub fn heap_bytes(&self) -> usize {
        match &self.0 {
            Store::Mapped(_) => 0,
            Store::Owned(v) => v.capacity(),
        }
    }

    /// Owned bytes for building; an in-place column is copied first.
    pub fn to_mut(&mut self) -> &mut Vec<u8> {
        if let Store::Mapped(b) = self.0 {
            self.0 = Store::Owned(b.to_vec());
        }
        match &mut self.0 {
            Store::Owned(v) => v,
            Store::Mapped(_) => unreachable!("converted above"),
        }
    }

    /// Drops unused capacity of owned bytes.
    pub fn shrink_to_fit(&mut self) {
        if let Store::Owned(v) = &mut self.0 {
            v.shrink_to_fit();
        }
    }
}

impl Default for Bytes {
    fn default() -> Self {
        Self(Store::Owned(Vec::new()))
    }
}

impl Deref for Bytes {
    type Target = [u8];
    #[inline(always)]
    fn deref(&self) -> &[u8] {
        match &self.0 {
            Store::Mapped(b) => b,
            Store::Owned(v) => v,
        }
    }
}

impl fmt::Debug for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = if self.heap_bytes() == 0 && !self.is_empty() { "mapped" } else { "owned" };
        write!(f, "Bytes({kind}, {} bytes)", self.len())
    }
}

#[derive(Clone)]
enum TextStore {
    Mapped(&'static str),
    Owned(String),
}

/// UTF-8 string arena, read in place or owned.
#[derive(Clone)]
pub struct Text(TextStore);

impl Text {
    /// Text read in place (see [`Bytes::mapped`]).
    pub fn mapped(text: &'static str) -> Self {
        Self(TextStore::Mapped(text))
    }

    /// Owned text.
    pub fn owned(text: String) -> Self {
        Self(TextStore::Owned(text))
    }

    /// Heap bytes held (0 when read in place).
    pub fn heap_bytes(&self) -> usize {
        match &self.0 {
            TextStore::Mapped(_) => 0,
            TextStore::Owned(s) => s.capacity(),
        }
    }
}

impl Deref for Text {
    type Target = str;
    #[inline(always)]
    fn deref(&self) -> &str {
        match &self.0 {
            TextStore::Mapped(s) => s,
            TextStore::Owned(s) => s,
        }
    }
}

impl fmt::Debug for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Text({} bytes)", self.len())
    }
}

macro_rules! column {
    ($(#[$doc:meta])* $name:ident, $ty:ty, $width:expr) => {
        $(#[$doc])*
        #[derive(Clone, Default, Debug)]
        pub struct $name(Bytes);

        impl $name {
            /// Bytes per value.
            pub const WIDTH: usize = $width;

            /// A column over `bytes` (length must be a multiple of the width).
            pub fn new(bytes: Bytes) -> Self {
                Self(bytes)
            }

            /// Empty owned column with room for `cap` values.
            pub fn with_capacity(cap: usize) -> Self {
                Self(Bytes::owned(Vec::with_capacity(cap * $width)))
            }

            /// Number of values.
            #[inline]
            pub fn len(&self) -> usize {
                self.0.len() / $width
            }

            /// True when the column has no values.
            #[inline]
            pub fn is_empty(&self) -> bool {
                self.0.is_empty()
            }

            /// Value at `idx` (panics when out of range, like slice indexing).
            #[inline(always)]
            pub fn get(&self, idx: usize) -> $ty {
                let bytes: &[u8] = &self.0;
                let start = idx * $width;
                let mut arr = [0u8; $width];
                arr.copy_from_slice(&bytes[start..start + $width]);
                <$ty>::from_le_bytes(arr)
            }

            /// Appends a value (copies an in-place column first).
            pub fn push(&mut self, value: $ty) {
                self.0.to_mut().extend_from_slice(&value.to_le_bytes());
            }

            /// Replaces the value at `idx`.
            pub fn set(&mut self, idx: usize, value: $ty) {
                let start = idx * $width;
                self.0.to_mut()[start..start + $width].copy_from_slice(&value.to_le_bytes());
            }

            /// Raw little-endian bytes.
            pub fn as_bytes(&self) -> &[u8] {
                &self.0
            }

            /// Heap bytes held (0 when read in place).
            pub fn heap_bytes(&self) -> usize {
                self.0.heap_bytes()
            }

            /// Drops unused capacity.
            pub fn shrink_to_fit(&mut self) {
                self.0.shrink_to_fit();
            }

            /// All values, in order.
            pub fn iter(&self) -> impl Iterator<Item = $ty> + '_ {
                (0..self.len()).map(move |i| self.get(i))
            }
        }

        impl PartialEq for $name {
            fn eq(&self, other: &Self) -> bool {
                self.as_bytes() == other.as_bytes()
            }
        }
    };
}

column!(/// Column of `u32`.
    U32Col, u32, 4);
column!(/// Column of `u16`.
    U16Col, u16, 2);
column!(/// Column of `i16` (weights and frequencies).
    I16Col, i16, 2);
column!(/// Column of `u8` (flags).
    U8Col, u8, 1);
