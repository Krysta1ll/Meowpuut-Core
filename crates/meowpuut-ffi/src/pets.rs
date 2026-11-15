//! C ABI of the pet rules (`mp_pets_*`, see the pets block of `include/meowpuut.h`).
//!
//! One opaque [`MPPets`] handle per process holds the persisted state, the
//! increments not yet flushed and the session memory of the moment director.
//! A handle is used on one thread. Every function tolerates NULL and returns
//! 0 (or does nothing) for it; invalid enum values plan nothing.
//!
//! # Examples
//!
//! ```
//! use meowpuut_ffi::*;
//! # #[allow(unsafe_code)]
//! // SAFETY: pointers to live locals; the handle is freed once.
//! unsafe {
//!     let pets = mp_pets_from_bytes(std::ptr::null(), 0, std::ptr::null_mut());
//!     let ambience = MPPetsAmbience { flags: MP_PETS_GLASS_SEAT | MP_PETS_EDGES, quiet_ms: u32::MAX, mode: MP_PETS_MODE_FULL, cats: 3 };
//!     let clock = MPPetsClock { mono_ms: 1_000, day: 20_406, minute: 720 };
//!     let mut plan = MPPetsPlan::default();
//!     assert_eq!(mp_pets_decide(pets, MP_PETS_APPEAR, 0, &ambience, &clock, 7, &mut plan), 1);
//!     assert_eq!(u32::from(plan.moment), MP_PETS_HELLO);
//!     let mut bytes = vec![0u8; mp_pets_to_bytes(pets, std::ptr::null_mut(), 0)];
//!     assert_eq!(mp_pets_to_bytes(pets, bytes.as_mut_ptr(), bytes.len()), MP_PETS_STATE_BYTES);
//!     mp_pets_free(pets);
//! }
//! ```

use std::panic::{catch_unwind, AssertUnwindSafe};

use meowpuut_pets::codec::{CodecError, Delta, PetState, DELTA_BYTES, STATE_BYTES};
use meowpuut_pets::moments::{rest_pose, Ambience, Clock, Director, Mode, Outcome, TouchKind, Trigger};
use meowpuut_pets::{Contacts, Pet, Snapshot};

/// Opaque handle (`MPPets` in C).
pub struct MPPets {
    director: Director,
}

/// Serialized state length (`MP_PETS_STATE_BYTES`).
pub const MP_PETS_STATE_BYTES: usize = STATE_BYTES;
/// Serialized delta length (`MP_PETS_DELTA_BYTES`).
pub const MP_PETS_DELTA_BYTES: usize = DELTA_BYTES;

/// Cat bits.
pub const MP_PETS_MIMIMO: u32 = 1;
/// Cat bits.
pub const MP_PETS_MOMO: u32 = 2;

/// Triggers.
pub const MP_PETS_APPEAR: u32 = 1;
/// `arg` = graphemes committed.
pub const MP_PETS_COMMIT: u32 = 2;
/// The idle timer fired.
pub const MP_PETS_IDLE: u32 = 3;
/// `arg` = cat.
pub const MP_PETS_TAP: u32 = 4;
/// `arg` = cat.
pub const MP_PETS_HOLD: u32 = 5;
/// `arg` ignored.
pub const MP_PETS_SEAT: u32 = 6;
/// `arg` = cat.
pub const MP_PETS_DROP: u32 = 7;
/// Keyboard hidden.
pub const MP_PETS_DISAPPEAR: u32 = 8;

/// Ambience flags.
pub const MP_PETS_COMPOSING: u32 = 1;
/// A cat is being dragged.
pub const MP_PETS_LIFTED: u32 = 2;
/// The keyboard is being dismissed.
pub const MP_PETS_DISMISSING: u32 = 4;
/// VoiceOver, Switch Control or TalkBack.
pub const MP_PETS_ASSISTIVE: u32 = 8;
/// Lightweight effects level.
pub const MP_PETS_LIGHTWEIGHT: u32 = 16;
/// Reduce Motion.
pub const MP_PETS_REDUCE_MOTION: u32 = 32;
/// Reduce Transparency or Increase Contrast.
pub const MP_PETS_REDUCE_TRANSPARENCY: u32 = 64;
/// Cats sit behind the glass bar (sinking works).
pub const MP_PETS_GLASS_SEAT: u32 = 128;
/// Edge seating.
pub const MP_PETS_EDGES: u32 = 256;
/// Festival outfit showing.
pub const MP_PETS_FESTIVAL: u32 = 512;

