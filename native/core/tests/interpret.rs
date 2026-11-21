//! The offline intervention compiler (#303): `sheltersim_core::interpret::compile_offline` answers
//! each request of `tests/data/offline_interpreter.json` with the bytes whose sha256 the file
//! records, or refuses it (`Error::Value`) where the file records `invalid`. The values were
//! recorded before #430 and pin the core's output now.

use std::fs;

use serde_json::{json, Value};
use sheltersim_core::checkpoint::sha256_hex;
use sheltersim_core::error::Error;
use sheltersim_core::interpret::compile_offline;

#[test]
fn offline_answers_are_the_recorded_ones() {
    let bytes = fs::read(format!("{}/tests/data/offline_interpreter.json", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let fixture: Value = serde_json::from_slice(&bytes).unwrap();
    let mut failures = Vec::new();
    let cases = fixture["cases"].as_array().unwrap();
    for case in cases {
        let request = json!({
            "text": case["text"],
            "branch_id": case["branch_id"],
            "selected_minute": case["minute"],
            "scenario_summary": fixture["summaries"][case["summary"].as_str().unwrap()],
        });
        let expected = case["answer"].as_str().unwrap();
        let got = match compile_offline(&serde_json::to_vec(&request).unwrap()) {
            Ok(body) => sha256_hex(&body),
            Err(Error::Value(_)) => "invalid".to_owned(),
            Err(error) => format!("error: {error}"),
        };
        if got != expected {
            let shown =
                compile_offline(&serde_json::to_vec(&request).unwrap()).map(|body| String::from_utf8(body).unwrap()).unwrap_or_else(|error| error.to_string());
            failures.push(format!("{} / {:?}: expected {expected}, got {got}\n  {shown}", case["summary"], case["text"]));
        }
    }
    assert!(failures.is_empty(), "{} of {} differ:\n{}", failures.len(), cases.len(), failures.join("\n"));
}

/// The redaction of a draft's text (`safe_document`'s, in `storage.projects`) and the question a
/// result question stores (#306, #308) are the recorded ones.
#[test]
fn redaction_and_questions_are_the_recorded_ones() {
    let bytes = fs::read(format!("{}/tests/data/offline_interpreter.json", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let fixture: Value = serde_json::from_slice(&bytes).unwrap();
    for item in fixture["safety"].as_array().unwrap() {
        let text = item["text"].as_str().unwrap();
        assert_eq!(sheltersim_core::interpret::redacts(text).unwrap(), item["redacts"].as_bool().unwrap(), "{text:?}");
        assert_eq!(sheltersim_core::interpret::question_text(text).unwrap(), item["question"].as_str().unwrap(), "{text:?}");
        let document = serde_json::to_vec(&json!({"a": [1, {"b": text}]})).unwrap();
        assert_eq!(sheltersim_core::interpret::document_redacts(&document).unwrap(), item["redacts"].as_bool().unwrap());
    }
    let merged = sheltersim_core::interpret::merge_objects(br#"{"b":0.0,"a":1}"#, br#"{"c":[1.0],"a":2}"#).unwrap();
    assert_eq!(merged, br#"{"a":2,"b":0.0,"c":[1.0]}"#);
}

/// A comparison's differences keep the types: int minus int is an int, anything with a float a
/// float, written as Python's `repr` writes it; a KPI that is not a number in both is left out.
#[test]
fn kpi_differences_keep_ints_and_floats() {
    let base = br#"{"kpis":{"a":3,"b":1.5,"c":2,"d":0.1,"e":true,"f":"x","g":1,"h":1.0},"x":1}"#;
    let other = br#"{"kpis":{"a":5,"b":2.5,"c":2.5,"d":0.3,"e":false,"f":"y","h":3}}"#;
    let got = sheltersim_core::interpret::kpi_differences(base, other).unwrap();
    assert_eq!(String::from_utf8(got).unwrap(), r#"{"a":2,"b":1.0,"c":0.5,"d":0.19999999999999998,"h":2.0}"#);
}

#[test]
fn a_pdf_paragraph_s_issue_is_the_first_text_refused() {
    use sheltersim_core::interpret::{pdf_prose_issue, PDF_UNSUPPORTED_WORDS};
    // The words the analysis's rules name (#608) are each refused, inside any phrase.
    for word in PDF_UNSUPPORTED_WORDS {
        let text = format!("避難所の記録では{word}ように見えると考えられます。");
        assert_eq!(pdf_prose_issue(&text).unwrap().as_deref(), Some(word), "{word}");
    }
    assert_eq!(pdf_prose_issue("最初に発生した時刻は読み取れないと考えられます。").unwrap().as_deref(), Some("発生した"));
    assert_eq!(pdf_prose_issue("待機が3人を超えた可能性があります。").unwrap().as_deref(), Some("3"));
    assert_eq!(pdf_prose_issue("「待機」が続いた可能性があります。").unwrap().as_deref(), Some("「"));
    assert_eq!(pdf_prose_issue("plain ascii only").unwrap().as_deref(), Some(""));
    assert_eq!(pdf_prose_issue("待機と支援需要を合わせて確認する必要があると考えられます。").unwrap(), None);
}
