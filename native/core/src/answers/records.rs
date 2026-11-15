//! The Project, Run and Branch records a store keeps, and the answers made of them alone:
//! `GET /api/v1/projects`, `GET {project}`, `GET {project}/runs`, `GET runs/{run}` and
//! `GET runs/{run}/evidence` (js/project_store.mjs's layout, src/sheltersim/storage/projects.py's
//! shapes).
//!
//! A store keeps a Project in a folder named by its id:
//!
//! ```text
//! <project>/project.json                               {schema_version, project_id, archived}
//! <project>/scenarios/<scenario>/v<n>.json             the Scenario revision's record
//! <project>/scenarios/<scenario>/v<n>.scenario.json    the Scenario's canonical JSON
//! <project>/runs/<run>/run.json                        the Run's record
//! <project>/runs/<run>/evidence.json                   evidence_context(scenario)
//! <project>/runs/<run>/branches/<branch>/branch.json   the branch's record
//! ```
//!
//! Records are JSON with sorted keys and `schema_version` 1. The caller lists a folder and reads
//! its files; the functions here keep the names that are identifiers, in the order of their bytes
//! (as the browser's store lists them), and read the records in the order the browser read them,
//! so a missing or unreadable record is refused at the same point.

use serde_json::{Map, Value};

use super::{js_trim_end, utf8, write_object, write_value, Answer, Refusal};
use crate::interpret::merge_objects;

/// A record read: its fields, in the order of their keys.
pub type Record = Map<String, Value>;

/// The folder the store's usage accounts are kept in, which is no Project.
pub const RESERVED: &str = "runtime-accounts";

/// Whether `text` is an identifier the store takes for a folder (Python's `ProjectStore._path`):
/// `^[A-Za-z0-9][A-Za-z0-9._:-]{0,63}$`.
pub fn is_id(text: &str) -> bool {
    let bytes = text.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..].iter().all(|&byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

/// An identifier, or the store's refusal.
pub fn check_id(text: &str) -> Answer<()> {
    if is_id(text) {
        Ok(())
    } else {
        Err(Refusal::storage("invalid artifact identifier").into())
    }
}

/// A record's bytes read: refused where they are not a JSON object of `schema_version` 1.
pub fn parse_record(bytes: &[u8]) -> Answer<Record> {
    let corrupt = || Refusal::storage("saved artifact is missing or corrupt");
    let text = std::str::from_utf8(bytes).map_err(|_| corrupt())?;
    let value: Value = serde_json::from_str(text).map_err(|_| corrupt())?;
    match value {
        Value::Object(record) if record.get("schema_version").and_then(super::js_number) == Some(1.0) => Ok(record),
        _ => Err(Refusal::storage("unsupported saved artifact version; migration required").into()),
    }
}

/// A record that must be there: refused where it is missing.
pub fn read_record(bytes: Option<&[u8]>) -> Answer<Record> {
    match bytes {
        Some(bytes) => parse_record(bytes),
        None => Err(Refusal::storage("saved artifact is missing or corrupt").into()),
    }
}

/// `JSON.stringify` of a record.
pub fn record_json(record: &Record) -> Vec<u8> {
    let mut out = Vec::new();
    write_object(&mut out, record);
    out
}

/// The names of a listing the store reads, as the browser's store reads them: identifiers only,
/// in the order of their bytes.
fn listed<'a, T>(entries: impl IntoIterator<Item = &'a (String, T)>) -> Vec<&'a (String, T)>
where
    T: 'a,
{
    let mut kept: Vec<&(String, T)> = entries.into_iter().filter(|(name, _)| is_id(name)).collect();
    kept.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    kept
}

/// A Run's records: its `run.json` (None where missing) and the `branch.json` of each folder of
/// its `branches/` that holds one, by folder name.
#[derive(Clone, Debug, Default)]
pub struct RunFiles {
    pub run: Option<Vec<u8>>,
    pub branches: Vec<(String, Vec<u8>)>,
}

impl RunFiles {
    /// The Run's record and its branches' (the browser's `runDocument`), after the ids are checked.
    pub fn document(&self, project_id: &str, run_id: &str) -> Answer<(Record, Vec<Record>)> {
        check_id(project_id)?;
        check_id(run_id)?;
        let run = read_record(self.run.as_deref())?;
        let mut branches = Vec::with_capacity(self.branches.len());
        for (_, bytes) in listed(&self.branches) {
            branches.push(parse_record(bytes)?);
        }
        Ok((run, branches))
    }