/// Modes.
pub const MP_PETS_MODE_FULL: u32 = 0;
/// Only what the user starts, plus greeting and doze.
pub const MP_PETS_MODE_QUIET: u32 = 1;
/// No plans.
pub const MP_PETS_MODE_OFF: u32 = 2;

/// Moments (`MPPetsPlan::moment`).
pub const MP_PETS_HELLO: u32 = 1;
/// Purr.
pub const MP_PETS_PURR: u32 = 2;
/// Peekaboo.
pub const MP_PETS_PEEKABOO: u32 = 3;
/// Found.
pub const MP_PETS_FOUND: u32 = 4;
/// Doze.
pub const MP_PETS_DOZE: u32 = 5;
/// Duet.
pub const MP_PETS_DUET: u32 = 6;
/// Flow.
pub const MP_PETS_FLOW: u32 = 7;

/// Outcomes for `mp_pets_resolve`.
pub const MP_PETS_COMPLETED: u32 = 0;
/// Cut short.
pub const MP_PETS_INTERRUPTED: u32 = 1;
/// Never shown.
pub const MP_PETS_CANCELLED: u32 = 2;

/// `MPPetsPlan::style`: full motion.
pub const MP_PETS_STYLE_MOTION: u32 = 0;
/// Pose cross-fade only.
pub const MP_PETS_STYLE_FADE: u32 = 1;
/// `MPPetsPlan::ripple`: none.
pub const MP_PETS_RIPPLE_NONE: u32 = 0;
/// Two soft rings.
pub const MP_PETS_RIPPLE_SOFT: u32 = 1;
/// Opaque thin line.
pub const MP_PETS_RIPPLE_LINE: u32 = 2;

/// `mp_pets_rest_pose`: keep the pose.
pub const MP_PETS_REST_KEEP: u32 = 0;
/// `edge_rest`.
pub const MP_PETS_REST_EDGE_REST: u32 = 1;
/// `edge_peek`.
pub const MP_PETS_REST_EDGE_PEEK: u32 = 2;
/// `edge_lounge`.
pub const MP_PETS_REST_EDGE_LOUNGE: u32 = 3;
/// `sleep`.
pub const MP_PETS_REST_SLEEP: u32 = 4;

/// `mp_pets_from_bytes` status.
pub const MP_PETS_OK: i32 = 0;
/// NULL or empty data: fresh state.
pub const MP_PETS_FRESH: i32 = 1;
/// Damaged data: fresh state.
pub const MP_PETS_CORRUPT: i32 = 2;
/// Another format version: fresh state.
pub const MP_PETS_UNKNOWN_VERSION: i32 = 3;

/// `mp_pets_idle_delay_ms`: do not arm the timer.
pub const MP_PETS_NEVER: u32 = u32::MAX;

/// Mirrors `MPPetsClock` (16 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MPPetsClock {
    /// Monotonic milliseconds.
    pub mono_ms: u64,
    /// Local day index (days since 1970-01-01, local time zone); 0 = unknown.
    pub day: u32,
    /// Local minute of the day, 0..1440.
    pub minute: u32,
}

/// Mirrors `MPPetsAmbience` (16 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MPPetsAmbience {
    /// `MP_PETS_COMPOSING` … `MP_PETS_FESTIVAL`.
    pub flags: u32,
    /// Milliseconds since the last key press; `UINT32_MAX` when none.
    pub quiet_ms: u32,
    /// `MP_PETS_MODE_*`.
    pub mode: u32,
    /// Cats on screen: `MP_PETS_MIMIMO | MP_PETS_MOMO`.
    pub cats: u32,
}

/// Mirrors `MPPetsPlan` (16 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MPPetsPlan {
    /// For `mp_pets_resolve`; never 0.
    pub token: u32,
    /// `MP_PETS_HELLO` …
    pub moment: u8,
    /// Cat bits.
    pub who: u8,
    /// 0 motion, 1 fade only.
    pub style: u8,
    /// 0 none, 1 soft rings, 2 opaque line.
    pub ripple: u8,
    /// Main motion length.
    pub duration_ms: u32,
    /// Peekaboo: surface by itself after this long.
    pub delay_ms: u32,
}

