//! A handler's suppressed and failed texts, byte for byte, raised by the core itself (Issue #225,
//! lane M1).
//!
//! These texts are saved: the dispatcher writes them to the event's record as `detail`. Each case
//! is one event that reaches one branch of one handler on a recorded world (`data/event_texts.json`
//! and `data/rumor_texts.json`: the gate's generated Scenario at minute 5, with its operational
//! conditions at minute 25, with its rumors at minute 40). The core's handler
//! (`dispatch::apply_alone`) must raise the outcome itself (not a refusal), as
//! `EventStatus::Suppressed` or `Failed`, with its recorded text, and change nothing
//! (`support/cases.rs`). Each test checks one group of the fixture's cases. The values were
//! recorded before #430 and pin the core's output now.

use crate::support;

use sheltersim_core::checkpoint::sha256_hex;
use sheltersim_core::world::WorldCell;

use support::cases::{check_group, resolve};
use support::{canonical, load};

const FILE: &str = "event_texts";
const RUMORS: &str = "rumor_texts";

#[test]
fn a_loaded_world_writes_the_recorded_canonical_json() {
    for (file, world) in [(FILE, "small"), (RUMORS, "rumor")] {
        let data = load::data(file);
        let entry = resolve(&data["worlds"], world);
        let loaded = load::loaded(&entry);
        let written = loaded.cell.with(|world| canonical::canonical_json(world, &entry["world"]["rng_state"])).unwrap();
        assert_eq!(sha256_hex(&written), data["canonical_sha256"][world].as_str().unwrap(), "{world}");
    }
}

#[test]
fn a_facility_change_s_text_is_the_recorded_one() {
    check_group(FILE, "a_facility_change_s_text_is_python_s", 16);
}

#[test]
fn an_outage_under_an_infrastructure_outage_fails_with_the_recorded_text() {
    check_group(FILE, "an_outage_under_an_infrastructure_outage_is_python_s_to_fail", 2);
}

#[test]
fn a_proximity_contact_without_its_model_fails_with_the_recorded_text() {
    check_group(FILE, "a_proximity_contact_without_its_model_is_python_s_to_fail", 1);
}

#[test]
fn a_delivery_of_an_unknown_shipment_fails_with_the_recorded_text() {
    check_group(FILE, "a_delivery_of_an_unknown_shipment_is_python_s_to_fail", 1);
}

#[test]
fn a_delay_s_texts_are_the_recorded_ones() {
    check_group(FILE, "a_delay_s_texts_are_python_s", 8);
}

#[test]
fn water_texts_are_the_recorded_ones() {
    check_group(FILE, "water_texts_are_python_s", 4);
}

#[test]
fn staff_absence_texts_are_the_recorded_ones() {
    check_group(FILE, "staff_absence_texts_are_python_s", 10);
}

#[test]
fn infrastructure_texts_are_the_recorded_ones() {
    check_group(FILE, "infrastructure_texts_are_python_s", 10);
}

#[test]
fn population_primitive_texts_are_the_recorded_ones() {
    check_group(FILE, "population_primitive_texts_are_python_s", 4);
}

#[test]
fn an_information_event_without_the_model_fails_with_the_recorded_text() {
    check_group(FILE, "an_information_event_without_the_model_is_python_s_to_fail", 1);
}

#[test]
fn rumor_texts_are_the_recorded_ones() {
    check_group(RUMORS, "rumor_texts_are_python_s", 7);
}

#[test]
fn correction_texts_are_the_recorded_ones() {
    check_group(RUMORS, "correction_texts_are_python_s", 9);
}

#[test]
fn contact_texts_are_the_recorded_ones() {
    check_group(RUMORS, "contact_texts_are_python_s", 18);
}

#[test]
fn staff_operation_texts_are_the_recorded_ones() {
    check_group(FILE, "staff_operation_texts_are_python_s", 7);
}

#[test]
fn a_staff_support_completion_s_texts_are_the_recorded_ones() {
    check_group(FILE, "a_staff_support_completion_s_texts_are_python_s", 3);
}

#[test]
fn an_effect_s_facility_modifier_texts_are_the_recorded_ones() {
    check_group(FILE, "an_effect_s_facility_modifier_texts_are_python_s", 4);
}

#[test]
fn the_loop_s_handlers_texts_are_the_recorded_ones() {
    check_group(FILE, "the_loop_s_handlers_texts_are_python_s", 7);
}

#[test]
fn a_typed_social_event_s_first_texts_are_the_recorded_ones() {
    check_group(FILE, "a_typed_social_event_s_first_texts_are_python_s", 15);
}

#[test]
fn an_outage_s_later_texts_are_the_recorded_ones() {
    check_group(FILE, "an_outage_s_later_texts_are_python_s", 5);
}
