//! The same person leaving and coming back.
//!
//! The rules are functions on the `World`. `population_lifecycle_v1`, `population_presence_v1` and
//! `geometry_overlay_v1` are `ValueRow` namespaces, the others typed rows: a rule reads and writes
//! their rows alone. A person who leaves is archived as values: the agent's record, the records the
//! typed namespaces held of them (`Tree::to_v`, the fields in the order of the tree) and the ties
//! they had. A person who comes back is a new row of `agents` (`Table::insert`, at the end of the
//! table), and their records go back into the typed rows (`Tree::from_v`).
//!
//! Each rule plans, refusing with `Unsupported` before its first write what no valid input holds,
//! and then writes. The texts (`EventSuppressed`, `EventApplicationError`, the validation's
//! `ValueError`s) are saved data, in a fixed order of checks.
//!
//! What the rules take besides the rows comes as an input, read through the host before the world
//! is borrowed (`Read`, lane P9-3a): whether `model_state` holds each namespace (`kit::Held`), the
//! sections of the configs, the floor plan, the movement model of a relocation, and the records of
//! the EventLog a command's program scheduled (`Host::find_record`). A command's contract
//! (`PopulationPresenceCommand`) goes through `Host::contract` (policy E15) and its `selected_ids`
//! are read here from the contract's dump; the command runs in the core's rule transaction
//! (`txn::rule`), and the validation of the whole world goes through the host
//! (`Host::validate_world`, `Host::validate_extensions`).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use indexmap::IndexMap;

use crate::config::movement::{MovementModel, EXACT_INT};
use crate::config::SectionId;
use crate::entities::{Activity, AgentRow, Mobility, NeedRow, TaskRow};
use crate::error::{Error, Result};
use crate::events::{EventKind, EventRow, EventStatus};
use crate::geometry::{Geo, Pt};
use crate::host::{Contract, ErrorClass, Holding, Host, HostFailure};
use crate::row::{same_names, IdMap, StrEnum, ValueRow};
use crate::rules::kit::movement::KEY as MOVEMENT;
use crate::rules::kit::{self, Events, Held};
use crate::rules::population::{
    self, build_agent, cancels_of, check_admission_paths, check_information, check_queue_rows, check_relationship_keys, doc, equals_int, failed, field,
    identifier, invalid, is_held, list_of, map_of, optional, optional_identifier, pairs_naming, plain, presence_is_plain, presence_state, refuse,
    reset_position, sha_hex, sorted_set, strings, typed_of, v_str, value_of, write_presence, write_room_admission, zone_rows, Built, Doc, Person,
    PresenceState, Transition, NO_ROW, PRESENCE,
};
use crate::rules::{agent_dynamics, movement, overlay, travel};
use crate::state::agent_dynamics::{AgentDynamicsRow, AgentMemory, NAME as AGENT_DYNAMICS};
use crate::state::body_state::{BodyRecord, BodyRow, Relationship as BodyTie, NAME as BODY};
use crate::state::effect_program::{EffectProgramRow, NAME as EFFECT_PROGRAM};
use crate::state::information::{BeliefRow, ContactRow, InformationRow, MessageRow, NAME as INFORMATION};
use crate::state::movement::{Journey, MovementRow};
use crate::state::social_network::{Relationship as SocialTie, SocialNetworkRow, NAME as SOCIAL_NETWORK};
use crate::state::social_propagation::{ContactRecord, SocialRow, NAME as SOCIAL_RUNTIME};
use crate::text::py_repr;
use crate::txn::TxnEnd;
use crate::value::V;
use crate::world::{World, WorldCell};

pub const KEY: &str = "population_lifecycle_v1";
pub const POLICY_VERSION: &str = "same-id-population-lifecycle-v1";
pub const SOURCE_ID: &str = "ASM-POPULATION-LIFECYCLE-V1";
pub const MAX_HISTORY: usize = 20_000;
pub const MAX_RETAINED_PEOPLE: usize = 10_000;
pub const MAX_RELATIONSHIP_ARCHIVE: usize = 200_000;
const OVERLAY: &str = population::OVERLAY;

// ---- The records of the typed rows as their trees hold them ---------------------------------------

/// A value of a field of a record, as the tree holds it and as `Tree::from_v` takes it back: of the
/// exact type, nothing else.
trait Leaf: Sized {
    fn leaf(&self) -> V;
    fn from_leaf(value: &V) -> Option<Self>;
}

impl Leaf for f64 {
    fn leaf(&self) -> V {
        V::Float(*self)
    }

    fn from_leaf(value: &V) -> Option<Self> {
        match value {
            V::Float(number) => Some(*number),
            _ => None,
        }
    }
}

impl Leaf for i64 {
    fn leaf(&self) -> V {
        V::Int(*self)
    }

    fn from_leaf(value: &V) -> Option<Self> {
        match value {
            V::Int(number) => Some(*number),
            _ => None,
        }
    }
}

impl Leaf for bool {
    fn leaf(&self) -> V {
        V::Bool(*self)
    }

    fn from_leaf(value: &V) -> Option<Self> {
        match value {
            V::Bool(flag) => Some(*flag),
            _ => None,
        }
    }
}

impl Leaf for Arc<str> {
    fn leaf(&self) -> V {
        V::Str(self.clone())
    }

    fn from_leaf(value: &V) -> Option<Self> {
        match value {
            V::Str(text) => Some(text.clone()),
            _ => None,
        }
    }
}

impl<T: Leaf> Leaf for Option<T> {
    fn leaf(&self) -> V {
        self.as_ref().map_or(V::Null, Leaf::leaf)
    }

    fn from_leaf(value: &V) -> Option<Self> {
        match value {
            V::Null => Some(None),
            other => T::from_leaf(other).map(Some),
        }
    }
}

impl<T: Leaf> Leaf for Vec<T> {
    fn leaf(&self) -> V {
        V::List(self.iter().map(Leaf::leaf).collect())
    }

    fn from_leaf(value: &V) -> Option<Self> {
        match value {
            V::List(items) => items.iter().map(T::from_leaf).collect(),
            _ => None,
        }
    }
}

/// A record of a typed row as its tree holds it: a dict of its fields, in the row's order.
pub trait Tree: Sized {
    fn to_v(&self) -> V;
    /// The record of a tree's value, or Unsupported for a value the row would not take.
    fn from_v(value: &V) -> Result<Self>;
}

macro_rules! tree {
    ($name:ty { $($field:ident),* $(,)? }) => {
        const _: () = assert!(same_names(<$name>::NAMES, &[$(stringify!($field)),*]), "tree! lists the fields in their order");

        impl Tree for $name {
            fn to_v(&self) -> V {
                let mut map = IndexMap::with_capacity(<$name>::NAMES.len());
                $( map.insert(Arc::from(stringify!($field)), Leaf::leaf(&self.$field)); )*
                V::Map(map)
            }

            fn from_v(value: &V) -> Result<Self> {
                let V::Map(map) = value else {
                    return refuse("an archived record is not a dict");
                };
                if map.len() != <$name>::NAMES.len() {
                    return refuse("an archived record has other fields than the row's");
                }
                Ok(Self {
                    $(
                        $field: map
                            .get(stringify!($field))
                            .and_then(Leaf::from_leaf)
                            .ok_or_else(|| Error::unsupported("an archived record's field does not fit its row"))?,
                    )*
                })
            }
        }
    };
}

tree!(BeliefRow {
    message_id,
    belief,
    source,
    last_contact,
    last_event_id,
    exposed,
    belief_strength,
    share_propensity,
    correction_exposed,
    correction_accepted,
    ever_misbelieved,
    ever_forgotten,
    last_correction_accepted,
});
tree!(MessageRow { id, truth_status, source_id, zone_id, time, origin_event_id, origin_agent_id });
tree!(ContactRow { last_seen_minute, consecutive_minutes });
tree!(ContactRecord { last_seen_minute, consecutive_minutes });
tree!(BodyRecord { sleep_debt_minutes, uncertainty, reassurance, discontent, positive_affect, negative_affect, hygiene_need });
tree!(BodyTie { contact_count, trust, contact_minutes, last_contact_minute, last_interaction, topic_ids });
tree!(SocialTie { contact_count, conversation_count, cooperation_count, trust, shared_message_ids, last_contact_minute, last_interaction });
tree!(AgentMemory { known_facility_ids, last_action, last_target_id, last_decision_minute, decision_due_minute, decision_reason });

/// A dict of records, as its tree holds it.
fn records_v<T: Tree>(records: &IdMap<T>) -> V {
    V::Map(records.iter().map(|(key, record)| (key.clone(), record.to_v())).collect())
}

fn records_from<T: Tree>(value: &V) -> Result<IdMap<T>> {
    let mut records = IdMap::default();
    for (key, record) in map_of(value)? {
        records.insert(key.clone(), T::from_v(record)?);
    }
    Ok(records)
}

// ---- What is read for the rules before the world is borrowed ------------------------------------

/// What a relocation reads of the configs: whether passages have capacities
/// (`max_agents_in_transit_by_edge`), and the movement model that times a route (None where the
/// configs time no route). An error is the refusal reading it gave.
pub struct Relocation {
    pub transit_capacities: Result<bool>,
    pub timing: Result<Option<MovementModel>>,
}

/// What the rules read beside the world's rows.
pub struct Inputs<'a> {
    /// The population rules' inputs: the namespaces, the overlay's schema, the agent dynamics'
    /// policy and awareness, the floor plan.
    pub population: &'a population::Inputs,
    /// Whether `model_state` holds `population_lifecycle_v1`.
    pub lifecycle: Held,
    /// What a relocation reads, read for a relocate command alone.
    pub relocation: Option<&'a Relocation>,
}

impl Inputs<'_> {
    fn geo(&self) -> Result<&Geo> {
        self.population.geo.as_deref().ok_or_else(|| Error::unsupported(population::PLAN_UNREAD))
    }

    fn places(&self) -> overlay::Places<'_> {
        let inputs = self.population;
        overlay::Places { spaces: &inputs.namespaces.spaces, overlay: &inputs.namespaces.overlay, schema: &inputs.schema }
    }

    fn intake(&self) -> overlay::Intake<'_> {
        let inputs = self.population;
        overlay::Intake { overlay: &inputs.namespaces.overlay, movement: &inputs.namespaces.movement, schema: &inputs.schema, geo: inputs.geo.as_deref() }
    }
}

/// `model_state.get(KEY)` as the rules read it: the row, or None for no such key or a None.
fn lifecycle_of<'w>(world: &'w World, inputs: &Inputs<'_>) -> Result<Option<crate::cell::ShardRef<'w, ValueRow>>> {
    value_of(world, KEY, &inputs.lifecycle)
}

/// The lifecycle's row, to write: the plan found it.
fn lifecycle_mut(world: &World) -> Result<crate::cell::ShardMut<'_, ValueRow>> {
    world.row_mut::<ValueRow>(KEY)
}

fn part_mut<'a>(state: &'a mut V, name: &str) -> Result<&'a mut V> {
    population::field_mut(state, name)
}

fn dict_mut(value: &mut V) -> Result<&mut Doc> {
    population::map_mut(value)
}

fn list_mut(value: &mut V) -> Result<&mut Vec<V>> {
    population::list_mut(value)
}

/// `value == text`, of a value of a document.
fn is_text(value: Option<&V>, wanted: &str) -> bool {
    matches!(value, Some(V::Str(found)) if &**found == wanted)
}

/// `str` of a value of a document that must be one.
fn text(value: &V) -> Result<Arc<str>> {
    population::text(value)
}

/// None, or the exact str.
fn optional_text(value: &V) -> Result<Option<Arc<str>>> {
    Ok(kit::optional_text(value)?.cloned())
}

/// What a plan read raised, as the plan says it: a refusal or a text of the rules stays itself, any
/// other error is `the plan read: ...`.
pub fn plan_read(error: Error) -> Error {
    match error {
        Error::Unsupported(_) | Error::Refused { .. } | Error::Outcome(..) | Error::MarkedValue(_) => error,
        other => Error::unsupported(format!("the plan read: {other}")),
    }
}

/// A namespace a rule writes must be the world's own.
fn check_own(held: &Held) -> Result<()> {
    match held {
        Held::Other(reason) => Err(Error::unsupported(*reason)),
        _ => Ok(()),
    }
}

/// What the archive and the restore write: each namespace the world's own.
fn check_writable(inputs: &Inputs<'_>) -> Result<()> {
    let namespaces = &inputs.population.namespaces;
    for held in [
        &namespaces.information,
        &namespaces.body,
        &namespaces.agent_dynamics,
        &namespaces.social_network,
        &namespaces.social_runtime,
        &namespaces.movement,
        &namespaces.overlay,
        &namespaces.presence,
    ] {
        check_own(held)?;
    }
    Ok(())
}

// ---- A person's presence and its history --------------------------------------------------------

/// What `set_presence` reads of an event.
#[derive(Clone)]
pub struct EventIds {
    pub event_id: Arc<str>,
    pub causation_id: Option<Arc<str>>,
    pub source_id: Arc<str>,
}

impl EventIds {
    /// The event of a command, checked as `PresenceTransition` will.
    pub fn of(row: &EventRow) -> Result<Self> {
        identifier(&row.event_id)?;
        optional_identifier(row.causation_id.as_deref())?;
        let Some(source_id) = row.source_id.clone() else {
            return refuse("an event without a source");
        };
        identifier(&source_id)?;
        Ok(Self { event_id: row.event_id.clone(), causation_id: row.causation_id.clone(), source_id })
    }
}

#[derive(Default, Clone)]
struct PresenceChange {
    from_zone: Option<Arc<str>>,
    to_zone: Option<Arc<str>>,
    return_zone: Option<Arc<str>>,
    expected_return: Option<i64>,
    household_id: Option<Arc<str>>,
    source_ids: Vec<Arc<str>>,
    action: Option<&'static str>,
}

/// Whether the world holds the presence ledger as a dict (`namespace(...).filter(is_dict)`).
fn has_presence(world: &World, inputs: &Inputs<'_>) -> Result<bool> {
    population::is_dict_namespace(world, PRESENCE, &inputs.population.namespaces.presence)
}

/// Sets a person's presence. A plan has called `presence_state`, so the record there is a plain
/// one.
fn set_presence(world: &World, inputs: &Inputs<'_>, event: &EventIds, agent_id: &Arc<str>, status: &str, minute: i64, change: &PresenceChange) -> Result<()> {
    let old = {
        let raw = value_of(world, PRESENCE, &inputs.population.namespaces.presence)?.ok_or_else(|| Error::unsupported("the presence left"))?;
        PresenceState::of(&raw.0).person(agent_id)?
    };
    let current_household = change.household_id.clone().or_else(|| old.as_ref().and_then(|person| person.household_id.clone()));
    let retained = matches!(status, "temporarily_absent" | "transferred" | "awaiting_admission");
    let archive_path = retained.then(|| vec![Arc::from(KEY), Arc::from("people"), agent_id.clone()]);
    let admitted = status == "admitted";
    let return_zone = if admitted { None } else { change.return_zone.clone().or_else(|| old.as_ref().and_then(|person| person.return_zone_id.clone())) };
    let expected = if admitted { None } else { change.expected_return.or_else(|| old.as_ref().and_then(|person| person.expected_return_minute)) };
    let mut events = old.as_ref().map_or(Vec::new(), |person| person.source_event_ids.clone());
    events.push(event.event_id.clone());
    events.extend(change.source_ids.iter().cloned());
    let person = Person {
        agent_id: agent_id.clone(),
        household_id: current_household.clone(),
        status: Arc::from(status),
        since_minute: minute,
        transition_zone_id: change
            .to_zone
            .clone()
            .or_else(|| change.from_zone.clone())
            .or_else(|| old.as_ref().and_then(|person| person.transition_zone_id.clone())),
        return_zone_id: return_zone,
        expected_return_minute: expected,
        archive_path,
        source_event_ids: sorted_set(events),
    };
    let action = change.action.unwrap_or(match status {
        "temporarily_absent" => "temporary_exit",
        "transferred" => "transferred",
        "awaiting_admission" => "waiting",
        _ => "admitted",
    });
    // A NUL between the parts.
    let digest = sha_hex(&format!("{}\0{agent_id}\0{action}", event.event_id));
    let mut sources = change.source_ids.clone();
    sources.push(event.source_id.clone());
    let transition = Transition {
        transition_id: format!("presence-{}", &digest[..24]),
        event_id: event.event_id.clone(),
        agent_id: agent_id.clone(),
        minute,
        action,
        from_zone_id: change.from_zone.clone(),
        to_zone_id: change.to_zone.clone(),
        from_household_id: old.as_ref().and_then(|person| person.household_id.clone()),
        to_household_id: current_household,
        causation_id: event.causation_id.clone(),
        source_ids: sorted_set(sources),
        state_snapshot_hash: Some(person.hash()?),
    };
    write_presence(world, &person, &transition)
}

