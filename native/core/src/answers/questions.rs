//! `POST runs/{run}/branches/{branch}/questions` (#308): the answer to a question about a
//! branch's results as the Python server gives it without an API key
//! (src/sheltersim/web/results_chat.py `_ask` and `_project_evidence`): the request's checks, the
//! evidence's checks (a completed branch, the minute, the selected person, the branches compared),
//! the fixed answer that no question can be answered without the Claude API, the context of the
//! question and its id, and the turn the caller keeps in the branch's stored Run.
//!
//! `QuestionPlan::new` checks the request and the branch; the caller makes the branch's frame at
//! `QuestionPlan::minute`; `check_frame` checks the person; where `comparison_ids` names branches,
//! the caller checks their comparison (`frames::comparison_range`, the receipts); `answer` gives
//! the answer and the turn. The context's id is "c", the first 32 hex digits of the sha256 of the
//! context's JSON, "-" and 24 random hex digits (the caller's), or the id the request gave when it
//! is of the same context.

use serde_json::Value;

use super::records::RunFiles;
use super::{code_points, js_integer, list, sha256_hex, string, Answer, Failure, Obj, Refusal};
use crate::error::Error;
use crate::interpret::question_text;

const MAX_QUESTION: usize = 2000;
const ALLOWED: [&str; 9] = ["run_id", "project_id", "comparison_branch_ids", "demo_branch_id", "branch_id", "minute", "agent_id", "question", "context_id"];
const ANSWER: &str = "Claude APIが利用できないため、結果に関する質問へ回答できません。レポートと記録済み根拠は引き続き確認できます。";
const LIMITATION: &str = "LLM is unavailable; no answer was generated.";

/// Python's `str.isspace` of a character.
fn is_space(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | '\u{1c}'..='\u{1f}' | ' ' | '\u{85}' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}')
}

/// Python's `bool()` of a JSON value.
fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::Number(_)) => value.and_then(super::js_number).is_some_and(|number| number != 0.0),
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(items)) => !items.is_empty(),
        Some(Value::Object(object)) => !object.is_empty(),
        Some(Value::Bool(true)) => true,
    }
}

/// A question the server takes (step 1 of its answer).
#[derive(Clone, Debug)]
pub struct QuestionPlan {
    project_id: String,
    run_id: String,
    branch_id: String,
    /// The minute the request asked for, then the evidence's (`evidence`).
    minute: Option<f64>,
    record: Option<super::records::Record>,
    agent_id: Option<String>,
    /// The request's `comparison_branch_ids`, checked with the frame.
    comparisons: Value,
    requested: Option<String>,
    stored: String,
}

impl QuestionPlan {
    /// The request's checks.
    pub fn new(body: Option<&[u8]>, project_id: &str, run_id: &str, branch_id: &str) -> Answer<QuestionPlan> {
        let invalid = || Failure::from(Refusal::invalid_request());
        let mut document = super::post_document(body)?;
        for (key, value) in [("project_id", project_id), ("run_id", run_id), ("branch_id", branch_id)] {
            document.insert(key.to_owned(), Value::String(value.to_owned()));
        }
        if document.keys().any(|key| !ALLOWED.contains(&key.as_str())) {
            return Err(invalid());
        }
        let question = match document.get("question") {
            Some(Value::String(question)) if !question.chars().all(is_space) && code_points(question) <= MAX_QUESTION => question.clone(),
            _ => return Err(invalid()),
        };
        let requested = match document.get("context_id") {
            None | Some(Value::Null) => None,
            Some(Value::String(id)) if code_points(id) <= 64 => Some(id.clone()),
            _ => return Err(invalid()),
        };
        let demo = document.get("demo_branch_id");
        if truthy(Some(&Value::String(run_id.to_owned()))) == truthy(demo) {
            return Err(invalid());
        }
        let minute = match document.get("minute") {
            None | Some(Value::Null) => None,
            Some(value) => Some(js_integer(value).filter(|minute| *minute >= 0.0).ok_or_else(invalid)?),
        };
        let agent_id = match document.get("agent_id") {
            None | Some(Value::Null) => None,
            Some(Value::String(id)) if code_points(id) <= 128 => Some(id.clone()),
            _ => return Err(invalid()),
        };
        let mut stored = question_text(&question)?;
        let demo_text = demo.and_then(Value::as_str);
        for private in [Some(run_id), demo_text, Some(branch_id), agent_id.as_deref()].into_iter().flatten() {
            if !private.is_empty() {
                stored = stored.replace(private, "[selected entity]");
            }
        }
        stored = stored.replace(project_id, "[selected project]");
        Ok(QuestionPlan {
            project_id: project_id.to_owned(),
            run_id: run_id.to_owned(),
            branch_id: branch_id.to_owned(),
            minute,
            record: None,
            agent_id,
            comparisons: document.get("comparison_branch_ids").cloned().unwrap_or(Value::Null),
            requested,
            stored,
        })
    }

    /// The evidence's first check: the branch's record (in `files`). The caller then reads the
    /// Run's Scenario.
    pub fn check_branch(&mut self, files: &RunFiles) -> Answer<()> {
        self.record = Some(files.branch_record(&self.project_id, &self.run_id, &self.branch_id)?);
        Ok(())
    }

