//! The files of the stored Runs on a disk (`sheltersim_store`, #426) for Python callers: a store's
//! records in the browser's shapes, its locks (the Python server's, on the same files), its limit,
//! a branch's stored Run of format v5 written and read, a branch computed from its head in chunks,
//! a fork and a question's turn. The local and VM servers compute into and read from these in
//! #427; nothing in Python calls them yet.
//!
//! Each method reads its arguments, calls one function of the store and converts what it gives: a
//! record as its JSON text (sorted keys, as the file holds it), bytes as `bytes`. A refusal of the
//! store is a ValueError with Python's `StorageError` text (a leading `code: ` names its code), an
//! error of the core its own class (`core::errors::to_py_err`), and an error of a file an OSError.
//! Waiting for the store's transaction releases the GIL, so a Python thread holding it can end.

use std::path::PathBuf;

use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use serde_json::Value;
use sheltersim_core::run_store::RecordKind;
use sheltersim_store::compute::{self, ForkRequest};
use sheltersim_store::records::{self, Branch, NewRun, Record};
use sheltersim_store::stored::FromStart;
use sheltersim_store::{compress, lock, quota, stored, Error};

use crate::core::errors::{to_py_err, OrPy};
use crate::rules::plain::PlainRun;

fn to_py(error: Error) -> PyErr {
    match error {
        Error::Refused(text) => PyValueError::new_err(text),
        Error::Core(error) => to_py_err(error),
        Error::Io(error) => error.into(),
    }
}

trait OrStore<T> {
    fn or_store(self) -> PyResult<T>;
}

impl<T> OrStore<T> for sheltersim_store::Result<T> {
    fn or_store(self) -> PyResult<T> {
        self.map_err(to_py)
    }
}

/// Files of a listing: `[(name, bytes)]`.
type Named<'py> = Vec<(String, Bound<'py, PyBytes>)>;

fn named(py: Python<'_>, files: Vec<(String, Vec<u8>)>) -> Named<'_> {
    files.into_iter().map(|(name, bytes)| (name, PyBytes::new(py, &bytes))).collect()
}

/// A record's JSON text, as its file holds it without the newline.
fn record_text(record: &Record) -> String {
    let bytes = records::record_bytes(record);
    String::from_utf8_lossy(&bytes[..bytes.len() - 1]).into_owned()
}

fn records_text(listed: &[Record]) -> String {
    format!("[{}]", listed.iter().map(record_text).collect::<Vec<_>>().join(","))
}

fn json_of(text: &str) -> PyResult<Value> {
    serde_json::from_str(text).map_err(|error| PyValueError::new_err(format!("not JSON: {error}")))
}

fn record_kind(kind: &str) -> PyResult<RecordKind> {
    RecordKind::ALL.into_iter().find(|each| each.name() == kind).ok_or_else(|| PyValueError::new_err(format!("no records of kind {kind}")))
}

fn lease_of(lease: &Lease) -> PyResult<&lock::Lease> {
    lease.inner.as_ref().ok_or_else(|| PyValueError::new_err("the lease was released"))
}

/// deflate-raw of `raw` (flate2, zlib's default level).
#[pyfunction]
fn deflate_raw<'py>(py: Python<'py>, raw: &[u8]) -> Bound<'py, PyBytes> {
    PyBytes::new(py, &compress::deflate_raw(raw))
}

/// The bytes of the deflate-raw `stored`, which must hold `raw_length` bytes.
#[pyfunction]
fn inflate_raw<'py>(py: Python<'py>, stored: &[u8], raw_length: u64) -> PyResult<Bound<'py, PyBytes>> {
    Ok(PyBytes::new(py, &compress::inflate_raw(stored, raw_length).or_store()?))
}

/// What the regular files under `root` take: `(bytes, files)`, as Python's `_measure_usage`.
#[pyfunction]
fn storage_usage(root: PathBuf) -> PyResult<(u64, u64)> {
    let usage = quota::usage(&root)?;
    Ok((usage.bytes, usage.files))
}

