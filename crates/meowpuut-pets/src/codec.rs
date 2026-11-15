//! Persisted pet state: its versioned little-endian binary form, the additive
//! [`Delta`] a process flushes, and [`PetState::merge`].
//!
//! # Binary layout (version 1, always [`STATE_BYTES`] long)
//!
//! | offset | size | field |
//! | --- | --- | --- |
//! | 0 | 2 | magic `9C A7` (not ASCII) |
//! | 2 | 1 | version (1) |
//! | 3 | 1 | reserved, 0 |
//! | 4 | 4 | epoch u32 (raised by "clear records") |
//! | 8 | 8 | committed graphemes u64 |
//! | 16 | 24 | Mimimo pets, falls, plays (u64 each) |
//! | 40 | 24 | Momo pets, falls, plays |
//! | 64 | 4 | daily: local day index u32 |
//! | 68 | 4 | daily: graphemes that day u32 |
//! | 72 | 1 | daily: flags (bit 0 greeted) |
//! | 73 | 1 | daily: spontaneous moments |
//! | 74 | 2 | daily: purr rewards Mimimo, Momo |
//! | 76 | 1 | daily: duets |
//! | 77 | 3 | reserved, 0 |
//! | 80 | 2800 | 400 marks: day u32, size u8, who u8, part u8 |
//! | 2880 | 4 | FNV-1a 32 of bytes 0..2880 |
//!
//! The length never depends on the content, and nothing in it is text.
//! `from_bytes` never panics: a wrong length, magic or checksum, or an
//! out-of-range field, is [`CodecError::Corrupt`]; another version is
//! [`CodecError::Version`]. Clients start fresh on any error ([`PetState::load`]).
//!
//! # Merge and Delta
//!
//! `merge` is a join: the higher epoch wins outright; with equal epochs the
//! lifetime counters take the max, the daily block of the later day wins (the
//! same day: max of counts, OR of flags) and the book joins slot by slot. It
//! is commutative, associative and idempotent.
//!
//! Max alone would lose increments made by two processes at once, so a flush
//! is: lock the file, read it, `apply_delta` the increments this process made
//! since its last flush, `merge` this process's state, write a temporary file,
//! rename, then `merge` the result back into the live state.

use crate::book::{size_band, Mark, PawBook, BOOK_DAYS};
use crate::{Contacts, Pet, Snapshot};

/// Current format version.
pub const VERSION: u8 = 1;
/// Serialized state length (version 1).
pub const STATE_BYTES: usize = 2884;
/// Serialized delta length (version 1).
pub const DELTA_BYTES: usize = 84;

const STATE_MAGIC: [u8; 2] = [0x9C, 0xA7];
const DELTA_MAGIC: [u8; 2] = [0x9C, 0xA8];
const MARKS_AT: usize = 80;
const MARK_BYTES: usize = 7;

const _: () = assert!(MARKS_AT + BOOK_DAYS * MARK_BYTES + 4 == STATE_BYTES);
const _: () = assert!(STATE_BYTES <= 4096);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodecError {
    /// Wrong length, magic or checksum, or a field out of range.
    Corrupt,
    /// A version this build does not know.
    Version(u8),
}

/// Counters of one local day; they limit how often moments happen.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct Daily {
    pub day: u32,
    pub graphemes: u32,
    pub greeted: bool,
    pub spontaneous: u8,
    pub purrs: [u8; 2],
    pub duets: u8,
}

impl Daily {
    fn join(self, other: Self) -> Self {
        match self.day.cmp(&other.day) {
            core::cmp::Ordering::Greater => self,
            core::cmp::Ordering::Less => other,
            core::cmp::Ordering::Equal => Self {
                day: self.day,
                graphemes: self.graphemes.max(other.graphemes),
                greeted: self.greeted | other.greeted,
                spontaneous: self.spontaneous.max(other.spontaneous),
                purrs: [self.purrs[0].max(other.purrs[0]), self.purrs[1].max(other.purrs[1])],
                duets: self.duets.max(other.duets),
            },
        }
    }
}

/// Everything persisted: counters, today's limits and the paw book.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PetState {
    pub epoch: u32,
    pub snapshot: Snapshot,
    pub daily: Daily,
    pub book: PawBook,
}

