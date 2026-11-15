//! `ToiletServiceManager` and `ToiletServiceCompletionHandler`: a person queues (`queue_up`,
//! `request`), the head starts a service of a drawn length (`start`), and a completion lets the
//! person go (`complete`). Each kernel plans with nothing changed and then writes, on the world's
//! rows alone.
//!
//! The caller reads first what the kernels take besides the rows (the manager's durations and
//! policy, the random source's seed, whether the world holds each namespace, the floor plan, an
//! answer of a tainted `staff_operations_v1`). The world's whole validation (`validate_state`) and
//! the draw of a random source that is not a `CounterRNG` (a test's) go through the `Host`. A
//! completion's walk home (`after_complete`): `complete` says the walk is due, and the dispatch
//! runs it (`toilet_completion` of `rules::dispatch`).

use std::sync::Arc;

use indexmap::IndexMap;
use sha2::{Digest, Sha256};

use crate::config::population::AccessibilityNeeds;
use crate::entities::{Activity, TaskRow};
use crate::error::{Error, Result};
use crate::events::writer::WriteResult;
use crate::events::{EventKind, EventRow};
use crate::geometry::Geo;
use crate::host::{Host, Part};
use crate::mathx::py_max;
use crate::rng::{label, Key, KeyedDraws};
use crate::rules::facility_queues::{self as fifo, point_of};
use crate::rules::kit::{add_metric, check_metric_state, held_row, is_toilet, set_agent, Events, Held, Kept};
use crate::rules::queues::{self as queues, Walk};
use crate::rules::service::{
    choose_queue, finishing_request, first_available, payload_text, plain, record_wait, release_staff, remove_from, remove_task, service_capacity,
    transition_row,
};
use crate::state::body_state::{BodyRow, NAME as BODY_STATE};
use crate::state::facility_queues::QueueRequest;
use crate::text::py_repr;
use crate::value::V;
use crate::world::{World, WorldCell};

/// `ToiletServiceCompletionHandler`: what a completed visit takes off the hygiene need.
pub const HYGIENE_RELIEF: f64 = 0.2;
const COMPLETED_METRIC: &str = "toilet_service_completed_count";
const DURATION_SOURCE: &str = "ASM-TOILET-SERVICE-DURATION-V1";
const NO_QUEUE: &str = "no accessible independent toilet service queue is available";

/// `sha256(f"{facility}\0{agent}\0{minute}")`, its first 24 hex digits.
pub fn queue_task_id(facility_id: &str, agent_id: &str, minute: i64) -> Arc<str> {
    let mut hash = Sha256::new();
    hash.update(facility_id.as_bytes());
    hash.update([0]);
    hash.update(agent_id.as_bytes());
    hash.update([0]);
    hash.update(minute.to_string().as_bytes());
    let digest = hash.finalize();
    let mut text = String::with_capacity(12 + 24);
    text.push_str("toilet-queue-");
    for byte in &digest[..12] {
        text.push(char::from_digit(u32::from(byte >> 4), 16).expect("a digit"));
        text.push(char::from_digit(u32::from(byte & 15), 16).expect("a digit"));
    }
    Arc::from(text)
}

// ---- request -------------------------------------------------------------------------------------

/// What `request` takes besides the world.
pub struct Request<'a> {
    pub agent_id: &'a str,
    pub facility_id: &'a str,
    /// The given `queue_id`, unless None.
    pub queue_id: Option<&'a str>,
    /// Whether the world holds `facility_queues_v1`.
    pub queues: &'a Held,
    /// The floor plan (`Geo::of`).
    pub geo: &'a Geo,
    /// The population's accessibility needs (`choose_queue_id`'s), as the caller read them.
    pub needs: &'a Kept<AccessibilityNeeds>,
}

/// `ToiletRequestReceipt`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receipt {
    pub agent_id: Arc<str>,
    pub facility_id: Arc<str>,
    pub requested_at: i64,
    pub queue_task_id: Arc<str>,
    pub queue_position: i64,
}

