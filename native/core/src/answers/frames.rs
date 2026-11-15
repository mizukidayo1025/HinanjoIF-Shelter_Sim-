//! The answers made of a branch's frames (#264, #307): `timeline`, `replay-frames` of one or two
//! branches and `compare`, and what a Run of other model versions shows (#368, Python's D2).
//!
//! The frame of a minute is the core's (`PlainRun::frame`), made by the caller from the branch's
//! Run at that minute (restored from its stored Run and computed to it). The functions here check
//! a request in the server's order (`timeline_plan`, `ReplayPlan`, `ComparePlan`), give the minutes
//! and branches whose frames the caller makes, and write the answer of the frames' JSON texts.
//! A comparison also reads each branch's fork receipt (the "fork" line of its stored Run's
//! journal, `Comparison::new`): the caller reads the journals once the checks have passed.

use serde_json::Value;

use super::query::{route_int, Query};
use super::records::{branch_of, Record, RunFiles};
use super::{js_integer, list, string, Answer, Failure, Obj, Refusal};
use crate::interpret::{kpi_differences, merge_objects};
use crate::model_version::{versions_of_json, ModelStatus, Versions};
use crate::run_store::{Journal, Line};

fn minute_of(record: &Record, key: &str) -> Option<f64> {
    record.get(key).and_then(js_integer)
}

/// The common computed range of records: the latest fork minute and the earliest computed minute;
/// None where a record lacks either.
fn common_range<'a>(records: impl IntoIterator<Item = &'a Record>) -> Option<(f64, f64)> {
    let (mut lower, mut upper) = (f64::NEG_INFINITY, f64::INFINITY);
    for record in records {
        let (fork, until) = (minute_of(record, "fork_minute")?, minute_of(record, "computed_until")?);
        if fork > lower {
            lower = fork;
        }
        if until < upper {
            upper = until;
        }
    }
    Some((lower, upper))
}

fn outside(minute: i64, record: &Record) -> bool {
    !matches!((minute_of(record, "fork_minute"), minute_of(record, "computed_until")), (Some(fork), Some(until)) if fork <= minute as f64 && minute as f64 <= until)
}

/// `GET …/branches/{branch}/timeline?minute=`, its checks (`minute` the query's one value): the
/// branch's record and the minute whose frame is the answer.
pub fn timeline_plan(minute: &str, project_id: &str, run_id: &str, files: &RunFiles, branch_id: &str) -> Answer<(Record, i64)> {
    let minute = route_int(minute, "minute must be an integer")?;
    let record = branch_of(project_id, run_id, files, branch_id)?;
    if outside(minute, &record) {
        return Err(Refusal::storage("minute is outside this branch's computed range").into());
    }
    Ok((record, minute))
}

// ---- Comparisons ---------------------------------------------------------------------------------

/// The common computed range of branches of a Run, with the server's checks of a comparison
/// (`ProjectWorkspace.comparison_context`): 2 to 8 distinct branches, each of the Run, the minute
/// within the range they all computed.
pub fn comparison_range(branch_ids: &[String], minute: Option<f64>, project_id: &str, run_id: &str, files: &RunFiles) -> Answer<(i64, i64)> {
    let distinct = branch_ids.iter().enumerate().all(|(index, id)| !branch_ids[..index].contains(id));
    if !(2..=8).contains(&branch_ids.len()) || !distinct {
        return Err(Refusal::storage("comparison requires 2–8 distinct project branches").into());
    }
    let (_, branches) = files.document(project_id, run_id)?;
    let mut records = Vec::with_capacity(branch_ids.len());
    for id in branch_ids {
        // The last record of an id, as a Map of the records by id keeps it.
        let record = branches.iter().rev().find(|record| record.get("branch_id").and_then(Value::as_str) == Some(id.as_str()));
        records.push(record.ok_or_else(|| Refusal::storage("comparison branch is missing from this Run"))?);
    }
    match (minute, common_range(records.iter().copied())) {
        (Some(minute), Some((lower, upper))) if minute >= lower && minute <= upper => Ok((lower as i64, upper as i64)),
        _ => Err(Refusal::storage("comparison minute is outside the common computed range").into()),
    }
}

/// A fork's receipt: where the branch was forked (its stored Run's "fork" line's `fork_origin`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receipt {
    pub branch_id: String,
    pub minute: i64,
    pub state_hash: String,
    pub checkpoint_hash: Option<String>,
}

impl Receipt {
    pub fn json(&self) -> Vec<u8> {
        let mut receipt = Obj::new();
        receipt.str("branch_id", &self.branch_id).int("minute", self.minute).str("state_hash", &self.state_hash);
        receipt.opt_str("checkpoint_hash", self.checkpoint_hash.as_deref());
        receipt.finish()
    }
}

/// The receipt a branch's journal holds, or None for a branch not made by a fork (or with no
/// stored Run: an empty journal).
pub fn fork_receipt(journal: &[u8]) -> Answer<Option<Receipt>> {
    let journal = Journal::parse(journal).map_err(Failure::from)?;
    Ok(journal.entries().iter().find_map(|entry| match &entry.line {
        Line::Fork(fork) => Some(Receipt {
            branch_id: fork.origin.branch_id.clone(),
            minute: fork.origin.minute,
            state_hash: fork.origin.state_hash.clone(),
            checkpoint_hash: fork.origin.checkpoint_hash.clone(),
        }),
        _ => None,
    }))
}

