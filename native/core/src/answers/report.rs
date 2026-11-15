//! `GET …/branches/{branch}/report` (#266): the report the server writes when an offline Run of
//! the screen completes (the review with the offline router, which falls back to the empty
//! review), or `{"status":"not_generated"}` before. A store keeps the report it made as the
//! branch's `report.json`.
//!
//! Two steps: `report_answer` checks the request and answers from what is kept (the report made
//! before, or that there is none yet); where it gives None, the caller restores the branch's Run
//! at its last minute and `build_report` makes the report, which the caller keeps.

use serde_json::{json, Map, Value};

use super::asking;
use super::query::no_query;
use super::records::RunFiles;
use super::{js_safe_integer, list, sha256_hex, string, write_value, Answer, Failure, Obj};
use crate::error::Error;
use crate::history::review_metrics;
use crate::llm::{Task, RUN_REVIEW};
use crate::plain::PlainRun;
use crate::world::WorldCell;

// The assumptions the report lists.
const ASSUMPTION_IDS: [&str; 26] = [
    "ASM-SYNTHETIC-PROFILES-V1",
    "ASM-SIMPLE-PSYCHOLOGY-RATES-V1",
    "ASM-H04-CAUSAL-STRESS-V1",
    "ASM-UTILITY-BEHAVIOR-V1",
    "ASM-CORE-MODEL-SUITE-V2",
    "ASM-SLEEP-AWARE-BEHAVIOR-V2",
    "ASM-PRIORITY-RULE-BEHAVIOR-V1",
    "ASM-TOILET-SERVICE-DURATION-V1",
    "ASM-DAY3-TOILET-OUTAGE-V1",
    "ASM-SPACE-AND-STAFF-CAPACITY-V1",
    "ASM-SOCIAL-PROPAGATION-V1",
    "ASM-INFORMATION-SPREAD-V1",
    "ASM-STAFF-SUPPORT-LIFECYCLE-V1",
    "ASM-ZONE-ROUTING-V1",
    "ASM-EVENT-TRIGGERED-SOCIAL-LIMITS-V1",
    "ASM-SUPPORT-PRIORITY-LIMITS-V1",
    "ASM-EVENT-TRIGGERED-SOCIAL-INTERACTIONS-V1",
    "ASM-SYNTHETIC-ZONE-DISPLAY-V1",
    "ASM-H02-RESOURCE-SERVICE-V1",
    "ASM-H02-TEST-INITIAL-STOCK-V1",
    "ASM-H02-QUEUE-STALLS-V1",
    "ASM-H03-OPERATIONAL-EVENTS-V1",
    "ASM-H05-OPERATIONAL-CAPACITY-V1",
    "ASM-H05-OPERATIONAL-STOCK-V1",
    "ASM-H05-OPERATIONAL-SCHEDULE-V1",
    "ASM-H05-INTERVENTION-V1",
];
const EVENT_TYPE_ALIASES: [(&str, &str); 19] = [
    ("facility_outage", "facility_outage"),
    ("facility_recovery", "facility_recovery"),
    ("water_outage", "water_outage"),
    ("water_restored", "water_restored"),
    ("resource_delivery", "resource_delivery"),
    ("delivery_delayed", "delivery_delayed"),
    ("staff_unavailable", "staff_unavailable"),
    ("staff_shift_change", "staff_shift_change"),
    ("power_outage", "power_outage"),
    ("power_restored", "power_restored"),
    ("equipment_failure", "equipment_failure"),
    ("equipment_repaired", "equipment_repaired"),
    ("extreme_heat", "extreme_heat"),
    ("extreme_cold", "extreme_cold"),
    ("social_interaction", "social_interaction"),
    ("toilet_service_complete", "completion"),
    ("staff_support_complete", "completion"),
    ("support_request_timeout", "completion"),
    ("agent_travel_complete", "completion"),
];
const COMPLETIONS: [&str; 4] = ["toilet_service_complete", "staff_support_complete", "support_request_timeout", "agent_travel_complete"];
const PROMPT_CONSTRAINTS: [&str; 8] = [
    "Do not make claims about real people or real-world outcomes.",
    "Only reference metric IDs, event references and assumption IDs supplied below.",
    "Observed metric direction must match the supplied values exactly.",
    "Event classifications must match their recorded type and outcome.",
    "Hypotheses must be labeled as hypotheses and include evidence references.",
    "Return at most 8 observed KPIs, 8 event observations, 3 hypotheses, and 3 recommendations.",
    "Prefer the most decision-relevant recorded evidence; do not repeat a metric or event.",
    "Return only the required JSON object. Do not include prose or numbers.",
];
// The markdown of a rule fallback with the empty review (`render_markdown`).
const FALLBACK_MARKDOWN: &str = "# Evidence-bound Opus review\n\n- Status: `rule_fallback`\n\nOpus review was unavailable; the recorded run evidence below \
remains authoritative.\n\n## Observed evidence\n\nNo structured observations were returned.\n\n## Assumptions\n\nNo assumption references were \
selected.\n\n## Hypotheses\n\nNo hypotheses were selected.\n\n## Recommendations\n\nNo recommendations were selected.\n\n## Limitations\n\nThis \
review summarizes recorded synthetic simulation outputs. It does not establish real-world safety, behavior, or causal effects.\n";

