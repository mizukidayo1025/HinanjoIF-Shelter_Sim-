//! What the store's tests share: a folder of their own, the core's fixtures, and the browser's
//! Projects of `native/wasm/tests/node_stored_runs.mjs` made again through the store.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use sheltersim_core::checkpoint::sha256_hex;
use sheltersim_core::model_version::engine_signatures;
use sheltersim_store::compute::{Computation, ForkRequest};
use sheltersim_store::records::{Branch, NewRun, ProjectFiles};
use sheltersim_store::stored::{read_journal, StoredRunWriter};

/// deflate-raw of stored blocks of at most `size` bytes each (`BTYPE` 00): any inflater reads them,
/// and they are the same bytes everywhere.
fn stored_blocks(raw: &[u8], size: usize) -> Vec<u8> {
    let size = size.clamp(1, 65535);
    let chunks: Vec<&[u8]> = if raw.is_empty() { vec![raw] } else { raw.chunks(size).collect() };
    let mut out = Vec::with_capacity(raw.len() + 5 * chunks.len());
    for (index, chunk) in chunks.iter().enumerate() {
        let length = chunk.len() as u16;
        out.push(u8::from(index + 1 == chunks.len()));
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&(!length).to_le_bytes());
        out.extend_from_slice(chunk);
    }
    out
}

/// What the browser's fixtures are compressed with (`node_stored_runs.mjs`): stored blocks of at
/// most 65535 bytes.
pub fn stored_deflate(raw: &[u8]) -> Vec<u8> {
    stored_blocks(raw, 65535)
}

/// A folder for one test, removed when it is dropped.
pub struct TempDir(PathBuf);

static NEXT: AtomicU64 = AtomicU64::new(0);

