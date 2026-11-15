//! Runner for `Tests/pets-moments.json`, shared by the pets crate test and the
//! C ABI test (`#[path]`-included there). A tiny JSON reader is enough for
//! the file; the workspace takes no external crates.
#![allow(dead_code)]

use std::collections::BTreeMap;

use meowpuut_pets::codec::PetState;
use meowpuut_pets::moments::{Ambience, Clock, Mode, Moment, Outcome, Plan, Ripple, Style, TouchKind, Trigger};
use meowpuut_pets::Pet;

pub const GOLDEN: &str = include_str!("../../../../Tests/pets-moments.json");

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(i64),
    Str(String),
    Arr(Vec<Json>),
    Obj(BTreeMap<String, Json>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(map) => map.get(key),
            _ => None,
        }
    }
    pub fn num(&self) -> i64 {
        match self {
            Json::Num(n) => *n,
            other => panic!("number expected, got {other:?}"),
        }
    }
    pub fn str(&self) -> &str {
        match self {
            Json::Str(s) => s,
            other => panic!("string expected, got {other:?}"),
        }
    }
    pub fn arr(&self) -> &[Json] {
        match self {
            Json::Arr(a) => a,
            other => panic!("array expected, got {other:?}"),
        }
    }
    pub fn obj(&self) -> &BTreeMap<String, Json> {
        match self {
            Json::Obj(o) => o,
            other => panic!("object expected, got {other:?}"),
        }
    }
    pub fn bool(&self) -> bool {
        match self {
            Json::Bool(b) => *b,
            other => panic!("bool expected, got {other:?}"),
        }
    }
}

pub fn parse(text: &str) -> Json {
    let mut p = Parser { s: text.as_bytes(), i: 0 };
    let v = p.value();
    p.ws();
    assert_eq!(p.i, p.s.len(), "trailing data in JSON");
    v
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn eat(&mut self, c: u8) {
        self.ws();
        assert_eq!(self.s.get(self.i), Some(&c), "expected '{}' at byte {}", c as char, self.i);
        self.i += 1;
    }
    fn lit(&mut self, word: &str, v: Json) -> Json {
        assert!(self.s[self.i..].starts_with(word.as_bytes()), "bad literal at byte {}", self.i);
        self.i += word.len();
        v
    }
    fn value(&mut self) -> Json {
        self.ws();
        match self.s[self.i] {
            b'{' => {
                self.i += 1;
                let mut map = BTreeMap::new();
                self.ws();
                if self.s[self.i] == b'}' {
                    self.i += 1;
                    return Json::Obj(map);
                }
                loop {
                    self.ws();
                    let Json::Str(key) = self.string() else { unreachable!() };
                    self.eat(b':');
                    let v = self.value();
                    assert!(map.insert(key.clone(), v).is_none(), "duplicate key {key}");
                    self.ws();
                    match self.s[self.i] {
                        b',' => self.i += 1,
                        b'}' => {
                            self.i += 1;
                            return Json::Obj(map);
                        }
                        c => panic!("unexpected '{}' at byte {}", c as char, self.i),
                    }
                }
            }
            b'[' => {
                self.i += 1;
                let mut items = Vec::new();
                self.ws();
                if self.s[self.i] == b']' {
                    self.i += 1;
                    return Json::Arr(items);
                }
                loop {
                    items.push(self.value());
                    self.ws();
                    match self.s[self.i] {
                        b',' => self.i += 1,
                        b']' => {
                            self.i += 1;
                            return Json::Arr(items);
                        }
                        c => panic!("unexpected '{}' at byte {}", c as char, self.i),
                    }
                }
            }
            b'"' => self.string(),
            b't' => self.lit("true", Json::Bool(true)),
            b'f' => self.lit("false", Json::Bool(false)),
            b'n' => self.lit("null", Json::Null),
            _ => {
                let start = self.i;
                if self.s[self.i] == b'-' {
                    self.i += 1;
                }
                while self.i < self.s.len() && self.s[self.i].is_ascii_digit() {
                    self.i += 1;
                }
                let text = std::str::from_utf8(&self.s[start..self.i]).unwrap();
                Json::Num(text.parse().unwrap_or_else(|_| panic!("bad number at byte {start}")))
            }
        }
    }
    fn string(&mut self) -> Json {
        self.eat(b'"');
        let mut out = Vec::new();
        loop {
            let c = self.s[self.i];
            self.i += 1;
            match c {
                b'"' => break,
                b'\\' => {
                    let e = self.s[self.i];
                    self.i += 1;
                    out.push(match e {
                        b'n' => b'\n',
                        b't' => b'\t',
                        b'"' | b'\\' | b'/' => e,
                        _ => panic!("unsupported escape \\{}", e as char),
                    });
                }
                _ => out.push(c),
            }
        }
        Json::Str(String::from_utf8(out).unwrap())
    }
}

