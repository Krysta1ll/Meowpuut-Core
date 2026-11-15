//! The moment director: given what just happened ([`Trigger`]), the state of
//! the keyboard ([`Ambience`]), a clock and a seed, it decides whether the
//! cats do something ([`Plan`]). The client plays the plan and reports how it
//! ended ([`Outcome`]); some endings earn a [`Reward`].
//!
//! Deterministic: no clock, randomness or IO. Clients call `decide` from five
//! places only (keyboard shown, commit callback, idle timer, touch on a cat or
//! its seat, keyboard hidden), never on the key path.
//!
//! Rules (see `README.md`):
//! - User-initiated responses (purr, found, duet) are never budgeted.
//! - Spontaneous moments (peekaboo) need a token (bucket of 2, one more every
//!   120 s), at most 1 per session and 3 per local day.
//! - No spontaneous moment while composing, within 600 ms of a key press,
//!   while a cat is lifted, while the keyboard is being dismissed, with
//!   VoiceOver/Switch Control/TalkBack, with lightweight effects, in quiet or
//!   off mode, or when the clock looks wrong. Any doubt: `None`.
//! - The greeting happens once per local day (not budgeted).
//! - A purr earns a play at most once per cat per 8 s and 12 times per cat a
//!   day; duets at most twice a day.
//! - The monotonic clock is clamped (never runs backwards); a local day index
//!   that goes backwards counts as the same day.

use crate::book::{size_band, WHO_MIMIMO, WHO_MOMO};
use crate::codec::{pet_index, Delta, PetState};
use crate::{Event, Pet};

/// Spontaneous moments per local day.
pub const DAILY_SPONTANEOUS: u8 = 3;
/// Spontaneous moments per session (keyboard shown until hidden).
pub const SESSION_SPONTANEOUS: u8 = 1;
/// Token bucket capacity and refill period.
pub const BUCKET_CAPACITY: u8 = 2;
pub const BUCKET_REFILL_MS: u64 = 120_000;
/// No spontaneous moment within this long of a key press.
pub const QUIET_MS: u32 = 600;
/// Purr rewards: per cat, minimum spacing and daily cap.
pub const PURR_SPACING_MS: u64 = 8_000;
pub const PURR_DAILY: u8 = 12;
/// Duets per local day, and the window in which both cats must be touched.
pub const DUET_DAILY: u8 = 2;
pub const DUET_WINDOW_MS: u64 = 60_000;
/// Doze after this long without a key press (late night: the shorter one).
pub const DOZE_MS: u32 = 25_000;
pub const DOZE_LATE_MS: u32 = 12_000;
/// Flow: this many commits within the window.
pub const FLOW_COMMITS: usize = 8;
pub const FLOW_WINDOW_MS: u64 = 30_000;
/// Peekaboo: 1 in this many keyboard appearances; surfaces by itself after 3–9 s.
pub const PEEKABOO_ODDS: u64 = 10;
pub const PEEKABOO_SURFACE_MIN_MS: u32 = 3_000;
pub const PEEKABOO_SURFACE_SPAN_MS: u32 = 6_000;
/// Any running moment yields within this long of a key press (client side).
pub const YIELD_MS: u32 = 180;
/// `idle_delay_ms` result meaning "do not arm the idle timer".
pub const NEVER: u32 = u32::MAX;

/// Part of the local day, from the local minute of the day.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum DayPart {
    /// 05:00–07:59
    Dawn = 0,
    /// 08:00–16:59
    Day = 1,
    /// 17:00–19:59
    Evening = 2,
    /// 20:00–22:59
    Night = 3,
    /// 23:00–04:59
    LateNight = 4,
}

impl DayPart {
    /// `None` for a minute outside 0..1440.
    pub const fn from_minute(minute: u16) -> Option<Self> {
        Some(match minute {
            300..=479 => Self::Dawn,
            480..=1019 => Self::Day,
            1020..=1199 => Self::Evening,
            1200..=1379 => Self::Night,
            0..=299 | 1380..=1439 => Self::LateNight,
            _ => return None,
        })
    }

    pub const fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// Resting pose chosen when the keyboard appears (edge seating only).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum RestPose {
    /// Leave the pose as it is.
    Keep = 0,
    EdgeRest = 1,
    EdgePeek = 2,
    EdgeLounge = 3,
    Sleep = 4,
}

/// Time as the client sees it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Clock {
    /// Monotonic milliseconds (`ProcessInfo.systemUptime`, `SystemClock.elapsedRealtime`).
    pub mono_ms: u64,
    /// Local day index: days since 1970-01-01 in the device's time zone. 0 = unknown.
    pub day: u32,
    /// Local minute of the day, 0..1440.
    pub minute: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Mode {
    /// Every moment.
    Full = 0,
    /// Only what the user starts, plus greeting and doze.
    Quiet = 1,
    /// Nothing from the director; the cats keep their old behaviour.
    Off = 2,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TouchKind {
    /// A tap on a cat (the client's existing reaction still plays).
    Tap,
    /// Held over 0.5 s, moved under the lift distance.
    Hold,
    /// A tap on the seat area (finds a hiding cat).
    Seat,
    /// A dragged cat was dropped back and accepted.
    Drop,
}

/// What just happened.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Trigger {
    Appear,
    /// Text committed (graphemes in this commit).
    Commit(u32),
    /// The single idle timer fired.
    Idle,
    Touch(TouchKind, Pet),
    Disappear,
}