/// `GET …/branches/{branch}/report`, from what is kept: the query (empty), the ids, the branch's
/// record (`branch.json`) and the report kept (`report.json`), each None where missing. The kept
/// report as it is, `{"status":"not_generated"}` for a branch not completed, or None: make it
/// (`build_report`).
pub fn report_answer(query: &str, project_id: &str, run_id: &str, branch_id: &str, branch: Option<&[u8]>, kept: Option<&[u8]>) -> Answer<Option<Vec<u8>>> {
    no_query(query)?;
    let files = RunFiles { run: None, branches: branch.map(|bytes| vec![(branch_id.to_owned(), bytes.to_vec())]).unwrap_or_default() };
    let record = files.branch_record(project_id, run_id, branch_id)?;
    if let Some(kept) = kept {
        return Ok(Some(super::js_trim_end(&String::from_utf8_lossy(kept)).as_bytes().to_vec()));
    }
    let completed = record.get("status").and_then(Value::as_str) == Some("completed");
    if !completed || record.get("checkpoint_hash").is_none_or(Value::is_null) {
        return Ok(Some(b"{\"status\":\"not_generated\"}".to_vec()));
    }
    Ok(None)
}

fn is_object(value: Option<&Value>) -> Option<&Map<String, Value>> {
    value.and_then(Value::as_object)
}

/// A field of a value as JS reads `value.field` (None where it has none); an error for null, as
/// JS throws reading a field of null.
fn field<'a>(value: &'a Value, name: &str) -> Answer<Option<&'a Value>> {
    match value {
        Value::Null => Err(Failure::Error(Error::Type(format!("Cannot read properties of null (reading '{name}')")))),
        Value::Object(object) => Ok(object.get(name)),
        _ => Ok(None),
    }
}

