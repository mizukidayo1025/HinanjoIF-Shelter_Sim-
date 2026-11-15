//! The Project, Scenario, Run and Branch records of a store, in the browser's shapes and folders
//! (`native/wasm/js/project_store.mjs`; `layout` lists the folders), with the store's refusals:
//!
//! ```text
//!   project.json            {schema_version, project_id, archived}
//!   v<n>.json               {schema_version, scenario_id, scenario_version, scenario_hash, approved}
//!   v<n>.scenario.json      the Scenario's canonical JSON as it was saved; its sha256 is the hash
//!   run.json                {schema_version, project_id, scenario_id, scenario_version,
//!                            scenario_hash, run_id, seed, model_versions, status, archived,
//!                            created_at}; schema version 2 (#447), 1 without created_at
//!   evidence.json           the Run's evidence (`sheltersim_core::initial::evidence`)
//!   branch.json             {schema_version, run_id, branch_id, parent_branch_id, fork_minute,
//!                            computed_until, checkpoint_hash, status}; no status before the
//!                            branch's first computation
//! ```
//!
//! A record is JSON with its keys sorted, no spaces, and a newline: the same bytes as the browser's.
//! A file is written whole (a temporary file flushed, renamed over the record, and the
//! folder flushed, as Python's `ProjectStore._write`), after the store's limit is checked
//! (`quota`). A record written once (project.json) may be written again only with the same bytes.
//! The records are not locked here: a caller that must keep another process out of a change holds
//! the store's transaction (`ProjectFiles::transaction`) around it, as Python does.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Map, Value};
use sheltersim_core::answers::records::{is_created_at, record_of, run_record_of};
use sheltersim_core::checkpoint::sha256_hex;

use crate::compress::{deflate_raw, Compress};
use crate::files::{read_if_there, sync_dir};
use crate::layout::{check_id, is_id, listed, Layout, RESERVED};
use crate::lock::{self, Lease};
use crate::quota::{self, Quota, Usage};
use crate::{Error, Result};

/// A Run's statuses.
pub const STATUSES: [&str; 7] = ["queued", "running", "paused", "completed", "cancelled", "failed", "interrupted"];

// The texts of a record that is not there or is not JSON, and of one of another schema version:
// the core's, which the browser's store refuses with.
pub use sheltersim_core::answers::records::{MISSING, UNSUPPORTED};

/// A record: a JSON object.
pub type Record = Map<String, Value>;

