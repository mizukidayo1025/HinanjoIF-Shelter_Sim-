//! `GET …/branches/{branch}/report` (#266): the report the server writes when an offline Run of
//! the screen completes (web/workspace.py `finish_review` with the offline router:
//! `generate_evidence_review` falls back to the empty review), or `{"status":"not_generated"}`
//! before. A store keeps the report it made as the branch's `report.json`.
//!
//! Two steps: `report_answer` checks the request and answers from what is kept (the report made
//! before, or that there is none yet); where it gives None, the caller restores the branch's Run
//! at its last minute and `build_report` makes the report, which the caller keeps.

use serde_json::{Map, Value};

use super::query::no_query;
use super::records::RunFiles;
use super::{js_safe_integer, list, sha256_hex, string, write_value, Answer, Failure, Obj};
use crate::error::Error;
use crate::history::review_metrics;
use crate::plain::PlainRun;
use crate::world::WorldCell;

// src/sheltersim/llm/report.py.
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
// `render_review_markdown` of a rule fallback with the empty review.
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

/// The log's record `index` as its canonical JSON (`EventRecord.to_dict()`).
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

/// The report of the Run `run` at its last minute (`finish_review` with the offline router) and
/// the Scenario's JSON bytes, for the branch `branch_id`: its JSON.
pub fn build_report(run: &PlainRun, scenario: &[u8], branch_id: &str) -> Answer {
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
    // `build_review_evidence` of the last 100 records: the critical ones, the last 20.
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
    let mut empty = Obj::new();
    for name in ["observed", "event_observations", "assumptions", "hypotheses", "recommendations"] {
        empty.raw(name, b"[]".to_vec());
    }
    let mut review = Obj::new();
    review.int("schema_version", 1).str("status", "rule_fallback").str("prompt_version", "evidence-review-v1").str("input_hash", &input_hash);
    review.object("evidence", evidence).object("review", empty).null("validation_issue");
    let mut report = Obj::new();
    report.int("schema_version", 1).str("branch_id", branch_id).int("minute", run.minute()?).str("state_hash", &run.state_digest()?);
    report.str("status", "rule_fallback").str("markdown", FALLBACK_MARKDOWN).object("review", review);
    Ok(report.finish())
}