/// `scenario_assumption_ids(scenario)` of the Scenario's JSON (its canonical JSON holds every field).
pub fn scenario_assumption_ids(scenario: &Value) -> Answer<Vec<String>> {
    let empty = Map::new();
    let top = scenario.as_object().unwrap_or(&empty);
    let configs = is_object(top.get("model_configs")).unwrap_or(&empty);
    let list = |name: &str| top.get(name).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
    let mut ids: Vec<&str> = Vec::new();
    let mut synthetic = false;
    for profile in list("profiles") {
        let kind = match field(profile, "provenance")? {
            Some(provenance) => provenance.as_object().and_then(|object| object.get("source_kind")),
            None => None,
        };
        if kind.is_none_or(Value::is_null) || kind.and_then(Value::as_str) == Some("synthetic") {
            synthetic = true;
            break;
        }
    }
    if synthetic {
        ids.push("ASM-SYNTHETIC-PROFILES-V1");
    }
    ids.extend(["ASM-SIMPLE-PSYCHOLOGY-RATES-V1", "ASM-H04-CAUSAL-STRESS-V1"]);
    let policy = is_object(configs.get("behavior")).and_then(|behavior| behavior.get("policy")).and_then(Value::as_str);
    ids.push(match policy {
        Some("priority-rule-v1") => "ASM-PRIORITY-RULE-BEHAVIOR-V1",
        Some("sleep-aware-utility-softmax-v2") => "ASM-SLEEP-AWARE-BEHAVIOR-V2",
        _ => "ASM-UTILITY-BEHAVIOR-V1",
    });
    let mut toilet = false;
    for facility in list("facilities") {
        // `String(facility.kind).toLowerCase().includes("toilet")`.
        if field(facility, "kind")?.and_then(Value::as_str).is_some_and(|kind| kind.to_lowercase().contains("toilet")) {
            toilet = true;
            break;
        }
    }
    if toilet {
        ids.push("ASM-TOILET-SERVICE-DURATION-V1");
    }
    let (zones, staff, facilities) = (!list("zones").is_empty(), !list("staff").is_empty(), !list("facilities").is_empty());
    if zones || staff || facilities {
        ids.push("ASM-SPACE-AND-STAFF-CAPACITY-V1");
    }
    if staff {
        ids.push("ASM-STAFF-SUPPORT-LIFECYCLE-V1");
    }
    if zones {
        ids.push("ASM-ZONE-ROUTING-V1");
    }
    if configs.contains_key("social_propagation") {
        ids.push("ASM-SOCIAL-PROPAGATION-V1");
    }
    if is_object(configs.get("information_spread")).and_then(|information| information.get("enabled")) == Some(&Value::Bool(true)) {
        ids.push("ASM-INFORMATION-SPREAD-V1");
    }
    ids.push("ASM-EVENT-TRIGGERED-SOCIAL-LIMITS-V1");
    if staff {
        ids.push("ASM-SUPPORT-PRIORITY-LIMITS-V1");
    }
    ids.push("ASM-EVENT-TRIGGERED-SOCIAL-INTERACTIONS-V1");
    let outage = "ASM-DAY3-TOILET-OUTAGE-V1";
    let mut scheduled = false;
    for event in list("scheduled_events") {
        let is_outage = field(event, "event_type")?.and_then(Value::as_str) == Some("facility_outage");
        let source = field(event, "source_id")?.and_then(Value::as_str) == Some(outage);
        let provenance =
            field(event, "provenance")?.and_then(Value::as_object).and_then(|object| object.get("source_id")).and_then(Value::as_str) == Some(outage);
        if is_outage && (source || provenance) {
            scheduled = true;
            break;
        }
    }
    if scheduled {
        ids.push(outage);
    }
    let mut ids: Vec<String> = ids.into_iter().map(str::to_owned).collect();
    if let Some(Value::Array(declared)) = is_object(top.get("provenance")).and_then(|provenance| provenance.get("model_assumption_ids")) {
        ids.extend(declared.iter().filter_map(Value::as_str).filter(|id| ASSUMPTION_IDS.contains(id)).map(str::to_owned));
    }
    Ok(ids)
}

/// The log's record `index` as its canonical JSON (the record's JSON form).
fn log_record(run: &PlainRun, index: usize) -> Answer<Vec<u8>> {
    let mut out = Vec::new();
    let written = run.loaded.cell.with(|world| world.log().and_then(|log| log.write_record(index, &mut out)))??;
    if !written {
        return Err(Failure::Error(Error::Value(format!("the log holds no record {index}"))));
    }
    Ok(out)
}

/// A number of the answer: a safe integer, else the browser's error.
fn integer_json(value: &Value) -> Answer<Vec<u8>> {
    match value {
        Value::Number(_) => js_safe_integer(value)
            .map(|number| number.to_string().into_bytes())
            .ok_or_else(|| Failure::Error(Error::Value("not an integer of an answer".to_owned()))),
        other => {
            let mut out = Vec::new();
            write_value(&mut out, other);
            Ok(out)
        }
    }
}

/// The evidence of a review of the Run at its last minute (`review_evidence`): the canonical JSON
/// of its parts, and the prompt's.
struct Evidence {
    /// `{"metrics", "events", "assumptions", "comparison_deltas", "event_id_by_ref", "generated_minute"}`.
    evidence: Vec<u8>,
    prompt: Vec<u8>,
    input_hash: String,
    minute: i64,
    state_hash: String,
}

