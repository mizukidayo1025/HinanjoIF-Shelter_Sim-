//! The decisions a Run makes, in the core (Issue #225, lane M11).
//!
//! The world is the generated Scenario the Project screen makes, at minute 0 with the Run's
//! policies (`generated_texts.json`'s `generated`). The core runs its first 90 ticks
//! (`support/ticks.rs`). The decisions' records this test pinned too are no longer written (#556).
//!
//! The test pins the digest of the world (`WORLD`): the first 16 hex of the sha256 of
//! `json.dumps(value, sort_keys=True)` of the world's canonical JSON (`support/canonical.rs`, with
//! the `rng_state` the engine gives the world it steps). The value was recorded before #430 and
//! pins the core's output now.

use crate::support;

use std::rc::Rc;
use std::sync::Arc;

use sheltersim_core::value::V;
use sheltersim_core::world::WorldCell;

use support::canonical::canonical_json;
use support::dumps::Dumps;
use support::load;
use support::texts::sha;
use support::{cases, ticks};

/// The recorded digest of the world.
const WORLD: &str = "ae0c6872ec1e5041";

/// The first 16 hex of the sha256 of `json.dumps(value, sort_keys=True)`.
fn digest(value: &V) -> String {
    sha(&Dumps::default().sorted().text(value))[..16].to_owned()
}

#[test]
fn a_run_gives_the_recorded_world() {
    let entry = cases::entry("generated_texts", "generated");
    let loaded = Rc::new(load::loaded(&entry));
    let mut host = load::following_host(&entry, &loaded);
    ticks::run(&loaded, &mut host, 90, &mut |_, _| {});
    // The `rng_state` the engine gives the world it steps: its keyed draws' state.
    let rng_state = serde_json::json!({"algorithm": "sha256-keyed-v1", "seed": entry["world"]["seed"]});
    let canonical = loaded.cell.with(|world| canonical_json(world, &rng_state)).unwrap();
    assert_eq!(digest(&V::Str(Arc::from(String::from_utf8(canonical).unwrap()))), WORLD);
}
