//! The answers of an intervention (#306): `intervention-preview` and `intervention-apply` without
//! an API key.
//!
//! Preview: `PreviewPlan::new` checks the request, the branch and the minute; the caller checks
//! the branch's model versions (`frames::require_current_model`) and restores the branch's Run at
//! the minute; `PreviewPlan::answer` reads the text with the offline compiler (or takes the typed
//! `program` the screen built from the catalog of operators, #584), checks the program on the
//! world, and gives the answer and the draft the caller keeps (`<project>/drafts/<id>.json`, its id
//! the caller's random `preview_id`).
//!
//! Apply: `apply_draft_id` reads the request's draft id; the caller reads the draft;
//! `ApplyPlan::new` checks it and the approval; the caller checks the model versions;
//! `ApplyPlan::fork_checks` checks the branch, the minute and the child; the caller makes the
//! child (`compute::Procedure::fork`, given the state the preview saw, `ApplyPlan::state_hash`)
//! and keeps its outcome (`created_outcome`, `failed_outcome`: the Run's
//! `last_fork`). The answer (`ApplyPlan::answer`) says `running`, as the server's did while a
//! worker made the child (before #427); the child is made before it now.

use serde_json::{Map, Value};

use super::records::{branch_of, is_id, Record, RunFiles};
use super::{check_fields, js_integer, post_document, sha256_hex, string, write_value, Answer, Failure, Obj, Refusal};
use crate::error::Error;
use crate::interpret::{compile_offline, document_redacts, merge_objects, Compiling};
use crate::plain::PlainRun;
use crate::program::EffectProgram;

// The fields of a DraftPlan that are an EffectProgram's (`EffectProgram.model_fields`).
const PROGRAM_FIELDS: [&str; 9] =
    ["schema_version", "program_id", "narrative", "source_text", "simulation_start_minute", "effects", "assumptions", "support_mode", "compiler_provenance"];

fn outside(minute: Option<f64>, record: &Record) -> bool {
    let at = |key: &str| record.get(key).and_then(js_integer);
    !matches!((minute, at("fork_minute"), at("computed_until")), (Some(minute), Some(fork), Some(until)) if fork <= minute && minute <= until)
}

/// `POST …/branches/{branch}/intervention-preview` (`{minute, text}` or `{minute, program}`), its
/// checks in the server's order: the fields, one of `text` and `program`, the branch, the minute.
#[derive(Clone, Debug)]
pub struct PreviewPlan {
    project_id: String,
    run_id: String,
    branch_id: String,
    record: Record,
    minute: i64,
    text: Option<Value>,
    program: Option<Value>,
}

impl PreviewPlan {
    pub fn new(body: Option<&[u8]>, project_id: &str, run_id: &str, files: &RunFiles, branch_id: &str) -> Answer<PreviewPlan> {
        let document = post_document(body)?;
        check_fields(&document, &["minute"], &["text", "program"])?;
        if document.contains_key("text") == document.contains_key("program") {
            return Err(Refusal::storage("provide exactly one of text or program").into());
        }
        let record = branch_of(project_id, run_id, files, branch_id)?;
        let minute = js_integer(&document["minute"]);
        if outside(minute, &record) {
            return Err(Refusal::storage("minute is outside this branch's computed range").into());
        }
        Ok(PreviewPlan {
            project_id: project_id.to_owned(),
            run_id: run_id.to_owned(),
            branch_id: branch_id.to_owned(),
            record,
            minute: minute.unwrap_or_default() as i64,
            text: document.get("text").cloned(),
            program: document.get("program").cloned(),
        })
    }

    /// The minute the branch's Run is restored at.
    pub fn minute(&self) -> i64 {
        self.minute
    }

    /// The branch's record.
    pub fn record(&self) -> &Record {
        &self.record
    }

    /// The compiler's request of the branch's Run at the minute (a `ParseRequest`'s JSON).
    fn request(&self, run: &mut PlainRun) -> Answer<Vec<u8>> {
        let summary = run.scenario_summary()?;
        let mut request = b"{\"branch_id\":".to_vec();
        request.extend_from_slice(&string(&self.branch_id));
        request.extend_from_slice(b",\"scenario_summary\":");
        request.extend_from_slice(&summary);
        request.extend_from_slice(format!(",\"selected_minute\":{},\"text\":", self.minute).as_bytes());
        write_value(&mut request, self.text.as_ref().unwrap_or(&Value::Null));
        request.push(b'}');
        Ok(request)
    }