fn review_evidence(run: &PlainRun, scenario: &[u8]) -> Answer<Evidence> {
    let samples = &run.host.samples;
    let count = samples.len();
    let (metrics, generated_minute) = match count.checked_sub(1) {
        Some(last) => {
            let (sample, _, _) = samples.sample(last)?.ok_or_else(|| Failure::Error(Error::Value("a sample is missing".to_owned())))?;
            let line = samples.line(last)?.unwrap_or_default();
            let minute = match serde_json::from_slice::<Value>(&line) {
                Ok(Value::Array(items)) if !items.is_empty() => integer_json(&items[0])?,
                _ => return Err(Failure::Error(Error::Value("a sample's line is not a list".to_owned()))),
            };
            (review_metrics(&sample)?, minute)
        }
        None => (b"{}".to_vec(), b"null".to_vec()),
    };
    // The review's events of the last 100 records: the critical ones, the last 20.
    let total = run.log_records()?;
    let mut critical = Vec::new();
    for index in total.saturating_sub(100)..total {
        let record: Value = serde_json::from_slice(&log_record(run, index)?).map_err(|error| Failure::Error(Error::Value(error.to_string())))?;
        let Some(event) = record.get("event").and_then(Value::as_object) else { continue };
        let status = record.get("status").and_then(Value::as_str);
        let kind = event.get("event_type").and_then(Value::as_str);
        let aliased = kind.is_some_and(|kind| EVENT_TYPE_ALIASES.iter().any(|(name, _)| *name == kind) && !COMPLETIONS.contains(&kind));
        if status == Some("failed") || status == Some("suppressed") || aliased {
            critical.push(record);
        }
    }
    let critical = &critical[critical.len().saturating_sub(20)..];
    let mut events = Vec::new();
    let mut event_id_by_ref = Obj::new();
    for (index, record) in critical.iter().enumerate() {
        let event = &record["event"];
        let Some(event_id) = event.get("event_id").and_then(Value::as_str).filter(|id| !id.is_empty()) else { continue };
        let reference = format!("E{:03}", index + 1);
        event_id_by_ref.str(&reference, event_id);
        let kind = event.get("event_type").and_then(Value::as_str);
        let alias = kind.and_then(|kind| EVENT_TYPE_ALIASES.iter().find(|(name, _)| *name == kind));
        let status = record.get("status").and_then(Value::as_str);
        let mut item = Obj::new();
        item.str("event_ref", &reference).raw("minute", integer_json(record.get("realized_minute").unwrap_or(&Value::Null))?);
        item.str("event_type", alias.map_or("other", |(name, _)| name));
        item.str("status", status.filter(|status| ["applied", "failed", "suppressed"].contains(status)).unwrap_or("other"));
        item.str(
            "classification",
            match status {
                Some("failed") => "failure",
                Some("suppressed") => "suppressed",
                _ => alias.map_or("other", |(_, class)| class),
            },
        );
        events.push(item.finish());
    }
    let events = list(events);
    let scenario: Value = serde_json::from_str(&String::from_utf8_lossy(scenario)).map_err(|error| Failure::Error(Error::Value(error.to_string())))?;
    let mut assumptions: Vec<String> = Vec::new();
    for id in scenario_assumption_ids(&scenario)? {
        if !assumptions.contains(&id) && ASSUMPTION_IDS.contains(&id.as_str()) {
            assumptions.push(id);
        }
    }
    assumptions.sort();
    let assumptions = list(assumptions.iter().map(|id| string(id)));
    let mut prompt = Obj::new();
    prompt.str("task", "Select evidence-bound review codes for one shelter simulation run.");
    prompt.raw("constraints", list(PROMPT_CONSTRAINTS.iter().map(|text| string(text))));
    prompt.raw("final_metrics", metrics.clone()).raw("comparison_deltas", b"{}".to_vec()).raw("events", events.clone());
    prompt.raw("assumption_ids", assumptions.clone()).bool("comparison_available", false);
    let prompt = prompt.finish();
    // `LLMTask.input_hash`: the sha256 of the input text's JSON.
    let input_hash = sha256_hex(&string(&String::from_utf8_lossy(&prompt)));
    let mut evidence = Obj::new();
    evidence.raw("metrics", metrics).raw("events", events).raw("assumptions", assumptions).raw("comparison_deltas", b"{}".to_vec());
    evidence.object("event_id_by_ref", event_id_by_ref).raw("generated_minute", generated_minute);
    Ok(Evidence { evidence: evidence.finish(), prompt, input_hash, minute: run.minute()?, state_hash: run.state_digest()? })
}

const EMPTY_REVIEW: [&str; 5] = ["observed", "event_observations", "assumptions", "hypotheses", "recommendations"];

fn empty_review() -> Vec<u8> {
    let mut empty = Obj::new();
    for name in EMPTY_REVIEW {
        empty.raw(name, b"[]".to_vec());
    }
    empty.finish()
}

/// The report's JSON: the review's `status`, its markdown and the review's record.
fn report_json(branch_id: &str, evidence: &Evidence, status: &str, markdown: &str, review: Vec<u8>, issue: Option<&str>) -> Vec<u8> {
    let mut result = Obj::new();
    result.int("schema_version", 1).str("status", status).str("prompt_version", "evidence-review-v1").str("input_hash", &evidence.input_hash);
    result.raw("evidence", evidence.evidence.clone()).raw("review", review).opt_str("validation_issue", issue);
    let mut report = Obj::new();
    report.int("schema_version", 1).str("branch_id", branch_id).int("minute", evidence.minute).str("state_hash", &evidence.state_hash);
    report.str("status", status).str("markdown", markdown).object("review", result);
    report.finish()
}

