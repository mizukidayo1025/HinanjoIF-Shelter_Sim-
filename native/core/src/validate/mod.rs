//! The whole world's validation (Issue #250, #347): the checks of the world's document, of its
//! simulator extensions and of a transaction's candidate, each giving the class and the text of the
//! first error it finds.
//!
//! The entry points are what a host answers its validation hooks with (`plain::PlainHost`):
//! `world` (with or without the event history, with an event in flight), `extensions` (the
//! extensions' checks, their errors as they are) and `candidate` (the world's check of a
//! transaction's candidate).
//!
//! This file holds the order of the checks, which is part of the contract: the first error found
//! is the one given. Each part is a function of its own file, and an error passes on as it is or
//! wrapped where this file wraps it. Each part reads the world through a `View` (`view.rs`) and
//! gives its error as the core's (`Error::Invariant` is `StateInvariantError`, `Error::Value` a
//! `ValueError`, ...).
//!
//! No history memo skips the entries of append-only histories that passed before: a later Issue
//! may give `View` one.

mod agent_dynamics;
pub(crate) mod body_state;
pub mod catalog;
pub(crate) mod declared;
mod effect_program;
mod entities;
mod events;
pub(crate) mod extension_head;
mod facility_queues;
mod geometry_overlay;
pub(crate) mod information;
mod infrastructure;
pub(crate) mod models;
mod movement;
mod operational_events;
pub(crate) mod population_dynamics;
pub(crate) mod population_lifecycle;
mod psychology;
pub mod py;
mod resource_ledger;
pub mod resource_service;
pub mod selection;
mod shared_operations;
pub mod signatures;
mod social_network;
pub(crate) mod staff_dynamics;
pub(crate) mod staff_operations;
mod temporary_spaces;
pub mod view;
pub(crate) mod zone_environment;

pub use view::{repr, repr_texts, StateRef, View};

use crate::error::{Error, Result};
use crate::events::EventRow;
use crate::host::{ErrorClass, HostFailure};
use crate::text::py_repr;
use crate::world::World;

/// What the world's validation is asked.
#[derive(Clone, Copy, Debug)]
pub struct Options<'a> {
    /// Whether the whole event history is read (true by default; `View::history`).
    pub history: bool,
    /// A pending event a handler is applying now (`View::in_flight`).
    pub in_flight: Option<&'a EventRow>,
}

impl Default for Options<'_> {
    fn default() -> Self {
        Self { history: true, in_flight: None }
    }
}

// ---- The errors' classes --------------------------------------------------------------------------

/// Whether `except ValueError` catches the error: a `ValueError` or one of its subclasses
/// (`StateInvariantError`, `ResourceLedgerError`, the outcomes, a refusal), as
/// `HostFailure::class` says.
pub fn is_value_error(error: &Error) -> bool {
    error.class() == ErrorClass::Value
}

/// Whether the error is a `StateInvariantError`.
pub fn is_invariant(error: &Error) -> bool {
    matches!(error, Error::Invariant(_) | Error::Corrupt { .. })
}

/// `str(exc)`: a `KeyError`'s text is the repr of its key.
pub fn py_str(error: &Error) -> String {
    match error {
        Error::Key(key) => py_repr(key),
        other => other.to_string(),
    }
}

/// `except ValueError as exc: raise StateInvariantError(f"{prefix}{exc}")`.
pub fn wrap_value(prefix: &'static str) -> impl FnOnce(Error) -> Error {
    move |error| if is_value_error(&error) { Error::Invariant(format!("{prefix}{}", py_str(&error))) } else { error }
}

// ---- The entry points -----------------------------------------------------------------------------

/// The whole world's validation, with the history and the event in flight `options` gives.
pub fn world(world: &World, options: &Options<'_>) -> Result<()> {
    validate(&View::new(world, options.history, options.in_flight))
}

