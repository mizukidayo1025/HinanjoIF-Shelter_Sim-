//! The events of an autonomous staff operation: arrival, relief, completion and interruption, with
//! `StaffOperationCompletionHandler`'s receipts, on the rows of a `World`. The caller (the dispatch
//! and the arbitrate phase) reads what these take besides the world (`Inputs`: whether
//! `model_state` holds each namespace, the floor plan, the staff operations formula of the configs)
//! and calls them with the event's row.
//!
//! Each plans with nothing changed and then writes: `staff_operations_v1` is written as its row,
//! and so are the entities, `body_state_v1`, `infrastructure_v1`, `movement_v1`, the event counter,
//! the queue and the log. A namespace whose row a write that did not fit dropped (tainted) is
//! refused, as is another document than the row's under its key. Unsupported comes only before the
//! first write. The outcomes are the rules' texts: each before any change, except a worker who
//! became unavailable on arrival or completion, whose operation stops first (`interrupt`).
//!
//! The staff positions after a change are `movement::staff_position_snapshots`, called directly:
//! its refusal is labelled with that call (`SNAPSHOTS`). So are the staff dynamics' readers
//! (`staff_dynamics::is_available_now`, `on_shift_now`).
//!
//! The admission of an intake is the population rules' (`population::check_waiting`,
//! `population::admit_arrivals`, lane P9-3a): `plan_complete` says `Work::Intake`, the caller
//! checks the waiting and admits, and then calls `write_complete`, in that order. The queue's slots
//! a rule takes out are added to `taken`, as it goes, for the host (`Host::slots_changed`).

use std::sync::Arc;

use indexmap::IndexMap;

use crate::cell::ShardRef;
use crate::config::staff::StaffOperations;
use crate::config::SectionId;
use crate::entities::Entities;
use crate::error::{Error, Result};
use crate::events::writer::WriteResult;
use crate::events::{EventKind, EventRow, EventStatus};
use crate::geometry::Geo;
use crate::host::Host;
use crate::mathx::{py_max, py_min};
use crate::row::ValueRow;
use crate::rules::care_receipts::{self, Made, OperationOutcome};
use crate::rules::common_operations::Change;
use crate::rules::kit::{self, Events, Held};
use crate::rules::movement::{self, StaffOps};
use crate::rules::staff_dynamics;
use crate::rules::travel::ZONES_TAINTED;
use crate::state::body_state::{BodyRecord, BodyRow, NAME as BODY};
use crate::state::effect_program::{EffectProgramRow, NAME as PROGRAM};
use crate::state::movement::{MovementRow, Position};
use crate::state::staff_operations::{Entries, FinishedTask, Number as ConfigNumber, OperationRecord, Outcome, Phase, StaffOperationsRow, Table, NAME};
use crate::value::V;
use crate::world::{World, WorldCell};

const MOVEMENT: &str = "movement_v1";
const INFRASTRUCTURE: &str = "infrastructure_v1";
const STAFF_DYNAMICS: &str = "staff_dynamics_v1";
const MAX_HISTORY: usize = 20_000;
const RELIEF_POLICY: &str = "same-site-staff-relief-v1";
const RELIEF_SOURCE: &str = "ASM-STAFF-RELIEF-V1";

/// The label of the refusal of `staff_position_snapshots`.
pub const SNAPSHOTS: &str = "movement.staff_position_snapshots";
/// The refusal of a rule that reads the staff operations formula the host could not read: the
/// caller gives what the read gave, where the rule read it (`Host::read_again`).
pub const FORMULA_UNREAD: &str = "the staff operations formula could not be read";
/// The refusal of `staff_position_snapshots` when the host could not read the floor plan: the
/// caller gives what the read gave, labelled with the snapshot (`SNAPSHOTS`) where it is a refusal.
pub const SNAPSHOT_PLAN_UNREAD: &str = "the floor plan of the staff positions could not be read";

/// What the rules read besides the rows, read before the world is borrowed.
pub struct Inputs<'a> {
    /// `staff_operations_v1`. A value that is not a dict is no state (`Absent`), as the rules read
    /// it with `isinstance(state, dict)`.
    pub staff: &'a Held,
    pub movement: &'a Held,
    pub body: &'a Held,
    /// `effect_program_v1`, for `kit::has_active_modifier`.
    pub program: &'a Held,
    /// `SHARED_OPERATIONS_KEY not in world.model_state`: no receipts.
    pub shared_absent: bool,
    /// The floor plan, or None where `Geo::of` raised.
    pub geo: Option<&'a Arc<Geo>>,
    /// The staff operations formula, or None where its read raised.
    pub formula: Option<&'a StaffOperations>,
}

/// The refusals of the namespaces the world holds other values of.
pub const OTHER_STAFF: &str = "the world holds another document than the row's";
pub const OTHER_MOVEMENT: &str = "the world holds another movement state";
pub const OTHER_BODY: &str = "the world holds another body state";
pub const OTHER_PROGRAM: &str = "the world holds another effect_program_v1";
const SHARED_OPERATIONS: &str = "shared_operations_v1";

/// What the rules read of the world's outside, read once per entry through the host before the
/// world is borrowed (lane P9-1a): whether `model_state` holds the namespaces (H1), the floor plan
/// (`Host::floor_plan`) where the entry may read it, and the staff operations formula (H2) where it
/// may read it. What the floor plan's read raised, and the formula's fault, are raised where the
/// core reads them (`PLAN_UNREAD`, `SNAPSHOT_PLAN_UNREAD`, `FORMULA_UNREAD`), by the caller.
pub struct Gathered<E> {
    pub staff: Held,
    pub movement: Held,
    pub body: Held,
    pub program: Held,
    pub shared_absent: bool,
    pub plan: Option<std::result::Result<Arc<Geo>, E>>,
    pub formula: Option<kit::Kept<StaffOperations>>,
}

impl<E> Gathered<E> {
    /// The namespaces, then the floor plan where `geo`, then the formula where `formula`.
    pub fn read<C: WorldCell, H: Host<Error = E>>(cell: &C, host: &mut H, geo: bool, formula: bool) -> std::result::Result<Self, E> {
        let holding = kit::holding(cell, host)?;
        let plan = if geo { Some(host.floor_plan()) } else { None };
        let formula =
            if formula { Some(kit::kept_section(cell, host, &holding, SectionId::StaffOperations, |configs| &configs.staff_operations)?) } else { None };
        Ok(Self {
            staff: holding.held_dict(NAME, OTHER_STAFF),
            movement: holding.held(crate::rules::movement::KEY, OTHER_MOVEMENT),
            body: holding.held(BODY, OTHER_BODY),
            program: holding.held(PROGRAM, OTHER_PROGRAM),
            shared_absent: !holding.contains(SHARED_OPERATIONS),
            plan,
            formula,
        })
    }

    pub fn inputs(&self) -> Inputs<'_> {
        Inputs {
            staff: &self.staff,
            movement: &self.movement,
            body: &self.body,
            program: &self.program,
            shared_absent: self.shared_absent,
            geo: self.plan.as_ref().and_then(|plan| plan.as_ref().ok()),
            formula: self.formula.as_ref().and_then(|formula| formula.as_ref().ok()).map(|formula| &**formula),
        }
    }
}

