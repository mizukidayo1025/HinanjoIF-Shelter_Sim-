//! The answers of the API's Project routes (`sheltersim_core::answers`, #449) for Python: the
//! local and VM versions answer `/api/v1/projects/...` with the same bytes as the browser
//! (`native/wasm/src/answers.rs`), from the files their store reads.
//!
//! The caller reads the files a route needs and hands them over as bytes (None where a file is
//! missing): a Run's records as `run` (its `run.json`) and `branches` (`(folder name, bytes of its
//! branch.json)` for each folder of its `branches/` that holds one), a Project's Scenarios as
//! `(scenario id, [(file name, bytes)])` and its Runs as `(folder name, run, branches)`. A function
//! gives the answer's JSON bytes. A refusal raises `Refused(status, error, code, body)`: the HTTP
//! status, the text, the code (or None) and the body the server answers it with. Any other error
//! is the core's, as its Python class.

use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use sheltersim_core::answers::{frames, history, interventions, pdf, questions, records, report, routes, Failure, Refusal};
use sheltersim_core::history::KpiTable;
use sheltersim_core::interpret::Step;

use crate::core::errors::{to_py_err, OrPy};

create_exception!(sheltersim_native.rules.answers, Refused, PyException, "A refusal of a request: (status, error, code, body).");

fn failed(failure: Failure) -> PyErr {
    match failure {
        Failure::Refused(refusal) => {
            let body = refusal.body();
            Python::with_gil(|py| Refused::new_err((refusal.status, refusal.error, refusal.code, PyBytes::new(py, &body).unbind())))
        }
        Failure::Error(error) => to_py_err(error),
    }
}

fn bytes<'py>(py: Python<'py>, result: sheltersim_core::answers::Answer) -> PyResult<Bound<'py, PyBytes>> {
    Ok(PyBytes::new(py, &result.map_err(failed)?))
}

type Branches = Vec<(String, Vec<u8>)>;

fn run_files(run: Option<Vec<u8>>, branches: Branches) -> records::RunFiles {
    records::RunFiles { run, branches }
}

fn runs_of(runs: Vec<(String, Option<Vec<u8>>, Branches)>) -> Vec<(String, records::RunFiles)> {
    runs.into_iter().map(|(name, run, branches)| (name, run_files(run, branches))).collect()
}

/// Whether `text` is an identifier the store takes for a folder.
#[pyfunction]
fn is_artifact_id(text: &str) -> bool {
    records::is_id(text)
}

/// `GET /api/v1/projects`: the `project.json` of each folder of the store, by folder name; the
/// store's usage in bytes and files, and what it may hold.
#[pyfunction]
#[pyo3(signature = (projects, usage_bytes, usage_files, limit_bytes=None))]
fn projects_answer<'py>(
    py: Python<'py>,
    projects: Vec<(String, Vec<u8>)>,
    usage_bytes: u64,
    usage_files: u64,
    limit_bytes: Option<f64>,
) -> PyResult<Bound<'py, PyBytes>> {
    bytes(py, records::projects_answer(&projects, usage_bytes, usage_files, limit_bytes))
}

/// `GET /api/v1/projects/{project}`.
#[pyfunction]
#[pyo3(signature = (project_id, project, scenarios, runs))]
fn project_answer<'py>(
    py: Python<'py>,
    project_id: &str,
    project: Option<Vec<u8>>,
    scenarios: Vec<(String, Vec<(String, Vec<u8>)>)>,
    runs: Vec<(String, Option<Vec<u8>>, Branches)>,
) -> PyResult<Bound<'py, PyBytes>> {
    let scenarios: Vec<records::ScenarioFolder> = scenarios.into_iter().map(|(scenario_id, files)| records::ScenarioFolder { scenario_id, files }).collect();
    bytes(py, records::project_answer(project_id, project.as_deref(), &scenarios, &runs_of(runs)))
}

/// `GET /api/v1/projects/{project}/runs`.
#[pyfunction]
#[pyo3(signature = (project_id, project, runs))]
fn runs_answer<'py>(py: Python<'py>, project_id: &str, project: Option<Vec<u8>>, runs: Vec<(String, Option<Vec<u8>>, Branches)>) -> PyResult<Bound<'py, PyBytes>> {
    bytes(py, records::runs_answer(project_id, project.as_deref(), &runs_of(runs)))
}

/// `GET /api/v1/projects/{project}/runs/{run}`, with the last fork's outcome (JSON) where one is
/// kept.
#[pyfunction]
#[pyo3(signature = (project_id, run_id, run, branches, last_fork=None))]
fn run_answer<'py>(
    py: Python<'py>,
    project_id: &str,
    run_id: &str,
    run: Option<Vec<u8>>,
    branches: Branches,
    last_fork: Option<Vec<u8>>,
) -> PyResult<Bound<'py, PyBytes>> {
    bytes(py, records::run_answer(project_id, run_id, &run_files(run, branches), last_fork.as_deref()))
}