/// `request` without the world's validation: the receipt. A refused request at the end of a walk is
/// saved as the event's detail (`_after_arrival`), so the checks keep a fixed order and their
/// texts.
pub fn queue_up(world: &World, request: &Request<'_>) -> WriteResult<Receipt> {
    let (agent_name, facility_name) = (request.agent_id, request.facility_id);
    let minute = world.minute_index()?;
    let task_id = queue_task_id(facility_name, agent_name, minute);
    {
        let rows = world.rows()?;
        let Some(agent) = rows.agents.get(agent_name) else {
            return Err(Error::value_error(format!("unknown Agent {}", py_repr(agent_name))).into());
        };
        if !rows.facilities.get(facility_name).is_some_and(|facility| is_toilet(&facility.kind)) {
            return Err(Error::value_error(format!("{} is not a toilet Facility", py_repr(facility_name))).into());
        }
        if !matches!(agent.activity, Activity::Idle | Activity::Resting | Activity::Sleeping) {
            return Err(Error::value_error(format!("Agent {} cannot request a toilet while active", py_repr(agent_name))).into());
        }
        for (_, other) in rows.facilities.iter() {
            if other.queue_agent_ids.iter().chain(&other.active_agent_ids).any(|id| &**id == agent_name) {
                return Err(Error::value_error(format!("Agent {} already has an active toilet request", py_repr(agent_name))).into());
            }
        }
        if rows.active_tasks.get(&task_id).is_some() {
            return Err(Error::invariant(format!("duplicate toilet queue task ID {}", py_repr(&task_id))).into());
        }
    }
    let independent = fifo::queues(world, request.queues)?.is_some_and(|row| fifo::has_independent(&row, facility_name));
    if request.queue_id.is_some() && !independent {
        return Err(Error::unsupported("a queue ID of a facility without independent queues").into());
    }
    let chosen_queue: Option<Arc<str>> = match (independent, request.queue_id) {
        (false, _) => None,
        (true, Some(queue)) if !queue.is_empty() => Some(Arc::from(queue)),
        (true, _) => choose_queue(world, request.queues, request.geo, facility_name, agent_name, request.needs)?,
    };
    if independent && chosen_queue.is_none() {
        return Err(Error::value_error(NO_QUEUE).into());
    }
    let point = match &chosen_queue {
        Some(queue) => match point_of(request.geo, queue) {
            Some(point) if &*point.facility_id == facility_name => Some(point),
            _ => return Err(Error::value_error(NO_QUEUE).into()),
        },
        None => None,
    };
    let queue_tail = match &chosen_queue {
        Some(queue) => match fifo::queues(world, request.queues)? {
            Some(row) => match row.queues.get(&**queue) {
                Some(queue) => Some(queue.queued_request_ids.len() as i64 + 1),
                None => return Err(Error::unsupported("no such queue").into()),
            },
            None => None,
        },
        None => None,
    };
    let position = {
        let rows = world.rows()?;
        let Some(facility) = rows.facilities.get(facility_name) else {
            return Err(Error::unsupported("no such agent or facility").into());
        };
        queue_tail.unwrap_or(facility.queue_agent_ids.len() as i64 + 1)
    };
    let events = Events::of(world)?;
    let transition = match point {
        Some(point) => {
            let known = fifo::queues(world, request.queues)?.map(|row| row.requests.contains_key(&*task_id));
            if known != Some(false) {
                return Err(Error::unsupported("the request's ID is in use").into());
            }
            let queued = QueueRequest {
                request_id: task_id.clone(),
                queue_id: point.queue_id.clone(),
                service_point_id: point.id.clone(),
                facility_id: point.facility_id.clone(),
                agent_id: Arc::from(agent_name),
                service_kind: Arc::from("toilet"),
                requested_minute: minute,
                status: Arc::from("queued"),
                queue_task_id: task_id.clone(),
                active_task_id: task_id.clone(),
            };
            let sequence = events.next_sequence()?;
            Some(transition_row(
                &events,
                &fifo::Transition { request: &queued, transition: "queued", task_id: &task_id, wait_minutes: None, causation_id: None },
                fifo::Place { queue_position: Some(position), capacity: Some(point.service_capacity) },
                sequence,
                minute,
            )?)
        }
        None => None,
    };

    // Write.
    let agent_arc: Arc<str> = Arc::from(agent_name);
    let facility_arc: Arc<str> = Arc::from(facility_name);
    {
        let mut rows = world.rows_mut()?;
        let Some(facility) = rows.facilities.get_mut(facility_name) else {
            return Err(Error::unsupported("no such facility").into());
        };
        facility.queue_agent_ids.push(agent_arc.clone());
    }
    if let Some(point) = point {
        fifo::write_enqueue(
            world,
            point,
            fifo::NewRequest { request_id: &task_id, task_id: &task_id, agent_id: agent_name, service_kind: "toilet", requested_minute: minute },
        )?;
    }
    {
        let mut rows = world.rows_mut()?;
        rows.active_tasks.insert(
            task_id.clone(),
            TaskRow {
                id: task_id.clone(),
                kind: Arc::from("toilet_queue"),
                agent_id: Some(agent_arc.clone()),
                resource_id: Some(facility_arc.clone()),
                start_minute: minute,
                due_minute: minute,
                staff_id: None,
            },
        );
        set_agent(&mut rows, agent_name, Activity::Queued, Some(facility_name))?;
    }
    if let Some(transition) = &transition {
        fifo::write_transition(&events, transition, minute)?;
    }
    Ok(Receipt { agent_id: agent_arc, facility_id: facility_arc, requested_at: minute, queue_task_id: task_id, queue_position: position })
}

/// `ToiletServiceManager.request`: the world's validation before and after (`validates`), through
/// the host, with the world let go of around each call.
pub fn request<C: WorldCell, H: Host>(cell: &C, host: &mut H, request: &Request<'_>, validates: bool) -> std::result::Result<Receipt, H::Error> {
    if validates {
        host.validate_world()?;
    }
    let receipt = cell.with(|world| queue_up(world, request))?.map_err(plain)?;
    if validates {
        host.validate_world()?;
    }
    Ok(receipt)
}

// ---- arbitrate -----------------------------------------------------------------------------------

