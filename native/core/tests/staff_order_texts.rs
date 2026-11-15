//! An operator's staff order through its EffectProgram's handler, and a worker who became
//! unavailable mid-task (Issue #225, lane M11).
//!
//! The worlds are the gate's generated Scenario (20 evacuees, 120 minutes) at minute 0, edited for
//! each case (`staff_order_texts.json`). The outcomes are the recorded class and text. The arrival
//! and the completion of a task whose worker became unavailable stop the operation and then are
//! suppressed: the case's `after` world is the one the stop leaves. The worlds and the outcomes
//! were recorded before #430 and pin the core's output now. Each case runs the core's handler
//! (`dispatch::apply_alone`) on a fresh load of its world (`support/cases.rs`).

use crate::support;

use support::cases::check_group;

const FILE: &str = "staff_order_texts";

/// The handler's own checks of an order (a modifier, a phase, a staff ID or a due minute that is
/// not the program's) and its guard (false, or a field with no facility target), before the order:
/// the world stays as it was.
#[test]
fn an_order_the_handler_turns_down_gives_the_recorded_text() {
    check_group(FILE, "turned_down", 6);
}

/// An order that would end past the Run when its event comes.
#[test]
fn an_order_that_went_stale_fails_in_the_handler_with_the_recorded_text() {
    check_group(FILE, "stale", 1);
}

/// The arrival and the completion: the operation stops (the worker idle, the task gone, and for
/// the arrival its completion out of the queue), then the event is suppressed with the recorded
/// text.
#[test]
fn a_worker_who_became_unavailable_stops_the_task() {
    check_group(FILE, "unavailable", 2);
}