/// `GET /api/v1/projects/{project}/runs/{run}/evidence`.
#[pyfunction]
#[pyo3(signature = (project_id, run_id, run, evidence))]
fn evidence_answer<'py>(py: Python<'py>, project_id: &str, run_id: &str, run: Option<Vec<u8>>, evidence: Option<Vec<u8>>) -> PyResult<Bound<'py, PyBytes>> {
    bytes(py, records::evidence_answer(project_id, run_id, run.as_deref(), evidence.as_deref()))
}

/// The record of a branch with computed minutes, as its JSON (the browser's `BrowserApi.branch`).
#[pyfunction]
#[pyo3(signature = (project_id, run_id, run, branches, branch_id))]
fn branch_of<'py>(py: Python<'py>, project_id: &str, run_id: &str, run: Option<Vec<u8>>, branches: Branches, branch_id: &str) -> PyResult<Bound<'py, PyBytes>> {
    bytes(py, records::branch_of(project_id, run_id, &run_files(run, branches), branch_id).map(|record| records::record_json(&record)))
}

/// `GET …/branches/{branch}/llm-status`: `query` the text after "?", `branch` the bytes of the
/// branch's `branch.json`.
#[pyfunction]
#[pyo3(signature = (query, project_id, run_id, branch_id, branch))]
fn llm_status_answer<'py>(py: Python<'py>, query: &str, project_id: &str, run_id: &str, branch_id: &str, branch: Option<Vec<u8>>) -> PyResult<Bound<'py, PyBytes>> {
    bytes(py, routes::llm_status_answer(query, project_id, run_id, branch_id, branch.as_deref()))
}

/// `GET …/branches/{branch}/conversations`, after `branch_of`: the branch's "conversations"
/// pieces, decompressed, in the order of its journal.
#[pyfunction]
fn conversations_answer<'py>(py: Python<'py>, pieces: Vec<Vec<u8>>) -> PyResult<Bound<'py, PyBytes>> {
    let raws: Vec<&[u8]> = pieces.iter().map(Vec::as_slice).collect();
    bytes(py, routes::conversations_answer(&raws))
}

/// `POST runs/{run}/pdf-report`: always raises (`Refused`, or the core's error).
#[pyfunction]
#[pyo3(signature = (project_id, run_id, body, run, branches))]
fn pdf_report_answer(project_id: &str, run_id: &str, body: Option<Vec<u8>>, run: Option<Vec<u8>>, branches: Branches) -> PyResult<()> {
    Err(failed(routes::pdf_report_answer(project_id, run_id, body.as_deref(), &run_files(run, branches))))
}

/// `POST runs/{run}/delete`: the answer, once the caller removes the Run's folder; `active`
/// whether a computation of the Run is running.
#[pyfunction]
#[pyo3(signature = (project_id, run_id, body, active, run, branches))]
fn delete_answer<'py>(
    py: Python<'py>,
    project_id: &str,
    run_id: &str,
    body: Option<Vec<u8>>,
    active: bool,
    run: Option<Vec<u8>>,
    branches: Branches,
) -> PyResult<Bound<'py, PyBytes>> {
    bytes(py, routes::delete_answer(project_id, run_id, body.as_deref(), active, &run_files(run, branches)))
}

/// A branch's metric samples read back (`history::KpiTable`): its stored Run's "sample_tables"
/// and "sample_block" pieces, decompressed, in the order of its journal.
#[pyclass(name = "KpiTable", module = "sheltersim_native.rules.answers")]
#[derive(Default)]
pub struct PyKpiTable {
    table: KpiTable,
}

#[pymethods]
impl PyKpiTable {
    #[new]
    fn new() -> Self {
        Self::default()
    }

    fn read_sample_tables(&mut self, raw: &[u8]) -> PyResult<()> {
        self.table.read_tables(raw).or_py()
    }

    /// How many samples the block held.
    fn read_sample_block(&mut self, raw: &[u8]) -> PyResult<usize> {
        self.table.read_block(raw).or_py()
    }

    /// How many samples were read.
    fn sample_count(&self) -> usize {
        self.table.len()
    }
}

/// A history request the server takes (`answers::history::HistoryPlan`): `history_plan` checks
/// it; the caller reads the branch's samples where it has a head (`anchor`), and `answer` writes
/// the page.
#[pyclass(name = "HistoryPlan", module = "sheltersim_native.rules.answers")]
pub struct PyHistoryPlan {
    plan: history::HistoryPlan,
}

#[pymethods]
impl PyHistoryPlan {
    /// The branch's head, or None for a branch without one (whose answer reads no samples).
    fn anchor(&self) -> Option<String> {
        self.plan.anchor().map(str::to_owned)
    }

    /// The answer, from the branch's samples read back (an empty table without a head).
    fn answer<'py>(&self, py: Python<'py>, table: &PyKpiTable) -> PyResult<Bound<'py, PyBytes>> {
        bytes(py, self.plan.answer(&table.table))
    }
}

