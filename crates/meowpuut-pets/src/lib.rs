#![forbid(unsafe_code)]
//! Device-local, platform-neutral pet rules. Clients supply grapheme counts
//! and completed gestures, then decide when to persist the numeric snapshot.

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
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Contacts {
    pub pets: u64,
    pub falls: u64,
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

    fn contacts_mut(&mut self, pet: Pet) -> &mut Contacts {
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
            mimimo: Contacts { pets: 1, falls: 1 },
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
        assert_eq!(state.contacts(Pet::Momo), Contacts { pets: 0, falls: 1 });
    }

    #[test]
    fn progress_caps_and_counters_saturate_even_for_imported_totals() {
        let mut state = Snapshot {
            committed_graphemes: u64::MAX - 1,
            momo: Contacts { pets: u64::MAX, falls: u64::MAX },
            ..Snapshot::default()
        };
        assert_eq!(state.apply(Event::CommittedGraphemes(3)), LevelChanges::default());
        state.apply(Event::Petted(Pet::Momo));
        assert_eq!(state.committed_graphemes, u64::MAX);
        assert_eq!(state.momo.pets, u64::MAX);
        assert_eq!(state.progress(Pet::Momo), Progress { level: MAX_LEVEL, next: None });
    }
}
