//! The answers of the API's Project routes (`sheltersim_core::answers`, #449) for the browser:
//! js/browser_api.mjs and the modules it calls read the files a route needs from the store, hand
//! them over in the classes here, and give back the answer's JSON text.
//!
//! A refusal is thrown as an Error whose message is `REFUSED` followed by the refusal's JSON
//! (`{"code","error","status"}`), which js/api_answers.mjs's `core` makes the browser's `ApiError`
//! or `ProjectStoreError` again; any other error is the core's text (a stored Run of an older
//! format is a refusal already, 409 `stored_run_outdated`).

use sheltersim_core::answers::{self, frames, records, routes, Failure};
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

    /// A file of text (a frame's JSON as JS holds it).
    #[wasm_bindgen(js_name = addText)]
    pub fn add_text(&mut self, name: &str, text: &str) {
        self.files.push((name.to_owned(), text.as_bytes().to_vec()));
    }
}

impl Files {
    fn raws(&self) -> Vec<&[u8]> {
        self.files.iter().map(|(_, bytes)| bytes.as_slice()).collect()
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
    answer(routes::conversations_answer(&pieces.raws()))
}

/// `POST runs/{run}/pdf-report` with the relay's passphrase (#470, `answers::pdf::PdfPlan`): the
/// route's checks; then each branch's Run at the minute (`addBranch`, after `checkSavedPoint` for a
/// view-only one), then the report's data, facts and analysis task (`analysis`).
#[wasm_bindgen]
pub struct PdfPlan {
    plan: answers::pdf::PdfPlan,
}

#[wasm_bindgen(js_name = pdfPlan)]
pub fn pdf_plan(project_id: &str, run_id: &str, body: Option<String>, files: &RunFiles) -> Result<PdfPlan, JsError> {
    Ok(PdfPlan { plan: answers::pdf::PdfPlan::new(project_id, run_id, body.as_deref().map(str::as_bytes), &files.inner).map_err(failed)? })
}

#[wasm_bindgen]
impl PdfPlan {
    pub fn minute(&self) -> f64 {
        self.plan.minute() as f64
    }

    /// The branches' records (the report's heads), as a JSON list in the request's order.
    #[wasm_bindgen(js_name = recordsJson)]
    pub fn records_json(&self) -> Result<String, JsError> {
        Ok(records_json(self.plan.records()))
    }

    /// A view-only branch's last saved point at or before the minute (0 where it has none): the
    /// refusal where it is not the minute.
    #[wasm_bindgen(js_name = checkSavedPoint)]
    pub fn check_saved_point(&self, saved: f64) -> Result<(), JsError> {
        self.plan.check_saved_point(saved as i64).map_err(failed)
    }

    /// The next branch: its Run at the minute, its frame there (JSON text), its stored samples and
    /// its journal.
    #[wasm_bindgen(js_name = addBranch)]
    pub fn add_branch(&mut self, simulation: &Simulation, frame: &str, table: &KpiHistory, journal: &[u8]) -> Result<(), JsError> {
        self.plan.add_branch(&simulation.run, frame.as_bytes(), &table.table, journal).map_err(failed)
    }

    /// The report's data, facts and the analysis's task, of the Run's Scenario (a first try: the
    /// browser keeps no report, so a retry is the same request).
    pub fn analysis(&self, scenario: &[u8]) -> Result<PdfAnalysis, JsError> {
        Ok(PdfAnalysis { analysis: self.plan.analysis(scenario, 0).map_err(failed)? })
    }
}

/// The report's analysis asked of an LLM: `task()`, the task's JSON for the relay, then
/// `read(reply)` of its reply (`llm::Reply`'s JSON; undefined where the call failed), the checked
/// analysis or the refusal, then `report(analysis)`, the route's answer.
#[wasm_bindgen]
pub struct PdfAnalysis {
    analysis: answers::pdf::PdfAnalysis,
}

#[wasm_bindgen]
impl PdfAnalysis {
    pub fn task(&self) -> Result<String, JsError> {
        answer(self.analysis.task_json())
    }