/// A lock held until it is released (or the object is dropped, or the process ends). A context
/// manager: `with files.claim_branch(...) as lease:`.
#[pyclass(module = "sheltersim_native.rules.run_files")]
pub struct Lease {
    inner: Option<lock::Lease>,
}

#[pymethods]
impl Lease {
    /// The lock's file.
    fn path(&self) -> PyResult<PathBuf> {
        Ok(lease_of(self)?.path().to_path_buf())
    }

    /// Whether the lock is still held by this object.
    fn held(&self) -> bool {
        self.inner.is_some()
    }

    fn release(&mut self) {
        self.inner.take();
    }

    fn __enter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    #[pyo3(signature = (*_args))]
    fn __exit__(&mut self, _args: &Bound<'_, pyo3::types::PyTuple>) -> bool {
        self.inner.take();
        false
    }
}

/// A store's records, locks and limit, and the stored Runs it writes (`sheltersim_store::records::
/// ProjectFiles`).
#[pyclass(frozen, module = "sheltersim_native.rules.run_files")]
pub struct ProjectFiles {
    inner: records::ProjectFiles,
}

#[pymethods]
impl ProjectFiles {
    /// The store at `root` (made where it is not there), which may hold `limit_bytes` (None: no
    /// limit).
    #[new]
    #[pyo3(signature = (root, limit_bytes=None))]
    fn new(root: PathBuf, limit_bytes: Option<u64>) -> PyResult<Self> {
        Ok(Self { inner: records::ProjectFiles::new(&root, limit_bytes).or_store()? })
    }

    /// What the store's files take now: `(bytes, files)`.
    fn usage(&self) -> PyResult<(u64, u64)> {
        let usage = self.inner.usage().or_store()?;
        Ok((usage.bytes, usage.files))
    }

    // ---- Locks ---------------------------------------------------------------------------------

    /// The store's transaction (Python's `ProjectStore.transaction`), waited for without the GIL.
    fn transaction(&self, py: Python<'_>) -> PyResult<Lease> {
        let lease = py.allow_threads(|| self.inner.transaction()).or_store()?;
        Ok(Lease { inner: Some(lease) })
    }

    /// The lease of a branch's writer (Python's `claim_branch`).
    fn claim_branch(&self, py: Python<'_>, project: &str, run: &str, branch: &str) -> PyResult<Lease> {
        let lease = py.allow_threads(|| self.inner.claim_branch(project, run, branch)).or_store()?;
        Ok(Lease { inner: Some(lease) })
    }

    /// The lease of a whole Run (Python's `claim_run`).
    fn claim_run(&self, py: Python<'_>, project: &str, run: &str) -> PyResult<Lease> {
        let lease = py.allow_threads(|| self.inner.claim_run(project, run)).or_store()?;
        Ok(Lease { inner: Some(lease) })
    }

    // ---- Records -------------------------------------------------------------------------------

    fn create_project(&self, project: &str) -> PyResult<String> {
        Ok(record_text(&self.inner.create_project(project).or_store()?))
    }

    fn project(&self, project: &str) -> PyResult<String> {
        Ok(record_text(&self.inner.project(project).or_store()?))
    }

    /// The Projects' records, as a JSON list.
    fn projects(&self) -> PyResult<String> {
        Ok(records_text(&self.inner.projects().or_store()?))
    }

    fn remove_project(&self, project: &str) -> PyResult<()> {
        self.inner.remove_project(project).or_store()
    }

    /// Saves a Scenario Python approved (its canonical JSON, whose sha256 is `scenario_hash`): the
    /// new revision's record.
    fn save_scenario(&self, project: &str, scenario: &[u8], scenario_hash: &str) -> PyResult<String> {
        Ok(record_text(&self.inner.save_scenario(project, scenario, scenario_hash).or_store()?))
    }

