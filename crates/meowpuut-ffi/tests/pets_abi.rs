//! C ABI of the pet rules (`mp_pets_*`): the golden vectors of
//! `Tests/pets-moments.json` through the ABI, the flush protocol between two
//! handles, damaged input, NULL handles and the header.
#![allow(unsafe_code)]
#![allow(clippy::undocumented_unsafe_blocks, clippy::multiple_unsafe_ops_per_block)]

#[path = "../../meowpuut-pets/tests/support/golden.rs"]
mod golden;

use std::mem::size_of;
use std::ptr::{null, null_mut};

use meowpuut_ffi::*;
use meowpuut_pets::codec::PetState;
use meowpuut_pets::moments::{Ambience, Clock, Mode, Moment, Outcome, Plan, Ripple, Style, TouchKind, Trigger};
use meowpuut_pets::Pet;

const HEADER: &str = include_str!("../include/meowpuut.h");

struct Abi(*mut MPPets);

impl Drop for Abi {
    fn drop(&mut self) {
        unsafe { mp_pets_free(self.0) };
    }
}

fn amb(a: &Ambience) -> MPPetsAmbience {
    let bits = [
        (a.composing, MP_PETS_COMPOSING),
        (a.lifted, MP_PETS_LIFTED),
        (a.dismissing, MP_PETS_DISMISSING),
        (a.assistive, MP_PETS_ASSISTIVE),
        (a.lightweight, MP_PETS_LIGHTWEIGHT),
        (a.reduce_motion, MP_PETS_REDUCE_MOTION),
        (a.reduce_transparency, MP_PETS_REDUCE_TRANSPARENCY),
        (a.glass_seat, MP_PETS_GLASS_SEAT),
        (a.edges, MP_PETS_EDGES),
        (a.festival, MP_PETS_FESTIVAL),
    ];
    MPPetsAmbience {
        flags: bits.iter().filter(|(on, _)| *on).map(|(_, bit)| bit).sum(),
        quiet_ms: a.quiet_ms,
        mode: match a.mode {
            Mode::Full => MP_PETS_MODE_FULL,
            Mode::Quiet => MP_PETS_MODE_QUIET,
            Mode::Off => MP_PETS_MODE_OFF,
        },
        cats: u32::from(a.cats),
    }
}

fn clk(c: Clock) -> MPPetsClock {
    MPPetsClock { mono_ms: c.mono_ms, day: c.day, minute: u32::from(c.minute) }
}

fn pet_id(p: Pet) -> u32 {
    match p {
        Pet::Mimimo => MP_PETS_MIMIMO,
        Pet::Momo => MP_PETS_MOMO,
    }
}

impl golden::Driver for Abi {
    fn decide(&mut self, trigger: Trigger, ambience: &Ambience, clock: Clock, seed: u64) -> Option<Plan> {
        let (kind, arg) = match trigger {
            Trigger::Appear => (MP_PETS_APPEAR, 0),
            Trigger::Commit(n) => (MP_PETS_COMMIT, n),
            Trigger::Idle => (MP_PETS_IDLE, 0),
            Trigger::Disappear => (MP_PETS_DISAPPEAR, 0),
            Trigger::Touch(TouchKind::Tap, p) => (MP_PETS_TAP, pet_id(p)),
            Trigger::Touch(TouchKind::Hold, p) => (MP_PETS_HOLD, pet_id(p)),
            Trigger::Touch(TouchKind::Seat, p) => (MP_PETS_SEAT, pet_id(p)),
            Trigger::Touch(TouchKind::Drop, p) => (MP_PETS_DROP, pet_id(p)),
        };
        let mut out = MPPetsPlan::default();
        let got = unsafe { mp_pets_decide(self.0, kind, arg, &amb(ambience), &clk(clock), seed, &mut out) };
        if got == 0 {
            return None;
        }
        let moment = [Moment::Hello, Moment::Purr, Moment::Peekaboo, Moment::Found, Moment::Doze, Moment::Duet, Moment::Flow]
            [usize::from(out.moment) - 1];
        Some(Plan {
            token: out.token,
            moment,
            who: out.who,
            style: [Style::Motion, Style::Fade][usize::from(out.style)],
            ripple: [Ripple::None, Ripple::Soft, Ripple::Line][usize::from(out.ripple)],
            duration_ms: out.duration_ms,
            delay_ms: out.delay_ms,
        })
    }
    fn resolve(&mut self, token: u32, outcome: Outcome, clock: Clock) -> u8 {
        let o = match outcome {
            Outcome::Completed => MP_PETS_COMPLETED,
            Outcome::Interrupted => MP_PETS_INTERRUPTED,
            Outcome::Cancelled => MP_PETS_CANCELLED,
        };
        u8::try_from(unsafe { mp_pets_resolve(self.0, token, o, &clk(clock)) }).unwrap()
    }
    fn state(&self) -> PetState {
        PetState::from_bytes(&bytes(self.0)).unwrap()
    }
}