/// Keyboard state at the time of the trigger.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ambience {
    pub composing: bool,
    /// Milliseconds since the last key press (`u32::MAX`: none yet).
    pub quiet_ms: u32,
    /// A cat is being dragged.
    pub lifted: bool,
    /// The keyboard is being dismissed.
    pub dismissing: bool,
    /// VoiceOver, Switch Control or TalkBack is running.
    pub assistive: bool,
    /// The lightweight effects level.
    pub lightweight: bool,
    pub reduce_motion: bool,
    /// Reduce Transparency or Increase Contrast.
    pub reduce_transparency: bool,
    /// The cats sit behind the glass bar with the cut-out mask, so sinking works.
    pub glass_seat: bool,
    /// Edge seating (`seating == .edges`); resting poses apply only there.
    pub edges: bool,
    /// A festival outfit is showing.
    pub festival: bool,
    /// Cats on screen: [`WHO_MIMIMO`] | [`WHO_MOMO`].
    pub cats: u8,
    pub mode: Mode,
}

impl Default for Ambience {
    fn default() -> Self {
        Self {
            composing: false,
            quiet_ms: u32::MAX,
            lifted: false,
            dismissing: false,
            assistive: false,
            lightweight: false,
            reduce_motion: false,
            reduce_transparency: false,
            glass_seat: true,
            edges: true,
            festival: false,
            cats: WHO_MIMIMO | WHO_MOMO,
            mode: Mode::Full,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Moment {
    /// First appearance of the local day: Momo waves, Mimimo reaches a beat later.
    Hello = 1,
    /// Held cat purrs (Mimimo `sleep`, Momo `happy`), breathing 2.6 s, up to `duration_ms`.
    Purr = 2,
    /// A cat sinks into the glass; it surfaces after `delay_ms` or when found.
    Peekaboo = 3,
    /// The hiding cat was found: pops up with `happy`.
    Found = 4,
    /// Doze until a key or touch wakes the cats (client: 0.3 s stretch on waking).
    Doze = 5,
    /// Momo `jump` and Mimimo `happy` together.
    Duet = 6,
    /// The glass rim glows (0 → 0.12 opacity over `duration_ms`) until typing stops for 2 s.
    Flow = 7,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Style {
    /// Full motion.
    Motion = 0,
    /// Pose cross-fade only (Reduce Motion, no glass seat, classic or pixel skins).
    Fade = 1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Ripple {
    None = 0,
    /// Two soft rings, 0.5 s.
    Soft = 1,
    /// Opaque thin line (Reduce Transparency / Increase Contrast).
    Line = 2,
}

/// What the client should play.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Plan {
    /// Pass to [`Director::resolve`] when the moment ends. Never 0.
    pub token: u32,
    pub moment: Moment,
    /// [`WHO_MIMIMO`] | [`WHO_MOMO`].
    pub who: u8,
    pub style: Style,
    pub ripple: Ripple,
    /// Main motion length (Doze: 0, it is a state).
    pub duration_ms: u32,
    /// Peekaboo: surface by itself after this long. Otherwise 0.
    pub delay_ms: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Outcome {
    /// Ran its course, or the user ended it (finger lifted from a purr).
    Completed = 0,
    /// Cut short by typing, a drag or the keyboard leaving.
    Interrupted = 1,
    /// Never shown.
    Cancelled = 2,
}

/// Cats that earned a play (also applied to the state as `Event::Played`).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Reward {
    pub who: u8,
}

#[derive(Clone, Copy, Debug, Default)]
struct Active {
    token: u32,
    moment: Option<Moment>,
    who: u8,
}

/// Session memory: never persisted.
#[derive(Clone, Debug, Default)]
struct Memory {
    last_mono: Option<u64>,
    tokens: u8,
    refilled_at: u64,
    session_spontaneous: u8,
    next_token: u32,
    active: [Active; 4],
    peekaboo: Option<(u32, u8)>,
    dozing: Option<u32>,
    flow: Option<u32>,
    commits: [u64; FLOW_COMMITS],
    commit_count: usize,
    touched: [Option<u64>; 2],
    purr_rewarded: [Option<u64>; 2],
}

/// See the module documentation.
#[derive(Clone, Debug, Default)]
pub struct Director {
    state: PetState,
    pending: Delta,
    mem: Memory,
}

/// Effective clock after clamping.
#[derive(Clone, Copy)]
struct Now {
    ms: u64,
    day: u32,
    part: Option<DayPart>,
    /// The clock went backwards or is unknown: no spontaneous moment.
    doubtful: bool,
}

const fn mix(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

const fn who_of(pet: Pet) -> u8 {
    match pet {
        Pet::Mimimo => WHO_MIMIMO,
        Pet::Momo => WHO_MOMO,
    }
}

fn style(ambience: &Ambience) -> Style {
    if ambience.reduce_motion {
        Style::Fade
    } else {
        Style::Motion
    }
}

/// Resting pose for `pet` when the keyboard appears. Only edge seating
/// changes; festival outfits and unknown times keep the current pose.
pub fn rest_pose(pet: Pet, ambience: &Ambience, minute: u16) -> RestPose {
    if !ambience.edges || ambience.festival || ambience.mode == Mode::Off {
        return RestPose::Keep;
    }
    match DayPart::from_minute(minute) {
        None => RestPose::Keep,
        Some(DayPart::Dawn) => RestPose::EdgeRest,
        Some(DayPart::Day) => RestPose::EdgePeek,
        Some(DayPart::Evening | DayPart::Night) => RestPose::EdgeLounge,
        Some(DayPart::LateNight) => match pet {
            Pet::Mimimo => RestPose::Sleep,
            Pet::Momo => RestPose::EdgeLounge,
        },
    }
}

impl Director {
    pub fn new(state: PetState) -> Self {
        Self { state, ..Self::default() }
    }

    pub fn state(&self) -> &PetState {
        &self.state
    }

    /// Increments since the last call (for the flush), leaving an empty delta.
    pub fn take_delta(&mut self) -> Delta {
        let fresh = Delta { epoch: self.state.epoch, day: self.state.daily.day, ..Delta::default() };
        core::mem::replace(&mut self.pending, fresh)
    }

    /// Adopt another copy (the file after a flush, or the app group mirror).
    pub fn merge(&mut self, other: &PetState) {
        let epoch = self.state.epoch;
        self.state.merge(other);
        if self.state.epoch != epoch {
            self.pending = Delta { epoch: self.state.epoch, ..Delta::default() };
        }
    }

    /// Clear every record ("clear cat records").
    pub fn clear(&mut self) {
        self.state.clear();
        self.pending = Delta { epoch: self.state.epoch, ..Delta::default() };
        self.mem.touched = [None; 2];
    }

    /// Import counters from the old per-platform storage (max with what is here).
    pub fn import_legacy(&mut self, snapshot: crate::Snapshot) {
        let other = PetState { epoch: self.state.epoch, snapshot, daily: self.state.daily, book: self.state.book.clone() };
        self.state.merge(&other);
    }

    /// How long until the idle timer should fire, given the time since the
    /// last key press; 0 means now, [`NEVER`] means do not arm it.
    pub fn idle_delay_ms(&self, ambience: &Ambience, clock: Clock) -> u32 {
        if ambience.mode == Mode::Off || self.mem.dozing.is_some() {
            return NEVER;
        }
        Self::doze_after(clock.minute).saturating_sub(ambience.quiet_ms)
    }

    fn doze_after(minute: u16) -> u32 {
        if DayPart::from_minute(minute) == Some(DayPart::LateNight) {
            DOZE_LATE_MS
        } else {
            DOZE_MS
        }
    }

    fn now(&mut self, clock: Clock) -> Now {
        let backwards = self.mem.last_mono.is_some_and(|last| clock.mono_ms < last);
        let ms = self.mem.last_mono.map_or(clock.mono_ms, |last| last.max(clock.mono_ms));
        if self.mem.last_mono.is_none() {
            self.mem.tokens = BUCKET_CAPACITY;
            self.mem.refilled_at = ms;
        }
        self.mem.last_mono = Some(ms);
        // Refill the bucket.
        let gained = (ms - self.mem.refilled_at) / BUCKET_REFILL_MS;
        if gained > 0 {
            self.mem.tokens = (u64::from(self.mem.tokens) + gained).min(u64::from(BUCKET_CAPACITY)) as u8;
            self.mem.refilled_at += gained * BUCKET_REFILL_MS;
        }
        if self.mem.tokens == BUCKET_CAPACITY {
            self.mem.refilled_at = ms;
        }
        // A day index that goes backwards is the same day.
        let day = clock.day.max(self.state.daily.day);
        if day > self.state.daily.day {
            self.state.daily = crate::codec::Daily { day, ..Default::default() };
        }
        if self.pending.day != day {
            self.pending = Delta {
                day,
                day_graphemes: 0,
                greeted: false,
                spontaneous: 0,
                purrs: [0; 2],
                duets: 0,
                who: 0,
                part: 0,
                ..self.pending
            };
        }
        self.pending.epoch = self.state.epoch;
        let part = DayPart::from_minute(clock.minute);
        Now { ms, day, part, doubtful: backwards || clock.day == 0 || part.is_none() }
    }

    fn issue(&mut self, moment: Moment, who: u8, style: Style, ripple: Ripple, duration_ms: u32, delay_ms: u32) -> Plan {
        self.mem.next_token = self.mem.next_token.wrapping_add(1).max(1);
        let token = self.mem.next_token;
        let slot = self
            .mem
            .active
            .iter()
            .position(|a| a.moment.is_none())
            .unwrap_or_else(|| {
                let oldest = (0..self.mem.active.len()).min_by_key(|&i| self.mem.active[i].token).unwrap_or(0);
                self.forget(self.mem.active[oldest].token);
                oldest
            });
        self.mem.active[slot] = Active { token, moment: Some(moment), who };
        Plan { token, moment, who, style, ripple, duration_ms, delay_ms }
    }

    fn forget(&mut self, token: u32) {
        if self.mem.peekaboo.is_some_and(|(t, _)| t == token) {
            self.mem.peekaboo = None;
        }
        if self.mem.dozing == Some(token) {
            self.mem.dozing = None;
        }
        if self.mem.flow == Some(token) {
            self.mem.flow = None;
        }
    }

    /// Mark activity in the book and the pending delta.
    fn mark(&mut self, now: Now, who: u8) {
        let part = now.part.map_or(0, DayPart::bit);
        self.state.book.record(now.day, size_band(self.state.daily.graphemes), who, part);
        self.pending.who |= who & 3;
        self.pending.part |= part;
    }

    fn event(&mut self, event: Event) {
        self.state.snapshot.apply(event);
        let c = &mut self.pending.contacts;
        match event {
            Event::CommittedGraphemes(n) => self.pending.graphemes = self.pending.graphemes.saturating_add(n),
            Event::Petted(p) => c[pet_index(p)].pets = c[pet_index(p)].pets.saturating_add(1),
            Event::DropAccepted(p) => c[pet_index(p)].falls = c[pet_index(p)].falls.saturating_add(1),
            Event::Played(p) => c[pet_index(p)].plays = c[pet_index(p)].plays.saturating_add(1),
        }
    }

    /// True when a spontaneous moment may start now (budget not checked).
    fn calm(ambience: &Ambience, now: Now) -> bool {
        ambience.mode == Mode::Full
            && !ambience.composing
            && ambience.quiet_ms >= QUIET_MS
            && !ambience.lifted
            && !ambience.dismissing
            && !ambience.assistive
            && !ambience.lightweight
            && !now.doubtful
    }

    fn budget(&self) -> bool {
        self.mem.tokens >= 1
            && self.mem.session_spontaneous < SESSION_SPONTANEOUS
            && self.state.daily.spontaneous < DAILY_SPONTANEOUS
    }

    fn spend(&mut self) {
        self.mem.tokens -= 1;
        self.mem.session_spontaneous += 1;
        self.state.daily.spontaneous = self.state.daily.spontaneous.saturating_add(1);
        self.pending.spontaneous = self.pending.spontaneous.saturating_add(1);
    }

    /// Decide what happens after `trigger`. `seed` is any per-call value the
    /// client chooses (it only picks among allowed outcomes).
    pub fn decide(&mut self, trigger: Trigger, ambience: &Ambience, clock: Clock, seed: u64) -> Option<Plan> {
        let now = self.now(clock);
        match trigger {
            Trigger::Appear => self.appear(ambience, now, seed),
            Trigger::Commit(graphemes) => self.commit(graphemes, ambience, now),
            Trigger::Idle => self.idle(ambience, now, clock.minute),
            Trigger::Touch(kind, pet) => self.touch(kind, pet, ambience, now),
            Trigger::Disappear => {
                self.end_session();
                None
            }
        }
    }

    fn end_session(&mut self) {
        self.mem.active = [Active::default(); 4];
        self.mem.peekaboo = None;
        self.mem.dozing = None;
        self.mem.flow = None;
        self.mem.commit_count = 0;
        self.mem.session_spontaneous = 0;
    }

    fn appear(&mut self, a: &Ambience, now: Now, seed: u64) -> Option<Plan> {
        self.end_session();
        if a.mode == Mode::Off || now.doubtful {
            return None;
        }
        let settled = !a.composing && a.quiet_ms >= QUIET_MS && !a.lifted && !a.dismissing && !a.assistive && !a.lightweight;
        if !self.state.daily.greeted {
            if !settled || a.cats == 0 {
                return None;
            }
            self.state.daily.greeted = true;
            self.pending.greeted = true;
            return Some(self.issue(Moment::Hello, a.cats & 3, style(a), Ripple::None, 900, 0));
        }
        let h = mix(seed);
        if !h.is_multiple_of(PEEKABOO_ODDS) || !Self::calm(a, now) || !self.budget() {
            return None;
        }
        let who = match a.cats & 3 {
            0 => return None,
            3 => {
                if (h >> 32) & 1 == 0 {
                    WHO_MIMIMO
                } else {
                    WHO_MOMO
                }
            }
            one => one,
        };
        self.spend();
        let sinks = a.glass_seat && !a.reduce_motion;
        let (style, ripple, duration) = if sinks {
            (Style::Motion, if a.reduce_transparency { Ripple::Line } else { Ripple::Soft }, 280)
        } else {
            (Style::Fade, Ripple::None, 250)
        };
        let delay = PEEKABOO_SURFACE_MIN_MS + ((h >> 40) % u64::from(PEEKABOO_SURFACE_SPAN_MS + 1)) as u32;
        let plan = self.issue(Moment::Peekaboo, who, style, ripple, duration, delay);
        self.mem.peekaboo = Some((plan.token, who));
        Some(plan)
    }

    fn commit(&mut self, graphemes: u32, a: &Ambience, now: Now) -> Option<Plan> {
        if graphemes > 0 {
            self.event(Event::CommittedGraphemes(u64::from(graphemes)));
            let d = &mut self.state.daily;
            d.graphemes = d.graphemes.saturating_add(graphemes);
            self.pending.day_graphemes = self.pending.day_graphemes.saturating_add(graphemes);
            self.mark(now, a.cats);
        }
        let m = &mut self.mem;
        m.commits[m.commit_count % FLOW_COMMITS] = now.ms;
        m.commit_count = m.commit_count.saturating_add(1);
        let oldest = m.commits[m.commit_count % FLOW_COMMITS];
        let flowing = m.commit_count >= FLOW_COMMITS && now.ms - oldest <= FLOW_WINDOW_MS;
        let allowed = a.mode == Mode::Full
            && !a.assistive
            && !a.lightweight
            && !a.reduce_motion
            && !a.reduce_transparency
            && !now.doubtful
            && self.mem.flow.is_none();
        if !(flowing && allowed) {
            return None;
        }
        let plan = self.issue(Moment::Flow, a.cats & 3, Style::Motion, Ripple::None, 3000, 0);
        self.mem.flow = Some(plan.token);
        Some(plan)
    }

    fn idle(&mut self, a: &Ambience, now: Now, minute: u16) -> Option<Plan> {
        let ready = a.quiet_ms >= Self::doze_after(minute);
        if a.mode == Mode::Off
            || !ready
            || a.composing
            || a.lifted
            || a.dismissing
            || a.cats == 0
            || self.mem.dozing.is_some()
            || self.mem.peekaboo.is_some()
            || now.doubtful
        {
            return None;
        }
        let plan = self.issue(Moment::Doze, a.cats & 3, style(a), Ripple::None, 0, 0);
        self.mem.dozing = Some(plan.token);
        Some(plan)
    }

    fn touch(&mut self, kind: TouchKind, pet: Pet, a: &Ambience, now: Now) -> Option<Plan> {
        let i = pet_index(pet);
        match kind {
            TouchKind::Tap => {
                self.event(Event::Petted(pet));
                self.mark(now, who_of(pet));
            }
            TouchKind::Drop => {
                self.event(Event::DropAccepted(pet));
                self.mark(now, who_of(pet));
                return None;
            }
            TouchKind::Hold => self.mark(now, who_of(pet)),
            TouchKind::Seat => {}
        }
        if a.mode == Mode::Off || a.lifted {
            return None;
        }
        match kind {
            TouchKind::Seat => {
                let (token, who) = self.mem.peekaboo?;
                self.forget(token);
                self.release(token);
                Some(self.issue(Moment::Found, who, style(a), Ripple::None, 350, 0))
            }
            TouchKind::Hold => {
                self.mem.touched[i] = Some(now.ms);
                Some(self.issue(Moment::Purr, who_of(pet), style(a), Ripple::None, 4000, 0))
            }
            TouchKind::Tap => {
                self.mem.touched[i] = Some(now.ms);
                let other = self.mem.touched[1 - i];
                let both = other.is_some_and(|t| now.ms - t <= DUET_WINDOW_MS);
                if !both || a.cats & 3 != 3 || self.state.daily.duets >= DUET_DAILY || now.doubtful {
                    return None;
                }
                self.mem.touched = [None; 2];
                self.state.daily.duets += 1;
                self.pending.duets = self.pending.duets.saturating_add(1);
                Some(self.issue(Moment::Duet, 3, style(a), Ripple::None, 600, 0))
            }
            TouchKind::Drop => None,
        }
    }

    fn release(&mut self, token: u32) -> Option<Active> {
        let slot = self.mem.active.iter_mut().find(|a| a.moment.is_some() && a.token == token)?;
        let found = *slot;
        *slot = Active::default();
        Some(found)
    }

    /// The client reports how a plan ended. Unknown or stale tokens earn nothing.
    pub fn resolve(&mut self, token: u32, outcome: Outcome, clock: Clock) -> Reward {
        let now = self.now(clock);
        let Some(active) = self.release(token) else {
            return Reward::default();
        };
        self.forget(token);
        if outcome == Outcome::Cancelled {
            return Reward::default();
        }
        let mut who = 0;
        match active.moment {
            Some(Moment::Purr) => {
                let pet = if active.who == WHO_MIMIMO { Pet::Mimimo } else { Pet::Momo };
                let i = pet_index(pet);
                let spaced = self.mem.purr_rewarded[i].is_none_or(|t| now.ms - t >= PURR_SPACING_MS);
                if outcome == Outcome::Completed && spaced && self.state.daily.purrs[i] < PURR_DAILY && !now.doubtful {
                    self.mem.purr_rewarded[i] = Some(now.ms);
                    self.state.daily.purrs[i] += 1;
                    self.pending.purrs[i] = self.pending.purrs[i].saturating_add(1);
                    who = active.who;
                }
            }
            Some(Moment::Found | Moment::Duet) => who = active.who,
            _ => {}
        }
        for pet in [Pet::Mimimo, Pet::Momo] {
            if who & who_of(pet) != 0 {
                self.event(Event::Played(pet));
            }
        }
        if who != 0 {
            self.mark(now, who);
        }
        Reward { who }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u32 = 20_406;
    const NOON: u16 = 12 * 60;

    fn at(ms: u64) -> Clock {
        Clock { mono_ms: ms, day: DAY, minute: NOON }
    }

    /// A seed whose peekaboo roll succeeds (and one that fails).
    fn lucky() -> u64 {
        (0..).find(|s| mix(*s).is_multiple_of(PEEKABOO_ODDS)).unwrap()
    }
    fn unlucky() -> u64 {
        (0..).find(|s| !mix(*s).is_multiple_of(PEEKABOO_ODDS)).unwrap()
    }

    fn greeted() -> Director {
        let mut d = Director::default();
        let hello = d.decide(Trigger::Appear, &Ambience::default(), at(1_000), 0).unwrap();
        assert_eq!(hello.moment, Moment::Hello);
        d.resolve(hello.token, Outcome::Completed, at(2_000));
        d
    }

    fn appear(d: &mut Director, ms: u64, a: &Ambience) -> Option<Plan> {
        let p = d.decide(Trigger::Appear, a, at(ms), lucky());
        d.decide(Trigger::Disappear, a, at(ms + 1), 0);
        p
    }

    #[test]
    fn greeting_once_per_local_day_and_never_twice_when_the_day_goes_back() {
        let mut d = Director::default();
        let a = Ambience::default();
        let p = d.decide(Trigger::Appear, &a, at(0), 5).unwrap();
        assert_eq!((p.moment, p.who, p.style, p.duration_ms), (Moment::Hello, 3, Style::Motion, 900));
        assert_ne!(d.decide(Trigger::Appear, &a, at(10), unlucky()).map(|p| p.moment), Some(Moment::Hello));
        let back = Clock { day: DAY - 1, ..at(20) };
        assert_eq!(d.decide(Trigger::Appear, &a, back, unlucky()), None);
        let next = Clock { day: DAY + 1, ..at(30) };
        assert_eq!(d.decide(Trigger::Appear, &a, next, unlucky()).map(|p| p.moment), Some(Moment::Hello));
        let rm = Ambience { reduce_motion: true, ..a };
        let next2 = Clock { day: DAY + 2, ..at(40) };
        assert_eq!(d.decide(Trigger::Appear, &rm, next2, 0).map(|p| p.style), Some(Style::Fade));
    }

    #[test]
    fn peekaboo_limits_bucket_session_and_day() {
        let mut d = greeted();
        let a = Ambience::default();
        // The first appearance of the day is the greeting, never a peekaboo.
        let p = appear(&mut d, 3_000, &a).unwrap();
        assert_eq!(p.moment, Moment::Peekaboo);
        assert!((3_000..=9_000).contains(&p.delay_ms));
        assert_eq!((p.style, p.ripple, p.duration_ms), (Style::Motion, Ripple::Soft, 280));
        // Second token is still in the bucket; session cap resets per appearance.
        assert!(appear(&mut d, 4_000, &a).is_some());
        // Bucket empty now (2 used within 120 s).
        assert_eq!(appear(&mut d, 5_000, &a), None);
        // One token back 120 s after the bucket was last full (at 3 s).
        assert_eq!(appear(&mut d, 122_999, &a), None);
        assert!(appear(&mut d, 123_000, &a).is_some());
        // Daily cap 3.
        assert_eq!(d.state().daily.spontaneous, 3);
        assert_eq!(appear(&mut d, 3_000 + 600_000, &a), None);
        // Next day: new allowance after its greeting.
        let next = Clock { mono_ms: 900_000, day: DAY + 1, minute: NOON };
        assert_eq!(d.decide(Trigger::Appear, &a, next, lucky()).map(|p| p.moment), Some(Moment::Hello));
        d.decide(Trigger::Disappear, &a, next, 0);
        let later = Clock { mono_ms: 900_100, ..next };
        assert_eq!(d.decide(Trigger::Appear, &a, later, lucky()).map(|p| p.moment), Some(Moment::Peekaboo));
    }

    #[test]
    fn session_cap_is_one() {
        let mut d = greeted();
        let a = Ambience::default();
        assert!(d.decide(Trigger::Appear, &a, at(3_000), lucky()).is_some());
        assert_eq!(d.state().daily.spontaneous, 1);
        assert!(!d.budget(), "same session: no second spontaneous moment");
    }

    #[test]
    fn spontaneous_moments_are_suppressed() {
        let base = Ambience::default();
        let cases = [
            Ambience { composing: true, ..base },
            Ambience { quiet_ms: 599, ..base },
            Ambience { lifted: true, ..base },
            Ambience { dismissing: true, ..base },
            Ambience { assistive: true, ..base },
            Ambience { lightweight: true, ..base },
            Ambience { mode: Mode::Quiet, ..base },
            Ambience { mode: Mode::Off, ..base },
            Ambience { cats: 0, ..base },
        ];
        for a in cases {
            let mut d = greeted();
            assert_eq!(appear(&mut d, 3_000, &a), None, "{a:?}");
            assert_eq!(d.state().daily.spontaneous, 0);
            // Flow is a typing-time glow: only the mode, assistive tech and
            // lightweight effects stop it (plus Reduce Motion/Transparency).
            if !(a.assistive || a.lightweight || a.mode != Mode::Full) {
                continue;
            }
            for n in 0..20 {
                assert_ne!(d.decide(Trigger::Commit(3), &a, at(4_000 + n * 100), 0).map(|p| p.moment), Some(Moment::Flow), "{a:?}");
            }
        }
        let mut d = greeted();
        assert!(appear(&mut d, 3_000, &Ambience { quiet_ms: 600, ..base }).is_some());
    }

    #[test]
    fn quiet_keeps_greeting_doze_and_what_the_user_starts_off_keeps_nothing() {
        let quiet = Ambience { mode: Mode::Quiet, ..Ambience::default() };
        let mut d = Director::default();
        assert_eq!(d.decide(Trigger::Appear, &quiet, at(0), 0).map(|p| p.moment), Some(Moment::Hello));
        let hold = d.decide(Trigger::Touch(TouchKind::Hold, Pet::Momo), &quiet, at(10), 0).unwrap();
        assert_eq!((hold.moment, hold.who), (Moment::Purr, WHO_MOMO));
        let idle = Ambience { quiet_ms: 25_000, ..quiet };
        assert_eq!(d.decide(Trigger::Idle, &idle, at(30_000), 0).map(|p| p.moment), Some(Moment::Doze));

        let off = Ambience { mode: Mode::Off, quiet_ms: 60_000, ..Ambience::default() };
        let mut d = Director::default();
        for t in [
            Trigger::Appear,
            Trigger::Idle,
            Trigger::Touch(TouchKind::Hold, Pet::Momo),
            Trigger::Touch(TouchKind::Seat, Pet::Momo),
            Trigger::Touch(TouchKind::Tap, Pet::Momo),
            Trigger::Touch(TouchKind::Tap, Pet::Mimimo),
        ] {
            assert_eq!(d.decide(t, &off, at(100), 0), None, "{t:?}");
        }
        assert_eq!(d.idle_delay_ms(&off, at(0)), NEVER);
        // Taps still count for the level in off mode.
        assert_eq!(d.state().snapshot.momo.pets, 1);
    }

    #[test]
    fn clock_going_backwards_is_doubt() {
        let mut d = greeted();
        let a = Ambience::default();
        d.decide(Trigger::Commit(1), &a, at(50_000), 0);
        assert_eq!(appear(&mut d, 40_000, &a), None, "mono ran backwards");
        // Time stays clamped: no extra refill from the jump.
        assert!(appear(&mut d, 50_001, &a).is_some());
        // Unknown day or minute: no spontaneous moment, no greeting.
        let mut d = Director::default();
        assert_eq!(d.decide(Trigger::Appear, &a, Clock { day: 0, ..at(0) }, 0), None);
        assert_eq!(d.decide(Trigger::Appear, &a, Clock { minute: 1440, ..at(1) }, 0), None);
        assert!(!d.state().daily.greeted);
    }

    #[test]
    fn purr_rewards_are_rate_limited() {
        let mut d = greeted();
        let a = Ambience::default();
        let mut rewarded = 0;
        let mut ms = 10_000;
        for _ in 0..40 {
            let p = d.decide(Trigger::Touch(TouchKind::Hold, Pet::Mimimo), &a, at(ms), 0).unwrap();
            assert_eq!(p.moment, Moment::Purr, "the response is never budgeted");
            if d.resolve(p.token, Outcome::Completed, at(ms + 1_000)).who == WHO_MIMIMO {
                rewarded += 1;
            }
            ms += 4_000;
        }
        // 40 purrs over 160 s, 8 s apart at most: 20 by spacing, capped at 12 a day.
        assert_eq!(rewarded, 12);
        assert_eq!(d.state().snapshot.mimimo.plays, 12);
        assert_eq!(d.state().snapshot.momo.plays, 0);
        let p = d.decide(Trigger::Touch(TouchKind::Hold, Pet::Momo), &a, at(ms), 0).unwrap();
        assert_eq!(d.resolve(p.token, Outcome::Completed, at(ms)).who, WHO_MOMO, "per cat");
        let p = d.decide(Trigger::Touch(TouchKind::Hold, Pet::Momo), &a, at(ms + 100_000), 0).unwrap();
        assert_eq!(d.resolve(p.token, Outcome::Interrupted, at(ms + 100_000)).who, 0);
        assert_eq!(d.resolve(p.token, Outcome::Completed, at(ms + 100_000)).who, 0, "stale token");
        // Level unchanged by plays.
        assert_eq!(d.state().snapshot.progress(Pet::Mimimo).level, 0);
    }

    #[test]
    fn peekaboo_found_and_duet() {
        let mut d = greeted();
        let a = Ambience::default();
        assert_eq!(d.decide(Trigger::Touch(TouchKind::Seat, Pet::Momo), &a, at(2_500), 0), None, "nothing hiding");
        let p = d.decide(Trigger::Appear, &a, at(3_000), lucky()).unwrap();
        let found = d.decide(Trigger::Touch(TouchKind::Seat, Pet::Momo), &a, at(4_000), 0).unwrap();
        assert_eq!((found.moment, found.who, found.duration_ms), (Moment::Found, p.who, 350));
        assert_eq!(d.resolve(p.token, Outcome::Completed, at(4_100)).who, 0, "the peekaboo itself earns nothing");
        assert_eq!(d.resolve(found.token, Outcome::Completed, at(4_400)).who, p.who);
        // Duet: both cats touched within 60 s, twice a day.
        let tap = |d: &mut Director, pet, ms| d.decide(Trigger::Touch(TouchKind::Tap, pet), &a, at(ms), 0);
        assert_eq!(tap(&mut d, Pet::Momo, 10_000), None);
        assert_eq!(tap(&mut d, Pet::Mimimo, 80_001), None, "61 s apart");
        let duet = tap(&mut d, Pet::Momo, 81_000).unwrap();
        assert_eq!((duet.moment, duet.who, duet.duration_ms), (Moment::Duet, 3, 600));
        assert_eq!(d.resolve(duet.token, Outcome::Completed, at(82_000)).who, 3);
        assert_eq!(tap(&mut d, Pet::Momo, 90_000), None);
        assert!(tap(&mut d, Pet::Mimimo, 90_100).is_some());
        assert_eq!(tap(&mut d, Pet::Momo, 91_000), None);
        assert_eq!(tap(&mut d, Pet::Mimimo, 91_100), None, "two duets a day");
        let one = Ambience { cats: WHO_MOMO, ..a };
        let mut d = greeted();
        d.decide(Trigger::Touch(TouchKind::Tap, Pet::Mimimo), &one, at(3_000), 0);
        assert_eq!(d.decide(Trigger::Touch(TouchKind::Tap, Pet::Momo), &one, at(3_100), 0), None, "one cat on screen");
    }

    #[test]
    fn peekaboo_falls_back_to_a_fade() {
        for (a, style, ripple) in [
            (Ambience { glass_seat: false, ..Ambience::default() }, Style::Fade, Ripple::None),
            (Ambience { reduce_motion: true, ..Ambience::default() }, Style::Fade, Ripple::None),
            (Ambience { reduce_transparency: true, ..Ambience::default() }, Style::Motion, Ripple::Line),
        ] {
            let mut d = greeted();
            let p = d.decide(Trigger::Appear, &a, at(3_000), lucky()).unwrap();
            assert_eq!((p.style, p.ripple), (style, ripple));
            assert_eq!(p.duration_ms, if style == Style::Fade { 250 } else { 280 });
        }
    }

    #[test]
    fn doze_and_idle_timer() {
        let mut d = greeted();
        let mut a = Ambience { quiet_ms: 10_000, ..Ambience::default() };
        assert_eq!(d.idle_delay_ms(&a, at(0)), 15_000);
        assert_eq!(d.decide(Trigger::Idle, &a, at(12_000), 0), None);
        let late = Clock { minute: 23 * 60 + 30, ..at(12_000) };
        assert_eq!(d.idle_delay_ms(&a, late), 2_000);
        a.quiet_ms = 12_000;
        let p = d.decide(Trigger::Idle, &a, late, 0).unwrap();
        assert_eq!((p.moment, p.duration_ms), (Moment::Doze, 0));
        assert_eq!(d.idle_delay_ms(&a, late), NEVER, "already dozing");
        assert_eq!(d.decide(Trigger::Idle, &a, late, 0), None);
        d.resolve(p.token, Outcome::Completed, at(13_000));
        a.quiet_ms = 30_000;
        assert_eq!(d.idle_delay_ms(&a, at(0)), 0);
    }

    #[test]
    fn flow_needs_eight_commits_in_thirty_seconds() {
        let mut d = greeted();
        let a = Ambience { quiet_ms: 0, composing: false, ..Ambience::default() };
        let mut plan = None;
        for n in 0..8u64 {
            plan = d.decide(Trigger::Commit(2), &a, at(10_000 + n * 4_000), 0);
            if n < 7 {
                assert_eq!(plan, None);
            }
        }
        let p = plan.unwrap();
        assert_eq!(p.moment, Moment::Flow);
        assert_eq!(d.decide(Trigger::Commit(2), &a, at(39_000), 0), None, "already flowing");
        d.resolve(p.token, Outcome::Completed, at(45_000));
        let mut d = greeted();
        for n in 0..8u64 {
            assert_eq!(d.decide(Trigger::Commit(2), &a, at(10_000 + n * 4_400), 0), None, "30.8 s window");
        }
        assert_eq!(d.state().daily.graphemes, 16);
        assert_eq!(d.state().snapshot.committed_graphemes, 16);
    }

    #[test]
    fn rest_pose_by_day_part_on_edges_only() {
        let a = Ambience::default();
        assert_eq!(rest_pose(Pet::Momo, &a, 6 * 60), RestPose::EdgeRest);
        assert_eq!(rest_pose(Pet::Momo, &a, NOON), RestPose::EdgePeek);
        assert_eq!(rest_pose(Pet::Momo, &a, 18 * 60), RestPose::EdgeLounge);
        assert_eq!(rest_pose(Pet::Mimimo, &a, 21 * 60), RestPose::EdgeLounge);
        assert_eq!(rest_pose(Pet::Mimimo, &a, 2 * 60), RestPose::Sleep);
        assert_eq!(rest_pose(Pet::Momo, &a, 2 * 60), RestPose::EdgeLounge);
        assert_eq!(rest_pose(Pet::Momo, &Ambience { edges: false, ..a }, NOON), RestPose::Keep);
        assert_eq!(rest_pose(Pet::Momo, &Ambience { festival: true, ..a }, NOON), RestPose::Keep);
        assert_eq!(rest_pose(Pet::Momo, &a, 1440), RestPose::Keep);
    }

    #[test]
    fn book_and_delta_follow_activity() {
        let mut d = greeted();
        let a = Ambience { cats: WHO_MOMO, ..Ambience::default() };
        d.decide(Trigger::Commit(600), &a, at(3_000), 0);
        let night = Clock { minute: 21 * 60, ..at(4_000) };
        d.decide(Trigger::Touch(TouchKind::Tap, Pet::Mimimo), &a, night, 0);
        let mark = d.state().book.get(DAY).unwrap();
        assert_eq!((mark.size, mark.who, mark.part), (2, 3, DayPart::Day.bit() | DayPart::Night.bit()));
        let delta = d.take_delta();
        assert_eq!((delta.graphemes, delta.day_graphemes, delta.contacts[0].pets, delta.greeted), (600, 600, 1, true));
        assert!(d.take_delta().is_empty());
        // Replaying the delta on an empty file reproduces the state.
        let mut disk = PetState::default();
        disk.apply_delta(&delta);
        disk.merge(d.state());
        assert_eq!(&disk, d.state());
    }

    /// Typical calls stay far below the 5 µs budget (decide never allocates).
    #[test]
    fn decide_is_fast() {
        let mut d = greeted();
        let a = Ambience::default();
        let triggers = [
            Trigger::Commit(3),
            Trigger::Idle,
            Trigger::Touch(TouchKind::Tap, Pet::Momo),
            Trigger::Appear,
            Trigger::Touch(TouchKind::Hold, Pet::Mimimo),
            Trigger::Disappear,
        ];
        let n = 200_000u64;
        let start = std::time::Instant::now();
        let mut plans = 0u64;
        for i in 0..n {
            let t = triggers[(i % 6) as usize];
            if let Some(p) = d.decide(t, &a, at(10_000 + i * 50), i) {
                plans += 1;
                d.resolve(p.token, Outcome::Completed, at(10_000 + i * 50));
            }
        }
        let per_call = start.elapsed().as_nanos() as f64 / n as f64;
        assert!(plans > 0);
        assert!(per_call < 5_000.0, "decide+resolve took {per_call:.0} ns");
        eprintln!("decide (+resolve when planned): {per_call:.0} ns per call");
    }
}