/// `GET …/branches/{branch}/history`, its checks: `query` the text after "?", `branch` the bytes
/// of the branch's `branch.json`.
#[pyfunction]
#[pyo3(signature = (query, project_id, run_id, branch_id, branch))]
fn history_plan(query: &str, project_id: &str, run_id: &str, branch_id: &str, branch: Option<Vec<u8>>) -> PyResult<PyHistoryPlan> {
    Ok(PyHistoryPlan { plan: history::HistoryPlan::new(query, project_id, run_id, branch_id, branch.as_deref()).map_err(failed)? })
}

/// `GET …/branches/{branch}/report`, from what is kept (`branch.json`, `report.json`): the kept
/// report as it is, `{"status":"not_generated"}`, or None: make it (`build_report`).
#[pyfunction]
#[pyo3(signature = (query, project_id, run_id, branch_id, branch, kept))]
fn report_answer<'py>(
    py: Python<'py>,
    query: &str,
    project_id: &str,
    run_id: &str,
    branch_id: &str,
    branch: Option<Vec<u8>>,
    kept: Option<Vec<u8>>,
) -> PyResult<Option<Bound<'py, PyBytes>>> {
    let answer = report::report_answer(query, project_id, run_id, branch_id, branch.as_deref(), kept.as_deref()).map_err(failed)?;
    Ok(answer.map(|bytes| PyBytes::new(py, &bytes)))
}

/// The report of a Run at its last minute (`answers::report::build_report`), for the Scenario's
/// JSON bytes and the branch `branch_id`.
#[pyfunction]
fn build_report<'py>(py: Python<'py>, run: &crate::rules::plain::PlainRun, scenario: &[u8], branch_id: &str) -> PyResult<Bound<'py, PyBytes>> {
    bytes(py, report::build_report(run.core(), scenario, branch_id))
}


fn records_json(records: &[records::Record]) -> Vec<u8> {
    sheltersim_core::answers::list(records.iter().map(records::record_json))
}

/// A fork receipt from Python: `(branch_id, minute, state_hash, checkpoint_hash)`.
type ReceiptTuple = (String, i64, String, Option<String>);

fn receipts_of(receipts: Vec<Option<ReceiptTuple>>) -> Vec<Option<frames::Receipt>> {
    receipts
        .into_iter()
        .map(|receipt| receipt.map(|(branch_id, minute, state_hash, checkpoint_hash)| frames::Receipt { branch_id, minute, state_hash, checkpoint_hash }))
        .collect()
}

/// `GET …/branches/{branch}/timeline?minute=`, its checks (`minute` the query's value): the
/// branch's record (JSON) and the minute whose frame is the answer.
#[pyfunction]
#[pyo3(signature = (minute, project_id, run_id, run, branches, branch_id))]
fn timeline_plan<'py>(
    py: Python<'py>,
    minute: &str,
    project_id: &str,
    run_id: &str,
    run: Option<Vec<u8>>,
    branches: Branches,
    branch_id: &str,
) -> PyResult<(Bound<'py, PyBytes>, i64)> {
    let (record, minute) = frames::timeline_plan(minute, project_id, run_id, &run_files(run, branches), branch_id).map_err(failed)?;
    Ok((PyBytes::new(py, &records::record_json(&record)), minute))
}

/// `GET runs/{run}/compare?branch_ids=&minute=` (`frames::ComparePlan`): its checks
/// (`compare_plan`), then the branches' fork receipts (`receipts` of their journals, or
/// `receipts_of` those read already), then the answer of their frames.
#[pyclass(name = "ComparePlan", module = "sheltersim_native.rules.answers")]
pub struct PyComparePlan {
    plan: frames::ComparePlan,
    comparison: Option<frames::Comparison>,
}

#[pyfunction]
#[pyo3(signature = (query, project_id, run_id, run, branches))]
fn compare_plan(query: &str, project_id: &str, run_id: &str, run: Option<Vec<u8>>, branches: Branches) -> PyResult<PyComparePlan> {
    Ok(PyComparePlan { plan: frames::ComparePlan::new(query, project_id, run_id, &run_files(run, branches)).map_err(failed)?, comparison: None })
}

#[pymethods]
impl PyComparePlan {
    #[getter]
    fn branch_ids(&self) -> Vec<String> {
        self.plan.branch_ids.clone()
    }

    #[getter]
    fn minute(&self) -> i64 {
        self.plan.minute
    }

    /// The branches' records, as a JSON list in the order of `branch_ids`.
    fn records_json<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &records_json(&self.plan.records))
    }

    /// The branches' journals (`journal.jsonl`, empty where there is none), in their order.
    fn receipts(&mut self, journals: Vec<Vec<u8>>) -> PyResult<()> {
        let raws: Vec<&[u8]> = journals.iter().map(Vec::as_slice).collect();
        self.comparison = Some(frames::Comparison::new(self.plan.branch_ids.clone(), self.plan.common_range, &raws).map_err(failed)?);
        Ok(())
    }

    /// The branches' fork receipts read already: `(branch_id, minute, state_hash,
    /// checkpoint_hash)` or None, in their order.
    fn receipts_of(&mut self, receipts: Vec<Option<ReceiptTuple>>) {
        self.comparison = Some(frames::Comparison::of_receipts(self.plan.branch_ids.clone(), self.plan.common_range, receipts_of(receipts)));
    }

    /// The answer of the branches' frames (JSON bytes, in the order of `branch_ids`).
    fn answer<'py>(&self, py: Python<'py>, run_id: &str, frames: Vec<Vec<u8>>) -> PyResult<Bound<'py, PyBytes>> {
        let comparison = self.comparison.as_ref().ok_or_else(|| pyo3::exceptions::PyRuntimeError::new_err("the receipts are not read"))?;
        let raws: Vec<&[u8]> = frames.iter().map(Vec::as_slice).collect();
        bytes(py, comparison.answer(run_id, self.plan.minute, &raws))
    }
}