/// Mirrors `MPPetsMark` (8 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MPPetsMark {
    /// Local day index.
    pub day: u32,
    /// Words band 0–4.
    pub size: u8,
    /// Cat bits.
    pub who: u8,
    /// Day parts, `1 << part` (0 dawn, 1 day, 2 evening, 3 night, 4 late night).
    pub part: u8,
    /// 0.
    pub reserved: u8,
}

/// Mirrors `MPPetsProgress` (40 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MPPetsProgress {
    /// 0–100.
    pub level: u32,
    /// Graphemes to the next level (0 at the cap).
    pub graphemes_to_next: u32,
    /// Pets or falls to the next level (0 at the cap).
    pub contacts_to_next: u32,
    /// 0.
    pub reserved: u32,
    /// Lifetime pets.
    pub pets: u64,
    /// Lifetime accepted falls.
    pub falls: u64,
    /// Lifetime rewarded moments (not part of the level).
    pub plays: u64,
}

fn clock(c: &MPPetsClock) -> Clock {
    Clock { mono_ms: c.mono_ms, day: c.day, minute: u16::try_from(c.minute).unwrap_or(u16::MAX) }
}

fn ambience(a: &MPPetsAmbience) -> Ambience {
    let f = |bit: u32| a.flags & bit != 0;
    Ambience {
        composing: f(MP_PETS_COMPOSING),
        quiet_ms: a.quiet_ms,
        lifted: f(MP_PETS_LIFTED),
        dismissing: f(MP_PETS_DISMISSING),
        assistive: f(MP_PETS_ASSISTIVE),
        lightweight: f(MP_PETS_LIGHTWEIGHT),
        reduce_motion: f(MP_PETS_REDUCE_MOTION),
        reduce_transparency: f(MP_PETS_REDUCE_TRANSPARENCY),
        glass_seat: f(MP_PETS_GLASS_SEAT),
        edges: f(MP_PETS_EDGES),
        festival: f(MP_PETS_FESTIVAL),
        cats: (a.cats & 3) as u8,
        // An unknown mode records events but plans nothing.
        mode: match a.mode {
            MP_PETS_MODE_FULL => Mode::Full,
            MP_PETS_MODE_QUIET => Mode::Quiet,
            _ => Mode::Off,
        },
    }
}

fn pet(value: u32) -> Option<Pet> {
    match value {
        MP_PETS_MIMIMO => Some(Pet::Mimimo),
        MP_PETS_MOMO => Some(Pet::Momo),
        _ => None,
    }
}

fn trigger(kind: u32, arg: u32) -> Option<Trigger> {
    Some(match kind {
        MP_PETS_APPEAR => Trigger::Appear,
        MP_PETS_COMMIT => Trigger::Commit(arg),
        MP_PETS_IDLE => Trigger::Idle,
        MP_PETS_TAP => Trigger::Touch(TouchKind::Tap, pet(arg)?),
        MP_PETS_HOLD => Trigger::Touch(TouchKind::Hold, pet(arg)?),
        MP_PETS_SEAT => Trigger::Touch(TouchKind::Seat, Pet::Mimimo),
        MP_PETS_DROP => Trigger::Touch(TouchKind::Drop, pet(arg)?),
        MP_PETS_DISAPPEAR => Trigger::Disappear,
        _ => return None,
    })
}

/// Borrows `size` bytes; NULL or empty gives an empty slice.
///
/// # Safety
/// `data` must be NULL or valid for reads of `size` bytes for `'a`.
#[allow(unsafe_code)]
unsafe fn bytes<'a>(data: *const u8, size: usize) -> &'a [u8] {
    if data.is_null() || size == 0 {
        return &[];
    }
    // SAFETY: non-null and readable for `size` bytes per the contract.
    unsafe { std::slice::from_raw_parts(data, size) }
}

/// Copies `src` to `out` when it fits; returns `src.len()` either way.
///
/// # Safety
/// `out` must be NULL or valid for writes of `capacity` bytes.
#[allow(unsafe_code)]
unsafe fn write_out(src: &[u8], out: *mut u8, capacity: usize) -> bool {
    if out.is_null() || capacity < src.len() {
        return false;
    }
    // SAFETY: `out` holds at least `src.len()` writable bytes; Rust-owned `src` cannot overlap it.
    unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), out, src.len()) };
    true
}

