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
use sheltersim_core::answers::{records, routes, Failure};

use crate::core::errors::to_py_err;

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

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
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