/// `GET runs/{run}/replay-frames?branch_ids=&start=&end=` (`frames::ReplayPlan`): its checks; for
/// two branches, their receipts and each minute's comparison (`compare`); then the answer.
#[pyclass(name = "ReplayPlan", module = "sheltersim_native.rules.answers")]
pub struct PyReplayPlan {
    plan: frames::ReplayPlan,
    comparison: Option<frames::Comparison>,
}

#[pyfunction]
#[pyo3(signature = (query, project_id, run_id, run, branches))]
fn replay_plan(query: &str, project_id: &str, run_id: &str, run: Option<Vec<u8>>, branches: Branches) -> PyResult<PyReplayPlan> {
    Ok(PyReplayPlan { plan: frames::ReplayPlan::new(query, project_id, run_id, &run_files(run, branches)).map_err(failed)?, comparison: None })
}

#[pymethods]
impl PyReplayPlan {
    #[getter]
    fn branch_ids(&self) -> Vec<String> {
        self.plan.branch_ids.clone()
    }

    #[getter]
    fn start(&self) -> i64 {
        self.plan.start
    }

    #[getter]
    fn end(&self) -> i64 {
        self.plan.end
    }

    /// The branches' records, as a JSON list in the order of `branch_ids`.
    fn records_json<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &records_json(&self.plan.records))
    }

    /// For two branches: the comparison's checks (the Run's records) and their journals.
    #[pyo3(signature = (project_id, run_id, run, branches, journals))]
    fn receipts(&mut self, project_id: &str, run_id: &str, run: Option<Vec<u8>>, branches: Branches, journals: Vec<Vec<u8>>) -> PyResult<()> {
        let range = self.plan.comparison_range(project_id, run_id, &run_files(run, branches)).map_err(failed)?;
        let raws: Vec<&[u8]> = journals.iter().map(Vec::as_slice).collect();
        self.comparison = Some(frames::Comparison::new(self.plan.branch_ids.clone(), range, &raws).map_err(failed)?);
        Ok(())
    }

    /// For two branches: the comparison's checks and their fork receipts read already.
    #[pyo3(signature = (project_id, run_id, run, branches, receipts))]
    fn receipts_of(&mut self, project_id: &str, run_id: &str, run: Option<Vec<u8>>, branches: Branches, receipts: Vec<Option<ReceiptTuple>>) -> PyResult<()> {
        let range = self.plan.comparison_range(project_id, run_id, &run_files(run, branches)).map_err(failed)?;
        self.comparison = Some(frames::Comparison::of_receipts(self.plan.branch_ids.clone(), range, receipts_of(receipts)));
        Ok(())
    }

    /// For two branches: the comparison of their frames at `minute`.
    fn compare<'py>(&self, py: Python<'py>, run_id: &str, minute: i64, frames: Vec<Vec<u8>>) -> PyResult<Bound<'py, PyBytes>> {
        let comparison = self.comparison.as_ref().ok_or_else(|| pyo3::exceptions::PyRuntimeError::new_err("the receipts are not read"))?;
        let raws: Vec<&[u8]> = frames.iter().map(Vec::as_slice).collect();
        bytes(py, comparison.answer(run_id, minute, &raws))
    }

    /// The answer of the samples (frames, or comparisons), one for each minute.
    fn answer<'py>(&self, py: Python<'py>, samples: Vec<Vec<u8>>) -> Bound<'py, PyBytes> {
        let raws: Vec<&[u8]> = samples.iter().map(Vec::as_slice).collect();
        PyBytes::new(py, &self.plan.answer(&raws))
    }
}

/// Python's `_require_current_model` of a branch: its `journal.jsonl` and the Run's Scenario.
#[pyfunction]
fn require_current_model(journal: &[u8], scenario: &[u8]) -> PyResult<()> {
    frames::require_current_model(journal, scenario).map_err(failed)
}

/// Whether the branch's stored Run is view-only (computed with other model versions than the
/// core's); false for a branch without a stored Run.
#[pyfunction]
fn view_only(journal: &[u8], scenario: &[u8]) -> PyResult<bool> {
    Ok(matches!(frames::model_view(journal, scenario).map_err(failed)?, Some((_, _, status)) if !status.current()))
}

