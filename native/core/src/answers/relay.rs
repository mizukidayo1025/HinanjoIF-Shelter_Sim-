//! The catalog of the tasks the browser's LLM relay (#287, #470) passes on for the answers made
//! with an LLM: a result question, the reading of an intervention's text (and its one repair) and
//! the PDF report's analysis.
//!
//! The browser builds each task in the core (`answers::asking`, `interpret::Compiling`) and sends
//! the task's JSON to the relay (`deploy/workers/relay_tasks.mjs`); the relay holds the API key
//! and builds the call: the system prompt (Python's `_system_prompt`: a fixed text and the task's
//! output schema), the model of the task's stage, and the task's input as the one user message.
//! It lets in only a task that is the catalog's: its name, stage, prompt and schema versions and
//! output size, an input of at most `max_input_bytes`, and the catalog's schema where the task's
//! schema is fixed (`output_schema` null: the intervention's, which follows the Scenario's
//! targets). The relay reads `relay_tasks.json`, which `catalog_json` writes
//! (`tests/relay_tasks.rs` fails while the file is stale).
//!
//! The review of a completed branch (`answers::report`) and the tick's questions are not here: the
//! browser computes its Runs without an LLM (#470).

use serde_json::{json, Value};

use super::{asking, pdf, questions};
use crate::interpret::asked;
use crate::llm::{INTERVENTION_COMPILER, INTERVENTION_REPAIR};

/// The router's generic input guard (Python's `LLMSettings.max_input_tokens`, 16,384 UTF-8 bytes):
/// the input limit of a task that declares none.
pub const GENERIC_INPUT_BYTES: i64 = 16_384;

fn entry(task: &str, stage: &str, prompt_version: &str, schema_version: i64, max_output_tokens: i64, input_byte_limit: Option<i64>, schema: Value) -> Value {
    json!({
        "task": task,
        "stage": stage,
        "prompt_version": prompt_version,
        "schema_version": schema_version,
        "max_output_tokens": max_output_tokens,
        "input_byte_limit": input_byte_limit,
        "max_input_bytes": GENERIC_INPUT_BYTES.max(input_byte_limit.unwrap_or(0)),
        "output_schema": schema,
    })
}

fn review(review: asking::Review<'_>, input_byte_limit: Option<i64>, schema: Value) -> Value {
    entry(review.task, asking::STAGE_REVIEW, review.prompt_version, review.schema_version, review.max_output_tokens, input_byte_limit, schema)
}

/// The catalog: `{"schema_version": 1, "tasks": [...]}`, each task's name, stage, prompt and schema
/// versions, the most tokens of its output, its own input byte limit (`input_byte_limit`, null
/// where it has none) and the limit the router applies (`max_input_bytes`), and its output schema
/// (null where the task's schema follows its input).
pub fn catalog() -> Value {
    let compiler = |task: &str, prompt_version: &str| {
        entry(task, asked::STAGE_INTERVENTION, prompt_version, asked::SCHEMA_VERSION, asked::MAX_OUTPUT_TOKENS, Some(asked::INPUT_BYTE_LIMIT), Value::Null)
    };
    json!({
        "schema_version": 1,
        "tasks": [
            review(questions::REVIEW, Some(questions::INPUT_BYTE_LIMIT), questions::output_schema()),
            compiler(INTERVENTION_COMPILER, crate::interpret::OPUS_COMPILER_PROMPT_VERSION),
            compiler(INTERVENTION_REPAIR, asked::REPAIR_PROMPT_VERSION),
            review(pdf::REVIEW, Some(pdf::INPUT_BYTE_LIMIT), pdf::output_schema()),
        ],
    })
}

/// The catalog's text as `relay_tasks.json` holds it: two-space indents, keys sorted, a newline at
/// the end.
pub fn catalog_json() -> String {
    let mut text = serde_json::to_string_pretty(&catalog()).expect("JSON of a value");
    text.push('\n');
    text
}