/// The append of `_history` and `admit_pending_returns`, kept to its last `MAX_HISTORY` rows.
fn append_history(world: &World, row: &HistoryRow<'_>) -> Result<()> {
    let mut state = lifecycle_mut(world)?;
    let history = list_mut(part_mut(&mut state.0, "history")?)?;
    history.push(row.value());
    if history.len() > MAX_HISTORY {
        let extra = history.len() - MAX_HISTORY;
        history.drain(..extra);
    }
    Ok(())
}

/// A person's current household: with the presence ledger as a dict, the household of the person's
/// record there (None for no record); otherwise the profile's group of an admitted person. Its own
/// refusals, which are not those of `kit::household_of`.
pub fn household_of(world: &World, presence: &Held, agent_id: &str) -> Result<Option<Arc<str>>> {
    if let Some(raw) = value_of(world, PRESENCE, presence)? {
        if let V::Map(state) = &raw.0 {
            let people = match state.get("people") {
                None => return Ok(None),
                Some(V::Map(people)) => people,
                Some(_) => return refuse("the presence's people are not a dict"),
            };
            match people.get(agent_id) {
                // `.get(agent_id, {})`, a dict without a household.
                None => return Ok(None),
                Some(V::Map(person)) => {
                    return match person.get("household_id") {
                        None | Some(V::Null) => Ok(None),
                        Some(V::Str(household)) => Ok(Some(household.clone())),
                        Some(_) => refuse("a household is not a str"),
                    };
                }
                // A record that is no dict: the profile's group.
                Some(_) => {}
            }
        }
    }
    Ok(world.rows()?.agents.get(agent_id).and_then(|agent| agent.profile.group_id.clone()))
}

/// A command as `PopulationPresenceCommand` validated it, with its `selected_ids`.
#[derive(Clone, Debug)]
pub struct Command {
    pub action: Arc<str>,
    pub zone_id: Option<Arc<str>>,
    pub household_id: Option<Arc<str>>,
    pub expected_return: Option<i64>,
    pub source_ids: Vec<Arc<str>>,
    pub ids: Vec<Arc<str>>,
}

impl Command {
    fn sources_with(&self, event: &EventIds) -> Vec<Arc<str>> {
        let mut sources = self.source_ids.clone();
        sources.push(event.source_id.clone());
        sorted_set(sources)
    }
}

/// What a row of the lifecycle's history says the person did: who, the action, the minute, and
/// the zones from and to.
#[derive(Clone, Copy)]
struct Moved<'a> {
    agent_id: &'a str,
    action: &'a str,
    minute: i64,
    from_zone: Option<&'a Arc<str>>,
    to_zone: Option<&'a Arc<str>>,
}

/// A row of the lifecycle's history: the event, what the person did, their household and the
/// sources.
struct HistoryRow<'a> {
    event_id: &'a Arc<str>,
    moved: Moved<'a>,
    household: Option<Arc<str>>,
    sources: &'a [Arc<str>],
}

impl HistoryRow<'_> {
    fn value(&self) -> V {
        let Moved { agent_id, action, minute, from_zone, to_zone } = self.moved;
        V::Map(doc([
            ("event_id", V::Str(self.event_id.clone())),
            ("agent_id", v_str(agent_id)),
            ("action", v_str(action)),
            ("minute", V::Int(minute)),
            ("from_zone_id", from_zone.map_or(V::Null, |zone| V::Str(zone.clone()))),
            ("to_zone_id", to_zone.map_or(V::Null, |zone| V::Str(zone.clone()))),
            ("household_id", optional(&self.household)),
            ("source_ids", strings(self.sources)),
        ]))
    }
}

/// `_history`: the row, written, and its copy for the receipt.
fn history(world: &World, inputs: &Inputs<'_>, event: &EventIds, moved: Moved<'_>, command: &Command) -> Result<V> {
    let household = match &command.household_id {
        Some(household) => Some(household.clone()),
        None => household_of(world, &inputs.population.namespaces.presence, moved.agent_id)?,
    };
    let sources = command.sources_with(event);
    let row = HistoryRow { event_id: &event.event_id, moved, household, sources: &sources };
    append_history(world, &row)?;
    Ok(row.value())
}

// ---- A person archived (`archive_person`) -------------------------------------------------------

/// A tie archived, unless the archive holds one of this namespace and key.
fn archive_relationship<T: Tree>(world: &World, namespace: &str, key: &Arc<str>, tie: &T) -> Result<()> {
    let pair = population::pair_of(key)?;
    let mut state = lifecycle_mut(world)?;
    let archive = list_mut(part_mut(&mut state.0, "relationship_archive")?)?;
    for item in archive.iter() {
        let item = map_of(item)?;
        if is_text(item.get("namespace"), namespace) && is_text(item.get("pair_key"), key) {
            return Ok(());
        }
    }
    archive.push(V::Map(doc([
        ("namespace", v_str(namespace)),
        ("pair_key", V::Str(key.clone())),
        ("agent_ids", V::List(pair.iter().map(|part| v_str(part)).collect())),
        ("value", tie.to_v()),
        ("restore_when_both_active", V::Bool(true)),
    ])));
    Ok(())
}

/// The archived record of an agent.
fn agent_record(agent: &AgentRow) -> V {
    V::Map(doc([
        ("profile", population::profile_record(agent)),
        ("zone_id", V::Str(agent.zone_id.clone())),
        ("activity", v_str(agent.activity.text())),
        ("target_id", optional(&agent.target_id)),
        ("needs", population::needs_value(&agent.needs)),
        ("stress", V::Float(agent.stress)),
        ("trust", V::Float(agent.trust)),
        ("next_decision_at", V::Int(agent.next_decision_at)),
        ("support_request_id", optional(&agent.support_request_id)),
        ("support_cooldown_until", V::Int(agent.support_cooldown_until)),
    ]))
}

fn all_finite(value: &V) -> bool {
    match value {
        V::Float(number) => number.is_finite(),
        V::List(items) => items.iter().all(all_finite),
        V::Map(map) => map.values().all(all_finite),
        _ => true,
    }
}

/// `population_lifecycle_v1` as the rules read it, borrowed from its row. Each read refuses what
/// it refused where the rules read the value.
#[derive(Clone, Copy)]
struct LifecycleState<'a>(&'a V);

impl<'a> LifecycleState<'a> {
    fn of(state: &'a V) -> Self {
        Self(state)
    }

    /// `agent_id in state["people"]`.
    fn retains(self, agent_id: &str) -> Result<bool> {
        Ok(map_of(field(self.0, "people")?)?.contains_key(agent_id))
    }

    /// The archive of a retained person, None for none (or one that is not a dict).
    fn archived(self, agent_id: &str) -> Result<Option<ArchivedPerson<'a>>> {
        Ok(map_of(field(self.0, "people")?)?.get(agent_id).filter(|snapshot| matches!(snapshot, V::Map(_))).map(ArchivedPerson))
    }

    /// Each claim kept fits the row it goes back to.
    fn check_messages(self) -> Result<()> {
        for message in map_of(field(self.0, "messages")?)?.values() {
            MessageRow::from_v(message)?;
        }
        Ok(())
    }

    /// Each deferred belief fits the row it goes back to.
    fn check_deferred_beliefs(self) -> Result<()> {
        for beliefs in map_of(field(self.0, "deferred_beliefs")?)?.values() {
            records_from::<BeliefRow>(beliefs)?;
        }
        Ok(())
    }

    /// The ties of `relationship_archive`.
    fn relationship_archive(self) -> Result<Vec<ArchivedTie<'a>>> {
        Ok(list_of(field(self.0, "relationship_archive")?)?.iter().map(ArchivedTie).collect())
    }

    /// Whether no one surely waits: a None, or a dict whose `pending_returns` is there and empty.
    fn nothing_waiting(self) -> bool {
        match self.0 {
            V::Null => true,
            V::Map(state) => state.get("pending_returns").is_some_and(|value| !value.is_truthy()),
            _ => false,
        }
    }
}

/// `population_lifecycle_v1` as a rule writes it, borrowed from its row.
struct LifecycleMut<'a>(&'a mut V);

impl<'a> LifecycleMut<'a> {
    fn of(state: &'a mut V) -> Self {
        Self(state)
    }

    /// `state["deferred_beliefs"].setdefault(agent_id, {})[message_id] = belief`.
    fn defer_belief(&mut self, agent_id: &Arc<str>, message_id: &Arc<str>, belief: V) -> Result<()> {
        let deferred = dict_mut(part_mut(self.0, "deferred_beliefs")?)?;
        let held = deferred.entry(agent_id.clone()).or_insert_with(|| V::Map(IndexMap::new()));
        dict_mut(held)?.insert(message_id.clone(), belief);
        Ok(())
    }
}

/// A retained person's archive (`state["people"][agent_id]`), borrowed.
#[derive(Clone, Copy)]
struct ArchivedPerson<'a>(&'a V);

impl<'a> ArchivedPerson<'a> {
    fn of(snapshot: &'a V) -> Self {
        Self(snapshot)
    }

    /// Whether the person was saved without the record of `agent_dynamics_v1`.
    fn lacks_dynamics(self) -> Result<bool> {
        Ok(matches!(field(self.0, "agent_dynamics")?, V::Null))
    }

    /// The agent the constructors make of the archived one (`restored_agent`), of the world's schema
    /// (read after the record).
    fn agent(self, zone_id: &Arc<str>, minute: i64, world: &World) -> Result<Built> {
        restored_agent(field(self.0, "agent")?, zone_id, minute, world.schema_version()?)
    }

    /// The person's record of `agent_dynamics_v1`, None where it was saved without one.
    fn dynamics(self) -> Result<Option<AgentMemory>> {
        let saved = field(self.0, "agent_dynamics")?;
        if matches!(saved, V::Null) {
            return Ok(None);
        }
        AgentMemory::from_v(saved).map(Some)
    }

    /// The person's record of `body_state_v1`, None where it was saved without one.
    fn body(self) -> Result<Option<BodyRecord>> {
        let saved = field(self.0, "body_agent")?;
        if matches!(saved, V::Null) {
            return Ok(None);
        }
        BodyRecord::from_v(saved).map(Some)
    }

    /// The archived beliefs fit their rows, and the query pressure is a float or nothing.
    fn check_information(self) -> Result<()> {
        let saved = field(self.0, "information")?;
        records_from::<BeliefRow>(field(saved, "beliefs")?)?;
        match field(saved, "query_pressure")? {
            pressure if !pressure.is_truthy() => {}
            V::Float(_) => {}
            _ => return refuse("an archived query pressure is not a float"),
        }
        Ok(())
    }
}

/// A tie of `relationship_archive`, borrowed.
#[derive(Clone, Copy)]
struct ArchivedTie<'a>(&'a V);

impl<'a> ArchivedTie<'a> {
    fn of(item: &'a V) -> Self {
        Self(item)
    }

    /// The tie fits the row it would go back to, and names two people a dict can look up.
    fn check(self, namespaces: &population::Namespaces) -> Result<()> {
        let item = self.0;
        for id in list_of(field(item, "agent_ids")?)? {
            if matches!(id, V::List(_) | V::Map(_)) {
                return refuse("an archived pair holds what a dict can't look up");
            }
        }
        let value = field(item, "value")?;
        match field(item, "namespace")? {
            V::Str(name) if &**name == SOCIAL_NETWORK && is_held(&namespaces.social_network) => drop(SocialTie::from_v(value)?),
            V::Str(name) if &**name == BODY && is_held(&namespaces.body) => drop(BodyTie::from_v(value)?),
            V::Str(name) if &**name == INFORMATION && is_held(&namespaces.information) => drop(ContactRow::from_v(value)?),
            V::Str(name) if &**name == SOCIAL_RUNTIME && is_held(&namespaces.social_runtime) => drop(ContactRecord::from_v(value)?),
            V::Str(_) => {}
            _ => return refuse("an archived namespace is not a str"),
        }
        text(field(item, "pair_key")?)?;
        Ok(())
    }

    /// Whether the tie waits: it is restored when both are back, and one of the two is not.
    fn waits(self, world: &World) -> Result<bool> {
        // Whether the world has this person: Unsupported for an ID a dict can't look up.
        let admitted = |id: &V| -> Result<bool> {
            match id {
                V::Str(id) => Ok(world.rows()?.agents.contains_key(id)),
                V::List(_) | V::Map(_) => refuse("an ID a dict can't look up"),
                _ => Ok(false),
            }
        };
        let ids = list_of(field(self.0, "agent_ids")?)?;
        let (first, second) = (ids.first().ok_or_else(|| Error::unsupported("no pair"))?, ids.get(1).ok_or_else(|| Error::unsupported("no pair"))?);
        Ok(field(self.0, "restore_when_both_active")?.is_truthy() && (!admitted(first)? || !admitted(second)?))
    }
}

/// The check of the snapshot (`snapshot_checked`) of the record `agent_record` will make, and the
/// other asks of `archive_person`.
fn check_archive(world: &World, inputs: &Inputs<'_>, state: LifecycleState<'_>, agent_id: &str) -> Result<()> {
    if state.retains(agent_id)? {
        return failed("person already has a retained lifecycle archive");
    }
    {
        let rows = world.rows()?;
        let Some(agent) = rows.agents.get(agent_id) else {
            return refuse("no such person");
        };
        let unit = |value: f64| value.is_finite() && (0.0..=1.0).contains(&value);
        let profile = &agent.profile;
        let needs_fit = [agent.needs.toilet_urgency, agent.needs.hunger, agent.needs.fatigue, agent.needs.hydration_need].iter().all(|value| value.is_finite());
        if !unit(agent.stress)
            || !unit(agent.trust)
            || !unit(profile.support_need)
            || !unit(profile.stress_sensitivity)
            || !needs_fit
            || !all_finite(&profile.provenance)
        {
            return refuse("a person's values do not fit a snapshot");
        }
    }
    check_writable(inputs)?;
    check_information(world, inputs.population)?;
    check_relationship_keys(world, inputs.population)?;
    check_queue_rows(world)
}

/// What a person who leaves keeps of the information state: their beliefs and query pressure
/// (None where the world holds no information state).
#[derive(Default)]
struct Information {
    beliefs: IdMap<BeliefRow>,
    query_pressure: Option<f64>,
}

