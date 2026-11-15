//! The answers of the API's Project routes (`sheltersim_core::answers`, #449) for the browser:
//! js/browser_api.mjs and the modules it calls read the files a route needs from the store, hand
//! them over in the classes here, and give back the answer's JSON text.
//!
//! A refusal is thrown as an Error whose message is `REFUSED` followed by the refusal's JSON
//! (`{"code","error","status"}`), which js/api_answers.mjs's `core` makes the browser's `ApiError`
//! or `ProjectStoreError` again; any other error is the core's text (a stored Run of an older
//! format is a refusal already, 409 `stored_run_outdated`).

use sheltersim_core::answers::{self, records, routes, Failure};
use wasm_bindgen::prelude::*;

use crate::{KpiHistory, Simulation};

/// What the message of a refusal starts with.
pub const REFUSED: &str = "answer_refused ";

pub(crate) fn failed(failure: Failure) -> JsError {
    match failure {
        Failure::Refused(refusal) => JsError::new(&format!("{REFUSED}{}", String::from_utf8_lossy(&refusal.to_json()))),
        Failure::Error(error) => JsError::new(&error.to_string()),
    }
}

pub(crate) fn text(bytes: Vec<u8>) -> Result<String, JsError> {
    String::from_utf8(bytes).map_err(|error| JsError::new(&error.to_string()))
}

fn answer(result: answers::Answer) -> Result<String, JsError> {
    text(result.map_err(failed)?)
}

/// Files by name, as a store listed and read them: the `project.json` of each Project's folder,
/// or the pieces of a stored Run.
#[wasm_bindgen]
#[derive(Default)]
pub struct Files {
    files: Vec<(String, Vec<u8>)>,
}

#[wasm_bindgen]
impl Files {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Files {
        Files::default()
    }

    pub fn add(&mut self, name: &str, bytes: Vec<u8>) {
        self.files.push((name.to_owned(), bytes));
    }
}

/// A Run's records (`records::RunFiles`): its `run.json`, and the `branch.json` of each folder of
/// its `branches/` that holds one.
#[wasm_bindgen]
pub struct RunFiles {
    inner: records::RunFiles,
}

#[wasm_bindgen]
impl RunFiles {
    /// `run` is the bytes of `run.json`, or undefined where it is missing.
    #[wasm_bindgen(constructor)]
    pub fn new(run: Option<Vec<u8>>) -> RunFiles {
        RunFiles { inner: records::RunFiles { run, branches: Vec::new() } }
    }

    #[wasm_bindgen(js_name = addBranch)]
    pub fn add_branch(&mut self, name: &str, bytes: Vec<u8>) {
        self.inner.branches.push((name.to_owned(), bytes));
    }
}

/// A Project's files: its `project.json`, the files of its Scenarios' folders and its Runs'
/// records.
#[wasm_bindgen]
pub struct ProjectFiles {
    project: Option<Vec<u8>>,
    scenarios: Vec<records::ScenarioFolder>,
    runs: Vec<(String, records::RunFiles)>,
}

#[wasm_bindgen]
impl ProjectFiles {
    #[wasm_bindgen(constructor)]
    pub fn new(project: Option<Vec<u8>>) -> ProjectFiles {
        ProjectFiles { project, scenarios: Vec::new(), runs: Vec::new() }
    }

    /// A file of the folder `scenarios/<scenario_id>/`.
    #[wasm_bindgen(js_name = addScenarioFile)]
    pub fn add_scenario_file(&mut self, scenario_id: &str, name: &str, bytes: Vec<u8>) {
        let file = (name.to_owned(), bytes);
        match self.scenarios.iter_mut().find(|folder| folder.scenario_id == scenario_id) {
            Some(folder) => folder.files.push(file),
            None => self.scenarios.push(records::ScenarioFolder { scenario_id: scenario_id.to_owned(), files: vec![file] }),
        }
    }

    /// The records of the folder `runs/<name>/`.
    #[wasm_bindgen(js_name = addRun)]
    pub fn add_run(&mut self, name: &str, run: &RunFiles) {
        self.runs.push((name.to_owned(), run.inner.clone()));
    }
}

/// Whether `text` is an identifier the store takes for a folder (`records::is_id`).
#[wasm_bindgen(js_name = isArtifactId)]
pub fn is_artifact_id(text: &str) -> bool {
    records::is_id(text)
}

/// `GET /api/v1/projects`: `projects` holds the `project.json` of each folder of the store, by
/// folder name; `bytes` and `count` the store's usage, `limit` what it may hold.
#[wasm_bindgen(js_name = projectsAnswer)]
pub fn projects_answer(projects: &Files, bytes: f64, count: f64, limit: Option<f64>) -> Result<String, JsError> {
    answer(records::projects_answer(&projects.files, bytes as u64, count as u64, limit))
}

/// `GET /api/v1/projects/{project}`.
#[wasm_bindgen(js_name = projectAnswer)]
pub fn project_answer(project_id: &str, files: &ProjectFiles) -> Result<String, JsError> {
    answer(records::project_answer(project_id, files.project.as_deref(), &files.scenarios, &files.runs))
}

/// `GET /api/v1/projects/{project}/runs` (the Scenarios of `files` are not read).
#[wasm_bindgen(js_name = runsAnswer)]
pub fn runs_answer(project_id: &str, files: &ProjectFiles) -> Result<String, JsError> {
    answer(records::runs_answer(project_id, files.project.as_deref(), &files.runs))
}

