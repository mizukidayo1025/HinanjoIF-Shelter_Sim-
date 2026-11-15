//! The plan of the end-of-tick check (`rules/after_tick.rs`): what its rules read of
//! `model_configs` (`tick_plan`).
//!
//! The plan is a section of the host (`Parser::Host`): it is made with the contracts
//! (`declared_states`, `simulator_signatures`, the canonical forms of the configs) when the entry
//! is made (`initial::sections`), and kept here by the configs' stamp (lane 7-1, P8 d4). A plan
//! that raised is kept as its fault.

use std::sync::Arc;

use rustc_hash::FxHashSet;

/// What the rules of the end-of-tick check read of the configs.
#[derive(Clone, Debug, Default)]
pub struct TickPlan {
    /// `declared_states(configs)`: each namespace and its schema version, in the order given.
    pub declared: Vec<(Arc<str>, i64)>,
    /// The namespaces `simulator_signatures` requires.
    pub required: Vec<Arc<str>>,
    pub independent_queues: bool,
    pub information_enabled: bool,
    pub mean_reverting: bool,
    pub shared_configured: bool,
    /// None where `operational_events_enabled` raises, which the rule reads only without a state.
    pub operational_enabled: Option<bool>,
    /// The zones of the zone environment's baselines, or None without its config.
    pub environment_zones: Option<Vec<Arc<str>>>,
    pub infrastructure_configured: bool,
    /// The event types that need the operational events' state.
    pub operational_events: FxHashSet<Arc<str>>,
    /// The event types that need the infrastructure model.
    pub infrastructure_events: FxHashSet<Arc<str>>,
    /// What the entity rules read of the parsed `resource_service` config: None for a WorldState v5,
    /// whose configs have none.
    pub resource_config: Option<ResourceNumbers>,
}

/// The parts of `ResourceServiceConfig` the entity rules read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceNumbers {
    pub water_ml_per_service: i64,
    pub food_meal_units_per_service: i64,
    pub water_staff_skill: Arc<str>,
    pub meal_staff_skill: Arc<str>,
}