/// `arbitrate`'s facilities with anyone queued, in ID order: those whose folded kind has "toilet",
/// with someone in their own list or in one of their independent queues.
pub fn facilities_to_serve(world: &World, queues: &Held) -> Result<Vec<Arc<str>>> {
    let waiting: Vec<Arc<str>> = match fifo::queues(world, queues)? {
        Some(row) => row.queues.values().filter(|queue| !queue.queued_request_ids.is_empty()).map(|queue| queue.facility_id.clone()).collect(),
        None => Vec::new(),
    };
    let rows = world.rows()?;
    let mut due: Vec<Arc<str>> = rows
        .facilities
        .iter()
        .filter(|(id, facility)| is_toilet(&facility.kind) && (!facility.queue_agent_ids.is_empty() || waiting.contains(id)))
        .map(|(id, _)| id.clone())
        .collect();
    due.sort_unstable();
    Ok(due)
}

/// `arbitrate`'s condition of a shared queue: someone in it and a free place.
pub fn has_room(world: &World, queues: &Held, placement: Option<&Geo>, facility_id: &str) -> Result<bool> {
    if fifo::queues(world, queues)?.is_some_and(|row| fifo::has_independent(&row, facility_id)) {
        return Err(Error::unsupported("a facility with independent queues"));
    }
    let rows = world.rows()?;
    let Some(facility) = rows.facilities.get(facility_id) else {
        return Err(Error::unsupported("no such facility"));
    };
    if facility.queue_agent_ids.is_empty() {
        return Ok(false);
    }
    Ok((facility.active_agent_ids.len() as i64) < service_capacity(placement, facility_id, facility.working_units))
}

/// The random source of the tick: a `CounterRNG` of this seed (`str(seed)`), drawn here, or another
/// (a test's), which the host draws.
pub enum Rng {
    Seed(String),
    Host,
}

/// What a pass of `arbitrate` takes besides the world.
pub struct Start<'a> {
    pub facility_id: &'a str,
    pub minute: i64,
    /// The independent queue of the pass, or None for the facility's shared queue.
    pub queue_id: Option<&'a str>,
    /// `min_service_minutes`, `max_service_minutes` and `policy_version` of the manager.
    pub lowest: i64,
    pub highest: i64,
    pub policy: Arc<str>,
    pub rng: &'a Rng,
    /// Whether the world holds `facility_queues_v1`.
    pub queues: &'a Held,
    pub geo: &'a Geo,
    /// The walk of `progress_physical_queue_head_to_service`, where the world is placed.
    pub walk: Option<&'a Walk<'a>>,
    /// Whether `world.metric_state` is its slot's tree.
    pub metric_state_held: bool,
}

/// What a pass did.
#[derive(Debug, PartialEq)]
pub enum Started {
    /// The head's service started: the completion's ID.
    Event(Arc<str>),
    /// Nothing started: the queue waits (what stopped an independent queue is marked on it).
    Stopped,
}

/// What a pass asks before it draws.
enum Pass {
    Done(Started),
    /// The parts of the draw's key, for a random source the host draws.
    Draw(Vec<Part>),
}

/// The head of an independent queue, read when a pass starts.
struct IndependentHead {
    queue_id: Arc<str>,
    request: QueueRequest,
    active_count: i64,
    point_capacity: i64,
    required_skills: Vec<Arc<str>>,
}

/// `_start_head` and `_serve_independent_head`: the host draws for a random source that is not a
/// `CounterRNG`, with the world let go of, and the pass then plans again from the start.
pub fn start<C: WorldCell, H: Host>(cell: &C, host: &mut H, start: &Start<'_>) -> std::result::Result<Started, H::Error> {
    match cell.with(|world| pass(world, start, None))?.map_err(plain)? {
        Pass::Done(started) => Ok(started),
        Pass::Draw(parts) => {
            let draw = host.draw(&parts)?;
            match cell.with(|world| pass(world, start, Some(draw)))?.map_err(plain)? {
                Pass::Done(started) => Ok(started),
                Pass::Draw(_) => Err(Error::Runtime("the pass asked for its draw again".to_owned()).into()),
            }
        }
    }
}