/// `GET /api/v1/projects/{project}/runs/{run}`, with the last fork's outcome (JSON) where one is
/// kept.
#[wasm_bindgen(js_name = runAnswer)]
pub fn run_answer(project_id: &str, run_id: &str, files: &RunFiles, last_fork: Option<String>) -> Result<String, JsError> {
    answer(records::run_answer(project_id, run_id, &files.inner, last_fork.as_deref().map(str::as_bytes)))
}

/// `GET /api/v1/projects/{project}/runs/{run}/evidence`.
#[wasm_bindgen(js_name = evidenceAnswer)]
pub fn evidence_answer(project_id: &str, run_id: &str, run: Option<Vec<u8>>, evidence: Option<Vec<u8>>) -> Result<String, JsError> {
    answer(records::evidence_answer(project_id, run_id, run.as_deref(), evidence.as_deref()))
}

/// The record of a branch with computed minutes, as its JSON (`records::branch_of`).
#[wasm_bindgen(js_name = branchOf)]
pub fn branch_of(project_id: &str, run_id: &str, files: &RunFiles, branch_id: &str) -> Result<String, JsError> {
    answer(records::branch_of(project_id, run_id, &files.inner, branch_id).map(|record| records::record_json(&record)))
}

/// The record of the branch folder `branch_id` (`RunFiles::branch_record`: its id checked, then
/// its record read), as its JSON.
#[wasm_bindgen(js_name = branchRecord)]
pub fn branch_record(project_id: &str, run_id: &str, files: &RunFiles, branch_id: &str) -> Result<String, JsError> {
    answer(files.inner.branch_record(project_id, run_id, branch_id).map(|record| records::record_json(&record)))
}

/// `GET …/branches/{branch}/llm-status`: `query` the text after "?", `branch` the bytes of the
/// branch's `branch.json`.
#[wasm_bindgen(js_name = llmStatusAnswer)]
pub fn llm_status_answer(query: &str, project_id: &str, run_id: &str, branch_id: &str, branch: Option<Vec<u8>>) -> Result<String, JsError> {
    answer(routes::llm_status_answer(query, project_id, run_id, branch_id, branch.as_deref()))
}

/// `GET …/branches/{branch}/conversations`, after `branchOf`: `pieces` the branch's
/// "conversations" pieces, decompressed, in the order of its journal.
#[wasm_bindgen(js_name = conversationsAnswer)]
pub fn conversations_answer(pieces: &Files) -> Result<String, JsError> {
    let raws: Vec<&[u8]> = pieces.files.iter().map(|(_, bytes)| bytes.as_slice()).collect();
    answer(routes::conversations_answer(&raws))
}

/// `POST runs/{run}/pdf-report`: always a refusal (`routes::pdf_report_answer`).
#[wasm_bindgen(js_name = pdfReport)]
pub fn pdf_report(project_id: &str, run_id: &str, body: Option<String>, files: &RunFiles) -> Result<(), JsError> {
    Err(failed(routes::pdf_report_answer(project_id, run_id, body.as_deref().map(str::as_bytes), &files.inner)))
}

/// `POST runs/{run}/delete`: the answer once the caller removes the Run's folder; `active` whether
/// a computation of the Run is running.
#[wasm_bindgen(js_name = deleteAnswer)]
pub fn delete_answer(project_id: &str, run_id: &str, body: Option<String>, active: bool, files: &RunFiles) -> Result<String, JsError> {
    answer(routes::delete_answer(project_id, run_id, body.as_deref().map(str::as_bytes), active, &files.inner))
}

/// A history request the server takes (`answers::history::HistoryPlan`): the samples to read, then
/// the page.
#[wasm_bindgen]
pub struct HistoryPlan {
    plan: answers::history::HistoryPlan,
}

/// `GET …/branches/{branch}/history`, its checks: `query` the text after "?", `branch` the bytes
/// of the branch's `branch.json`.
#[wasm_bindgen(js_name = historyPlan)]
pub fn history_plan(query: &str, project_id: &str, run_id: &str, branch_id: &str, branch: Option<Vec<u8>>) -> Result<HistoryPlan, JsError> {
    let plan = answers::history::HistoryPlan::new(query, project_id, run_id, branch_id, branch.as_deref()).map_err(failed)?;
    Ok(HistoryPlan { plan })
}

#[wasm_bindgen]
impl HistoryPlan {
    /// The branch's head, or undefined for a branch without one (whose answer reads no samples).
    pub fn anchor(&self) -> Option<String> {
        self.plan.anchor().map(str::to_owned)
    }

    /// The answer, from the branch's samples read back (an empty table without a head).
    pub fn answer(&self, table: &KpiHistory) -> Result<String, JsError> {
        answer(self.plan.answer(&table.table))
    }
}

/// `GET …/branches/{branch}/report`, from what is kept: the kept report as it is,
/// `{"status":"not_generated"}`, or undefined: make it (`Simulation.report`).
#[wasm_bindgen(js_name = reportAnswer)]
pub fn report_answer(
    query: &str,
    project_id: &str,
    run_id: &str,
    branch_id: &str,
    branch: Option<Vec<u8>>,
    kept: Option<Vec<u8>>,
) -> Result<Option<String>, JsError> {
    answers::report::report_answer(query, project_id, run_id, branch_id, branch.as_deref(), kept.as_deref()).map_err(failed)?.map(text).transpose()
}

#[wasm_bindgen]
impl Simulation {
    /// The report of the Run at its last minute (`answers::report::build_report`), for the
    /// Scenario's JSON bytes and the branch `branch_id`: its JSON text.
    pub fn report(&self, scenario: &[u8], branch_id: &str) -> Result<String, JsError> {
        answer(answers::report::build_report(&self.run, scenario, branch_id))
    }
}