/// Creates a handle from stored bytes (the keyboard's or app's file).
/// Always returns a handle: NULL, empty, damaged or unknown-version data give
/// a fresh state, and `*status` (when not NULL) says which `MP_PETS_OK`,
/// `MP_PETS_FRESH`, `MP_PETS_CORRUPT` or `MP_PETS_UNKNOWN_VERSION`.
///
/// # Safety
/// `data` must be NULL or valid for reads of `size` bytes; `status` NULL or writable.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mp_pets_from_bytes(data: *const u8, size: usize, status: *mut i32) -> *mut MPPets {
    // SAFETY: forwarded contract.
    let input = unsafe { bytes(data, size) };
    let (state, code) = if input.is_empty() {
        (PetState::default(), MP_PETS_FRESH)
    } else {
        match catch_unwind(|| PetState::from_bytes(input)) {
            Ok(Ok(state)) => (state, MP_PETS_OK),
            Ok(Err(CodecError::Version(_))) => (PetState::default(), MP_PETS_UNKNOWN_VERSION),
            _ => (PetState::default(), MP_PETS_CORRUPT),
        }
    };
    if !status.is_null() {
        // SAFETY: non-null and writable per the contract.
        unsafe { status.write(code) };
    }
    Box::into_raw(Box::new(MPPets { director: Director::new(state) }))
}

/// Frees a handle.
///
/// # Safety
/// `pets` must be NULL or a live handle, freed at most once.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mp_pets_free(pets: *mut MPPets) {
    if !pets.is_null() {
        // SAFETY: a live Box allocation from `mp_pets_from_bytes`, freed once.
        drop(unsafe { Box::from_raw(pets) });
    }
}

/// Decides whether something happens after `trigger`; writes `*plan` and
/// returns 1, or returns 0 for no plan (and for invalid arguments).
///
/// # Safety
/// `pets` NULL or live and not aliased; the struct pointers NULL or valid.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mp_pets_decide(
    pets: *mut MPPets,
    trigger_kind: u32,
    arg: u32,
    ambience_in: *const MPPetsAmbience,
    clock_in: *const MPPetsClock,
    seed: u64,
    plan: *mut MPPetsPlan,
) -> i32 {
    // SAFETY: NULL or a unique live handle per the contract.
    let Some(pets) = (unsafe { pets.as_mut() }) else { return 0 };
    // SAFETY: NULL or valid per the contract.
    let Some(a) = (unsafe { ambience_in.as_ref() }) else { return 0 };
    // SAFETY: NULL or valid per the contract.
    let Some(c) = (unsafe { clock_in.as_ref() }) else { return 0 };
    let Some(t) = trigger(trigger_kind, arg) else { return 0 };
    let (a, c) = (ambience(a), clock(c));
    let decided = catch_unwind(AssertUnwindSafe(|| pets.director.decide(t, &a, c, seed))).ok().flatten();
    let Some(p) = decided else { return 0 };
    if !plan.is_null() {
        let out = MPPetsPlan {
            token: p.token,
            moment: p.moment as u8,
            who: p.who,
            style: p.style as u8,
            ripple: p.ripple as u8,
            duration_ms: p.duration_ms,
            delay_ms: p.delay_ms,
        };
        // SAFETY: non-null and writable per the contract.
        unsafe { plan.write(out) };
    }
    1
}

/// Reports how a plan ended (`MP_PETS_COMPLETED`, `_INTERRUPTED`,
/// `_CANCELLED`). Returns the cat bits that earned a play (0 for unknown or
/// stale tokens).
///
/// # Safety
/// `pets` NULL or live and not aliased; `clock_in` NULL or valid.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mp_pets_resolve(pets: *mut MPPets, token: u32, outcome: u32, clock_in: *const MPPetsClock) -> u32 {
    // SAFETY: NULL or a unique live handle per the contract.
    let Some(pets) = (unsafe { pets.as_mut() }) else { return 0 };
    // SAFETY: NULL or valid per the contract.
    let Some(c) = (unsafe { clock_in.as_ref() }) else { return 0 };
    let outcome = match outcome {
        MP_PETS_COMPLETED => Outcome::Completed,
        MP_PETS_INTERRUPTED => Outcome::Interrupted,
        _ => Outcome::Cancelled,
    };
    let c = clock(c);
    catch_unwind(AssertUnwindSafe(|| u32::from(pets.director.resolve(token, outcome, c).who))).unwrap_or(0)
}

