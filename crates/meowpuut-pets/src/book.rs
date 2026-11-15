//! The paw book: at most one mark per local day, for the last [`BOOK_DAYS`] days.
//!
//! A mark holds only the local day index, a size band (0–4) of the words
//! committed that day, which cats were there and which parts of the day had
//! activity. No text, no times of day beyond the five day parts.
//!
//! The book is a ring indexed by `day % BOOK_DAYS`. Joining two books slot by
//! slot (the later day wins, the same day combines field by field with max/OR)
//! is commutative, associative and idempotent, so copies kept by several
//! processes can be merged in any order.

/// Days kept in the ring.
pub const BOOK_DAYS: usize = 400;

/// Who bits: Mimimo.
pub const WHO_MIMIMO: u8 = 1;
/// Who bits: Momo.
pub const WHO_MOMO: u8 = 2;
/// Largest size band.
pub const MAX_SIZE: u8 = 4;
/// All day-part bits (`1 << DayPart as u8`, five parts).
pub const PART_BITS: u8 = 0b1_1111;

/// One local day. `day == 0` marks an empty slot (1970-01-01 is never recorded).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct Mark {
    /// Local day index (days since 1970-01-01 in the device's time zone).
    pub day: u32,
    /// Committed-words band 0–4 (see [`size_band`]).
    pub size: u8,
    /// [`WHO_MIMIMO`] | [`WHO_MOMO`].
    pub who: u8,
    /// Day parts with activity, `1 << DayPart`.
    pub part: u8,
}

impl Mark {
    pub const fn is_empty(&self) -> bool {
        self.day == 0
    }

    /// Lattice join: the later day wins; the same day combines.
    #[must_use]
    pub fn join(self, other: Self) -> Self {
        match self.day.cmp(&other.day) {
            core::cmp::Ordering::Greater => self,
            core::cmp::Ordering::Less => other,
            core::cmp::Ordering::Equal => Self {
                day: self.day,
                size: self.size.max(other.size),
                who: self.who | other.who,
                part: self.part | other.part,
            },
        }
    }

    /// A mark as stored: fields clamped to their ranges.
    pub(crate) fn valid(&self) -> bool {
        if self.day == 0 {
            return self.size == 0 && self.who == 0 && self.part == 0;
        }
        self.size <= MAX_SIZE && self.who <= (WHO_MIMIMO | WHO_MOMO) && self.part <= PART_BITS
    }
}

/// Size band of the graphemes committed on one day. Bands, not counts, are
/// what the book shows ("a few words", "quite a lot").
pub const fn size_band(graphemes: u32) -> u8 {
    match graphemes {
        0..=99 => 0,
        100..=499 => 1,
        500..=1499 => 2,
        1500..=3999 => 3,
        _ => 4,
    }
}

/// Ring of marks; see the module documentation.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct PawBook {
    slots: [Mark; BOOK_DAYS],
}

impl Default for PawBook {
    fn default() -> Self {
        Self { slots: [Mark::default(); BOOK_DAYS] }
    }
}

impl PawBook {
    /// Adds activity to `day` (joined into its slot). Day 0 is ignored, and a
    /// slot already holding a later day keeps it.
    pub fn record(&mut self, day: u32, size: u8, who: u8, part: u8) {
        if day == 0 {
            return;
        }
        let mark = Mark {
            day,
            size: size.min(MAX_SIZE),
            who: who & (WHO_MIMIMO | WHO_MOMO),
            part: part & PART_BITS,
        };
        let slot = &mut self.slots[day as usize % BOOK_DAYS];
        *slot = slot.join(mark);
    }

    /// The mark for `day`, if the ring still holds it.
    pub fn get(&self, day: u32) -> Option<Mark> {
        let mark = self.slots[day as usize % BOOK_DAYS];
        (day != 0 && mark.day == day).then_some(mark)
    }

    /// Joins another book into this one, slot by slot.
    pub fn merge(&mut self, other: &Self) {
        for (mine, theirs) in self.slots.iter_mut().zip(other.slots.iter()) {
            *mine = mine.join(*theirs);
        }
    }

    /// Non-empty marks, oldest day first.
    pub fn marks(&self) -> impl Iterator<Item = Mark> + '_ {
        let mut start = 0;
        let mut latest = 0;
        for (i, mark) in self.slots.iter().enumerate() {
            if mark.day > latest {
                latest = mark.day;
                start = i;
            }
        }
        // The slot after the latest day holds the oldest kept day.
        (1..=BOOK_DAYS).map(move |k| self.slots[(start + k) % BOOK_DAYS]).filter(|m| !m.is_empty())
    }

    pub fn len(&self) -> usize {
        self.slots.iter().filter(|m| !m.is_empty()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(crate) fn slots(&self) -> &[Mark; BOOK_DAYS] {
        &self.slots
    }

    /// Builds a book from stored slots; `None` unless every slot is valid and
    /// sits at `day % BOOK_DAYS`.
    pub(crate) fn from_slots(slots: [Mark; BOOK_DAYS]) -> Option<Self> {
        let ok = slots
            .iter()
            .enumerate()
            .all(|(i, m)| m.valid() && (m.day == 0 || m.day as usize % BOOK_DAYS == i));
        ok.then_some(Self { slots })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_cover_every_count() {
        assert_eq!([0, 99, 100, 499, 500, 1499, 1500, 3999, 4000, u32::MAX].map(size_band), [0, 0, 1, 1, 2, 2, 3, 3, 4, 4]);
    }

    #[test]
    fn ring_keeps_the_last_400_days_in_order() {
        let mut book = PawBook::default();
        assert!(book.is_empty());
        for day in 20_000..20_500 {
            book.record(day, (day % 5) as u8, WHO_MOMO, 1);
        }
        assert_eq!(book.len(), BOOK_DAYS);
        let days: Vec<u32> = book.marks().map(|m| m.day).collect();
        assert_eq!(days, (20_100..20_500).collect::<Vec<_>>());
        assert_eq!(book.get(20_099), None);
        assert_eq!(book.get(20_499).map(|m| m.size), Some(4));
        // An older day never overwrites a newer one in its slot.
        book.record(20_099, 4, WHO_MIMIMO, 2);
        assert_eq!(book.get(20_499).map(|m| m.who), Some(WHO_MOMO));
        assert_eq!(book.get(20_099), None);
        // The same day combines.
        book.record(20_499, 0, WHO_MIMIMO, 4);
        assert_eq!(book.get(20_499), Some(Mark { day: 20_499, size: 4, who: 3, part: 5 }));
        book.record(0, 4, 3, 3);
        assert_eq!(book.len(), BOOK_DAYS);
    }

    #[test]
    fn record_clamps_fields() {
        let mut book = PawBook::default();
        book.record(7, 200, 0xff, 0xff);
        assert_eq!(book.get(7), Some(Mark { day: 7, size: 4, who: 3, part: PART_BITS }));
        assert!(PawBook::from_slots(*book.slots()).is_some());
        let mut slots = *book.slots();
        slots[8] = Mark { day: 7, size: 0, who: 0, part: 0 };
        assert!(PawBook::from_slots(slots).is_none());
    }
}