/// A comparison of branches at a minute, after its checks: the common range, each branch's fork
/// receipt and whether they share their origin.
#[derive(Clone, Debug)]
pub struct Comparison {
    pub branch_ids: Vec<String>,
    pub common_range: (i64, i64),
    pub receipts: Vec<Option<Receipt>>,
    pub paired: bool,
}

impl Comparison {
    /// `journals`: each branch's `journal.jsonl` (empty where it has none), in `branch_ids`' order.
    pub fn new(branch_ids: Vec<String>, common_range: (i64, i64), journals: &[&[u8]]) -> Answer<Comparison> {
        let mut receipts = Vec::with_capacity(journals.len());
        for journal in journals {
            receipts.push(fork_receipt(journal)?);
        }
        Ok(Comparison::of_receipts(branch_ids, common_range, receipts))
    }

    /// The comparison of branches whose fork receipts the caller has read already.
    pub fn of_receipts(branch_ids: Vec<String>, common_range: (i64, i64), receipts: Vec<Option<Receipt>>) -> Comparison {
        let origin = receipts.iter().flatten().next();
        let paired =
            origin.is_some_and(|origin| branch_ids.iter().zip(&receipts).all(|(id, receipt)| receipt.as_ref() == Some(origin) || *id == origin.branch_id));
        Comparison { branch_ids, common_range, receipts, paired }
    }

    /// `comparison_projection`: the comparison of the frames (JSON texts, in `branch_ids`' order) at
    /// `minute`, each branch's KPI differences from the first (the core's: ints stay ints).
    pub fn answer(&self, run_id: &str, minute: i64, frames: &[&[u8]]) -> Answer {
        let Some((first, rest)) = frames.split_first() else {
            return Err(Failure::Error(crate::error::Error::Value("a comparison has no frames".to_owned())));
        };
        let mut differences = Vec::with_capacity(rest.len());
        for frame in rest {
            let branch_id = match serde_json::from_slice::<Value>(frame) {
                Ok(Value::Object(object)) => object.get("branch_id").cloned().unwrap_or(Value::Null),
                _ => return Err(Failure::Error(crate::error::Error::Value("a frame is not a JSON object".to_owned()))),
            };
            let mut item = Obj::new();
            let mut id = Vec::new();
            super::write_value(&mut id, &branch_id);
            item.raw("branch_id", id).raw("metrics", kpi_differences(first, frame)?);
            differences.push(item.finish());
        }
        let mut answer = Obj::new();
        answer.raw("common_range", format!("[{},{}]", self.common_range.0, self.common_range.1).into_bytes());
        answer.raw("differences", list(differences)).raw("frames", list(frames.iter().map(|frame| frame.to_vec())));
        answer.str("interpretation", "Same Run/seed/time descriptive model comparison. Shared lineage does not establish real-world causation.");
        answer.int("minute", minute).raw("origin_receipts", list(self.receipts.iter().map(|receipt| receipt.as_ref().map_or(b"null".to_vec(), Receipt::json))));
        answer.str("run_id", run_id).int("schema_version", 1).bool("shared_origin", self.paired);
        Ok(answer.finish())
    }
}

/// `GET runs/{run}/compare?branch_ids=&minute=`, its checks (the route takes a query of exactly
/// these two names): the branches and the minute whose frames the answer compares. Then the
/// caller reads the branches' journals (`Comparison::new`) and makes their frames.
#[derive(Clone, Debug)]
pub struct ComparePlan {
    pub branch_ids: Vec<String>,
    pub minute: i64,
    pub common_range: (i64, i64),
    /// Each branch's record (with computed minutes), in `branch_ids`' order.
    pub records: Vec<Record>,
}

impl ComparePlan {
    pub fn new(query: &str, project_id: &str, run_id: &str, files: &RunFiles) -> Answer<ComparePlan> {
        let query = Query::parse(query);
        if query.fields.iter().any(|(_, values)| values.len() != 1) {
            return Err(Refusal::storage("comparison parameters must each occur once").into());
        }
        let minute = route_int(query.one("minute").unwrap_or(""), "comparison minute must be an integer")?;
        let branch_ids: Vec<String> = query.one("branch_ids").unwrap_or("").split(',').map(str::to_owned).collect();
        let common_range = comparison_range(&branch_ids, Some(minute as f64), project_id, run_id, files)?;
        let mut records = Vec::with_capacity(branch_ids.len());
        for id in &branch_ids {
            records.push(branch_of(project_id, run_id, files, id)?);
        }
        Ok(ComparePlan { branch_ids, minute, common_range, records })
    }
}

// ---- Replay --------------------------------------------------------------------------------------

/// `GET runs/{run}/replay-frames?branch_ids=&start=&end=`, its checks in the server's order: the
/// branches (one, or two: each minute's comparison), their records and the minutes from `start` to
/// `end`.
#[derive(Clone, Debug)]
pub struct ReplayPlan {
    pub branch_ids: Vec<String>,
    pub start: i64,
    pub end: i64,
    pub records: Vec<Record>,
}

