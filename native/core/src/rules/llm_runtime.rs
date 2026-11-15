//! The rules that ask an LLM during a tick (in the core since #428): a pending support request's
//! queue priority (`prioritize_support_request`, of the propose phase) and the event-triggered
//! social policy (a support request's response, a long-waiting complaint's, a cooperation, an
//! information reaction).
//!
//! The rules read and write the world, and ask through a host's `LlmLink` (`crate::llm`), which
//! only passes the task on: the prompt, the check of the answer and what the answer does are here.
//! A host with a link calls them from its hooks (`Host::prioritize_support_request`,
//! `Host::consider_*`; `PlainHost`, `plain.rs`).
//!
//! What they write:
//! - `model_state["support_priority_runtime_v1"]`: the attempts by day and the skips by reason of
//!   the priority's limits; `model_state["llm"]["support_priorities"][request]`: the priority, which
//!   the staff's assignment reads (`staff.rs`).
//! - `model_state["social_runtime_v1"]`: the social policy's attempts by day, by person and day,
//!   each person's last attempt, and how many decisions it made; a `social_interaction` event for an
//!   action it accepted, which `social_interaction.rs` applies.
//! - The record of each social decision, accepted or not (`Decision::record`, with `record_kind`
//!   `dialogue`), where the host collects decision records.
//! - The tape's entry of each call (`llm::tape_entry`).
//!
//! Offline (a link without an API key), nothing is asked and nothing of the world is written: the
//! priority returns at once, and the social policy records why it did not ask (`offline_rule_mode`
//! or a check before it). A Run offline is the Run without a link, but for these records.
//!
//! The prompts are data written as canonical JSON (keys sorted, no spaces). The state's hash a task
//! carries is the core's digest of the world at the call (`canonical::state_digest_with`): a
//! response tape recorded before #428 (a Checkpoint's) does not replay here.

use std::sync::Arc;

use indexmap::IndexMap;
use serde_json::Value;

use crate::canonical::{state_digest_with, ListNode};
use crate::entities::RequestStatus;
use crate::error::{Error, Result};
use crate::events::{EventKind, EventRow};
use crate::host::Holding;
use crate::llm::{self, check_answer, kept_text, tape_entry, Limits, LlmLink, Reply, Task, STAGE_RUNTIME};
use crate::row::ValueRow;
use crate::rules::information_events::OTHER_DOCUMENT;
use crate::rules::kit::{self, Events};
use crate::rules::social_dialogue::capture_context;
use crate::state::information::{InformationRow, NAME as INFORMATION};
use crate::value::V;
use crate::world::{Place, World, WorldCell};

/// The key of the social policy's state in `model_state`.
pub const SOCIAL_STATE: &str = "social_runtime_v1";
/// The key of the support priority's state in `model_state`.
pub const PRIORITY_STATE: &str = "support_priority_runtime_v1";
/// The namespace that holds the priorities (`model_state["llm"]["support_priorities"]`).
pub const LLM_STATE: &str = "llm";
/// The policy version of the event-triggered social policy.
pub const SOCIAL_POLICY_VERSION: &str = "event-triggered-social-v2";
/// The policy version of the support priority.
pub const PRIORITY_POLICY_VERSION: &str = "support-priority-v2";
/// The source ID of the social policy's limits.
pub const SOCIAL_LIMITS_SOURCE: &str = "ASM-EVENT-TRIGGERED-SOCIAL-LIMITS-V1";
/// The source ID of the social policy's interactions.
pub const SOCIAL_INTERACTIONS_SOURCE: &str = "ASM-EVENT-TRIGGERED-SOCIAL-INTERACTIONS-V1";
/// The priority of a `social_interaction` event the policy schedules.
const EVENT_PRIORITY: i64 = 10;
const SOCIAL_ACTIONS: [&str; 3] = ["offer_reassurance", "share_practical_information", "no_interaction"];

/// What a Run with a link keeps for these rules: the link, the limits, what the state's hash reads,
/// and the tape's entries not taken yet.
pub struct Runtime {
    pub link: Box<dyn LlmLink>,
    pub limits: Limits,
    /// The world document's `rng_state`, which the state's hash reads (`PlainRun::rng_state`).
    rng_state: V,
    /// The log's node of the last hash (`state_digest_with`).
    log: ListNode,
    tape: Vec<Vec<u8>>,
    calls: u64,
}

impl Runtime {
    pub fn new(link: Box<dyn LlmLink>, limits: Limits, rng_state: V) -> Result<Self> {
        limits.check()?;
        Ok(Self { link, limits, rng_state, log: ListNode::default(), tape: Vec::new(), calls: 0 })
    }

    /// The tape's entries since the last call, in order.
    pub fn take_tape(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.tape)
    }

    /// How many calls the link answered.
    pub fn calls(&self) -> u64 {
        self.calls
    }

    fn state_hash(&mut self, world: &World) -> Result<String> {
        state_digest_with(world, &self.rng_state, &mut self.log)
    }

    /// Asks the link, reads the answer against the task's schema (an answer that does not fit is
    /// the fallback, `invalid_output`), and keeps the tape's entry.
    fn ask(&mut self, minute: i64, task: &Task) -> Result<(Reply, Value)> {
        let mut reply = self.link.complete(task)?;
        let answer = match (reply.fallback_used, check_answer(&reply.text, &task.output_schema)) {
            (false, Some(answer)) => answer,
            (used, _) => {
                if !used {
                    reply = Reply { outcome: "invalid_output".to_owned(), fallback_used: true, ..reply };
                }
                check_answer(&task.fallback_text, &task.output_schema)
                    .ok_or_else(|| Error::Runtime("the rule's fallback does not fit its schema".to_owned()))?
            }
        };
        reply.text = if reply.fallback_used { task.fallback_text.clone() } else { kept_text(&answer)? };
        self.calls += 1;
        self.tape.push(tape_entry(minute, task, &reply)?);
        Ok((reply, answer))
    }
}

// ---- The support request's priority ---------------------------------------------------------------

