# Meowpuut Pets

The pet rules are separate from the input engine and keyboard views. Clients
can restore `Snapshot` from their existing local counters, then supply the
number of committed graphemes, a completed pet, or a completed fall.
`apply` returns the cats whose levels changed; `progress` gives both routes
to the next level. No user text or storage permissions enter the core.

The current Android and iOS keyboards still own their storage and event
collection. Their private counters keep working unchanged until the core is
wired into those clients. The two stable IDs are `mimimo` and `momo`, regardless
of the active skin, outfit or pose.

## Moments, paw book and storage

Nothing in the crate reads a clock, a random source or a file. Clients pass a
monotonic clock in milliseconds, the local day index and minute, and a seed.

| file | what |
| --- | --- |
| `src/lib.rs` | `Snapshot`, `Event`, the level rule. `Contacts.plays` counts rewarded moments and is not part of the level, so old snapshots keep their level exactly. |
| `src/moments.rs` | `Director::decide(trigger, &ambience, clock, seed) -> Option<Plan>`, `resolve(token, outcome, clock) -> Reward`, `idle_delay_ms`, `rest_pose`, `DayPart`. |
| `src/book.rs` | `PawBook`: a ring of 400 `Mark { day, size, who, part }`, one per local day. |
| `src/codec.rs` | `PetState` (snapshot, today's limits, book, epoch), its 2,884-byte versioned little-endian form, `Delta` and `merge`. |

Rules the director enforces:

- Responses to the user (purr, found, duet) are never budgeted.
- Spontaneous moments (peekaboo) need a token from a bucket of 2 that refills one
  every 120 s, at most 1 per session (appear to disappear) and 3 per local day.
- No spontaneous moment while composing, within 600 ms of a key, while a cat is
  lifted, while the keyboard is dismissed, with VoiceOver, Switch Control or
  TalkBack, with lightweight effects, or in quiet or off mode.
- The greeting comes once per local day. Quiet mode keeps the greeting, the doze
  and what the user starts; off mode plans nothing (taps and drops still count).
- A purr earns a play at most once per cat every 8 s and 12 times per cat a day.
  Two duets a day.
- The monotonic clock is clamped, and while it runs backwards no spontaneous
  moment starts. A day index that goes backwards is the same day. An unknown day
  (0) or minute (≥ 1440) plans no spontaneous moment and no greeting.
- Reduce Motion turns every plan into a pose fade; without a glass seat the
  peekaboo is a fade; Reduce Transparency or Increase Contrast draws the ripple
  as an opaque line and turns the flow glow off.

Storage: `PetState::to_bytes` is always `STATE_BYTES` (2,884) long, holds no
text and ends with a checksum. `from_bytes` never panics; damaged bytes are
`Err(Corrupt)`, another version is `Err(Version)`, and `PetState::load` starts
fresh on either. `merge` is commutative, associative and idempotent (higher
epoch wins; then max of counters, the later day's limits, slot-wise join of the
book). Deltas add, so two processes flushing one file keep both increments; the
flush order is in the module documentation of `codec.rs` and in the C header.

## C ABI and golden vectors

`meowpuut-ffi` exports the director as `mp_pets_*` (`crates/meowpuut-ffi/src/pets.rs`,
documented in `include/meowpuut.h`). `Tests/pets-moments.json` holds golden
vectors (steps with a clock, trigger, ambience, seed and the expected plan,
reward or state). `tests/golden_moments.rs` runs them against `Director`, and
`meowpuut-ffi/tests/pets_abi.rs` runs the same file through the C ABI. Swift and
Kotlin tests should load the same file and drive `mp_pets_*`; the file's
`about` list describes its format.

## Tests

`cargo test -p meowpuut-pets` checks the level rules (and that plays and old
imports leave levels unchanged), the token bucket and the daily and session
caps, every suppression rule, clocks running backwards, the purr limits, the
golden vectors, merge properties on generated states (a SplitMix64 generator,
no extra crates), concurrent deltas, truncated, random and bit-flipped bytes,
privacy (fixed length, no text) and that `decide` stays under 5 µs per call
(measured: tens of nanoseconds).