fn fresh() -> Abi {
    let mut status = -1;
    let h = unsafe { mp_pets_from_bytes(null(), 0, &mut status) };
    assert!(!h.is_null());
    assert_eq!(status, MP_PETS_FRESH);
    Abi(h)
}

fn bytes(h: *const MPPets) -> Vec<u8> {
    let n = unsafe { mp_pets_to_bytes(h, null_mut(), 0) };
    let mut out = vec![0u8; n];
    assert_eq!(unsafe { mp_pets_to_bytes(h, out.as_mut_ptr(), out.len()) }, n);
    out
}

fn open(data: &[u8]) -> (Abi, i32) {
    let mut status = -1;
    let h = unsafe { mp_pets_from_bytes(data.as_ptr(), data.len(), &mut status) };
    (Abi(h), status)
}

const AMB: MPPetsAmbience = MPPetsAmbience {
    flags: MP_PETS_GLASS_SEAT | MP_PETS_EDGES,
    quiet_ms: u32::MAX,
    mode: MP_PETS_MODE_FULL,
    cats: 3,
};

fn at(ms: u64) -> MPPetsClock {
    MPPetsClock { mono_ms: ms, day: 20_406, minute: 720 }
}

#[test]
fn struct_layouts_match_the_header() {
    assert_eq!(size_of::<MPPetsClock>(), 16);
    assert_eq!(size_of::<MPPetsAmbience>(), 16);
    assert_eq!(size_of::<MPPetsPlan>(), 16);
    assert_eq!(size_of::<MPPetsMark>(), 8);
    assert_eq!(size_of::<MPPetsProgress>(), 40);
    assert_eq!((MP_PETS_STATE_BYTES, MP_PETS_DELTA_BYTES), (2884, 84));
}

#[test]
fn golden_moments_through_the_c_abi() {
    let steps = golden::run(|| Box::new(fresh()));
    assert!(steps >= 80);
}

