//! What the answers made with an LLM share (#463): a result question's answer (`questions`), the
//! review of a completed branch (`report`), the analysis of the PDF report (`pdf`) and the reading
//! of an intervention's text (`interventions`).
//!
//! Each is two steps around the core's port to an LLM (`crate::llm`): the core builds the task
//! from the evidence it is given (`Task::to_json`), the caller hands the task to its link
//! (`LlmLink::complete`; Python's `RouterLink` for the local and VM versions, the relay of #287
//! for the browser later) and gives the link's reply back (`Reply::to_json`), and the core reads
//! the answer against the task's schema and the evidence, and writes the route's answer. Two
//! steps rather than a link held through the call, so that a host whose link answers later (the
//! browser's `fetch`) can ask the same task. An answer is untrusted input: one that does not fit
//! the task's schema is the fallback (outcome `invalid_output`), and the evidence checks each
//! feature makes come after.

use serde_json::Value;

use super::{sha256_hex, write_value, Answer, Failure};
use crate::error::Error;
use crate::llm::{check_answer, Reply, Task};
use crate::value::V;

/// Python's `LLMStage.REVIEW`: the analysis of results (Opus where the router's settings say so).
pub const STAGE_REVIEW: &str = "review";
/// Python's `LLMStage.INTERVENTION`: the intervention compiler.
pub const STAGE_INTERVENTION: &str = "intervention";

/// The link's reply, read: the reply, and the answer's value where it fits the task's schema
/// (None where the link fell back, or where the answer does not fit: the reply then says
/// `invalid_output` and that it fell back).
#[derive(Clone, Debug)]
pub struct Read {
    pub reply: Reply,
    pub answer: Option<Value>,
}

/// The reply `raw` (`llm::Reply`'s JSON) to `task`, read.
pub fn read_reply(task: &Task, raw: &[u8]) -> Answer<Read> {
    let mut reply = Reply::from_json(raw).map_err(Failure::Error)?;
    let answer = if reply.fallback_used {
        None
    } else {
        match check_answer(&reply.text, &task.output_schema) {
            Some(answer) => Some(answer),
            None => {
                reply.outcome = "invalid_output".to_owned();
                reply.fallback_used = true;
                None
            }
        }
    };
    Ok(Read { reply, answer })
}

/// The canonical JSON of a value (keys sorted, no spaces, `ensure_ascii=False`'s strings, numbers
/// as they were written): a task's input text.
pub fn canonical(value: &Value) -> String {
    let mut out = Vec::new();
    write_value(&mut out, value);
    String::from_utf8(out).expect("JSON of strings is UTF-8")
}

/// The sha256 of a value's canonical JSON, in hex (Python's `digest`).
pub fn digest(value: &Value) -> String {
    sha256_hex(canonical(value).as_bytes())
}

/// A schema written as JSON, as the core's port holds it.
pub fn schema(value: &Value) -> Answer<V> {
    V::from_json(value).ok_or_else(|| Failure::Error(Error::Value("a schema the core cannot hold".to_owned())))
}

/// A task of the analysis stage.
pub fn review_task(
    task: &'static str,
    prompt_version: &str,
    schema_version: i64,
    input: &Value,
    state_hash: String,
    output_schema: &Value,
    fallback_text: &str,
    max_output_tokens: i64,
) -> Answer<Task> {
    Ok(Task {
        task,
        stage: STAGE_REVIEW,
        prompt_version: prompt_version.to_owned(),
        schema_version,
        input_text: canonical(input),
        state_hash,
        output_schema: schema(output_schema)?,
        fallback_text: fallback_text.to_owned(),
        max_output_tokens,
        input_byte_limit: None,
    })
}

/// The task's JSON, as it crosses the port.
pub fn task_json(task: &Task) -> Answer {
    task.to_json().map_err(Failure::Error)
}

/// The reply's record as an answer shows it: `{"api_request_count", "estimated_cost_usd",
/// "fallback_used", "input_tokens", "outcome", "output_tokens", "stage"}`.
pub fn usage_fields(reply: &Reply, stage: &str) -> serde_json::Map<String, Value> {
    let mut usage = serde_json::Map::new();
    usage.insert("api_request_count".to_owned(), Value::from(reply.api_request_count));
    usage.insert("estimated_cost_usd".to_owned(), float(reply.estimated_cost_usd));
    usage.insert("fallback_used".to_owned(), Value::Bool(reply.fallback_used));
    usage.insert("input_tokens".to_owned(), Value::from(reply.input_tokens));
    usage.insert("outcome".to_owned(), Value::String(reply.outcome.clone()));
    usage.insert("output_tokens".to_owned(), Value::from(reply.output_tokens));
    usage.insert("stage".to_owned(), Value::String(stage.to_owned()));
    usage
}

/// A float as Python's `json.dumps` writes it (`repr`), as a JSON number.
pub fn float(number: f64) -> Value {
    let mut out = Vec::new();
    match crate::json::write_float(&mut out, number) {
        Ok(()) => serde_json::from_slice(&out).unwrap_or(Value::Null),
        Err(_) => Value::Null,
    }
}

/// A string value.
pub fn text(value: &str) -> Value {
    Value::String(value.to_owned())
}

/// The keys of a string's set, for an enum.
pub fn texts<S: AsRef<str>>(values: &[S]) -> Value {
    Value::Array(values.iter().map(|value| text(value.as_ref())).collect())
}