/// The information part of `archive_person`, of a world that holds the namespace: the person's
/// beliefs and pressure leave, their contacts are archived, and the claims they started are kept
/// with everyone's beliefs in them, deferred.
fn archive_information(world: &World, agent_id: &Arc<str>) -> Result<Information> {
    let mut contacts = Vec::new();
    let mut moved = Vec::new();
    let information = {
        let mut info = world.row_mut::<InformationRow>(INFORMATION)?;
        let beliefs = info.beliefs.shift_remove(&**agent_id).unwrap_or_default();
        let query_pressure = info.query_pressure.shift_remove(&**agent_id).unwrap_or(0.0);
        for key in pairs_naming(info.contacts.iter(), agent_id)? {
            let record = info.contacts.shift_remove(&*key).ok_or_else(|| Error::Key(key.to_string()))?;
            contacts.push((key, record));
        }
        let mut message_ids: Vec<Arc<str>> = info.messages.keys().cloned().collect();
        message_ids.sort_unstable_by(|first, second| first.as_bytes().cmp(second.as_bytes()));
        for message_id in message_ids {
            if info.messages.get(&*message_id).is_none_or(|message| message.origin_agent_id != *agent_id) {
                continue;
            }
            let message = info.messages.shift_remove(&*message_id).ok_or_else(|| Error::Key(message_id.to_string()))?;
            let mut deferred = Vec::new();
            for (other, held) in info.beliefs.iter_mut() {
                if let Some(belief) = held.shift_remove(&*message_id) {
                    deferred.push((other.clone(), belief));
                }
            }
            moved.push((message_id, message, deferred));
        }
        info.support_requests.retain(|item| item.agent_id != *agent_id);
        Information { beliefs, query_pressure: Some(query_pressure) }
    };
    for (key, contact) in contacts {
        archive_relationship(world, INFORMATION, &key, &contact)?;
    }
    let mut state = lifecycle_mut(world)?;
    for (message_id, message, deferred) in moved {
        dict_mut(part_mut(&mut state.0, "messages")?)?.insert(message_id.clone(), message.to_v());
        for (other, belief) in deferred {
            LifecycleMut::of(&mut state.0).defer_belief(&other, &message_id, belief.to_v())?;
        }
        // The beliefs of those archived before.
        let mut found = Vec::new();
        for (archived, archive) in dict_mut(part_mut(&mut state.0, "people")?)?.iter_mut() {
            let beliefs = dict_mut(part_mut(part_mut(archive, "information")?, "beliefs")?)?;
            if let Some(belief) = beliefs.shift_remove(&*message_id) {
                if !matches!(belief, V::Null) {
                    found.push((archived.clone(), belief));
                }
            }
        }
        for (archived, belief) in found {
            LifecycleMut::of(&mut state.0).defer_belief(&archived, &message_id, belief)?;
        }
    }
    Ok(information)
}

/// The ties of a map of pairs that name the person, taken out in key order, then archived.
fn archive_ties<T: Tree>(world: &World, namespace: &str, ties: Vec<(Arc<str>, T)>) -> Result<()> {
    for (key, tie) in ties {
        archive_relationship(world, namespace, &key, &tie)?;
    }
    Ok(())
}

/// Archives a person. Gives the serials of the queue's slots its cancellations took out.
fn archive_person(world: &World, inputs: &Inputs<'_>, agent_id: &Arc<str>, row: &EventRow) -> Result<Vec<u64>> {
    let namespaces = &inputs.population.namespaces;
    let agent = world.rows()?.agents.get(agent_id).cloned().ok_or_else(|| Error::Key(agent_id.to_string()))?;
    release_room(world, inputs, agent_id)?;
    let mut information = Information::default();
    if is_held(&namespaces.information) {
        information = archive_information(world, agent_id)?;
    }
    let withdrawn: HashSet<Arc<str>> = {
        let state = lifecycle_mut(world)?;
        map_of(field(&state.0, "messages")?)?
            .iter()
            .filter(|(_, message)| matches!(message, V::Map(message) if is_text(message.get("origin_agent_id"), agent_id)))
            .map(|(message_id, _)| message_id.clone())
            .collect()
    };
    let mut body_agent = V::Null;
    if is_held(&namespaces.body) {
        let ties = {
            let mut body = world.row_mut::<BodyRow>(BODY)?;
            if let Some(record) = body.agents.shift_remove(&**agent_id) {
                body_agent = record.to_v();
            }
            let mut ties = Vec::new();
            for key in pairs_naming(body.relationships.map.iter(), agent_id)? {
                let tie = body.relationships.map.shift_remove(&*key).ok_or_else(|| Error::Key(key.to_string()))?;
                ties.push((key, tie));
            }
            ties
        };
        archive_ties(world, BODY, ties)?;
    }
    let mut dynamics_record = V::Null;
    if is_held(&namespaces.agent_dynamics) {
        if let Some(record) = world.row_mut::<AgentDynamicsRow>(AGENT_DYNAMICS)?.agents.shift_remove(&**agent_id) {
            dynamics_record = record.to_v();
        }
    }
    if is_held(&namespaces.social_network) {
        let ties = {
            let mut network = world.row_mut::<SocialNetworkRow>(SOCIAL_NETWORK)?;
            let mut ties = Vec::new();
            for key in pairs_naming(network.relationships.map.iter(), agent_id)? {
                let tie = network.relationships.map.shift_remove(&*key).ok_or_else(|| Error::Key(key.to_string()))?;
                ties.push((key, tie));
            }
            ties
        };
        archive_ties(world, SOCIAL_NETWORK, ties)?;
    }
    if is_held(&namespaces.social_runtime) {
        let contacts = {
            let mut runtime = world.row_mut::<SocialRow>(SOCIAL_RUNTIME)?;
            let mut contacts = Vec::new();
            for key in pairs_naming(runtime.contacts.iter(), agent_id)? {
                let contact = runtime.contacts.shift_remove(&*key).ok_or_else(|| Error::Key(key.to_string()))?;
                contacts.push((key, contact));
            }
            runtime.agent_cooldown_until.shift_remove(&**agent_id);
            contacts
        };
        archive_ties(world, SOCIAL_RUNTIME, contacts)?;
    }
    let mut events = vec![row.event_id.clone()];
    events.extend(row.causation_id.iter().filter(|id| !id.is_empty()).cloned());
    let snapshot = V::Map(doc([
        ("agent", agent_record(&agent)),
        ("agent_dynamics", dynamics_record),
        ("body_agent", body_agent),
        ("information", V::Map(doc([("beliefs", records_v(&information.beliefs)), ("query_pressure", information.query_pressure.map_or(V::Null, V::Float))]))),
        ("since_minute", V::Int(row.due_minute)),
        ("source_event_ids", strings(&sorted_set(events))),
    ]));
    {
        let mut state = lifecycle_mut(world)?;
        dict_mut(part_mut(&mut state.0, "people")?)?.insert(agent_id.clone(), snapshot);
    }
    let serials = cancel_agent_events(world, agent_id, &withdrawn)?;
    {
        let mut rows = world.rows_mut()?;
        let requests: Vec<Arc<str>> = rows.support_requests.iter().filter(|(_, request)| request.agent_id == *agent_id).map(|(id, _)| id.clone()).collect();
        for id in requests {
            rows.support_requests.remove_row(&id);
        }
    }
    if is_held(&namespaces.movement) {
        let mut movement = world.row_mut::<MovementRow>(MOVEMENT)?;
        movement.journeys.shift_remove(&**agent_id);
        movement.positions.shift_remove(&**agent_id);
    }
    world.rows_mut()?.agents.remove_row(agent_id);
    Ok(serials)
}

/// The person's events cancelled: the person's pending events, then the claims of theirs withdrawn,
/// settled as SUPPRESSED on the world's queue and log. Gives the serials of the slots taken out.
fn cancel_agent_events(world: &World, agent_id: &str, withdrawn: &HashSet<Arc<str>>) -> Result<Vec<u64>> {
    let pending: Vec<Arc<EventRow>> = {
        let queue = world.queue()?;
        queue.ordered().iter().filter_map(|(_, serial)| queue.slot(*serial).row.clone()).collect()
    };
    let mut found = Vec::new();
    cancels_of(&pending, &mut HashSet::new(), agent_id, withdrawn, &mut found);
    let mut serials = Vec::new();
    if !found.is_empty() {
        let events = Events::of(world)?;
        let minute = world.minute_index()?;
        for (event, detail) in &found {
            let settled = events.settle(event, minute, EventStatus::Suppressed, Some(detail)).map_err(plain)?;
            serials.extend(settled.taken);
        }
    }
    Ok(serials)
}

/// The person's bed and room admission released (`overlay::release`), of an overlay that is a dict:
/// each of the two parts that is a dict lets the person go.
fn release_room(world: &World, inputs: &Inputs<'_>, agent_id: &str) -> Result<()> {
    let held = &inputs.population.namespaces.overlay;
    let parts: Vec<&str> = {
        let Some(state) = value_of(world, OVERLAY, held)? else {
            return Ok(());
        };
        let V::Map(state) = &state.0 else {
            return Ok(());
        };
        ["bed_reservations", "room_admissions"]
            .into_iter()
            .filter(|name| matches!(state.get(*name), Some(V::Map(part)) if part.contains_key(agent_id)))
            .collect()
    };
    if parts.is_empty() {
        return Ok(());
    }
    let mut row = world.row_mut::<ValueRow>(OVERLAY)?;
    for name in parts {
        dict_mut(part_mut(&mut row.0, name)?)?.shift_remove(agent_id);
    }
    Ok(())
}

// ---- A person restored (`restore_person`) -------------------------------------------------------

/// The keys of a person's need levels.
const NEED_NAMES: [&str; 4] = ["toilet_urgency", "hunger", "fatigue", "hydration_need"];

/// The agent of an archived person in `zone`, with its needs, stress, trust and support cooldown
/// and its next decision at `minute`, as `restore_person` makes it (`schema_version` is the
/// world's: v5 has no hydration, which is then 0.0): the row, or the `ValueError` of the checks in
/// their order (the mobility, the initial needs, then the stress, trust, `next_decision_at` and
/// `support_cooldown_until`). Unsupported where they raise another error or make an object no row
/// holds.
pub fn restored_agent(raw: &V, zone_id: &Arc<str>, minute: i64, schema_version: i64) -> Result<Built> {
    // The profile's constructors; the minute is checked after the person's values.
    let mut agent = match build_agent(field(raw, "profile")?, zone_id, minute.max(0), schema_version)? {
        Built::Row(agent) => agent,
        raises @ Built::Raises(_) => return Ok(raises),
    };
    // The need levels, with v5's hydration: the levels are not checked, but each is a float.
    let mut needs: Doc = map_of(field(raw, "needs")?)?.clone();
    if schema_version == 5 {
        needs.insert(Arc::from("hydration_need"), V::Float(0.0));
    }
    if needs.keys().any(|key| !NEED_NAMES.contains(&&**key)) {
        return refuse("archived needs have another key");
    }
    let mut levels = [0.0; 4];
    for (place, name) in NEED_NAMES.iter().enumerate() {
        match needs.get(*name) {
            None => {}
            Some(V::Float(value)) => levels[place] = *value,
            Some(_) => return refuse("an archived need is not a float"),
        }
    }
    // `validate_unit_interval` of stress and trust.
    let unit = |name: &str, value: &V| -> Result<std::result::Result<f64, Error>> {
        match value {
            V::Float(number) if number.is_finite() && (0.0..=1.0).contains(number) => Ok(Ok(*number)),
            V::Int(0 | 1) => refuse("an archived value is an int"),
            V::Int(_) | V::Float(_) | V::Big(_) => Ok(Err(Error::Value(format!("{name} must be finite and between 0 and 1")))),
            _ => refuse("an archived value is not a number"),
        }
    };
    let stress = match unit("stress", field(raw, "stress")?)? {
        Ok(stress) => stress,
        Err(error) => return Ok(Built::Raises(error)),
    };
    let trust = match unit("trust", field(raw, "trust")?)? {
        Ok(trust) => trust,
        Err(error) => return Ok(Built::Raises(error)),
    };
    if minute < 0 {
        return Ok(Built::Raises(Error::Value("next_decision_at must be a non-negative integer".to_owned())));
    }
    let cooldown = match field(raw, "support_cooldown_until")? {
        V::Int(cooldown) if *cooldown >= 0 => *cooldown,
        V::Int(_) | V::Big(_) | V::Float(_) | V::Bool(_) => {
            return Ok(Built::Raises(Error::Value("support_cooldown_until must be a non-negative integer".to_owned())))
        }
        _ => return refuse("an archived cooldown is not a number"),
    };
    agent.needs = NeedRow { toilet_urgency: levels[0], hunger: levels[1], fatigue: levels[2], hydration_need: levels[3] };
    agent.stress = stress;
    agent.trust = trust;
    agent.next_decision_at = minute;
    agent.support_cooldown_until = cooldown;
    Ok(Built::Row(agent))
}

/// What `restore_person` asks that only the plan answers: the archive, the paths it writes through,
/// and that each archived value fits the row it goes back to.
fn check_restore(world: &World, inputs: &Inputs<'_>, state: LifecycleState<'_>, agent_id: &str, zone: &Arc<str>) -> Result<()> {
    let Some(snapshot) = state.archived(agent_id)? else {
        return refuse("the retained person is missing");
    };
    let namespaces = &inputs.population.namespaces;
    // The initial dynamics record of a person saved without their record.
    if is_held(&namespaces.agent_dynamics) && snapshot.lacks_dynamics()? {
        agent_dynamics::check_add_agent(world, &inputs.population.policy, &inputs.population.awareness)?;
    }
    check_admission_paths(world, inputs.population, &[zone])?;
    check_writable(inputs)?;
    // What the writes put back into the rows.
    snapshot.agent(zone, 0, world)?;
    if is_held(&namespaces.agent_dynamics) {
        world.row::<AgentDynamicsRow>(AGENT_DYNAMICS)?;
        snapshot.dynamics()?;
    }
    if is_held(&namespaces.body) {
        world.row::<BodyRow>(BODY)?;
        snapshot.body()?;
    }
    if is_held(&namespaces.information) {
        world.row::<InformationRow>(INFORMATION)?;
        snapshot.check_information()?;
        state.check_messages()?;
        state.check_deferred_beliefs()?;
    }
    if is_held(&namespaces.movement) {
        world.row::<MovementRow>(MOVEMENT)?;
    }
    check_archive_ties(inputs, state)
}

/// Each archived tie fits the row it would go back to, and names two people a dict can look up.
fn check_archive_ties(inputs: &Inputs<'_>, state: LifecycleState<'_>) -> Result<()> {
    for tie in state.relationship_archive()? {
        tie.check(&inputs.population.namespaces)?;
    }
    Ok(())
}

/// Restores a person. The plan checked what it reads (`check_restore`).
fn restore_person(
    world: &World,
    inputs: &Inputs<'_>,
    agent_id: &Arc<str>,
    zone_id: &Arc<str>,
    minute: i64,
    room: Option<&(Arc<str>, Pt)>,
    event_id: &str,
) -> Result<()> {
    let namespaces = &inputs.population.namespaces;
    let snapshot = {
        let state = lifecycle_mut(world)?;
        field(field(&state.0, "people")?, agent_id)?.clone()
    };
    let snapshot = ArchivedPerson::of(&snapshot);
    let agent = match snapshot.agent(zone_id, minute, world)? {
        Built::Row(agent) => agent,
        Built::Raises(error) => return Err(error),
    };
    // At the end of the table.
    world.rows_mut()?.agents.insert(agent_id.clone(), *agent);
    if is_held(&namespaces.agent_dynamics) {
        // The initial dynamics record of the person just added, for one saved without their record.
        let mut record = match snapshot.dynamics()? {
            Some(record) => record,
            None => {
                let policy = *kit::kept(&inputs.population.policy)?;
                let awareness = kit::kept(&inputs.population.awareness)?.as_ref();
                let state = agent_dynamics::create_state(world, policy, awareness)?;
                state.agents.get(&**agent_id).cloned().ok_or_else(|| Error::Key(agent_id.to_string()))?
            }
        };
        record.decision_due_minute = minute;
        world.row_mut::<AgentDynamicsRow>(AGENT_DYNAMICS)?.agents.insert(agent_id.clone(), record);
    }
    if is_held(&namespaces.body) {
        if let Some(record) = snapshot.body()? {
            world.row_mut::<BodyRow>(BODY)?.agents.insert(agent_id.clone(), record);
        }
    }
    if is_held(&namespaces.information) {
        restore_information(world, agent_id, snapshot)?;
    }
    if is_held(&namespaces.movement) {
        world.row_mut::<MovementRow>(MOVEMENT)?.journeys.shift_remove(&**agent_id);
        reset_position(world, inputs.geo()?, agent_id, zone_id)?;
    }
    if let Some((room_id, point)) = room {
        write_room_admission(world, &namespaces.overlay, agent_id, room_id, minute, event_id)?;
        if is_held(&namespaces.movement) {
            let mut row = world.row_mut::<MovementRow>(MOVEMENT)?;
            kit::place_in_bed(&mut row, agent_id, zone_id, *point);
        }
    }
    restore_ready_relationships(world, inputs)?;
    restore_deferred_beliefs(world, inputs)?;
    let mut state = lifecycle_mut(world)?;
    dict_mut(part_mut(&mut state.0, "people")?)?.shift_remove(&**agent_id);
    let returns = part_mut(&mut state.0, "pending_returns")?;
    let kept: Vec<V> = list_of(returns)?.iter().filter(|item| !matches!(item, V::Map(item) if is_text(item.get("agent_id"), agent_id))).cloned().collect();
    *returns = V::List(kept);
    Ok(())
}

