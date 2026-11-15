//! The entity rules of the whole world's validation (`sheltersim_core::validate`, #250, #348):
//! `validate::entities`, the entities' rules and the zones' capacity. Each broken world of
//! `validate_entities.json` gives the recorded class and text at each entry point, and the
//! accepted ones pass. The cases were recorded before #430 and pin the core's output now.

use crate::support;

use std::sync::Arc;

use sheltersim_core::world::World;
use support::validate;

#[test]
fn each_broken_world_gives_the_recorded_first_entity_error() {
    assert!(validate::check_file("validate_entities") >= 140);
}

/// A case of `validate_entity_rows.json`, its world changed here as the case's edit needs: a
/// WorldState v5 document holds no hydration need and no resource slots, so the loader reads none.
fn check_row_case(case: &str, change: impl FnOnce(&World)) {
    let world = validate::world_of("validate_entity_rows", case);
    change(&world);
    let got = validate::outcomes(&world, None);
    let differences = validate::differences(case, &validate::expected_of("validate_entity_rows", case), &got);
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

#[test]
fn a_v5_world_s_hydration_need_and_resource_slots_give_the_recorded_errors() {
    check_row_case("v5_hydration_need", |world| {
        world.rows_mut().unwrap().agents.get_mut("A003").unwrap().needs.hydration_need = 0.1;
    });
    check_row_case("v5_initial_hydration_need", |world| {
        world.rows_mut().unwrap().agents.get_mut("A003").unwrap().profile.initial_needs.hydration_need = 0.1;
    });
    check_row_case("v5_resource_occupancy", |world| {
        world.rows_mut().unwrap().facilities.get_mut("toilet_4").unwrap().resource_queue_agent_ids.push(Arc::from("A003"));
    });
}