/// Increments made by one process since its last flush. Applying it adds.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Delta {
    /// Epoch of the state the increments were made on.
    pub epoch: u32,
    pub graphemes: u64,
    pub contacts: [Contacts; 2],
    /// Day of the daily fields below (0: none).
    pub day: u32,
    pub day_graphemes: u32,
    pub greeted: bool,
    pub spontaneous: u8,
    pub purrs: [u8; 2],
    pub duets: u8,
    /// Who and day-part bits for that day's mark.
    pub who: u8,
    pub part: u8,
}

impl Delta {
    pub fn is_empty(&self) -> bool {
        *self == Self { epoch: self.epoch, day: self.day, ..Self::default() }
    }
}

pub(crate) const fn pet_index(pet: Pet) -> usize {
    match pet {
        Pet::Mimimo => 0,
        Pet::Momo => 1,
    }
}

fn max_contacts(a: Contacts, b: Contacts) -> Contacts {
    Contacts { pets: a.pets.max(b.pets), falls: a.falls.max(b.falls), plays: a.plays.max(b.plays) }
}

fn add_contacts(a: Contacts, b: Contacts) -> Contacts {
    Contacts {
        pets: a.pets.saturating_add(b.pets),
        falls: a.falls.saturating_add(b.falls),
        plays: a.plays.saturating_add(b.plays),
    }
}

impl PetState {
    /// A state holding counters imported from the old per-platform storage.
    pub fn from_snapshot(snapshot: Snapshot) -> Self {
        Self { snapshot, ..Self::default() }
    }

    /// Parses stored bytes; any error yields a fresh state.
    pub fn load(bytes: &[u8]) -> Self {
        Self::from_bytes(bytes).unwrap_or_default()
    }

    /// Join with another copy; see the module documentation.
    pub fn merge(&mut self, other: &Self) {
        if other.epoch > self.epoch {
            *self = other.clone();
            return;
        }
        if other.epoch < self.epoch {
            return;
        }
        let (a, b) = (self.snapshot, other.snapshot);
        self.snapshot = Snapshot {
            committed_graphemes: a.committed_graphemes.max(b.committed_graphemes),
            mimimo: max_contacts(a.mimimo, b.mimimo),
            momo: max_contacts(a.momo, b.momo),
        };
        self.daily = self.daily.join(other.daily);
        self.book.merge(&other.book);
    }

    /// Adds a flushed delta. A delta from an older epoch (made before the
    /// records were cleared) is dropped; one from a newer epoch starts over.
    pub fn apply_delta(&mut self, delta: &Delta) {
        if delta.epoch < self.epoch {
            return;
        }
        if delta.epoch > self.epoch {
            *self = Self { epoch: delta.epoch, ..Self::default() };
        }
        let s = &mut self.snapshot;
        s.committed_graphemes = s.committed_graphemes.saturating_add(delta.graphemes);
        s.mimimo = add_contacts(s.mimimo, delta.contacts[0]);
        s.momo = add_contacts(s.momo, delta.contacts[1]);
        if delta.day == 0 {
            return;
        }
        let band = if delta.day > self.daily.day {
            self.daily = Daily { day: delta.day, ..Daily::default() };
            Some(())
        } else if delta.day == self.daily.day {
            Some(())
        } else {
            None
        };
        let size = if band.is_some() {
            let d = &mut self.daily;
            d.graphemes = d.graphemes.saturating_add(delta.day_graphemes);
            d.greeted |= delta.greeted;
            d.spontaneous = d.spontaneous.saturating_add(delta.spontaneous);
            d.purrs[0] = d.purrs[0].saturating_add(delta.purrs[0]);
            d.purrs[1] = d.purrs[1].saturating_add(delta.purrs[1]);
            d.duets = d.duets.saturating_add(delta.duets);
            size_band(d.graphemes)
        } else {
            size_band(delta.day_graphemes)
        };
        if delta.day_graphemes > 0 || delta.who != 0 || delta.part != 0 {
            self.book.record(delta.day, size, delta.who, delta.part);
        }
    }

    /// Clears every record and raises the epoch so other processes' older
    /// copies cannot bring them back through `merge`.
    pub fn clear(&mut self) {
        *self = Self { epoch: self.epoch.wrapping_add(1).max(1), ..Self::default() };
    }