#[test]
fn null_handles_and_bad_arguments_are_harmless() {
    unsafe {
        let mut plan = MPPetsPlan::default();
        assert_eq!(mp_pets_decide(null_mut(), MP_PETS_APPEAR, 0, &AMB, &at(0), 0, &mut plan), 0);
        assert_eq!(mp_pets_resolve(null_mut(), 1, 0, &at(0)), 0);
        assert_eq!(mp_pets_idle_delay_ms(null(), &AMB, &at(0)), MP_PETS_NEVER);
        assert_eq!(mp_pets_to_bytes(null(), null_mut(), 0), 0);
        assert_eq!(mp_pets_take_delta(null_mut(), null_mut(), 0), 0);
        assert_eq!(mp_pets_apply_delta(null_mut(), null(), 0), 0);
        assert_eq!(mp_pets_merge(null_mut(), null(), 0), 0);
        assert_eq!(mp_pets_import_legacy(null_mut(), 1, 1, 1, 1, 1), 0);
        assert_eq!(mp_pets_progress(null(), MP_PETS_MOMO, null_mut(), 0), 0);
        assert_eq!(mp_pets_marks(null(), null_mut(), 0), 0);
        assert_eq!(mp_pets_rest_pose(MP_PETS_MOMO, null(), 720), MP_PETS_REST_KEEP);
        mp_pets_clear(null_mut());
        mp_pets_free(null_mut());
        let h = fresh();
        assert_eq!(mp_pets_decide(h.0, MP_PETS_APPEAR, 0, null(), &at(0), 0, &mut plan), 0);
        assert_eq!(mp_pets_decide(h.0, MP_PETS_APPEAR, 0, &AMB, null(), 0, &mut plan), 0);
        assert_eq!(mp_pets_decide(h.0, 99, 0, &AMB, &at(0), 0, &mut plan), 0);
        assert_eq!(mp_pets_decide(h.0, MP_PETS_HOLD, 7, &AMB, &at(0), 0, &mut plan), 0, "unknown cat");
        let weird_mode = MPPetsAmbience { mode: 9, ..AMB };
        assert_eq!(mp_pets_decide(h.0, MP_PETS_APPEAR, 0, &weird_mode, &at(0), 0, &mut plan), 0, "unknown mode acts as off");
        // A NULL plan still decides (the greeting is used up).
        assert_eq!(mp_pets_decide(h.0, MP_PETS_APPEAR, 0, &AMB, &at(1), 0, null_mut()), 1);
        assert_eq!(mp_pets_progress(h.0, 0, &mut MPPetsProgress::default(), 40), 0);
    }
}

#[test]
fn from_bytes_reports_status_and_survives_any_bytes() {
    let h = fresh();
    unsafe { mp_pets_decide(h.0, MP_PETS_COMMIT, 120, &AMB, &at(1), 0, null_mut()) };
    let good = bytes(h.0);
    assert_eq!(good.len(), MP_PETS_STATE_BYTES);
    let (copy, status) = open(&good);
    assert_eq!(status, MP_PETS_OK);
    assert_eq!(bytes(copy.0), good);
    let mut newer = good.clone();
    newer[2] = 2;
    assert_eq!(open(&newer).1, MP_PETS_UNKNOWN_VERSION);
    assert_eq!(bytes(open(&newer).0 .0), bytes(fresh().0));
    assert_eq!(open(&good[..100]).1, MP_PETS_CORRUPT);
    let mut seed = 99u64;
    for _ in 0..3000 {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        let len = (seed >> 33) as usize % (MP_PETS_STATE_BYTES + 8);
        let junk: Vec<u8> = (0..len).map(|i| (seed >> (i % 56)) as u8 ^ i as u8).collect();
        let (j, status) = open(&junk);
        assert!(!j.0.is_null());
        assert!(matches!(status, MP_PETS_FRESH | MP_PETS_CORRUPT | MP_PETS_UNKNOWN_VERSION), "{status}");
        unsafe {
            assert_eq!(mp_pets_merge(j.0, junk.as_ptr(), junk.len()), 0);
            assert_eq!(mp_pets_apply_delta(j.0, junk.as_ptr(), junk.len()), 0);
        }
    }
}

