//! The answers made with an LLM (#463; `sheltersim_core::answers::asking`, `questions::Asking`,
//! `report::format_g`) and the schema check every answer passes (`llm::check_answer`).
//!
//! - The check reads the schema subset Python's `validate_task_output` reads: arrays and their
//!   items, `$ref`, `anyOf`, `oneOf`, `const`, numbers' bounds, typed `additionalProperties`, a
//!   string's `pattern`, and Python's `==` for `enum`.
//! - A question's task holds its evidence packet; the answer keeps the scalars the reply cited
//!   exactly, drops explanations with digits or names, and says when nothing is supported. A reply
//!   that does not fit the schema, or a call that failed, cites nothing.
//! - The intervention compiler offers the LLM only the operators the core runs, asks once to
//!   repair a plan its checks refuse, and without an accepted plan answers with the rules (or,
//!   under a stop policy, says the compiler is unavailable).
//! - The relay's catalog (`answers::relay`, #470) is the file the relay reads, and the tasks the
//!   core builds fit it.
//!
//! The answers through the Project API, with a fake Provider and a replayed tape, are
//! `tests/integration/test_llm_answers.py`'s.

use serde_json::{json, Value};
use sheltersim_core::answers::questions::QuestionPlan;
use sheltersim_core::answers::records::RunFiles;
use sheltersim_core::answers::report::format_g;
use sheltersim_core::llm::{check_answer, Reply};
use sheltersim_core::value::V;

fn schema(value: Value) -> V {
    V::from_json(&value).unwrap()
}