    pub fn read(&self, reply: Option<String>) -> Result<String, JsError> {
        answer(self.analysis.read(reply.as_deref().map(str::as_bytes)))
    }

    pub fn report(&self, analysis: &str) -> Result<String, JsError> {
        answer(self.analysis.report(analysis.as_bytes()))
    }
}

/// `POST runs/{run}/pdf-report` without the relay's passphrase: always a refusal
/// (`routes::pdf_report_answer`).
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

fn records_json(records: &[records::Record]) -> String {
    let items: Vec<Vec<u8>> = records.iter().map(records::record_json).collect();
    String::from_utf8_lossy(&answers::list(items)).into_owned()
}

/// `GET …/branches/{branch}/timeline?minute=`, its checks (`minute` the query's value): JSON,
/// `{"minute", "record"}`: the minute whose frame is the answer and the branch's record.
#[wasm_bindgen(js_name = timelinePlan)]
pub fn timeline_plan(minute: &str, project_id: &str, run_id: &str, files: &RunFiles, branch_id: &str) -> Result<String, JsError> {
    let (record, minute) = frames::timeline_plan(minute, project_id, run_id, &files.inner, branch_id).map_err(failed)?;
    Ok(format!("{{\"minute\":{minute},\"record\":{}}}", String::from_utf8_lossy(&records::record_json(&record))))
}

/// `GET runs/{run}/compare?branch_ids=&minute=` (`frames::ComparePlan`): its checks, then the
/// branches' journals (`receipts`), then their frames (`answer`).
#[wasm_bindgen]
pub struct ComparePlan {
    plan: frames::ComparePlan,
    comparison: Option<frames::Comparison>,
}

#[wasm_bindgen(js_name = comparePlan)]
pub fn compare_plan(query: &str, project_id: &str, run_id: &str, files: &RunFiles) -> Result<ComparePlan, JsError> {
    Ok(ComparePlan { plan: frames::ComparePlan::new(query, project_id, run_id, &files.inner).map_err(failed)?, comparison: None })
}

#[wasm_bindgen]
impl ComparePlan {
    #[wasm_bindgen(js_name = branchIds)]
    pub fn branch_ids(&self) -> Vec<String> {
        self.plan.branch_ids.clone()
    }

    pub fn minute(&self) -> f64 {
        self.plan.minute as f64
    }

    /// The branches' records, as a JSON list in the order of `branchIds`.
    #[wasm_bindgen(js_name = recordsJson)]
    pub fn records_json(&self) -> String {
        records_json(&self.plan.records)
    }

    /// Reads the branches' fork receipts: `journals` each branch's `journal.jsonl` (empty where it
    /// has none), in the order of `branchIds`.
    pub fn receipts(&mut self, journals: &Files) -> Result<(), JsError> {
        let comparison = frames::Comparison::new(self.plan.branch_ids.clone(), self.plan.common_range, &journals.raws()).map_err(failed)?;
        self.comparison = Some(comparison);
        Ok(())
    }

    /// The answer of the branches' frames (JSON texts, in the order of `branchIds`), after
    /// `receipts`.
    pub fn answer(&self, run_id: &str, frames: &Files) -> Result<String, JsError> {
        let comparison = self.comparison.as_ref().ok_or_else(|| JsError::new("the receipts are not read"))?;
        answer(comparison.answer(run_id, self.plan.minute, &frames.raws()))
    }
}

/// `GET runs/{run}/replay-frames?branch_ids=&start=&end=` (`frames::ReplayPlan`): its checks;
/// for two branches, their journals (`receipts`) and each minute's comparison (`compare`); then
/// the answer of the samples.
#[wasm_bindgen]
pub struct ReplayPlan {
    plan: frames::ReplayPlan,
    comparison: Option<frames::Comparison>,
}

#[wasm_bindgen(js_name = replayPlan)]
pub fn replay_plan(query: &str, project_id: &str, run_id: &str, files: &RunFiles) -> Result<ReplayPlan, JsError> {
    Ok(ReplayPlan { plan: frames::ReplayPlan::new(query, project_id, run_id, &files.inner).map_err(failed)?, comparison: None })
}

#[wasm_bindgen]
impl ReplayPlan {
    #[wasm_bindgen(js_name = branchIds)]
    pub fn branch_ids(&self) -> Vec<String> {
        self.plan.branch_ids.clone()
    }