/// A pending support request's priority: offline, nothing. Otherwise the request is asked about
/// only while its person's stress reaches the limit and the day's attempts are under the cap (each
/// skip counted by its reason); the answer's priority (0, or 1 for urgent) is kept by the request's
/// ID.
pub fn prioritize_support_request<C: WorldCell>(cell: &C, runtime: &mut Runtime, request_id: &str, agent_id: &str, observed_state_hash: &str) -> Result<()> {
    if runtime.link.offline() {
        return Ok(());
    }
    let limits = runtime.limits.priority;
    let planned = cell.with(|world| -> Result<Option<(i64, Task)>> {
        let minute = world.minute_index()?;
        let (stress, trust, support_need, needs) = {
            let rows = world.rows()?;
            let agent = rows.agents.get(agent_id).ok_or_else(|| Error::Key(format!("'{agent_id}'")))?;
            (agent.stress, agent.trust, agent.profile.support_need, [agent.needs.toilet_urgency, agent.needs.hunger, agent.needs.fatigue])
        };
        let mut state = PriorityRuntime::read(world)?;
        let day = day_key(minute);
        let attempts = count(&state.attempts_by_day, &day);
        let skip = if stress < limits.stress_threshold {
            Some("below_stress_threshold")
        } else if attempts >= limits.global_per_day {
            Some("global_daily_cap")
        } else {
            None
        };
        match skip {
            Some(reason) => {
                let skipped = count(&state.skipped_by_reason, reason);
                state.skipped_by_reason.insert(Arc::from(reason), skipped + 1);
                put_value(world, PRIORITY_STATE, state.value())?;
                return Ok(None);
            }
            None => {
                state.attempts_by_day.insert(Arc::from(day.as_str()), attempts + 1);
                put_value(world, PRIORITY_STATE, state.value())?;
            }
        }
        let constraints = [
            "Use only the functional need and current state values below.",
            "Do not infer from names, demographics, or group membership.",
            "Do not approve or deny support; this only orders a staff queue.",
            "Return the required JSON object only.",
            "Keep rationale to one short phrase of at most 80 characters.",
        ];
        let prompt = map([
            ("task", text("Set queue priority for one synthetic staff support request.")),
            ("allowed_priority", map([("0", text("normal")), ("1", text("urgent"))])),
            ("constraints", V::List(constraints.iter().map(|item| text(item)).collect())),
            ("minute", V::Int(minute)),
            ("support_need", V::Float(support_need)),
            ("stress", V::Float(stress)),
            ("trust", V::Float(trust)),
            ("needs", map([("toilet_urgency", V::Float(needs[0])), ("hunger", V::Float(needs[1])), ("fatigue", V::Float(needs[2]))])),
        ]);
        let task = Task {
            task: llm::SUPPORT_PRIORITY,
            stage: STAGE_RUNTIME,
            prompt_version: PRIORITY_POLICY_VERSION.to_owned(),
            schema_version: 1,
            input_text: json_text(&prompt)?,
            state_hash: observed_state_hash.to_owned(),
            output_schema: support_advice_schema(),
            fallback_text: r#"{"priority":0,"rationale":"rule-based fallback"}"#.to_owned(),
            max_output_tokens: 96,
            input_byte_limit: None,
        };
        Ok(Some((minute, task)))
    })??;
    let Some((minute, task)) = planned else {
        return Ok(());
    };
    let (_, answer) = runtime.ask(minute, &task)?;
    let priority = answer.get("priority").and_then(Value::as_i64).ok_or_else(|| Error::Runtime("a priority without its value".to_owned()))?;
    cell.with(|world| -> Result<()> {
        let mut state = match world.slots.try_value(LLM_STATE)? {
            None => IndexMap::new(),
            Some(value) => match &*value {
                V::Map(state) => state.clone(),
                _ => return Err(Error::Value("model_state['llm'] is not a dict".to_owned())),
            },
        };
        let priorities = state.entry(Arc::from("support_priorities")).or_insert_with(|| V::Map(IndexMap::new()));
        let V::Map(priorities) = priorities else {
            return Err(Error::Value("model_state['llm']['support_priorities'] is not a dict".to_owned()));
        };
        priorities.insert(Arc::from(request_id), V::Int(priority));
        put_value(world, LLM_STATE, V::Map(state))
    })?
}

fn support_advice_schema() -> V {
    object_schema(&[
        ("priority", map([("type", text("integer")), ("enum", V::List(vec![V::Int(0), V::Int(1)]))])),
        ("rationale", map([("type", text("string")), ("maxLength", V::Int(160))])),
    ])
}

// ---- The social policy ----------------------------------------------------------------------------

/// A social decision, written as its record (`record`).
struct Decision<'a> {
    minute: i64,
    agent_id: &'a str,
    request_id: Option<&'a str>,
    trigger: Option<&'a str>,
    interaction_type: &'a str,
    target_agent_id: Option<&'a str>,
    trigger_id: Option<&'a str>,
    action: Option<&'a str>,
    accepted: bool,
    reason: String,
    reason_short: Option<&'a str>,
    event_id: Option<&'a str>,
    call: Option<(&'a Task, &'a Reply)>,
}

impl Decision<'_> {
    /// The decision's record with `record_kind` `dialogue` and its `speech_status` (the row an
    /// indexed sink keeps), as canonical JSON.
    fn record(&self) -> Result<Vec<u8>> {
        let optional = |value: Option<&str>| value.map_or(V::Null, text);
        let (input_hash, state_hash, model_id, cache_hit, fallback_used, cost, requests, input_tokens, output_tokens) = match self.call {
            None => (V::Null, V::Null, V::Null, false, false, 0.0, 0, 0, 0),
            Some((task, reply)) => (
                text(&task.input_hash()),
                text(&task.state_hash),
                optional(reply.model_id.as_deref()),
                reply.cache_hit,
                reply.fallback_used,
                reply.estimated_cost_usd,
                reply.api_request_count,
                reply.input_tokens,
                reply.output_tokens,
            ),
        };
        let source = if self.interaction_type == "support_request" { SOCIAL_LIMITS_SOURCE } else { SOCIAL_INTERACTIONS_SOURCE };
        let record = map([
            ("record_kind", text("dialogue")),
            ("minute", V::Int(self.minute)),
            ("agent_id", text(self.agent_id)),
            ("request_id", optional(self.request_id)),
            ("trigger", text(self.trigger.unwrap_or("pending_support_request"))),
            ("interaction_type", text(self.interaction_type)),
            ("target_agent_id", optional(self.target_agent_id)),
            ("trigger_id", optional(self.trigger_id)),
            ("action", optional(self.action)),
            ("accepted", V::Bool(self.accepted)),
            ("status", text(if self.accepted { "accepted" } else { "rejected" })),
            ("reason", text(&self.reason)),
            ("reason_short", optional(self.reason_short)),
            ("effect_code", optional(effect_code(self.interaction_type, self.action))),
            ("event_id", optional(self.event_id)),
            ("input_hash", input_hash),
            ("state_hash", state_hash),
            ("model_id", model_id),
            ("estimated_cost_usd", V::Float(cost)),
            ("api_request_count", V::Int(requests)),
            ("input_tokens", V::Int(input_tokens)),
            ("output_tokens", V::Int(output_tokens)),
            ("source_type", text("assumption")),
            ("source_id", text(source)),
            ("cache_hit", V::Bool(cache_hit)),
            ("fallback_used", V::Bool(fallback_used)),
            ("speech_status", text("structured_interaction_only_no_recorded_utterance")),
        ]);
        record.to_json()
    }
}