/// The information part of `restore_person`: the beliefs in claims still told, the others deferred,
/// the pressure, and the claims the person started back.
fn restore_information(world: &World, agent_id: &Arc<str>, snapshot: ArchivedPerson<'_>) -> Result<()> {
    let saved = field(snapshot.0, "information")?;
    let beliefs = map_of(field(saved, "beliefs")?)?;
    let pressure = match field(saved, "query_pressure")? {
        V::Float(pressure) if *pressure != 0.0 => *pressure,
        pressure if !pressure.is_truthy() => 0.0,
        _ => return refuse("an archived query pressure is not a float"),
    };
    let (restored, deferred): (Vec<_>, Vec<_>) = {
        let info = world.row::<InformationRow>(INFORMATION)?;
        beliefs.iter().map(|(message_id, belief)| (message_id.clone(), belief.clone())).partition(|(message_id, _)| info.messages.contains_key(&**message_id))
    };
    let moving: Vec<(Arc<str>, V)> = {
        let mut state = lifecycle_mut(world)?;
        let all = dict_mut(part_mut(&mut state.0, "deferred_beliefs")?)?;
        let held = all.entry(agent_id.clone()).or_insert_with(|| V::Map(IndexMap::new()));
        let held = dict_mut(held)?;
        for (message_id, belief) in deferred {
            held.insert(message_id, belief);
        }
        if held.is_empty() {
            all.shift_remove(&**agent_id);
        }
        let messages = dict_mut(part_mut(&mut state.0, "messages")?)?;
        let ids: Vec<Arc<str>> = messages
            .iter()
            .filter(|(_, message)| matches!(message, V::Map(message) if is_text(message.get("origin_agent_id"), agent_id)))
            .map(|(message_id, _)| message_id.clone())
            .collect();
        ids.into_iter().filter_map(|message_id| messages.shift_remove(&*message_id).map(|message| (message_id, message))).collect()
    };
    let mut info = world.row_mut::<InformationRow>(INFORMATION)?;
    let mut records = IdMap::default();
    for (message_id, belief) in restored {
        records.insert(message_id, BeliefRow::from_v(&belief)?);
    }
    info.beliefs.insert(agent_id.clone(), records);
    info.query_pressure.insert(agent_id.clone(), pressure);
    for (message_id, message) in moving {
        info.messages.insert(message_id, MessageRow::from_v(&message)?);
    }
    Ok(())
}

/// The archived ties whose two people are back, each into its namespace unless the namespace has
/// one of its key.
fn restore_ready_relationships(world: &World, inputs: &Inputs<'_>) -> Result<()> {
    let namespaces = &inputs.population.namespaces;
    let archive = {
        let state = lifecycle_mut(world)?;
        list_of(field(&state.0, "relationship_archive")?)?.clone()
    };
    let total = archive.len();
    let mut remaining = Vec::with_capacity(total);
    for item in archive {
        if ArchivedTie::of(&item).waits(world)? {
            remaining.push(item);
            continue;
        }
        let name = text(field(&item, "namespace")?)?;
        let key = text(field(&item, "pair_key")?)?;
        let value = field(&item, "value")?;
        let held = match &*name {
            SOCIAL_NETWORK => &namespaces.social_network,
            BODY => &namespaces.body,
            INFORMATION => &namespaces.information,
            SOCIAL_RUNTIME => &namespaces.social_runtime,
            _ => {
                remaining.push(item);
                continue;
            }
        };
        if !is_held(held) {
            remaining.push(item);
            continue;
        }
        match &*name {
            SOCIAL_NETWORK => {
                let mut row = world.row_mut::<SocialNetworkRow>(SOCIAL_NETWORK)?;
                if !row.relationships.map.contains_key(&*key) {
                    row.relationships.map.insert(key.clone(), SocialTie::from_v(value)?);
                    row.relationships.note_write(&key);
                }
            }
            BODY => {
                let mut row = world.row_mut::<BodyRow>(BODY)?;
                if !row.relationships.map.contains_key(&*key) {
                    row.relationships.map.insert(key.clone(), BodyTie::from_v(value)?);
                    row.relationships.written.push(key.clone());
                }
            }
            INFORMATION => {
                let mut row = world.row_mut::<InformationRow>(INFORMATION)?;
                if !row.contacts.contains_key(&*key) {
                    row.contacts.insert(key.clone(), ContactRow::from_v(value)?);
                }
            }
            _ => {
                let mut row = world.row_mut::<SocialRow>(SOCIAL_RUNTIME)?;
                if !row.contacts.contains_key(&*key) {
                    row.contacts.insert(key.clone(), ContactRecord::from_v(value)?);
                }
            }
        }
    }
    if remaining.len() != total {
        let mut state = lifecycle_mut(world)?;
        *part_mut(&mut state.0, "relationship_archive")? = V::List(remaining);
    }
    Ok(())
}

/// The deferred beliefs of the people here in claims still told, restored.
fn restore_deferred_beliefs(world: &World, inputs: &Inputs<'_>) -> Result<()> {
    if !is_held(&inputs.population.namespaces.information) {
        return Ok(());
    }
    let mut agents: Vec<Arc<str>> = {
        let state = lifecycle_mut(world)?;
        map_of(field(&state.0, "deferred_beliefs")?)?.keys().cloned().collect()
    };
    agents.sort_unstable_by(|first, second| first.as_bytes().cmp(second.as_bytes()));
    for agent_id in agents {
        if !world.rows()?.agents.contains_key(&agent_id) {
            continue;
        }
        let mut state = lifecycle_mut(world)?;
        let deferred = dict_mut(part_mut(&mut state.0, "deferred_beliefs")?)?;
        let beliefs = dict_mut(deferred.get_mut(&*agent_id).ok_or_else(|| Error::Key(agent_id.to_string()))?)?;
        let mut message_ids: Vec<Arc<str>> = beliefs.keys().cloned().collect();
        message_ids.sort_unstable_by(|first, second| first.as_bytes().cmp(second.as_bytes()));
        let mut info = world.row_mut::<InformationRow>(INFORMATION)?;
        for message_id in message_ids {
            if !info.messages.contains_key(&*message_id) {
                continue;
            }
            let belief = beliefs.shift_remove(&*message_id).ok_or_else(|| Error::Key(message_id.to_string()))?;
            let record = BeliefRow::from_v(&belief)?;
            info.beliefs.get_mut(&*agent_id).ok_or_else(|| Error::Key(agent_id.to_string()))?.insert(message_id, record);
        }
        if beliefs.is_empty() {
            deferred.shift_remove(&*agent_id);
        }
    }
    Ok(())
}

// ---- Room in a zone (`zone_has_room`) -----------------------------------------------------------

/// What `effective_capacity` is made of, for one zone.
#[derive(Clone, Copy)]
struct Room {
    nominal: i64,
    active_people: i64,
    expired_people: i64,
}

impl Room {
    /// `nominal + active_people + retained`, for this many people.
    fn effective(self, current: i64) -> i64 {
        let retained = self.expired_people.min((current - self.nominal - self.active_people).max(0));
        self.nominal + self.active_people + retained
    }
}

/// The people of a world and the rooms given in this plan, as the lifecycle counts them.
pub struct Places {
    occupancy: HashMap<Arc<str>, i64>,
    incoming: HashMap<Arc<str>, i64>,
    capacity: HashMap<Arc<str>, Option<Room>>,
    planned_rooms: Vec<(Arc<str>, Option<Pt>)>,
}

/// The zone a moving person is bound for: their target's facility's zone, or the target itself.
fn bound_for<'a>(rows: &'a crate::entities::Entities, agent: &'a AgentRow) -> Option<&'a Arc<str>> {
    if agent.activity != Activity::Moving {
        return None;
    }
    let target = agent.target_id.as_ref()?;
    Some(rows.facilities.get(target).map_or(target, |facility| &facility.zone_id))
}

impl Places {
    pub fn of(world: &World) -> Result<Self> {
        let rows = world.rows()?;
        let mut occupancy: HashMap<Arc<str>, i64> = HashMap::new();
        let mut incoming: HashMap<Arc<str>, i64> = HashMap::new();
        for (_, agent) in rows.agents.iter() {
            *occupancy.entry(agent.zone_id.clone()).or_default() += 1;
            if let Some(zone) = bound_for(&rows, agent) {
                *incoming.entry(zone.clone()).or_default() += 1;
            }
        }
        Ok(Self { occupancy, incoming, capacity: HashMap::new(), planned_rooms: Vec::new() })
    }

    fn capacity(&mut self, world: &World, inputs: &Inputs<'_>, zone_id: &str) -> Result<Option<Room>> {
        if let Some(known) = self.capacity.get(zone_id) {
            return Ok(*known);
        }
        let found = match overlay::nominal_capacity(world, zone_id)? {
            None => None,
            Some(nominal) => {
                let read = overlay::capacity(world, &inputs.places(), zone_id, nominal, Some(nominal))?;
                Some(Room { nominal, active_people: read.active_people, expired_people: read.expired_people })
            }
        };
        self.capacity.insert(Arc::from(zone_id), found);
        Ok(found)
    }

    /// Whether the zone has room: the residents and the people bound for the zone, `excluding` one
    /// person, below its effective capacity (none: always).
    pub fn has_room_for(&mut self, world: &World, inputs: &Inputs<'_>, zone_id: &str, excluding: Option<&str>) -> Result<bool> {
        let mut total = self.occupancy.get(zone_id).copied().unwrap_or(0) + self.incoming.get(zone_id).copied().unwrap_or(0);
        if let Some(excluded) = excluding {
            let rows = world.rows()?;
            if let Some(agent) = rows.agents.get(excluded) {
                total -= i64::from(&*agent.zone_id == zone_id);
                total -= i64::from(bound_for(&rows, agent).is_some_and(|zone| &**zone == zone_id));
            }
        }
        let Some(capacity) = self.capacity(world, inputs, zone_id)? else {
            return Ok(true);
        };
        Ok(total < capacity.effective(total))
    }

    /// A person is planned into this zone.
    pub fn add(&mut self, zone_id: &Arc<str>) {
        *self.occupancy.entry(zone_id.clone()).or_default() += 1;
    }

    /// The temporary room open in the zone (`overlay::available_room_after`), and the point
    /// `overlay::admit` then gives.
    pub fn available_room(&self, world: &World, inputs: &Inputs<'_>, zone_id: &str) -> Result<Option<(Arc<str>, Pt)>> {
        overlay::available_room_after(world, &inputs.intake(), zone_id, &self.planned_rooms)
    }

    /// The room this plan fills, for the admissions that follow it.
    pub fn take_room(&mut self, inputs: &Inputs<'_>, room_id: &Arc<str>, point: Pt) {
        self.planned_rooms.push((room_id.clone(), is_held(&inputs.population.namespaces.movement).then_some(point)));
    }
}

// ---- What a command reads of the world it came from ------------------------------------------------

fn free(activity: Activity) -> bool {
    matches!(activity, Activity::Idle | Activity::Resting | Activity::Sleeping)
}

/// Why a command waits for a busy person: None for someone who is free.
fn busy_reason(world: &World, agent_id: &str) -> Result<Option<String>> {
    let rows = world.rows()?;
    let Some(agent) = rows.agents.get(agent_id) else {
        return refuse("no such person");
    };
    if !free(agent.activity) {
        return Ok(Some(format!("the person is {}", agent.activity.text())));
    }
    if agent.support_request_id.is_some() {
        return Ok(Some("the person is waiting for support".to_owned()));
    }
    if rows.active_tasks.iter().any(|(_, task)| task.agent_id.as_deref() == Some(agent_id)) {
        return Ok(Some("the person owns an active task".to_owned()));
    }
    Ok(None)
}

/// The records of the EventLog a command's program scheduled, by event ID, as `logged_through` read
/// them (`event_log.find(...).event`): the row of an `effect_program_population` event, or the
/// refusal reading it gave. Another event is not here.
pub type Logged = HashMap<Arc<str>, Result<EventRow>>;

/// The event IDs the program of an event's `effect_program_v1` provenance scheduled, to look up in
/// the EventLog (`logged_through`).
pub fn program_event_ids(world: &World, effects: &Held, row: &EventRow) -> Result<Vec<Arc<str>>> {
    let Some(V::Map(marker)) = kit::provenance(row, "effect_program_v1") else {
        return Ok(Vec::new());
    };
    let Some(V::Str(program_id)) = marker.get("program_id") else {
        return Ok(Vec::new());
    };
    let Some(programs) = typed_of::<EffectProgramRow>(world, EFFECT_PROGRAM, effects)? else {
        return Ok(Vec::new());
    };
    Ok(programs.programs.get(&**program_id).map_or(Vec::new(), |program| program.scheduled_event_ids.clone()))
}

/// The due minutes of the program's population commands of this action for this person, pending or
/// in the log.
fn program_minutes(world: &World, effects: &Held, logged: &Logged, row: &EventRow, agent_id: &str, action: &str) -> Result<Vec<i64>> {
    let mut minutes = Vec::new();
    let ids = program_event_ids(world, effects, row)?;
    if ids.is_empty() {
        return Ok(minutes);
    }
    let queue = world.queue()?;
    for event_id in ids {
        let scheduled: Arc<EventRow> = match queue.serial_of(&event_id) {
            Some(serial) => queue.slot(serial).row.clone().ok_or_else(|| Error::unsupported(NO_ROW))?,
            None => match logged.get(&event_id) {
                Some(Ok(found)) => Arc::new(found.clone()),
                Some(Err(error)) => return Err(error.clone()),
                None => continue,
            },
        };
        if &*scheduled.event_type != "effect_program_population" {
            continue;
        }
        if let Some(V::Map(command)) = kit::payload(&scheduled, "command") {
            if is_text(command.get("action"), action) && is_text(command.get("agent_id"), agent_id) {
                minutes.push(scheduled.due_minute);
            }
        }
    }
    Ok(minutes)
}

/// What the plan of a command reads of `world` rather than of the world it changes (the same in a
/// transaction): whether the person is busy, and the program's minutes of their returns and exits.
/// Each is read now and raised where the plan reads it.
pub struct WorldReads {
    busy: Result<Option<String>>,
    returns: Result<Vec<i64>>,
    exits: Result<Vec<i64>>,
}

/// The reads of `world` the plan of this command makes. `effects` is whether `world` holds
/// `effect_program_v1`, `logged` its log's records of the program.
pub fn world_reads(world: &World, effects: &Held, logged: &Logged, row: &EventRow, command: &Command) -> WorldReads {
    // Not counted as a refusal: no plan raises it.
    fn not_read<T>() -> Result<T> {
        Err(Error::Unsupported("the plan does not read it".into()))
    }
    let Some(first) = command.ids.first() else {
        return WorldReads { busy: not_read(), returns: not_read(), exits: not_read() };
    };
    let action = &*command.action;
    WorldReads {
        busy: if matches!(action, "temporary_exit" | "transfer") { busy_reason(world, first) } else { not_read() },
        returns: if action == "temporary_exit" { program_minutes(world, effects, logged, row, first, "return") } else { not_read() },
        exits: if action == "return" { program_minutes(world, effects, logged, row, first, "temporary_exit") } else { not_read() },
    }
}

// ---- The commands ---------------------------------------------------------------------------------