/// The report of the Run `run` at its last minute without an LLM (the rule fallback) and the
/// Scenario's JSON bytes, for the branch `branch_id`: its JSON.
pub fn build_report(run: &PlainRun, scenario: &[u8], branch_id: &str) -> Answer {
    let evidence = review_evidence(run, scenario)?;
    Ok(report_json(branch_id, &evidence, "rule_fallback", FALLBACK_MARKDOWN, empty_review(), None))
}

/// The review of a completed branch asked of an LLM (#463): the task
/// of the Run `run` at its last minute and the Scenario's JSON bytes; `Reviewing::report` reads
/// the link's reply into the report the server keeps.
pub fn review(run: &PlainRun, scenario: &[u8], branch_id: &str) -> Answer<Reviewing> {
    let evidence = review_evidence(run, scenario)?;
    let parsed: Value = serde_json::from_slice(&evidence.evidence).map_err(|error| Failure::Error(Error::Value(error.to_string())))?;
    let prompt: Value = serde_json::from_slice(&evidence.prompt).map_err(|error| Failure::Error(Error::Value(error.to_string())))?;
    let metric_ids: Vec<String> = parsed["metrics"].as_object().map(|metrics| metrics.keys().cloned().collect()).unwrap_or_default();
    let event_refs: Vec<String> = parsed["events"]
        .as_array()
        .map(|events| events.iter().filter_map(|event| event["event_ref"].as_str().map(str::to_owned)).collect())
        .unwrap_or_default();
    let assumptions: Vec<String> =
        parsed["assumptions"].as_array().map(|items| items.iter().filter_map(|item| item.as_str().map(str::to_owned)).collect()).unwrap_or_default();
    let schema = review_schema(&metric_ids, &event_refs, &assumptions);
    let state_hash = sha256_hex(&evidence.evidence);
    let mut task = asking::review_task(
        asking::Review {
            task: RUN_REVIEW,
            prompt_version: "evidence-review-v1",
            schema_version: 1,
            fallback_text: &String::from_utf8_lossy(&empty_review()),
            max_output_tokens: 2048,
        },
        &prompt,
        state_hash,
        &schema,
    )?;
    // The prompt as the offline report records its hash.
    task.input_text = String::from_utf8_lossy(&evidence.prompt).into_owned();
    Ok(Reviewing { branch_id: branch_id.to_owned(), evidence, parsed, task })
}

/// A review asked of an LLM.
pub struct Reviewing {
    branch_id: String,
    evidence: Evidence,
    parsed: Value,
    task: Task,
}

impl Reviewing {
    /// The task's JSON, for the link.
    pub fn task_json(&self) -> Answer {
        asking::task_json(&self.task)
    }

    /// The report of the link's reply (`llm::Reply`'s JSON; None where the call failed): the
    /// review's codes where they fit the evidence (`validated`), the empty review where they do
    /// not (`rejected`, with the issue) or where the link fell back (`rule_fallback`); its
    /// markdown rendered from the evidence and fixed vocabulary only.
    pub fn report(&self, reply: Option<&[u8]>) -> Answer {
        let read = match reply {
            Some(raw) => Some(asking::read_reply(&self.task, raw)?),
            None => None,
        };
        let Some(answer) = read.and_then(|read| read.answer) else {
            return Ok(report_json(&self.branch_id, &self.evidence, "rule_fallback", FALLBACK_MARKDOWN, empty_review(), None));
        };
        if let Some(issue) = validation_issue(&answer, &self.parsed) {
            let markdown = render_markdown("rejected", Some(&issue), &json!({}), &self.parsed);
            return Ok(report_json(&self.branch_id, &self.evidence, "rejected", &markdown, empty_review(), Some(&issue)));
        }
        let markdown = render_markdown("validated", None, &answer, &self.parsed);
        Ok(report_json(&self.branch_id, &self.evidence, "validated", &markdown, asking::canonical(&answer).into_bytes(), None))
    }
}