/// The fixed label of what an accepted action does.
fn effect_code(interaction_type: &str, action: Option<&str>) -> Option<&'static str> {
    Some(match (interaction_type, action?) {
        ("support_request", "offer_reassurance") => "requester_stress_relief_and_trust_increase",
        ("support_request", "share_practical_information") => "requester_trust_increase",
        ("complaint_response", "acknowledge_complaint" | "explain_staff_process") => "requester_stress_relief_and_trust_increase",
        ("complaint_response", "share_practical_information") => "requester_trust_increase",
        ("cooperation", "offer_mutual_support") => "both_agents_stress_relief_and_trust_increase",
        ("information_reaction", "seek_claim_verification") => "verification_query_pressure_increased",
        _ => return None,
    })
}

/// Where a decision's record goes: the host's decision records, or nowhere.
pub type Records<'a> = Option<&'a mut Vec<Vec<u8>>>;

fn keep(records: &mut Records<'_>, decision: &Decision<'_>) -> Result<()> {
    if let Some(records) = records {
        records.push(decision.record()?);
    }
    Ok(())
}

/// What a rejection names besides its reason.
#[derive(Clone, Copy)]
struct Rejected<'a> {
    request_id: Option<&'a str>,
    trigger: Option<&'a str>,
    interaction_type: &'a str,
    target_agent_id: Option<&'a str>,
    trigger_id: Option<&'a str>,
}

impl<'a> Rejected<'a> {
    fn support(request_id: &'a str) -> Self {
        Self { request_id: Some(request_id), trigger: None, interaction_type: "support_request", target_agent_id: None, trigger_id: None }
    }