    pub fn start(&self) -> f64 {
        self.plan.start as f64
    }

    pub fn end(&self) -> f64 {
        self.plan.end as f64
    }

    /// The branches' records, as a JSON list in the order of `branchIds`.
    #[wasm_bindgen(js_name = recordsJson)]
    pub fn records_json(&self) -> String {
        records_json(&self.plan.records)
    }

    /// For two branches: the comparison's checks (the Run's records again) and the branches' fork
    /// receipts (`journals`, as `ComparePlan.receipts` takes them).
    pub fn receipts(&mut self, project_id: &str, run_id: &str, files: &RunFiles, journals: &Files) -> Result<(), JsError> {
        let range = self.plan.comparison_range(project_id, run_id, &files.inner).map_err(failed)?;
        self.comparison = Some(frames::Comparison::new(self.plan.branch_ids.clone(), range, &journals.raws()).map_err(failed)?);
        Ok(())
    }

    /// For two branches: the comparison of their frames at `minute`, after `receipts`.
    pub fn compare(&self, run_id: &str, minute: f64, frames: &Files) -> Result<String, JsError> {
        let comparison = self.comparison.as_ref().ok_or_else(|| JsError::new("the receipts are not read"))?;
        answer(comparison.answer(run_id, minute as i64, &frames.raws()))
    }

    /// The answer of the samples (a branch's frames, or the comparisons), one for each minute.
    pub fn answer(&self, samples: &Files) -> Result<String, JsError> {
        text(self.plan.answer(&samples.raws()))
    }
}

/// How the model versions of a branch's stored Run compare with the core's (`frames::model_view`):
/// JSON `{"recorded", "installed", "status"}` (the status as `modelStatus` writes it), or
/// undefined for a branch without a stored Run or whose journal cannot be read. `journal` is the
/// branch's `journal.jsonl`, `scenario` the Run's Scenario.
#[wasm_bindgen(js_name = branchModelView)]
pub fn branch_model_view(journal: &[u8], scenario: &[u8]) -> Result<Option<String>, JsError> {
    let view = frames::model_view(journal, scenario).map_err(failed)?;
    Ok(view.map(|(recorded, installed, status)| {
        format!(
            "{{\"installed\":{},\"recorded\":{},\"status\":{}}}",
            crate::versions_json(&installed),
            crate::versions_json(&recorded),
            crate::status_json(&status)
        )
    }))
}

/// The checks of comparing branches at a minute (`frames::comparison_range`): `branch_ids` the ids, `minute` an integer (undefined for any other value). The common range.
#[wasm_bindgen(js_name = comparisonRange)]
pub fn comparison_range(branch_ids: Vec<String>, minute: Option<f64>, project_id: &str, run_id: &str, files: &RunFiles) -> Result<Vec<f64>, JsError> {
    let (lower, upper) = frames::comparison_range(&branch_ids, minute, project_id, run_id, &files.inner).map_err(failed)?;
    Ok(vec![lower as f64, upper as f64])
}

/// Reads the fork receipts of branches' journals (`frames::fork_receipt`), as a comparison does:
/// an error for a journal it cannot read.
#[wasm_bindgen(js_name = forkReceipts)]
pub fn fork_receipts(journals: &Files) -> Result<(), JsError> {
    for journal in journals.raws() {
        frames::fork_receipt(journal).map_err(failed)?;
    }
    Ok(())
}

/// `POST …/intervention-preview` (`answers::interventions::PreviewPlan`): its checks; then, after
/// the model versions are checked (`checkTyped`), the answer of the branch's Run at `minute`.
#[wasm_bindgen]
pub struct PreviewPlan {
    plan: answers::interventions::PreviewPlan,
    draft: Option<Vec<u8>>,
}

#[wasm_bindgen(js_name = previewPlan)]
pub fn preview_plan(body: Option<String>, project_id: &str, run_id: &str, files: &RunFiles, branch_id: &str) -> Result<PreviewPlan, JsError> {
    let plan = answers::interventions::PreviewPlan::new(body.as_deref().map(str::as_bytes), project_id, run_id, &files.inner, branch_id).map_err(failed)?;
    Ok(PreviewPlan { plan, draft: None })
}

#[wasm_bindgen]
impl PreviewPlan {
    pub fn minute(&self) -> f64 {
        self.plan.minute() as f64
    }