/// One pass, with the draw the host made, if any.
fn pass(world: &World, start: &Start<'_>, drawn: Option<f64>) -> WriteResult<Pass> {
    let facility_name = start.facility_id;
    let minute = start.minute;
    if start.queue_id.is_none() && fifo::queues(world, start.queues)?.is_some_and(|row| fifo::has_independent(&row, facility_name)) {
        return Err(Error::unsupported("a facility with independent queues").into());
    }
    if minute != world.minute_index()? {
        return Err(Error::unsupported("another minute").into());
    }
    let head = independent_head(world, start)?;
    if let Some(head) = &head {
        let (active, limit) = facility_load(world, facility_name)?;
        if head.active_count >= head.point_capacity || active >= limit {
            fifo::write_stall(world, &head.queue_id, &head.request.request_id, "capacity")?;
            return Ok(Pass::Done(Started::Stopped));
        }
        // Writes only what ends this pass.
        if let Some(walk) = start.walk {
            if queues::head_to_service(world, walk, facility_name, false, minute, Some(&head.queue_id))? {
                return Ok(Pass::Done(Started::Stopped));
            }
        }
    }
    let (agent_id, queue_task_id, queue_start) = queue_task(world, facility_name, &head)?;
    let wait = minute.checked_sub(queue_start).ok_or_else(|| Error::unsupported("a start out of range"))?;
    if !(0..1 << 53).contains(&wait) {
        return Err(Error::unsupported("a wait for the Python rule to judge").into());
    }
    // The draw and its key, as `rng_key` holds them.
    let draw = match draw_of(start, drawn, &agent_id, &head)? {
        Ok(draw) => draw,
        Err(parts) => return Ok(Pass::Draw(parts)),
    };
    let (lowest, highest) = (start.lowest, start.highest);
    let span = highest - lowest + 1;
    let service_minutes = (lowest + (draw * span as f64) as i64).min(highest);
    let Some(due) = minute.checked_add(service_minutes) else {
        return Err(Error::unsupported("a due minute out of range").into());
    };
    let events = Events::of(world)?;
    if due >= events.duration() {
        if let Some(head) = &head {
            fifo::write_stall(world, &head.queue_id, &head.request.request_id, "run_end")?;
        }
        return Ok(Pass::Done(Started::Stopped));
    }
    // The first free skilled person in ID order, where the point asks for skills.
    let mut staff_id: Option<Arc<str>> = None;
    if let Some(head) = head.as_ref().filter(|head| !head.required_skills.is_empty()) {
        let candidates = skilled_staff(world, facility_name, head)?;
        match first_available(world, candidates, minute)? {
            Some(chosen) => staff_id = Some(chosen),
            None => {
                fifo::write_stall(world, &head.queue_id, &head.request.request_id, "staff")?;
                return Ok(Pass::Done(Started::Stopped));
            }
        }
    }
    let sequence = events.next_sequence()?;
    let event_id: Arc<str> = Arc::from(format!("toilet-complete-{sequence}"));
    let task_id: Arc<str> = Arc::from(format!("toilet-service-{sequence}"));
    identity_is_free(world, &events, &event_id, &task_id, sequence)?;
    let facility_arc: Arc<str> = Arc::from(facility_name);
    let text = |value: &str| V::Str(Arc::from(value));
    let payload = completion_payload(&facility_arc, &agent_id, &task_id, head.is_some(), &staff_id);
    let mut provenance = IndexMap::with_capacity(13);
    provenance.insert(Arc::from("source_type"), text("assumption"));
    provenance.insert(Arc::from("source_id"), text(DURATION_SOURCE));
    provenance.insert(Arc::from("policy_version"), V::Str(start.policy.clone()));
    provenance.insert(Arc::from("duration_distribution"), text("discrete_uniform_inclusive"));
    provenance.insert(Arc::from("minimum_minutes"), V::Int(lowest));
    provenance.insert(Arc::from("maximum_minutes"), V::Int(highest));
    provenance.insert(Arc::from("service_minutes"), V::Int(service_minutes));
    provenance.insert(Arc::from("queue_wait_minutes"), V::Int(wait));
    provenance.insert(Arc::from("requested_at"), V::Int(queue_start));
    provenance.insert(Arc::from("service_started_at"), V::Int(minute));
    provenance.insert(Arc::from("rng_key"), rng_key(start, &facility_arc, &agent_id, &head));
    provenance.insert(Arc::from("rng_draw"), V::Float(draw));
    if let Some(head) = &head {
        provenance.insert(Arc::from("queue_id"), V::Str(head.queue_id.clone()));
    }
    let Some(event) = EventRow {
        event_id: event_id.clone(),
        kind: EventKind::Completion,
        due_minute: due,
        priority: 0,
        sequence,
        event_type: Arc::from("toilet_service_complete"),
        payload: V::Map(payload),
        source_id: Some(task_id.clone()),
        causation_id: Some(queue_task_id.clone()),
        provenance: V::Map(provenance),
    }
    .checked() else {
        return Err(Error::unsupported("the event's values are not what an event takes").into());
    };
    check_metric_state(world, start.metric_state_held)?;
    let transition = match &head {
        Some(head) => Some(transition_row(
            &events,
            &fifo::Transition {
                request: &head.request,
                transition: "service_started",
                task_id: &task_id,
                wait_minutes: Some(wait),
                causation_id: Some(&event_id),
            },
            fifo::Place { queue_position: None, capacity: Some(head.point_capacity) },
            sequence + 1,
            minute,
        )?),
        None => None,
    };

    // Write.
    events.allocate_sequence()?;
    queued_to_active(world, facility_name, &agent_id, head.is_some())?;
    remove_task(world, &queue_task_id)?;
    {
        let mut rows = world.rows_mut()?;
        rows.active_tasks.insert(
            task_id.clone(),
            TaskRow {
                id: task_id.clone(),
                kind: Arc::from("toilet_service"),
                agent_id: Some(agent_id.clone()),
                resource_id: Some(facility_arc.clone()),
                start_minute: minute,
                due_minute: due,
                staff_id: staff_id.clone(),
            },
        );
        if let Some(staff_id) = &staff_id {
            let Some(row) = rows.staff.get_mut(staff_id) else {
                return Err(Error::unsupported("no such staff").into());
            };
            row.active_task_id = Some(task_id.clone());
            row.busy_until = due;
            row.work_queue = vec![task_id.clone()];
        }
        set_agent(&mut rows, &agent_id, Activity::InService, Some(facility_name))?;
    }
    events.schedule(event)?;
    if let (Some(head), Some(transition)) = (&head, &transition) {
        fifo::write_start(world, &head.request.request_id, &head.queue_id, &task_id)?;
        fifo::write_transition(&events, transition, minute)?;
    }
    record_wait(world, "toilet_wait", wait)?;
    Ok(Pass::Done(Started::Event(event_id)))
}