    fn decision(&self, minute: i64, agent_id: &'a str, reason: &str) -> Decision<'a> {
        Decision {
            minute,
            agent_id,
            request_id: self.request_id,
            trigger: self.trigger,
            interaction_type: self.interaction_type,
            target_agent_id: self.target_agent_id,
            trigger_id: self.trigger_id,
            action: None,
            accepted: false,
            reason: reason.to_owned(),
            reason_short: None,
            event_id: None,
            call: None,
        }
    }
}

/// `consider_support_request(world, agent_id=, request_id=)`: in the controller's order, a request
/// no longer pending, a person below the stress limit, offline, a response due past the Run's end,
/// the person's and the day's caps and the person's cooldown each reject it; otherwise the policy
/// asks, counts the attempt, and schedules the chosen action (`social-interaction-<sequence>`, due
/// after `event_delay_minutes`) unless it is `no_interaction` or the answer fell back.
pub fn consider_support_request<C: WorldCell>(cell: &C, runtime: &mut Runtime, mut records: Records<'_>, agent_id: &str, request_id: &str) -> Result<()> {
    let limits = runtime.limits.social;
    let offline = runtime.link.offline();
    let rejected = Rejected::support(request_id);
    let planned = cell.with(|world| -> Result<std::result::Result<(i64, i64, Task), (i64, &'static str)>> {
        let minute = world.minute_index()?;
        let (stress, trust, support_need, needs, status, requested) = {
            let rows = world.rows()?;
            let (Some(agent), Some(request)) = (rows.agents.get(agent_id), rows.support_requests.get(request_id)) else {
                return Err(Error::Value("social proposal requires a matching Agent support request".to_owned()));
            };
            if &*request.agent_id != agent_id {
                return Err(Error::Value("social proposal requires a matching Agent support request".to_owned()));
            }
            (
                agent.stress,
                agent.trust,
                agent.profile.support_need,
                [agent.needs.toilet_urgency, agent.needs.hunger, agent.needs.fatigue],
                request.status,
                request.requested_minute,
            )
        };
        if status != RequestStatus::Pending {
            return Ok(Err((minute, "request_not_pending")));
        }
        if stress < limits.stress_threshold {
            return Ok(Err((minute, "below_stress_threshold")));
        }
        if offline {
            return Ok(Err((minute, "offline_rule_mode")));
        }
        let due = minute + limits.event_delay_minutes;
        if due >= world.duration_minutes()? {
            return Ok(Err((minute, "scenario_deadline")));
        }
        let state = SocialRuntime::read(world)?;
        let day = minute.div_euclid(1440);
        if count(&state.attempts_by_agent_day, &format!("{day}:{agent_id}")) >= limits.per_agent_per_day {
            return Ok(Err((minute, "agent_daily_cap")));
        }
        if count(&state.attempts_by_day, &day.to_string()) >= limits.global_per_day {
            return Ok(Err((minute, "global_daily_cap")));
        }
        if let Some(prior) = state.last_attempt(agent_id) {
            if minute - prior < limits.cooldown_minutes {
                return Ok(Err((minute, "cooldown")));
            }
        }
        let input = map([
            ("trigger", text("pending_support_request")),
            ("allowed_actions", V::List(SOCIAL_ACTIONS.iter().map(|action| text(action)).collect())),
            (
                "constraints",
                V::List(
                    [
                        "Use only the anonymous functional context below.",
                        "Do not infer demographics or create factual claims about people.",
                        "Choose no_interaction when context does not support an action.",
                        "Return exactly the required JSON object.",
                    ]
                    .iter()
                    .map(|item| text(item))
                    .collect(),
                ),
            ),
            ("minute", V::Int(minute)),
            (
                "agent_context",
                map([
                    ("stress", V::Float(stress)),
                    ("trust", V::Float(trust)),
                    ("support_need", V::Float(support_need)),
                    ("toilet_urgency", V::Float(needs[0])),
                    ("hunger", V::Float(needs[1])),
                    ("fatigue", V::Float(needs[2])),
                ]),
            ),
            ("request_context", map([("status", text(status.text())), ("wait_minutes", V::Int(minute - requested))])),
        ]);
        let task = Task {
            task: llm::SOCIAL_SUPPORT_REQUEST,
            stage: STAGE_RUNTIME,
            prompt_version: SOCIAL_POLICY_VERSION.to_owned(),
            schema_version: 1,
            input_text: json_text(&input)?,
            state_hash: runtime.state_hash(world)?,
            output_schema: object_schema(&[(
                "action",
                map([("type", text("string")), ("enum", V::List(SOCIAL_ACTIONS.iter().map(|action| text(action)).collect()))]),
            )]),
            fallback_text: r#"{"action":"no_interaction"}"#.to_owned(),
            max_output_tokens: 24,
            input_byte_limit: None,
        };
        Ok(Ok((minute, due, task)))
    })??;
    let (minute, due, task) = match planned {
        Err((minute, reason)) => return keep(&mut records, &rejected.decision(minute, agent_id, reason)),
        Ok(planned) => planned,
    };
    let (reply, answer) = runtime.ask(minute, &task)?;
    let action = answer.get("action").and_then(Value::as_str).ok_or_else(|| Error::Runtime("an action without its value".to_owned()))?.to_owned();
    let participants = [agent_id];
    let event_id = cell.with(|world| -> Result<Option<String>> {
        count_attempts(world, minute, &participants)?;
        if reply.fallback_used || action == "no_interaction" {
            return Ok(None);
        }
        let events = Events::of(world)?;
        let sequence = events.allocate_sequence()?;
        let event_id = format!("social-interaction-{sequence}");
        let payload = map_of([("agent_id", text(agent_id)), ("request_id", text(request_id)), ("action", text(&action))]);
        let provenance = provenance(SOCIAL_LIMITS_SOURCE, &task, &reply);
        schedule(&events, &event_id, due, sequence, payload, request_id, provenance)?;
        Ok(Some(event_id))
    })??;
    let reason = match (&event_id, reply.fallback_used) {
        (_, true) => format!("rule_fallback:{}", reply.outcome),
        (None, false) => "model_selected_no_interaction".to_owned(),
        (Some(_), false) => "accepted_and_scheduled".to_owned(),
    };
    let decision = Decision {
        action: Some(&action),
        accepted: !reply.fallback_used,
        reason,
        event_id: event_id.as_deref(),
        call: Some((&task, &reply)),
        trigger: Some("pending_support_request"),
        ..rejected.decision(minute, agent_id, "")
    };
    keep(&mut records, &decision)
}

/// `consider_complaint_response(world, agent_id=, request_id=)`: a request still pending that has
/// waited `complaint_wait_minutes`, of a person whose stress reaches the limit, is a typed
/// interaction of that person alone.
pub fn consider_complaint_response<C: WorldCell>(cell: &C, runtime: &mut Runtime, mut records: Records<'_>, agent_id: &str, request_id: &str) -> Result<()> {
    let limits = runtime.limits.social;
    let rejected = Rejected {
        request_id: Some(request_id),
        trigger: Some("long_waiting_support_request"),
        interaction_type: "complaint_response",
        target_agent_id: None,
        trigger_id: None,
    };
    let checked = cell.with(|world| -> Result<std::result::Result<Candidate, (i64, &'static str)>> {
        let minute = world.minute_index()?;
        let rows = world.rows()?;
        let (Some(agent), Some(request)) = (rows.agents.get(agent_id), rows.support_requests.get(request_id)) else {
            return Err(Error::Value("complaint response requires a matching Agent support request".to_owned()));
        };
        if &*request.agent_id != agent_id {
            return Err(Error::Value("complaint response requires a matching Agent support request".to_owned()));
        }
        if request.status != RequestStatus::Pending {
            return Ok(Err((minute, "request_not_pending")));
        }
        let wait = minute - request.requested_minute;
        if wait < limits.complaint_wait_minutes {
            return Ok(Err((minute, "complaint_wait_threshold_not_met")));
        }
        if agent.stress < limits.stress_threshold {
            return Ok(Err((minute, "below_stress_threshold")));
        }
        Ok(Ok(Candidate {
            interaction_type: "complaint_response",
            trigger: "long_waiting_support_request",
            initiator: agent_id.to_owned(),
            participants: vec![agent_id.to_owned()],
            trigger_id: request_id.to_owned(),
            request_id: Some(request_id.to_owned()),
            target_agent_id: None,
            zone_id: agent.zone_id.to_string(),
            relationship: "pending_staff_support_request",
            observed_facts: map([
                ("request_status", text(request.status.text())),
                ("wait_minutes", V::Int(wait)),
                ("stress", V::Float(agent.stress)),
                ("trust", V::Float(agent.trust)),
            ]),
            allowed_actions: &["acknowledge_complaint", "explain_staff_process", "share_practical_information", "no_interaction"],
            reason_codes: &["long_wait", "high_stress", "request_still_pending"],
            event_payload: map_of([
                ("interaction_type", text("complaint_response")),
                ("agent_id", text(agent_id)),
                ("request_id", text(request_id)),
                ("zone_id", text(&agent.zone_id)),
                ("stress_threshold", V::Float(limits.stress_threshold)),
                ("complaint_wait_minutes", V::Int(limits.complaint_wait_minutes)),
            ]),
        }))
    })??;
    match checked {
        Err((minute, reason)) => keep(&mut records, &rejected.decision(minute, agent_id, reason)),
        Ok(candidate) => typed(cell, runtime, records, candidate),
    }
}

/// A cooperation the social policy considers after a contact (`dispatch::consider_contact`): two
/// people of the same group in the same zone, one of whom is stressed past the limit, are a typed
/// interaction of both.
pub fn consider_cooperation<C: WorldCell>(
    cell: &C,
    runtime: &mut Runtime,
    mut records: Records<'_>,
    initiator: &str,
    participant: &str,
    trigger_event_id: &str,
) -> Result<()> {
    let limits = runtime.limits.social;
    let rejected = Rejected {
        request_id: None,
        trigger: Some("modeled_social_contact"),
        interaction_type: "cooperation",
        target_agent_id: Some(participant),
        trigger_id: Some(trigger_event_id),
    };
    let checked = cell.with(|world| -> Result<std::result::Result<Candidate, (i64, &'static str)>> {
        let minute = world.minute_index()?;
        let rows = world.rows()?;
        let (Some(first), Some(second)) = (rows.agents.get(initiator), rows.agents.get(participant)) else {
            return Err(Error::Value("cooperation requires two different known Agent IDs".to_owned()));
        };
        if initiator == participant {
            return Err(Error::Value("cooperation requires two different known Agent IDs".to_owned()));
        }
        if first.zone_id != second.zone_id {
            return Ok(Err((minute, "participants_not_in_same_zone")));
        }
        if first.profile.group_id.is_none() || first.profile.group_id != second.profile.group_id {
            return Ok(Err((minute, "modeled_relationship_not_eligible")));
        }
        if crate::mathx::py_max(first.stress, second.stress) < limits.stress_threshold {
            return Ok(Err((minute, "participants_below_stress_threshold")));
        }
        Ok(Ok(Candidate {
            interaction_type: "cooperation",
            trigger: "modeled_social_contact",
            initiator: initiator.to_owned(),
            participants: vec![initiator.to_owned(), participant.to_owned()],
            trigger_id: trigger_event_id.to_owned(),
            request_id: None,
            target_agent_id: Some(participant.to_owned()),
            zone_id: first.zone_id.to_string(),
            relationship: "same_group",
            observed_facts: map([
                ("initiator_stress", V::Float(first.stress)),
                ("participant_stress", V::Float(second.stress)),
                ("contact_event_applied", V::Bool(true)),
            ]),
            allowed_actions: &["offer_mutual_support", "no_interaction"],
            reason_codes: &["shared_group_contact", "elevated_stress"],
            event_payload: map_of([
                ("interaction_type", text("cooperation")),
                ("agent_id", text(initiator)),
                ("participant_agent_id", text(participant)),
                ("zone_id", text(&first.zone_id)),
                ("stress_threshold", V::Float(limits.stress_threshold)),
                ("relationship", text("same_group")),
                ("trigger_event_id", text(trigger_event_id)),
            ]),
        }))
    })??;
    match checked {
        Err((minute, reason)) => keep(&mut records, &rejected.decision(minute, initiator, reason)),
        Ok(candidate) => typed(cell, runtime, records, candidate),
    }
}

/// An information reaction the social policy considers after a contact
/// (`dispatch::consider_contact`): a person who has heard a false claim from someone in the same
/// zone may ask for it to be verified, a typed interaction of both.
pub fn consider_information_reaction<C: WorldCell>(
    cell: &C,
    runtime: &mut Runtime,
    mut records: Records<'_>,
    agent_id: &str,
    participant: &str,
    message_id: &str,
    trigger_event_id: &str,
) -> Result<()> {
    let rejected = Rejected {
        request_id: None,
        trigger: Some("unverified_claim_contact"),
        interaction_type: "information_reaction",
        target_agent_id: Some(participant),
        trigger_id: Some(trigger_event_id),
    };
    let checked = cell.with(|world| -> Result<std::result::Result<Candidate, (i64, &'static str)>> {
        let minute = world.minute_index()?;
        let (zone, other_zone) = {
            let rows = world.rows()?;
            let (Some(agent), Some(other)) = (rows.agents.get(agent_id), rows.agents.get(participant)) else {
                return Err(Error::Value("information reaction requires two different known Agent IDs".to_owned()));
            };
            if agent_id == participant {
                return Err(Error::Value("information reaction requires two different known Agent IDs".to_owned()));
            }
            (agent.zone_id.clone(), other.zone_id.clone())
        };
        let held = Holding::of_world(world)?.held_dict(INFORMATION, OTHER_DOCUMENT);
        let Some(state) = kit::held_row::<InformationRow>(world, INFORMATION, &held)? else {
            return Ok(Err((minute, "information_state_unavailable")));
        };
        let (Some(message), Some(belief)) = (state.messages.get(message_id), state.beliefs.get(agent_id).and_then(|beliefs| beliefs.get(message_id))) else {
            return Ok(Err((minute, "information_claim_not_current")));
        };
        if !message.parses() || !belief.parses() {
            return Err(Error::unsupported("a message or a belief the contract does not take"));
        }
        if &*message.truth_status != "false" || &*belief.belief != "heard" {
            return Ok(Err((minute, "not_an_unverified_false_claim")));
        }
        if zone != other_zone {
            return Ok(Err((minute, "participants_not_in_same_zone")));
        }
        Ok(Ok(Candidate {
            interaction_type: "information_reaction",
            trigger: "unverified_claim_contact",
            initiator: agent_id.to_owned(),
            participants: vec![agent_id.to_owned(), participant.to_owned()],
            trigger_id: trigger_event_id.to_owned(),
            request_id: None,
            target_agent_id: Some(participant.to_owned()),
            zone_id: zone.to_string(),
            relationship: "same_zone_contact",
            observed_facts: map([("belief_status", text("heard")), ("claim_is_locally_unverified", V::Bool(true))]),
            allowed_actions: &["seek_claim_verification", "no_interaction"],
            reason_codes: &["heard_through_contact", "request_verification"],
            event_payload: map_of([
                ("interaction_type", text("information_reaction")),
                ("agent_id", text(agent_id)),
                ("participant_agent_id", text(participant)),
                ("zone_id", text(&zone)),
                ("message_id", text(message_id)),
                ("trigger_event_id", text(trigger_event_id)),
            ]),
        }))
    })??;
    match checked {
        Err((minute, reason)) => keep(&mut records, &rejected.decision(minute, agent_id, reason)),
        Ok(candidate) => typed(cell, runtime, records, candidate),
    }
}

/// A typed interaction the policy may ask about (what `typed` takes).
struct Candidate {
    interaction_type: &'static str,
    trigger: &'static str,
    initiator: String,
    participants: Vec<String>,
    trigger_id: String,
    request_id: Option<String>,
    target_agent_id: Option<String>,
    zone_id: String,
    relationship: &'static str,
    observed_facts: V,
    allowed_actions: &'static [&'static str],
    reason_codes: &'static [&'static str],
    event_payload: IndexMap<Arc<str>, V>,
}