impl TempDir {
    pub fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("sheltersim-store-{name}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A file of the core's test data.
pub fn data(path: &str) -> Vec<u8> {
    fs::read(format!("{}/../core/tests/data/{path}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// The browser's fixture of a stored Run: its journal's bytes.
pub fn journal_fixture(name: &str) -> Vec<u8> {
    data(&format!("stored_runs/{name}.journal.jsonl"))
}

/// The browser's fixture of a stored Run: data.pack's length and sha256.
pub fn data_fixture(name: &str) -> (u64, String) {
    let runs: Value = serde_json::from_slice(&data("stored_runs/runs.json")).unwrap();
    (runs[name]["data_bytes"].as_u64().unwrap(), runs[name]["data_sha256"].as_str().unwrap().to_owned())
}

/// The model versions of the fixtures' Runs: the engine's items, as JSON.
pub fn engine() -> Value {
    serde_json::to_value(engine_signatures()).unwrap()
}

/// The folders of the browser's six stored Runs, as `node_stored_runs.mjs` names them, made again
/// in `root` through the store.
pub struct Made {
    #[allow(dead_code)]
    pub files: ProjectFiles,
    pub folders: Vec<(&'static str, PathBuf)>,
}

impl Made {
    pub fn folder(&self, name: &str) -> &Path {
        &self.folders.iter().find(|(each, _)| *each == name).unwrap().1
    }
}

/// Computes `branch` from its head to `target`, `ticks` at a time, as the browser's `resume`: the
/// branch and the Run `running`, then the computation's status.
pub fn resume(files: &ProjectFiles, project: &str, run: &str, branch: &str, target: i64, ticks: u64) -> &'static str {
    let lease = files.claim_branch(project, run, branch).unwrap();
    let record = Branch::of_record(&files.branch_record(project, run, branch).unwrap()).unwrap();
    files.write_branch(project, run, &Branch { status: Some("running".to_owned()), ..record }).unwrap();
    files.set_run_status(project, run, "running").unwrap();
    let mut computation: Computation = files.go_on(&lease, project, run, branch, target).unwrap();
    let status = loop {
        if let Some(status) = computation.chunk(files, ticks).unwrap() {
            break status;
        }
    };
    computation.end(files, status).unwrap();
    status
}

/// The line a page closed in the middle left cut short, and the question's turn kept, of the Run
/// `stopped` (as `node_stored_runs.mjs` writes them).
const TORN: &str = r#"{"kind":"log_block","offset":1"#;
const TURN: &str = r#"{"answer":"a","question":"q"}"#;

/// Computes `branch` from its head toward `target` as the browser's `resume`, stopped (a cancel)
/// after `chunks` chunks of `ticks`: the branch and the Run `running`, then the status it ends with.
pub fn stop_after(files: &ProjectFiles, project: &str, run: &str, branch: &str, target: i64, ticks: u64, chunks: usize) -> &'static str {
    let lease = files.claim_branch(project, run, branch).unwrap();
    let record = Branch::of_record(&files.branch_record(project, run, branch).unwrap()).unwrap();
    files.write_branch(project, run, &Branch { status: Some("running".to_owned()), ..record }).unwrap();
    files.set_run_status(project, run, "running").unwrap();
    let mut computation: Computation = files.go_on(&lease, project, run, branch, target).unwrap();
    for _ in 0..chunks {
        assert_eq!(computation.chunk(files, ticks).unwrap(), None);
    }
    let status = computation.stop(files).unwrap();
    computation.end(files, status).unwrap();
    status
}

/// A Run to compute beside `r-sample` of `p1` (compute_run.mjs): the sample's "run" line and
/// "export" piece, copied, and its records, `queued` at minute 0.
fn make_compute_run(files: &ProjectFiles, run_id: &str) {
    let source = files.reader("p1", "r-sample", "main").unwrap();
    let export = source.journal().find("export").unwrap();
    let compressed = source.compressed(export).unwrap();
    let mut writer =
        StoredRunWriter::create(&files.branch_folder("p1", run_id, "main").unwrap(), files.quota().unwrap()).unwrap().with_compress(files.compress());
    writer.core.same_start(source.journal()).unwrap();
    let head = writer.flush(Some(&compressed)).unwrap().unwrap();
    drop(writer);
    let source_record = files.run_record("p1", "r-sample").unwrap();
    let evidence = files.evidence_bytes("p1", "r-sample").unwrap();
    let run = NewRun {
        run_id,
        scenario_id: "trial",
        scenario_version: 1,
        seed: 7,
        model_versions: &source_record["model_versions"],
        evidence: &evidence,
        created_at: "2026-11-15T01:30:00Z",
    };
    files.create_run("p1", &run).unwrap();
    let main = Branch { branch_id: "main".to_owned(), checkpoint_hash: Some(head), status: Some("queued".to_owned()), ..Branch::default() };
    files.write_branch("p1", run_id, &main).unwrap();
}

/// The browser's stored Runs made again: from_zero and no_points (`runAndStore` in a folder of
/// their own), computed (a Run of the sample's exported world computed in chunks to a target and
/// on to the end; stopped, the same stopped, interrupted, recovered, computed on and a turn kept)
/// and fork_parent and fork (a branch made by an intervention and computed to the
/// end), with the store's compressor.
pub fn make_browser_runs(root: &Path, files: ProjectFiles) -> Made {
    use sheltersim_store::stored::{run_and_store, FromStart};
    let tick = data("tick_core.json");
    let mut folders = Vec::new();
    for (name, every) in [("from_zero", 360), ("no_points", 0)] {
        let folder = root.join("plain").join(name);
        let versions = engine_signatures();
        let start = FromStart { export: &tick, world: Some("small_replay"), ticks: 50, save_every: every };
        run_and_store(&folder, &start, versions, files.quota().unwrap(), files.compress()).unwrap();
        folders.push((name, folder));
    }

    // computed: a sample computed with runAndStore, beside a Run of its exported world.
    let duration = sheltersim_store::compute::plain_run(&tick, Some("small_replay")).unwrap().duration().unwrap();
    files.create_project("p1").unwrap();
    let scenario = format!("{{\"duration_minutes\":{duration},\"id\":\"trial\",\"seed\":7}}").into_bytes();
    files.save_scenario("p1", &scenario, &sha256_hex(&scenario)).unwrap();
    let engine = engine();
    let evidence = br#"{"schema_version":1}"#;
    let sample = NewRun {
        run_id: "r-sample",
        scenario_id: "trial",
        scenario_version: 1,
        seed: 7,
        model_versions: &engine,
        evidence,
        created_at: "2026-11-15T01:30:00Z",
    };
    files.create_run("p1", &sample).unwrap();
    let lease = files.claim_branch("p1", "r-sample", "main").unwrap();
    let start = FromStart { export: &tick, world: Some("small_replay"), ticks: 120, save_every: 360 };
    files.compute_branch(&lease, "p1", "r-sample", "main", &start).unwrap();
    lease.release();
    files.set_run_status("p1", "r-sample", "completed").unwrap();
    // The Run to compute (compute_run.mjs): the sample's "run" line and "export" piece, copied.
    let compute = "run-browser-compute";
    make_compute_run(&files, compute);
    assert_eq!(resume(&files, "p1", compute, "main", 360 + 140, 60), "paused");
    assert_eq!(resume(&files, "p1", compute, "main", duration, 60), "completed");
    folders.push(("computed", files.branch_folder("p1", compute, "main").unwrap()));

    // stopped: stopped after two chunks, interrupted with a line cut short after its head,
    // recovered, computed to the end, and a question's turn kept.
    let stopped = "run-stopped";
    make_compute_run(&files, stopped);
    assert_eq!(stop_after(&files, "p1", stopped, "main", duration, 60, 2), "paused");
    let folder = files.branch_folder("p1", stopped, "main").unwrap();
    let mut journal = fs::OpenOptions::new().append(true).open(folder.join("journal.jsonl")).unwrap();
    std::io::Write::write_all(&mut journal, TORN.as_bytes()).unwrap();
    drop(journal);
    let record = Branch::of_record(&files.branch_record("p1", stopped, "main").unwrap()).unwrap();
    files.write_branch("p1", stopped, &Branch { status: Some("running".to_owned()), ..record }).unwrap();
    files.set_run_status("p1", stopped, "running").unwrap();
    assert_eq!(files.recover_interrupted().unwrap(), [("p1".to_owned(), stopped.to_owned(), "main".to_owned())]);
    assert_eq!(resume(&files, "p1", stopped, "main", duration, 60), "completed");
    let lease = files.claim_branch("p1", stopped, "main").unwrap();
    files.keep_turn(&lease, "p1", stopped, "main", TURN.as_bytes()).unwrap();
    lease.release();
    folders.push(("stopped", folder));

    // fork_parent and fork: interventions.json's world, its branch `main` computed to the end, and a
    // branch made at minute 400 with the fixture's program, computed to the end.
    let raw = data("interventions.json");
    let fixture: Value = serde_json::from_slice(&raw).unwrap();
    let meta = &fixture["run"];
    let (project, run_id) = (meta["project_id"].as_str().unwrap(), meta["run_id"].as_str().unwrap());
    files.create_project(project).unwrap();
    let scenario = fixture["scenario_json"].as_str().unwrap().as_bytes();
    let saved = files.save_scenario(project, scenario, &sha256_hex(scenario)).unwrap();
    let run = NewRun {
        run_id,
        scenario_id: saved["scenario_id"].as_str().unwrap(),
        scenario_version: saved["scenario_version"].as_i64().unwrap(),
        seed: meta["seed"].as_i64().unwrap(),
        model_versions: &meta["model_versions"],
        evidence: br#"{"schema_version":1}"#,
        created_at: "2026-11-15T01:30:00Z",
    };
    files.create_run(project, &run).unwrap();
    let lease = files.claim_branch(project, run_id, "main").unwrap();
    let start = FromStart { export: &raw, world: Some("interventions"), ticks: 60, save_every: 360 };
    files.compute_branch(&lease, project, run_id, "main", &start).unwrap();
    lease.release();
    files.set_run_status(project, run_id, "completed").unwrap();
    let fork_line = fork_line();
    let lease = files.claim_branch(project, run_id, "c1").unwrap();
    let request = ForkRequest {
        parent: "main".to_owned(),
        child: "c1".to_owned(),
        minute: 400,
        program: fork_line["program"].as_str().unwrap().to_owned(),
        approved_program_hash: fork_line["approved_program_hash"].as_str().unwrap().to_owned(),
        state_hash: Some(fork_line["fork_origin"]["state_hash"].as_str().unwrap().to_owned()),
    };
    let forked = files.fork(&lease, project, run_id, &request).unwrap();
    lease.release();
    assert_eq!(forked.receipt, fork_line["receipt"].as_str().unwrap());
    assert_eq!(resume(&files, project, run_id, "c1", meta["minutes"].as_i64().unwrap(), 60), "completed");
    folders.push(("fork_parent", files.branch_folder(project, run_id, "main").unwrap()));
    folders.push(("fork", files.branch_folder(project, run_id, "c1").unwrap()));
    Made { files, folders }
}

/// The "fork" line of the browser's fork fixture.
pub fn fork_line() -> Value {
    let journal = sheltersim_core::run_store::Journal::parse(&journal_fixture("fork")).unwrap();
    let index = journal.find("fork").unwrap();
    serde_json::from_str(&journal.entries()[index].json).unwrap()
}

/// The journal of a folder's stored Run, parsed.
pub fn journal_of(folder: &Path) -> sheltersim_core::run_store::Journal {
    read_journal(folder).unwrap()
}