/// The head of the signed point's queue a pass serves, with its point's capacity and skills; None
/// for the facility's shared queue.
fn independent_head(world: &World, start: &Start<'_>) -> Result<Option<IndependentHead>> {
    let facility_name = start.facility_id;
    let head = match start.queue_id {
        Some(queue_name) => {
            let Some(point) = point_of(start.geo, queue_name) else {
                return Err(Error::unsupported("a queue with no signed point").into());
            };
            let Some(row) = fifo::queues(world, start.queues)? else {
                return Err(Error::unsupported("no queue state").into());
            };
            let Some(queue) = row.queues.get(queue_name) else {
                return Err(Error::unsupported("no such queue").into());
            };
            let Some(first) = queue.queued_request_ids.first() else {
                return Err(Error::unsupported("the queue is empty").into());
            };
            let Some(request) = row.requests.get(&**first) else {
                return Err(Error::unsupported("a queue lists an unknown request").into());
            };
            if &*point.facility_id != facility_name {
                return Err(Error::unsupported("a queue's point is another facility's").into());
            }
            Some(IndependentHead {
                queue_id: Arc::from(queue_name),
                request: request.clone(),
                active_count: queue.active_request_ids.len() as i64,
                point_capacity: point.service_capacity,
                required_skills: point.required_staff_skills.clone(),
            })
        }
        None => None,
    };
    Ok(head)
}

/// `_queue_task` of the head: the person, the queue task and its start.
fn queue_task(world: &World, facility_name: &str, head: &Option<IndependentHead>) -> Result<(Arc<str>, Arc<str>, i64)> {
    let (agent_id, queue_task_id, queue_start) = {
        let rows = world.rows()?;
        let Some(facility) = rows.facilities.get(facility_name) else {
            return Err(Error::unsupported("no such facility").into());
        };
        match &head {
            Some(head) => {
                let Some(task) = rows.active_tasks.get(&head.request.queue_task_id).filter(|task| &*task.kind == "toilet_queue") else {
                    return Err(Error::unsupported("a queue's request lost its task").into());
                };
                if !facility.queue_agent_ids.contains(&head.request.agent_id) {
                    return Err(Error::unsupported("a request is missing from the facility's list").into());
                }
                (head.request.agent_id.clone(), task.id.clone(), task.start_minute)
            }
            None => {
                let Some(agent_id) = facility.queue_agent_ids.first() else {
                    return Err(Error::unsupported("the queue is empty").into());
                };
                // `_queue_task`.
                let mut found = None;
                for (_, task) in rows.active_tasks.iter() {
                    if &*task.kind == "toilet_queue" && task.agent_id.as_deref() == Some(&**agent_id) && task.resource_id.as_deref() == Some(facility_name) {
                        if found.is_some() {
                            return Err(Error::unsupported("a person with two queue tasks").into());
                        }
                        found = Some((task.id.clone(), task.start_minute));
                    }
                }
                let Some((task_id, start)) = found else {
                    return Err(Error::unsupported("a person with no queue task").into());
                };
                (agent_id.clone(), task_id, start)
            }
        }
    };
    Ok((agent_id, queue_task_id, queue_start))
}

/// The draw of the service's minutes and its key, as `rng_key` holds them: `Err` holds the parts of
/// the key for the host to draw from, where the host draws and has not yet.
fn draw_of(start: &Start<'_>, drawn: Option<f64>, agent_id: &Arc<str>, head: &Option<IndependentHead>) -> Result<std::result::Result<f64, Vec<Part>>> {
    let (facility_name, minute) = (start.facility_id, start.minute);
    let draw = match (start.rng, drawn) {
        (Rng::Seed(seed), _) => {
            let draws = KeyedDraws::new(seed);
            let mut key = Key::labelled(label::TOILET_SERVICE);
            key.text(facility_name).text(&agent_id).text("minute").int(minute).text(&start.policy).text("duration");
            if let Some(head) = &head {
                key.text(&head.queue_id);
            }
            key.draw(&draws)
        }
        (Rng::Host, None) => {
            let mut parts = vec![
                Part::Text(Arc::from(label::TOILET_SERVICE)),
                Part::Text(Arc::from(facility_name)),
                Part::Text(agent_id.clone()),
                Part::Text(Arc::from("minute")),
                Part::Int(minute),
                Part::Text(start.policy.clone()),
                Part::Text(Arc::from("duration")),
            ];
            if let Some(head) = &head {
                parts.push(Part::Text(head.queue_id.clone()));
            }
            return Ok(Err(parts));
        }
        (Rng::Host, Some(draw)) => {
            if !(0.0..1.0).contains(&draw) {
                return Err(Error::value_error("keyed RNG draw must be between zero and one").into());
            }
            draw
        }
    };
    Ok(Ok(draw))
}