    /// The Scenario revisions of a Project: `(record, scenario bytes)` each.
    fn scenarios<'py>(&self, py: Python<'py>, project: &str) -> PyResult<Vec<(String, Bound<'py, PyBytes>)>> {
        Ok(self.inner.scenarios(project).or_store()?.iter().map(|(record, text)| (record_text(record), PyBytes::new(py, text))).collect())
    }

    /// Creates a Run of a saved Scenario revision, queued (`model_versions`: a JSON object of
    /// strings; `evidence`: the JSON bytes of `evidence_context`).
    #[allow(clippy::too_many_arguments)]
    fn create_run(
        &self,
        project: &str,
        run_id: &str,
        scenario_id: &str,
        scenario_version: i64,
        seed: i64,
        model_versions: &str,
        evidence: &[u8],
    ) -> PyResult<String> {
        let model_versions = json_of(model_versions)?;
        let run = NewRun { run_id, scenario_id, scenario_version, seed, model_versions: &model_versions, evidence };
        Ok(record_text(&self.inner.create_run(project, &run).or_store()?))
    }

    fn run_record(&self, project: &str, run: &str) -> PyResult<String> {
        Ok(record_text(&self.inner.run_record(project, run).or_store()?))
    }

    fn run_ids(&self, project: &str) -> PyResult<Vec<String>> {
        self.inner.run_ids(project).or_store()
    }

    fn set_run_status(&self, project: &str, run: &str, status: &str) -> PyResult<String> {
        Ok(record_text(&self.inner.set_run_status(project, run, status).or_store()?))
    }

    fn run_scenario_bytes<'py>(&self, py: Python<'py>, project: &str, run: &str) -> PyResult<Bound<'py, PyBytes>> {
        Ok(PyBytes::new(py, &self.inner.run_scenario_bytes(project, run).or_store()?))
    }

    fn evidence_bytes<'py>(&self, py: Python<'py>, project: &str, run: &str) -> PyResult<Bound<'py, PyBytes>> {
        Ok(PyBytes::new(py, &self.inner.evidence_bytes(project, run).or_store()?))
    }

    fn remove_run(&self, project: &str, run: &str) -> PyResult<()> {
        self.inner.remove_run(project, run).or_store()
    }

    /// Writes a branch's record (the browser's `writeBranch`).
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (project, run, branch_id, parent_branch_id=None, fork_minute=0, computed_until=None, checkpoint_hash=None, status=None))]
    fn write_branch(
        &self,
        project: &str,
        run: &str,
        branch_id: String,
        parent_branch_id: Option<String>,
        fork_minute: i64,
        computed_until: Option<i64>,
        checkpoint_hash: Option<String>,
        status: Option<String>,
    ) -> PyResult<String> {
        let branch = Branch { branch_id, parent_branch_id, fork_minute, computed_until, checkpoint_hash, status };
        Ok(record_text(&self.inner.write_branch(project, run, &branch).or_store()?))
    }

    fn branch_record(&self, project: &str, run: &str, branch: &str) -> PyResult<String> {
        Ok(record_text(&self.inner.branch_record(project, run, branch).or_store()?))
    }

    /// The branches' records of a Run, as a JSON list.
    fn branches(&self, project: &str, run: &str) -> PyResult<String> {
        Ok(records_text(&self.inner.branches(project, run).or_store()?))
    }

    /// The folder of a branch's stored Run.
    fn branch_folder(&self, project: &str, run: &str, branch: &str) -> PyResult<PathBuf> {
        self.inner.branch_folder(project, run, branch).or_store()
    }

    // ---- Stored Runs ---------------------------------------------------------------------------

    /// A branch's stored Run opened to read, with its parents'.
    fn reader(&self, project: &str, run: &str, branch: &str) -> PyResult<StoredRun> {
        Ok(StoredRun { inner: self.inner.reader(project, run, branch).or_store()? })
    }

    /// Starts a branch's stored Run at minute 0 of the exported world `export` (the world `world`
    /// of a file of named worlds): the branch's record at that head.
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (lease, project, run, branch, export, world=None, save_every=360))]
    fn start_branch(&self, lease: &Lease, project: &str, run: &str, branch: &str, export: &[u8], world: Option<&str>, save_every: u64) -> PyResult<String> {
        let start = FromStart { export, world, ticks: 0, save_every };
        Ok(record_text(&self.inner.start_branch(lease_of(lease)?, project, run, branch, &start).or_store()?))
    }

    /// Computes a branch from minute 0 of `export` in one go and stores it: its record.
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (lease, project, run, branch, export, world=None, ticks=1440, save_every=360))]
    fn compute_branch(
        &self,
        lease: &Lease,
        project: &str,
        run: &str,
        branch: &str,
        export: &[u8],
        world: Option<&str>,
        ticks: u64,
        save_every: u64,
    ) -> PyResult<String> {
        let start = FromStart { export, world, ticks, save_every };
        Ok(record_text(&self.inner.compute_branch(lease_of(lease)?, project, run, branch, &start).or_store()?.0))
    }

    /// Goes on with a branch from its record's head toward `target`.
    fn go_on(&self, lease: &Lease, project: &str, run: &str, branch: &str, target: i64) -> PyResult<Computation> {
        Ok(Computation { inner: Some(self.inner.go_on(lease_of(lease)?, project, run, branch, target).or_store()?) })
    }

    /// Makes the branch `child` of `parent` at `minute` with the EffectProgram `program` (its JSON
    /// text): `(the child's record, the receipt's JSON text, the child's state hash)`.
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (lease, project, run, parent, child, minute, program, approved_program_hash, state_hash=None))]
    fn fork(
        &self,
        lease: &Lease,
        project: &str,
        run: &str,
        parent: &str,
        child: &str,
        minute: i64,
        program: &str,
        approved_program_hash: &str,
        state_hash: Option<&str>,
    ) -> PyResult<(String, String, String)> {
        let request = ForkRequest { parent, child, minute, program, approved_program_hash, state_hash };
        let forked = self.inner.fork(lease_of(lease)?, project, run, &request).or_store()?;
        Ok((record_text(&forked.record), forked.receipt, forked.state_hash))
    }

    /// Keeps a question's turn (its JSON text) after the "end" line of a branch's stored Run.
    fn keep_turn(&self, lease: &Lease, project: &str, run: &str, branch: &str, turn: &[u8]) -> PyResult<()> {
        self.inner.keep_turn(lease_of(lease)?, project, run, branch, turn).or_store()
    }

    /// The question turns kept in a branch's stored Run, oldest first; at most the last `limit`.
    #[pyo3(signature = (project, run, branch, limit=100))]
    fn conversations<'py>(&self, py: Python<'py>, project: &str, run: &str, branch: &str, limit: usize) -> PyResult<Vec<Bound<'py, PyBytes>>> {
        Ok(self.inner.conversations(project, run, branch, limit).or_store()?.iter().map(|turn| PyBytes::new(py, turn)).collect())
    }

    /// The latest line of a branch's stored Run a branch forked from it reads up to, or None.
    fn kept_through(&self, project: &str, run: &str, branch: &str) -> PyResult<Option<usize>> {
        self.inner.kept_through(project, run, branch).or_store()
    }

    /// Sets a branch's status and keeps the Run's their summary (Python's `set_branch_status`):
    /// the branch's record.
    #[pyo3(signature = (project, run, branch, status, run_status=None))]
    fn set_branch_status(&self, project: &str, run: &str, branch: &str, status: &str, run_status: Option<&str>) -> PyResult<String> {
        Ok(record_text(&self.inner.set_branch_status(project, run, branch, status, run_status).or_store()?))
    }

    /// Sets a Run's status to `running` while a branch of it is queued or running, else `idle`.
    fn settle_run_status(&self, project: &str, run: &str, idle: &str) -> PyResult<String> {
        Ok(record_text(&self.inner.settle_run_status(project, run, idle).or_store()?))
    }

    // ---- What the answers read (`sheltersim_store::listing`) ---------------------------------

    /// The project.json of each Project's folder: `[(folder, bytes)]`.
    fn projects_listing<'py>(&self, py: Python<'py>) -> PyResult<Vec<(String, Bound<'py, PyBytes>)>> {
        Ok(self.inner.projects_listing().or_store()?.into_iter().map(|(name, bytes)| (name, PyBytes::new(py, &bytes))).collect())
    }

    /// A Run's records: `(run.json or None, [(branch folder, branch.json)])`.
    fn run_listing<'py>(&self, py: Python<'py>, project: &str, run: &str) -> PyResult<(Option<Bound<'py, PyBytes>>, Named<'py>)> {
        let (record, branches) = self.inner.run_listing(project, run).or_store()?;
        Ok((record.map(|bytes| PyBytes::new(py, &bytes)), named(py, branches)))
    }

    /// A Project's records: `(project.json or None, [(scenario id, [(file, bytes)])] where
    /// `scenarios`, [(run folder, run.json or None, [(branch folder, branch.json)])])`.
    #[allow(clippy::type_complexity)]
    fn project_listing<'py>(
        &self,
        py: Python<'py>,
        project: &str,
        scenarios: bool,
    ) -> PyResult<(Option<Bound<'py, PyBytes>>, Vec<(String, Named<'py>)>, Vec<(String, Option<Bound<'py, PyBytes>>, Named<'py>)>)> {
        let (record, listed_scenarios, runs) = self.inner.project_listing(project, scenarios).or_store()?;
        Ok((
            record.map(|bytes| PyBytes::new(py, &bytes)),
            listed_scenarios.into_iter().map(|(id, files)| (id, named(py, files))).collect(),
            runs.into_iter().map(|(id, record, branches)| (id, record.map(|bytes| PyBytes::new(py, &bytes)), named(py, branches))).collect(),
        ))
    }

    /// The file `name` of a Run's folder, or None.
    fn run_file<'py>(&self, py: Python<'py>, project: &str, run: &str, name: &str) -> PyResult<Option<Bound<'py, PyBytes>>> {
        if !matches!(name, "run.json" | "evidence.json") {
            return Err(PyValueError::new_err(format!("no file {name} of a Run is read")));
        }
        Ok(self.inner.run_file(project, run, name).or_store()?.map(|bytes| PyBytes::new(py, &bytes)))
    }

    /// The file `name` of a branch's folder, or None; a journal up to its last whole line.
    fn branch_file<'py>(&self, py: Python<'py>, project: &str, run: &str, branch: &str, name: &str) -> PyResult<Option<Bound<'py, PyBytes>>> {
        if !matches!(name, "branch.json" | "journal.jsonl" | "report.json") {
            return Err(PyValueError::new_err(format!("no file {name} of a branch is read")));
        }
        Ok(self.inner.branch_file(project, run, branch, name).or_store()?.map(|bytes| PyBytes::new(py, &bytes)))
    }

    /// The draft `id` of an intervention's preview, or None.
    fn draft<'py>(&self, py: Python<'py>, project: &str, id: &str) -> PyResult<Option<Bound<'py, PyBytes>>> {
        Ok(self.inner.draft(project, id).or_store()?.map(|bytes| PyBytes::new(py, &bytes)))
    }

    /// Keeps the draft `id` of an intervention's preview. Hold the transaction.
    fn keep_draft(&self, project: &str, id: &str, draft: &[u8]) -> PyResult<()> {
        self.inner.keep_draft(project, id, draft).or_store()
    }

    /// Keeps a branch's report (made when it is first asked for). Hold the transaction.
    fn keep_report(&self, project: &str, run: &str, branch: &str, report: &[u8]) -> PyResult<()> {
        self.inner.keep_report(project, run, branch, report).or_store()
    }

    /// The bundle of a Project (the browser's `exportProject`, `.shelterproj`); `source` is the JSON
    /// text its manifest keeps as what made it.
    #[pyo3(signature = (project, source="null"))]
    fn export_project<'py>(&self, py: Python<'py>, project: &str, source: &str) -> PyResult<Bound<'py, PyBytes>> {
        Ok(PyBytes::new(py, &self.inner.export_project(project, &json_of(source)?).or_store()?))
    }

    /// The branches left `running` that no writer holds become `interrupted`, with their Runs:
    /// `(project, run, branch)` each.
    fn recover_interrupted(&self) -> PyResult<Vec<(String, String, String)>> {
        self.inner.recover_interrupted().or_store()
    }
}