    /// The branch's record, as its JSON.
    #[wasm_bindgen(js_name = recordJson)]
    pub fn record_json(&self) -> Result<String, JsError> {
        text(records::record_json(self.plan.record()))
    }

    /// A typed program is refused (after the model versions are checked).
    #[wasm_bindgen(js_name = checkTyped)]
    pub fn check_typed(&self) -> Result<(), JsError> {
        self.plan.check_typed().map_err(failed)
    }

    /// The answer, of the branch's Run at the minute; `previewId` the draft's id. The draft to keep
    /// is then `draft()`.
    pub fn answer(&mut self, simulation: &mut Simulation, preview_id: &str) -> Result<String, JsError> {
        let (answer, draft) = self.plan.answer(&mut simulation.run, preview_id).map_err(failed)?;
        self.draft = draft;
        text(answer)
    }

    /// The draft to keep (`<project>/drafts/<previewId>.json`), or undefined where the compiler
    /// gave no plan.
    pub fn draft(&self) -> Option<Vec<u8>> {
        self.draft.clone()
    }

    /// The text read by an LLM through the site's relay (#470, `interpret::Compiling`), of the
    /// branch's Run at the minute: the compiler's steps, or undefined where no operator the core
    /// runs is eligible (`answer` then). The browser has no stop policy: without an accepted plan
    /// the rules read the text.
    pub fn compiling(&self, simulation: &mut Simulation) -> Result<Option<Compiling>, JsError> {
        Ok(self.plan.compiling(&mut simulation.run, false).map_err(failed)?.map(|compiling| Compiling { compiling, done: None }))
    }

    /// The answer of the compiler's result (once `Compiling.read` said it is done), of the
    /// branch's Run at the minute; the draft to keep is then `draft()`.
    #[wasm_bindgen(js_name = answerCompiled)]
    pub fn answer_compiled(&mut self, simulation: &mut Simulation, preview_id: &str, compiling: &Compiling) -> Result<String, JsError> {
        let compiled = compiling.done.as_deref().ok_or_else(|| JsError::new("the compiler has not given its result"))?;
        let (answer, draft) = self.plan.answer_compiled(&mut simulation.run, preview_id, compiled).map_err(failed)?;
        self.draft = draft;
        text(answer)
    }
}

/// An intervention's text read by an LLM (`interpret::Compiling`): `task()` the first task's JSON
/// for the relay, then `read(reply)` of each reply (`llm::Reply`'s JSON; undefined where the call
/// failed): the next task's JSON to ask, or undefined once the result is made
/// (`PreviewPlan.answerCompiled`).
#[wasm_bindgen]
pub struct Compiling {
    compiling: sheltersim_core::interpret::Compiling,
    done: Option<Vec<u8>>,
}

#[wasm_bindgen]
impl Compiling {
    pub fn task(&self) -> Result<String, JsError> {
        text(self.compiling.task_json().map_err(|error| JsError::new(&error.to_string()))?)
    }