/// The frame of a view-only Run (`frames::view_frame`): the frame of its saved point at or before
/// the minute asked for, with the fields the answer adds.
#[pyfunction]
fn view_frame<'py>(py: Python<'py>, frame: &[u8], journal: &[u8], scenario: &[u8], saved_minute: i64, requested_minute: i64) -> PyResult<Bound<'py, PyBytes>> {
    let Some((_, _, status)) = frames::model_view(journal, scenario).map_err(failed)? else {
        return Err(pyo3::exceptions::PyValueError::new_err("the branch has no stored Run"));
    };
    bytes(py, frames::view_frame(frame, &status, saved_minute, requested_minute))
}


/// The checks of comparing branches at a minute (`frames::comparison_range`): the common range.
#[pyfunction]
#[pyo3(signature = (branch_ids, minute, project_id, run_id, run, branches))]
fn comparison_range(branch_ids: Vec<String>, minute: Option<i64>, project_id: &str, run_id: &str, run: Option<Vec<u8>>, branches: Branches) -> PyResult<(i64, i64)> {
    frames::comparison_range(&branch_ids, minute.map(|minute| minute as f64), project_id, run_id, &run_files(run, branches)).map_err(failed)
}

/// `POST …/intervention-preview` (`interventions::PreviewPlan`): its checks; then, after the
/// model versions are checked (`check_typed`), the answer of the branch's Run at `minute` and
/// the draft to keep.
#[pyclass(name = "PreviewPlan", module = "sheltersim_native.rules.answers")]
pub struct PyPreviewPlan {
    plan: interventions::PreviewPlan,
}

#[pyfunction]
#[pyo3(signature = (body, project_id, run_id, run, branches, branch_id))]
fn preview_plan(body: Option<Vec<u8>>, project_id: &str, run_id: &str, run: Option<Vec<u8>>, branches: Branches, branch_id: &str) -> PyResult<PyPreviewPlan> {
    let plan = interventions::PreviewPlan::new(body.as_deref(), project_id, run_id, &run_files(run, branches), branch_id).map_err(failed)?;
    Ok(PyPreviewPlan { plan })
}

#[pymethods]
impl PyPreviewPlan {
    #[getter]
    fn minute(&self) -> i64 {
        self.plan.minute()
    }

    /// The branch's record, as its JSON.
    fn record_json<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &records::record_json(self.plan.record()))
    }

    fn check_typed(&self) -> PyResult<()> {
        self.plan.check_typed().map_err(failed)
    }

    /// The answer and the draft to keep (None where the compiler gave no plan), of the branch's
    /// Run at the minute; `preview_id` the draft's id (a uuid4's hex).
    fn answer<'py>(&self, py: Python<'py>, run: &mut crate::rules::plain::PlainRun, preview_id: &str) -> PyResult<(Bound<'py, PyBytes>, Option<Bound<'py, PyBytes>>)> {
        let (answer, draft) = self.plan.answer(run.core_mut(), preview_id).map_err(failed)?;
        Ok((PyBytes::new(py, &answer), draft.map(|draft| PyBytes::new(py, &draft))))
    }

    /// The text read by an LLM (#463): the compiler's steps (`Compiling`), or None where no
    /// operator the core runs is eligible (`answer` then); `stop` whether the router stops instead
    /// of falling back.
    fn compiling(&self, run: &mut crate::rules::plain::PlainRun, stop: bool) -> PyResult<Option<PyCompiling>> {
        Ok(self.plan.compiling(run.core_mut(), stop).map_err(failed)?.map(|compiling| PyCompiling { compiling }))
    }

    /// The answer and the draft of the compiler's result (what `Compiling.read` gave as done).
    fn answer_compiled<'py>(
        &self,
        py: Python<'py>,
        run: &mut crate::rules::plain::PlainRun,
        preview_id: &str,
        compiled: &[u8],
    ) -> PyResult<(Bound<'py, PyBytes>, Option<Bound<'py, PyBytes>>)> {
        let (answer, draft) = self.plan.answer_compiled(run.core_mut(), preview_id, compiled).map_err(failed)?;
        Ok((PyBytes::new(py, &answer), draft.map(|draft| PyBytes::new(py, &draft))))
    }
}

/// An intervention's text read by an LLM (`interpret::Compiling`): `task()` the first task's JSON,
/// then `read(reply)` of each reply (None where the call failed): `("ask", task)` to hand to the
/// link next, `("done", result)` the compiler's result for `PreviewPlan.answer_compiled`; under a
/// stop policy without a plan it raises the refusal (409 `intervention_compiler_unavailable`).
#[pyclass(name = "Compiling", module = "sheltersim_native.rules.answers", unsendable)]
pub struct PyCompiling {
    compiling: sheltersim_core::interpret::Compiling,
}