    /// The answer, of the branch's Run at the minute, and the draft to keep (its bytes, None where
    /// the compiler gave no plan): `preview_id` the draft's id (a uuid4's hex, random).
    pub fn answer(&self, run: &mut PlainRun, preview_id: &str) -> Answer<(Vec<u8>, Option<Vec<u8>>)> {
        if let Some(program) = &self.program {
            return self.answer_program(run, preview_id, program);
        }
        let request = self.request(run)?;
        let compiled = match compile_offline(&request) {
            Ok(compiled) => compiled,
            Err(Error::Value(_)) => return Err(Refusal::invalid_request().into()),
            Err(error) => return Err(error.into()),
        };
        self.answer_compiled(run, preview_id, &compiled)
    }

    /// The text read by an LLM (#463, `interpret::Compiling`), of the branch's Run at the minute;
    /// `stop` whether the router's error policy stops instead of falling back. The caller hands
    /// its tasks to the link (`Compiling::task_json`, then each `Step::Ask`) until it is done, then
    /// answers with `answer_compiled`; None where no operator the core runs is eligible
    /// (`answer`'s, without an LLM), and for a typed program (nothing to read).
    pub fn compiling(&self, run: &mut PlainRun, stop: bool) -> Answer<Option<Compiling>> {
        if self.program.is_some() {
            return Ok(None);
        }
        let request = self.request(run)?;
        match Compiling::new(&request, stop) {
            Ok(compiling) => Ok(compiling),
            Err(Error::Value(_)) => Err(Refusal::invalid_request().into()),
            Err(error) => Err(error.into()),
        }
    }

    /// The compiler's refusal under a stop policy (`intervention_compiler_unavailable`): 409.
    pub fn unavailable(reason: &str) -> Failure {
        Refusal::storage_with(reason, "intervention_compiler_unavailable").into()
    }

    /// The answer and the draft of the compiler's result (`compile_offline`'s, or the one
    /// `Compiling` gave): the program read and dumped by the core's contract and checked on the
    /// branch's Run.
    pub fn answer_compiled(&self, run: &mut PlainRun, preview_id: &str, compiled: &[u8]) -> Answer<(Vec<u8>, Option<Vec<u8>>)> {
        let compiled = compiled.to_vec();
        let result: Value = serde_json::from_slice(&compiled).map_err(|error| Failure::Error(Error::Value(error.to_string())))?;
        let plan = match (result.get("status").and_then(Value::as_str), result.get("draft_plan")) {
            (Some("compiled"), Some(Value::Object(plan))) => plan,
            _ => return Ok((compiled, None)),
        };
        let mut program = Map::new();
        for key in PROGRAM_FIELDS {
            if let Some(value) = plan.get(key) {
                program.insert(key.to_owned(), value.clone());
            }
        }
        let mut program_json = Vec::new();
        super::write_object(&mut program_json, &program);
        let dump = EffectProgram::of_json(&program_json).and_then(|program| program.dump_json())?;
        self.answer_dump(run, preview_id, &compiled, dump)
    }

    /// The answer and the draft of a typed program (#584): the program read and dumped by the
    /// core's contract (400 `invalid_effect_program` with the contract's reason where it refuses
    /// it) and checked on the branch's Run. No text is read and no LLM is asked: the answer is
    /// `compiled`, with the program's support mode and its narrative as the mechanism, no
    /// fallback and no usage.
    fn answer_program(&self, run: &mut PlainRun, preview_id: &str, program: &Value) -> Answer<(Vec<u8>, Option<Vec<u8>>)> {
        let mut raw = Vec::new();
        write_value(&mut raw, program);
        let program = match EffectProgram::of_json(&raw) {
            Ok(program) => program,
            Err(Error::Value(reason)) => return Err(Refusal::new(400, reason, Some("invalid_effect_program")).into()),
            Err(error) => return Err(error.into()),
        };
        let mut base = Obj::new();
        base.raw("choices", b"[]".to_vec()).bool("fallback_used", false).raw("limitations", b"[]".to_vec());
        base.str("mechanism_summary", &program.narrative).raw("questions", b"[]".to_vec()).str("status", "compiled");
        base.str("support_mode", &program.support_mode);
        self.answer_dump(run, preview_id, &base.finish(), program.dump_json()?)
    }