/// The simulator extensions' check (`simulator_extensions`), with or without the whole history:
/// its errors as they are (`ValueError`, `KeyError`, `TypeError`).
pub fn extensions(world: &World, history: bool) -> Result<()> {
    simulator_extensions(&View::new(world, history, None))
}

/// A transaction's candidate: the whole world's validation, an error other than a
/// `StateInvariantError` wrapped as reading a world document wraps it.
pub fn candidate(world: &World) -> Result<()> {
    match self::world(world, &Options::default()) {
        Err(error) if !is_invariant(&error) && (is_value_error(&error) || matches!(error, Error::Key(_) | Error::Type(_))) => {
            Err(Error::Invariant(format!("invalid WorldState document: {}", py_str(&error))))
        }
        other => other,
    }
}

// ---- The validation -----------------------------------------------------------------------------

/// The keys of `model_state` whose presence has `validate` check the extensions.
const EXTENSION_KEYS: [&str; 9] = [
    "movement_v1",
    "agent_dynamics_v1",
    "social_network_v1",
    "staff_dynamics_v1",
    "population_dynamics_v1",
    "body_state_v1",
    "staff_operations_v1",
    "temporary_spaces_v1",
    "shared_operations_v1",
];

const SHARED_OPERATIONS: &str = "shared_operations_v1";
const LEDGER: &str = "resource_ledger_v1";

/// The world's validation, its parts in their order.
fn validate(view: &View<'_>) -> Result<()> {
    let version = view.schema_version()?;
    if version != 5 && version != 6 {
        return Err(Error::Invariant(format!("unsupported world-state schema version: {version}")));
    }
    let selection = selection::selected_models(view).and_then(|selection| declared::validate(view).map(|()| selection)).map_err(wrap_value(""))?;
    if has_extensions(view)? {
        simulator_extensions(view).map_err(|error| {
            if is_value_error(&error) || matches!(error, Error::Key(_) | Error::Type(_)) {
                Error::Invariant("invalid simulator extension state".to_owned())
            } else {
                error
            }
        })?;
    }
    let service = resource_service::validate(view)?;
    information::validate(view)?;
    psychology::validate(view, &selection)?;
    operational_events::validate(view).map_err(wrap_value("invalid operational events state: "))?;
    infrastructure::validate(view).map_err(wrap_value("invalid infrastructure state: "))?;
    zone_environment::validate(view).map_err(wrap_value("invalid zone environment state: "))?;
    effect_program::validate(view).map_err(wrap_value("invalid EffectProgram state: "))?;
    entities::validate(view, service.as_ref())?;
    if view.contains(LEDGER)? {
        resource_ledger::validate(view).map_err(wrap_value("invalid resource ledger: "))?;
    }
    events::validate(view)
}

/// Whether the world has extensions: a namespace of `EXTENSION_KEYS` in `model_state`, or the
/// configs' signed extensions (`signatures::simulator_signatures`); the signatures' error passes on
/// as it is.
fn has_extensions(view: &View<'_>) -> Result<bool> {
    for key in EXTENSION_KEYS {
        if view.contains(key)? {
            return Ok(true);
        }
    }
    Ok(!signatures::simulator_signatures(view)?.is_empty())
}

// ---- The simulator extensions ---------------------------------------------------------------------

/// The simulator extensions' check, its parts in their order.
fn simulator_extensions(view: &View<'_>) -> Result<()> {
    extension_head::validate(view)?;
    agent_dynamics::validate(view)?;
    body_state::validate(view)?;
    population_dynamics::validate(view)?;
    population_lifecycle::validate(view)?;
    social_network::validate(view)?;
    staff_dynamics::validate(view)?;
    staff_operations::validate(view)?;
    temporary_spaces::validate(view)?;
    movement::validate(view)?;
    geometry_overlay::validate(view)?;
    facility_queues::validate(view)?;
    if view.contains(SHARED_OPERATIONS)? && !view.has_config("shared_operations")? {
        return Err(Error::Value("shared_operations_v1 requires its signed Scenario contract".to_owned()));
    }
    shared_operations::validate(view)
}