#[pymethods]
impl PyCompiling {
    fn task<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        Ok(PyBytes::new(py, &self.compiling.task_json().or_py()?))
    }

    #[pyo3(signature = (reply))]
    fn read<'py>(&mut self, py: Python<'py>, reply: Option<Vec<u8>>) -> PyResult<(&'static str, Bound<'py, PyBytes>)> {
        match self.compiling.read(reply.as_deref()).or_py()? {
            Step::Ask(task) => Ok(("ask", PyBytes::new(py, &task))),
            Step::Done(result) => Ok(("done", PyBytes::new(py, &result))),
            Step::Unavailable(reason) => Err(failed(interventions::PreviewPlan::unavailable(&reason))),
        }
    }
}

/// `POST …/intervention-apply`, its first checks: the draft's id, whose draft the caller reads.
#[pyfunction]
#[pyo3(signature = (body))]
fn apply_draft_id(body: Option<Vec<u8>>) -> PyResult<String> {
    interventions::apply_draft_id(body.as_deref()).map_err(failed)
}

/// `POST …/intervention-apply` (`interventions::ApplyPlan`): the draft and the approval checked
/// (`busy` whether the branch computes now); after the model versions, `fork_checks`.
#[pyclass(name = "ApplyPlan", module = "sheltersim_native.rules.answers")]
pub struct PyApplyPlan {
    plan: interventions::ApplyPlan,
}

#[pyfunction]
#[pyo3(signature = (body, draft, project_id, run_id, branch_id, busy))]
fn apply_plan(body: Option<Vec<u8>>, draft: Option<Vec<u8>>, project_id: &str, run_id: &str, branch_id: &str, busy: bool) -> PyResult<PyApplyPlan> {
    let plan = interventions::ApplyPlan::new(body.as_deref(), draft.as_deref(), project_id, run_id, branch_id, busy).map_err(failed)?;
    Ok(PyApplyPlan { plan })
}

#[pymethods]
impl PyApplyPlan {
    /// The branch, the minute and the child, from the Run's records: the branch's record (JSON).
    #[pyo3(signature = (run, branches))]
    fn fork_checks<'py>(&self, py: Python<'py>, run: Option<Vec<u8>>, branches: Branches) -> PyResult<Bound<'py, PyBytes>> {
        bytes(py, self.plan.fork_checks(&run_files(run, branches)).map(|record| records::record_json(&record)))
    }

    #[getter]
    fn child_id(&self) -> Option<String> {
        self.plan.child_id().map(str::to_owned)
    }

    #[getter]
    fn minute(&self) -> i64 {
        self.plan.minute()
    }

    #[getter]
    fn program_text(&self) -> String {
        self.plan.program_text().to_owned()
    }

    #[getter]
    fn hash(&self) -> Option<String> {
        self.plan.hash().map(str::to_owned)
    }

    /// The answer while the child is made.
    fn answer<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.plan.answer())
    }
}

/// The Run's `last_fork` once the child is made.
#[pyfunction]
fn created_outcome<'py>(py: Python<'py>, branch_id: &str, minute: i64, state_hash: &str) -> Bound<'py, PyBytes> {
    PyBytes::new(py, &interventions::created_outcome(branch_id, minute, state_hash))
}

/// The Run's `last_fork` when making the child failed: `error` its text; `refused` whether the
/// store refused it (a 409 with `code`, or the text's leading word).
#[pyfunction]
#[pyo3(signature = (error, refused, code=None))]
fn failed_outcome<'py>(py: Python<'py>, error: &str, refused: bool, code: Option<String>) -> Bound<'py, PyBytes> {
    let refusal = refused.then(|| Refusal { status: 409, error: error.to_owned(), code });
    PyBytes::new(py, &interventions::failed_outcome(error, refusal.as_ref()))
}

/// `POST …/questions` (`questions::QuestionPlan`): the request's checks; then the branch's
/// (`check_branch`), the evidence's (`evidence`, of the Run's Scenario) and the frame's
/// (`check_frame`); then the answer and the turn to keep.
#[pyclass(name = "QuestionPlan", module = "sheltersim_native.rules.answers")]
pub struct PyQuestionPlan {
    plan: questions::QuestionPlan,
}

#[pyfunction]
#[pyo3(signature = (body, project_id, run_id, branch_id))]
fn question_plan(body: Option<Vec<u8>>, project_id: &str, run_id: &str, branch_id: &str) -> PyResult<PyQuestionPlan> {
    Ok(PyQuestionPlan { plan: questions::QuestionPlan::new(body.as_deref(), project_id, run_id, branch_id).map_err(failed)? })
}

#[pymethods]
impl PyQuestionPlan {
    #[pyo3(signature = (run, branches))]
    fn check_branch(&mut self, run: Option<Vec<u8>>, branches: Branches) -> PyResult<()> {
        self.plan.check_branch(&run_files(run, branches)).map_err(failed)
    }

    fn evidence(&mut self, scenario: &[u8]) -> PyResult<()> {
        self.plan.evidence(scenario).map_err(failed)
    }

    #[getter]
    fn minute(&self) -> i64 {
        self.plan.minute()
    }

    /// The frame's checks: the branches whose comparison the caller then checks (or none).
    fn check_frame(&self, frame: &[u8]) -> PyResult<Vec<String>> {
        self.plan.check_frame(frame).map_err(failed)
    }