    /// The answer and the draft of a program's dump (`dump`), checked on the branch's Run: `base`
    /// (the compiler's result, or a typed program's) with the approval's hash, whether it can be
    /// applied, the preview's id, the program and the checks' issues.
    fn answer_dump(&self, run: &mut PlainRun, preview_id: &str, base: &[u8], dump: Vec<u8>) -> Answer<(Vec<u8>, Option<Vec<u8>>)> {
        let issues = run.validate_effect_program_json(&dump)?;
        let dump_text = String::from_utf8_lossy(&dump).into_owned();
        let hash = sha256_hex(&dump);
        let can_apply = issues.trim() == "[]";
        let mut content = Obj::new();
        content.str("kind", "intervention").str("project_id", &self.project_id).str("run_id", &self.run_id).str("branch_id", &self.branch_id);
        content.int("minute", self.minute).str("state_hash", &run.state_digest()?).raw("program", dump_text.as_bytes().to_vec());
        content.str("hash", &hash).bool("can_apply", can_apply);
        let content = content.finish();
        if document_redacts(&content)? {
            return Err(Refusal::storage("remove sensitive text before saving or approving a draft").into());
        }
        let mut draft = Obj::new();
        draft.int("schema_version", 1).str("draft_id", preview_id).str("kind", "intervention").raw("content", content).str("program_text", &dump_text);
        let mut draft = draft.finish();
        draft.push(b'\n');
        let mut extra = Obj::new();
        extra.str("approved_program_hash", &hash).bool("can_apply", can_apply).str("preview_id", preview_id);
        extra.raw("program", dump_text.into_bytes()).raw("validation_issues", issues.into_bytes());
        Ok((merge_objects(base, &extra.finish())?, Some(draft)))
    }
}

/// `POST …/intervention-apply`, its first checks: the fields and the draft's id, whose draft the
/// caller reads (`<project>/drafts/<id>.json`).
pub fn apply_draft_id(body: Option<&[u8]>) -> Answer<String> {
    let document = post_document(body)?;
    check_fields(&document, &["preview_id", "approved_program_hash", "approved", "child_id"], &[])?;
    match document["preview_id"].as_str() {
        Some(id) if is_id(id) => Ok(id.to_owned()),
        _ => Err(Refusal::storage("invalid artifact identifier").into()),
    }
}

/// `POST …/intervention-apply` (`{preview_id, approved_program_hash, approved, child_id}`): the
/// draft's checks, the approval, then the branch's and the child's.
#[derive(Clone, Debug)]
pub struct ApplyPlan {
    document: Map<String, Value>,
    content: Map<String, Value>,
    program_text: Option<String>,
    project_id: String,
    run_id: String,
    branch_id: String,
}

impl ApplyPlan {
    /// The draft (`draft`, None where missing) and the approval checked; `busy` whether the branch
    /// computes now.
    pub fn new(body: Option<&[u8]>, draft: Option<&[u8]>, project_id: &str, run_id: &str, branch_id: &str, busy: bool) -> Answer<ApplyPlan> {
        apply_draft_id(body)?;
        let document = post_document(body)?;
        let draft = draft.ok_or_else(|| Refusal::storage("saved artifact is missing or corrupt"))?;
        let draft: Value = serde_json::from_str(&String::from_utf8_lossy(draft)).map_err(|error| Failure::Error(Error::Value(error.to_string())))?;
        let content = match (draft.get("schema_version").and_then(super::js_number), draft.get("content")) {
            (Some(version), Some(Value::Object(content))) if version == 1.0 => content.clone(),
            (Some(version), Some(Value::Null)) if version == 1.0 => {
                return Err(Failure::Error(Error::Type("Cannot read properties of null (reading 'project_id')".to_owned())));
            }
            _ => return Err(Refusal::storage("unsupported saved artifact version; migration required").into()),
        };
        let field = |key: &str| content.get(key).and_then(Value::as_str);
        if field("project_id") != Some(project_id) {
            return Err(Refusal::storage("draft belongs to another Project").into());
        }
        if field("kind") != Some("intervention") || field("run_id") != Some(run_id) || field("branch_id") != Some(branch_id) {
            return Err(Refusal::storage("intervention preview expired or has another scope").into());
        }
        let can_apply = content.get("can_apply").is_some_and(Value::is_truthy_js);
        let approved = match content.get("hash") {
            Some(hash) => document.get("approved_program_hash") == Some(hash),
            None => false,
        };
        if !can_apply || !approved {
            return Err(Refusal::storage("intervention approval does not match validated preview").into());
        }
        if document.get("approved") != Some(&Value::Bool(true)) {
            return Err(Refusal::storage("branch creation needs explicit human approval").into());
        }
        if busy {
            return Err(Refusal::storage("pause background calculation before forking").into());
        }
        let program_text = draft.get("program_text").and_then(Value::as_str).map(str::to_owned);
        Ok(ApplyPlan { document, content, program_text, project_id: project_id.to_owned(), run_id: run_id.to_owned(), branch_id: branch_id.to_owned() })
    }