/// The JSON text of `value` with every object's keys sorted, no spaces (the browser's `sortedJson`,
/// Python's `canonical`).
pub fn sorted_json(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                sorted_json(&map[key], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                sorted_json(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// A record's bytes: its sorted JSON and a newline.
pub fn record_bytes(record: &Record) -> Vec<u8> {
    let mut text = String::with_capacity(256);
    sorted_json(&Value::Object(record.clone()), &mut text);
    text.push('\n');
    text.into_bytes()
}

/// The record of `bytes`: a JSON object of schema version 1, or the core's refusal (`record_of`).
pub fn parse_record(bytes: &[u8]) -> Result<Record> {
    record_of(bytes).map_err(Error::refused)
}

/// The schema version of the Run records the store writes: 2 adds `created_at` (#447).
pub const RUN_SCHEMA_VERSION: i64 = 2;

/// A Run's record of `bytes` (`run.json`): schema version 1, or 2 with its `created_at` (the
/// core's `run_record_of`).
pub fn parse_run_record(bytes: &[u8]) -> Result<Record> {
    run_record_of(bytes).map_err(Error::refused)
}

/// A branch's record as a writer gives it (`writeBranch`'s argument): the fork's minute 0 and the
/// computed minute the fork's by default; no status for a branch not computed yet.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Branch {
    pub branch_id: String,
    pub parent_branch_id: Option<String>,
    pub fork_minute: i64,
    pub computed_until: Option<i64>,
    pub checkpoint_hash: Option<String>,
    pub status: Option<String>,
}

impl Branch {
    /// The branch of `record` (a branch's record as read).
    pub fn of_record(record: &Record) -> Result<Self> {
        let text = |key: &str| record.get(key).and_then(Value::as_str).map(str::to_owned);
        Ok(Self {
            branch_id: text("branch_id").ok_or_else(|| Error::refused(INVALID_ID_TEXT))?,
            parent_branch_id: text("parent_branch_id"),
            fork_minute: record.get("fork_minute").and_then(Value::as_i64).ok_or_else(|| Error::refused("invalid fork minute"))?,
            computed_until: Some(record.get("computed_until").and_then(Value::as_i64).ok_or_else(|| Error::refused("invalid computed minute"))?),
            checkpoint_hash: text("checkpoint_hash"),
            status: text("status"),
        })
    }
}

const INVALID_ID_TEXT: &str = crate::layout::INVALID_ID;

/// What a new Run is made of (`createRun`'s argument).
#[derive(Clone, Debug)]
pub struct NewRun<'a> {
    pub run_id: &'a str,
    pub scenario_id: &'a str,
    pub scenario_version: i64,
    pub seed: i64,
    /// The model versions (`sheltersim_core::initial::model_versions`): a JSON object of strings.
    pub model_versions: &'a Value,
    /// The JSON bytes of the Run's evidence (`sheltersim_core::initial::evidence`).
    pub evidence: &'a [u8],
    /// When the Run is created, UTC to the second (`YYYY-MM-DDTHH:MM:SSZ`): the caller's clock.
    pub created_at: &'a str,
}

const MAX_SAFE: i64 = 9_007_199_254_740_991;

fn is_hash(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// The browser's `checkBranch`: a branch's record holds together, and is of the Run `run_id`.
pub fn check_branch(record: &Record, run_id: &str) -> Result<()> {
    let refuse = |text: &str| Err(Error::refused(text));
    match record.get("branch_id").and_then(Value::as_str) {
        Some(id) if is_id(id) => {}
        _ => return refuse(INVALID_ID_TEXT),
    }
    match record.get("parent_branch_id") {
        Some(Value::Null) => {}
        Some(Value::String(id)) if is_id(id) => {}
        _ => return refuse(INVALID_ID_TEXT),
    }
    let fork_minute = match record.get("fork_minute").and_then(Value::as_i64) {
        Some(minute) if (0..=MAX_SAFE).contains(&minute) => minute,
        _ => return refuse("invalid fork minute"),
    };
    match record.get("computed_until").and_then(Value::as_i64) {
        Some(minute) if (fork_minute..=MAX_SAFE).contains(&minute) => {}
        _ => return refuse("invalid computed minute"),
    }
    match record.get("checkpoint_hash") {
        Some(Value::Null) => {}
        Some(Value::String(hash)) if is_hash(hash) => {}
        _ => return refuse("invalid checkpoint hash"),
    }
    match record.get("status") {
        None => {}
        Some(Value::String(status)) if STATUSES.contains(&status.as_str()) => {}
        _ => return refuse("unsupported run status"),
    }
    if record.get("run_id").and_then(Value::as_str) != Some(run_id) {
        return refuse("branch record of another run");
    }
    Ok(())
}

static PENDING: AtomicU64 = AtomicU64::new(0);

/// A store's records, its locks, its limit, and the compressor of the stored Runs it writes.
#[derive(Clone, Debug)]
pub struct ProjectFiles {
    layout: Layout,
    limit: Option<u64>,
    compress: Compress,
}

impl ProjectFiles {
    /// The store at `root` (made where it is not there), which may hold `limit` bytes (none: no
    /// limit).
    pub fn new(root: &Path, limit: Option<u64>) -> Result<Self> {
        if limit == Some(0) {
            return Err(Error::refused("max_bytes must be positive"));
        }
        Ok(Self { layout: Layout::new(root)?, limit, compress: deflate_raw })
    }

    /// The store with another compressor of deflate-raw for the stored Runs it writes (the tests'
    /// stored blocks).
    pub fn with_compress(mut self, compress: Compress) -> Self {
        self.compress = compress;
        self
    }

    /// The compressor of the stored Runs it writes.
    pub fn compress(&self) -> Compress {
        self.compress
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn limit(&self) -> Option<u64> {
        self.limit
    }

    /// What the store's files take now.
    pub fn usage(&self) -> Result<Usage> {
        Ok(quota::usage(self.layout.root())?)
    }

    /// The store's limit and what it holds now, for a writer to count its writes on.
    pub fn quota(&self) -> Result<Quota> {
        Quota::measure(self.layout.root(), self.limit)
    }

    // ---- Locks -----------------------------------------------------------------------------

    /// The store's transaction (`lock::transaction`).
    pub fn transaction(&self) -> Result<Lease> {
        lock::transaction(&self.layout)
    }

    /// The lease of a branch's writer (`lock::claim_branch`); the Run's record must be there.
    pub fn claim_branch(&self, project: &str, run: &str, branch: &str) -> Result<Lease> {
        lock::claim_branch(&self.layout, project, run, branch, || self.run_record(project, run).map(drop))
    }

    /// The lease of a whole Run (`lock::claim_run`); the Run's record must be there.
    pub fn claim_run(&self, project: &str, run: &str) -> Result<Lease> {
        lock::claim_run(&self.layout, project, run, || self.run_record(project, run).map(drop))
    }

    /// Whether `lease` is the lease of the branch's writer.
    pub fn holds_branch(&self, lease: &Lease, project: &str, run: &str, branch: &str) -> Result<bool> {
        Ok(lease.path() == self.layout.branch_file(project, run, branch, "worker-lock")?)
    }

    // ---- Files -------------------------------------------------------------------------------

    fn read_record(&self, path: &Path) -> Result<Record> {
        parse_record(&read_if_there(path)?.ok_or_else(|| Error::refused(MISSING))?)
    }

    /// Writes `bytes` to `path` whole: refused past the store's limit.
    pub(crate) fn write_file(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        let previous = match fs::metadata(path) {
            Ok(metadata) => metadata.len(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
            Err(error) => return Err(error.into()),
        };
        if let Some(limit) = self.limit {
            if (self.usage()?.bytes + bytes.len() as u64).saturating_sub(previous) > limit {
                return Err(Error::refused(quota::CAPACITY_EXCEEDED));
            }
        }
        let folder = path.parent().ok_or_else(|| Error::refused("unsafe artifact path"))?;
        fs::create_dir_all(folder)?;
        let pending = pending_file(folder, bytes)?;
        let renamed = retry_while_shared(|| fs::rename(&pending, path));
        if renamed.is_err() {
            let _ = fs::remove_file(&pending);
        }
        renamed?;
        sync_dir(folder)?;
        Ok(())
    }

    /// Writes a record that may be written again only with the same bytes (Python's immutable
    /// write).
    fn write_once(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        if let Some(old) = read_if_there(path)? {
            return if old == bytes { Ok(()) } else { Err(Error::refused("immutable saved artifact already exists")) };
        }
        if let Some(limit) = self.limit {
            if self.usage()?.bytes + bytes.len() as u64 > limit {
                return Err(Error::refused(quota::CAPACITY_EXCEEDED));
            }
        }
        let folder = path.parent().ok_or_else(|| Error::refused("unsafe artifact path"))?;
        fs::create_dir_all(folder)?;
        let pending = pending_file(folder, bytes)?;
        let linked = fs::hard_link(&pending, path);
        let _ = fs::remove_file(&pending);
        match linked {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if read_if_there(path)?.as_deref() != Some(bytes) {
                    return Err(Error::refused("immutable saved artifact already exists"));
                }
            }
            Err(error) => return Err(error.into()),
        }
        sync_dir(folder)?;
        Ok(())
    }

    // ---- Projects ----------------------------------------------------------------------------

    /// `POST /api/v1/projects`: the new Project's record (also where it is there with the same).
    pub fn create_project(&self, project: &str) -> Result<Record> {
        check_id(project)?;
        if project == RESERVED {
            return Err(Error::refused("project_id_reserved_for_usage_accounts"));
        }
        let record = object([("schema_version", 1.into()), ("project_id", project.into()), ("archived", false.into())]);
        self.write_once(&self.layout.project_file(project, "project.json")?, &record_bytes(&record))?;
        Ok(record)
    }

    /// A Project's record.
    pub fn project(&self, project: &str) -> Result<Record> {
        self.read_record(&self.layout.project_file(project, "project.json")?)
    }

    /// The Projects' records, by id.
    pub fn projects(&self) -> Result<Vec<Record>> {
        let mut records = Vec::new();
        for name in listed(self.layout.root(), true)? {
            if name == RESERVED || !is_id(&name) {
                continue;
            }
            if let Some(bytes) = read_if_there(&self.layout.project_file(&name, "project.json")?)? {
                records.push(parse_record(&bytes)?);
            }
        }
        Ok(records)
    }

    /// Removes a Project and everything in its folder.
    pub fn remove_project(&self, project: &str) -> Result<()> {
        remove_all(&self.layout.project(project)?)
    }

    // ---- Scenarios ---------------------------------------------------------------------------

    /// Saves a Scenario Python approved: `scenario` its canonical JSON, whose sha256 must be
    /// `scenario_hash`. The record of the new revision.
    pub fn save_scenario(&self, project: &str, scenario: &[u8], scenario_hash: &str) -> Result<Record> {
        self.project(project)?;
        let hash = sha256_hex(scenario);
        if hash != scenario_hash {
            return Err(Error::refused("scenario approval hash does not match content"));
        }
        let id = serde_json::from_slice::<Value>(scenario)
            .ok()
            .and_then(|value| value.get("id").and_then(Value::as_str).map(str::to_owned))
            .ok_or_else(|| Error::refused("scenario is not JSON"))?;
        check_id(&id)?;
        let folder = self.layout.scenario(project, &id)?;
        let version = scenario_versions(&folder)?.into_iter().map(|(_, version)| version).max().unwrap_or(0) + 1;
        self.write_file(&folder.join(format!("v{version}.scenario.json")), scenario)?;
        let record = object([
            ("schema_version", 1.into()),
            ("scenario_id", id.as_str().into()),
            ("scenario_version", version.into()),
            ("scenario_hash", hash.into()),
            ("approved", true.into()),
        ]);
        self.write_file(&folder.join(format!("v{version}.json")), &record_bytes(&record))?;
        Ok(record)
    }

    /// The Scenario records of a Project in order (by Scenario id, then file name), each with the
    /// Scenario's bytes as they were saved.
    pub fn scenarios(&self, project: &str) -> Result<Vec<(Record, Vec<u8>)>> {
        let mut listed_scenarios = Vec::new();
        for id in listed(&self.layout.scenarios(project)?, true)? {
            if !is_id(&id) {
                continue;
            }
            let folder = self.layout.scenario(project, &id)?;
            for (name, version) in scenario_versions(&folder)? {
                let record = self.read_record(&folder.join(name))?;
                let text = read_if_there(&folder.join(format!("v{version}.scenario.json")))?.ok_or_else(|| Error::refused(MISSING))?;
                listed_scenarios.push((record, text));
            }
        }
        Ok(listed_scenarios)
    }

    // ---- Runs --------------------------------------------------------------------------------

    /// Creates a Run of a saved Scenario revision, queued. An existing Run of the same Scenario
    /// revision is returned as it is.
    pub fn create_run(&self, project: &str, run: &NewRun<'_>) -> Result<Record> {
        self.project(project)?;
        if !(1..=MAX_SAFE).contains(&run.scenario_version) {
            return Err(Error::refused("invalid scenario revision"));
        }
        let scenario = self.read_record(&self.layout.scenario(project, run.scenario_id)?.join(format!("v{}.json", run.scenario_version)))?;
        if !(0..=MAX_SAFE).contains(&run.seed) {
            return Err(Error::refused("invalid seed"));
        }
        match run.model_versions {
            Value::Object(versions) if versions.values().all(Value::is_string) => {}
            _ => return Err(Error::refused("invalid model versions")),
        }
        if !is_created_at(run.created_at) {
            return Err(Error::refused("invalid creation time"));
        }
        let path = self.layout.run_file(project, run.run_id, "run.json")?;
        if let Some(old) = read_if_there(&path)? {
            let old = parse_run_record(&old)?;
            if old.get("scenario_id").and_then(Value::as_str) != Some(run.scenario_id)
                || old.get("scenario_version").and_then(Value::as_i64) != Some(run.scenario_version)
            {
                return Err(Error::refused("run_id_conflict"));
            }
            return Ok(old);
        }
        parse_record(run.evidence)?;
        self.write_file(&self.layout.run_file(project, run.run_id, "evidence.json")?, run.evidence)?;
        let record = object([
            ("schema_version", RUN_SCHEMA_VERSION.into()),
            ("project_id", project.into()),
            ("scenario_id", run.scenario_id.into()),
            ("scenario_version", run.scenario_version.into()),
            ("scenario_hash", scenario.get("scenario_hash").cloned().unwrap_or(Value::Null)),
            ("run_id", run.run_id.into()),
            ("seed", run.seed.into()),
            ("model_versions", run.model_versions.clone()),
            ("status", "queued".into()),
            ("archived", false.into()),
            ("created_at", run.created_at.into()),
        ]);
        self.write_file(&path, &record_bytes(&record))?;
        Ok(record)
    }

    /// A Run's record.
    pub fn run_record(&self, project: &str, run: &str) -> Result<Record> {
        parse_run_record(&read_if_there(&self.layout.run_file(project, run, "run.json")?)?.ok_or_else(|| Error::refused(MISSING))?)
    }

    /// The ids of a Project's Runs (the folders with a record), sorted.
    pub fn run_ids(&self, project: &str) -> Result<Vec<String>> {
        let mut ids = Vec::new();
        for name in listed(&self.layout.runs(project)?, true)? {
            if is_id(&name) && self.layout.run_file(project, &name, "run.json")?.is_file() {
                ids.push(name);
            }
        }
        Ok(ids)
    }

    /// Sets a Run's status.
    pub fn set_run_status(&self, project: &str, run: &str, status: &str) -> Result<Record> {
        if !STATUSES.contains(&status) {
            return Err(Error::refused("unsupported run status"));
        }
        let path = self.layout.run_file(project, run, "run.json")?;
        let mut record = self.run_record(project, run)?;
        record.insert("status".to_owned(), status.into());
        self.write_file(&path, &record_bytes(&record))?;
        Ok(record)
    }

    /// Sets a branch's status and keeps the Run's status their summary (#48): the Run is `running` while any of its branches is queued or
    /// running, else `run_status`, or this branch's status. The branch's record.
    pub fn set_branch_status(&self, project: &str, run: &str, branch: &str, status: &str, run_status: Option<&str>) -> Result<Record> {
        if !STATUSES.contains(&status) {
            return Err(Error::refused("unsupported run status"));
        }
        let mut record = Branch::of_record(&self.branch_record(project, run, branch)?)?;
        record.status = Some(status.to_owned());
        let record = self.write_branch(project, run, &record)?;
        self.settle_run_status(project, run, run_status.unwrap_or(status))?;
        Ok(record)
    }

    /// Sets a Run's status to `running` while any of its branches is queued or running, else to
    /// `idle`. The Run's record.
    pub fn settle_run_status(&self, project: &str, run: &str, idle: &str) -> Result<Record> {
        let busy = self.branches(project, run)?.iter().any(|record| matches!(record.get("status").and_then(Value::as_str), Some("queued" | "running")));
        self.set_run_status(project, run, if busy { "running" } else { idle })
    }

    /// The Scenario of a Run, as the bytes saved.
    pub fn run_scenario_bytes(&self, project: &str, run: &str) -> Result<Vec<u8>> {
        let record = self.run_record(project, run)?;
        let scenario = record.get("scenario_id").and_then(Value::as_str).ok_or_else(|| Error::refused(MISSING))?;
        let version = record.get("scenario_version").and_then(Value::as_i64).ok_or_else(|| Error::refused(MISSING))?;
        read_if_there(&self.layout.scenario(project, scenario)?.join(format!("v{version}.scenario.json")))?.ok_or_else(|| Error::refused(MISSING))
    }

    /// A Run's evidence, as the bytes saved.
    pub fn evidence_bytes(&self, project: &str, run: &str) -> Result<Vec<u8>> {
        self.run_record(project, run)?;
        read_if_there(&self.layout.run_file(project, run, "evidence.json")?)?.ok_or_else(|| Error::refused(MISSING))
    }

    /// Removes a Run and everything in its folder; its record is read first, so a Run that is not
    /// there is refused.
    pub fn remove_run(&self, project: &str, run: &str) -> Result<()> {
        self.run_record(project, run)?;
        remove_all(&self.layout.run(project, run)?)
    }

    // ---- Branches ----------------------------------------------------------------------------

    /// Writes a branch's record. A child's parent must have a record; a branch's lineage (parent
    /// and fork minute) never changes.
    pub fn write_branch(&self, project: &str, run: &str, branch: &Branch) -> Result<Record> {
        let mut record = object([
            ("schema_version", 1.into()),
            ("run_id", run.into()),
            ("branch_id", branch.branch_id.as_str().into()),
            ("parent_branch_id", branch.parent_branch_id.as_deref().map_or(Value::Null, Value::from)),
            ("fork_minute", branch.fork_minute.into()),
            ("computed_until", branch.computed_until.unwrap_or(branch.fork_minute).into()),
            ("checkpoint_hash", branch.checkpoint_hash.as_deref().map_or(Value::Null, Value::from)),
        ]);
        if let Some(status) = &branch.status {
            record.insert("status".to_owned(), status.as_str().into());
        }
        check_branch(&record, run)?;
        self.run_record(project, run)?;
        if let Some(parent) = &branch.parent_branch_id {
            self.branch_record(project, run, parent)?;
        }
        let path = self.layout.branch_file(project, run, &branch.branch_id, "branch.json")?;
        if let Some(old) = read_if_there(&path)? {
            let old = parse_record(&old)?;
            if old.get("parent_branch_id") != record.get("parent_branch_id") || old.get("fork_minute").and_then(Value::as_i64) != Some(branch.fork_minute) {
                return Err(Error::refused("branch lineage is immutable"));
            }
        }
        self.write_file(&path, &record_bytes(&record))?;
        Ok(record)
    }

    /// A branch's record.
    pub fn branch_record(&self, project: &str, run: &str, branch: &str) -> Result<Record> {
        self.read_record(&self.layout.branch_file(project, run, branch, "branch.json")?)
    }

    /// The branches' records of a Run, by branch id.
    pub fn branches(&self, project: &str, run: &str) -> Result<Vec<Record>> {
        let mut records = Vec::new();
        for name in listed(&self.layout.branches(project, run)?, true)? {
            if !is_id(&name) {
                continue;
            }
            if let Some(bytes) = read_if_there(&self.layout.branch_file(project, run, &name, "branch.json")?)? {
                records.push(parse_record(&bytes)?);
            }
        }
        Ok(records)
    }

    /// The folder of a branch's stored Run.
    pub fn branch_folder(&self, project: &str, run: &str, branch: &str) -> Result<PathBuf> {
        self.layout.branch(project, run, branch)
    }
}

/// A JSON object of `entries`.
fn object<const N: usize>(entries: [(&str, Value); N]) -> Record {
    entries.into_iter().map(|(key, value)| (key.to_owned(), value)).collect()
}

/// The Scenario revisions' record files of a folder: `(name, version)`, by name.
fn scenario_versions(folder: &Path) -> Result<Vec<(String, i64)>> {
    let mut versions = Vec::new();
    for name in listed(folder, false)? {
        let Some(number) = name.strip_prefix('v').and_then(|rest| rest.strip_suffix(".json")) else { continue };
        let digits = number.as_bytes();
        if (1..=9).contains(&digits.len()) && digits[0] != b'0' && digits.iter().all(u8::is_ascii_digit) {
            let version = number.parse().expect("1 to 9 digits");
            versions.push((name, version));
        }
    }
    Ok(versions)
}

/// A new temporary file in `folder` holding `bytes`, flushed to the disk.
fn pending_file(folder: &Path, bytes: &[u8]) -> Result<PathBuf> {
    loop {
        let path = folder.join(format!(".pending-{}-{}", std::process::id(), PENDING.fetch_add(1, Ordering::Relaxed)));
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        };
        let written = file.write_all(bytes).and_then(|()| file.sync_all());
        if let Err(error) = written {
            drop(file);
            let _ = fs::remove_file(&path);
            return Err(error.into());
        }
        return Ok(path);
    }
}

/// Runs a file action, again for a moment on Windows while another handle has the file open
/// (Python's `retry_while_shared`); once elsewhere.
fn retry_while_shared<T>(mut action: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    if !cfg!(windows) {
        return action();
    }
    let mut attempt = 0;
    loop {
        match action() {
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied && attempt < 49 => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            other => return other,
        }
    }
}

fn remove_all(folder: &Path) -> Result<()> {
    match fs::remove_dir_all(folder) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}