/// A branch's stored Run opened to read (`sheltersim_store::stored::StoredRun`).
#[pyclass(unsendable, module = "sheltersim_native.rules.run_files")]
pub struct StoredRun {
    inner: stored::StoredRun,
}

#[pymethods]
impl StoredRun {
    /// The stored Run in `folder`; a fork's reads its parents' in the folders beside it (the same
    /// Run's `branches`).
    #[staticmethod]
    fn open(folder: PathBuf) -> PyResult<Self> {
        let branches = folder.parent().map(PathBuf::from).unwrap_or_default();
        Ok(Self { inner: stored::StoredRun::open(&folder, &|parent| Ok(branches.join(parent))).or_store()? })
    }

    /// The lines read, as a JSON list of their JSON texts as the journal holds them.
    fn lines_json(&self) -> String {
        format!("[{}]", self.inner.journal().entries().iter().map(|entry| entry.json.as_str()).collect::<Vec<_>>().join(","))
    }

    /// Whether reading stopped at a line cut short or not matching its sha256.
    fn torn(&self) -> bool {
        self.inner.journal().torn()
    }

    /// The lines a branch's head may be: a JSON list of `{kind, minute, index, head}`.
    fn heads_json(&self) -> String {
        let journal = self.inner.journal();
        let heads: Vec<Value> = journal
            .heads()
            .iter()
            .map(|head| {
                serde_json::json!({"kind": head.kind.name(), "minute": head.minute, "index": head.index, "head": journal.entries()[head.index].head})
            })
            .collect();
        Value::Array(heads).to_string()
    }

