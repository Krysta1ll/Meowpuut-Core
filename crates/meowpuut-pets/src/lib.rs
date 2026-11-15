#![forbid(unsafe_code)]
//! Device-local, platform-neutral pet rules. Clients supply grapheme counts
//! and completed gestures, then decide when to persist the numeric snapshot.
//!
//! - [`Snapshot`]: lifetime counters and the level rule (unchanged since 0.8).
//! - [`moments::Director`]: decides the small moments (greeting, purr,
//!   peekaboo, doze, duet, flow) from a clock and a seed passed in.
//! - [`book::PawBook`]: one mark per local day, the last 400 days.
//! - [`codec`]: the persisted state, its versioned binary form, `Delta` and `merge`.
//!
//! Nothing here reads a clock, a random source or a file.

pub mod book;
pub mod codec;
pub mod moments;

const GRAPHEMES_PER_LEVEL: u64 = 200;
const CONTACTS_PER_LEVEL: u64 = 3;
pub const MAX_LEVEL: u8 = 100;

/// Stable identity, independent of pose, costume or keyboard skin.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Pet {
    Mimimo,
    Momo,
}

impl Pet {
    pub const fn key(self) -> &'static str {
        match self {
            Self::Mimimo => "mimimo",
            Self::Momo => "momo",
        }
    }
}

/// Both contact types keep the meaning of existing mobile counters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event {
    CommittedGraphemes(u64),
    Petted(Pet),
    DropAccepted(Pet),
    /// A shared moment (purr, found, duet) that earned a reward. Counted in
    /// [`Contacts::plays`]; it does not change the level.
    Played(Pet),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Contacts {
    pub pets: u64,
    pub falls: u64,
    /// Rewarded moments. Not part of the level formula, so a snapshot
    /// imported from the old counters (plays = 0) keeps exactly its level.
    pub plays: u64,
}

impl Contacts {
    fn total(self) -> u64 {
        self.pets.saturating_add(self.falls)
    }
}

/// The full state can be restored from the existing Android or iOS counters.
/// It contains no text, timestamps, application identifiers, or user identifiers.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Snapshot {
    pub committed_graphemes: u64,
    pub mimimo: Contacts,
    pub momo: Contacts,
}

impl Snapshot {
    pub const fn contacts(&self, pet: Pet) -> Contacts {
        match pet {
            Pet::Mimimo => self.mimimo,
            Pet::Momo => self.momo,
        }
    }

    pub(crate) fn contacts_mut(&mut self, pet: Pet) -> &mut Contacts {
        match pet {
            Pet::Mimimo => &mut self.mimimo,
            Pet::Momo => &mut self.momo,
        }
    }

    /// Apply exactly one completed event. The UI can react to a level change
    /// without fetching a second snapshot or depending on an animation.
    pub fn apply(&mut self, event: Event) -> LevelChanges {
        let before = [self.progress(Pet::Mimimo).level, self.progress(Pet::Momo).level];
        match event {
            Event::CommittedGraphemes(count) => {
                self.committed_graphemes = self.committed_graphemes.saturating_add(count);
            }
            Event::Petted(pet) => {
                let contacts = self.contacts_mut(pet);
                contacts.pets = contacts.pets.saturating_add(1);
            }
            Event::DropAccepted(pet) => {
                let contacts = self.contacts_mut(pet);
                contacts.falls = contacts.falls.saturating_add(1);
            }
            Event::Played(pet) => {
                let contacts = self.contacts_mut(pet);
                contacts.plays = contacts.plays.saturating_add(1);
            }
        }
        let changed = |pet, was| {
            let now = self.progress(pet).level;
            (was != now).then_some(now)
        };
        LevelChanges {
            mimimo: changed(Pet::Mimimo, before[0]),
            momo: changed(Pet::Momo, before[1]),
        }
    }