/// Milliseconds until the single idle timer should fire (0: now), or
/// `MP_PETS_NEVER`.
///
/// # Safety
/// Pointers NULL or valid.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mp_pets_idle_delay_ms(pets: *const MPPets, ambience_in: *const MPPetsAmbience, clock_in: *const MPPetsClock) -> u32 {
    // SAFETY: NULL or live per the contract.
    let Some(pets) = (unsafe { pets.as_ref() }) else { return MP_PETS_NEVER };
    // SAFETY: NULL or valid per the contract.
    let Some(a) = (unsafe { ambience_in.as_ref() }) else { return MP_PETS_NEVER };
    // SAFETY: NULL or valid per the contract.
    let Some(c) = (unsafe { clock_in.as_ref() }) else { return MP_PETS_NEVER };
    pets.director.idle_delay_ms(&ambience(a), clock(c))
}

/// Resting pose for `pet` when the keyboard appears: 0 keep, 1 edge_rest,
/// 2 edge_peek, 3 edge_lounge, 4 sleep.
///
/// # Safety
/// `ambience_in` NULL or valid.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mp_pets_rest_pose(pet_id: u32, ambience_in: *const MPPetsAmbience, minute: u32) -> u32 {
    // SAFETY: NULL or valid per the contract.
    let Some(a) = (unsafe { ambience_in.as_ref() }) else { return 0 };
    let Some(p) = pet(pet_id) else { return 0 };
    rest_pose(p, &ambience(a), u16::try_from(minute).unwrap_or(u16::MAX)) as u32
}

/// Writes the persisted state (`MP_PETS_STATE_BYTES`) when `capacity` is
/// enough; returns `MP_PETS_STATE_BYTES` (0 for NULL `pets`).
///
/// # Safety
/// `pets` NULL or live; `out` NULL or writable for `capacity` bytes.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mp_pets_to_bytes(pets: *const MPPets, out: *mut u8, capacity: usize) -> usize {
    // SAFETY: NULL or live per the contract.
    let Some(pets) = (unsafe { pets.as_ref() }) else { return 0 };
    let encoded = pets.director.state().to_bytes();
    // SAFETY: forwarded contract.
    unsafe { write_out(&encoded, out, capacity) };
    STATE_BYTES
}

/// Moves the increments made since the last call into `out`
/// (`MP_PETS_DELTA_BYTES`) and clears them, only when `capacity` is enough.
/// Returns `MP_PETS_DELTA_BYTES` (0 for NULL `pets`).
///
/// # Safety
/// `pets` NULL or live and not aliased; `out` NULL or writable for `capacity` bytes.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mp_pets_take_delta(pets: *mut MPPets, out: *mut u8, capacity: usize) -> usize {
    // SAFETY: NULL or a unique live handle per the contract.
    let Some(pets) = (unsafe { pets.as_mut() }) else { return 0 };
    if !out.is_null() && capacity >= DELTA_BYTES {
        let encoded = pets.director.take_delta().to_bytes();
        // SAFETY: forwarded contract; capacity checked above.
        unsafe { write_out(&encoded, out, capacity) };
    }
    DELTA_BYTES
}

/// Adds a delta from `mp_pets_take_delta` (usually of another handle) to this
/// state. Returns 1, or 0 when the bytes are not a valid delta.
///
/// # Safety
/// `pets` NULL or live and not aliased; `data` NULL or readable for `size` bytes.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mp_pets_apply_delta(pets: *mut MPPets, data: *const u8, size: usize) -> i32 {
    // SAFETY: NULL or a unique live handle per the contract.
    let Some(pets) = (unsafe { pets.as_mut() }) else { return 0 };
    // SAFETY: forwarded contract.
    let Ok(delta) = Delta::from_bytes(unsafe { bytes(data, size) }) else { return 0 };
    let mut state = pets.director.state().clone();
    state.apply_delta(&delta);
    pets.director.merge(&state);
    1
}