const HYPOTHESES: [(&str, &str); 5] = [
    ("branch_effect_is_descriptive", "The branch difference is descriptive and needs repeated-seed study."),
    ("capacity_may_contribute_to_wait", "Capacity may contribute to observed queue or wait values."),
    ("event_may_explain_metric_change", "A recorded event may be relevant to a metric difference."),
    ("staff_load_may_affect_support", "Staff load may affect support throughput."),
    ("stress_pattern_needs_validation", "The stress pattern needs validation against suitable data."),
];
const RECOMMENDATIONS: [(&str, &str); 5] = [
    ("calibrate_with_aggregate_data", "Calibrate assumptions only with suitable aggregate operational data."),
    ("compare_capacity_intervention", "Compare a capacity intervention against the same baseline conditions."),
    ("inspect_event_chain", "Inspect the cited event chain and its downstream state changes."),
    ("repeat_across_seeds", "Repeat the paired comparison across multiple seeds."),
    ("review_branch_differences", "Review the paired branch differences before drawing conclusions."),
];
const ASSUMPTION_LABELS: [(&str, &str); 26] = [
    ("ASM-SYNTHETIC-PROFILES-V1", "Synthetic profile distributions are assumptions."),
    ("ASM-SIMPLE-PSYCHOLOGY-RATES-V1", "Psychology rates are uncalibrated assumptions."),
    ("ASM-H04-CAUSAL-STRESS-V1", "Causal stress coefficients are bounded, uncalibrated assumptions."),
    ("ASM-UTILITY-BEHAVIOR-V1", "Behavior thresholds and utilities are rule assumptions."),
    ("ASM-CORE-MODEL-SUITE-V2", "Core model suite v2 is an internal uncalibrated operator combination."),
    ("ASM-SLEEP-AWARE-BEHAVIOR-V2", "Sleep-debt utility normalization is an uncalibrated behavioral assumption."),
    ("ASM-PRIORITY-RULE-BEHAVIOR-V1", "Priority-rule action selection and shared utilities are uncalibrated assumptions."),
    ("ASM-TOILET-SERVICE-DURATION-V1", "Toilet service duration bounds are assumptions."),
    ("ASM-DAY3-TOILET-OUTAGE-V1", "Day 3 outage timing and scale are a synthetic stress test."),
    ("ASM-SPACE-AND-STAFF-CAPACITY-V1", "Zone, staff and facility counts are illustrative inputs."),
    ("ASM-SOCIAL-PROPAGATION-V1", "Social propagation coefficients are experimental assumptions."),
    ("ASM-INFORMATION-SPREAD-V1", "Information contact and inquiry coefficients are uncalibrated experimental assumptions."),
    ("ASM-STAFF-SUPPORT-LIFECYCLE-V1", "Staff support durations and cooldowns are uncalibrated assumptions."),
    ("ASM-ZONE-ROUTING-V1", "Route selection is deterministic; fixture travel minutes are illustrative."),
    ("ASM-EVENT-TRIGGERED-SOCIAL-LIMITS-V1", "Stress trigger, cooldown and daily caps bound optional LLM proposals."),
    ("ASM-SUPPORT-PRIORITY-LIMITS-V1", "Stress trigger and daily cap bound optional LLM support-priority requests."),
    ("ASM-EVENT-TRIGGERED-SOCIAL-INTERACTIONS-V1", "Typed complaint, cooperation and information-verification effects are uncalibrated."),
    ("ASM-SYNTHETIC-ZONE-DISPLAY-V1", "Fallback map coordinates are presentation-only synthetic layout."),
    ("ASM-H02-RESOURCE-SERVICE-V1", "Resource service durations, quantities, need effects, thresholds, and utilities are assumptions."),
    ("ASM-H02-TEST-INITIAL-STOCK-V1", "H02 initial stock quantities are synthetic software test inputs."),
    ("ASM-H02-QUEUE-STALLS-V1", "Resource service queue stall reasons and retry order are software contracts."),
    ("ASM-H03-OPERATIONAL-EVENTS-V1", "Operational event schedules and outages are synthetic, uncalibrated assumptions."),
    ("ASM-H05-OPERATIONAL-CAPACITY-V1", "H05 facility, staffing and population capacities are illustrative inputs."),
    ("ASM-H05-OPERATIONAL-STOCK-V1", "H05 initial and delivered stock amounts are synthetic scenario quantities."),
    ("ASM-H05-OPERATIONAL-SCHEDULE-V1", "H05 event times and schedules are synthetic assumptions, not measured timelines."),
    ("ASM-H05-INTERVENTION-V1", "H05 interventions are simulated settings and do not estimate real-world effects."),
];