/// What the runner needs from a director: the Rust type or the C ABI.
pub trait Driver {
    fn decide(&mut self, trigger: Trigger, ambience: &Ambience, clock: Clock, seed: u64) -> Option<Plan>;
    fn resolve(&mut self, token: u32, outcome: Outcome, clock: Clock) -> u8;
    fn state(&self) -> PetState;
}

fn pet(v: Option<&Json>) -> Pet {
    match v.map(Json::str) {
        Some("mimimo") => Pet::Mimimo,
        Some("momo") => Pet::Momo,
        other => panic!("pet expected, got {other:?}"),
    }
}

pub fn ambience(defaults: &Json, overrides: Option<&Json>) -> Ambience {
    let mut fields = defaults.obj().clone();
    if let Some(o) = overrides {
        for (k, v) in o.obj() {
            assert!(fields.contains_key(k), "unknown ambience field {k}");
            fields.insert(k.clone(), v.clone());
        }
    }
    let b = |k: &str| fields[k].bool();
    let a = Ambience {
        composing: b("composing"),
        quiet_ms: u32::try_from(fields["quiet_ms"].num()).unwrap(),
        lifted: b("lifted"),
        dismissing: b("dismissing"),
        assistive: b("assistive"),
        lightweight: b("lightweight"),
        reduce_motion: b("reduce_motion"),
        reduce_transparency: b("reduce_transparency"),
        glass_seat: b("glass_seat"),
        edges: b("edges"),
        festival: b("festival"),
        cats: u8::try_from(fields["cats"].num()).unwrap(),
        mode: match fields["mode"].str() {
            "full" => Mode::Full,
            "quiet" => Mode::Quiet,
            "off" => Mode::Off,
            m => panic!("mode {m}"),
        },
    };
    assert_eq!(fields.len(), 13, "ambience fields");
    a
}

pub fn moment_name(m: Moment) -> &'static str {
    match m {
        Moment::Hello => "hello",
        Moment::Purr => "purr",
        Moment::Peekaboo => "peekaboo",
        Moment::Found => "found",
        Moment::Doze => "doze",
        Moment::Duet => "duet",
        Moment::Flow => "flow",
    }
}

fn plan_json(p: &Plan) -> String {
    let style = match p.style {
        Style::Motion => "motion",
        Style::Fade => "fade",
    };
    let ripple = match p.ripple {
        Ripple::None => "none",
        Ripple::Soft => "soft",
        Ripple::Line => "line",
    };
    format!(
        "token={} moment={} who={} style={} ripple={} duration_ms={} delay_ms={}",
        p.token,
        moment_name(p.moment),
        p.who,
        style,
        ripple,
        p.duration_ms,
        p.delay_ms
    )
}

fn expected(e: &Json) -> Option<String> {
    if *e == Json::Null {
        return None;
    }
    let o = e.obj();
    assert_eq!(o.len(), 7, "every plan field is listed");
    Some(format!(
        "token={} moment={} who={} style={} ripple={} duration_ms={} delay_ms={}",
        o["token"].num(),
        o["moment"].str(),
        o["who"].num(),
        o["style"].str(),
        o["ripple"].str(),
        o["duration_ms"].num(),
        o["delay_ms"].num()
    ))
}

