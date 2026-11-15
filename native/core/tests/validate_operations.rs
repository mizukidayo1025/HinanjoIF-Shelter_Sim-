//! The operational events and the infrastructure of the whole world's validation
//! (`sheltersim_core::validate`, #250, #348): `validate::operational_events` and
//! `validate::infrastructure`. Each broken world of `validate_operations.json` gives the recorded
//! class and text at each entry point, with an event in flight too, and the accepted ones pass.
//! The cases were recorded before #430 and pin the core's output now.

use crate::support;

use support::validate;

#[test]
fn each_broken_world_gives_the_recorded_first_operational_or_infrastructure_error() {
    assert!(validate::check_file("validate_operations") >= 100);
}