fn label(table: &[(&str, &'static str)], key: &str) -> &'static str {
    table.iter().find(|(name, _)| *name == key).map_or("", |(_, label)| *label)
}

/// The review's output schema for the evidence's metric ids, event references and assumptions.
fn review_schema(metric_ids: &[String], event_refs: &[String], assumptions: &[String]) -> Value {
    let refs = |values: &[String]| json!({"type": "array", "items": {"type": "string", "enum": values}});
    let mut classes: Vec<&str> = EVENT_TYPE_ALIASES.iter().map(|(_, class)| *class).chain(["failure", "suppressed", "other"]).collect();
    classes.sort_unstable();
    classes.dedup();
    let cited = |key: &str, names: Vec<&str>| {
        json!({
            "type": "array",
            "maxItems": 3,
            "items": {
                "type": "object",
                "properties": {
                    key: {"type": "string", "enum": names},
                    "metric_ids": refs(metric_ids),
                    "event_refs": refs(event_refs),
                    "assumption_ids": refs(assumptions),
                },
                "required": [key, "metric_ids", "event_refs", "assumption_ids"],
                "additionalProperties": false,
            },
        })
    };
    json!({
        "type": "object",
        "properties": {
            "observed": {
                "type": "array",
                "maxItems": 8,
                "items": {
                    "type": "object",
                    "properties": {
                        "metric_id": {"type": "string", "enum": metric_ids},
                        "direction": {"type": "string", "enum": ["zero", "nonzero", "increase", "decrease", "unchanged"]},
                    },
                    "required": ["metric_id", "direction"],
                    "additionalProperties": false,
                },
            },
            "event_observations": {
                "type": "array",
                "maxItems": 8,
                "items": {
                    "type": "object",
                    "properties": {
                        "event_ref": {"type": "string", "enum": event_refs},
                        "classification": {"type": "string", "enum": classes},
                    },
                    "required": ["event_ref", "classification"],
                    "additionalProperties": false,
                },
            },
            "assumptions": {"type": "array", "items": {"type": "string", "enum": assumptions}},
            "hypotheses": cited("hypothesis", HYPOTHESES.iter().map(|(name, _)| *name).collect()),
            "recommendations": cited("action", RECOMMENDATIONS.iter().map(|(name, _)| *name).collect()),
        },
        "required": ["observed", "event_observations", "assumptions", "hypotheses", "recommendations"],
        "additionalProperties": false,
    })
}

fn items<'v>(value: &'v Value, key: &str) -> &'v [Value] {
    value.get(key).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}

/// The review's checks against the evidence: the issue, or None where the codes fit the evidence.
fn validation_issue(review: &Value, evidence: &Value) -> Option<String> {
    for item in items(review, "observed") {
        let metric = item["metric_id"].as_str().unwrap_or_default();
        let value = evidence["metrics"].get(metric).and_then(super::js_number).unwrap_or(f64::NAN);
        match item["direction"].as_str().unwrap_or_default() {
            "zero" if value != 0.0 => return Some(format!("metric direction mismatch for {metric}")),
            "nonzero" if value == 0.0 => return Some(format!("metric direction mismatch for {metric}")),
            // The server's reviews have no paired comparison: a direction against one has no fact.
            "increase" | "decrease" | "unchanged" => return Some(format!("metric {metric} has no paired comparison fact")),
            _ => {}
        }
    }
    for item in items(review, "event_observations") {
        let reference = item["event_ref"].as_str().unwrap_or_default();
        let recorded = items(evidence, "events").iter().find(|event| event["event_ref"].as_str() == Some(reference));
        if recorded.and_then(|event| event["classification"].as_str()) != item["classification"].as_str() {
            return Some(format!("event classification mismatch for {reference}"));
        }
    }
    for item in items(review, "hypotheses").iter().chain(items(review, "recommendations")) {
        if items(item, "metric_ids").is_empty() && items(item, "event_refs").is_empty() && items(item, "assumption_ids").is_empty() {
            return Some("hypothesis or recommendation has no cited evidence".to_owned());
        }
    }
    None
}

/// Python's `format(value, "g")`.
pub fn format_g(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 { "inf".to_owned() } else { "-inf".to_owned() };
    }
    if value == 0.0 {
        return if value.is_sign_negative() { "-0".to_owned() } else { "0".to_owned() };
    }
    let scientific = format!("{value:.5e}");
    let (mantissa, exponent) = scientific.split_once('e').expect("an exponent");
    let exponent: i32 = exponent.parse().expect("an exponent's digits");
    let trim = |text: &str| -> String {
        if text.contains('.') {
            text.trim_end_matches('0').trim_end_matches('.').to_owned()
        } else {
            text.to_owned()
        }
    };
    if (-4..6).contains(&exponent) {
        trim(&format!("{value:.*}", (5 - exponent) as usize))
    } else {
        format!("{}e{}{:02}", trim(mantissa), if exponent < 0 { '-' } else { '+' }, exponent.abs())
    }
}