impl Inputs<'_> {
    #[track_caller]
    fn formula(&self) -> Result<&StaffOperations> {
        self.formula.ok_or_else(|| Error::unsupported(FORMULA_UNREAD))
    }

    fn plan(&self) -> Result<&Arc<Geo>> {
        crate::rules::travel::plan(self.geo)
    }

    fn movement_held(&self) -> bool {
        !matches!(self.movement, Held::Absent)
    }
}

// ---- Readers ----------------------------------------------------------------------------------------

/// The row of `staff_operations_v1`, or None where the world has no state.
fn ops_row<'w>(world: &'w World, inputs: &Inputs<'_>) -> Result<Option<ShardRef<'w, StaffOperationsRow>>> {
    kit::held_row::<StaffOperationsRow>(world, NAME, inputs.staff)
}

/// Whether one member's status lets them work (`StaffStatus::is_available`).
fn available(world: &World, id: &str) -> Result<bool> {
    staff_dynamics::with_staff_status(world, |status| status.is_available(id))
}

/// `staff_dynamics::is_available_now`: true without the namespace.
pub fn dynamics_available(world: &World, id: &str, minute: i64) -> Result<bool> {
    if kit::namespace_absent(world, STAFF_DYNAMICS)? {
        return Ok(true);
    }
    staff_dynamics::is_available_now(world, id, minute, false)
}

/// `staff_dynamics::on_shift_now`: true without the namespace.
fn dynamics_on_shift(world: &World, id: &str, minute: i64) -> Result<bool> {
    if kit::namespace_absent(world, STAFF_DYNAMICS)? {
        return Ok(true);
    }
    staff_dynamics::on_shift_now(world, id, minute)
}

/// An optional text of a row as a value: a str or None.
fn opt_text(text: &Option<Arc<str>>) -> V {
    text.clone().map_or(V::Null, V::Str)
}

/// A value of a payload where the rule compares it with an optional text of a row: a str, None (or
/// no such key, as `payload.get` reads it), or any other value, which equals no such text.
#[derive(Clone, Debug)]
enum PayloadText {
    Text(Arc<str>),
    Null,
    Other,
}

impl PayloadText {
    fn of_value(value: &V) -> Self {
        match value {
            V::Str(text) => Self::Text(text.clone()),
            V::Null => Self::Null,
            _ => Self::Other,
        }
    }

    /// `payload.get(name)`.
    fn of(event: &EventRow, name: &str) -> Self {
        Self::of_value(kit::payload(event, name).unwrap_or(&V::Null))
    }

    /// An optional text of a row, as a value: a str or None.
    fn of_row(text: &Option<Arc<str>>) -> Self {
        text.clone().map_or(Self::Null, Self::Text)
    }

    fn text(&self) -> Option<&Arc<str>> {
        match self {
            Self::Text(text) => Some(text),
            Self::Null | Self::Other => None,
        }
    }

    /// `value == text` of this value and an optional text of a row.
    fn is(&self, text: &Option<Arc<str>>) -> bool {
        match (self, text) {
            (Self::Text(value), Some(text)) => value == text,
            (Self::Null, None) => true,
            _ => false,
        }
    }

    /// `value == self` of another payload's value, where this one is a str or None: `Other` is
    /// never compared (each caller has matched it with a row's text first).
    fn is_value(&self, value: &V) -> bool {
        match (self, value) {
            (Self::Text(text), V::Str(value)) => text == value,
            (Self::Null, V::Null) => true,
            _ => false,
        }
    }
}

/// `dict.get(value)` of a table keyed by str: None for a value that is no str, Unsupported for one
/// Python can't hash.
#[track_caller]
fn key_of(value: &V) -> Result<Option<&Arc<str>>> {
    match value {
        V::Str(text) => Ok(Some(text)),
        V::List(_) | V::Map(_) => Err(Error::unsupported("a key Python cannot hash")),
        _ => Ok(None),
    }
}

/// `sorted(set(ids + extra))`.
fn causation_with(ids: &[Arc<str>], extra: &[Arc<str>]) -> Vec<Arc<str>> {
    let mut all: Vec<Arc<str>> = ids.iter().chain(extra).cloned().collect();
    all.sort_unstable();
    all.dedup();
    all
}

/// The record of an idle member.
fn idle_record() -> OperationRecord {
    OperationRecord {
        phase: Phase::Idle,
        task_id: None,
        task_kind: None,
        target_id: None,
        target_zone_id: None,
        reason: None,
        started_minute: None,
        travel_due_minute: None,
        arrival_minute: None,
        due_minute: None,
        route_zone_ids: Vec::new(),
        route_edge_ids: Vec::new(),
        travel_minutes: 0,
        causation_event_ids: Vec::new(),
    }
}

/// `state["history"].append(entry)` and the cut to its limit.
fn append_history(row: &mut StaffOperationsRow, entry: FinishedTask) {
    row.history.push(entry);
    let length = row.history.len();
    if length > MAX_HISTORY {
        row.history.drain(..length - MAX_HISTORY);
    }
}

#[track_caller]
fn gone(what: &str) -> Error {
    Error::Runtime(format!("{what} left under the plan"))
}

/// `staff.work_queue.clear(); staff.active_task_id = None; staff.busy_until = 0`.
fn free_staff(rows: &mut Entities, staff_id: &str) -> Result<()> {
    let staff = rows.staff.get_mut(staff_id).ok_or_else(|| gone("the staff member"))?;
    staff.work_queue.clear();
    staff.active_task_id = None;
    staff.busy_until = 0;
    Ok(())
}

/// `state["staff"][staff_id] = record`.
fn set_record(world: &World, staff_id: &Arc<str>, record: OperationRecord) -> Result<()> {
    world.row_mut::<StaffOperationsRow>(NAME)?.staff.insert(staff_id.clone(), record);
    Ok(())
}

/// The pending events in queue order, as rows: the caller reads the queue's rows through the host
/// (`Host::read_queue_rows`) first.
fn pending_rows(world: &World) -> Result<Vec<Arc<EventRow>>> {
    let queue = world.queue()?;
    let mut rows = Vec::with_capacity(queue.ordered().len());
    for (_, serial) in queue.ordered() {
        let Some(row) = &queue.slot(*serial).row else {
            return Err(Error::unsupported("an event in the queue is not a plain one"));
        };
        rows.push(row.clone());
    }
    Ok(rows)
}

/// `event.payload.get("task_id") == task_id`.
fn names_task(row: &EventRow, task_id: &PayloadText) -> Result<bool> {
    let V::Map(payload) = &row.payload else {
        return Err(Error::unsupported("a payload is not a plain dict"));
    };
    Ok(task_id.is_value(payload.get("task_id").unwrap_or(&V::Null)))
}

/// `event_queue.remove(event.event_id)` and the SUPPRESSED record of each event.
fn suppress(events: &Events<'_>, pending: &[Arc<EventRow>], minute: i64, detail: &str, taken: &mut Vec<u64>) -> WriteResult<()> {
    for event in pending {
        let slot = events.world().queue_mut()?.take_slot(&event.event_id);
        if let Some((serial, _)) = slot {
            taken.push(serial);
        }
        events.record(event, minute, EventStatus::Suppressed, Some(detail))?;
    }
    Ok(())
}