    /// The answer and the turn to keep: `random` 24 hex digits, `turn_id` the turn's id.
    fn answer<'py>(&self, py: Python<'py>, random: &str, turn_id: &str) -> (Bound<'py, PyBytes>, Bound<'py, PyBytes>) {
        let (answer, turn) = self.plan.answer(random, turn_id);
        (PyBytes::new(py, &answer), PyBytes::new(py, &turn))
    }

    /// The question asked of an LLM (#463, `questions::QuestionPlan::ask`): the branch's frame at
    /// `minute`, the comparison's answer (or None), the Run's `evidence.json` (or None), its
    /// Scenario, the branch's "conversations" pieces (decompressed) and `random` as `answer`'s.
    #[pyo3(signature = (frame, comparison, evidence, scenario, turns, random))]
    fn ask(&self, frame: &[u8], comparison: Option<Vec<u8>>, evidence: Option<Vec<u8>>, scenario: &[u8], turns: Vec<Vec<u8>>, random: &str) -> PyResult<PyAsking> {
        let pieces: Vec<&[u8]> = turns.iter().map(Vec::as_slice).collect();
        Ok(PyAsking { asking: self.plan.ask(frame, comparison.as_deref(), evidence.as_deref(), scenario, &pieces, random).map_err(failed)? })
    }
}

/// A question asked of an LLM: `task()`, the task's JSON for the link (`RouterLink.complete`),
/// then `answer(reply, mode, turn_id)` of the link's reply (None where the call failed).
#[pyclass(name = "Asking", module = "sheltersim_native.rules.answers")]
pub struct PyAsking {
    asking: questions::Asking,
}

#[pymethods]
impl PyAsking {
    fn task<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        bytes(py, self.asking.task_json())
    }

    #[pyo3(signature = (reply, mode, turn_id))]
    fn answer<'py>(&self, py: Python<'py>, reply: Option<Vec<u8>>, mode: &str, turn_id: &str) -> PyResult<(Bound<'py, PyBytes>, Bound<'py, PyBytes>)> {
        let (answer, turn) = self.asking.answer(reply.as_deref(), mode, turn_id).map_err(failed)?;
        Ok((PyBytes::new(py, &answer), PyBytes::new(py, &turn)))
    }
}

/// The review of a completed branch asked of an LLM (#463, `report::review`): `task()`, then
/// `report(reply)` of the link's reply (None where the call failed), the report to keep.
#[pyclass(name = "Reviewing", module = "sheltersim_native.rules.answers")]
pub struct PyReviewing {
    reviewing: report::Reviewing,
}

#[pymethods]
impl PyReviewing {
    fn task<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        bytes(py, self.reviewing.task_json())
    }

    #[pyo3(signature = (reply))]
    fn report<'py>(&self, py: Python<'py>, reply: Option<Vec<u8>>) -> PyResult<Bound<'py, PyBytes>> {
        bytes(py, self.reviewing.report(reply.as_deref()))
    }
}

/// The review of a Run at its last minute asked of an LLM, for the Scenario's JSON bytes and the
/// branch `branch_id`.
#[pyfunction]
fn review(run: &crate::rules::plain::PlainRun, scenario: &[u8], branch_id: &str) -> PyResult<PyReviewing> {
    Ok(PyReviewing { reviewing: report::review(run.core(), scenario, branch_id).map_err(failed)? })
}

/// `POST runs/{run}/pdf-report` with an API key (#463, `pdf::PdfPlan`): the route's checks; then
/// each branch's Run at the minute (`add_branch`), then the report's data, facts and analysis
/// task (`analysis`).
#[pyclass(name = "PdfPlan", module = "sheltersim_native.rules.answers")]
pub struct PyPdfPlan {
    plan: pdf::PdfPlan,
}

#[pyfunction]
#[pyo3(signature = (project_id, run_id, body, run, branches))]
fn pdf_plan(project_id: &str, run_id: &str, body: Option<Vec<u8>>, run: Option<Vec<u8>>, branches: Branches) -> PyResult<PyPdfPlan> {
    Ok(PyPdfPlan { plan: pdf::PdfPlan::new(project_id, run_id, body.as_deref(), &run_files(run, branches)).map_err(failed)? })
}

#[pymethods]
impl PyPdfPlan {
    #[getter]
    fn branch_ids(&self) -> Vec<String> {
        self.plan.branch_ids().to_vec()
    }

    #[getter]
    fn minute(&self) -> i64 {
        self.plan.minute()
    }

    /// The branches' records (the report's heads), as a JSON list in `branch_ids`' order.
    fn records_json<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &records_json(self.plan.records()))
    }

    /// The next branch: its Run at the minute, its frame there, its KPI table and its journal.
    fn add_branch(&mut self, run: &crate::rules::plain::PlainRun, frame: &[u8], table: &PyKpiTable, journal: &[u8]) -> PyResult<()> {
        self.plan.add_branch(run.core(), frame, &table.table, journal).map_err(failed)
    }

    /// The report's data, facts and the analysis's task, of the Run's Scenario; `attempt` the
    /// retry's number (0 first).
    fn analysis(&self, scenario: &[u8], attempt: i64) -> PyResult<PyPdfAnalysis> {
        Ok(PyPdfAnalysis { analysis: self.plan.analysis(scenario, attempt).map_err(failed)? })
    }
}