/// The review's markdown: fixed vocabulary and the evidence's facts, no text of the model's.
fn render_markdown(status: &str, issue: Option<&str>, review: &Value, evidence: &Value) -> String {
    let mut lines: Vec<String> = vec!["# Evidence-bound Opus review".to_owned(), String::new(), format!("- Status: `{status}`")];
    if status == "rejected" {
        lines.push(String::new());
        lines.push("The structured review was rejected because it did not match the supplied evidence.".to_owned());
        lines.push(format!("Validation issue: `{}`", issue.unwrap_or("None")));
    }
    let alias = |reference: &str| evidence["event_id_by_ref"].get(reference).and_then(Value::as_str).map(str::to_owned);
    lines.extend([String::new(), "## Observed evidence".to_owned(), String::new()]);
    let observed = items(review, "observed");
    let events = items(review, "event_observations");
    if observed.is_empty() {
        lines.push("No structured observations were returned.".to_owned());
    }
    for item in observed {
        let metric = item["metric_id"].as_str().unwrap_or_default();
        let value = evidence["metrics"].get(metric).and_then(super::js_number).unwrap_or(f64::NAN);
        lines.push(format!("- `{metric}` is {} in the final snapshot ({}).", item["direction"].as_str().unwrap_or_default(), format_g(value)));
    }
    for item in events {
        let reference = item["event_ref"].as_str().unwrap_or_default();
        let Some(event) = items(evidence, "events").iter().find(|event| event["event_ref"].as_str() == Some(reference)) else { continue };
        let mut minute = Vec::new();
        super::write_value(&mut minute, &event["minute"]);
        lines.push(format!(
            "- `{}` at minute {} is recorded as `{}` ({}).",
            alias(reference).unwrap_or_default(),
            String::from_utf8_lossy(&minute),
            event["classification"].as_str().unwrap_or_default(),
            event["status"].as_str().unwrap_or_default()
        ));
    }
    lines.extend([String::new(), "## Assumptions".to_owned(), String::new()]);
    let assumptions = items(review, "assumptions");
    if assumptions.is_empty() {
        lines.push("No assumption references were selected.".to_owned());
    }
    for id in assumptions {
        let id = id.as_str().unwrap_or_default();
        lines.push(format!("- `{id}`: {}", label(&ASSUMPTION_LABELS, id)));
    }
    let references = |item: &Value| -> String {
        let mut citations: Vec<String> = items(item, "metric_ids").iter().filter_map(Value::as_str).map(|metric| format!("`{metric}`")).collect();
        citations.extend(
            items(item, "event_refs").iter().filter_map(Value::as_str).filter_map(|reference| alias(reference)).map(|event| format!("event `{event}`")),
        );
        citations.extend(items(item, "assumption_ids").iter().filter_map(Value::as_str).map(|id| format!("assumption `{id}`")));
        if citations.is_empty() {
            String::new()
        } else {
            format!(" Evidence: {}.", citations.join(", "))
        }
    };
    lines.extend([String::new(), "## Hypotheses".to_owned(), String::new()]);
    let hypotheses = items(review, "hypotheses");
    if hypotheses.is_empty() {
        lines.push("No hypotheses were selected.".to_owned());
    }
    for item in hypotheses {
        lines.push(format!("- {}{}", label(&HYPOTHESES, item["hypothesis"].as_str().unwrap_or_default()), references(item)));
    }
    lines.extend([String::new(), "## Recommendations".to_owned(), String::new()]);
    let recommendations = items(review, "recommendations");
    if recommendations.is_empty() {
        lines.push("No recommendations were selected.".to_owned());
    }
    for item in recommendations {
        lines.push(format!("- {}{}", label(&RECOMMENDATIONS, item["action"].as_str().unwrap_or_default()), references(item)));
    }
    lines.extend([
        String::new(),
        "## Limitations".to_owned(),
        String::new(),
        "This review summarizes recorded synthetic simulation outputs. It does not establish real-world safety, behavior, or causal effects.".to_owned(),
        String::new(),
    ]);
    lines.join("\n")
}