/// The free staff with the point's skills in the facility's zone, in the table's order.
fn skilled_staff(world: &World, facility_name: &str, head: &IndependentHead) -> Result<Vec<Arc<str>>> {
    let rows = world.rows()?;
    let Some(facility) = rows.facilities.get(facility_name) else {
        return Err(Error::unsupported("no such facility").into());
    };
    let candidates: Vec<Arc<str>> = rows
        .staff
        .iter()
        .filter(|(_, person)| {
            head.required_skills.iter().all(|wanted| person.skills.0.contains(wanted)) && person.zone_id == facility.zone_id && person.active_task_id.is_none()
        })
        .map(|(_, person)| person.id.clone())
        .collect();
    Ok(candidates)
}

/// The people the facility serves now, and its capacity for its working units.
fn facility_load(world: &World, facility_name: &str) -> Result<(i64, i64)> {
    let rows = world.rows()?;
    let Some(facility) = rows.facilities.get(facility_name) else {
        return Err(Error::unsupported("no such facility"));
    };
    Ok((facility.active_agent_ids.len() as i64, facility.capacity.min(facility.working_units)))
}

/// The completion's identity and sequence free: no event and no task of its IDs, and no record of
/// its sequence.
fn identity_is_free(world: &World, events: &Events<'_>, event_id: &str, task_id: &str, sequence: i64) -> Result<()> {
    let task_is_taken = world.rows()?.active_tasks.get(task_id).is_some();
    if events.is_taken(event_id, None)? || task_is_taken {
        return Err(Error::unsupported("the service's identity is taken"));
    }
    if events.with_log(|log| log.has_sequence(sequence))? {
        return Err(Error::unsupported("the service's sequence is taken"));
    }
    Ok(())
}

/// The payload of the service's completion; an independent queue's names its staff (or None).
fn completion_payload(
    facility_arc: &Arc<str>,
    agent_id: &Arc<str>,
    task_id: &Arc<str>,
    independent: bool,
    staff_id: &Option<Arc<str>>,
) -> IndexMap<Arc<str>, V> {
    let mut payload = IndexMap::with_capacity(4);
    payload.insert(Arc::from("facility_id"), V::Str(facility_arc.clone()));
    payload.insert(Arc::from("agent_id"), V::Str(agent_id.clone()));
    payload.insert(Arc::from("task_id"), V::Str(task_id.clone()));
    if independent {
        payload.insert(Arc::from("staff_id"), staff_id.as_ref().map_or(V::Null, |id| V::Str(id.clone())));
    }
    payload
}

/// The person moved from the facility's queue (its head, or their place in an independent one) to
/// the people it serves.
fn queued_to_active(world: &World, facility_name: &str, agent_id: &Arc<str>, independent: bool) -> Result<()> {
    let mut rows = world.rows_mut()?;
    let Some(facility) = rows.facilities.get_mut(facility_name) else {
        return Err(Error::unsupported("no such facility"));
    };
    if independent {
        remove_from(&mut facility.queue_agent_ids, agent_id)?;
    } else {
        facility.queue_agent_ids.remove(0);
    }
    facility.active_agent_ids.push(agent_id.clone());
    Ok(())
}

/// The key of the service's draw, as the completion's provenance records it.
fn rng_key(start: &Start<'_>, facility_arc: &Arc<str>, agent_id: &Arc<str>, head: &Option<IndependentHead>) -> V {
    let text = |value: &str| V::Str(Arc::from(value));
    let minute = start.minute;
    let mut rng_key = vec![
        text(label::TOILET_SERVICE),
        V::Str(facility_arc.clone()),
        V::Str(agent_id.clone()),
        text("minute"),
        V::Int(minute),
        V::Str(start.policy.clone()),
        text("duration"),
    ];
    if let Some(head) = &head {
        rng_key.push(V::Str(head.queue_id.clone()));
    }
    V::List(rng_key)
}

// ---- complete ------------------------------------------------------------------------------------

/// What a completion asks once it is written: the walk home of the handler's `after_complete`,
/// which the dispatch runs as `travel::schedule_return` when it is the controller's own step.
#[must_use]
#[derive(Debug, PartialEq, Eq)]
pub struct ScheduleReturn;