/// The PDF report's data and facts, and its analysis asked of an LLM: `task()`, then `read(reply)`
/// (None where the call failed), the checked analysis or the refusal.
#[pyclass(name = "PdfAnalysis", module = "sheltersim_native.rules.answers")]
pub struct PyPdfAnalysis {
    analysis: pdf::PdfAnalysis,
}

#[pymethods]
impl PyPdfAnalysis {
    fn data<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.analysis.data_json())
    }

    fn facts<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.analysis.facts_json())
    }

    fn task<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        bytes(py, self.analysis.task_json())
    }

    #[pyo3(signature = (reply))]
    fn read<'py>(&self, py: Python<'py>, reply: Option<Vec<u8>>) -> PyResult<Bound<'py, PyBytes>> {
        bytes(py, self.analysis.read(reply.as_deref()))
    }
}

/// `GET …/branches/{branch}/llm-status` of a Run computed with an LLM: its mode (`llm.json`) and
/// its stored tape's entries (`records("llm_tape")`).
#[pyfunction]
#[pyo3(signature = (query, project_id, run_id, branch_id, branch, mode, tape))]
// Kept (#493): a Python function, whose arguments are the Python call's keyword arguments.
#[allow(clippy::too_many_arguments)]
fn llm_run_status_answer<'py>(
    py: Python<'py>,
    query: &str,
    project_id: &str,
    run_id: &str,
    branch_id: &str,
    branch: Option<Vec<u8>>,
    mode: &str,
    tape: Vec<Vec<u8>>,
) -> PyResult<Bound<'py, PyBytes>> {
    bytes(py, routes::llm_run_status_answer(query, project_id, run_id, branch_id, branch.as_deref(), mode, &tape))
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyPreviewPlan>()?;
    m.add_class::<PyApplyPlan>()?;
    m.add_class::<PyQuestionPlan>()?;
    m.add_class::<PyCompiling>()?;
    m.add_function(wrap_pyfunction!(comparison_range, m)?)?;
    m.add_function(wrap_pyfunction!(preview_plan, m)?)?;
    m.add_function(wrap_pyfunction!(apply_draft_id, m)?)?;
    m.add_function(wrap_pyfunction!(apply_plan, m)?)?;
    m.add_function(wrap_pyfunction!(created_outcome, m)?)?;
    m.add_function(wrap_pyfunction!(failed_outcome, m)?)?;
    m.add_function(wrap_pyfunction!(question_plan, m)?)?;
    m.add_class::<PyComparePlan>()?;
    m.add_class::<PyReplayPlan>()?;
    m.add_function(wrap_pyfunction!(timeline_plan, m)?)?;
    m.add_function(wrap_pyfunction!(compare_plan, m)?)?;
    m.add_function(wrap_pyfunction!(replay_plan, m)?)?;
    m.add_function(wrap_pyfunction!(require_current_model, m)?)?;
    m.add_function(wrap_pyfunction!(view_only, m)?)?;
    m.add_function(wrap_pyfunction!(view_frame, m)?)?;
    m.add_class::<PyKpiTable>()?;
    m.add_class::<PyHistoryPlan>()?;
    m.add_function(wrap_pyfunction!(history_plan, m)?)?;
    m.add_function(wrap_pyfunction!(report_answer, m)?)?;
    m.add_function(wrap_pyfunction!(build_report, m)?)?;
    m.add_class::<PyAsking>()?;
    m.add_class::<PyReviewing>()?;
    m.add_function(wrap_pyfunction!(review, m)?)?;
    m.add_function(wrap_pyfunction!(llm_run_status_answer, m)?)?;
    m.add_class::<PyPdfPlan>()?;
    m.add_class::<PyPdfAnalysis>()?;
    m.add_function(wrap_pyfunction!(pdf_plan, m)?)?;
    m.add("Refused", m.py().get_type::<Refused>())?;
    m.add_function(wrap_pyfunction!(is_artifact_id, m)?)?;
    m.add_function(wrap_pyfunction!(projects_answer, m)?)?;
    m.add_function(wrap_pyfunction!(project_answer, m)?)?;
    m.add_function(wrap_pyfunction!(runs_answer, m)?)?;
    m.add_function(wrap_pyfunction!(run_answer, m)?)?;
    m.add_function(wrap_pyfunction!(evidence_answer, m)?)?;
    m.add_function(wrap_pyfunction!(branch_of, m)?)?;
    m.add_function(wrap_pyfunction!(llm_status_answer, m)?)?;
    m.add_function(wrap_pyfunction!(conversations_answer, m)?)?;
    m.add_function(wrap_pyfunction!(pdf_report_answer, m)?)?;
    m.add_function(wrap_pyfunction!(delete_answer, m)?)?;
    Ok(())
}