/// Two keyboard processes and one file: the flush in the header comment.
#[test]
fn flush_protocol_keeps_every_increment() {
    let mut file = bytes(fresh().0);
    let (a, b) = (fresh(), fresh());
    let flush = |live: &Abi, file: &mut Vec<u8>| unsafe {
        let (disk, status) = open(file);
        assert_eq!(status, MP_PETS_OK);
        let mut delta = vec![0u8; MP_PETS_DELTA_BYTES];
        assert_eq!(mp_pets_take_delta(live.0, delta.as_mut_ptr(), delta.len()), MP_PETS_DELTA_BYTES);
        assert_eq!(mp_pets_apply_delta(disk.0, delta.as_ptr(), delta.len()), 1);
        let mine = bytes(live.0);
        assert_eq!(mp_pets_merge(disk.0, mine.as_ptr(), mine.len()), 1);
        *file = bytes(disk.0);
        assert_eq!(mp_pets_merge(live.0, file.as_ptr(), file.len()), 1);
    };
    let mut ms = 1;
    for round in 0..1000u32 {
        for (h, pet) in [(&a, MP_PETS_MIMIMO), (&b, MP_PETS_MOMO)] {
            ms += 7;
            unsafe {
                mp_pets_decide(h.0, MP_PETS_COMMIT, 3, &AMB, &at(ms), 0, null_mut());
                mp_pets_decide(h.0, MP_PETS_TAP, pet, &AMB, &at(ms), 0, null_mut());
            }
        }
        // Interleaved flushes, with the order changing every round.
        if round % 2 == 0 {
            flush(&a, &mut file);
            flush(&b, &mut file);
        } else {
            flush(&b, &mut file);
            flush(&a, &mut file);
        }
    }
    let state = PetState::from_bytes(&file).unwrap();
    assert_eq!(state.snapshot.committed_graphemes, 6000);
    assert_eq!((state.snapshot.mimimo.pets, state.snapshot.momo.pets), (1000, 1000));
    let mut p = MPPetsProgress::default();
    unsafe {
        let (disk, _) = open(&file);
        assert_eq!(mp_pets_progress(disk.0, MP_PETS_MOMO, &mut p, size_of::<MPPetsProgress>()), 1);
    }
    assert_eq!(p.level, 100, "6000 / 200 + 1000 / 3 is past the cap");
    assert_eq!((p.pets, p.plays), (1000, 0));
    // Taking a delta without room leaves the increments in place.
    unsafe {
        mp_pets_decide(a.0, MP_PETS_COMMIT, 5, &AMB, &at(ms + 1), 0, null_mut());
        let mut small = [0u8; 8];
        assert_eq!(mp_pets_take_delta(a.0, small.as_mut_ptr(), small.len()), MP_PETS_DELTA_BYTES);
        assert_eq!(small, [0; 8]);
    }
    flush(&a, &mut file);
    assert_eq!(PetState::from_bytes(&file).unwrap().snapshot.committed_graphemes, 6005);
}

#[test]
fn legacy_import_keeps_the_level_and_is_idempotent() {
    let h = fresh();
    let mut p = MPPetsProgress::default();
    unsafe {
        assert_eq!(mp_pets_import_legacy(h.0, 1234, 5, 6, 7, 8), 1);
        assert_eq!(mp_pets_import_legacy(h.0, 1234, 5, 6, 7, 8), 1);
        mp_pets_progress(h.0, MP_PETS_MIMIMO, &mut p, size_of::<MPPetsProgress>());
    }
    assert_eq!((p.level, p.graphemes_to_next, p.contacts_to_next, p.pets, p.falls, p.plays), (1234 / 200 + 11 / 3, 166, 1, 5, 6, 0));
    // A short info_size writes only the leading fields.
    let mut short = MPPetsProgress { pets: 77, ..MPPetsProgress::default() };
    unsafe { mp_pets_progress(h.0, MP_PETS_MOMO, &mut short, 16) };
    assert_eq!((short.level, short.pets), (1234 / 200 + 15 / 3, 77));
}

