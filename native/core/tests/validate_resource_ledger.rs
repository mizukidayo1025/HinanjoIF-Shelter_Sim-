//! The resource ledger block of the whole world's validation (`sheltersim_core::validate`, #250,
//! #351): the identity history of `population_dynamics_v1` and `validate::resource_ledger` (the
//! whole audit trail replayed, and the tick's check) give the recorded class and text for each
//! broken ledger of `validate_resource_ledger.json`, and pass the valid ones. The cases were
//! recorded before #430 and pin the core's output now.

use crate::support;

use support::validate;

#[test]
fn a_broken_ledger_gives_the_recorded_first_error_at_each_entry_point() {
    assert!(validate::check_file("validate_resource_ledger") >= 90);
}
