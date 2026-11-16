//! A decision's outcome, the step `record_outcome` of the propose phase (`tick::propose`): the
//! action's lifecycle, then the person's memory, in that order. No record of the decision is
//! written (#556): no Run keeps one since #478. A decision whose next decision is due no later than
//! its own minute is refused (`NOT_AFTER`) before either.
//!
//! The lifecycle and the memory are `action_lifecycle::commit_action` and
//! `agent_dynamics::remember_decision`, called directly. What they refuse has changed nothing and
//! is labelled with their rule (`Error::refused` with `LIFECYCLE` or `MEMORY`).

use crate::error::Result;
use crate::rules::action_lifecycle::{self, ActiveActions};
use crate::rules::agent_dynamics::{self, Decision as Memory};
use crate::rules::behavior::Proposal;
use crate::rules::world_view::ViewData;
use crate::world::World;

/// `ActionProposal.trigger` of a scheduled decision.
pub const TRIGGER: &str = "scheduled_decision";
/// The label of the refusal of the lifecycle's rule.
pub const LIFECYCLE: &str = "decisions.commit";
/// The label of the refusal of the memory's rule.
pub const MEMORY: &str = "decisions.remember";
/// The refusal of a decision whose next decision is due no later than its own minute.
pub const NOT_AFTER: &str = "a decision's next decision is not after its minute";

/// A decision and its outcome, as `record_outcome` takes them.
pub struct Recording<'a> {
    /// The view the proposal was made from.
    pub data: &'a ViewData,
    pub proposal: &'a Proposal,
    pub target_id: Option<&'a str>,
    /// After the proposal's minute.
    pub next_decision_at: i64,
    pub committed: bool,
    /// Whether `model_state` has `agent_dynamics_v1`.
    pub remembers: bool,
}

impl Recording<'_> {
    fn agent_id(&self) -> &str {
        &self.data.agents[self.proposal.index].id
    }

    fn minute(&self) -> i64 {
        self.data.minute_index
    }
}

/// The action's lifecycle of a committed decision (`record_action_proposal`).
pub fn commit(world: &World, active: &mut ActiveActions, recording: &Recording<'_>) -> Result<()> {
    if !recording.committed {
        return Ok(());
    }
    let proposal = action_lifecycle::Proposal {
        agent_id: recording.agent_id(),
        action: recording.proposal.outcome.action.value(),
        target_id: recording.target_id,
        minute: recording.minute(),
        trigger: TRIGGER,
    };
    action_lifecycle::commit_action(world, active, &proposal)
}

/// The person's memory of the decision (`record_proposal`), when the world keeps one.
pub fn remember(world: &World, recording: &Recording<'_>) -> Result<()> {
    if !recording.remembers {
        return Ok(());
    }
    let decision = Memory {
        agent_id: recording.agent_id(),
        action: recording.proposal.outcome.action.value(),
        target_id: recording.target_id,
        minute: recording.minute(),
        next_decision_at: recording.next_decision_at,
        trigger: Some(TRIGGER),
    };
    agent_dynamics::remember_decision(world, &decision)
}