/// `ToiletServiceCompletionHandler.apply` up to `after_complete`, with `complete_toilet_service`.
/// `body` says whether the world holds `body_state_v1`.
pub fn complete(world: &World, event: &EventRow, queues: &Held, geo: &Geo, body: &Held, metric_state_held: bool) -> WriteResult<ScheduleReturn> {
    let ids = (payload_text(event, "facility_id"), payload_text(event, "agent_id"), payload_text(event, "task_id"));
    let (Some(facility_id), Some(agent_id), Some(task_id)) = ids else {
        return Err(Error::failed("toilet completion requires facility_id, agent_id, and task_id").into());
    };
    if facility_id.is_empty() || agent_id.is_empty() || task_id.is_empty() {
        return Err(Error::failed("toilet completion requires facility_id, agent_id, and task_id").into());
    }
    let due = event.due_minute;
    if !world.rows()?.active_tasks.get(&task_id).is_some_and(|task| task.due_minute == due) {
        return Err(Error::failed("toilet completion does not match an active due task").into());
    }
    // `complete_toilet_service`'s checks, in its order: the handler raises its `ValueError`'s text.
    let (staff_id, task_start) = {
        let rows = world.rows()?;
        let (agent, facility) = (py_repr(&agent_id), py_repr(&facility_id));
        let failed = |text: String| -> crate::events::writer::WriteError { Error::failed(text).into() };
        let Some(facility_row) = rows.facilities.get(&facility_id).filter(|row| is_toilet(&row.kind)) else {
            return Err(failed(format!("{facility} is not a toilet Facility")));
        };
        let Some(agent_row) = rows.agents.get(&agent_id) else {
            return Err(failed(format!("Agent {agent} does not exist")));
        };
        let Some(task) = rows.active_tasks.get(&task_id).filter(|task| &*task.kind == "toilet_service") else {
            return Err(failed(format!("toilet service task {} is not active", py_repr(&task_id))));
        };
        if task.agent_id.as_deref() != Some(&*agent_id) || task.resource_id.as_deref() != Some(&*facility_id) {
            return Err(failed("toilet completion payload does not match its active task".to_owned()));
        }
        if agent_row.activity != Activity::InService || agent_row.target_id.as_deref() != Some(&*facility_id) {
            return Err(failed(format!("Agent {agent} is not using Facility {facility}")));
        }
        if facility_row.active_agent_ids.iter().filter(|id| ***id == *agent_id).count() != 1 {
            return Err(failed(format!("Agent {agent} must occupy exactly one toilet service slot")));
        }
        // The check of a reserved person, made before anything is written.
        if let Some(staff_id) = &task.staff_id {
            let consistent = rows.staff.get(staff_id).is_some_and(|staff| {
                staff.active_task_id.as_deref() == Some(&*task.id)
                    && staff.work_queue.len() == 1
                    && staff.work_queue[0] == task.id
                    && staff.busy_until == task.due_minute
            });
            if !consistent {
                return Err(Error::unsupported("the staff reservation is inconsistent").into());
            }
        }
        (task.staff_id.clone(), task.start_minute)
    };
    check_metric_state(world, metric_state_held)?;
    // `finish_service` and `record_queue_transition`.
    let events = Events::of(world)?;
    let minute = world.minute_index()?;
    let finished = match finishing_request(world, queues, &task_id)? {
        Some((request, position)) => {
            let wait = task_start.checked_sub(request.requested_minute).ok_or_else(|| Error::unsupported("a wait out of range"))?;
            let capacity = point_of(geo, &request.queue_id).map(|point| point.service_capacity);
            let sequence = events.next_sequence()?;
            let row = transition_row(
                &events,
                &fifo::Transition { request: &request, transition: "completed", task_id: &task_id, wait_minutes: Some(wait), causation_id: Some(&task_id) },
                fifo::Place { queue_position: position, capacity },
                sequence,
                minute,
            )?;
            Some((request, row))
        }
        None => None,
    };
    // `world.model_state.get("body_state_v1", {}).get("agents", {})`.
    let hygiene = match held_row::<BodyRow>(world, BODY_STATE, body)? {
        None => None,
        Some(row) => {
            let need = row.agents.get(&*agent_id).map(|record| record.hygiene_need);
            if need.is_some_and(|need| !need.is_finite()) {
                return Err(Error::unsupported("a need is not finite").into());
            }
            need.map(|need| py_max(0.0, need - HYGIENE_RELIEF))
        }
    };

    // Write.
    if let Some((request, row)) = &finished {
        fifo::write_finish(world, request)?;
        fifo::write_transition(&events, row, minute)?;
    }
    {
        let mut rows = world.rows_mut()?;
        let Some(facility) = rows.facilities.get_mut(&facility_id) else {
            return Err(Error::unsupported("no such facility").into());
        };
        remove_from(&mut facility.active_agent_ids, &agent_id)?;
    }
    if let Some(staff_id) = &staff_id {
        release_staff(world, staff_id)?;
    }
    remove_task(world, &task_id)?;
    {
        let mut rows = world.rows_mut()?;
        set_agent(&mut rows, &agent_id, Activity::Idle, None)?;
        let Some(row) = rows.agents.get_mut(&agent_id) else {
            return Err(Error::unsupported("no such agent").into());
        };
        row.needs.toilet_urgency = 0.0;
    }
    add_metric(world, COMPLETED_METRIC)?;
    if let Some(need) = hygiene {
        let mut row = world.row_mut::<BodyRow>(BODY_STATE)?;
        let Some(record) = row.agents.get_mut(&*agent_id) else {
            return Err(Error::Runtime("a planned record left the row".to_owned()).into());
        };
        record.hygiene_need = need;
    }
    Ok(ScheduleReturn)
}

// ---- arbitrate (the arbitrate phase's loop) -----------------------------------------------------