    /// The record of the branch folder `branch_id` (the browser's `branchRecord`): its id checked,
    /// then its record read.
    pub fn branch_record(&self, project_id: &str, run_id: &str, branch_id: &str) -> Answer<Record> {
        check_id(project_id)?;
        check_id(run_id)?;
        check_id(branch_id)?;
        read_record(self.branches.iter().find(|(name, _)| name == branch_id).map(|(_, bytes)| bytes.as_slice()))
    }
}

/// The record of a branch (the browser's `BrowserApi.branch`): the Run's records read, the branch
/// found by its record's id, and refused where it has no computed minutes.
pub fn branch_of(project_id: &str, run_id: &str, files: &RunFiles, branch_id: &str) -> Answer<Record> {
    let (_, branches) = files.document(project_id, run_id)?;
    let record = branches
        .into_iter()
        .find(|record| record.get("branch_id").and_then(Value::as_str) == Some(branch_id))
        .ok_or_else(|| Refusal::storage("saved artifact is missing or corrupt"))?;
    if record.get("checkpoint_hash").is_none_or(Value::is_null) {
        return Err(Refusal::storage("branch has no computed minutes").into());
    }
    Ok(record)
}

/// `{...run, branches}`, as `JSON.stringify` writes it: the branches last, unless the record has
/// a field of that name.
fn write_run_document(out: &mut Vec<u8>, run: &Record, branches: &[Record]) {
    let mut list = Vec::new();
    list.push(b'[');
    for (index, branch) in branches.iter().enumerate() {
        if index > 0 {
            list.push(b',');
        }
        write_object(&mut list, branch);
    }
    list.push(b']');
    out.push(b'{');
    let mut first = true;
    let mut placed = false;
    for (key, value) in run {
        if !first {
            out.push(b',');
        }
        first = false;
        crate::json::write_json_string(out, key);
        out.push(b':');
        if key == "branches" {
            out.extend_from_slice(&list);
            placed = true;
        } else {
            write_value(out, value);
        }
    }
    if !placed {
        if !first {
            out.push(b',');
        }
        out.extend_from_slice(b"\"branches\":");
        out.extend_from_slice(&list);
    }
    out.push(b'}');
}

/// A number as `JSON.stringify` writes it: an integer's digits, else the shortest digits.
fn js_number_text(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 1e21 {
        format!("{}", value as i128)
    } else if value.is_finite() {
        let mut buffer = ryu::Buffer::new();
        buffer.format_finite(value).to_owned()
    } else {
        "null".to_owned()
    }
}

/// `GET /api/v1/projects`: the `project.json` of each folder of the store that holds one (by
/// folder name; the usage accounts' folder and names that are no identifier are left out), and
/// the store's usage: its bytes and files, and what it may hold (`limit_bytes`).
pub fn projects_answer(projects: &[(String, Vec<u8>)], bytes: u64, files: u64, limit_bytes: Option<f64>) -> Answer {
    let mut out = b"{\"projects\":[".to_vec();
    let kept = listed(projects).into_iter().filter(|(name, _)| name != RESERVED);
    for (index, (_, record)) in kept.enumerate() {
        if index > 0 {
            out.push(b',');
        }
        out.extend_from_slice(&record_json(&parse_record(record)?));
    }
    out.extend_from_slice(format!("],\"storage\":{{\"bytes\":{bytes},\"files\":{files},\"limit_bytes\":").as_bytes());
    match limit_bytes {
        Some(limit) => out.extend_from_slice(js_number_text(limit).as_bytes()),
        None => out.extend_from_slice(b"null"),
    }
    out.extend_from_slice(b"}}");
    Ok(out)
}

/// The files of a Scenario's folder (`scenarios/<scenario>/`): each file's name and bytes.
#[derive(Clone, Debug, Default)]
pub struct ScenarioFolder {
    pub scenario_id: String,
    pub files: Vec<(String, Vec<u8>)>,
}

/// The revision of a record's file name `v<n>.json`, or None.
fn revision(name: &str) -> Option<u32> {
    let digits = name.strip_prefix('v')?.strip_suffix(".json")?;
    let valid = (1..=9).contains(&digits.len()) && digits.as_bytes()[0] != b'0' && digits.bytes().all(|byte| byte.is_ascii_digit());
    valid.then(|| digits.parse().ok()).flatten()
}