    /// After the model versions are checked: the branch's record, the minute and the child, from
    /// the Run's records (`files`).
    pub fn fork_checks(&self, files: &RunFiles) -> Answer<Record> {
        let record = files.branch_record(&self.project_id, &self.run_id, &self.branch_id)?;
        if outside(self.content.get("minute").and_then(js_integer), &record) {
            return Err(Refusal::storage("minute is outside this branch's computed range").into());
        }
        let child = self.child_id().ok_or_else(|| Refusal::storage("invalid artifact identifier"))?;
        if files.branches.iter().any(|(name, _)| name == child) {
            return Err(Refusal::storage("child branch already exists").into());
        }
        let text = self.program_text.as_deref().unwrap_or("undefined");
        if Some(sha256_hex(text.as_bytes()).as_str()) != self.hash() {
            return Err(Refusal::storage("EffectProgram approval hash does not match preview").into());
        }
        Ok(record)
    }

    /// The child's id, where it is an identifier.
    pub fn child_id(&self) -> Option<&str> {
        self.document.get("child_id").and_then(Value::as_str).filter(|id| is_id(id))
    }

    /// The minute of the fork.
    pub fn minute(&self) -> i64 {
        self.content.get("minute").and_then(js_integer).unwrap_or_default() as i64
    }

    /// The approved program's JSON text, as the preview wrote it.
    pub fn program_text(&self) -> &str {
        self.program_text.as_deref().unwrap_or("")
    }

    /// The approval's hash (the program text's sha256).
    pub fn hash(&self) -> Option<&str> {
        self.content.get("hash").and_then(Value::as_str)
    }

    /// The parent's state the preview saw (its draft's `state_hash`).
    pub fn state_hash(&self) -> Option<&str> {
        self.content.get("state_hash").and_then(Value::as_str)
    }

    /// The answer while the child is made: `{"branch_id", "minute", "status": "running"}`.
    pub fn answer(&self) -> Vec<u8> {
        let mut answer = Obj::new();
        answer.str("branch_id", self.child_id().unwrap_or_default()).int("minute", self.minute()).str("status", "running");
        answer.finish()
    }
}

/// The Run's `last_fork` once the child is made: `{"status": "created", "branch_id", "minute",
/// "state_hash"}` (the child's world's digest after the program).
pub fn created_outcome(branch_id: &str, minute: i64, state_hash: &str) -> Vec<u8> {
    let mut outcome = Obj::new();
    outcome.str("status", "created").str("branch_id", branch_id).int("minute", minute).str("state_hash", state_hash);
    outcome.finish()
}

/// The Run's `last_fork` when making the child failed, as the server's worker answers it: a
/// refusal of the store (`refusal`, its text and code) as it is; a program that could not be
/// scheduled as the server's ValueError; anything else as a failed operation.
pub fn failed_outcome(error: &str, refusal: Option<&Refusal>) -> Vec<u8> {
    let mut outcome = Obj::new();
    outcome.str("status", "failed");
    match refusal {
        Some(refusal) => outcome.str("error", &refusal.error).str("code", &refusal.code.clone().unwrap_or_else(|| super::storage_code(&refusal.error))),
        None if error.starts_with("EffectProgram could not be scheduled") => outcome.str("error", "invalid project request").null("code"),
        None => outcome.str("error", "project operation failed").null("code"),
    };
    outcome.finish()
}

/// JS's truthiness of a JSON value.
trait Truthy {
    fn is_truthy_js(&self) -> bool;
}

impl Truthy for Value {
    fn is_truthy_js(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(flag) => *flag,
            Value::Number(_) => super::js_number(self).is_some_and(|number| number != 0.0 && !number.is_nan()),
            Value::String(text) => !text.is_empty(),
            Value::Array(_) | Value::Object(_) => true,
        }
    }
}