#[test]
fn an_answer_is_read_against_the_schema_subset_python_reads() {
    let list = schema(json!({
        "type": "object",
        "properties": {
            "items": {"type": "array", "minItems": 1, "maxItems": 2, "items": {"type": "string", "pattern": "^[a-z]+$", "minLength": 2}},
            "count": {"type": "integer", "minimum": 0, "exclusiveMaximum": 10},
            "kind": {"const": "fixed"},
            "either": {"anyOf": [{"type": "null"}, {"$ref": "#/$defs/Small"}]},
            "one": {"oneOf": [{"type": "integer"}, {"type": "number"}]},
            "extra": {"type": "object", "additionalProperties": {"type": "boolean"}},
        },
        "required": ["items"],
        "additionalProperties": false,
        "$defs": {"Small": {"type": "number", "maximum": 1}},
    }));
    for fits in [
        r#"{"items":["ab"]}"#,
        r#"{"items":["ab","cd"],"count":9,"kind":"fixed","either":null}"#,
        r#"{"items":["ab"],"either":0.5,"one":1.5,"extra":{"x":true}}"#,
    ] {
        assert!(check_answer(fits, &list).is_some(), "{fits}");
    }
    for refused in [
        r#"{"items":[]}"#,
        r#"{"items":["ab","cd","ef"]}"#,
        r#"{"items":["Ab"]}"#,
        r#"{"items":["a"]}"#,
        r#"{"items":["ab"],"count":10}"#,
        r#"{"items":["ab"],"count":-1}"#,
        r#"{"items":["ab"],"count":1.0}"#,
        r#"{"items":["ab"],"kind":"other"}"#,
        r#"{"items":["ab"],"either":2}"#,
        r#"{"items":["ab"],"one":1}"#,
        r#"{"items":["ab"],"extra":{"x":1}}"#,
        r#"{"items":["ab"],"unknown":1}"#,
        r#"{"count":1}"#,
    ] {
        assert_eq!(check_answer(refused, &list), None, "{refused}");
    }
    // Python's `in` of an enum: 1 == 1.0 == True, but a bool is not an integer.
    let numbers = schema(json!({"type": "object", "properties": {"n": {"type": "number", "enum": [1]}}, "additionalProperties": false}));
    assert!(check_answer(r#"{"n":1.0}"#, &numbers).is_some());
    assert!(check_answer(r#"{"n":true}"#, &numbers).is_none());
}

#[test]
fn a_float_is_written_as_python_s_g_format_writes_it() {
    for (value, text) in [
        (0.0, "0"),
        (-0.0, "-0"),
        (1.0, "1"),
        (0.5, "0.5"),
        (123456.0, "123456"),
        (1234567.0, "1.23457e+06"),
        (0.0001, "0.0001"),
        (0.00001234, "1.234e-05"),
        (2.0 / 3.0, "0.666667"),
        (f64::INFINITY, "inf"),
    ] {
        assert_eq!(format_g(value), text, "{value}");
    }
}

const SCENARIO: &str = r#"{"duration_minutes":30,"provenance":{"sources":[{"id":"src-1","title":"Guide","source_kind":"assumption","url":null}]}}"#;
const FRAME: &str = r#"{"branch_id":"main","kpis":{"minute_index":30,"stress_mean":0.25,"toilet_queue_length":3,"other":1},"people":[{"id":"agent-7","activity":"rest","stress":0.5,"trust":0.75,"needs":{"hunger":0.25},"mobility":"independent"}],"events":[{"event_id":"ev-1","minute":12,"event_type":"social_interaction","status":"applied","actors":["agent-7"]}]}"#;

fn asking(question: &str) -> sheltersim_core::answers::questions::Asking {
    let body = json!({"question": question, "agent_id": "agent-7"}).to_string();
    let mut plan = QuestionPlan::new(Some(body.as_bytes()), "p", "r", "main").unwrap();
    let branch = br#"{"branch_id":"main","computed_until":30,"fork_minute":0,"checkpoint_hash":"h","schema_version":1,"status":"completed"}"#.to_vec();
    plan.check_branch(&RunFiles { run: None, branches: vec![("main".to_owned(), branch)] }).unwrap();
    plan.evidence(SCENARIO.as_bytes()).unwrap();
    assert!(plan.check_frame(FRAME.as_bytes()).unwrap().is_empty());
    let evidence = r#"{"models":{"behavior":{"id":"m-1","status":"assumption"}},"interpretation":"仮定です。","state_hash":"x"}"#;
    plan.ask(FRAME.as_bytes(), None, Some(evidence.as_bytes()), SCENARIO.as_bytes(), &[], &"a".repeat(24)).unwrap()
}

fn reply(text: &Value, fallback_used: bool) -> Vec<u8> {
    let reply = Reply {
        text: text.to_string(),
        outcome: if fallback_used { "provider_error" } else { "recorded" }.to_owned(),
        fallback_used,
        cache_hit: false,
        model_id: Some("claude-opus-5-5".to_owned()),
        estimated_cost_usd: 0.25,
        api_request_count: 1,
        input_tokens: 10,
        output_tokens: 5,
    };
    reply.to_json().unwrap()
}

fn answered(asking: &sheltersim_core::answers::questions::Asking, reply: Option<&[u8]>) -> (Value, Value) {
    let (answer, turn) = asking.answer(reply, "record", "t-1").unwrap();
    (serde_json::from_slice(&answer).unwrap(), serde_json::from_slice(&turn).unwrap())
}

#[test]
fn a_question_s_task_holds_its_evidence_packet() {
    let asking = asking("What happened?");
    let task: Value = serde_json::from_slice(&asking.task_json().unwrap()).unwrap();
    assert_eq!(task["task"], "result_question");
    assert_eq!(task["stage"], "review");
    assert_eq!(task["prompt_version"], "read-only-results-chat-v2-grounded-explanations");
    let input: Value = serde_json::from_str(task["input_text"].as_str().unwrap()).unwrap();
    let evidence = input["evidence"].as_array().unwrap();
    let kinds: Vec<&str> = evidence.iter().map(|item| item["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["kpi", "agent", "model", "source", "event"]);
    assert_eq!(evidence[0]["data"], json!({"minute_index": 30, "stress_mean": 0.25, "toilet_queue_length": 3}));
    // Lists become objects keyed by their index, a page's metadata left out.
    assert_eq!(evidence[2]["data"], json!({"models": {"behavior": {"id": "m-1", "status": "assumption"}}, "interpretation": "仮定です。"}));
    assert_eq!(input["scope"]["entity_selected"], true);
    assert_eq!(input["recent_questions_and_answers"], json!([]));
}

#[test]
fn an_answer_keeps_the_scalars_its_reply_cited_exactly() {
    let asking = asking("What happened?");
    let cited = json!({
        "claims": [{"evidence": [{"alias": "S1", "field": "stress_mean", "value": "0.25"}]}, {"evidence": [{"alias": "S2", "field": "activity", "value": "rest"}]}],
        "explanations": [
            {"kind": "hypothesis", "text": "待ちが続いた可能性があります。", "evidence": [{"alias": "S1", "field": "toilet_queue_length", "value": "3"}]},
            {"kind": "suggestion", "text": "3人を増やす", "evidence": [{"alias": "S1", "field": "toilet_queue_length", "value": "3"}]},
            {"kind": "synthetic_dialogue", "text": "順番を待ちましょう。", "evidence": [{"alias": "S1", "field": "toilet_queue_length", "value": "3"}]},
        ],
    });
    let (answer, turn) = answered(&asking, Some(&reply(&cited, false)));
    let facts: Vec<&str> = answer["facts"].as_array().unwrap().iter().map(|fact| fact["text"].as_str().unwrap()).collect();
    assert_eq!(facts, ["記録値 · Boundary KPI · 30 · 平均ストレス = 0.25", "記録値 · Selected synthetic Agent state · 現在の行動 = \"rest\""]);
    // Digits, and a dialogue without an event or social source, are not shown.
    let explanations = answer["explanations"].as_array().unwrap();
    assert_eq!(explanations.len(), 1);
    assert_eq!(explanations[0]["text"], "AIの解釈・仮説（記録された因果の断定ではありません）: 待ちが続いた可能性があります。");
    assert_eq!(answer["usage"]["rejected_explanation_batch"], true);
    assert_eq!(answer["usage"]["accepted_fact_count"], 2);
    assert_eq!(answer["usage"]["estimated_cost_usd"], 0.25);
    assert_eq!(answer["model"], "claude-opus-5-5");
    assert_eq!(answer["availability"], "configured");
    let aliases: Vec<&str> = answer["sources"].as_array().unwrap().iter().map(|source| source["alias"].as_str().unwrap()).collect();
    assert_eq!(aliases, ["S1", "S2"]);
    assert_eq!(turn["answer"], answer["answer"]);
    assert_eq!(turn["evidence"][0]["reference_id"], "boundary-kpis");
    assert_eq!(turn["conversation_id"], answer["context_id"]);
}

#[test]
fn a_reply_that_cites_a_value_not_recorded_supports_nothing() {
    let asking = asking("What happened?");
    let wrong = json!({"claims": [{"evidence": [{"alias": "S1", "field": "stress_mean", "value": "0.3"}]}], "explanations": []});
    let (answer, _) = answered(&asking, Some(&reply(&wrong, false)));
    assert_eq!(answer["facts"], json!([]));
    assert_eq!(answer["usage"]["rejected_fact_batch"], true);
    assert_eq!(answer["answer"], "提示された記録だけでは回答を裏付けられません。");
    assert_eq!(answer["availability"], "unavailable");
    // A reply that does not fit the schema is the fallback, whatever the link said.
    let (answer, _) = answered(&asking, Some(&reply(&json!({"claims": 3}), false)));
    assert_eq!(answer["usage"]["outcome"], "invalid_output");
    assert_eq!(answer["usage"]["fallback_used"], true);
    // A failed call.
    let (answer, _) = answered(&asking, None);
    assert_eq!(answer["usage"], json!({"api_request_count": 0, "estimated_cost_usd": 0, "outcome": "call_failed"}));
}

#[test]
fn a_question_naming_one_field_quotes_it_where_the_reply_cites_nothing() {
    let asking = asking("トイレ待ち人数は？");
    let (answer, _) = answered(&asking, Some(&reply(&json!({"claims": [], "explanations": []}), true)));
    assert_eq!(answer["usage"]["answer_fallback_kind"], "deterministic_exact_recorded_field");
    assert_eq!(answer["answer"], "記録値 · Boundary KPI · 30 · トイレ待ち人数 = 3");
    assert!(answer["limitations"].as_array().unwrap().iter().any(|item| item.as_str().unwrap().starts_with("Claudeが返した引用候補")));
}

// ---- The intervention compiler asked of an LLM (`interpret::Compiling`) ---------------------------

fn compiler_request() -> Vec<u8> {
    let bytes = std::fs::read(format!("{}/tests/data/offline_interpreter.json", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let fixture: Value = serde_json::from_slice(&bytes).unwrap();
    serde_json::to_vec(&json!({"text": "トイレが足りない", "branch_id": "main", "selected_minute": 10, "scenario_summary": fixture["summaries"]["generated"]}))
        .unwrap()
}

fn compiler_reply(text: &str, fallback_used: bool) -> Vec<u8> {
    let reply = Reply {
        text: text.to_owned(),
        outcome: if fallback_used { "replay_miss" } else { "recorded" }.to_owned(),
        fallback_used,
        cache_hit: false,
        model_id: Some("claude-opus-5-5".to_owned()),
        estimated_cost_usd: 0.5,
        api_request_count: 1,
        input_tokens: 10,
        output_tokens: 5,
    };
    reply.to_json().unwrap()
}

fn toilets(zone: &str, extra: Value) -> String {
    let mut effect = json!({"op": "add_toilet", "target_kind": "zone", "target_id": zone, "start_minute": 10, "parameters": {"count": 2}});
    effect.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    json!({
        "status": "compiled", "support_mode": "native", "mechanism_summary": "仮設トイレを追加します。",
        "effects": [effect], "assumptions": [], "limitations": [], "questions": [], "choices": [],
    })
    .to_string()
}

#[test]
fn the_compiler_offers_only_what_the_core_runs_and_repairs_once() {
    use sheltersim_core::interpret::{Compiling, Step};
    let mut compiling = Compiling::new(&compiler_request(), false).unwrap().unwrap();
    let task: Value = serde_json::from_slice(&compiling.task_json().unwrap()).unwrap();
    assert_eq!(
        (task["task"].as_str(), task["stage"].as_str(), task["input_byte_limit"].as_i64()),
        (Some("intervention_compiler"), Some("intervention"), Some(65_536))
    );
    let payload: Value = serde_json::from_str(task["input_text"].as_str().unwrap()).unwrap();
    let ops: Vec<&str> = payload["installed_primitives"].as_array().unwrap().iter().map(|item| item["op"].as_str().unwrap()).collect();
    assert!(!ops.is_empty() && ops.iter().all(|op| sheltersim_core::program::supports(op)), "{ops:?}");
    let zone = payload["target_ids_by_kind"]["zone"][0].as_str().unwrap().to_owned();
    // A one-shot operation with an end minute: the errors go back in one repair task.
    let Step::Ask(repair) = compiling.read(Some(&compiler_reply(&toilets(&zone, json!({"end_minute": 20})), false))).unwrap() else { panic!("no repair") };
    let repair: Value = serde_json::from_slice(&repair).unwrap();
    assert_eq!(repair["prompt_version"], "h07-opus-effect-compiler-v1-repair1");
    let repair_payload: Value = serde_json::from_str(repair["input_text"].as_str().unwrap()).unwrap();
    assert_eq!(repair_payload["repair_request"]["validation_errors"], json!(["effect 0 adds end_minute to a one-shot operation"]));
    let Step::Done(result) = compiling.read(Some(&compiler_reply(&toilets(&zone, json!({})), false))).unwrap() else { panic!("not done") };
    let result: Value = serde_json::from_slice(&result).unwrap();
    assert_eq!(result["status"], "compiled");
    assert_eq!(result["fallback_used"], false);
    assert_eq!(result["usage"]["api_request_count"], 2);
    assert_eq!(result["usage"]["prompt_version"], "geometry-population-effect-compiler-v2");
    let plan = &result["draft_plan"];
    assert_eq!(plan["compiler_provenance"]["compiler_kind"], "llm");
    assert_eq!(plan["effects"][0]["parameters"], json!({"count": 2}));
    assert!(plan["program_id"].as_str().unwrap().starts_with("opus-"));
}

#[test]
fn without_an_accepted_plan_the_rules_answer_or_a_stop_policy_refuses() {
    use sheltersim_core::interpret::{Compiling, Step};
    let mut compiling = Compiling::new(&compiler_request(), false).unwrap().unwrap();
    let Step::Done(result) = compiling.read(Some(&compiler_reply("{}", true))).unwrap() else { panic!("not done") };
    let result: Value = serde_json::from_slice(&result).unwrap();
    assert_eq!(result["fallback_used"], true);
    assert_eq!(result["usage"]["error_kind"], "replay_miss");
    let mut stopping = Compiling::new(&compiler_request(), true).unwrap().unwrap();
    assert_eq!(stopping.read(None).unwrap(), Step::Unavailable("AI（Opus）から検証済みの案を受け取れませんでした。".to_owned()));
}

/// The catalog's entry of a task, the task checked against it as the relay checks it
/// (deploy/workers/relay_tasks.mjs): the same name, stage, versions and sizes, an input within the
/// limit, and the catalog's schema where it has one.
fn fits_catalog(task: &Value) {
    let catalog = sheltersim_core::answers::relay::catalog();
    let entry = catalog["tasks"].as_array().unwrap().iter().find(|entry| entry["task"] == task["task"]).expect("a task of the catalog");
    for key in ["stage", "prompt_version", "schema_version", "max_output_tokens"] {
        assert_eq!(task[key], entry[key], "{key}");
    }
    assert_eq!(task.get("input_byte_limit").unwrap_or(&Value::Null), &entry["input_byte_limit"]);
    assert!(task["input_text"].as_str().unwrap().len() as u64 <= entry["max_input_bytes"].as_u64().unwrap());
    if !entry["output_schema"].is_null() {
        assert_eq!(task["output_schema"], entry["output_schema"]);
    }
}

#[test]
fn the_relay_s_catalog_is_current_and_the_core_s_tasks_fit_it() {
    use sheltersim_core::interpret::{Compiling, Step};
    let path = format!("{}/src/answers/relay_tasks.json", env!("CARGO_MANIFEST_DIR"));
    let text = sheltersim_core::answers::relay::catalog_json();
    if std::env::var_os("SHELTERSIM_WRITE_RELAY_TASKS").is_some() {
        std::fs::write(&path, &text).unwrap();
    }
    assert_eq!(
        std::fs::read_to_string(&path).unwrap_or_default(),
        text,
        "relay_tasks.json is stale: SHELTERSIM_WRITE_RELAY_TASKS=1 cargo test -p sheltersim_core --test core llm_answers::"
    );
    let catalog: Value = serde_json::from_str(&text).unwrap();
    let names: Vec<&str> = catalog["tasks"].as_array().unwrap().iter().map(|entry| entry["task"].as_str().unwrap()).collect();
    assert_eq!(names, ["result_question", "intervention_compiler", "intervention_repair", "pdf_analysis"]);

    fits_catalog(&serde_json::from_slice(&asking("What happened?").task_json().unwrap()).unwrap());
    let mut compiling = Compiling::new(&compiler_request(), false).unwrap().unwrap();
    fits_catalog(&serde_json::from_slice(&compiling.task_json().unwrap()).unwrap());
    let zone = {
        let task: Value = serde_json::from_slice(&compiling.task_json().unwrap()).unwrap();
        let payload: Value = serde_json::from_str(task["input_text"].as_str().unwrap()).unwrap();
        payload["target_ids_by_kind"]["zone"][0].as_str().unwrap().to_owned()
    };
    let Step::Ask(repair) = compiling.read(Some(&compiler_reply(&toilets(&zone, json!({"end_minute": 20})), false))).unwrap() else { panic!("no repair") };
    fits_catalog(&serde_json::from_slice(&repair).unwrap());
}