    fn duration(&self) -> PyResult<i64> {
        self.inner.journal().duration().or_py()
    }

    fn fork_minute(&self) -> PyResult<i64> {
        self.inner.journal().fork_minute().or_py()
    }

    fn save_every(&self) -> PyResult<u64> {
        Ok(self.inner.journal().run().or_py()?.save_every)
    }

    /// The model versions the Run was computed with, as JSON.
    fn model_versions_json(&self) -> PyResult<String> {
        Ok(serde_json::to_string(&self.inner.journal().run().or_py()?.model_versions).expect("strings"))
    }

    /// The pieces of the stored metric samples, in order: `[(kind, bytes)]`, "sample_tables" or
    /// "sample_block" (what `answers.KpiTable` reads).
    fn sample_pieces<'py>(&self, py: Python<'py>) -> PyResult<Vec<(&'static str, Bound<'py, PyBytes>)>> {
        Ok(self.inner.sample_pieces().or_store()?.into_iter().map(|(kind, bytes)| (kind, PyBytes::new(py, &bytes))).collect())
    }

    /// The number of metric samples the "end" line counts, where there is one.
    fn end_samples(&self) -> Option<u64> {
        self.inner.end_samples()
    }

    /// The JSON texts of the records of `kind` ("decisions", "conversations", "llm_tape").
    fn records<'py>(&self, py: Python<'py>, kind: &str) -> PyResult<Vec<Bound<'py, PyBytes>>> {
        Ok(self.inner.records(record_kind(kind)?).or_store()?.iter().map(|record| PyBytes::new(py, record)).collect())
    }

    /// The exported world the Run started from.
    fn export_bytes<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        Ok(PyBytes::new(py, &self.inner.export_bytes().or_store()?))
    }

    /// The Run at `minute`, restored from the nearest saved point at or before it and computed to
    /// it: `(PlainRun, the minute it was computed from, the branch's line restored)`.
    #[pyo3(signature = (minute, decisions=false))]
    fn seek(&mut self, minute: i64, decisions: bool) -> PyResult<(PlainRun, i64, usize)> {
        let sought = self.inner.seek(minute, decisions).or_store()?;
        Ok((PlainRun::of_run(sought.run), sought.from, sought.base))
    }

    /// Every piece read back and checked (the core's `Verify`), as JSON (`Verified::to_json`).
    fn verify_json(&self) -> PyResult<String> {
        let verified = self.inner.verify().or_store()?;
        String::from_utf8(verified.to_json()).map_err(|error| PyRuntimeError::new_err(error.to_string()))
    }
}