/// Runs every case with a fresh driver; returns the number of steps checked.
pub fn run(mut fresh: impl FnMut() -> Box<dyn Driver>) -> usize {
    let root = parse(GOLDEN);
    assert_eq!(root.get("format").map(Json::str), Some("meowpuut-pets-moments-1"));
    assert_eq!(root.get("state_bytes").map(Json::num), Some(meowpuut_pets::codec::STATE_BYTES as i64));
    assert_eq!(root.get("delta_bytes").map(Json::num), Some(meowpuut_pets::codec::DELTA_BYTES as i64));
    let defaults = root.get("ambience_default").expect("ambience_default");
    let mut checked = 0;
    for case in root.get("cases").unwrap().arr() {
        let name = case.get("name").unwrap().str();
        let mut d = fresh();
        for (n, step) in case.get("steps").unwrap().arr().iter().enumerate() {
            let at = |s: &Json| {
                let a = s.get("at").unwrap().arr();
                Clock {
                    mono_ms: u64::try_from(a[0].num()).unwrap(),
                    day: u32::try_from(a[1].num()).unwrap(),
                    minute: u16::try_from(a[2].num()).unwrap(),
                }
            };
            let ctx = format!("{name}, step {n}");
            if let Some(kind) = step.get("decide") {
                let trigger = match kind.str() {
                    "appear" => Trigger::Appear,
                    "commit" => Trigger::Commit(u32::try_from(step.get("graphemes").unwrap().num()).unwrap()),
                    "idle" => Trigger::Idle,
                    "disappear" => Trigger::Disappear,
                    "tap" => Trigger::Touch(TouchKind::Tap, pet(step.get("pet"))),
                    "hold" => Trigger::Touch(TouchKind::Hold, pet(step.get("pet"))),
                    "seat" => Trigger::Touch(TouchKind::Seat, pet(step.get("pet"))),
                    "drop" => Trigger::Touch(TouchKind::Drop, pet(step.get("pet"))),
                    other => panic!("{ctx}: trigger {other}"),
                };
                let a = ambience(defaults, step.get("ambience"));
                let seed = step.get("seed").map_or(0, Json::num) as u64;
                let got = d.decide(trigger, &a, at(step), seed).as_ref().map(plan_json);
                assert_eq!(got, expected(step.get("expect").expect("expect")), "{ctx}");
            } else if let Some(token) = step.get("resolve") {
                let outcome = match step.get("outcome").unwrap().str() {
                    "completed" => Outcome::Completed,
                    "interrupted" => Outcome::Interrupted,
                    "cancelled" => Outcome::Cancelled,
                    o => panic!("{ctx}: outcome {o}"),
                };
                let got = d.resolve(u32::try_from(token.num()).unwrap(), outcome, at(step));
                assert_eq!(i64::from(got), step.get("reward").unwrap().num(), "{ctx}");
            } else if let Some(check) = step.get("check") {
                let s = d.state();
                for (k, v) in check.obj() {
                    let pair = |a: u64, b: u64| v.arr().iter().map(Json::num).collect::<Vec<_>>() == vec![a as i64, b as i64];
                    let ok = match k.as_str() {
                        "spontaneous" => i64::from(s.daily.spontaneous) == v.num(),
                        "greeted" => s.daily.greeted == v.bool(),
                        "graphemes" => s.snapshot.committed_graphemes as i64 == v.num(),
                        "plays" => pair(s.snapshot.mimimo.plays, s.snapshot.momo.plays),
                        "pets" => pair(s.snapshot.mimimo.pets, s.snapshot.momo.pets),
                        other => panic!("{ctx}: check {other}"),
                    };
                    assert!(ok, "{ctx}: {k} = {v:?}, state {s:?}");
                }
            } else {
                panic!("{ctx}: unknown step");
            }
            checked += 1;
        }
    }
    checked
}