    pub fn read(&mut self, reply: Option<String>) -> Result<Option<String>, JsError> {
        use sheltersim_core::interpret::Step;
        match self.compiling.read(reply.as_deref().map(str::as_bytes)).map_err(|error| JsError::new(&error.to_string()))? {
            Step::Ask(task) => Ok(Some(text(task)?)),
            Step::Done(result) => {
                self.done = Some(result);
                Ok(None)
            }
            Step::Unavailable(reason) => Err(failed(answers::interventions::PreviewPlan::unavailable(&reason))),
        }
    }
}

/// `POST …/intervention-apply`, its first checks: the draft's id, whose draft the caller reads.
#[wasm_bindgen(js_name = applyDraftId)]
pub fn apply_draft_id(body: Option<String>) -> Result<String, JsError> {
    answers::interventions::apply_draft_id(body.as_deref().map(str::as_bytes)).map_err(failed)
}

/// `POST …/intervention-apply` (`answers::interventions::ApplyPlan`): the draft and the approval
/// checked (`busy` whether the branch computes now); after the model versions, `forkChecks`.
#[wasm_bindgen]
pub struct ApplyPlan {
    plan: answers::interventions::ApplyPlan,
}

#[wasm_bindgen(js_name = applyPlan)]
pub fn apply_plan(body: Option<String>, draft: Option<Vec<u8>>, project_id: &str, run_id: &str, branch_id: &str, busy: bool) -> Result<ApplyPlan, JsError> {
    let plan =
        answers::interventions::ApplyPlan::new(body.as_deref().map(str::as_bytes), draft.as_deref(), project_id, run_id, branch_id, busy).map_err(failed)?;
    Ok(ApplyPlan { plan })
}

#[wasm_bindgen]
impl ApplyPlan {
    /// The branch, the minute and the child, from the Run's records: the branch's record (JSON).
    #[wasm_bindgen(js_name = forkChecks)]
    pub fn fork_checks(&self, files: &RunFiles) -> Result<String, JsError> {
        answer(self.plan.fork_checks(&files.inner).map(|record| records::record_json(&record)))
    }

    #[wasm_bindgen(js_name = childId)]
    pub fn child_id(&self) -> Option<String> {
        self.plan.child_id().map(str::to_owned)
    }

    pub fn minute(&self) -> f64 {
        self.plan.minute() as f64
    }

    #[wasm_bindgen(js_name = programText)]
    pub fn program_text(&self) -> String {
        self.plan.program_text().to_owned()
    }

    pub fn hash(&self) -> Option<String> {
        self.plan.hash().map(str::to_owned)
    }

    /// The parent's state the preview saw (the draft's), which the fork checks
    /// (`Procedure.fork`).
    #[wasm_bindgen(js_name = stateHash)]
    pub fn state_hash(&self) -> Option<String> {
        self.plan.state_hash().map(str::to_owned)
    }