#[test]
fn marks_and_clear() {
    let h = fresh();
    unsafe {
        for day in 20_000..20_010u32 {
            let c = MPPetsClock { mono_ms: u64::from(day), day, minute: 600 };
            mp_pets_decide(h.0, MP_PETS_COMMIT, 120 * (day - 19_999), &AMB, &c, 0, null_mut());
        }
        assert_eq!(mp_pets_marks(h.0, null_mut(), 0), 10);
        let mut marks = [MPPetsMark::default(); 4];
        assert_eq!(mp_pets_marks(h.0, marks.as_mut_ptr(), marks.len()), 10);
        assert_eq!(marks[0], MPPetsMark { day: 20_000, size: 1, who: 3, part: 2, reserved: 0 });
        assert_eq!(marks[3].day, 20_003);
        let before = bytes(h.0);
        mp_pets_clear(h.0);
        assert_eq!(mp_pets_marks(h.0, null_mut(), 0), 0);
        // The older copy cannot bring the records back.
        assert_eq!(mp_pets_merge(h.0, before.as_ptr(), before.len()), 1);
        assert_eq!(mp_pets_marks(h.0, null_mut(), 0), 0);
        assert_eq!(mp_pets_rest_pose(MP_PETS_MIMIMO, &AMB, 120), MP_PETS_REST_SLEEP);
        assert_eq!(mp_pets_rest_pose(MP_PETS_MOMO, &AMB, 120), MP_PETS_REST_EDGE_LOUNGE);
        assert_eq!(mp_pets_rest_pose(MP_PETS_MOMO, &MPPetsAmbience { flags: 0, ..AMB }, 120), MP_PETS_REST_KEEP);
        assert_eq!(mp_pets_idle_delay_ms(h.0, &MPPetsAmbience { quiet_ms: 5_000, ..AMB }, &at(0)), 20_000);
    }
}