/// `movement::staff_position_snapshots` at the boundary, where `model_state` holds `movement_v1`:
/// the staff positions at the boundary written, and every one in ID order. A refusal is the
/// snapshot's `Error::Refused`, but for `SNAPSHOT_PLAN_UNREAD`.
pub fn snapshots(world: &World, inputs: &Inputs<'_>, boundary: i64) -> Result<Vec<(Arc<str>, Position)>> {
    let run = || -> Result<Vec<(Arc<str>, Position)>> {
        if let Held::Other(reason) = inputs.movement {
            return Err(Error::unsupported(*reason));
        }
        let Some(geo) = inputs.geo else {
            return Err(Error::unsupported(SNAPSHOT_PLAN_UNREAD));
        };
        let staff = match inputs.staff {
            Held::Absent => StaffOps::None,
            Held::Held => StaffOps::Row,
            Held::Other(reason) => return Err(Error::unsupported(*reason)),
        };
        movement::staff_position_snapshots(world, geo, &staff, boundary)
    };
    match run() {
        Err(Error::Unsupported(reason)) if reason == SNAPSHOT_PLAN_UNREAD => Err(Error::Unsupported(reason)),
        other => other.map_err(|error| Error::refused(SNAPSHOTS, error)),
    }
}

// ---- Interruption -------------------------------------------------------------------------------

/// The event that stops an operation: its ID and due minute.
pub struct Stopping<'a> {
    pub event_id: &'a str,
    pub due_minute: i64,
}

/// What `interrupt` read before it writes.
struct InterruptPlan {
    staff_id: Arc<str>,
    task_id: Option<Arc<str>>,
    nearest: Option<Arc<str>>,
    pending: Vec<Arc<EventRow>>,
    entry: FinishedTask,
}

fn plan_interrupt(world: &World, inputs: &Inputs<'_>, staff_id: &str, event: &Stopping<'_>, current: Option<&str>) -> Result<Option<InterruptPlan>> {
    let record = match ops_row(world, inputs)? {
        Some(row) => row.staff.get(staff_id).cloned(),
        None => None,
    };
    let staff = world.rows()?.staff.get(staff_id).map(|staff| staff.id.clone());
    let (Some(staff_key), Some(record)) = (staff, record) else {
        return Ok(None);
    };
    if record.phase == Phase::Idle {
        return Ok(None);
    }
    let task_id = PayloadText::of_row(&record.task_id);
    let due = event.due_minute;
    let Some(started) = record.started_minute else {
        return Err(Error::unsupported("a value is not an int"));
    };
    let travel = record.travel_minutes;
    let elapsed = 0.max(travel.min(due - started));
    let mut nearest = None;
    if record.phase == Phase::Travel {
        let rows = world.entities.try_borrow()?;
        if rows.zones_tainted().is_some() {
            return Err(Error::unsupported(ZONES_TAINTED));
        }
        let zones = &record.route_zone_ids;
        let zone_at = |index: usize| zones.get(index).cloned().ok_or_else(|| Error::unsupported("a route has fewer zones than its edges need"));
        let mut cumulative = 0_i64;
        let mut zone = zone_at(0)?;
        for (index, edge_id) in record.route_edge_ids.iter().enumerate() {
            let Some(edge) = rows.zone_edges.get(edge_id) else {
                return Err(Error::Key(edge_id.to_string()));
            };
            let edge_minutes = edge.travel_minutes;
            if edge_minutes != 0 && elapsed < cumulative + edge_minutes {
                let progress = (elapsed - cumulative) as f64 / edge_minutes as f64;
                zone = zone_at(if progress < 0.5 { index } else { index + 1 })?;
                break;
            }
            cumulative += edge_minutes;
            zone = zone_at(index + 1)?;
            if elapsed <= cumulative {
                break;
            }
        }
        nearest = Some(zone);
    }
    let mut causation_ids: Vec<Arc<str>> = vec![Arc::from(event.event_id)];
    let mut pending = Vec::new();
    for row in pending_rows(world)? {
        if current.is_some_and(|current| &*row.event_id == current)
            || !matches!(&*row.event_type, "staff_operation_arrive" | "staff_operation_complete")
            || !names_task(&row, &task_id)?
        {
            continue;
        }
        causation_ids.push(row.event_id.clone());
        pending.push(row);
    }
    let arrival = match record.arrival_minute {
        None if travel > 0 && elapsed >= travel => Some(started + travel),
        arrival => arrival,
    };
    let (Some(task), Some(kind), Some(zone), Some(reason)) =
        (record.task_id.clone(), record.task_kind.clone(), record.target_zone_id.clone(), record.reason.clone())
    else {
        return Err(Error::unsupported("an active record lacks a field of the history"));
    };
    let entry = FinishedTask {
        task_id: task,
        staff_id: Arc::from(staff_id),
        task_kind: kind,
        target_id: record.target_id.clone(),
        target_zone_id: zone,
        reason,
        status: Outcome::Interrupted,
        start_minute: started,
        arrival_minute: arrival,
        complete_minute: due,
        travel_minutes: travel,
        causation_event_ids: causation_with(&record.causation_event_ids, &causation_ids),
    };
    Ok(Some(InterruptPlan { staff_id: staff_key, task_id: record.task_id, nearest, pending, entry }))
}

/// The member's operation interrupted by `event`, with `current` the event being applied: whether
/// it stopped an operation. Its reads refuse before its first write.
pub fn interrupt(world: &World, inputs: &Inputs<'_>, staff_id: &str, event: &Stopping<'_>, current: Option<&str>, taken: &mut Vec<u64>) -> WriteResult<bool> {
    let Some(plan) = plan_interrupt(world, inputs, staff_id, event, current)? else {
        return Ok(false);
    };
    let events = Events::of(world)?;

    // Write.
    if let Some(zone) = plan.nearest {
        if let Some(staff) = world.rows_mut()?.staff.get_mut(&plan.staff_id) {
            staff.zone_id = zone;
        }
    }
    suppress(&events, &plan.pending, event.due_minute, "Staff operation was interrupted by staff unavailability", taken)?;
    append_history(&mut *world.row_mut::<StaffOperationsRow>(NAME)?, plan.entry);
    {
        let mut rows = world.rows_mut()?;
        if let Some(task) = &plan.task_id {
            rows.active_tasks.remove_row(task);
        }
        free_staff(&mut rows, &plan.staff_id)?;
    }
    set_record(world, &plan.staff_id, idle_record())?;
    if inputs.movement_held() {
        snapshots(world, inputs, event.due_minute)?;
    }
    Ok(true)
}

/// What a plan found: go on, or the worker became unavailable (by ID), which stops the operation and
/// then suppresses the event (`stop_and_suppress`).
pub enum Planned<P> {
    Go(P),
    Unavailable(Arc<str>),
}

/// The member's operation interrupted by the event (`interrupt`), and then the event suppressed
/// with `text`: the stop stays in the world, as the dispatcher keeps it.
pub fn stop_and_suppress(world: &World, inputs: &Inputs<'_>, staff_id: &str, event: &EventRow, text: &str, taken: &mut Vec<u64>) -> WriteResult<()> {
    let stopping = Stopping { event_id: &event.event_id, due_minute: event.due_minute };
    interrupt(world, inputs, staff_id, &stopping, Some(&event.event_id), taken)?;
    Err(Error::suppressed(text).into())
}

