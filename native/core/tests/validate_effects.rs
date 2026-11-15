//! The zone environment and EffectProgram parts of the whole world's validation
//! (`sheltersim_core::validate`, #250, #351): `validate::zone_environment` and
//! `validate::effect_program`, with and without the history, give the recorded class and text for
//! each broken world of `validate_effects.json`, the contracts' texts included, and pass the valid
//! ones. The cases were recorded before #430 and pin the core's output now.

use crate::support;

use support::validate;

#[test]
fn a_broken_effect_or_zone_state_gives_the_recorded_first_error_at_each_entry_point() {
    assert!(validate::check_file("validate_effects") >= 95);
}