impl ReplayPlan {
    pub fn new(query: &str, project_id: &str, run_id: &str, files: &RunFiles) -> Answer<ReplayPlan> {
        let query = Query::parse(query);
        let mut names: Vec<&str> = query.fields.iter().map(|(name, _)| name.as_str()).collect();
        names.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
        if names != ["branch_ids", "end", "start"] || query.fields.iter().any(|(_, values)| values.len() != 1) {
            return Err(Refusal::storage("replay parameters must each occur once").into());
        }
        let start = route_int(query.one("start").unwrap_or(""), "replay bounds must be integers")?;
        let end = route_int(query.one("end").unwrap_or(""), "replay bounds must be integers")?;
        let branch_ids: Vec<String> = query.one("branch_ids").unwrap_or("").split(',').map(str::to_owned).collect();
        let distinct = branch_ids.len() < 2 || branch_ids[0] != branch_ids[1];
        if !(1..=2).contains(&branch_ids.len()) || !distinct {
            return Err(Refusal::storage("replay requires one or two distinct branches").into());
        }
        if !(0 <= start && start <= end && end < start + 8) {
            return Err(Refusal::storage("replay interval must contain 1–8 integer boundaries").into());
        }
        let mut records = Vec::with_capacity(branch_ids.len());
        for id in &branch_ids {
            records.push(branch_of(project_id, run_id, files, id)?);
        }
        let within = common_range(&records).is_some_and(|(lower, upper)| start as f64 >= lower && end as f64 <= upper);
        if !within {
            return Err(Refusal::storage("replay interval is outside the common computed range").into());
        }
        Ok(ReplayPlan { branch_ids, start, end, records })
    }

    /// For two branches, their comparison's checks at `start` (`comparison_range`).
    pub fn comparison_range(&self, project_id: &str, run_id: &str, files: &RunFiles) -> Answer<(i64, i64)> {
        comparison_range(&self.branch_ids, Some(self.start as f64), project_id, run_id, files)
    }

    /// The answer of the samples (JSON texts, one for each minute from `start` to `end`): a
    /// branch's frames, or two branches' comparisons (`Comparison::answer`).
    pub fn answer(&self, samples: &[&[u8]]) -> Vec<u8> {
        let mut answer = Obj::new();
        answer.raw("branch_ids", list(self.branch_ids.iter().map(|id| string(id)))).int("end", self.end);
        answer.raw("samples", list(samples.iter().map(|sample| sample.to_vec()))).int("schema_version", 1).int("start", self.start);
        answer.finish()
    }
}

// ---- A Run of other model versions -----------------------------------------------------------------

/// How the model versions a branch's stored Run was computed with compare with the ones the core
/// records now for the Run's Scenario: `journal` the branch's `journal.jsonl`, `scenario` the
/// Scenario's JSON (`initial::model_versions`; for a Scenario the core cannot sign, the stored
/// Run's own with the core's engine items, so only the engine's are compared). None for a branch
/// without a stored Run (an empty journal) or whose journal cannot be read; a stored Run of an
/// older format is refused (`stored_run_outdated`).
pub fn model_view(journal: &[u8], scenario: &[u8]) -> Answer<Option<(Versions, Versions, ModelStatus)>> {
    if journal.is_empty() {
        return Ok(None);
    }
    let recorded = match Journal::parse(journal).and_then(|journal| journal.run().map(|run| run.model_versions.clone())) {
        Ok(recorded) => recorded,
        Err(error) => {
            return match Failure::from(error) {
                refused @ Failure::Refused(_) => Err(refused),
                Failure::Error(_) => Ok(None),
            };
        }
    };
    let installed = match crate::initial::model_versions(scenario).ok().and_then(|value| value.to_json().ok()).and_then(|json| versions_of_json(&json).ok()) {
        Some(installed) => installed,
        None => {
            let mut installed = recorded.clone();
            installed.extend(crate::model_version::engine_signatures());
            installed
        }
    };
    let status = ModelStatus::of(&recorded, &installed);
    Ok(Some((recorded, installed, status)))
}

/// Python's `_require_current_model`: refused (409 `run_model_view_only`, Python's text) where
/// the branch's stored Run was computed with other model versions than the core records now.
pub fn require_current_model(journal: &[u8], scenario: &[u8]) -> Answer<()> {
    match model_view(journal, scenario)? {
        Some((_, _, status)) if !status.current() => Err(Refusal::storage_with(status.error_text(), crate::model_version::VIEW_ONLY_CODE).into()),
        _ => Ok(()),
    }
}

/// The frame of a Run of other model versions: the frame of the saved point at or before the
/// minute asked for (`saved_minute`), with the fields `SavedPointView.describe()` adds
/// (`ModelStatus::describe_json`).
pub fn view_frame(frame: &[u8], status: &ModelStatus, saved_minute: i64, requested_minute: i64) -> Answer {
    Ok(merge_objects(frame, &status.describe_json(saved_minute, requested_minute))?)
}