// ---- Arrival -----------------------------------------------------------------------------------------

/// What an arrival writes.
pub struct ArrivePlan {
    staff_id: Arc<str>,
    zone: Arc<str>,
    due: i64,
    event_id: Arc<str>,
}

/// The checks of a staff operation's arrival, in a fixed order.
pub fn plan_arrive(world: &World, inputs: &Inputs<'_>, event: &EventRow) -> Result<Planned<ArrivePlan>> {
    if &*event.event_type != "staff_operation_arrive" {
        return Err(Error::failed("staff operation arrival handler received another event"));
    }
    let not_active = || Err(Error::failed("staff travel arrival does not match an active operation"));
    let payload = |name: &str| kit::payload(event, name).cloned().unwrap_or(V::Null);
    let (V::Str(id), V::Str(task)) = (payload("staff_id"), payload("task_id")) else {
        return not_active();
    };
    let Some(row) = ops_row(world, inputs)? else {
        return not_active();
    };
    let record = row.staff.get(&*id).cloned();
    drop(row);
    let (staff_task, has_task) = {
        let rows = world.rows()?;
        (rows.staff.get(&*id).map(|staff| staff.active_task_id.clone()), rows.active_tasks.contains_key(&*task))
    };
    let (Some(record), Some(staff_task)) = (record, staff_task) else {
        return not_active();
    };
    if record.phase != Phase::Travel
        || record.task_id.as_ref() != Some(&task)
        || !has_task
        || staff_task.as_ref() != Some(&task)
        || record.travel_due_minute != Some(event.due_minute)
        || !PayloadText::of(event, "target_zone_id").is(&record.target_zone_id)
    {
        return not_active();
    }
    if !available(world, &id)? {
        return Ok(Planned::Unavailable(id));
    }
    let Some(zone) = record.target_zone_id else {
        return Err(Error::unsupported("a zone is not a str"));
    };
    Ok(Planned::Go(ArrivePlan { staff_id: id, zone, due: event.due_minute, event_id: event.event_id.clone() }))
}

/// The writes of a staff operation's arrival: the member is at the task, which it works from now.
/// The arrival: the plan (`plan_arrive`), then its writes; the staff member who became
/// unavailable before reaching the task, if any.
pub fn arrive(world: &World, inputs: &Inputs<'_>, event: &EventRow) -> Result<Option<Arc<str>>> {
    match plan_arrive(world, inputs, event)? {
        Planned::Go(plan) => write_arrive(world, plan).map(|()| None),
        Planned::Unavailable(id) => Ok(Some(id)),
    }
}

pub fn write_arrive(world: &World, plan: ArrivePlan) -> Result<()> {
    if let Some(staff) = world.rows_mut()?.staff.get_mut(&plan.staff_id) {
        staff.zone_id = plan.zone;
    }
    let mut row = world.row_mut::<StaffOperationsRow>(NAME)?;
    let record = row.staff.get_mut(&plan.staff_id).ok_or_else(|| gone("the record"))?;
    record.phase = Phase::Working;
    record.travel_due_minute = None;
    record.arrival_minute = Some(plan.due);
    record.route_zone_ids.clear();
    record.route_edge_ids.clear();
    record.causation_event_ids.push(plan.event_id);
    Ok(())
}

// ---- Relief ------------------------------------------------------------------------------------------

/// `{"cleaning": {...}, ...}[kind]`.
pub fn relief_skills(kind: &str) -> Option<[&'static str; 2]> {
    match kind {
        "cleaning" => Some(["cleaning", "sanitation"]),
        "maintenance" => Some(["maintenance", "repair"]),
        "information_outreach" => Some(["support", "information"]),
        "intake" => Some(["support", "intake"]),
        "patrol" => Some(["support", "patrol"]),
        _ => None,
    }
}

/// What a relief writes.
pub struct ReliefPlan {
    task_id: Arc<str>,
    previous_id: Arc<str>,
    replacement_id: Arc<str>,
    reason_code: Arc<str>,
    pending: Vec<Arc<EventRow>>,
    updated: OperationRecord,
    due: i64,
    now: i64,
    event_id: Arc<str>,
    formula_source: Arc<str>,
    formula_policy: Arc<str>,
    event_source: Arc<str>,
}

/// `config["task_duration_minutes"][kind]`, an int.
fn task_minutes(config: Option<Entries<'_>>, kind: &str) -> Result<i64> {
    let Some(config) = config else {
        return Err(Error::unsupported("the config is not a map"));
    };
    let durations = match config.table("task_duration_minutes") {
        Table::Map(durations) => durations,
        Table::Missing => return Err(Error::Key("task_duration_minutes".to_owned())),
        Table::Other => return Err(Error::unsupported("the task durations are not a map")),
    };
    match durations.number(kind) {
        Some(ConfigNumber::Int(minutes)) => Ok(minutes),
        Some(ConfigNumber::Big) => Err(Error::unsupported("an int is too large")),
        Some(_) => Err(Error::unsupported("a value is not an int")),
        None => Err(Error::Key(kind.to_owned())),
    }
}

/// The config of the row the plan read before, read again where the rule reads it.
fn with_config<R>(world: &World, inputs: &Inputs<'_>, f: impl FnOnce(Option<Entries<'_>>) -> Result<R>) -> Result<R> {
    let row = ops_row(world, inputs)?.ok_or_else(|| gone("the row"))?;
    f(row.config_entries())
}