struct PlannedRoute {
    zone_ids: Vec<Arc<str>>,
    edge_ids: Vec<Arc<str>>,
    destination: Arc<str>,
    due: i64,
    event: EventRow,
    task_id: Arc<str>,
}

enum Plan {
    /// The rule's `EventSuppressed`, with nothing changed.
    Suppress(&'static str),
    /// Deferred while the person is busy: the retry on the world itself, and then the suppression.
    Defer(String),
    Exit {
        agent_id: Arc<str>,
        from_zone: Arc<str>,
    },
    Transfer {
        agent_id: Arc<str>,
        from_zone: Arc<str>,
    },
    ReturnWait {
        agent_id: Arc<str>,
        destination: Arc<str>,
        household: Option<Arc<str>>,
        from_zone: Option<Arc<str>>,
    },
    Return {
        agent_id: Arc<str>,
        destination: Arc<str>,
        household: Option<Arc<str>>,
        room: Option<(Arc<str>, Pt)>,
    },
    RelocateHere {
        agent_id: Arc<str>,
        from_zone: Arc<str>,
    },
    Relocate {
        agent_id: Arc<str>,
        from_zone: Arc<str>,
        destination: Arc<str>,
        route: Box<PlannedRoute>,
    },
    Reunite {
        zone: Arc<str>,
    },
}

fn zone_of(world: &World, agent_id: &str) -> Result<Option<Arc<str>>> {
    Ok(world.rows()?.agents.get(agent_id).map(|agent| agent.zone_id.clone()))
}

fn zone_known(world: &World, zone_id: &str) -> Result<bool> {
    Ok(zone_rows(world)?.zones.contains_key(zone_id))
}

/// The route of a relocation, its arrival event and its task.
fn plan_relocation(
    world: &World,
    inputs: &Inputs<'_>,
    row: &EventRow,
    agent_id: &Arc<str>,
    from_zone: &Arc<str>,
    destination: &Arc<str>,
) -> Result<Box<PlannedRoute>> {
    let relocation = inputs.relocation.ok_or_else(|| Error::unsupported("the relocation's configs were not read"))?;
    if relocation.transit_capacities.clone()? {
        return refuse("passage capacities read every trip under way");
    }
    const NO_ROUTE: &str = "no accessible, physically clear relocation route is available";
    let Some(route) = travel::world_route(world, from_zone, destination).map_err(plan_read)? else {
        return failed(NO_ROUTE);
    };
    let mobility = world.rows()?.agents.get(&**agent_id).map(kit::mobility_text).ok_or_else(|| Error::unsupported("no such person"))?;
    let geo = inputs.geo()?;
    if route.edge_ids.iter().any(|edge| !geo.route_is_clear(edge, mobility)) {
        return failed(NO_ROUTE);
    }
    // The route's minutes under the movement model, where the configs time routes.
    let minutes = match relocation.timing.as_ref().map_err(|error| error.clone())? {
        None => route.minutes,
        Some(model) => {
            if route.minutes.abs() >= EXACT_INT {
                return refuse("the route's minutes are too large");
            }
            let zones: Vec<&str> = route.zone_ids.iter().map(|zone| &**zone).collect();
            let edges: Vec<&str> = route.edge_ids.iter().map(|edge| &**edge).collect();
            let timed = movement::movement_minutes(world, geo, model, agent_id, &zones, &edges, route.minutes).map_err(plan_read)?;
            if edges.is_empty() {
                route.minutes
            } else {
                timed
            }
        }
    };
    let due = row.due_minute + minutes;
    let events = Events::of(world)?;
    if due >= events.duration() {
        return failed("relocation route would finish after the scenario boundary");
    }
    if is_held(&inputs.population.namespaces.movement) {
        if minutes <= 0 {
            return refuse("a journey has no duration");
        }
        check_admission_paths(world, inputs.population, &[])?;
    }
    let sequence = events.next_sequence()?;
    let task_id: Arc<str> = Arc::from(format!("presence-relocation-{sequence}"));
    let event = kit::new_event(kit::NewEvent {
        event_id: Arc::from(format!("presence-relocation-arrival-{sequence}")),
        kind: EventKind::Completion,
        due_minute: due,
        priority: row.priority,
        sequence,
        event_type: Arc::from("agent_travel_complete"),
        payload: doc([
            ("agent_id", V::Str(agent_id.clone())),
            ("task_id", V::Str(task_id.clone())),
            ("purpose", v_str("presence_relocation")),
            ("destination_zone_id", V::Str(destination.clone())),
            ("origin_zone_id", V::Str(from_zone.clone())),
            ("route_zone_ids", strings(&route.zone_ids)),
            ("route_edge_ids", strings(&route.edge_ids)),
            ("travel_minutes", V::Int(minutes)),
        ]),
        source_id: Some(Arc::from(SOURCE_ID)),
        causation_id: Some(row.event_id.clone()),
        provenance: kit::assumption(v_str(SOURCE_ID), POLICY_VERSION),
    })
    .map_err(|_| Error::unsupported("ScheduledEvent refuses the arrival"))?;
    Ok(Box::new(PlannedRoute { zone_ids: route.zone_ids, edge_ids: route.edge_ids, destination: route.destination, due, event, task_id }))
}

/// The plan of a command on the world it changes; `reads` are what it reads of the world it came
/// from.
fn plan_action(world: &World, inputs: &Inputs<'_>, row: &EventRow, command: &Command, state: LifecycleState<'_>, reads: &WorldReads) -> Result<Plan> {
    let minute = row.due_minute;
    let presence = presence_state(world, &inputs.population.namespaces.presence)?;
    let first = command.ids.first().cloned().ok_or_else(|| Error::unsupported("no one is selected"))?;
    let action = &*command.action;
    match action {
        "temporary_exit" | "transfer" => {
            let Some(from_zone) = zone_of(world, &first)? else {
                return failed(if action == "temporary_exit" { "temporary exit requires an admitted person" } else { "transfer requires an admitted person" });
            };
            if action == "temporary_exit" && reads.returns.clone()?.iter().any(|due| *due <= minute) {
                return Ok(Plan::Suppress("temporary exit dropped: the person was not free before the return in the same EffectProgram"));
            }
            if let Some(reason) = reads.busy.clone()? {
                return Ok(Plan::Defer(reason));
            }
            identifier(&first)?;
            identifier(&from_zone)?;
            check_archive(world, inputs, state, &first)?;
            if presence {
                let old = {
                    let raw = value_of(world, PRESENCE, &inputs.population.namespaces.presence)?.ok_or_else(|| Error::unsupported("no presence"))?;
                    PresenceState::of(&raw.0).person(&first)?
                };
                let expected = command.expected_return.or_else(|| old.and_then(|person| person.expected_return_minute));
                if action == "temporary_exit" && expected.is_some_and(|due| due < minute) {
                    return refuse("an expected return before the exit");
                }
                if let Some(zone) = &command.zone_id {
                    identifier(zone)?;
                }
            }
            Ok(if action == "temporary_exit" { Plan::Exit { agent_id: first, from_zone } } else { Plan::Transfer { agent_id: first, from_zone } })
        }
        "return" => {
            let is_admitted = world.rows()?.agents.contains_key(&first);
            if is_admitted && reads.exits.clone()?.iter().any(|due| *due < minute) {
                return Ok(Plan::Suppress("return skipped: the person had not left before the return in the same EffectProgram"));
            }
            if is_admitted || !state.retains(&first)? {
                return failed("return requires a retained absent person");
            }
            let old = if presence {
                let raw = value_of(world, PRESENCE, &inputs.population.namespaces.presence)?.ok_or_else(|| Error::unsupported("no presence"))?;
                PresenceState::of(&raw.0).person(&first)?
            } else {
                None
            };
            let destination = command.zone_id.clone().or_else(|| old.as_ref().and_then(|person| person.return_zone_id.clone()));
            let destination = match destination {
                Some(zone) if zone_known(world, &zone)? => zone,
                _ => return failed("return requires a valid destination zone"),
            };
            identifier(&destination)?;
            identifier(&first)?;
            let household = command.household_id.clone().or_else(|| old.as_ref().and_then(|person| person.household_id.clone()));
            let mut places = Places::of(world)?;
            if places.has_room_for(world, inputs, &destination, None)? {
                check_restore(world, inputs, state, &first, &destination)?;
                check_information(world, inputs.population)?;
                return Ok(Plan::Return { agent_id: first, destination, household, room: None });
            }
            match places.available_room(world, inputs, &destination)? {
                Some(room) => {
                    check_restore(world, inputs, state, &first, &destination)?;
                    check_information(world, inputs.population)?;
                    Ok(Plan::Return { agent_id: first, destination, household, room: Some(room) })
                }
                None => Ok(Plan::ReturnWait { agent_id: first, destination, household, from_zone: old.and_then(|person| person.transition_zone_id) }),
            }
        }
        "relocate" => {
            let not_here = || failed("relocation requires an admitted person and valid destination");
            let Some(from_zone) = zone_of(world, &first)? else {
                return not_here();
            };
            let destination = match command.zone_id.clone() {
                Some(zone) if zone_known(world, &zone)? => zone,
                _ => return not_here(),
            };
            let activity = world.rows()?.agents.get(&*first).map(|agent| agent.activity).ok_or_else(|| Error::unsupported("no row"))?;
            if !free(activity) {
                return failed("relocation requires a person who is not in an active task");
            }
            if !Places::of(world)?.has_room_for(world, inputs, &destination, Some(&first))? {
                return failed("relocation destination has no available capacity");
            }
            identifier(&first)?;
            identifier(&from_zone)?;
            identifier(&destination)?;
            check_own(&inputs.population.namespaces.overlay)?;
            if destination == from_zone {
                Ok(Plan::RelocateHere { agent_id: first, from_zone })
            } else {
                let route = plan_relocation(world, inputs, row, &first, &from_zone, &destination)?;
                Ok(Plan::Relocate { agent_id: first, from_zone, destination, route })
            }
        }
        "reunite" => {
            if command.household_id.is_none() {
                return failed("reunion requires a target household id");
            }
            // Each one's row, then each one's zone: the rule's order of checks.
            let zones: Vec<Option<Arc<str>>> = {
                let rows = world.rows()?;
                command.ids.iter().map(|id| rows.agents.get(id).map(|agent| agent.zone_id.clone())).collect()
            };
            if zones.iter().any(Option::is_none) {
                return failed("reunion requires every person to be admitted");
            }
            let mut distinct: Vec<Arc<str>> = Vec::with_capacity(zones.len());
            for zone in zones.into_iter().flatten() {
                if !distinct.contains(&zone) {
                    distinct.push(zone);
                }
            }
            if distinct.len() != 1 {
                return failed("reunion requires people to be physically co-located");
            }
            if !presence {
                return failed("reunion requires the population presence contract");
            }
            for id in &command.ids {
                identifier(id)?;
            }
            let zone = distinct[0].clone();
            identifier(&zone)?;
            Ok(Plan::Reunite { zone })
        }
        _ => refuse("another action"),
    }
}

/// What a command wrote: its receipt, and the serials of the queue's slots it took out.
pub struct Written {
    pub receipt: V,
    pub taken: Vec<u64>,
}

fn write_action(world: &World, inputs: &Inputs<'_>, row: &EventRow, event: &EventIds, command: &Command, plan: Plan) -> Result<Written> {
    let minute = row.due_minute;
    let change = |value: PresenceChange| PresenceChange { household_id: command.household_id.clone(), source_ids: command.source_ids.clone(), ..value };
    let presence = has_presence(world, inputs)?;
    let mut taken = Vec::new();
    let writing = Writing { world, inputs, event, command, minute, presence };
    let receipt = match plan {
        Plan::Suppress(_) | Plan::Defer(_) => return Err(Error::Runtime("a deferral has nothing to write".to_owned())),
        Plan::Exit { agent_id, from_zone } | Plan::Transfer { agent_id, from_zone } => {
            let exit = &*command.action == "temporary_exit";
            taken = archive_person(world, inputs, &agent_id, row)?;
            if presence {
                let status = if exit { "temporarily_absent" } else { "transferred" };
                let return_zone = if exit { Some(from_zone.clone()) } else { command.zone_id.clone() };
                let details =
                    change(PresenceChange { from_zone: Some(from_zone.clone()), return_zone, expected_return: command.expected_return, ..Default::default() });
                set_presence(world, inputs, event, &agent_id, status, minute, &details)?;
            }
            let (action, to_zone) = if exit { ("temporary_exit", None) } else { ("transferred", command.zone_id.as_ref()) };
            history(world, inputs, event, Moved { agent_id: &agent_id, action, minute, from_zone: Some(&from_zone), to_zone }, command)?
        }
        Plan::ReturnWait { agent_id, destination, household, from_zone } => write_return_wait(writing, agent_id, destination, household, from_zone)?,
        Plan::Return { agent_id, destination, household, room } => {
            restore_person(world, inputs, &agent_id, &destination, minute, room.as_ref(), &event.event_id)?;
            if presence {
                let details = PresenceChange {
                    to_zone: Some(destination.clone()),
                    household_id: household,
                    source_ids: command.source_ids.clone(),
                    ..Default::default()
                };
                set_presence(world, inputs, event, &agent_id, "admitted", minute, &details)?;
            }
            history(world, inputs, event, Moved { agent_id: &agent_id, action: "returned", minute, from_zone: None, to_zone: Some(&destination) }, command)?
        }
        Plan::RelocateHere { agent_id, from_zone } => {
            world.rows_mut()?.agents.get_mut(&agent_id).ok_or_else(|| Error::Key(agent_id.to_string()))?.next_decision_at = minute + 1;
            if presence {
                let details = change(PresenceChange {
                    from_zone: Some(from_zone.clone()),
                    to_zone: Some(from_zone.clone()),
                    action: Some("relocated"),
                    ..Default::default()
                });
                set_presence(world, inputs, event, &agent_id, "admitted", minute, &details)?;
            }
            history(
                world,
                inputs,
                event,
                Moved { agent_id: &agent_id, action: "relocated", minute, from_zone: Some(&from_zone), to_zone: Some(&from_zone) },
                command,
            )?
        }
        Plan::Relocate { agent_id, from_zone, destination, route } => write_relocation(writing, agent_id, from_zone, destination, route)?,
        Plan::Reunite { zone } => {
            if !presence {
                return refuse("the presence left");
            }
            for agent_id in &command.ids {
                let details =
                    change(PresenceChange { from_zone: Some(zone.clone()), to_zone: Some(zone.clone()), action: Some("reunited"), ..Default::default() });
                set_presence(world, inputs, event, agent_id, "admitted", minute, &details)?;
                history(world, inputs, event, Moved { agent_id, action: "reunited", minute, from_zone: Some(&zone), to_zone: Some(&zone) }, command)?;
            }
            V::Map(doc([
                ("event_id", V::Str(event.event_id.clone())),
                ("agent_ids", strings(&command.ids)),
                ("household_id", optional(&command.household_id)),
                ("minute", V::Int(minute)),
                ("action", v_str("reunited")),
            ]))
        }
    };
    Ok(Written { receipt, taken })
}

/// What the writes of a lifecycle command read: the world, the inputs, the event's IDs, the
/// command, its minute, and whether the world holds the presence.
#[derive(Clone, Copy)]
struct Writing<'a> {
    world: &'a World,
    inputs: &'a Inputs<'a>,
    event: &'a EventIds,
    command: &'a Command,
    minute: i64,
    presence: bool,
}

