//! `Tests/pets-moments.json` against the Rust director. The C ABI test
//! (`meowpuut-ffi/tests/pets_abi.rs`) runs the same file through `mp_pets_*`.

#[path = "support/golden.rs"]
mod golden;

use meowpuut_pets::codec::PetState;
use meowpuut_pets::moments::{Ambience, Clock, Director, Outcome, Plan, Trigger};

impl golden::Driver for Director {
    fn decide(&mut self, trigger: Trigger, ambience: &Ambience, clock: Clock, seed: u64) -> Option<Plan> {
        Director::decide(self, trigger, ambience, clock, seed)
    }
    fn resolve(&mut self, token: u32, outcome: Outcome, clock: Clock) -> u8 {
        Director::resolve(self, token, outcome, clock).who
    }
    fn state(&self) -> PetState {
        Director::state(self).clone()
    }
}

#[test]
fn golden_moments() {
    let steps = golden::run(|| Box::new(Director::default()));
    assert!(steps >= 80, "only {steps} steps");
}

#[test]
fn parser_reads_what_the_file_uses() {
    let v = golden::parse(r#"{"a": [1, -2, true, false, null, "x\"y"], "b": {}}"#);
    assert_eq!(v.get("a").unwrap().arr().len(), 6);
    assert_eq!(v.get("a").unwrap().arr()[1].num(), -2);
    assert_eq!(v.get("a").unwrap().arr()[5].str(), "x\"y");
}
