//! The core's port to an LLM and the rules that ask it during a tick (#428): `llm` and
//! `rules::llm_runtime`, through `PlainRun::set_llm`.
//!
//! The worlds are `tick_core.json`'s (`support/load.rs`). A fake link answers each task with a
//! choice it allows; the limits are opened (everyone eligible, no caps) so that each world asks.
//! - The port: a task and a reply as JSON, the limits, the check of an answer, the tape's entries.
//! - Offline, nothing is asked and nothing of the world changes: the Run is the Run without a link,
//!   and the social policy records why it did not ask.
//! - With a link, the rules write the priorities, their own state and the events the answers
//!   chose, and keep one tape entry a call, without the model's free text. An answer that does not
//!   fit its schema is the fallback.
//! - A tape answers the same Run again without the link (`Replay`); a call that is not the tape's
//!   next, or past its end, stops the tick.
//! - A stored Run keeps the tape in its `llm_tape` pieces; a seek restores a saved point and
//!   computes the ticks after it from the tape.
//!
//! The rules' parity with the Python ones (the prompts byte for byte, the world after the ticks) is
//! `tests/integration/test_plain_run_llm.py`'s, which runs both.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use serde_json::{json, Value};
use sheltersim_core::compute::{Replaying, Seeker};
use sheltersim_core::error::{Error, Result};
use sheltersim_core::llm::{self, check_answer, Entry, Limits, LlmLink, Offline, Replay, Reply, Task, REPLAY_MISMATCH, REPLAY_MISSING};
use sheltersim_core::model_version;
use sheltersim_core::plain::PlainRun;
use sheltersim_core::run_store::{records, Chain, Journal, Next, RecordKind, RunWriter, Schedule, Storing};
use sheltersim_core::value::V;
use sheltersim_core::world::WorldCell;

use crate::support::load;

/// Everyone eligible, no cap and no cooldown.
const OPEN: &[u8] =
    br#"{"social":{"stress_threshold":0.0,"cooldown_minutes":0,"global_per_day":1000,"per_agent_per_day":1000},"priority":{"stress_threshold":0.0,"global_per_day":1000}}"#;

/// What a fake link was asked, shared with the test.
type Asked = Rc<RefCell<Vec<Task>>>;

/// Answers each task with the first choice it allows (a priority of 1), or with `text` where given.
struct Fake {
    asked: Asked,
    text: Option<String>,
}

impl Fake {
    fn new() -> (Self, Asked) {
        let asked = Asked::default();
        (Self { asked: Rc::clone(&asked), text: None }, asked)
    }
}

impl LlmLink for Fake {
    fn complete(&mut self, task: &Task) -> Result<Reply> {
        self.asked.borrow_mut().push(task.clone());
        let input: Value = serde_json::from_str(&task.input_text).unwrap();
        let text = self.text.clone().unwrap_or_else(|| match task.task {
            llm::SUPPORT_PRIORITY => r#"{"priority":1,"rationale":"call me at someone@example.com"}"#.to_owned(),
            llm::SOCIAL_SUPPORT_REQUEST => r#"{"action":"offer_reassurance"}"#.to_owned(),
            _ => json!({"event_id": input["event_id"], "action": input["allowed_choices"][0], "reason_short": input["reason_codes"][0]}).to_string(),
        });
        Ok(Reply {
            text,
            outcome: "recorded".to_owned(),
            fallback_used: false,
            cache_hit: false,
            model_id: Some("claude-haiku-test".to_owned()),
            estimated_cost_usd: 0.0125,
            api_request_count: 1,
            input_tokens: 10,
            output_tokens: 5,
        })
    }
}

fn run_of(name: &str) -> PlainRun {
    PlainRun::of_entry(&load::entry("tick_core", name)).unwrap()
}