    /// The evidence's checks, after `check_branch`: a branch computed to the Scenario's end (its
    /// JSON), and the minute within it.
    pub fn evidence(&mut self, scenario: &[u8]) -> Answer<()> {
        let record = self.record.as_ref().ok_or_else(|| Failure::Error(Error::Value("the branch is not checked".to_owned())))?;
        let scenario: Value = serde_json::from_str(&String::from_utf8_lossy(scenario)).map_err(|error| Failure::Error(Error::Value(error.to_string())))?;
        let until = record.get("computed_until").and_then(super::js_number);
        let duration = scenario.get("duration_minutes").and_then(super::js_number);
        if until.is_none() || until != duration {
            return Err(Refusal::invalid_request().into());
        }
        let fork = record.get("fork_minute").and_then(super::js_number);
        let minute = self.minute.unwrap_or(until.unwrap_or_default());
        if !matches!((fork, until), (Some(fork), Some(until)) if fork <= minute && minute <= until) {
            return Err(Refusal::storage("minute is outside this branch's computed range").into());
        }
        self.minute = Some(minute);
        Ok(())
    }

    /// The minute of the evidence (after `evidence`): the branch's frame there is read.
    pub fn minute(&self) -> i64 {
        self.minute.unwrap_or_default() as i64
    }

    /// The selected person must be in the frame (the branch's at `minute`, its JSON), and the
    /// branches compared a list of at most 7 ids. The branches whose comparison with this one at
    /// `minute` the caller then checks (this branch first, each once), or none.
    pub fn check_frame(&self, frame: &[u8]) -> Answer<Vec<String>> {
        if let Some(agent) = self.agent_id.as_deref().filter(|agent| !agent.is_empty()) {
            let frame: Value = serde_json::from_slice(frame).map_err(|error| Failure::Error(Error::Value(error.to_string())))?;
            let people = frame.get("people").and_then(Value::as_array).ok_or_else(|| Failure::Error(Error::Value("a frame without people".to_owned())))?;
            if !people.iter().any(|person| person.get("id").and_then(Value::as_str) == Some(agent)) {
                return Err(Refusal::invalid_request().into());
            }
        }
        let comparisons = self.comparisons()?;
        if comparisons.is_empty() {
            return Ok(Vec::new());
        }
        let mut ids = vec![self.branch_id.clone()];
        for id in comparisons {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        Ok(ids)
    }

    fn comparisons(&self) -> Answer<Vec<String>> {
        match &self.comparisons {
            Value::Null => Ok(Vec::new()),
            Value::Array(items) if items.len() <= 7 => {
                items.iter().map(|item| item.as_str().map(str::to_owned)).collect::<Option<Vec<_>>>().ok_or_else(|| Refusal::invalid_request().into())
            }
            _ => Err(Refusal::invalid_request().into()),
        }
    }

    fn context(&self) -> Vec<u8> {
        let mut context = Obj::new();
        context.str("project_id", &self.project_id).str("run_id", &self.run_id).str("branch_id", &self.branch_id);
        context.int("requested_minute", self.minute()).int("captured_minute", self.minute()).opt_str("agent_id", self.agent_id.as_deref());
        context.raw("comparison_branch_ids", list(self.comparisons().unwrap_or_default().iter().map(|id| string(id)))).null("history_scope");
        context.finish()
    }

    /// The answer and the turn to keep (its JSON): `random` 24 hex digits for a new context's id,
    /// `turn_id` the turn's id (`t-` and the time in nanoseconds, `-` and 8 random hex digits).
    pub fn answer(&self, random: &str, turn_id: &str) -> (Vec<u8>, Vec<u8>) {
        let context = self.context();
        let prefix = &sha256_hex(&context)[..32];
        let kept = self.requested.as_deref().filter(|requested| {
            requested
                .strip_prefix('c')
                .and_then(|rest| rest.strip_prefix(prefix))
                .and_then(|rest| rest.strip_prefix('-'))
                .is_some_and(|tail| tail.len() == 24 && tail.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
        });
        let context_id = kept.map_or_else(|| format!("c{prefix}-{random}"), str::to_owned);
        let usage = b"{\"api_request_count\":0,\"estimated_cost_usd\":0}".to_vec();
        let limitations = list([string(LIMITATION)]);
        let mut result = Obj::new();
        result.str("answer", ANSWER).raw("sources", b"[]".to_vec()).raw("limitations", limitations.clone()).str("mode", "offline");
        result.null("model").raw("usage", usage.clone()).str("context_id", &context_id).raw("context", context.clone()).str("availability", "unavailable");
        let mut turn = Obj::new();
        turn.str("turn_id", turn_id).str("question", &self.stored).str("answer", ANSWER).raw("evidence", b"[]".to_vec());
        turn.raw("limitations", limitations).raw("usage", usage).null("model_version").int("minute", self.minute());
        turn.str("conversation_id", &context_id).raw("claims", b"[]".to_vec()).raw("context", context).raw("sources", b"[]".to_vec());
        (result.finish(), turn.finish())
    }
}