/// The checks of `StaffOperationReliefHandler.apply`, in its order: the event's provenance and
/// payload, the staleness of the relief (`EventSuppressed`), the replacement's skill, the pending
/// completion.
pub fn plan_relief(world: &World, inputs: &Inputs<'_>, event: &EventRow) -> Result<ReliefPlan> {
    let now = world.minute_index()?;
    let provenance_is = |key: &str, wanted: &str| matches!(kit::provenance(event, key), Some(V::Str(text)) if &**text == wanted);
    let fields_are = match &event.payload {
        V::Map(payload) => {
            payload.len() == 4 && ["task_id", "previous_staff_id", "replacement_staff_id", "reason_code"].iter().all(|name| payload.contains_key(*name))
        }
        _ => false,
    };
    if &*event.event_type != "staff_operation_relief"
        || !kit::kind_is(event, EventKind::Endogenous)
        || event.due_minute != now
        || event.source_id.as_deref() != Some(RELIEF_POLICY)
        || !provenance_is("source_type", "assumption")
        || !provenance_is("source_id", RELIEF_SOURCE)
        || !provenance_is("policy_version", RELIEF_POLICY)
        || !fields_are
    {
        return Err(Error::failed("staff operation relief provenance or payload is invalid"));
    }
    let payload = |name: &str| kit::payload(event, name).unwrap_or(&V::Null);
    let task_id = payload("task_id");
    let task = PayloadText::of_value(task_id);
    let previous_id = kit::exact_str(payload("previous_staff_id"))?.clone();
    let replacement_id = kit::exact_str(payload("replacement_staff_id"))?.clone();
    let stale = || Err(Error::suppressed("staff relief is stale or its replacement is no longer available"));
    let Some(row) = ops_row(world, inputs)? else {
        return stale();
    };
    let Some(operation) = row.staff.get(&*previous_id).cloned() else {
        return stale();
    };
    let replacement_phase = row.staff.get(&*replacement_id).map(|record| record.phase);
    drop(row);
    let (task_staff, previous_task, replacement) = {
        let rows = world.rows()?;
        let task = match key_of(task_id)? {
            Some(key) => rows.active_tasks.get(key).map(|task| task.staff_id.clone()),
            None => None,
        };
        let previous = rows.staff.get(&*previous_id).map(|staff| staff.active_task_id.clone());
        let replacement = rows.staff.get(&*replacement_id).map(|staff| (staff.zone_id.clone(), staff.active_task_id.is_some(), staff.skills.0.clone()));
        (task, previous, replacement)
    };
    let replacement_zone = replacement.as_ref().map(|(zone, _, _)| zone.clone());
    // An `or` in a fixed order: each test is reached only if those before it passed.
    let is_stale = (|| -> Result<bool> {
        if operation.phase == Phase::Idle || !task.is(&operation.task_id) || operation.target_zone_id != replacement_zone {
            return Ok(true);
        }
        let Some(task_staff) = &task_staff else {
            return Ok(true);
        };
        if task_staff.as_deref() != Some(&*previous_id) {
            return Ok(true);
        }
        let Some(previous_task) = &previous_task else {
            return Ok(true);
        };
        if !task.is(previous_task) {
            return Ok(true);
        }
        let Some((_, replacement_busy, _)) = &replacement else {
            return Ok(true);
        };
        if *replacement_busy || replacement_phase != Some(Phase::Idle) || !available(world, &replacement_id)? {
            return Ok(true);
        }
        if !dynamics_available(world, &replacement_id, now).map_err(|error| Error::refused("staff_dynamics.is_available", error))? {
            return Ok(true);
        }
        if dynamics_on_shift(world, &previous_id, now).map_err(|error| Error::refused("staff_dynamics.on_shift", error))? && available(world, &previous_id)? {
            return Ok(true);
        }
        match operation.due_minute {
            Some(due) => Ok(due <= now),
            None => Err(Error::unsupported("a due minute is not an int")),
        }
    })()?;
    if is_stale {
        return stale();
    }
    let Some(kind) = operation.task_kind.clone() else {
        return Err(Error::unsupported("a value is not a str"));
    };
    let Some(required) = relief_skills(&kind) else {
        return Err(Error::unsupported("an unknown kind of task"));
    };
    let skills = replacement.map(|(_, _, skills)| skills).unwrap_or_default();
    if !skills.iter().any(|skill| required.contains(&&**skill)) {
        return Err(Error::suppressed("replacement staff lacks the task skill"));
    }
    let (formula_source, formula_policy, event_source) = {
        let (source, policy, event_source) = inputs.formula()?.provenance();
        (Arc::from(source), Arc::from(policy), Arc::from(event_source))
    };
    let mut pending = Vec::new();
    let mut has_completion = false;
    for candidate in pending_rows(world)? {
        let completes = &*candidate.event_type == "staff_operation_complete";
        if (completes || &*candidate.event_type == "staff_operation_arrive") && names_task(&candidate, &task)? {
            has_completion |= completes;
            pending.push(candidate);
        }
    }
    if !has_completion {
        return Err(Error::failed("relieved staff operation lacks a pending completion"));
    }
    let work_minutes = with_config(world, inputs, |config| task_minutes(config, &kind))?;
    let causation = causation_with(&operation.causation_event_ids, std::slice::from_ref(&event.event_id));
    let (Some(task), Some(reason_code)) = (operation.task_id.clone(), kit::optional_text(payload("reason_code"))?.cloned()) else {
        return Err(Error::unsupported("a value is not a str"));
    };
    let due = now + work_minutes;
    let mut updated = operation;
    updated.phase = Phase::Working;
    updated.started_minute = Some(now);
    updated.travel_due_minute = None;
    updated.arrival_minute = Some(now);
    updated.due_minute = Some(due);
    updated.route_zone_ids = Vec::new();
    updated.route_edge_ids = Vec::new();
    updated.travel_minutes = 0;
    updated.causation_event_ids = causation;
    Ok(ReliefPlan {
        task_id: task,
        previous_id,
        replacement_id,
        reason_code,
        pending,
        updated,
        due,
        now,
        event_id: event.event_id.clone(),
        formula_source,
        formula_policy,
        event_source,
    })
}

/// The writes of `StaffOperationReliefHandler.apply`, and its handoff receipt, if the world holds
/// `shared_operations_v1`.
/// `StaffOperationReliefHandler.apply`: the plan (`plan_relief`), then its writes (lane P10-1c).
pub fn relief(world: &World, inputs: &Inputs<'_>, event: &EventRow, taken: &mut Vec<u64>) -> WriteResult<Option<Made>> {
    let plan = plan_relief(world, inputs, event)?;
    write_relief(world, inputs, plan, taken)
}

pub fn write_relief(world: &World, inputs: &Inputs<'_>, plan: ReliefPlan, taken: &mut Vec<u64>) -> WriteResult<Option<Made>> {
    let events = Events::of(world)?;
    let now = plan.now;
    let mut previous_position: Option<Position> = None;
    if inputs.movement_held() {
        let positions = snapshots(world, inputs, now)?;
        if let Some((_, position)) = positions.into_iter().find(|(id, _)| *id == plan.previous_id) {
            if let Some(staff) = world.rows_mut()?.staff.get_mut(&plan.previous_id) {
                staff.zone_id = position.zone_id.clone();
            }
            previous_position = Some(position);
        }
    }
    suppress(&events, &plan.pending, now, "staff operation was replaced by a qualified co-located worker", taken)?;
    {
        let mut rows = world.rows_mut()?;
        free_staff(&mut rows, &plan.previous_id)?;
        let replacement = rows.staff.get_mut(&plan.replacement_id).ok_or_else(|| gone("the staff member"))?;
        replacement.active_task_id = Some(plan.task_id.clone());
        replacement.work_queue.clear();
        replacement.work_queue.push(plan.task_id.clone());
        replacement.busy_until = plan.due;
        let task = rows.active_tasks.get_mut(&plan.task_id).ok_or_else(|| gone("the task"))?;
        task.staff_id = Some(plan.replacement_id.clone());
        task.start_minute = now;
        task.due_minute = plan.due;
    }
    let updated = plan.updated;
    let mut payload = IndexMap::with_capacity(5);
    payload.insert(Arc::from("staff_id"), V::Str(plan.replacement_id.clone()));
    payload.insert(Arc::from("task_id"), V::Str(plan.task_id.clone()));
    payload.insert(Arc::from("task_kind"), opt_text(&updated.task_kind));
    payload.insert(Arc::from("target_id"), opt_text(&updated.target_id));
    payload.insert(Arc::from("target_zone_id"), opt_text(&updated.target_zone_id));
    let mut provenance = kit::assumption(V::Str(plan.formula_source.clone()), &plan.formula_policy);
    provenance.insert(Arc::from("reason"), opt_text(&updated.reason));
    {
        let mut row = world.row_mut::<StaffOperationsRow>(NAME)?;
        row.staff.insert(plan.previous_id.clone(), idle_record());
        row.staff.insert(plan.replacement_id.clone(), updated);
    }
    let sequence = events.allocate_sequence()?;
    let completion = kit::new_event(kit::NewEvent {
        event_id: Arc::from(format!("staff-operation-complete-{sequence}")),
        kind: EventKind::Completion,
        due_minute: plan.due,
        priority: 5,
        sequence,
        event_type: Arc::from("staff_operation_complete"),
        payload,
        source_id: Some(plan.event_source.clone()),
        causation_id: Some(plan.event_id.clone()),
        provenance,
    })?;
    events.schedule(completion)?;
    if inputs.movement_held() {
        snapshots(world, inputs, now)?;
        if let Some(position) = previous_position {
            world.row_mut::<MovementRow>(MOVEMENT)?.staff_positions.insert(plan.previous_id.clone(), position);
        }
    }
    if inputs.shared_absent {
        return Ok(None);
    }
    let receipt = care_receipts::handoff_record(
        world,
        &plan.task_id,
        &plan.previous_id,
        &plan.replacement_id,
        &plan.reason_code,
        &plan.event_id,
        vec![Arc::from(RELIEF_SOURCE)],
    )?;
    Ok(Some(receipt))
}