/// The Runs of a Project's `runs/` that the store lists: folders named by an identifier that hold
/// a `run.json`, by name.
fn listed_runs(runs: &[(String, RunFiles)]) -> Vec<&(String, RunFiles)> {
    listed(runs).into_iter().filter(|(_, files)| files.run.is_some()).collect()
}

/// `GET /api/v1/projects/{project}`: the Project's record with its Scenarios (each revision's
/// record with the Scenario's JSON as it was written, `scenario`) and its Runs (each with its
/// branches), from its `project.json`, its Scenarios' folders and its Runs' records.
pub fn project_answer(project_id: &str, project: Option<&[u8]>, scenarios: &[ScenarioFolder], runs: &[(String, RunFiles)]) -> Answer {
    check_id(project_id)?;
    let project = read_record(project)?;
    let mut out = record_json(&project);
    out.pop();
    out.extend_from_slice(b",\"scenarios\":[");
    let mut folders: Vec<&ScenarioFolder> = scenarios.iter().filter(|folder| is_id(&folder.scenario_id)).collect();
    folders.sort_by(|a, b| a.scenario_id.as_bytes().cmp(b.scenario_id.as_bytes()));
    let mut first = true;
    for folder in folders {
        let mut names: Vec<(&String, u32)> = folder.files.iter().filter_map(|(name, _)| revision(name).map(|version| (name, version))).collect();
        names.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        let file = |wanted: &str| folder.files.iter().find(|(name, _)| name == wanted).map(|(_, bytes)| bytes.as_slice());
        for (name, version) in names {
            let record = read_record(file(name))?;
            let text = file(&format!("v{version}.scenario.json")).ok_or_else(|| Refusal::storage("saved artifact is missing or corrupt"))?;
            if !first {
                out.push(b',');
            }
            first = false;
            let mut written = record_json(&record);
            written.pop();
            out.extend_from_slice(&written);
            out.extend_from_slice(b",\"scenario\":");
            out.extend_from_slice(js_trim_end(utf8(text)?).as_bytes());
            out.push(b'}');
        }
    }
    out.extend_from_slice(b"],\"runs\":[");
    for (index, (run_id, files)) in listed_runs(runs).into_iter().enumerate() {
        if index > 0 {
            out.push(b',');
        }
        let (run, branches) = files.document(project_id, run_id)?;
        write_run_document(&mut out, &run, &branches);
    }
    out.extend_from_slice(b"]}");
    Ok(out)
}

/// `GET /api/v1/projects/{project}/runs`: the Project's Runs, each with its branches.
pub fn runs_answer(project_id: &str, project: Option<&[u8]>, runs: &[(String, RunFiles)]) -> Answer {
    check_id(project_id)?;
    read_record(project)?;
    let mut out = b"{\"runs\":[".to_vec();
    for (index, (run_id, files)) in listed_runs(runs).into_iter().enumerate() {
        if index > 0 {
            out.push(b',');
        }
        let (run, branches) = files.document(project_id, run_id)?;
        write_run_document(&mut out, &run, &branches);
    }
    out.extend_from_slice(b"]}");
    Ok(out)
}

/// `GET /api/v1/projects/{project}/runs/{run}`: the Run's record with its branches, and the
/// outcome of its last fork (`last_fork`, JSON) where the caller keeps one; with it, the answer's
/// keys are sorted (the answer and the field merged, `interpret::merge_objects`).
pub fn run_answer(project_id: &str, run_id: &str, files: &RunFiles, last_fork: Option<&[u8]>) -> Answer {
    let (run, branches) = files.document(project_id, run_id)?;
    let mut out = Vec::new();
    write_run_document(&mut out, &run, &branches);
    match last_fork {
        None => Ok(out),
        Some(outcome) => {
            let mut extra = b"{\"last_fork\":".to_vec();
            extra.extend_from_slice(outcome);
            extra.push(b'}');
            Ok(merge_objects(&out, &extra)?)
        }
    }
}

/// `GET /api/v1/projects/{project}/runs/{run}/evidence`: the Run's `evidence.json` as it was
/// written, after its record is read.
pub fn evidence_answer(project_id: &str, run_id: &str, run: Option<&[u8]>, evidence: Option<&[u8]>) -> Answer {
    check_id(project_id)?;
    check_id(run_id)?;
    read_record(run)?;
    let evidence = evidence.ok_or_else(|| Refusal::storage("saved artifact is missing or corrupt"))?;
    Ok(js_trim_end(utf8(evidence)?).as_bytes().to_vec())
}