    pub fn to_bytes(&self) -> [u8; STATE_BYTES] {
        let mut out = [0u8; STATE_BYTES];
        let mut w = Writer { buf: &mut out, at: 0 };
        w.bytes(&STATE_MAGIC);
        w.u8(VERSION);
        w.u8(0);
        w.u32(self.epoch);
        w.u64(self.snapshot.committed_graphemes);
        for c in [self.snapshot.mimimo, self.snapshot.momo] {
            w.u64(c.pets);
            w.u64(c.falls);
            w.u64(c.plays);
        }
        let d = self.daily;
        w.u32(d.day);
        w.u32(d.graphemes);
        w.u8(u8::from(d.greeted));
        w.u8(d.spontaneous);
        w.u8(d.purrs[0]);
        w.u8(d.purrs[1]);
        w.u8(d.duets);
        w.bytes(&[0; 3]);
        for m in self.book.slots() {
            w.u32(m.day);
            w.u8(m.size);
            w.u8(m.who);
            w.u8(m.part);
        }
        let sum = fnv1a(&w.buf[..STATE_BYTES - 4]);
        w.u32(sum);
        debug_assert_eq!(w.at, STATE_BYTES);
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CodecError> {
        let (mut r, _) = Reader::open(bytes, STATE_MAGIC, STATE_BYTES)?;
        let epoch = r.u32();
        let committed_graphemes = r.u64();
        let mut contacts = [Contacts::default(); 2];
        for c in &mut contacts {
            *c = Contacts { pets: r.u64(), falls: r.u64(), plays: r.u64() };
        }
        let day = r.u32();
        let graphemes = r.u32();
        let flags = r.u8();
        let daily = Daily {
            day,
            graphemes,
            greeted: flags & 1 != 0,
            spontaneous: r.u8(),
            purrs: [r.u8(), r.u8()],
            duets: r.u8(),
        };
        if flags > 1 || r.take(3) != [0; 3] {
            return Err(CodecError::Corrupt);
        }
        let mut slots = [Mark::default(); BOOK_DAYS];
        for slot in &mut slots {
            *slot = Mark { day: r.u32(), size: r.u8(), who: r.u8(), part: r.u8() };
        }
        let book = PawBook::from_slots(slots).ok_or(CodecError::Corrupt)?;
        Ok(Self { epoch, snapshot: Snapshot { committed_graphemes, mimimo: contacts[0], momo: contacts[1] }, daily, book })
    }
}

impl Delta {
    pub fn to_bytes(&self) -> [u8; DELTA_BYTES] {
        let mut out = [0u8; DELTA_BYTES];
        let mut w = Writer { buf: &mut out, at: 0 };
        w.bytes(&DELTA_MAGIC);
        w.u8(VERSION);
        w.u8(0);
        w.u32(self.epoch);
        w.u64(self.graphemes);
        for c in self.contacts {
            w.u64(c.pets);
            w.u64(c.falls);
            w.u64(c.plays);
        }
        w.u32(self.day);
        w.u32(self.day_graphemes);
        w.u8(u8::from(self.greeted));
        w.u8(self.spontaneous);
        w.u8(self.purrs[0]);
        w.u8(self.purrs[1]);
        w.u8(self.duets);
        w.u8(self.who);
        w.u8(self.part);
        w.u8(0);
        let sum = fnv1a(&w.buf[..DELTA_BYTES - 4]);
        w.u32(sum);
        debug_assert_eq!(w.at, DELTA_BYTES);
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CodecError> {
        let (mut r, _) = Reader::open(bytes, DELTA_MAGIC, DELTA_BYTES)?;
        let epoch = r.u32();
        let graphemes = r.u64();
        let mut contacts = [Contacts::default(); 2];
        for c in &mut contacts {
            *c = Contacts { pets: r.u64(), falls: r.u64(), plays: r.u64() };
        }
        let day = r.u32();
        let day_graphemes = r.u32();
        let flags = r.u8();
        let delta = Self {
            epoch,
            graphemes,
            contacts,
            day,
            day_graphemes,
            greeted: flags & 1 != 0,
            spontaneous: r.u8(),
            purrs: [r.u8(), r.u8()],
            duets: r.u8(),
            who: r.u8(),
            part: r.u8(),
        };
        if flags > 1 || r.u8() != 0 || delta.who > 3 || delta.part > crate::book::PART_BITS {
            return Err(CodecError::Corrupt);
        }
        Ok(delta)
    }
}

const fn fnv1a(bytes: &[u8]) -> u32 {
    let mut hash = 0x811C_9DC5u32;
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u32;
        hash = hash.wrapping_mul(0x0100_0193);
        i += 1;
    }
    hash
}

struct Writer<'a> {
    buf: &'a mut [u8],
    at: usize,
}