/// A branch being computed from its head (`sheltersim_store::compute::Computation`).
#[pyclass(unsendable, module = "sheltersim_native.rules.run_files")]
pub struct Computation {
    inner: Option<compute::Computation>,
}

impl Computation {
    fn get(&mut self) -> PyResult<&mut compute::Computation> {
        self.inner.as_mut().ok_or_else(|| PyValueError::new_err("the computation has ended"))
    }
}

#[pymethods]
impl Computation {
    /// The minute the Run got to.
    fn minute(&mut self) -> PyResult<i64> {
        self.get()?.run.minute().or_py()
    }

    fn target(&mut self) -> PyResult<i64> {
        Ok(self.get()?.target())
    }

    fn duration(&mut self) -> PyResult<i64> {
        Ok(self.get()?.duration())
    }

    /// The branch's head now: `(minute, hash)`.
    fn head(&mut self) -> PyResult<(i64, String)> {
        let (minute, hash) = self.get()?.head();
        Ok((minute, hash.to_owned()))
    }

    /// One chunk of at most `ticks` ticks: the status once the computation ends ("completed",
    /// "paused"), else None.
    fn chunk(&mut self, files: &ProjectFiles, ticks: u64) -> PyResult<Option<&'static str>> {
        self.get()?.chunk(&files.inner, ticks).or_store()
    }

    /// Ends the computation where it got to (a cancel): its status.
    fn stop(&mut self, files: &ProjectFiles) -> PyResult<&'static str> {
        self.get()?.stop(&files.inner).or_store()
    }

    /// Writes the final `status` to the branch's record and the Run's: the branch's record. The
    /// computation cannot be used after.
    fn end(&mut self, files: &ProjectFiles, status: &str) -> PyResult<String> {
        let computation = self.inner.take().ok_or_else(|| PyValueError::new_err("the computation has ended"))?;
        Ok(record_text(&computation.end(&files.inner, status).or_store()?))
    }
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Lease>()?;
    m.add_class::<ProjectFiles>()?;
    m.add_class::<StoredRun>()?;
    m.add_class::<Computation>()?;
    m.add_function(wrap_pyfunction!(deflate_raw, m)?)?;
    m.add_function(wrap_pyfunction!(inflate_raw, m)?)?;
    m.add_function(wrap_pyfunction!(storage_usage, m)?)?;
    m.add("COMPRESSION", compress::COMPRESSION)?;
    m.add("SAVE_EVERY", sheltersim_core::run_store::SAVE_EVERY)?;
    m.add("CAPACITY_EXCEEDED", quota::CAPACITY_EXCEEDED)?;
    m.add("BUNDLE_EXTENSION", sheltersim_store::bundle::BUNDLE_EXTENSION)?;
    Ok(())
}