/// A return that waits for admission: its pending return (in place of the person's earlier one),
/// the presence awaiting admission, and the history's row.
fn write_return_wait(writing: Writing<'_>, agent_id: Arc<str>, destination: Arc<str>, household: Option<Arc<str>>, from_zone: Option<Arc<str>>) -> Result<V> {
    let Writing { world, inputs, event, command, minute, presence } = writing;
    let item = V::Map(doc([
        ("agent_id", V::Str(agent_id.clone())),
        ("zone_id", V::Str(destination.clone())),
        ("event_id", V::Str(event.event_id.clone())),
        ("minute", V::Int(minute)),
        ("household_id", optional(&household)),
        ("source_ids", strings(&command.sources_with(event))),
    ]));
    {
        let mut state = lifecycle_mut(world)?;
        let returns = part_mut(&mut state.0, "pending_returns")?;
        let mut kept: Vec<V> =
            list_of(returns)?.iter().filter(|other| !matches!(other, V::Map(other) if is_text(other.get("agent_id"), &agent_id))).cloned().collect();
        kept.push(item);
        *returns = V::List(kept);
    }
    if presence {
        let details = PresenceChange {
            from_zone,
            return_zone: Some(destination.clone()),
            household_id: household,
            source_ids: command.source_ids.clone(),
            ..Default::default()
        };
        set_presence(world, inputs, event, &agent_id, "awaiting_admission", minute, &details)?;
    }
    history(world, inputs, event, Moved { agent_id: &agent_id, action: "waiting", minute, from_zone: None, to_zone: Some(&destination) }, command)
}

/// A relocation to another zone: the person's room let go, the travel task and its arrival, the
/// walk where the world holds the movement, and the pending relocation; its receipt.
fn write_relocation(writing: Writing<'_>, agent_id: Arc<str>, from_zone: Arc<str>, destination: Arc<str>, route: Box<PlannedRoute>) -> Result<V> {
    let Writing { world, inputs, event, command, minute, .. } = writing;
    release_room(world, inputs, &agent_id)?;
    let events = Events::of(world)?;
    events.allocate_sequence()?;
    let event_id = route.event.event_id.clone();
    {
        let mut rows = world.rows_mut()?;
        rows.active_tasks.insert(
            route.task_id.clone(),
            TaskRow {
                id: route.task_id.clone(),
                kind: Arc::from("travel_return"),
                agent_id: Some(agent_id.clone()),
                resource_id: None,
                start_minute: minute,
                due_minute: route.due,
                staff_id: None,
            },
        );
        kit::set_agent(&mut rows, &agent_id, Activity::Moving, Some(&destination))?;
    }
    events.schedule(route.event.clone()).map_err(plain)?;
    if is_held(&inputs.population.namespaces.movement) {
        let journey = Journey {
            event_id: event_id.clone(),
            task_id: route.task_id.clone(),
            purpose: Arc::from("presence_relocation"),
            facility_id: None,
            destination_zone_id: route.destination.clone(),
            route_zone_ids: route.zone_ids.clone(),
            route_edge_ids: route.edge_ids.clone(),
            route_points: Vec::new(),
            started_minute: minute,
            due_minute: route.due,
            queue_id: None,
            support_request_id: None,
            support_staff_id: None,
        };
        let duration = route.due - minute;
        if duration <= 0 || duration >= EXACT_INT {
            return refuse("a journey has no plain duration");
        }
        let start = movement::Start { agent_id: &agent_id, journey, origin: &from_zone, points: &[], edge_ids: &route.edge_ids, started: minute, duration };
        movement::start(world, inputs.geo()?, start)?;
    }
    let sources = strings(&command.sources_with(event));
    let record = V::Map(doc([
        ("task_id", V::Str(route.task_id.clone())),
        ("event_id", V::Str(event_id.clone())),
        ("from_zone_id", V::Str(from_zone.clone())),
        ("to_zone_id", V::Str(destination.clone())),
        ("household_id", optional(&command.household_id)),
        ("source_ids", sources.clone()),
        ("started_minute", V::Int(minute)),
        ("due_minute", V::Int(route.due)),
    ]));
    {
        let mut state = lifecycle_mut(world)?;
        dict_mut(part_mut(&mut state.0, "pending_relocations")?)?.insert(agent_id.clone(), record);
    }
    Ok(V::Map(doc([
        ("event_id", V::Str(event.event_id.clone())),
        ("agent_id", V::Str(agent_id.clone())),
        ("action", v_str("relocation_scheduled")),
        ("minute", V::Int(minute)),
        ("from_zone_id", V::Str(from_zone)),
        ("to_zone_id", V::Str(destination)),
        ("arrival_minute", V::Int(route.due)),
        ("task_id", V::Str(route.task_id.clone())),
        ("source_ids", sources),
    ])))
}

/// What a command did: what it wrote, or why it waits for the person (`busy_reason`), whose retry
/// the caller schedules on the world as it was before the command.
pub enum Acted {
    Done(Written),
    Deferred(String),
}

/// The text of a deferred command, after its retry.
pub fn deferred_text(reason: &str) -> String {
    format!("population exit waits until the person is free: {reason}")
}

/// The part of a presence command (`presence_command`) between the command's parse and the
/// validation of the world it changed: the plan reads `world` and `reads` (what it reads of the
/// world the command came from, the same world in a transaction), and the writes go to `world`.
pub fn presence_action(world: &World, inputs: &Inputs<'_>, row: &EventRow, command: &Command, reads: &WorldReads) -> Result<Acted> {
    let (event, plan) = {
        let Some(state) = lifecycle_of(world, inputs)? else {
            return refuse("the lifecycle is not installed");
        };
        let event = EventIds::of(row)?;
        let plan = plan_action(world, inputs, row, command, LifecycleState::of(&state.0), reads)?;
        (event, plan)
    };
    match plan {
        Plan::Suppress(text) => Err(Error::suppressed(text)),
        Plan::Defer(reason) => Ok(Acted::Deferred(reason)),
        plan => kit::writing(write_action(world, inputs, row, &event, command, plan)).map(Acted::Done),
    }
}

/// The checks of a presence command on the world of its transaction, before the command: whether
/// the command is dropped (a retried exit whose expected return has passed).
pub fn check_command(world: &World, row: &EventRow, expected_return: Option<i64>) -> Result<bool> {
    if row.due_minute != world.minute_index()? {
        return failed("population command minute must match the current event boundary");
    }
    if let Some(expected) = expected_return {
        if expected < row.due_minute {
            if kit::provenance(row, "population_retry").is_none() {
                return failed("expected return minute precedes the departure event");
            }
            return Ok(true);
        }
    }
    Ok(false)
}

/// The text of a dropped command.
pub const DROPPED: &str = "temporary exit dropped: the person was not free before the expected return";

// ---- The returns admitted (`admit_returns`) -----------------------------------------------------

struct PendingReturn {
    item: V,
    minute: i64,
    event_id: Arc<str>,
    agent_id: Arc<str>,
    zone_id: Arc<str>,
}

/// A pending return the admission takes: the return, the room it takes, and the household and
/// sources its record names, read where the plan checks them.
struct Returning {
    entry: PendingReturn,
    room: Option<(Arc<str>, Pt)>,
    household: Option<Arc<str>>,
    sources: Vec<Arc<str>>,
}

/// Whether `admit_returns` surely has nothing to do: the world holds no lifecycle, or one whose
/// `pending_returns` is there and empty.
pub fn nothing_to_admit(world: &World, lifecycle: &Held) -> Result<bool> {
    Ok(nothing_waiting(value_of(world, KEY, lifecycle)?.as_deref().map(|row| LifecycleState::of(&row.0))))
}

/// Whether the lifecycle surely has no one waiting: absent, None, or a dict whose
/// `pending_returns` is there and empty.
fn nothing_waiting(state: Option<LifecycleState<'_>>) -> bool {
    state.is_none_or(LifecycleState::nothing_waiting)
}

/// `admit_pending_returns`: in (minute, event, person) order, each waiting person whose zone has
/// room, or a room of the overlay, comes back; the archived ties of people both back follow.
/// Gives the IDs admitted.
pub fn admit_returns(world: &World, inputs: &Inputs<'_>, minute: i64) -> Result<Vec<Arc<str>>> {
    let (admit, remaining) = {
        let held = lifecycle_of(world, inputs)?;
        if nothing_waiting(held.as_deref().map(|row| LifecycleState::of(&row.0))) {
            return Ok(Vec::new());
        }
        let Some(state) = held.filter(|row| matches!(row.0, V::Map(_))) else {
            return Ok(Vec::new());
        };
        let pending = field(&state.0, "pending_returns")?;
        if !pending.is_truthy() {
            return Ok(Vec::new());
        }
        presence_state(world, &inputs.population.namespaces.presence)?;
        let mut waiting = Vec::new();
        for item in list_of(pending)? {
            waiting.push(PendingReturn {
                minute: kit::exact_int(field(item, "minute")?)?,
                event_id: text(field(item, "event_id")?)?,
                agent_id: text(field(item, "agent_id")?)?,
                zone_id: text(field(item, "zone_id")?)?,
                item: item.clone(),
            });
        }
        waiting.sort_by(|first, second| {
            (first.minute, first.event_id.as_bytes(), first.agent_id.as_bytes()).cmp(&(second.minute, second.event_id.as_bytes(), second.agent_id.as_bytes()))
        });
        let mut places = Places::of(world)?;
        let mut admit = Vec::new();
        let mut remaining = Vec::new();
        for entry in waiting {
            let room = if places.has_room_for(world, inputs, &entry.zone_id, None)? {
                Some(None)
            } else {
                places.available_room(world, inputs, &entry.zone_id)?.map(Some)
            };
            let Some(room) = room else {
                remaining.push(entry.item);
                continue;
            };
            check_restore(world, inputs, LifecycleState::of(&state.0), &entry.agent_id, &entry.zone_id)?;
            identifier(&entry.event_id)?;
            identifier(&entry.agent_id)?;
            identifier(&entry.zone_id)?;
            let household = optional_text(field(&entry.item, "household_id")?)?;
            optional_identifier(household.as_deref())?;
            let mut sources = Vec::new();
            for source in list_of(field(&entry.item, "source_ids")?)? {
                let source = kit::exact_str(source)?;
                identifier(source)?;
                sources.push(source.clone());
            }
            places.add(&entry.zone_id);
            if let Some((room_id, point)) = &room {
                places.take_room(inputs, room_id, *point);
            }
            admit.push(Returning { entry, room, household, sources });
        }
        if !admit.is_empty() {
            check_information(world, inputs.population)?;
            check_relationship_keys(world, inputs.population)?;
        }
        drop(state);
        validate(world, &inputs.lifecycle, &inputs.population.namespaces.presence, None)?;
        (admit, remaining)
    };
    kit::writing(write_returns(world, inputs, minute, admit, remaining))
}

fn write_returns(world: &World, inputs: &Inputs<'_>, minute: i64, admit: Vec<Returning>, remaining: Vec<V>) -> Result<Vec<Arc<str>>> {
    let presence = has_presence(world, inputs)?;
    let mut admitted: Vec<Arc<str>> = Vec::with_capacity(admit.len());
    for Returning { entry, room, household, sources } in &admit {
        restore_person(world, inputs, &entry.agent_id, &entry.zone_id, minute, room.as_ref(), &entry.event_id)?;
        let (household, sources) = (household.clone(), sources.clone());
        if presence {
            let synthetic = EventIds { event_id: entry.event_id.clone(), causation_id: None, source_id: Arc::from(SOURCE_ID) };
            let details = PresenceChange {
                to_zone: Some(entry.zone_id.clone()),
                household_id: household.clone(),
                source_ids: sources.clone(),
                action: Some("returned"),
                ..Default::default()
            };
            set_presence(world, inputs, &synthetic, &entry.agent_id, "admitted", minute, &details)?;
        }
        append_history(
            world,
            &HistoryRow {
                event_id: &entry.event_id,
                moved: Moved { agent_id: &entry.agent_id, action: "admitted", minute, from_zone: None, to_zone: Some(&entry.zone_id) },
                household,
                sources: &sources,
            },
        )?;
        admitted.push(entry.agent_id.clone());
    }
    // No one admitted leaves the same people in the same order.
    if !admit.is_empty() {
        let mut state = lifecycle_mut(world)?;
        *part_mut(&mut state.0, "pending_returns")? = V::List(remaining);
    }
    restore_ready_relationships(world, inputs)?;
    Ok(admitted)
}

/// What `admit_returns` asks of the waiting, whoever gets a place: a rule that writes and then
/// admits asks this first, as it can't refuse then.
fn check_waiting_returns(world: &World, inputs: &Inputs<'_>) -> Result<()> {
    let Some(state) = lifecycle_of(world, inputs)?.filter(|row| matches!(row.0, V::Map(_))) else {
        return Ok(());
    };
    let pending = field(&state.0, "pending_returns")?;
    if !pending.is_truthy() {
        return Ok(());
    }
    presence_state(world, &inputs.population.namespaces.presence)?;
    let mut places = Places::of(world)?;
    for item in list_of(pending)? {
        kit::exact_int(field(item, "minute")?)?;
        let (event_id, agent_id, zone_id) = (text(field(item, "event_id")?)?, text(field(item, "agent_id")?)?, text(field(item, "zone_id")?)?);
        places.has_room_for(world, inputs, &zone_id, None)?;
        places.available_room(world, inputs, &zone_id)?;
        check_restore(world, inputs, LifecycleState::of(&state.0), &agent_id, &zone_id)?;
        identifier(&event_id)?;
        identifier(&agent_id)?;
        identifier(&zone_id)?;
        optional_identifier(optional_text(field(item, "household_id")?)?.as_deref())?;
        for source in list_of(field(item, "source_ids")?)? {
            identifier(kit::exact_str(source)?)?;
        }
    }
    check_information(world, inputs.population)?;
    check_relationship_keys(world, inputs.population)
}

// ---- A relocation's completion (`complete_relocation`) ------------------------------------------

const MISMATCH: &str = "relocation completion does not match its pending route";

struct Completion {
    agent_id: Arc<str>,
    event: EventIds,
    from_zone: Arc<str>,
    to_zone: Arc<str>,
    household: Option<Arc<str>>,
    sources: Vec<Arc<str>>,
}