// ---- Completion --------------------------------------------------------------------------------------

/// A number of a measured field: an int or a float.
#[derive(Clone, Copy, Debug)]
enum Number {
    Int(i64),
    Float(f64),
}

impl Number {
    /// `float(value)`.
    fn value(self) -> f64 {
        match self {
            Number::Int(number) => number as f64,
            Number::Float(number) => number,
        }
    }

    /// `!=` of two values of one field.
    fn differs(self, other: Number) -> bool {
        match (self, other) {
            (Number::Int(one), Number::Int(two)) => one != two,
            (one, two) => one.value() != two.value(),
        }
    }
}

/// One measurement of a staff operation: kind, entity, path, value, unit.
struct Measure {
    kind: &'static str,
    entity: Arc<str>,
    path: String,
    value: Number,
    unit: &'static str,
}

const BODY_MEASURES: [&str; 3] = ["uncertainty", "reassurance", "positive_affect"];

/// One person, as the rules of a zone read them: ID, key of the table, zone and trust.
struct Person {
    id: Arc<str>,
    key: Arc<str>,
    zone: Arc<str>,
    trust: f64,
}

/// The agents in the order of their IDs.
fn sorted_agents(rows: &Entities) -> Vec<Person> {
    let mut people: Vec<Person> = rows
        .agents
        .iter()
        .map(|(key, agent)| Person { id: agent.profile.id.clone(), key: key.clone(), zone: agent.zone_id.clone(), trust: agent.trust })
        .collect();
    people.sort_by(|first, second| first.id.cmp(&second.id));
    people
}

fn body_value(record: &BodyRecord, field: &str) -> f64 {
    match field {
        "uncertainty" => record.uncertainty,
        "reassurance" => record.reassurance,
        _ => record.positive_affect,
    }
}

/// The measurements of a staff operation, of the payload's kind, target and zone.
fn measurements(world: &World, inputs: &Inputs<'_>, kind: &str, target: Option<&Arc<str>>, zone: Option<&Arc<str>>) -> Result<Vec<Measure>> {
    let mut values = Vec::new();
    match (kind, target, zone) {
        ("cleaning", _, Some(zone)) => {
            if let Some(row) = ops_row(world, inputs)? {
                if let Some(record) = row.zones.get(&**zone) {
                    values.push(Measure {
                        kind: "model_state",
                        entity: zone.clone(),
                        path: "staff_operations_v1.zones.sanitation".to_owned(),
                        value: Number::Float(record.sanitation),
                        unit: "normalized_0_1",
                    });
                }
            }
        }
        ("maintenance", Some(target), _) => {
            if let Some(facility) = world.rows()?.facilities.get(&**target) {
                values.push(Measure {
                    kind: "facility",
                    entity: facility.id.clone(),
                    path: "working_units".to_owned(),
                    value: Number::Int(facility.working_units),
                    unit: "facility_unit",
                });
            }
        }
        ("information_outreach" | "patrol", _, Some(zone)) => {
            let body = kit::held_row::<BodyRow>(world, BODY, inputs.body)?;
            for person in sorted_agents(&*world.rows()?) {
                if person.zone != *zone {
                    continue;
                }
                values.push(Measure {
                    kind: "agent",
                    entity: person.id.clone(),
                    path: "trust".to_owned(),
                    value: Number::Float(person.trust),
                    unit: "normalized_0_1",
                });
                if let Some(record) = body.as_ref().and_then(|body| body.agents.get(&*person.id)) {
                    for field in BODY_MEASURES {
                        values.push(Measure {
                            kind: "model_state",
                            entity: person.id.clone(),
                            path: format!("body_state_v1.agents.{field}"),
                            value: Number::Float(body_value(record, field)),
                            unit: "normalized_0_1",
                        });
                    }
                }
            }
        }
        _ => {}
    }
    Ok(values)
}

/// The work of a completed task, planned.
pub enum Work {
    None,
    Cleaning {
        zone: Arc<str>,
        sanitation: f64,
    },
    Repair {
        facility: Arc<str>,
        units: i64,
    },
    Blocked,
    /// Each person by the key of the table: the next uncertainty, reassurance and positive affect
    /// of a body record, and the next trust.
    People {
        people: Vec<(Arc<str>, Arc<str>, Option<[f64; 3]>, f64)>,
    },
    /// The admission of the arrivals (`population::check_waiting`, `population::admit_arrivals`),
    /// which the caller checks and runs before `write_complete`.
    Intake,
}

/// What a completion writes.
pub struct CompletePlan {
    staff_id: Arc<str>,
    task_id: Arc<str>,
    entry: FinishedTask,
    pub work: Work,
    before: Vec<Measure>,
    previous_units: Option<(Arc<str>, i64)>,
    event_id: Arc<str>,
    event_source: Option<Arc<str>>,
    /// The payload's `task_kind`, and its `target_id` and `target_zone_id` where they are str.
    kind: Arc<str>,
    target: Option<Arc<str>>,
    zone: Option<Arc<str>>,
}

/// `config["effects_by_zone"].get(zone_id, {}).get(name, config[name])`, as a float.
fn effect(config: Option<Entries<'_>>, zone: &str, name: &str) -> Result<f64> {
    let Some(config) = config else {
        return Err(Error::unsupported("the config is not a map"));
    };
    let by_zone = match config.table("effects_by_zone") {
        Table::Map(by_zone) => by_zone,
        Table::Missing => return Err(Error::Key("effects_by_zone".to_owned())),
        Table::Other => return Err(Error::unsupported("a value is not a plain dict")),
    };
    let Some(default) = config.number(name) else {
        return Err(Error::Key(name.to_owned()));
    };
    let value = match by_zone.table(zone) {
        Table::Missing => default,
        Table::Map(zone_effects) => zone_effects.number(name).unwrap_or(default),
        Table::Other => return Err(Error::unsupported("a value is not a plain dict")),
    };
    match value {
        ConfigNumber::Float(number) => Ok(number),
        ConfigNumber::Int(number) if (-(1 << 53)..=1 << 53).contains(&number) => Ok(number as f64),
        ConfigNumber::Int(_) => Err(Error::unsupported("a number does not fit a float")),
        ConfigNumber::Big | ConfigNumber::Other => Err(Error::unsupported("a value is not an int")),
    }
}