/// A typed interaction considered: offline, a response due past the Run's end, and a participant
/// who has left reject it; then the participants' state is captured into the event's payload, and
/// the day's cap and each participant's cap and cooldown reject it. Otherwise the policy asks with a
/// candidate ID made from what it asks about, counts the attempt for every participant, and
/// schedules the chosen action as an event of that ID unless it is `no_interaction` or the answer
/// fell back.
fn typed<C: WorldCell>(cell: &C, runtime: &mut Runtime, mut records: Records<'_>, mut candidate: Candidate) -> Result<()> {
    let limits = runtime.limits.social;
    let offline = runtime.link.offline();
    let interaction_type = candidate.interaction_type;
    let mut participants = candidate.participants.clone();
    participants.sort();
    participants.dedup();
    let planned = cell.with(|world| -> Result<std::result::Result<(i64, i64, String, Task), (i64, &'static str)>> {
        let minute = world.minute_index()?;
        let due = minute + limits.event_delay_minutes;
        if offline {
            return Ok(Err((minute, "offline_rule_mode")));
        }
        if due >= world.duration_minutes()? {
            return Ok(Err((minute, "scenario_deadline")));
        }
        {
            let rows = world.rows()?;
            if participants.is_empty() || participants.iter().any(|id| rows.agents.get(id.as_str()).is_none()) {
                return Ok(Err((minute, "participant_state_changed")));
            }
        }
        let ids: Vec<&str> = participants.iter().map(String::as_str).collect();
        candidate.event_payload.insert(Arc::from("conversation_state_context"), capture_context(world, &ids)?);
        let state = SocialRuntime::read(world)?;
        let day = minute.div_euclid(1440);
        if count(&state.attempts_by_day, &day.to_string()) >= limits.global_per_day {
            return Ok(Err((minute, "global_daily_cap")));
        }
        for id in &participants {
            if count(&state.attempts_by_agent_day, &format!("{day}:{id}")) >= limits.per_agent_per_day {
                return Ok(Err((minute, "agent_daily_cap")));
            }
            if let Some(prior) = state.last_attempt(id) {
                if minute - prior < limits.cooldown_minutes {
                    return Ok(Err((minute, "cooldown")));
                }
            }
        }
        let (proposal_id, task) = social_task(world, runtime, &candidate, &participants, minute, due)?;
        Ok(Ok((minute, due, proposal_id, task)))
    })??;
    let base = Rejected {
        request_id: candidate.request_id.as_deref(),
        trigger: Some(candidate.trigger),
        interaction_type,
        target_agent_id: candidate.target_agent_id.as_deref(),
        trigger_id: Some(&candidate.trigger_id),
    };
    let (minute, due, proposal_id, task) = match planned {
        Err((minute, reason)) => return keep(&mut records, &base.decision(minute, &candidate.initiator, reason)),
        Ok(planned) => planned,
    };
    let (reply, answer) = runtime.ask(minute, &task)?;
    let field = |name: &str| answer.get(name).and_then(Value::as_str).map(str::to_owned).ok_or_else(|| Error::Runtime(format!("an answer without its {name}")));
    let (action, reason_short) = (field("action")?, field("reason_short")?);
    if field("event_id")? != proposal_id {
        return Err(Error::Value("social proposal event_id does not match its candidate".to_owned()));
    }
    let ids: Vec<&str> = participants.iter().map(String::as_str).collect();
    let scheduled = cell.with(|world| -> Result<bool> {
        count_attempts(world, minute, &ids)?;
        if reply.fallback_used || action == "no_interaction" {
            return Ok(false);
        }
        let events = Events::of(world)?;
        let sequence = events.allocate_sequence()?;
        let mut payload = candidate.event_payload.clone();
        payload.insert(Arc::from("action"), text(&action));
        payload.insert(Arc::from("reason_short"), text(&reason_short));
        let provenance = provenance(SOCIAL_INTERACTIONS_SOURCE, &task, &reply);
        schedule(&events, &proposal_id, due, sequence, payload, &candidate.trigger_id, provenance)?;
        Ok(true)
    })??;
    let reason = match (scheduled, reply.fallback_used) {
        (_, true) => format!("rule_fallback:{}", reply.outcome),
        (false, false) => "model_selected_no_interaction".to_owned(),
        (true, false) => "accepted_and_scheduled".to_owned(),
    };
    let decision = Decision {
        action: Some(&action),
        accepted: !reply.fallback_used,
        reason,
        reason_short: Some(&reason_short),
        event_id: scheduled.then_some(proposal_id.as_str()),
        call: Some((&task, &reply)),
        ..base.decision(minute, &candidate.initiator, "")
    };
    keep(&mut records, &decision)
}