/// The plan of `complete_relocation`, or the rule's text, before anything is written. `arriving` is
/// the zone a caller's writes will put the person in.
fn plan_completion(world: &World, inputs: &Inputs<'_>, row: &EventRow, arriving: Option<&str>) -> Result<std::result::Result<Completion, &'static str>> {
    let Some(V::Str(agent_id)) = kit::payload(row, "agent_id").cloned() else {
        return refuse("the event names no one");
    };
    let Some(state) = lifecycle_of(world, inputs)?.filter(|row| matches!(row.0, V::Map(_))) else {
        return Ok(Err(MISMATCH));
    };
    let relocation = match map_of(&state.0)?.get("pending_relocations") {
        None => None,
        Some(relocations) => map_of(relocations)?.get(&*agent_id).cloned(),
    };
    let Some(relocation @ V::Map(_)) = relocation else {
        return Ok(Err(MISMATCH));
    };
    if !is_text(Some(field(&relocation, "event_id")?), &row.event_id) || !equals_int(field(&relocation, "due_minute")?, row.due_minute) {
        return Ok(Err(MISMATCH));
    }
    let to_zone = text(field(&relocation, "to_zone_id")?)?;
    let in_place = arriving == Some(&*to_zone) || world.rows()?.agents.get(&*agent_id).is_some_and(|agent| agent.zone_id == to_zone);
    if !in_place {
        return Ok(Err(MISMATCH));
    }
    presence_state(world, &inputs.population.namespaces.presence)?;
    let event = EventIds::of(row)?;
    identifier(&agent_id)?;
    identifier(&to_zone)?;
    let from_zone = text(field(&relocation, "from_zone_id")?)?;
    identifier(&from_zone)?;
    let household = optional_text(field(&relocation, "household_id")?)?;
    optional_identifier(household.as_deref())?;
    let mut sources = Vec::new();
    for source in list_of(field(&relocation, "source_ids")?)? {
        let source = text(source)?;
        identifier(&source)?;
        sources.push(source);
    }
    drop(state);
    validate(world, &inputs.lifecycle, &inputs.population.namespaces.presence, Some(&agent_id))?;
    check_waiting_returns(world, inputs)?;
    Ok(Ok(Completion { agent_id, event, from_zone, to_zone, household, sources }))
}

/// What `complete_relocation` refuses, for a rule that writes first and then calls it: `arriving`
/// is the zone the caller's writes will put the person in.
pub fn check_relocation_completes(world: &World, inputs: &Inputs<'_>, row: &EventRow, arriving: &str) -> Result<()> {
    match plan_completion(world, inputs, row, Some(arriving))? {
        Ok(_) => Ok(()),
        // Raised after the caller's writes: that caller's rule says it.
        Err(_) => refuse("the completion does not match its route"),
    }
}

/// A relocation's completion: the presence change and the history row of a relocation that arrived,
/// the relocation dropped, then `admit_pending_returns` at the due minute. Gives the IDs that
/// admitted.
pub fn complete_relocation(world: &World, inputs: &Inputs<'_>, row: &EventRow) -> Result<Vec<Arc<str>>> {
    let completion = match plan_completion(world, inputs, row, None)? {
        Ok(completion) => completion,
        Err(text) => return failed(text),
    };
    kit::writing((|| {
        let Completion { agent_id, event, from_zone, to_zone, household, sources } = completion;
        if has_presence(world, inputs)? {
            let details = PresenceChange {
                from_zone: Some(from_zone.clone()),
                to_zone: Some(to_zone.clone()),
                household_id: household.clone(),
                source_ids: sources.clone(),
                action: Some("relocated"),
                ..Default::default()
            };
            set_presence(world, inputs, &event, &agent_id, "admitted", row.due_minute, &details)?;
        }
        append_history(
            world,
            &HistoryRow {
                event_id: &row.event_id,
                moved: Moved { agent_id: &agent_id, action: "relocated", minute: row.due_minute, from_zone: Some(&from_zone), to_zone: Some(&to_zone) },
                household,
                sources: &sources,
            },
        )?;
        let mut state = lifecycle_mut(world)?;
        dict_mut(part_mut(&mut state.0, "pending_relocations")?)?.shift_remove(&*agent_id);
        Ok(())
    })())?;
    // Written, so nothing can fall back from here: `check_waiting_returns` read what this reads.
    kit::writing(admit_returns(world, inputs, row.due_minute))
}

// ---- The check of `population_lifecycle_v1` (`validate::population_lifecycle`) ------------------

/// The first value `json.dumps(value, allow_nan=False, sort_keys=True)` refuses, in its order: the
/// keys of each dict sorted.
fn first_not_finite(value: &V) -> Option<f64> {
    match value {
        V::Float(number) if !number.is_finite() => Some(*number),
        V::List(items) => items.iter().find_map(first_not_finite),
        V::Map(map) => {
            let mut keys: Vec<&Arc<str>> = map.keys().collect();
            keys.sort_unstable_by(|first, second| first.as_bytes().cmp(second.as_bytes()));
            keys.into_iter().find_map(|key| first_not_finite(&map[&**key]))
        }
        _ => None,
    }
}

/// `json.dumps`' ValueError for a value that is not finite, with its text. A value read into `V`
/// holds nothing else `json.dumps` refuses.
fn json_checked(value: &V) -> Result<()> {
    match first_not_finite(value) {
        None => Ok(()),
        // `repr` of the float: only these three are not finite.
        Some(number) => invalid(&format!(
            "Out of range float values are not JSON compliant: {}",
            if number.is_nan() {
                "nan"
            } else if number > 0.0 {
                "inf"
            } else {
                "-inf"
            }
        )),
    }
}

/// `Activity(value)` or `Mobility(value)`: the enum's ValueError, as a validation's. A value that is
/// not a str, whose repr Python would write, is refused.
fn enum_checked<E: StrEnum>(name: &str, value: &V) -> Result<()> {
    match value {
        V::Str(text) if E::from_text(text).is_some() => Ok(()),
        V::Str(text) => invalid(&format!("{} is not a valid {name}", py_repr(text))),
        _ => refuse("an enum's value is not a str"),
    }
}

/// The check of an archived agent's snapshot.
fn snapshot_checked(raw: &V, expected_id: &str) -> Result<()> {
    const FIELDS: [&str; 10] =
        ["profile", "zone_id", "activity", "target_id", "needs", "stress", "trust", "next_decision_at", "support_request_id", "support_cooldown_until"];
    let fields = match raw {
        V::Map(fields) if fields.len() == FIELDS.len() && FIELDS.iter().all(|key| fields.contains_key(*key)) => fields,
        _ => return invalid("population lifecycle Agent snapshot has an invalid schema"),
    };
    let profile = match fields.get("profile") {
        Some(V::Map(profile)) if is_text(profile.get("id"), expected_id) => profile,
        _ => return invalid("population lifecycle Agent snapshot identity is invalid"),
    };
    const PROFILE: [&str; 9] =
        ["id", "group_id", "initial_zone_id", "mobility", "support_need", "stress_sensitivity", "rng_key", "initial_needs", "provenance"];
    if profile.len() != PROFILE.len() || PROFILE.iter().any(|key| !profile.contains_key(*key)) || !matches!(profile.get("initial_needs"), Some(V::Map(_))) {
        return invalid("population lifecycle profile snapshot is invalid");
    }
    enum_checked::<Activity>("Activity", fields.get("activity").ok_or_else(|| Error::unsupported("no activity"))?)?;
    enum_checked::<Mobility>("Mobility", profile.get("mobility").ok_or_else(|| Error::unsupported("no mobility"))?)?;
    for value in [fields.get("stress"), fields.get("trust"), profile.get("support_need"), profile.get("stress_sensitivity")] {
        let good = match value {
            Some(V::Float(number)) => number.is_finite() && (0.0..=1.0).contains(number),
            Some(V::Int(number)) => (0..=1).contains(number),
            _ => false,
        };
        if !good {
            return invalid("population lifecycle normalized Agent value is invalid");
        }
    }
    json_checked(raw)
}

/// `value in mapping` of a value Python hashes as it is: false for a scalar that is not a str.
fn scalar_in(value: &V, contains: impl FnOnce(&str) -> Result<bool>) -> Result<bool> {
    match value {
        V::Str(text) => contains(text),
        V::List(_) | V::Map(_) => refuse("a key is not hashable"),
        _ => Ok(false),
    }
}

/// A task as the validator reads it: its person, kind and due minute.
pub struct TaskFacts {
    pub agent_id: Option<Arc<str>>,
    pub kind: Arc<str>,
    pub due_minute: i64,
}

/// What `validate_population_lifecycle_state` reads of a world: the rows of the world (`validate`
/// here, and `validate::population_lifecycle` of the whole validation).
pub struct Facts<'a> {
    /// `model_state.get(KEY)`.
    pub state: Option<&'a V>,
    /// `model_state[PRESENCE]` where `model_state` has the key.
    pub presence: Option<Option<&'a V>>,
    pub has_agent: &'a dyn Fn(&str) -> Result<bool>,
    pub has_zone: &'a dyn Fn(&str) -> Result<bool>,
    /// The active tasks of this ID.
    pub tasks: &'a dyn Fn(&str) -> Result<Vec<TaskFacts>>,
    /// The pending event of this ID, if any.
    pub event: &'a dyn Fn(&str) -> Result<Option<Arc<EventRow>>>,
}

const LIFECYCLE_SCHEMA: &str = "population_lifecycle_v1 has an invalid exact schema";

/// `validate_population_lifecycle_state` of the world's rows. `lifecycle` and `presence` are
/// whether the world holds the two namespaces; `leaving` is a person whose finished relocation is
/// about to be dropped.
pub fn validate(world: &World, lifecycle: &Held, presence: &Held, leaving: Option<&str>) -> Result<()> {
    let state = value_of(world, KEY, lifecycle)?;
    let presence_held = presence;
    let presence = match presence_held {
        Held::Absent => None,
        held => Some(kit::held_row::<ValueRow>(world, PRESENCE, held)?),
    };
    let has_agent = |id: &str| -> Result<bool> { Ok(world.rows()?.agents.contains_key(id)) };
    let has_zone = |id: &str| -> Result<bool> { zone_known(world, id) };
    let tasks = |id: &str| -> Result<Vec<TaskFacts>> {
        let rows = world.rows()?;
        Ok(rows
            .active_tasks
            .iter()
            .filter(|(_, task)| &*task.id == id)
            .map(|(_, task)| TaskFacts { agent_id: task.agent_id.clone(), kind: task.kind.clone(), due_minute: task.due_minute })
            .collect())
    };
    let event = |id: &str| -> Result<Option<Arc<EventRow>>> {
        let queue = world.queue()?;
        match queue.serial_of(id) {
            None => Ok(None),
            Some(serial) => queue.slot(serial).row.clone().map(Some).ok_or_else(|| Error::unsupported(NO_ROW)),
        }
    };
    validate_facts(
        &Facts {
            state: state.as_deref().map(|row| &row.0),
            presence: presence.as_ref().map(|row| row.as_deref().map(|row| &row.0)),
            has_agent: &has_agent,
            has_zone: &has_zone,
            tasks: &tasks,
            event: &event,
        },
        leaving,
    )
}

/// `validate_population_lifecycle_state`: its ValueErrors in its order of checks. Unsupported for
/// what `PresenceState` decided (its plain form is read here) and for a key that is not hashable.
pub fn validate_facts(facts: &Facts<'_>, leaving: Option<&str>) -> Result<()> {
    let Some(state) = facts.state.filter(|state| !matches!(state, V::Null)) else {
        return Ok(());
    };
    let V::Map(document) = state else {
        return invalid(LIFECYCLE_SCHEMA);
    };
    const FIELDS: [&str; 10] = [
        "schema_version",
        "policy_version",
        "source_id",
        "people",
        "relationship_archive",
        "messages",
        "deferred_beliefs",
        "pending_returns",
        "pending_relocations",
        "history",
    ];
    if document.len() != FIELDS.len() || FIELDS.iter().any(|key| !document.contains_key(*key)) {
        return invalid(LIFECYCLE_SCHEMA);
    }
    let header = population::is_one(document.get("schema_version"))
        && is_text(document.get("policy_version"), POLICY_VERSION)
        && is_text(document.get("source_id"), SOURCE_ID);
    let (
        Some(V::Map(people)),
        Some(V::List(archive)),
        Some(V::Map(messages)),
        Some(V::Map(deferred)),
        Some(V::List(returns)),
        Some(V::Map(relocations)),
        Some(V::List(history_rows)),
    ) = (
        document.get("people"),
        document.get("relationship_archive"),
        document.get("messages"),
        document.get("deferred_beliefs"),
        document.get("pending_returns"),
        document.get("pending_relocations"),
        document.get("history"),
    )
    else {
        return invalid(LIFECYCLE_SCHEMA);
    };
    if !header || people.len() > MAX_RETAINED_PEOPLE || archive.len() > MAX_RELATIONSHIP_ARCHIVE || history_rows.len() > MAX_HISTORY {
        return invalid(LIFECYCLE_SCHEMA);
    }
    check_retained_people(facts, people)?;
    let pending_ids = check_pending_returns(facts, people, returns)?;
    check_pending_relocations(facts, relocations)?;
    check_relationship_archive(archive)?;
    check_relocation_tasks(facts, relocations, leaving)?;
    for (message_id, message) in messages {
        match message {
            V::Map(fields) if matches!(fields.get("id"), Some(V::Str(id)) if id == message_id) => json_checked(message)?,
            _ => return invalid("population_lifecycle_v1 retained message is invalid"),
        }
    }
    for beliefs in deferred.values() {
        if !matches!(beliefs, V::Map(_)) {
            return invalid("population_lifecycle_v1 deferred beliefs are invalid");
        }
        json_checked(beliefs)?;
    }
    if let Some(raw) = facts.presence {
        let raw = raw.filter(|raw| !matches!(raw, V::Null)).ok_or_else(|| Error::unsupported("no presence"))?;
        presence_is_plain(raw)?;
        let people = map_of(field(raw, "people")?)?;
        for agent_id in pending_ids {
            let record = people.get(agent_id).filter(|record| !matches!(record, V::Null)).map(Person::parse).transpose()?;
            if !record.is_some_and(|person| &*person.status == "awaiting_admission") {
                return invalid("pending return is inconsistent with population_presence_v1");
            }
        }
    }
    Ok(())
}

/// The archived people: none admitted, each of its record's shape, its snapshot and values plain.
fn check_retained_people(facts: &Facts<'_>, people: &Doc) -> Result<()> {
    const PERSON: &str = "population_lifecycle_v1 archived person is invalid";
    const RECORD: [&str; 6] = ["agent", "agent_dynamics", "body_agent", "information", "since_minute", "source_event_ids"];
    for (agent_id, record) in people {
        let V::Map(fields) = record else {
            return invalid(PERSON);
        };
        if (facts.has_agent)(agent_id)? || fields.len() != RECORD.len() || RECORD.iter().any(|key| !fields.contains_key(*key)) {
            return invalid(PERSON);
        }
        if !matches!(fields.get("since_minute"), Some(V::Int(minute)) if *minute >= 0) || !matches!(fields.get("source_event_ids"), Some(V::List(_))) {
            return invalid(PERSON);
        }
        let Some(V::Map(information)) = fields.get("information") else {
            return invalid(PERSON);
        };
        if information.len() != 2 || !matches!(information.get("beliefs"), Some(V::Map(_))) || !information.contains_key("query_pressure") {
            return invalid(PERSON);
        }
        snapshot_checked(fields.get("agent").ok_or_else(|| Error::unsupported("no agent"))?, agent_id)?;
        json_checked(record)?;
    }
    Ok(())
}

/// The pending returns: each of an archived person not admitted, to a known zone, once; their IDs.
fn check_pending_returns<'a>(facts: &Facts<'_>, people: &Doc, returns: &'a [V]) -> Result<Vec<&'a str>> {
    let mut pending_ids: Vec<&str> = Vec::with_capacity(returns.len());
    const RETURN_INVALID: &str = "population_lifecycle_v1 pending return is invalid";
    const RETURN: [&str; 6] = ["agent_id", "zone_id", "event_id", "minute", "household_id", "source_ids"];
    for item in returns {
        let V::Map(fields) = item else {
            return invalid(RETURN_INVALID);
        };
        if fields.len() != RETURN.len() || RETURN.iter().any(|key| !fields.contains_key(*key)) {
            return invalid(RETURN_INVALID);
        }
        let (Some(agent_value), Some(zone_value)) = (fields.get("agent_id"), fields.get("zone_id")) else {
            return invalid(RETURN_INVALID);
        };
        if !scalar_in(agent_value, |id| Ok(people.contains_key(id)))? {
            return invalid(RETURN_INVALID);
        }
        let V::Str(agent_id) = agent_value else {
            return refuse("a returning ID is not a str");
        };
        if (facts.has_agent)(agent_id)? || !scalar_in(zone_value, |zone| (facts.has_zone)(zone))? || pending_ids.contains(&&**agent_id) {
            return invalid(RETURN_INVALID);
        }
        if !matches!(fields.get("minute"), Some(V::Int(minute)) if *minute >= 0)
            || !matches!(fields.get("event_id"), Some(V::Str(_)))
            || !matches!(fields.get("source_ids"), Some(V::List(_)))
        {
            return invalid(RETURN_INVALID);
        }
        pending_ids.push(agent_id);
    }
    Ok(pending_ids)
}

/// The pending relocations: each of an admitted person, of its shape, between known zones, due after it starts.
fn check_pending_relocations(facts: &Facts<'_>, relocations: &Doc) -> Result<()> {
    const RELOCATION_INVALID: &str = "population_lifecycle_v1 pending relocation is invalid";
    const RELOCATION: [&str; 8] = ["task_id", "event_id", "from_zone_id", "to_zone_id", "household_id", "source_ids", "started_minute", "due_minute"];
    for (agent_id, relocation) in relocations {
        if !(facts.has_agent)(agent_id)? {
            return invalid(RELOCATION_INVALID);
        }
        let V::Map(fields) = relocation else {
            return invalid(RELOCATION_INVALID);
        };
        if fields.len() != RELOCATION.len() || RELOCATION.iter().any(|key| !fields.contains_key(*key)) {
            return invalid(RELOCATION_INVALID);
        }
        let (Some(from), Some(to)) = (fields.get("from_zone_id"), fields.get("to_zone_id")) else {
            return invalid(RELOCATION_INVALID);
        };
        if !scalar_in(from, |zone| (facts.has_zone)(zone))? || !scalar_in(to, |zone| (facts.has_zone)(zone))? {
            return invalid(RELOCATION_INVALID);
        }
        let (Some(V::Int(started)), Some(V::Int(due)), Some(V::List(_))) = (fields.get("started_minute"), fields.get("due_minute"), fields.get("source_ids"))
        else {
            return invalid(RELOCATION_INVALID);
        };
        if due <= started {
            return invalid(RELOCATION_INVALID);
        }
    }
    Ok(())
}

