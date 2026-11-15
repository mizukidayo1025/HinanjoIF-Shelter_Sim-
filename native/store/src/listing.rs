//! The files the core's answers read (`sheltersim_core::answers`, #449), gathered as the
//! browser's `project_store.mjs` gathers them for WASM (`readNamed`, `runFiles`, `projectFiles`):
//! a file's bytes, or None where it is missing or an id that names it is not an identifier (the
//! core's answer then refuses it), and the folders of a listing that are identifiers, by name.
//! The local and VM versions hand them to the same answers through PyO3 (#427).

use std::fs;

use crate::files::read_if_there;
use crate::layout::{is_id, listed};
use crate::records::ProjectFiles;
use crate::Result;

/// A file of a listing: its folder's (or its own) name and its bytes.
pub type Named = (String, Vec<u8>);
/// A Run's records: its run.json and the branch.json of each of its branches' folders.
pub type RunListing = (Option<Vec<u8>>, Vec<Named>);
/// A Run of a Project's listing: its folder's name and its records.
pub type ListedRun = (String, Option<Vec<u8>>, Vec<Named>);
/// A Project's records: its project.json, its Scenarios' folders (each `(scenario id, its files)`)
/// and its Runs.
pub type ProjectListing = (Option<Vec<u8>>, Vec<(String, Vec<Named>)>, Vec<ListedRun>);

impl ProjectFiles {
    /// The project.json of each folder of the store that holds one (`GET /api/v1/projects`).
    pub fn projects_listing(&self) -> Result<Vec<Named>> {
        let mut out = Vec::new();
        for name in listed(self.layout().root(), true)? {
            if !is_id(&name) {
                continue;
            }
            if let Some(bytes) = read_if_there(&self.layout().project_file(&name, "project.json")?)? {
                out.push((name, bytes));
            }
        }
        Ok(out)
    }

    /// A Run's records.
    pub fn run_listing(&self, project: &str, run: &str) -> Result<RunListing> {
        if !(is_id(project) && is_id(run)) {
            return Ok((None, Vec::new()));
        }
        let record = read_if_there(&self.layout().run_file(project, run, "run.json")?)?;
        let mut branches = Vec::new();
        for name in listed(&self.layout().branches(project, run)?, true)? {
            if !is_id(&name) {
                continue;
            }
            if let Some(bytes) = read_if_there(&self.layout().branch_file(project, run, &name, "branch.json")?)? {
                branches.push((name, bytes));
            }
        }
        Ok((record, branches))
    }

    /// A Project's records; its Scenarios' files where `scenarios`.
    pub fn project_listing(&self, project: &str, scenarios: bool) -> Result<ProjectListing> {
        if !is_id(project) {
            return Ok((None, Vec::new(), Vec::new()));
        }
        let record = read_if_there(&self.layout().project_file(project, "project.json")?)?;
        let mut listed_scenarios = Vec::new();
        if scenarios {
            for scenario in listed(&self.layout().scenarios(project)?, true)? {
                if !is_id(&scenario) {
                    continue;
                }
                let folder = self.layout().scenario(project, &scenario)?;
                let mut files = Vec::new();
                for name in listed(&folder, false)? {
                    files.push((name.clone(), fs::read(folder.join(&name))?));
                }
                listed_scenarios.push((scenario, files));
            }
        }
        let mut runs = Vec::new();
        for run in listed(&self.layout().runs(project)?, true)? {
            if !is_id(&run) {
                continue;
            }
            let (record, branches) = self.run_listing(project, &run)?;
            runs.push((run, record, branches));
        }
        Ok((record, listed_scenarios, runs))
    }

    /// The file `name` of a Run's folder (run.json, evidence.json), or None.
    pub fn run_file(&self, project: &str, run: &str, name: &str) -> Result<Option<Vec<u8>>> {
        if !(is_id(project) && is_id(run)) {
            return Ok(None);
        }
        Ok(read_if_there(&self.layout().run_file(project, run, name)?)?)
    }

    /// The file `name` of a branch's folder (branch.json, journal.jsonl, report.json), or None. A
    /// journal is read up to its last whole line: a writer may be appending to it.
    pub fn branch_file(&self, project: &str, run: &str, branch: &str, name: &str) -> Result<Option<Vec<u8>>> {
        if !(is_id(project) && is_id(run) && is_id(branch)) {
            return Ok(None);
        }
        let path = self.layout().branch_file(project, run, branch, name)?;
        let Some(mut bytes) = read_if_there(&path)? else { return Ok(None) };
        if name == crate::stored::JOURNAL {
            bytes.truncate(bytes.iter().rposition(|&byte| byte == b'\n').map_or(0, |at| at + 1));
        }
        Ok(Some(bytes))
    }

    /// Keeps `bytes` as a branch's report.json (made when it is first asked for). The caller
    /// holds the store's transaction.
    pub fn keep_report(&self, project: &str, run: &str, branch: &str, bytes: &[u8]) -> Result<()> {
        self.branch_record(project, run, branch)?;
        let path = self.layout().branch_file(project, run, branch, "report.json")?;
        self.write_file(&path, bytes)
    }

    /// The draft `id` of an intervention's preview (`<project>/drafts/<id>.json`), or None.
    pub fn draft(&self, project: &str, id: &str) -> Result<Option<Vec<u8>>> {
        if !(is_id(project) && is_id(id)) {
            return Ok(None);
        }
        Ok(read_if_there(&self.layout().project_file(project, "drafts")?.join(format!("{id}.json")))?)
    }

    /// Keeps the draft `id` of an intervention's preview. The caller holds the store's
    /// transaction.
    pub fn keep_draft(&self, project: &str, id: &str, bytes: &[u8]) -> Result<()> {
        self.project(project)?;
        let folder = self.layout().project_file(project, "drafts")?;
        fs::create_dir_all(&folder)?;
        self.write_file(&folder.join(format!("{}.json", crate::layout::check_id(id)?)), bytes)
    }
}
