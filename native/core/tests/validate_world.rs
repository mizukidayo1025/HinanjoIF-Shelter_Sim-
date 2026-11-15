//! The rules of the whole world's validation that #347 moved into the core (#250): each broken
//! world of `validate_selection.json`, `validate_service.json`, `validate_information.json` and
//! `validate_events.json` gives the recorded first error, with its class and text, at each entry
//! point (`sheltersim_core::validate`). The cases were recorded before #430 and pin the core's
//! output now.

use crate::support;

use support::validate::check_file;

#[test]
fn the_selected_models_and_the_declared_states_give_the_recorded_errors() {
    assert!(check_file("validate_selection") > 50);
}

#[test]
fn the_resource_service_gives_the_recorded_errors() {
    assert!(check_file("validate_service") > 40);
}

#[test]
fn the_information_spread_and_the_distress_give_the_recorded_errors() {
    assert!(check_file("validate_information") > 50);
}

#[test]
fn the_event_rules_give_the_recorded_errors() {
    assert!(check_file("validate_events") > 10);
}