/// The relationship archive: each of its shape, of a known namespace and a pair of two people.
fn check_relationship_archive(archive: &[V]) -> Result<()> {
    const ARCHIVE_INVALID: &str = "population_lifecycle_v1 relationship archive is invalid";
    const NAMES: [&str; 4] = [BODY, SOCIAL_NETWORK, INFORMATION, SOCIAL_RUNTIME];
    const ARCHIVE: [&str; 5] = ["namespace", "pair_key", "agent_ids", "value", "restore_when_both_active"];
    for item in archive {
        let V::Map(fields) = item else {
            return invalid(ARCHIVE_INVALID);
        };
        if fields.len() != ARCHIVE.len() || ARCHIVE.iter().any(|key| !fields.contains_key(*key)) {
            return invalid(ARCHIVE_INVALID);
        }
        let namespace_value = fields.get("namespace").ok_or_else(|| Error::unsupported("no namespace"))?;
        let good = scalar_in(namespace_value, |name| Ok(NAMES.contains(&name)))?
            && matches!(fields.get("pair_key"), Some(V::Str(_)))
            && matches!(fields.get("agent_ids"), Some(V::List(ids)) if ids.len() == 2 && ids[0] != ids[1])
            && matches!(fields.get("restore_when_both_active"), Some(V::Bool(_)));
        if !good {
            return invalid(ARCHIVE_INVALID);
        }
        json_checked(fields.get("value").ok_or_else(|| Error::unsupported("no value"))?)?;
    }
    Ok(())
}

/// Each pending relocation (but the leaving person's) with its one travel task and its arrival event.
fn check_relocation_tasks(facts: &Facts<'_>, relocations: &Doc, leaving: Option<&str>) -> Result<()> {
    for (agent_id, relocation) in relocations {
        const INCONSISTENT: &str = "population_lifecycle_v1 relocation task/event is inconsistent";
        if leaving == Some(&**agent_id) {
            continue;
        }
        let V::Map(fields) = relocation else {
            return refuse("a relocation is not a mapping");
        };
        let (Some(V::Str(task_id)), Some(V::Str(event_id)), Some(V::Int(due))) = (fields.get("task_id"), fields.get("event_id"), fields.get("due_minute"))
        else {
            return invalid(INCONSISTENT);
        };
        let matching = (facts.tasks)(task_id)?;
        let task_fits = match matching.as_slice() {
            [task] => task.agent_id.as_deref() == Some(&**agent_id) && &*task.kind == "travel_return" && task.due_minute == *due,
            _ => false,
        };
        let event = (facts.event)(event_id)?;
        let event_fits =
            event.is_some_and(|event| &*event.event_type == "agent_travel_complete" && is_text(kit::payload(&event, "purpose"), "presence_relocation"));
        if !task_fits || !event_fits {
            return invalid(INCONSISTENT);
        }
    }
    Ok(())
}

// ---- What the rules read through the host (lane P9-3a) ---------------------------------------------

/// The refusal of a lifecycle state the world holds another value of.
pub const OTHER_LIFECYCLE: &str = "the world holds another lifecycle state";

/// What a relocation reads of the configs, each read's error the refusal the plan gives.
fn relocation_of<C: WorldCell, H: Host>(cell: &C, host: &mut H, holding: &Holding) -> std::result::Result<Relocation, H::Error> {
    let transit_capacities = match kit::kept_section(cell, host, holding, SectionId::TransitCapacities, |configs| &configs.transit_capacities) {
        Ok(Ok(found)) => Ok(*found),
        Ok(Err(fault)) => Err(population::plan_read_of(&kit::raise_section(host, SectionId::TransitCapacities, &fault))),
        Err(error) => Err(population::plan_read_of(&error)),
    };
    let timing = match kit::kept_section_value(cell, host, holding, SectionId::Movement, |configs| &configs.movement, crate::config::movement::Movement::parse)
    {
        Err(error) => Err(population::plan_read_of(&error)),
        Ok(section) => (|| -> Result<Option<MovementModel>> {
            let movement = kit::kept(&section)?;
            if !movement.times_routes() {
                return Ok(None);
            }
            match &movement.model {
                Ok(model) => Ok(Some(*model)),
                Err(fault) if fault.is_refusal() => Err(Error::unsupported(fault.text.to_string())),
                Err(_) => Err(Error::corrupt(crate::world::MODEL_CONFIGS, "a value of a section holds an error")),
            }
        })()
        .map_err(plan_read),
    };
    Ok(Relocation { transit_capacities, timing })
}

/// What the rules read beside the world's rows, through the host: the population rules' inputs,
/// whether the world holds the lifecycle, and what a relocation reads.
pub struct Read {
    pub population: population::Read,
    pub lifecycle: Held,
    pub relocation: Option<Relocation>,
}

impl Read {
    /// `relocation`: whether the rule plans a relocation, which reads the floor plan and the
    /// movement model.
    pub fn read<C: WorldCell, H: Host>(cell: &C, host: &mut H, relocation: bool) -> std::result::Result<Self, H::Error> {
        let population = population::Read::read(cell, host, false, relocation)?;
        let holding = kit::holding(cell, host)?;
        let lifecycle = holding.held(KEY, OTHER_LIFECYCLE);
        let relocation = if relocation { Some(relocation_of(cell, host, &holding)?) } else { None };
        Ok(Self { population, lifecycle, relocation })
    }

    pub fn inputs(&self) -> Inputs<'_> {
        Inputs { population: &self.population.inputs, lifecycle: self.lifecycle.clone(), relocation: self.relocation.as_ref() }
    }

    /// The rule's result, with what reading the floor plan raised where the rule read the plan.
    pub fn finish<T, E: HostFailure + From<Error>>(&self, result: std::result::Result<T, E>) -> std::result::Result<T, E> {
        self.population.finish(result)
    }

    /// A kernel on the world with these inputs, its result finished.
    pub fn run<C: WorldCell, E: HostFailure + From<Error>, T>(
        &self,
        cell: &C,
        kernel: impl FnOnce(&World, &Inputs<'_>) -> Result<T>,
    ) -> std::result::Result<T, E> {
        let inputs = self.inputs();
        self.finish(cell.with(|world| kernel(world, &inputs)).and_then(|done| done).map_err(E::from))
    }
}

/// The EventLog's records of the program of the event's provenance, as the plan of an exit or a
/// return reads them (`Host::find_record`): the row of each `effect_program_population` event
/// found; what reading one raised is raised where the plan reads it.
pub fn logged_through<C: WorldCell, H: Host>(cell: &C, host: &mut H, effects: &Held, row: &EventRow) -> std::result::Result<Logged, H::Error> {
    let mut found = Logged::new();
    let ids = cell.with(|world| program_event_ids(world, effects, row)).and_then(|done| done).map_err(H::Error::from)?;
    for event_id in ids {
        match host.find_record(&event_id) {
            Ok(None) => {}
            Ok(Some(record)) => {
                if &*record.event.event_type == "effect_program_population" {
                    found.insert(event_id, Ok(record.event));
                }
            }
            Err(error) => {
                found.insert(event_id, Err(population::plan_read_of(&error)));
            }
        }
    }
    Ok(found)
}

/// The command on the world of its transaction: the plan reads the pending events as rows (an
/// archive cancels the person's), and an exit or a return the EventLog's records of its program.
pub fn act<C: WorldCell, H: Host>(cell: &C, host: &mut H, row: &EventRow, command: &Command) -> std::result::Result<Acted, H::Error> {
    let read = Read::read(cell, host, &*command.action == "relocate")?;
    let effects = kit::holding(cell, host)?.held(EFFECT_PROGRAM, population::OTHER_PROGRAM);
    host.read_queue_rows()?;
    let logged = if matches!(&*command.action, "temporary_exit" | "return") { logged_through(cell, host, &effects, row)? } else { Logged::new() };
    read.run(cell, |world, inputs| {
        let reads = world_reads(world, &effects, &logged, row, command);
        presence_action(world, inputs, row, command, &reads)
    })
}

/// `PopulationPresenceCommand.selected_ids()` of the contract's dump: Err(text) is its
/// `ValueError`.
pub fn selected_ids(dumped: &V) -> Result<std::result::Result<Vec<Arc<str>>, String>> {
    let fields = map_of(dumped)?;
    let mut ids = Vec::new();
    if let Some(V::List(agent_ids)) = fields.get("agent_ids") {
        for id in agent_ids {
            ids.push(text(id)?);
        }
    }
    if let Some(V::Str(agent_id)) = fields.get("agent_id") {
        if !agent_id.is_empty() {
            ids.push(agent_id.clone());
        }
    }
    // `sorted(set(...))`: bytes of UTF-8 sort as code points do.
    ids.sort_unstable();
    ids.dedup();
    let action = match fields.get("action") {
        Some(V::Str(action)) => action.clone(),
        _ => return refuse("a value is not a str"),
    };
    if ids.is_empty() {
        return Ok(Err("population command requires at least one agent id".to_owned()));
    }
    if matches!(&*action, "temporary_exit" | "return" | "transfer" | "relocate") && ids.len() != 1 {
        return Ok(Err(format!("{action} requires exactly one agent id")));
    }
    if &*action == "reunite" && ids.len() < 2 {
        return Ok(Err("reunite requires at least two agent ids".to_owned()));
    }
    Ok(Ok(ids))
}

impl Command {
    /// What `PopulationPresenceCommand` validated (`model_dump(mode="json")`), with its
    /// `selected_ids`.
    pub fn of_dumped(dumped: &V, ids: Vec<Arc<str>>) -> Result<Self> {
        let fields = map_of(dumped)?;
        let field = |key: &str| fields.get(key).unwrap_or(&V::Null);
        let optional_str = |key: &str| -> Result<Option<Arc<str>>> {
            match field(key) {
                V::Null => Ok(None),
                V::Str(found) => Ok(Some(found.clone())),
                _ => refuse("a value is not a str"),
            }
        };
        let mut source_ids = Vec::new();
        match field("source_ids") {
            V::List(sources) => {
                for source in sources {
                    match source {
                        V::Str(source) => source_ids.push(source.clone()),
                        _ => return refuse("a value is not a str"),
                    }
                }
            }
            _ => return refuse("an input is not iterable"),
        }
        let action = match field("action") {
            V::Str(action) => action.clone(),
            _ => return refuse("a value is not a str"),
        };
        let expected_return = match field("expected_return_minute") {
            V::Null => None,
            V::Int(minute) => Some(*minute),
            _ => return refuse("a value is not an int"),
        };
        Ok(Command { action, zone_id: optional_str("zone_id")?, household_id: optional_str("household_id")?, expected_return, source_ids, ids })
    }
}

/// How a presence command's transaction ended.
enum Step<E> {
    Done(V),
    /// The exit was dropped: the person was not free before the expected return.
    Dropped,
    /// The command waits: its retry is scheduled on the world as it was.
    Deferred(String),
    /// The world it left failed its checks.
    Invalid(E),
}

/// A presence command as a payload gives it: the contract (`Host::contract`, policy E15) and its
/// `selected_ids`, whose `TypeError` or `ValueError` is the rule's text, then
/// `presence_command_typed`.
pub fn presence_command<C: WorldCell, H: Host>(cell: &C, host: &mut H, row: &EventRow, command: &V) -> std::result::Result<V, H::Error> {
    let dumped = match host.contract(&Contract::PresenceCommand, command) {
        Ok(dumped) => dumped,
        Err(error) if matches!(error.class(), ErrorClass::Type | ErrorClass::Value) => {
            return Err(Error::failed(format!("population command is invalid: {}", error.text())).into());
        }
        Err(error) => return Err(population::planning(error)),
    };
    presence_command_typed(cell, host, row, &dumped)
}

/// A presence command the contract validated (its dump): the `selected_ids`, the installed
/// lifecycle, then the command's rule in a transaction that stands for a copy of the world
/// (`from_dict(deepcopy(to_dict()))`, the EventLog included). A dropped exit, a deferral and a
/// world that fails its checks roll it back, and their outcome is raised after: a deferral
/// schedules its retry on the world as it was. The receipt.
pub fn presence_command_typed<C: WorldCell, H: Host>(cell: &C, host: &mut H, row: &EventRow, dumped: &V) -> std::result::Result<V, H::Error> {
    let command = (|| -> std::result::Result<Command, H::Error> {
        let ids = match selected_ids(dumped)? {
            Ok(ids) => ids,
            Err(text) => return Err(Error::failed(format!("population command is invalid: {text}")).into()),
        };
        let holding = kit::holding(cell, host)?;
        if !holding.table {
            return Err(Error::unsupported(crate::host::TABLE_NOT_PLAIN).into());
        }
        if !holding.keys.contains_key(KEY) {
            return Err(Error::failed("population lifecycle contract is not installed").into());
        }
        Ok(Command::of_dumped(dumped, ids)?)
    })()
    .map_err(population::planning)?;
    let ended = crate::txn::rule(cell, host, crate::txn::TxnKind::Full, |cell, host| {
        let checked = cell
            .with(|world| check_command(world, row, command.expected_return))
            .and_then(|done| done)
            .map_err(H::Error::from)
            .map_err(population::planning)?;
        if checked {
            return Ok(TxnEnd::Rollback(Step::Dropped));
        }
        let done = match act(cell, host, row, &command)? {
            Acted::Done(done) => done,
            Acted::Deferred(reason) => return Ok(TxnEnd::Rollback(Step::Deferred(reason))),
        };
        host.slots_changed(&done.taken)?;
        let checked = (|| -> std::result::Result<(), H::Error> {
            let holding = kit::holding(cell, host)?;
            let (lifecycle, presence) = (holding.held(KEY, OTHER_LIFECYCLE), holding.held(PRESENCE, "the world holds another presence state"));
            cell.with(|world| validate(world, &lifecycle, &presence, None)).and_then(|done| done).map_err(H::Error::from).map_err(population::planning)?;
            host.validate_world()?;
            host.validate_extensions()
        })();
        Ok(match checked {
            Ok(()) => TxnEnd::Commit(Step::Done(done.receipt)),
            Err(error) => TxnEnd::Rollback(Step::Invalid(error)),
        })
    })?;
    match ended {
        Step::Done(receipt) => Ok(receipt),
        Step::Dropped => Err(Error::suppressed(DROPPED).into()),
        Step::Deferred(reason) => {
            population::schedule_retry_through(cell, host, row, "presence-retry", "population exit")?;
            Err(Error::suppressed(deferred_text(&reason)).into())
        }
        Step::Invalid(error) => {
            if matches!(error.class(), ErrorClass::Type | ErrorClass::Key | ErrorClass::Value) {
                return Err(Error::failed(format!("population command failed validation: {}", error.text())).into());
            }
            Err(error)
        }
    }
}

/// `admit_pending_returns`: nothing is read past the namespace while no one waits.
pub fn admit_returns_through<C: WorldCell, H: Host>(cell: &C, host: &mut H, minute: i64) -> std::result::Result<Vec<Arc<str>>, H::Error> {
    let lifecycle = kit::holding(cell, host)?.held(KEY, OTHER_LIFECYCLE);
    let nothing = cell.with(|world| nothing_to_admit(world, &lifecycle)).and_then(|done| done).map_err(H::Error::from).map_err(population::planning)?;
    if nothing {
        return Ok(Vec::new());
    }
    let read = Read::read(cell, host, false)?;
    // The admission reads the pending events as rows.
    host.read_queue_rows()?;
    read.run(cell, |world, inputs| admit_returns(world, inputs, minute))
}