#[test]
fn header_declares_every_function_and_constant() {
    let block = HEADER.split("typedef struct MPPets MPPets;").nth(1).expect("pets block in the header");
    for decl in [
        "MPPets *mp_pets_from_bytes(const uint8_t *data, size_t size, int *status);",
        "void mp_pets_free(MPPets *pets);",
        "int mp_pets_decide(MPPets *pets, uint32_t trigger, uint32_t arg, const MPPetsAmbience *ambience,",
        "uint32_t mp_pets_resolve(MPPets *pets, uint32_t token, uint32_t outcome, const MPPetsClock *clock);",
        "uint32_t mp_pets_idle_delay_ms(const MPPets *pets, const MPPetsAmbience *ambience, const MPPetsClock *clock);",
        "uint32_t mp_pets_rest_pose(uint32_t pet, const MPPetsAmbience *ambience, uint32_t minute);",
        "size_t mp_pets_to_bytes(const MPPets *pets, uint8_t *out, size_t capacity);",
        "size_t mp_pets_take_delta(MPPets *pets, uint8_t *out, size_t capacity);",
        "int mp_pets_apply_delta(MPPets *pets, const uint8_t *data, size_t size);",
        "int mp_pets_merge(MPPets *pets, const uint8_t *data, size_t size);",
        "int mp_pets_import_legacy(MPPets *pets, uint64_t committed_graphemes, uint64_t mimimo_pets,",
        "void mp_pets_clear(MPPets *pets);",
        "int mp_pets_progress(const MPPets *pets, uint32_t pet, MPPetsProgress *info, size_t info_size);",
        "size_t mp_pets_marks(const MPPets *pets, MPPetsMark *out, size_t capacity);",
        "#define MP_PETS_STATE_BYTES 2884",
        "#define MP_PETS_DELTA_BYTES 84",
    ] {
        assert!(block.contains(decl), "missing in the header: {decl}");
    }
    let consts: &[(&str, u32)] = &[
        ("MP_PETS_MIMIMO", MP_PETS_MIMIMO),
        ("MP_PETS_MOMO", MP_PETS_MOMO),
        ("MP_PETS_APPEAR", MP_PETS_APPEAR),
        ("MP_PETS_COMMIT", MP_PETS_COMMIT),
        ("MP_PETS_IDLE", MP_PETS_IDLE),
        ("MP_PETS_TAP", MP_PETS_TAP),
        ("MP_PETS_HOLD", MP_PETS_HOLD),
        ("MP_PETS_SEAT", MP_PETS_SEAT),
        ("MP_PETS_DROP", MP_PETS_DROP),
        ("MP_PETS_DISAPPEAR", MP_PETS_DISAPPEAR),
        ("MP_PETS_COMPOSING", MP_PETS_COMPOSING),
        ("MP_PETS_LIFTED", MP_PETS_LIFTED),
        ("MP_PETS_DISMISSING", MP_PETS_DISMISSING),
        ("MP_PETS_ASSISTIVE", MP_PETS_ASSISTIVE),
        ("MP_PETS_LIGHTWEIGHT", MP_PETS_LIGHTWEIGHT),
        ("MP_PETS_REDUCE_MOTION", MP_PETS_REDUCE_MOTION),
        ("MP_PETS_REDUCE_TRANSPARENCY", MP_PETS_REDUCE_TRANSPARENCY),
        ("MP_PETS_GLASS_SEAT", MP_PETS_GLASS_SEAT),
        ("MP_PETS_EDGES", MP_PETS_EDGES),
        ("MP_PETS_FESTIVAL", MP_PETS_FESTIVAL),
        ("MP_PETS_MODE_FULL", MP_PETS_MODE_FULL),
        ("MP_PETS_MODE_QUIET", MP_PETS_MODE_QUIET),
        ("MP_PETS_MODE_OFF", MP_PETS_MODE_OFF),
        ("MP_PETS_HELLO", MP_PETS_HELLO),
        ("MP_PETS_PURR", MP_PETS_PURR),
        ("MP_PETS_PEEKABOO", MP_PETS_PEEKABOO),
        ("MP_PETS_FOUND", MP_PETS_FOUND),
        ("MP_PETS_DOZE", MP_PETS_DOZE),
        ("MP_PETS_DUET", MP_PETS_DUET),
        ("MP_PETS_FLOW", MP_PETS_FLOW),
        ("MP_PETS_STYLE_MOTION", MP_PETS_STYLE_MOTION),
        ("MP_PETS_STYLE_FADE", MP_PETS_STYLE_FADE),
        ("MP_PETS_RIPPLE_NONE", MP_PETS_RIPPLE_NONE),
        ("MP_PETS_RIPPLE_SOFT", MP_PETS_RIPPLE_SOFT),
        ("MP_PETS_RIPPLE_LINE", MP_PETS_RIPPLE_LINE),
        ("MP_PETS_COMPLETED", MP_PETS_COMPLETED),
        ("MP_PETS_INTERRUPTED", MP_PETS_INTERRUPTED),
        ("MP_PETS_CANCELLED", MP_PETS_CANCELLED),
        ("MP_PETS_OK", MP_PETS_OK as u32),
        ("MP_PETS_FRESH", MP_PETS_FRESH as u32),
        ("MP_PETS_CORRUPT", MP_PETS_CORRUPT as u32),
        ("MP_PETS_UNKNOWN_VERSION", MP_PETS_UNKNOWN_VERSION as u32),
        ("MP_PETS_REST_KEEP", MP_PETS_REST_KEEP),
        ("MP_PETS_REST_EDGE_REST", MP_PETS_REST_EDGE_REST),
        ("MP_PETS_REST_EDGE_PEEK", MP_PETS_REST_EDGE_PEEK),
        ("MP_PETS_REST_EDGE_LOUNGE", MP_PETS_REST_EDGE_LOUNGE),
        ("MP_PETS_REST_SLEEP", MP_PETS_REST_SLEEP),
    ];
    for (name, value) in consts {
        assert!(block.contains(&format!("{name} = {value}")), "header value of {name} is not {value}");
    }
    // The Rust enums carry the same numbers.
    assert_eq!(Moment::Flow as u32, MP_PETS_FLOW);
    assert_eq!(Style::Fade as u32, MP_PETS_STYLE_FADE);
    assert_eq!(Ripple::Line as u32, MP_PETS_RIPPLE_LINE);
    assert_eq!(meowpuut_pets::moments::RestPose::Sleep as u32, MP_PETS_REST_SLEEP);
}