impl Writer<'_> {
    fn bytes(&mut self, b: &[u8]) {
        self.buf[self.at..self.at + b.len()].copy_from_slice(b);
        self.at += b.len();
    }
    fn u8(&mut self, v: u8) {
        self.bytes(&[v]);
    }
    fn u32(&mut self, v: u32) {
        self.bytes(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }
}

/// Reads from a buffer whose length was checked up front, so no read is out of bounds.
struct Reader<'a> {
    buf: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn open(bytes: &'a [u8], magic: [u8; 2], len: usize) -> Result<(Self, u8), CodecError> {
        if bytes.len() < 4 || bytes[..2] != magic {
            return Err(CodecError::Corrupt);
        }
        if bytes[2] != VERSION {
            return Err(CodecError::Version(bytes[2]));
        }
        if bytes.len() != len || bytes[3] != 0 {
            return Err(CodecError::Corrupt);
        }
        let stored = u32::from_le_bytes([bytes[len - 4], bytes[len - 3], bytes[len - 2], bytes[len - 1]]);
        if stored != fnv1a(&bytes[..len - 4]) {
            return Err(CodecError::Corrupt);
        }
        Ok((Self { buf: &bytes[..len - 4], at: 4 }, bytes[2]))
    }
    fn take(&mut self, n: usize) -> &'a [u8] {
        // Lengths are fixed and checked in `open`; `get` keeps this panic-free regardless.
        let out = self.buf.get(self.at..self.at + n).unwrap_or(&[]);
        self.at += n;
        out
    }
    fn u8(&mut self) -> u8 {
        self.take(1).first().copied().unwrap_or(0)
    }
    fn u32(&mut self) -> u32 {
        let mut b = [0u8; 4];
        if let Some(src) = self.buf.get(self.at..self.at + 4) {
            b.copy_from_slice(src);
        }
        self.at += 4;
        u32::from_le_bytes(b)
    }
    fn u64(&mut self) -> u64 {
        let mut b = [0u8; 8];
        if let Some(src) = self.buf.get(self.at..self.at + 8) {
            b.copy_from_slice(src);
        }
        self.at += 8;
        u64::from_le_bytes(b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::book::{PART_BITS, WHO_MIMIMO, WHO_MOMO};

    /// SplitMix64: a tiny deterministic generator for property tests.
    pub(crate) struct Gen(u64);

    impl Gen {
        pub(crate) fn new(seed: u64) -> Self {
            Self(seed)
        }
        pub(crate) fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }
        pub(crate) fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    fn contacts(g: &mut Gen) -> Contacts {
        Contacts { pets: g.below(500), falls: g.below(500), plays: g.below(500) }
    }

    /// Small ranges so that equal epochs, equal days and shared slots are common.
    fn state(g: &mut Gen) -> PetState {
        let mut s = PetState {
            epoch: g.below(2) as u32,
            snapshot: Snapshot { committed_graphemes: g.below(10_000), mimimo: contacts(g), momo: contacts(g) },
            daily: Daily {
                day: 20_000 + g.below(3) as u32,
                graphemes: g.below(5000) as u32,
                greeted: g.below(2) == 1,
                spontaneous: g.below(4) as u8,
                purrs: [g.below(13) as u8, g.below(13) as u8],
                duets: g.below(3) as u8,
            },
            book: PawBook::default(),
        };
        for _ in 0..g.below(30) {
            let day = 19_500 + g.below(1200) as u32;
            s.book.record(day, g.below(5) as u8, g.below(4) as u8, g.below(32) as u8);
        }
        s
    }

    fn delta(g: &mut Gen) -> Delta {
        Delta {
            epoch: g.below(2) as u32,
            graphemes: g.below(1000),
            contacts: [contacts(g), contacts(g)],
            day: if g.below(5) == 0 { 0 } else { 20_000 + g.below(3) as u32 },
            day_graphemes: g.below(2000) as u32,
            greeted: g.below(2) == 1,
            spontaneous: g.below(2) as u8,
            purrs: [g.below(3) as u8, g.below(3) as u8],
            duets: g.below(2) as u8,
            who: g.below(4) as u8,
            part: g.below(32) as u8,
        }
    }

    fn merged(a: &PetState, b: &PetState) -> PetState {
        let mut out = a.clone();
        out.merge(b);
        out
    }

    #[test]
    fn merge_is_commutative_associative_and_idempotent() {
        let mut g = Gen::new(0xC0FFEE);
        for _ in 0..3000 {
            let (a, b, c) = (state(&mut g), state(&mut g), state(&mut g));
            assert_eq!(merged(&a, &b), merged(&b, &a));
            assert_eq!(merged(&merged(&a, &b), &c), merged(&a, &merged(&b, &c)));
            assert_eq!(merged(&a, &a), a);
            let ab = merged(&a, &b);
            assert_eq!(merged(&ab, &b), ab, "absorbs a repeated merge");
            // Merging never lowers a counter of the winning epoch.
            if a.epoch == ab.epoch {
                assert!(ab.snapshot.committed_graphemes >= a.snapshot.committed_graphemes);
                assert!(ab.snapshot.momo.plays >= a.snapshot.momo.plays);
            }
        }
    }

    #[test]
    fn bytes_round_trip_and_merge_commutes_with_the_codec() {
        let mut g = Gen::new(42);
        for _ in 0..1000 {
            let (a, b) = (state(&mut g), state(&mut g));
            let bytes = a.to_bytes();
            assert_eq!(bytes.len(), STATE_BYTES);
            assert_eq!(PetState::from_bytes(&bytes), Ok(a.clone()));
            let ab = merged(&PetState::from_bytes(&a.to_bytes()).unwrap(), &PetState::from_bytes(&b.to_bytes()).unwrap());
            assert_eq!(ab, merged(&a, &b));
            let d = delta(&mut g);
            assert_eq!(Delta::from_bytes(&d.to_bytes()), Ok(d));
        }
    }

    /// Two processes flushing deltas against one file: lifetime counters end
    /// up as the sum, in either flush order.
    #[test]
    fn concurrent_deltas_add_up_in_any_order() {
        let mut g = Gen::new(7);
        for _ in 0..500 {
            let base = PetState { epoch: 0, ..state(&mut g) };
            let (mut d1, mut d2) = (delta(&mut g), delta(&mut g));
            d1.epoch = 0;
            d2.epoch = 0;
            let mut x = base.clone();
            x.apply_delta(&d1);
            x.apply_delta(&d2);
            let mut y = base.clone();
            y.apply_delta(&d2);
            y.apply_delta(&d1);
            // Lifetime counters never depend on the order. The day's mark band
            // can when two deltas straddle midnight (only bands are stored).
            assert_eq!(x.snapshot, y.snapshot);
            if d1.day == d2.day {
                assert_eq!(x, y);
            }
            assert_eq!(x.snapshot.committed_graphemes, base.snapshot.committed_graphemes + d1.graphemes + d2.graphemes);
            assert_eq!(x.snapshot.momo.pets, base.snapshot.momo.pets + d1.contacts[1].pets + d2.contacts[1].pets);
        }
    }

    #[test]
    fn flush_protocol_keeps_both_processes_increments() {
        let base = PetState::default();
        let mut disk = base.clone();
        let (mut a, mut b) = (base.clone(), base.clone());
        let mut da = Delta { graphemes: 10, day: 20_000, day_graphemes: 10, who: WHO_MIMIMO, part: 1, ..Delta::default() };
        da.contacts[0].pets = 2;
        let mut db = Delta { graphemes: 5, day: 20_000, day_graphemes: 5, who: WHO_MOMO, part: 2, ..Delta::default() };
        db.contacts[0].pets = 1;
        a.apply_delta(&da);
        b.apply_delta(&db);
        for (live, d) in [(&mut a, da), (&mut b, db)] {
            disk.apply_delta(&d);
            disk.merge(live);
            live.merge(&disk);
        }
        assert_eq!(disk.snapshot.committed_graphemes, 15);
        assert_eq!(disk.snapshot.mimimo.pets, 3);
        assert_eq!(disk.daily.graphemes, 15);
        assert_eq!(disk.book.get(20_000), Some(Mark { day: 20_000, size: 0, who: 3, part: 3 }));
        assert_eq!(b, disk);
    }

    #[test]
    fn clear_wins_over_older_copies() {
        let mut g = Gen::new(9);
        let old = PetState { epoch: 1, ..state(&mut g) };
        let mut cleared = old.clone();
        cleared.clear();
        assert_eq!(cleared.epoch, 2);
        assert!(cleared.book.is_empty());
        assert_eq!(merged(&cleared, &old), cleared);
        assert_eq!(merged(&old, &cleared), cleared);
        let mut d = delta(&mut g);
        d.epoch = 1;
        let mut after = cleared.clone();
        after.apply_delta(&d);
        assert_eq!(after, cleared, "a delta from before the clear is dropped");
    }

    #[test]
    fn delta_for_an_older_day_marks_the_book_but_not_today() {
        let mut s = PetState { daily: Daily { day: 20_010, spontaneous: 1, ..Daily::default() }, ..PetState::default() };
        s.apply_delta(&Delta { day: 20_009, day_graphemes: 600, spontaneous: 2, who: WHO_MOMO, part: 4, ..Delta::default() });
        assert_eq!(s.daily.spontaneous, 1);
        assert_eq!(s.book.get(20_009), Some(Mark { day: 20_009, size: 2, who: WHO_MOMO, part: 4 }));
        s.apply_delta(&Delta { day: 20_011, purrs: [3, 0], ..Delta::default() });
        assert_eq!(s.daily, Daily { day: 20_011, purrs: [3, 0], ..Daily::default() });
        assert_eq!(s.book.get(20_011), None, "no activity, no mark");
    }

    #[test]
    fn from_bytes_never_panics_on_truncated_or_random_bytes() {
        let mut g = Gen::new(1234);
        let good = state(&mut g).to_bytes();
        for len in 0..good.len() {
            assert_eq!(PetState::from_bytes(&good[..len]), Err(CodecError::Corrupt), "truncated to {len}");
        }
        let mut longer = good.to_vec();
        longer.push(0);
        assert_eq!(PetState::from_bytes(&longer), Err(CodecError::Corrupt));
        for _ in 0..20_000 {
            let len = match g.below(4) {
                0 => STATE_BYTES,
                1 => DELTA_BYTES,
                _ => g.below(STATE_BYTES as u64 + 10) as usize,
            };
            let mut bytes: Vec<u8> = (0..len).map(|_| g.next() as u8).collect();
            if g.below(2) == 0 && len >= 4 {
                bytes[..4].copy_from_slice(&[0x9C, 0xA7, 1, 0]);
            }
            let _ = PetState::from_bytes(&bytes);
            let _ = Delta::from_bytes(&bytes);
            let loaded = PetState::load(&bytes);
            assert!(loaded.book.len() <= BOOK_DAYS);
        }
        // Single flipped bits are caught by the checksum or the range checks.
        let mut flipped = good;
        for i in 0..STATE_BYTES {
            for bit in 0..8 {
                flipped[i] ^= 1 << bit;
                assert!(PetState::from_bytes(&flipped).is_err(), "byte {i} bit {bit}");
                flipped[i] ^= 1 << bit;
            }
        }
        assert!(PetState::from_bytes(&flipped).is_ok());
    }

    #[test]
    fn unknown_version_starts_fresh() {
        let mut bytes = PetState { epoch: 3, ..PetState::default() }.to_bytes();
        bytes[2] = 2;
        assert_eq!(PetState::from_bytes(&bytes), Err(CodecError::Version(2)));
        assert_eq!(PetState::from_bytes(&bytes[..10]), Err(CodecError::Version(2)));
        assert_eq!(PetState::load(&bytes), PetState::default());
        let mut d = Delta::default().to_bytes();
        d[2] = 9;
        assert_eq!(Delta::from_bytes(&d), Err(CodecError::Version(9)));
    }

    /// The stored form is the same length whatever was typed, and contains no text.
    #[test]
    fn privacy_fixed_length_and_no_strings() {
        let mut lengths = std::collections::BTreeSet::new();
        let typed = ["你好，今天天氣很好", "hello there, password123", "ねこがすき"];
        for (i, text) in typed.iter().enumerate() {
            let mut s = PetState::default();
            let n = text.chars().count() as u64 * (i as u64 + 1) * 37;
            s.apply_delta(&Delta { graphemes: n, day: 20_400 + i as u32, day_graphemes: n as u32, who: 3, part: PART_BITS, ..Delta::default() });
            let bytes = s.to_bytes();
            lengths.insert(bytes.len());
            for needle in [text.as_bytes(), b"mimimo".as_slice(), b"momo".as_slice()] {
                assert!(!bytes.windows(needle.len()).any(|w| w == needle));
            }
            let longest_ascii_run = bytes
                .split(|b| !(0x20..0x7F).contains(b))
                .map(<[u8]>::len)
                .max()
                .unwrap_or(0);
            assert!(longest_ascii_run < 4, "printable run of {longest_ascii_run}");
        }
        lengths.insert(PetState::default().to_bytes().len());
        assert_eq!(lengths.into_iter().collect::<Vec<_>>(), vec![STATE_BYTES]);
    }
}