/// The checks and the plan of `StaffOperationCompletionHandler.apply`, in its order. An intake's
/// admission is the caller's to check (`check_waiting_for_admission`) once this says `Work::Intake`.
pub fn plan_complete(world: &World, inputs: &Inputs<'_>, event: &EventRow) -> Result<Planned<CompletePlan>> {
    if !matches!(event.payload, V::Map(_)) {
        return Err(Error::unsupported("a payload is not a plain dict"));
    }
    let payload = |name: &str| kit::payload(event, name).cloned().unwrap_or(V::Null);
    let kind_text = kit::exact_str(kit::payload(event, "task_kind").unwrap_or(&V::Null))?.clone();
    let payload_target = PayloadText::of(event, "target_id").text().cloned();
    let payload_zone = PayloadText::of(event, "target_zone_id");
    let previous_units = match &payload_target {
        Some(target) if &*kind_text == "maintenance" => world.rows()?.facilities.get(&**target).map(|facility| (facility.id.clone(), facility.working_units)),
        _ => None,
    };
    let before = measurements(world, inputs, &kind_text, payload_target.as_ref(), payload_zone.text())?;
    if &*event.event_type != "staff_operation_complete" {
        return Err(Error::failed("staff operation completion handler received another event"));
    }
    let not_active = || Err(Error::failed("staff operation completion does not match active work"));
    let (V::Str(id), V::Str(task)) = (payload("staff_id"), payload("task_id")) else {
        return not_active();
    };
    let Some(row) = ops_row(world, inputs)? else {
        return not_active();
    };
    let record = row.staff.get(&*id).cloned();
    let sanitation = record.as_ref().and_then(|record| record.target_zone_id.as_deref()).and_then(|zone| row.zones.get(zone)).map(|zone| zone.sanitation);
    drop(row);
    let (staff_task, has_task) = {
        let rows = world.rows()?;
        (rows.staff.get(&*id).map(|staff| staff.active_task_id.clone()), rows.active_tasks.contains_key(&*task))
    };
    let (Some(record), Some(staff_task)) = (record, staff_task) else {
        return not_active();
    };
    if record.phase != Phase::Working
        || record.task_id.as_ref() != Some(&task)
        || !has_task
        || staff_task.as_ref() != Some(&task)
        || record.due_minute != Some(event.due_minute)
        || record.task_kind.as_deref() != Some(&*kind_text)
        || !payload_zone.is(&record.target_zone_id)
    {
        return not_active();
    }
    if !available(world, &id)? {
        return Ok(Planned::Unavailable(id));
    }
    let (Some(kind), Some(zone_id), Some(reason), Some(started)) =
        (record.task_kind.clone(), record.target_zone_id.clone(), record.reason.clone(), record.started_minute)
    else {
        return Err(Error::unsupported("an active record lacks a field of the history"));
    };
    let target_id = record.target_id.clone();
    let target_facility = match target_id.as_deref() {
        Some(target) => world.rows()?.facilities.get(target).map(|facility| (facility.id.clone(), facility.working_units, facility.capacity)),
        None => None,
    };
    let work = match (&*kind, target_facility) {
        ("cleaning", _) => {
            let Some(sanitation) = sanitation else {
                return Err(Error::Key(zone_id.to_string()));
            };
            Work::Cleaning {
                zone: zone_id.clone(),
                sanitation: py_min(1.0, sanitation + with_config(world, inputs, |config| effect(config, &zone_id, "cleaning_sanitation_recovery"))?),
            }
        }
        ("maintenance", Some((facility, units, capacity))) => {
            // Whether maintenance is blocked: at capacity, then the outage, then the modifiers.
            let blocked = units >= capacity
                || kit::active_outage(world.slots.try_value(INFRASTRUCTURE)?.as_deref(), &facility)?
                || kit::has_active_modifier(kit::held_row::<EffectProgramRow>(world, PROGRAM, inputs.program)?.as_deref(), &facility);
            if blocked {
                Work::Blocked
            } else {
                Work::Repair { facility, units: units + 1 }
            }
        }
        ("information_outreach" | "patrol", _) => {
            let name = if &*kind == "information_outreach" { "outreach_uncertainty_relief" } else { "patrol_reassurance" };
            let relief = with_config(world, inputs, |config| effect(config, &zone_id, name))?;
            let body = kit::held_row::<BodyRow>(world, BODY, inputs.body)?;
            let mut people = Vec::new();
            for person in sorted_agents(&*world.rows()?) {
                if person.zone != zone_id {
                    continue;
                }
                let next_body = body.as_ref().and_then(|body| body.agents.get(&*person.id)).map(|record| {
                    [
                        py_max(0.0, record.uncertainty - relief),
                        py_min(1.0, record.reassurance + relief * 0.5),
                        py_min(1.0, record.positive_affect + relief * 0.25),
                    ]
                });
                people.push((person.key, person.id, next_body, py_min(1.0, person.trust + relief * 0.1)));
            }
            Work::People { people }
        }
        ("intake", _) => Work::Intake,
        _ => Work::None,
    };
    let entry = FinishedTask {
        task_id: task.clone(),
        staff_id: id.clone(),
        task_kind: kind,
        target_id,
        target_zone_id: zone_id,
        reason,
        status: Outcome::Completed,
        start_minute: started,
        arrival_minute: record.arrival_minute.or(Some(started)),
        complete_minute: event.due_minute,
        travel_minutes: record.travel_minutes,
        causation_event_ids: causation_with(&record.causation_event_ids, std::slice::from_ref(&event.event_id)),
    };
    Ok(Planned::Go(CompletePlan {
        staff_id: id,
        task_id: task,
        entry,
        work,
        before,
        previous_units,
        event_id: event.event_id.clone(),
        event_source: event.source_id.clone(),
        kind: kind_text,
        target: payload_target,
        zone: payload_zone.text().cloned(),
    }))
}

/// The writes of a staff operation's completion (an intake's admission is the caller's, before
/// this), and the handler's receipts: the operation's, and a facility recovery when a repair
/// changed its working units, where the world holds `shared_operations_v1`. What the plan of a
/// completion led to (`complete_or_intake`).
pub enum Completed {
    /// Written: the receipts.
    Done(Vec<Made>),
    /// The staff member became unavailable before completing the task.
    Unavailable(Arc<str>),
    /// An intake, written once the admission ran (`write_complete`).
    Intake(Box<CompletePlan>),
}

/// A staff operation's completion: the plan (`plan_complete`), then its writes, but for an intake,
/// whose admission runs first (lane P10-1c).
pub fn complete_or_intake(world: &World, inputs: &Inputs<'_>, event: &EventRow) -> Result<Completed> {
    match plan_complete(world, inputs, event)? {
        Planned::Unavailable(id) => Ok(Completed::Unavailable(id)),
        Planned::Go(plan) if matches!(plan.work, Work::Intake) => Ok(Completed::Intake(Box::new(plan))),
        Planned::Go(plan) => write_complete(world, inputs, plan).map(Completed::Done),
    }
}