/// The world `name` to its end with `link` (none: no LLM), its decisions collected: the Run, and
/// the records of the social policy.
fn run_to_end(name: &str, link: Option<Box<dyn LlmLink>>, limits: &[u8]) -> (PlainRun, Vec<Value>) {
    let mut run = run_of(name);
    run.collect_decisions(true);
    if let Some(link) = link {
        run.set_llm(link, Limits::from_json(limits).unwrap()).unwrap();
    }
    let ticks = run.duration().unwrap() as u64;
    assert_eq!(run.advance(ticks).unwrap(), ticks);
    let dialogue =
        run.take_decisions().iter().map(|raw| serde_json::from_slice::<Value>(raw).unwrap()).filter(|record| record["record_kind"] == "dialogue").collect();
    (run, dialogue)
}

fn reasons(records: &[Value]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for record in records {
        *counts.entry(format!("{}:{}", record["interaction_type"].as_str().unwrap(), record["reason"].as_str().unwrap())).or_insert(0) += 1;
    }
    counts
}

fn namespace(run: &PlainRun, name: &str) -> Option<Value> {
    run.loaded.cell.with(|world| world.slots.try_value(name).unwrap().map(|value| serde_json::from_slice(&value.to_json().unwrap()).unwrap())).unwrap()
}

fn task() -> Task {
    Task {
        task: llm::SOCIAL_SUPPORT_REQUEST,
        stage: llm::STAGE_RUNTIME,
        prompt_version: "event-triggered-social-v2".to_owned(),
        schema_version: 1,
        input_text: r#"{"minute":3,"note":"避難所"}"#.to_owned(),
        state_hash: "ab".repeat(32),
        output_schema: V::from_json(&json!({
            "type": "object",
            "properties": {"action": {"type": "string", "enum": ["offer_reassurance", "no_interaction"]}, "rationale": {"type": "string", "maxLength": 4}},
            "required": ["action"],
            "additionalProperties": false,
        }))
        .unwrap(),
        fallback_text: r#"{"action":"no_interaction"}"#.to_owned(),
        max_output_tokens: 24,
        input_byte_limit: None,
    }
}

#[test]
fn a_task_and_a_reply_cross_the_port_as_json() {
    let task = task();
    let document: Value = serde_json::from_slice(&task.to_json().unwrap()).unwrap();
    let mut keys: Vec<&str> = document.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["fallback_text", "input_text", "max_output_tokens", "output_schema", "prompt_version", "schema_version", "stage", "state_hash", "task"]);
    assert_eq!(document["input_text"], task.input_text);
    // Python's `LLMTask.input_hash`: the sha256 of `json.dumps(input_text, ensure_ascii=False)`.
    let quoted = serde_json::to_string(&task.input_text).unwrap();
    assert_eq!(task.input_hash(), sheltersim_core::checkpoint::sha256_hex(quoted.as_bytes()));
    let reply = task.fallback("offline");
    assert_eq!(Reply::from_json(&reply.to_json().unwrap()).unwrap(), reply);
    assert_eq!(reply.text, task.fallback_text);
    let full: Value = serde_json::from_slice(&reply.to_json().unwrap()).unwrap();
    let edited = |edit: &dyn Fn(&mut Value)| {
        let mut value = full.clone();
        edit(&mut value);
        Reply::from_json(&serde_json::to_vec(&value).unwrap()).map_err(|error| error.to_string())
    };
    assert_eq!(edited(&|value| drop(value.as_object_mut().unwrap().remove("outcome"))).unwrap_err(), "an LLM link's reply: outcome is not a string");
    assert_eq!(edited(&|value| value["prompt"] = json!("no")).unwrap_err(), "an LLM link's reply: an unknown field prompt");
    assert_eq!(edited(&|value| value["input_tokens"] = json!(-1)).unwrap_err(), "an LLM link's reply: input_tokens is not a count");
    assert_eq!(edited(&|value| value["estimated_cost_usd"] = json!(-0.5)).unwrap_err(), "an LLM link's reply: estimated_cost_usd is not a cost");
    assert_eq!(edited(&|value| value["model_id"] = json!("m")).unwrap().model_id.as_deref(), Some("m"));
    assert_eq!(Reply::from_json(b"[]").unwrap_err().to_string(), "an LLM link's reply: not an object");
    assert_eq!(Reply::from_json(b"{").unwrap_err().to_string(), "an LLM link's reply: not JSON");
}

