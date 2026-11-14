//! A Scenario text read with the LLM's answer (Issue #421): the core's `parse_answer` gives what
//! Python's `parse_scenario_text` gives with a router whose provider answers the same text, for
//! each case of `data/scenario_text_answers.json` (`tests/support/core_scenario_text.py`): valid
//! answers merged with the rules' values, answers the task's schema refuses (the rules'
//! fallback), answers `_parse_llm_contract` refuses (the rules alone) and requests Python then
//! refuses. Where the answer holds a condition the browser does not compute, the core answers
//! `Unsupported` and Python accepts it.

use serde_json::Value;
use sheltersim_core::scenario::request::Refusal;
use sheltersim_core::scenario::text::{parse_answer, task, TASK_JSON};
use sheltersim_core::value::V;

fn fixture() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/scenario_text_answers.json");
    serde_json::from_slice(&std::fs::read(path).expect("the fixture")).expect("its JSON")
}

/// A parse's JSON with the request's field origins sorted (Python takes them from a set).
fn sorted(mut value: Value) -> Value {
    if let Some(origins) = value.pointer_mut("/request/field_origins") {
        let mut entries: Vec<(String, Value)> = origins.as_object().unwrap().clone().into_iter().collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        *origins = Value::Object(entries.into_iter().collect());
    }
    value
}

#[test]
fn each_answer_is_read_as_python_reads_it() {
    let document = fixture();
    let cases = document["cases"].as_array().unwrap();
    assert!(cases.len() >= 25);
    let mut wrong = Vec::new();
    let mut unsupported = 0;
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let text = case["text"].as_str().unwrap();
        let output = case["output"].as_str().unwrap();
        let python = &case["python"];
        match parse_answer(text, output) {
            Ok(parsed) => {
                let ours: Value = serde_json::from_slice(&parsed.to_value().to_json().unwrap()).unwrap();
                if python["kind"] != "ok" || sorted(ours.clone()) != sorted(python["value"].clone()) {
                    wrong.push(format!("{name}: {ours}\n  python: {python}"));
                }
            }
            Err(Refusal::Invalid(why)) => {
                if python["kind"] != "error" {
                    wrong.push(format!("{name}: invalid ({why}); python: {python}"));
                }
            }
            Err(Refusal::Unsupported(why)) => {
                unsupported += 1;
                if python["kind"] != "ok" {
                    wrong.push(format!("{name}: unsupported ({why}); python: {python}"));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    assert!(unsupported >= 2, "the cases of conditions the browser does not compute");
}

#[test]
fn the_task_is_the_one_the_answers_were_made_with() {
    let document = fixture();
    let V::Map(task) = task() else { panic!("the task is an object") };
    assert_eq!(task.get("prompt_version"), Some(&V::Str(document["task"].as_str().unwrap().into())));
    assert!(TASK_JSON.contains("\"schema\""));
}
