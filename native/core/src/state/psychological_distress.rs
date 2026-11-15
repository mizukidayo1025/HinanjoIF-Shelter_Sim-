//! `psychological_distress_v1`: the template of an owned namespace
//! (docs/rust-port/lanes/p5-template-namespace.md). The rule that writes it is
//! `rules/psychology.rs`; `validate/psychology.rs` checks a saved namespace.

use crate::row::IdMap;
use crate::state_row;

pub const NAME: &str = "psychological_distress_v1";
const SCHEMA_VERSION: i64 = 1;
const MAX_AGENTS: usize = 100_000;

state_row! {
    #[derive(Copy)]
    pub struct AgentRecord {
        baseline_distress: f64,
        long_term_distress: f64,
        daily_acute_sum: f64,
        daily_high_distress_minutes: i64,
        daily_observed_minutes: i64,
        completed_days: i64,
    }
}

state_row! {
    pub struct DistressRow {
        schema_version: i64,
        last_updated_minute: i64,
        agents: IdMap<AgentRecord>,
    }
}

impl AgentRecord {
    /// validate_psychological_state's rules for one record of the row's exact types.
    pub fn is_valid(&self) -> bool {
        let observed = self.daily_observed_minutes;
        let high = self.daily_high_distress_minutes;
        let total = self.daily_acute_sum;
        (0.0..=1.0).contains(&self.baseline_distress)
            && (0.0..=1.0).contains(&self.long_term_distress)
            && total.is_finite()
            && (0..1440).contains(&observed)
            && (0..=observed).contains(&high)
            && self.completed_days >= 0
            && total >= 0.0
            && total <= observed as f64 + 1e-9
    }
}

impl DistressRow {
    /// Whether validate_psychological_state passes the namespace. False says nothing about it:
    /// the whole validation (`validate::world`) then says what is wrong.
    pub fn is_valid(&self) -> bool {
        self.schema_version == SCHEMA_VERSION
            && self.last_updated_minute >= -1
            && self.agents.len() <= MAX_AGENTS
            && self.agents.iter().all(|(id, record)| !id.is_empty() && record.is_valid())
    }
}