#[test]
fn an_answer_is_read_against_its_schema() {
    let schema = task().output_schema;
    assert_eq!(check_answer(r#"{"action":"offer_reassurance"}"#, &schema), Some(json!({"action": "offer_reassurance"})));
    assert!(check_answer(r#"{"action":"no_interaction","rationale":"abcd"}"#, &schema).is_some());
    for refused in [
        "",
        "  ",
        "not json",
        r#"{"action":"run this code"}"#,
        r#"{"action":"no_interaction","extra":1}"#,
        r#"{"rationale":"ab"}"#,
        r#"{"action":"no_interaction","rationale":"abcde"}"#,
        r#"{"action":1}"#,
        r#"["no_interaction"]"#,
    ] {
        assert_eq!(check_answer(refused, &schema), None, "{refused}");
    }
    let priority = V::from_json(&json!({
        "type": "object",
        "properties": {"priority": {"type": "integer", "enum": [0, 1]}},
        "required": ["priority"],
        "additionalProperties": false,
    }))
    .unwrap();
    assert!(check_answer(r#"{"priority":1}"#, &priority).is_some());
    for refused in [r#"{"priority":1.0}"#, r#"{"priority":2}"#, r#"{"priority":true}"#, r#"{"priority":"1"}"#] {
        assert_eq!(check_answer(refused, &priority), None, "{refused}");
    }
}

#[test]
fn the_limits_are_the_models_unless_set_and_checked_as_python_checks_them() {
    let limits = Limits::default();
    assert_eq!((limits.social.stress_threshold, limits.social.cooldown_minutes, limits.social.per_agent_per_day), (0.65, 240, 3));
    assert_eq!((limits.social.global_per_day, limits.social.event_delay_minutes, limits.social.complaint_wait_minutes), (4, 1, 30));
    assert_eq!((limits.priority.stress_threshold, limits.priority.global_per_day), (0.65, 4));
    assert_eq!(Limits::from_json(b"{}").unwrap(), limits);
    let set = Limits::from_json(br#"{"social":{"cooldown_minutes":5},"priority":{"global_per_day":2}}"#).unwrap();
    assert_eq!((set.social.cooldown_minutes, set.priority.global_per_day, set.social.global_per_day), (5, 2, 4));
    for (raw, text) in [
        (&br#"{"social":{"stress_threshold":1.5}}"#[..], "social stress threshold must be in 0..1"),
        (br#"{"social":{"event_delay_minutes":0}}"#, "social cooldown must be non-negative and delays positive"),
        (br#"{"social":{"global_per_day":0}}"#, "social daily request caps must be positive"),
        (br#"{"priority":{"stress_threshold":-0.1}}"#, "support priority stress threshold must be in 0..1"),
        (br#"{"priority":{"global_per_day":0}}"#, "support priority daily cap must be positive"),
        (br#"{"social":{"budget":1}}"#, "the LLM's limits have no social.budget"),
        (br#"{"social":{"cooldown_minutes":"5"}}"#, "the LLM's limit social.cooldown_minutes is not an int"),
    ] {
        assert_eq!(Limits::from_json(raw).unwrap_err().to_string(), text);
    }
}

/// Offline, the rules ask nothing and write nothing of the world: the Run is the Run without a
/// link, the samples too. The social policy records why it did not ask, as Python's does on a
/// Project's Run (the `dialogue` rows of #385).
#[test]
fn an_offline_link_changes_nothing_of_the_world_and_records_why_it_did_not_ask() {
    for name in ["generated", "small_replay"] {
        let (without, none) = run_to_end(name, None, b"{}");
        assert!(none.is_empty());
        let (offline, dialogue) = run_to_end(name, Some(Box::new(Offline)), b"{}");
        assert_eq!(offline.digests().unwrap(), without.digests().unwrap(), "{name}");
        assert_eq!(offline.host.metrics_digest(), without.host.metrics_digest(), "{name}");
        assert_eq!(offline.llm_calls(), 0);
        assert!(!dialogue.is_empty(), "{name}");
        for record in &dialogue {
            let reason = record["reason"].as_str().unwrap();
            assert!(["offline_rule_mode", "below_stress_threshold", "request_not_pending"].contains(&reason), "{name}: {reason}");
            let flags = (record["accepted"].as_bool(), record["status"].as_str(), record["source_type"].as_str());
            assert_eq!(flags, (Some(false), Some("rejected"), Some("assumption")));
            assert_eq!(record["input_hash"], Value::Null);
        }
        for state in ["social_runtime_v1", "support_priority_runtime_v1", "llm"] {
            assert_eq!(namespace(&offline, state), None, "{name}: {state}");
        }
    }
}

/// With a link the rules ask, and write what the answers chose: the priorities, their own state,
/// the events of the actions. One tape entry a call, in order, without the model's free text.
#[test]
fn the_rules_ask_the_link_and_write_what_it_answered() {
    let (fake, asked) = Fake::new();
    let (mut run, dialogue) = run_to_end("generated", Some(Box::new(fake)), OPEN);
    let (without, _) = run_to_end("generated", None, b"{}");
    assert_ne!(run.digests().unwrap(), without.digests().unwrap());
    let asked = asked.borrow();
    assert_eq!(run.llm_calls(), asked.len() as u64);
    let mut kinds = BTreeMap::new();
    for task in asked.iter() {
        *kinds.entry(task.prompt_version.as_str()).or_insert(0usize) += 1;
        assert_eq!(task.stage, "runtime");
        assert!(check_answer(&task.fallback_text, &task.output_schema).is_some(), "{}", task.fallback_text);
        // The input is canonical JSON.
        let value: Value = serde_json::from_str(&task.input_text).unwrap();
        assert_eq!(V::from_json(&value).unwrap().to_json().unwrap(), task.input_text.as_bytes());
    }
    assert_eq!(kinds.keys().copied().collect::<Vec<_>>(), ["event-triggered-social-v2", "event-triggered-social-v2-complaint_response", "support-priority-v2"]);
    let counts = reasons(&dialogue);
    assert_eq!(counts.get("support_request:accepted_and_scheduled"), Some(&kinds["event-triggered-social-v2"]));
    assert_eq!(counts.get("complaint_response:accepted_and_scheduled"), Some(&kinds["event-triggered-social-v2-complaint_response"]));
    // The accepted actions were applied the minute after.
    for record in dialogue.iter().filter(|record| record["accepted"] == true) {
        let event_id = record["event_id"].as_str().unwrap();
        let logged = run.loaded.cell.with(|world| sheltersim_core::load::find_record(world, event_id).unwrap()).unwrap();
        let logged = logged.unwrap_or_else(|| panic!("{event_id} was not applied"));
        assert_eq!(logged.realized_minute, record["minute"].as_i64().unwrap() + 1);
        assert_eq!(record["input_hash"].as_str().unwrap().len(), 64);
        assert_eq!((record["model_id"].as_str(), record["api_request_count"].as_i64()), (Some("claude-haiku-test"), Some(1)));
    }
    // The rules' state.
    let social = namespace(&run, "social_runtime_v1").unwrap();
    let social_calls = kinds["event-triggered-social-v2"] + kinds["event-triggered-social-v2-complaint_response"];
    assert_eq!(social["decision_count"], social_calls);
    assert_eq!(social["attempts_by_day"]["0"], social_calls);
    let priority = namespace(&run, "support_priority_runtime_v1").unwrap();
    assert_eq!(priority, json!({"attempts_by_day": {"0": kinds["support-priority-v2"]}, "schema_version": 1, "skipped_by_reason": {}}));
    let priorities = namespace(&run, "llm").unwrap();
    let priorities = priorities["support_priorities"].as_object().unwrap();
    assert_eq!(priorities.len(), kinds["support-priority-v2"]);
    assert!(priorities.values().all(|priority| priority == 1));
    // The tape: one entry a call, the free text of a priority kept empty.
    let tape = run.take_tape();
    assert_eq!(tape.len(), asked.len());
    assert!(run.take_tape().is_empty());
    for (raw, task) in tape.iter().zip(asked.iter()) {
        let entry = Entry::from_json(raw).unwrap();
        assert!(entry.is_of(task));
        assert!(!String::from_utf8_lossy(raw).contains("example.com"));
        assert_eq!((entry.reply.outcome.as_str(), entry.reply.fallback_used), ("recorded", false));
        if task.task == llm::SUPPORT_PRIORITY {
            assert_eq!(entry.reply.text, r#"{"priority":1,"rationale":""}"#);
        }
    }
    // The same Run again gives the same world and the same tape.
    let (fake, _) = Fake::new();
    let (mut again, _) = run_to_end("generated", Some(Box::new(fake)), OPEN);
    assert_eq!(again.digests().unwrap(), run.digests().unwrap());
    assert_eq!(again.take_tape(), tape);
}

/// The model's limits hold back what the open ones let through: a day's calls of each rule are at
/// most 4, and each skip of the priority is counted by its reason.
#[test]
fn the_models_limits_bound_the_calls() {
    let (fake, asked) = Fake::new();
    let (run, dialogue) = run_to_end("small_replay", Some(Box::new(fake)), b"{}");
    let asked = asked.borrow();
    let priorities = asked.iter().filter(|task| task.task == llm::SUPPORT_PRIORITY).count();
    assert!(priorities <= 4, "{priorities}");
    let state = namespace(&run, "support_priority_runtime_v1").unwrap();
    assert_eq!(state["attempts_by_day"].get("0").and_then(Value::as_u64).unwrap_or(0), priorities as u64);
    let skipped: i64 = state["skipped_by_reason"].as_object().unwrap().values().map(|count| count.as_i64().unwrap()).sum();
    assert!(skipped > 0);
    let social = asked.iter().filter(|task| task.task != llm::SUPPORT_PRIORITY).count();
    assert!(social <= 4, "{social}");
    assert!(reasons(&dialogue).keys().any(|reason| reason.ends_with(":below_stress_threshold")));
}

/// An answer that does not fit its schema is the task's fallback, whatever the link said: the call
/// is counted, the action rejected (`rule_fallback:invalid_output`), and nothing is scheduled.
#[test]
fn an_answer_that_does_not_fit_is_the_fallback() {
    let (mut fake, asked) = Fake::new();
    fake.text = Some(r#"{"action":"run this code","priority":"urgent"}"#.to_owned());
    let (mut run, dialogue) = run_to_end("generated", Some(Box::new(fake)), OPEN);
    assert!(!asked.borrow().is_empty());
    let asked_records: Vec<&Value> = dialogue.iter().filter(|record| record["input_hash"] != Value::Null).collect();
    assert!(!asked_records.is_empty());
    for record in asked_records {
        assert_eq!((record["accepted"].as_bool(), record["reason"].as_str()), (Some(false), Some("rule_fallback:invalid_output")));
        assert_eq!((record["fallback_used"].as_bool(), record["event_id"].as_str()), (Some(true), None));
    }
    for raw in run.take_tape() {
        let entry = Entry::from_json(&raw).unwrap();
        assert_eq!((entry.reply.outcome.as_str(), entry.reply.fallback_used), ("invalid_output", true));
        assert!(!String::from_utf8_lossy(&raw).contains("run this code"));
    }
    let priorities = namespace(&run, "llm").unwrap();
    assert!(priorities["support_priorities"].as_object().unwrap().values().all(|priority| priority == 0));
}

/// A tape answers the same Run again without asking anyone (Python's `RecordedRouter`): the same
/// world and the same tape. A call that is not the tape's next stops the tick, and so does one past
/// its end, unless a link goes on from there (Python's `ContinuingRouter`).
#[test]
fn a_tape_answers_the_same_run_again_without_the_link() {
    let (fake, _) = Fake::new();
    let (mut recorded, _) = run_to_end("generated", Some(Box::new(fake)), OPEN);
    let tape = recorded.take_tape();
    let (mut replayed, _) = run_to_end("generated", Some(Box::new(Replay::new(&tape, false).unwrap())), OPEN);
    assert_eq!(replayed.digests().unwrap(), recorded.digests().unwrap());
    assert_eq!(replayed.host.metrics_digest(), recorded.host.metrics_digest());
    assert_eq!(replayed.take_tape(), tape);

    let stopped = |link: Replay| -> String {
        let mut run = run_of("generated");
        run.set_llm(Box::new(link), Limits::from_json(OPEN).unwrap()).unwrap();
        let ticks = run.duration().unwrap() as u64;
        run.advance(ticks).unwrap_err().error.to_string()
    };
    // Past the end.
    let half = &tape[..tape.len() / 2];
    assert_eq!(stopped(Replay::new(half, false).unwrap()), REPLAY_MISSING);
    // Not the next entry: another state.
    let mut other: Vec<Vec<u8>> = tape.clone();
    let mut entry: Value = serde_json::from_slice(&other[3]).unwrap();
    entry["state_hash"] = json!("0".repeat(64));
    other[3] = serde_json::to_vec(&entry).unwrap();
    assert_eq!(stopped(Replay::new(&other, false).unwrap()), REPLAY_MISMATCH);
    // Past the end, a link goes on.
    let (fake, asked) = Fake::new();
    let (mut continued, _) = run_to_end("generated", Some(Box::new(Replay::new(half, false).unwrap().then(Box::new(fake)))), OPEN);
    assert_eq!(continued.digests().unwrap(), recorded.digests().unwrap());
    assert_eq!(asked.borrow().len(), tape.len() - half.len());
    assert_eq!(continued.take_tape(), tape);
    // An offline Run's replay is offline: it asks nothing.
    let (offline, _) = run_to_end("generated", Some(Box::new(Replay::new(&[], true).unwrap())), OPEN);
    let (without, _) = run_to_end("generated", None, b"{}");
    assert_eq!(offline.digests().unwrap(), without.digests().unwrap());
    // A tape that is not one.
    assert!(Replay::new(&[b"{}".to_vec()], false).is_err());
}

/// A link's error stops the tick, with no tape entry of that call.
#[test]
fn a_links_error_stops_the_tick() {
    struct Failing;
    impl LlmLink for Failing {
        fn complete(&mut self, _task: &Task) -> Result<Reply> {
            Err(Error::Runtime("Claude budget_exceeded: stop".to_owned()))
        }
    }
    let mut run = run_of("generated");
    run.set_llm(Box::new(Failing), Limits::from_json(OPEN).unwrap()).unwrap();
    let stopped = run.advance(360).unwrap_err();
    assert_eq!(stopped.error.to_string(), "Claude budget_exceeded: stop");
    assert!(run.take_tape().is_empty());
}

/// In-memory files of a stored Run, without compression.
#[derive(Default)]
struct Files {
    journal: Vec<u8>,
    raw: Vec<Option<Vec<u8>>>,
    offset: u64,
}

impl Files {
    fn flush(&mut self, writer: &mut RunWriter) {
        while let Some(next) = writer.next() {
            let encoded = match next {
                Next::Line => {
                    self.raw.push(None);
                    writer.commit_line().unwrap()
                }
                Next::Piece { raw, .. } => {
                    let raw = raw.to_vec();
                    let length = raw.len() as u64;
                    self.raw.push(Some(raw));
                    let offset = self.offset;
                    self.offset += length;
                    writer.commit_piece(offset, length).unwrap()
                }
                Next::Copy { .. } => panic!("no copies here"),
            };
            self.journal.extend_from_slice(&encoded.bytes);
        }
    }

    fn piece(&self, chain: &Chain, position: usize) -> &[u8] {
        self.raw[chain.pieces()[position].index].as_deref().unwrap()
    }
}

/// A stored Run keeps the tape's entries in its `llm_tape` pieces, counted at each saved point. A
/// seek restores the saved point before the minute and computes the ticks after it from the tape's
/// entries after the point's count: the world is the Run's at that minute, and nothing is asked.
#[test]
fn a_stored_run_keeps_the_tape_and_a_seek_computes_from_it() {
    let export = serde_json::to_vec(&load::data("tick_core")).unwrap();
    let limits = Limits::from_json(OPEN).unwrap();
    let linked = |run: &mut PlainRun| {
        let (fake, _) = Fake::new();
        run.set_llm(Box::new(fake), limits).unwrap();
    };
    let mut run = PlainRun::of_worlds_json(&export, "generated").unwrap();
    linked(&mut run);
    let every = 120;
    let storing = Storing { model_versions: model_version::engine_signatures(), compression: "none".to_owned(), save_every: every };
    let mut writer = RunWriter::new();
    let mut files = Files::default();
    writer.start(&run, export.clone(), Some("generated".to_owned()), &storing).unwrap();
    files.flush(&mut writer);
    let schedule = Schedule { every };
    let duration = run.duration().unwrap();
    loop {
        let step = schedule.step(run.minute().unwrap(), 50, None);
        let ran = run.advance(step).unwrap();
        writer.take(&mut run, false).unwrap();
        if ran < step {
            break;
        }
        if schedule.due(run.minute().unwrap(), duration) {
            writer.save(&mut run).unwrap();
        }
        files.flush(&mut writer);
    }
    writer.take(&mut run, true).unwrap();
    writer.save(&mut run).unwrap();
    writer.finish(&mut run).unwrap();
    files.flush(&mut writer);
    let journal = Journal::parse(&files.journal).unwrap();
    let chain = Chain::new(journal.clone()).unwrap();
    let tape: Vec<Vec<u8>> =
        chain.records(RecordKind::Tape).unwrap().into_iter().flat_map(|position| records::read(files.piece(&chain, position)).unwrap()).collect();
    let end = journal.end().unwrap();
    assert!(end.counts.tape > 0);
    assert_eq!(end.counts.tape, tape.len() as u64);
    let points = journal.saved_points();
    assert!(points.len() >= 2);
    let counts: Vec<u64> = points.iter().map(|index| journal.saved(*index).unwrap().counts.tape).collect();
    assert!(counts.windows(2).all(|pair| pair[0] <= pair[1]) && counts[0] > 0, "{counts:?}");
    // A seek between two saved points (the core's, every reader's): the point restored, the ticks
    // after it from the tape, no link set.
    let minute = 300;
    let mut seeker = Seeker::new(&chain, minute, Replaying { offline: false, limits }).unwrap();
    let saved = *journal.saved(seeker.seek().from.unwrap()).unwrap();
    while let Some(position) = seeker.next(&chain).unwrap() {
        seeker.give(&chain, files.piece(&chain, position).to_vec()).unwrap();
    }
    let sought = seeker.finish().unwrap();
    assert!(sought.from < minute && sought.from == saved.minute);
    let mut seeking = sought.run;
    let from = saved.counts.tape as usize;
    let mut straight = PlainRun::of_worlds_json(&export, "generated").unwrap();
    linked(&mut straight);
    straight.advance(minute as u64).unwrap();
    assert_eq!(seeking.state_digest().unwrap(), straight.state_digest().unwrap());
    let calls = seeking.llm_calls() as usize;
    assert!(calls > 0);
    assert_eq!(seeking.take_tape(), tape[from..from + calls].to_vec());
}