/// `ToiletServiceManager.arbitrate`: each facility with anyone queued, in ID order; a facility with
/// independent queues serves each of them (`_arbitrate_independent_facility`), any other its shared
/// queue while it has room. Returns the IDs of the started events, in order. The manager is the one
/// `tick.policies` read: the controller's.
pub fn arbitrate<C: WorldCell, H: Host, R: crate::rules::tick::RunCell>(
    tick: &mut crate::rules::tick::Tick<'_, C, H, R>,
) -> crate::rules::tick::Step<Vec<Arc<str>>, H::Error> {
    use crate::rules::tick::Labelled;
    let due = facilities_of(tick).at("toilet.facilities_to_serve")?;
    let mut started = Vec::new();
    for id in due {
        if let Some(queues) = tick.independent_queues(&id)? {
            started.extend(arbitrate_independent(tick, &id, &queues)?);
            continue;
        }
        tick.prepare_queue(&id, false)?;
        loop {
            let room = room_of(tick, &id).at("toilet.has_room")?;
            if !room || tick.progress(&id, false, false, None)? || tick.progress(&id, false, true, None)? {
                break;
            }
            match start_of(tick, &id, None).at("toilet.start_head")? {
                Started::Event(event_id) => started.push(event_id),
                Started::Stopped => break,
            }
        }
    }
    Ok(started)
}

/// `_arbitrate_independent_facility`: each queue, with the check of its signed point. Returns the
/// IDs of the started events, in order.
pub fn arbitrate_independent<C: WorldCell, H: Host, R: crate::rules::tick::RunCell>(
    tick: &mut crate::rules::tick::Tick<'_, C, H, R>,
    id: &str,
    queues: &[Arc<str>],
) -> crate::rules::tick::Step<Vec<Arc<str>>, H::Error> {
    use crate::rules::tick::Labelled;
    let mut started = Vec::new();
    for queue in queues {
        tick.prepare_independent(queue, false)?;
        tick.check_signed_point(queue, "service")?;
        while tick.queue_has_people(queue)? {
            if tick.progress(id, false, false, Some(queue))? {
                break;
            }
            match start_of(tick, id, Some(queue)).at("toilet.serve_independent_head")? {
                Started::Event(event_id) => started.push(event_id),
                Started::Stopped => break,
            }
        }
    }
    Ok(started)
}

/// `facilities_to_serve` of the manager's world.
fn facilities_of<C: WorldCell, H: Host, R: crate::rules::tick::RunCell>(
    tick: &mut crate::rules::tick::Tick<'_, C, H, R>,
) -> std::result::Result<Vec<Arc<str>>, H::Error> {
    let holding = tick.holding()?;
    let queues = holding.held("facility_queues_v1", "the world holds another queue state");
    tick.cell.with(|world| facilities_to_serve(world, &queues)).and_then(|due| due).map_err(H::Error::from)
}

/// `has_room` of the tick (`toilet.has_room`).
fn room_of<C: WorldCell, H: Host, R: crate::rules::tick::RunCell>(
    tick: &mut crate::rules::tick::Tick<'_, C, H, R>,
    id: &str,
) -> std::result::Result<bool, H::Error> {
    let holding = tick.holding()?;
    if !holding.table {
        return Err(Error::unsupported("a value is not a plain dict").into());
    }
    let queues = holding.held("facility_queues_v1", "the world holds another queue state");
    let placement = tick.placement()?;
    tick.cell.with(|world| has_room(world, &queues, placement.as_deref(), id)).and_then(|room| room).map_err(H::Error::from)
}

/// A pass of the loop after the positions (`start_one` of the toilet's glue): the manager's
/// durations and policy as the Run read them, the tick's random source.
fn start_of<C: WorldCell, H: Host, R: crate::rules::tick::RunCell>(
    tick: &mut crate::rules::tick::Tick<'_, C, H, R>,
    facility_id: &str,
    queue_id: Option<&str>,
) -> std::result::Result<Started, H::Error> {
    use crate::rules::tick::Seed;
    use crate::run::held::Held as Read;
    let minute = tick.minute;
    let random = match &tick.seed {
        Seed::Counter { text: Some(text), .. } => Rng::Seed(text.to_string()),
        Seed::Counter { text: None, .. } => return Err(Error::unsupported("a value is not an int").into()),
        Seed::Other => Rng::Host,
    };
    let policies = tick.policies.clone();
    let manager = match policies.services.toilet.object() {
        Ok(manager) => manager,
        Err(fault) => return Err(tick.policy_error(&["toilet_service"], &fault)),
    };
    let int = |name: &str| -> Result<i64> {
        match manager.get(name) {
            Ok(Read::Value(V::Int(value))) => Ok(*value),
            _ => Err(Error::unsupported("a value is not an int")),
        }
    };
    let lowest = int("min_service_minutes")?;
    let highest = int("max_service_minutes")?;
    let policy = match manager.get("policy_version") {
        Ok(Read::Value(V::Str(text))) => text.clone(),
        _ => return Err(Error::unsupported("a value is not a str").into()),
    };
    let holding = tick.holding()?;
    let queues = holding.held("facility_queues_v1", "the world holds another queue state");
    let geo = tick.host.floor_plan()?;
    let walking = match queue_id {
        Some(_) => tick.walking(true)?,
        None => None,
    };
    let walk = walking.as_ref().map(|walking| walking.walk());
    let pass = Start {
        facility_id,
        minute,
        queue_id,
        lowest,
        highest,
        policy,
        rng: &random,
        queues: &queues,
        geo: &geo,
        walk: walk.as_ref(),
        metric_state_held: holding.field(crate::world::METRIC_STATE),
    };
    start(tick.cell, tick.host, &pass)
}