pub fn write_complete(world: &World, inputs: &Inputs<'_>, plan: CompletePlan) -> Result<Vec<Made>> {
    let CompletePlan { staff_id, task_id, mut entry, work, before, previous_units, event_id, event_source, kind, target, zone } = plan;
    match work {
        Work::None | Work::Intake => {}
        Work::Blocked => entry.status = Outcome::Blocked,
        Work::Cleaning { zone, sanitation } => {
            let mut row = world.row_mut::<StaffOperationsRow>(NAME)?;
            row.zones.get_mut(&zone).ok_or_else(|| gone("the zone"))?.sanitation = sanitation;
        }
        Work::Repair { facility, units } => {
            if let Some(found) = world.rows_mut()?.facilities.get_mut(&facility) {
                found.working_units = units;
            }
            let held = matches!(world.slots.try_value(INFRASTRUCTURE)?.as_deref(), Some(V::Map(_)));
            if held {
                let mut row = world.row_mut::<ValueRow>(INFRASTRUCTURE)?;
                let V::Map(state) = &mut row.0 else {
                    return Err(gone("the infrastructure"));
                };
                match state.get_mut("base_working_units") {
                    Some(V::Map(base)) => {
                        base.insert(facility, V::Int(units));
                    }
                    Some(_) => return Err(Error::unsupported("the base working units are not a plain dict")),
                    None => return Err(Error::Key("base_working_units".to_owned())),
                }
            }
        }
        Work::People { people } => {
            let mut body = match inputs.body {
                Held::Held if people.iter().any(|(_, _, body, _)| body.is_some()) => Some(world.row_mut::<BodyRow>(BODY)?),
                _ => None,
            };
            let mut rows = world.rows_mut()?;
            for (key, id, next_body, trust) in people {
                if let (Some([uncertainty, reassurance, positive]), Some(body)) = (next_body, body.as_mut()) {
                    if let Some(record) = body.agents.get_mut(&*id) {
                        record.uncertainty = uncertainty;
                        record.reassurance = reassurance;
                        record.positive_affect = positive;
                    }
                }
                if let Some(agent) = rows.agents.get_mut(&key) {
                    agent.trust = trust;
                }
            }
        }
    }
    let outcome = OperationOutcome {
        task_id: task_id.clone(),
        staff_id: staff_id.clone(),
        task_kind: entry.task_kind.clone(),
        target_id: entry.target_id.clone(),
        target_zone_id: entry.target_zone_id.clone(),
        reason: entry.reason.clone(),
        status: entry.status,
        start_minute: entry.start_minute,
        arrival_minute: entry.arrival_minute,
        travel_minutes: entry.travel_minutes,
        changes: Vec::new(),
        measured: !before.is_empty(),
        causation: event_id.clone(),
    };
    append_history(&mut *world.row_mut::<StaffOperationsRow>(NAME)?, entry);
    {
        let mut rows = world.rows_mut()?;
        rows.active_tasks.remove_row(&task_id);
        free_staff(&mut rows, &staff_id)?;
    }
    set_record(world, &staff_id, idle_record())?;

    // The handler's receipts.
    let mut after = measurements(world, inputs, &kind, target.as_ref(), zone.as_ref())?;
    after.sort_by(|first, second| (first.kind, &first.entity, &first.path).cmp(&(second.kind, &second.entity, &second.path)));
    let mut changes = Vec::new();
    for measure in &after {
        let Some(previous) = before.iter().find(|known| known.kind == measure.kind && known.entity == measure.entity && known.path == measure.path) else {
            continue;
        };
        if previous.value.differs(measure.value) {
            changes.push(Change {
                entity_id: measure.entity.clone(),
                entity_kind: Arc::from(measure.kind),
                field_path: Arc::from(measure.path.as_str()),
                before: previous.value.value(),
                after: measure.value.value(),
                unit: Arc::from(measure.unit),
            });
        }
    }
    let mut receipts = Vec::new();
    if !inputs.shared_absent {
        receipts.push(care_receipts::staff_operation_record(world, OperationOutcome { changes, ..outcome })?);
    }
    if let Some((facility, previous)) = previous_units {
        let Some(units) = world.rows()?.facilities.get(&facility).map(|found| found.working_units) else {
            return Err(gone("the facility"));
        };
        if units != previous && !inputs.shared_absent {
            let sources = match event_source {
                Some(source) if !source.is_empty() => vec![source],
                _ => Vec::new(),
            };
            receipts.push(care_receipts::facility_recovery_record(
                world,
                care_receipts::Recovery { facility_id: &facility, component_id: &task_id, status: "restored", previous, units },
                &event_id,
                sources,
            )?);
        }
    }
    Ok(receipts)
}

// ---- The trips on closed routes -----------------------------------------------------------------

/// Whether an applied `effect_program_geometry` closes a route, so that the trips on closed routes
/// stop (`interrupt_trips`): its payload's `op` is `set_route_access`, and its `parameters`
/// (`payload.get("parameters", {})`) are a mapping whose `accessible` is the bool False.
pub fn closes_a_route(event: &EventRow) -> bool {
    let op = matches!(kit::payload(event, "op"), Some(V::Str(op)) if &**op == "set_route_access");
    let closed = match kit::payload(event, "parameters") {
        None => false,
        Some(V::Map(parameters)) => matches!(parameters.get("accessible"), Some(V::Bool(false))),
        Some(_) => false,
    };
    op && closed
}

/// The trips on closed routes stopped: each travelling member whose route has an edge that is gone,
/// closed, or not clear for an independent walker on the floor plan is stopped, in ID order. The
/// members it stopped.
pub fn interrupt_trips(world: &World, inputs: &Inputs<'_>, event: &Stopping<'_>, taken: &mut Vec<u64>) -> WriteResult<Vec<Arc<str>>> {
    let mut interrupted = Vec::new();
    let travelling: Vec<(Arc<str>, Vec<Arc<str>>)> = {
        let Some(row) = ops_row(world, inputs)? else {
            return Ok(interrupted);
        };
        inputs.plan()?;
        let mut records: Vec<(&Arc<str>, &OperationRecord)> = row.staff.iter().collect();
        records.sort_unstable_by(|first, second| first.0.cmp(second.0));
        records.into_iter().filter(|(_, record)| record.phase == Phase::Travel).map(|(id, record)| (id.clone(), record.route_edge_ids.clone())).collect()
    };
    let geo = inputs.plan()?.clone();
    for (staff_id, route) in travelling {
        let blocked = {
            let rows = world.entities.try_borrow()?;
            if rows.zones_tainted().is_some() && !route.is_empty() {
                return Err(Error::unsupported(ZONES_TAINTED).into());
            }
            route.iter().any(|edge_id| match rows.zone_edges.get(edge_id) {
                None => true,
                Some(edge) => edge.closed || !geo.route_is_clear(edge_id, "independent"),
            })
        };
        if blocked && interrupt(world, inputs, &staff_id, event, Some(event.event_id), taken)? {
            interrupted.push(staff_id);
        }
    }
    if !interrupted.is_empty() && inputs.movement_held() {
        snapshots(world, inputs, event.due_minute)?;
    }
    Ok(interrupted)
}