    pub fn progress(&self, pet: Pet) -> Progress {
        let total = self.contacts(pet).total();
        let level = (self.committed_graphemes / GRAPHEMES_PER_LEVEL + total / CONTACTS_PER_LEVEL)
            .min(u64::from(MAX_LEVEL)) as u8;
        let next = (level < MAX_LEVEL).then_some(NextLevel {
            graphemes: (GRAPHEMES_PER_LEVEL - self.committed_graphemes % GRAPHEMES_PER_LEVEL) as u16,
            contacts: (CONTACTS_PER_LEVEL - total % CONTACTS_PER_LEVEL) as u8,
        });
        Progress { level, next }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LevelChanges {
    pub mimimo: Option<u8>,
    pub momo: Option<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Progress {
    pub level: u8,
    pub next: Option<NextLevel>,
}

/// Either route is enough to reach the next level; neither is a requirement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NextLevel {
    pub graphemes: u16,
    pub contacts: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_totals_keep_their_level_and_cats_grow_independently() {
        let mut state = Snapshot {
            committed_graphemes: 199,
            mimimo: Contacts { pets: 1, falls: 1, plays: 0 },
            ..Snapshot::default()
        };
        assert_eq!(
            state.progress(Pet::Mimimo),
            Progress {
                level: 0,
                next: Some(NextLevel { graphemes: 1, contacts: 1 }),
            }
        );
        assert_eq!(
            state.apply(Event::Petted(Pet::Mimimo)),
            LevelChanges { mimimo: Some(1), momo: None }
        );
        assert_eq!(
            state.apply(Event::CommittedGraphemes(1)),
            LevelChanges { mimimo: Some(2), momo: Some(1) }
        );
        assert_eq!(state.apply(Event::DropAccepted(Pet::Momo)), LevelChanges::default());
        assert_eq!(state.contacts(Pet::Momo), Contacts { pets: 0, falls: 1, plays: 0 });
    }

    #[test]
    fn progress_caps_and_counters_saturate_even_for_imported_totals() {
        let mut state = Snapshot {
            committed_graphemes: u64::MAX - 1,
            momo: Contacts { pets: u64::MAX, falls: u64::MAX, plays: 0 },
            ..Snapshot::default()
        };
        assert_eq!(state.apply(Event::CommittedGraphemes(3)), LevelChanges::default());
        state.apply(Event::Petted(Pet::Momo));
        assert_eq!(state.committed_graphemes, u64::MAX);
        assert_eq!(state.momo.pets, u64::MAX);
        assert_eq!(state.progress(Pet::Momo), Progress { level: MAX_LEVEL, next: None });
    }

    /// The old level rule, written out independently of `progress`.
    fn legacy_level(graphemes: u64, pets: u64, falls: u64) -> u8 {
        (graphemes / 200 + pets.saturating_add(falls) / 3).min(100) as u8
    }

    #[test]
    fn plays_never_change_the_level_and_legacy_imports_keep_theirs() {
        let mut seed = 0x5eed_u64;
        for _ in 0..2000 {
            let mut next = || {
                seed = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
                let mut z = seed;
                z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
                (z ^ (z >> 31)) % 50_000
            };
            let (g, p, f) = (next(), next() % 400, next() % 400);
            let legacy = Snapshot { committed_graphemes: g, mimimo: Contacts { pets: p, falls: f, plays: 0 }, ..Snapshot::default() };
            assert_eq!(legacy.progress(Pet::Mimimo).level, legacy_level(g, p, f));
            let mut played = legacy;
            for _ in 0..(next() % 50) {
                assert_eq!(played.apply(Event::Played(Pet::Mimimo)), LevelChanges::default());
            }
            assert_eq!(played.progress(Pet::Mimimo), legacy.progress(Pet::Mimimo));
            // The persisted form carries the old counters unchanged.
            let state = codec::PetState::from_snapshot(legacy);
            let back = codec::PetState::from_bytes(&state.to_bytes()).unwrap();
            assert_eq!(back.snapshot, legacy);
            assert_eq!(back.snapshot.progress(Pet::Momo).level, legacy_level(g, 0, 0));
        }
    }
}