/// The question `typed` asks of a candidate at `minute`: its proposal ID, made from what it asks
/// about, and the task with its input, schema and fallback.
fn social_task(world: &World, runtime: &mut Runtime, candidate: &Candidate, participants: &[String], minute: i64, due: i64) -> Result<(String, Task)> {
    let interaction_type = candidate.interaction_type;
    let agent_ids = V::List(participants.iter().map(|id| text(id)).collect());
    let identity = map([
        ("interaction_type", text(interaction_type)),
        ("minute", V::Int(minute)),
        ("trigger_id", text(&candidate.trigger_id)),
        ("agent_ids", agent_ids.clone()),
        ("zone_id", text(&candidate.zone_id)),
        ("relationship", text(candidate.relationship)),
        ("event_context", V::Map(candidate.event_payload.clone())),
    ]);
    let digest = crate::checkpoint::hex(&<sha2::Sha256 as sha2::Digest>::digest(identity.to_json()?));
    let proposal_id = format!("social-interaction-{}", &digest[..16]);
    let mut reason_codes: Vec<V> = candidate.reason_codes.iter().map(|code| text(code)).collect();
    reason_codes.push(text("rule_fallback"));
    let allowed = V::List(candidate.allowed_actions.iter().map(|action| text(action)).collect());
    let schema = object_schema(&[
        ("event_id", map([("type", text("string")), ("enum", V::List(vec![text(&proposal_id)]))])),
        ("action", map([("type", text("string")), ("enum", allowed.clone())])),
        ("reason_short", map([("type", text("string")), ("enum", V::List(reason_codes))])),
    ]);
    let input = map([
        ("event_id", text(&proposal_id)),
        ("interaction_type", text(interaction_type)),
        ("minute", V::Int(minute)),
        ("due_minute", V::Int(due)),
        ("agent_ids", agent_ids),
        ("zone_id", text(&candidate.zone_id)),
        ("relationship", text(candidate.relationship)),
        ("observed_facts", candidate.observed_facts.clone()),
        ("allowed_choices", allowed),
        ("reason_codes", V::List(candidate.reason_codes.iter().map(|code| text(code)).collect())),
        (
            "constraints",
            V::List(
                [
                    "Select only a listed action and reason code.",
                    "Use no conversation text and do not add facts.",
                    "Choose no_interaction when context does not support an action.",
                    "Echo the supplied event_id exactly.",
                ]
                .iter()
                .map(|item| text(item))
                .collect(),
            ),
        ),
    ]);
    // `json.dumps({"event_id": ..., "action": ..., "reason_short": ...})`, in that order.
    let mut fallback = Vec::with_capacity(96);
    fallback.extend_from_slice(b"{\"event_id\":");
    crate::json::write_json_string(&mut fallback, &proposal_id);
    fallback.extend_from_slice(br#","action":"no_interaction","reason_short":"rule_fallback"}"#);
    let task = Task {
        task: llm::SOCIAL_INTERACTION,
        stage: STAGE_RUNTIME,
        prompt_version: format!("{SOCIAL_POLICY_VERSION}-{interaction_type}"),
        schema_version: 1,
        input_text: json_text(&input)?,
        state_hash: runtime.state_hash(world)?,
        output_schema: schema,
        fallback_text: String::from_utf8(fallback).map_err(|_| Error::Runtime("a fallback that is not UTF-8".to_owned()))?,
        // Runtime proposals include a long candidate ID and strict reason code; 40 tokens can be
        // consumed before the JSON object is complete.
        max_output_tokens: 128,
        input_byte_limit: None,
    };
    Ok((proposal_id, task))
}

/// After a call: the day's attempts, and each participant's attempts of the day and last attempt,
/// counted; one decision more.
fn count_attempts(world: &mut World, minute: i64, participants: &[&str]) -> Result<()> {
    let mut state = SocialRuntime::read(world)?;
    let day = minute.div_euclid(1440);
    let day_key = day.to_string();
    let attempts = count(&state.attempts_by_day, &day_key);
    state.attempts_by_day.insert(Arc::from(day_key.as_str()), attempts + 1);
    for id in participants {
        let key = format!("{day}:{id}");
        let attempts = count(&state.attempts_by_agent_day, &key);
        state.attempts_by_agent_day.insert(Arc::from(key.as_str()), attempts + 1);
        state.last_attempt_minute.insert(Arc::from(*id), LastAttempt::Minute(minute));
    }
    state.decision_count += 1;
    put_value(world, SOCIAL_STATE, state.value())
}

// ---- The runtime states ----------------------------------------------------------------------------

/// A table of counts by key, in the order the state holds them.
type Counts = IndexMap<Arc<str>, i64>;

/// `state[table].get(key, 0)` of a table of counts.
fn count(table: &Counts, key: &str) -> i64 {
    table.get(key).copied().unwrap_or(0)
}

/// The names of a state in the order it holds them, where it has exactly these names (in any
/// order), else None: the state is written back in its order.
fn names_of(state: &IndexMap<Arc<str>, V>, names: &[&'static str]) -> Option<Vec<&'static str>> {
    if state.len() != names.len() {
        return None;
    }
    state.keys().map(|key| names.iter().copied().find(|name| **key == **name)).collect()
}

/// A table of a state, read where it holds only ints.
fn counts_of(table: &IndexMap<Arc<str>, V>) -> Option<Counts> {
    table
        .iter()
        .map(|(key, count)| match count {
            V::Int(count) => Some((key.clone(), *count)),
            _ => None,
        })
        .collect()
}

fn counts_value(counts: &Counts) -> V {
    V::Map(counts.iter().map(|(key, count)| (key.clone(), V::Int(*count))).collect())
}

/// `model_state["support_priority_runtime_v1"]`: the priority's attempts by day and its skips by
/// reason.
struct PriorityRuntime {
    /// The state's names in its order.
    names: Vec<&'static str>,
    attempts_by_day: Counts,
    skipped_by_reason: Counts,
}

impl PriorityRuntime {
    const NAMES: [&'static str; 3] = ["schema_version", "attempts_by_day", "skipped_by_reason"];

    /// The support priority's state: the state, or a new one (`setdefault`, stored by the caller's
    /// write); an error for one of another shape.
    fn read(world: &World) -> Result<Self> {
        let Some(value) = world.slots.try_value(PRIORITY_STATE)? else {
            return Ok(Self { names: Self::NAMES.to_vec(), attempts_by_day: Counts::new(), skipped_by_reason: Counts::new() });
        };
        let incompatible = || Error::Value("saved support priority runtime state is incompatible".to_owned());
        let V::Map(state) = &*value else {
            return Err(incompatible());
        };
        let Some(names) = names_of(state, &Self::NAMES).filter(|_| matches!(state.get("schema_version"), Some(V::Int(1)))) else {
            return Err(incompatible());
        };
        let table = |name: &str| {
            match state.get(name) {
                Some(V::Map(table)) => counts_of(table).ok_or(()),
                _ => Err(()),
            }
            .map_err(|()| Error::Value(format!("saved support priority runtime {name} must map to integers")))
        };
        let attempts_by_day = table("attempts_by_day")?;
        let skipped_by_reason = table("skipped_by_reason")?;
        Ok(Self { names, attempts_by_day, skipped_by_reason })
    }

    /// The state as it is stored, in its order.
    fn value(&self) -> V {
        V::Map(
            self.names
                .iter()
                .map(|name| {
                    let value = match *name {
                        "schema_version" => V::Int(1),
                        "attempts_by_day" => counts_value(&self.attempts_by_day),
                        _ => counts_value(&self.skipped_by_reason),
                    };
                    (Arc::from(*name), value)
                })
                .collect(),
        )
    }
}

/// A person's last attempt of the social policy: the minute the policy writes, or another value
/// the state held, which reads as none and is kept as it is.
enum LastAttempt {
    Minute(i64),
    Other(V),
}

/// `model_state["social_runtime_v1"]`: the social policy's attempts by day, by person and day, each
/// person's last attempt, and how many decisions it made.
struct SocialRuntime {
    /// The state's names in its order.
    names: Vec<&'static str>,
    attempts_by_day: Counts,
    attempts_by_agent_day: Counts,
    last_attempt_minute: IndexMap<Arc<str>, LastAttempt>,
    decision_count: i64,
}

impl SocialRuntime {
    const NAMES: [&'static str; 5] = ["schema_version", "attempts_by_day", "attempts_by_agent_day", "last_attempt_minute", "decision_count"];

    /// The social policy's state: the state, or a new one, which is stored at once (`setdefault`);
    /// an error for one of another shape.
    fn read(world: &mut World) -> Result<Self> {
        let found = world.slots.try_value(SOCIAL_STATE)?.map(|value| Self::of(&value));
        match found {
            Some(state) => state,
            None => {
                let state = Self {
                    names: Self::NAMES.to_vec(),
                    attempts_by_day: Counts::new(),
                    attempts_by_agent_day: Counts::new(),
                    last_attempt_minute: IndexMap::new(),
                    decision_count: 0,
                };
                put_value(world, SOCIAL_STATE, state.value())?;
                Ok(state)
            }
        }
    }

    fn of(value: &V) -> Result<Self> {
        let incompatible = || Error::Value("saved social runtime state is incompatible".to_owned());
        let V::Map(state) = value else {
            return Err(incompatible());
        };
        let Some(names) = names_of(state, &Self::NAMES).filter(|_| matches!(state.get("schema_version"), Some(V::Int(1)))) else {
            return Err(incompatible());
        };
        let object = |name: &str| match state.get(name) {
            Some(V::Map(table)) => Ok(table),
            _ => Err(Error::Value(format!("saved social runtime {name} must be an object"))),
        };
        let by_day = object("attempts_by_day")?;
        let by_agent_day = object("attempts_by_agent_day")?;
        let last = object("last_attempt_minute")?;
        let Some(V::Int(decision_count)) = state.get("decision_count") else {
            return Err(Error::Value("saved social runtime decision_count must be an integer".to_owned()));
        };
        let (Some(attempts_by_day), Some(attempts_by_agent_day)) = (counts_of(by_day), counts_of(by_agent_day)) else {
            return Err(incompatible());
        };
        let last_attempt_minute = last
            .iter()
            .map(|(id, value)| {
                let last = match value {
                    V::Int(minute) => LastAttempt::Minute(*minute),
                    other => LastAttempt::Other(other.clone()),
                };
                (id.clone(), last)
            })
            .collect();
        Ok(Self { names, attempts_by_day, attempts_by_agent_day, last_attempt_minute, decision_count: *decision_count })
    }

    /// `state["last_attempt_minute"].get(id)`, where it is an int.
    fn last_attempt(&self, id: &str) -> Option<i64> {
        match self.last_attempt_minute.get(id) {
            Some(LastAttempt::Minute(minute)) => Some(*minute),
            _ => None,
        }
    }

    /// The state as it is stored, in its order.
    fn value(&self) -> V {
        V::Map(
            self.names
                .iter()
                .map(|name| {
                    let value = match *name {
                        "schema_version" => V::Int(1),
                        "attempts_by_day" => counts_value(&self.attempts_by_day),
                        "attempts_by_agent_day" => counts_value(&self.attempts_by_agent_day),
                        "last_attempt_minute" => V::Map(
                            self.last_attempt_minute
                                .iter()
                                .map(|(id, last)| {
                                    let value = match last {
                                        LastAttempt::Minute(minute) => V::Int(*minute),
                                        LastAttempt::Other(value) => value.clone(),
                                    };
                                    (id.clone(), value)
                                })
                                .collect(),
                        ),
                        _ => V::Int(self.decision_count),
                    };
                    (Arc::from(*name), value)
                })
                .collect(),
        )
    }
}

// ---- Helpers --------------------------------------------------------------------------------------

/// A `social_interaction` event scheduled on the world's queue: endogenous, due at `due`, priority
/// 10, with the payload, the policy version as its source, `cause` as its causation and the
/// provenance.
fn schedule(
    events: &Events<'_>,
    event_id: &str,
    due: i64,
    sequence: i64,
    payload: IndexMap<Arc<str>, V>,
    cause: &str,
    provenance: IndexMap<Arc<str>, V>,
) -> Result<()> {
    let row = EventRow {
        event_id: Arc::from(event_id),
        kind: EventKind::Endogenous,
        due_minute: due,
        priority: EVENT_PRIORITY,
        sequence,
        event_type: Arc::from("social_interaction"),
        payload: V::Map(payload),
        source_id: Some(Arc::from(SOCIAL_POLICY_VERSION)),
        causation_id: Some(Arc::from(cause)),
        provenance: V::Map(provenance),
    }
    .checked()
    .ok_or_else(|| Error::Value("the social interaction is not an event".to_owned()))?;
    events.schedule(row).map_err(Error::from)
}

/// The provenance of an event the policy schedules: the source, the policy, the model and the
/// prompt's version, and the input's hash.
fn provenance(source: &str, task: &Task, reply: &Reply) -> IndexMap<Arc<str>, V> {
    map_of([
        ("source_type", text("assumption")),
        ("source_id", text(source)),
        ("policy_version", text(SOCIAL_POLICY_VERSION)),
        ("model_id", reply.model_id.as_deref().map_or(V::Null, text)),
        ("prompt_version", text(&task.prompt_version)),
        ("input_hash", text(&task.input_hash())),
    ])
}

/// `model_state[name] = value`: the namespace's slot (added where the world has none), its row the
/// value, with a new stamp.
fn put_value(world: &mut World, name: &str, value: V) -> Result<()> {
    if world.slots.get(name).is_none() {
        world.slots.add(Arc::from(name), Place::Value)?;
    }
    let slot = world.slots.get(name).ok_or_else(|| Error::Runtime("a namespace without its slot".to_owned()))?;
    let mut slot = slot.try_borrow_mut()?;
    slot.row = Some(Box::new(ValueRow(value)));
    slot.taint = None;
    slot.touch();
    Ok(())
}

fn day_key(minute: i64) -> String {
    minute.div_euclid(1440).to_string()
}

fn object_schema(properties: &[(&str, V)]) -> V {
    map([
        ("type", text("object")),
        ("properties", V::Map(properties.iter().map(|(name, schema)| (Arc::from(*name), schema.clone())).collect())),
        ("required", V::List(properties.iter().map(|(name, _)| text(name)).collect())),
        ("additionalProperties", V::Bool(false)),
    ])
}

fn json_text(value: &V) -> Result<String> {
    String::from_utf8(value.to_json()?).map_err(|_| Error::Runtime("a prompt that is not UTF-8".to_owned()))
}

fn text(value: &str) -> V {
    V::Str(Arc::from(value))
}

fn map_of<const N: usize>(entries: [(&str, V); N]) -> IndexMap<Arc<str>, V> {
    entries.into_iter().map(|(name, value)| (Arc::from(name), value)).collect()
}

fn map<const N: usize>(entries: [(&str, V); N]) -> V {
    V::Map(map_of(entries))
}