    /// The answer while the child is made.
    pub fn answer(&self) -> Result<String, JsError> {
        text(self.plan.answer())
    }
}

/// The Run's `last_fork` once the child is made.
#[wasm_bindgen(js_name = createdOutcome)]
pub fn created_outcome(branch_id: &str, minute: f64, state_hash: &str) -> Result<String, JsError> {
    text(answers::interventions::created_outcome(branch_id, minute as i64, state_hash))
}

/// The Run's `last_fork` when making the child failed: `error` the error's text, `refusal` the
/// refusal's JSON (`{"code","error","status"}`) where the store refused, or undefined.
#[wasm_bindgen(js_name = failedOutcome)]
pub fn failed_outcome(error: &str, refusal_status: Option<u16>, refusal_code: Option<String>) -> Result<String, JsError> {
    let refusal = refusal_status.map(|status| answers::Refusal { status, error: error.to_owned(), code: refusal_code });
    text(answers::interventions::failed_outcome(error, refusal.as_ref()))
}

/// `POST …/questions` (`answers::questions::QuestionPlan`): the request's and the evidence's
/// checks; then the frame's (`checkFrame`); then the answer and the turn.
#[wasm_bindgen]
pub struct QuestionPlan {
    plan: answers::questions::QuestionPlan,
    turn: Vec<u8>,
}

#[wasm_bindgen(js_name = questionPlan)]
pub fn question_plan(body: Option<String>, project_id: &str, run_id: &str, branch_id: &str) -> Result<QuestionPlan, JsError> {
    let plan = answers::questions::QuestionPlan::new(body.as_deref().map(str::as_bytes), project_id, run_id, branch_id).map_err(failed)?;
    Ok(QuestionPlan { plan, turn: Vec::new() })
}

#[wasm_bindgen]
impl QuestionPlan {
    /// The branch's record, from the Run's records.
    #[wasm_bindgen(js_name = checkBranch)]
    pub fn check_branch(&mut self, files: &RunFiles) -> Result<(), JsError> {
        self.plan.check_branch(&files.inner).map_err(failed)
    }

    /// The evidence's checks, of the Run's Scenario (its JSON).
    pub fn evidence(&mut self, scenario: &[u8]) -> Result<(), JsError> {
        self.plan.evidence(scenario).map_err(failed)
    }

    pub fn minute(&self) -> f64 {
        self.plan.minute() as f64
    }

    /// The frame's checks: the branches whose comparison the caller then checks (or none).
    #[wasm_bindgen(js_name = checkFrame)]
    pub fn check_frame(&self, frame: &str) -> Result<Vec<String>, JsError> {
        self.plan.check_frame(frame.as_bytes()).map_err(failed)
    }

    /// The answer; the turn to keep is then `turn()`.
    pub fn answer(&mut self, random: &str, turn_id: &str) -> Result<String, JsError> {
        let (answer, turn) = self.plan.answer(random, turn_id);
        self.turn = turn;
        text(answer)
    }

    pub fn turn(&self) -> Vec<u8> {
        self.turn.clone()
    }

    /// The question asked of an LLM through the site's relay (#470, `QuestionPlan::ask`): the
    /// branch's frame at the minute, the comparison's answer (or undefined), the Run's
    /// `evidence.json` (or undefined), its Scenario, the branch's "conversations" pieces
    /// (decompressed) and `random` as `answer`'s.
    pub fn ask(
        &self,
        frame: &str,
        comparison: Option<String>,
        evidence: Option<Vec<u8>>,
        scenario: &[u8],
        turns: &Files,
        random: &str,
    ) -> Result<Asking, JsError> {
        let asking =
            self.plan.ask(frame.as_bytes(), comparison.as_deref().map(str::as_bytes), evidence.as_deref(), scenario, &turns.raws(), random).map_err(failed)?;
        Ok(Asking { asking, turn: Vec::new() })
    }
}

/// A question asked of an LLM: `task()`, the task's JSON for the relay, then `answer(reply, mode,
/// turnId)` of the relay's reply (`llm::Reply`'s JSON; undefined where the call failed); the turn
/// to keep is then `turn()`.
#[wasm_bindgen]
pub struct Asking {
    asking: answers::questions::Asking,
    turn: Vec<u8>,
}

#[wasm_bindgen]
impl Asking {
    pub fn task(&self) -> Result<String, JsError> {
        answer(self.asking.task_json())
    }

    pub fn answer(&mut self, reply: Option<String>, mode: &str, turn_id: &str) -> Result<String, JsError> {
        let (answer, turn) = self.asking.answer(reply.as_deref().map(str::as_bytes), mode, turn_id).map_err(failed)?;
        self.turn = turn;
        text(answer)
    }

    pub fn turn(&self) -> Vec<u8> {
        self.turn.clone()
    }
}