/// Joins a stored state (`mp_pets_to_bytes` of another handle or a file)
/// into this one. Returns 1, or 0 when the bytes are not a valid state.
///
/// # Safety
/// `pets` NULL or live and not aliased; `data` NULL or readable for `size` bytes.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mp_pets_merge(pets: *mut MPPets, data: *const u8, size: usize) -> i32 {
    // SAFETY: NULL or a unique live handle per the contract.
    let Some(pets) = (unsafe { pets.as_mut() }) else { return 0 };
    // SAFETY: forwarded contract.
    let Ok(other) = PetState::from_bytes(unsafe { bytes(data, size) }) else { return 0 };
    pets.director.merge(&other);
    1
}

/// Imports the old per-platform counters (max with what the handle holds).
/// Returns 1, 0 for NULL.
///
/// # Safety
/// `pets` NULL or live and not aliased.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mp_pets_import_legacy(
    pets: *mut MPPets,
    committed_graphemes: u64,
    mimimo_pets: u64,
    mimimo_falls: u64,
    momo_pets: u64,
    momo_falls: u64,
) -> i32 {
    // SAFETY: NULL or a unique live handle per the contract.
    let Some(pets) = (unsafe { pets.as_mut() }) else { return 0 };
    pets.director.import_legacy(Snapshot {
        committed_graphemes,
        mimimo: Contacts { pets: mimimo_pets, falls: mimimo_falls, plays: 0 },
        momo: Contacts { pets: momo_pets, falls: momo_falls, plays: 0 },
    });
    1
}

/// Clears every record and raises the epoch (flush afterwards; do not just
/// delete the file, so older copies in other processes cannot merge back).
///
/// # Safety
/// `pets` NULL or live and not aliased.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mp_pets_clear(pets: *mut MPPets) {
    // SAFETY: NULL or a unique live handle per the contract.
    if let Some(pets) = unsafe { pets.as_mut() } {
        pets.director.clear();
    }
}

/// Level and counters of one cat; writes the fields fitting in `info_size`.
/// Returns 1, or 0 for invalid arguments.
///
/// # Safety
/// `pets` NULL or live; `out` NULL or writable for `info_size` bytes.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mp_pets_progress(pets: *const MPPets, pet_id: u32, out: *mut MPPetsProgress, info_size: usize) -> i32 {
    // SAFETY: NULL or live per the contract.
    let Some(pets) = (unsafe { pets.as_ref() }) else { return 0 };
    let Some(p) = pet(pet_id) else { return 0 };
    if out.is_null() || info_size == 0 {
        return 0;
    }
    let snapshot = pets.director.state().snapshot;
    let progress = snapshot.progress(p);
    let contacts = snapshot.contacts(p);
    let full = MPPetsProgress {
        level: u32::from(progress.level),
        graphemes_to_next: progress.next.map_or(0, |n| u32::from(n.graphemes)),
        contacts_to_next: progress.next.map_or(0, |n| u32::from(n.contacts)),
        reserved: 0,
        pets: contacts.pets,
        falls: contacts.falls,
        plays: contacts.plays,
    };
    let n = info_size.min(std::mem::size_of::<MPPetsProgress>());
    // SAFETY: `full` is a live repr(C) value without padding, readable for `n` ≤ its size bytes.
    let src = unsafe { std::slice::from_raw_parts((&raw const full).cast::<u8>(), n) };
    // SAFETY: forwarded contract; `n` ≤ `info_size`.
    unsafe { write_out(src, out.cast::<u8>(), info_size) };
    1
}

/// Copies up to `capacity` paw-book marks, oldest day first; returns how many
/// marks there are (call with `capacity` 0 to size the buffer; at most 400).
///
/// # Safety
/// `pets` NULL or live; `out` NULL or writable for `capacity` marks.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn mp_pets_marks(pets: *const MPPets, out: *mut MPPetsMark, capacity: usize) -> usize {
    // SAFETY: NULL or live per the contract.
    let Some(pets) = (unsafe { pets.as_ref() }) else { return 0 };
    let mut count = 0;
    for (i, m) in pets.director.state().book.marks().enumerate() {
        if !out.is_null() && i < capacity {
            let mark = MPPetsMark { day: m.day, size: m.size, who: m.who, part: m.part, reserved: 0 };
            // SAFETY: `i` < `capacity`, so the offset stays inside the caller's buffer.
            let slot = unsafe { out.add(i) };
            // SAFETY: `slot` is writable per the contract.
            unsafe { slot.write(mark) };
        }
        count = i + 1;
    }
    count
}
