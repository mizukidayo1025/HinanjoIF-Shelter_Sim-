//! The computing procedure of a stored Run without I/O (`compute`, #476): a host in memory does its
//! steps as the browser's `js/procedure.mjs` and the store's `sheltersim_store::compute` do them
//! with their files. Computed from the start and gone on with in chunks, it writes the browser's
//! stored Runs (`tests/data/stored_runs/`, with the fixtures' stand-in compression) byte for byte;
//! the branch's record moves at each saved point and at the target; a line cut short after the
//! head is cut off first; a computation stopped early saves where it got to; a fork, a question's
//! turn and the recovery of an interrupted branch take their steps; and what is refused is
//! refused with the store's texts.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use serde_json::Value;
use sheltersim_core::answers::Failure;
use sheltersim_core::checkpoint::sha256_hex;
use sheltersim_core::compute::{BranchRecord, ForkRequest, Given, Lineage, Procedure, Start, Step, Where, OUT_OF_RANGE};
use sheltersim_core::model_version;
use sheltersim_core::run_store::Journal;

fn data(path: &str) -> Vec<u8> {
    fs::read(format!("{}/tests/data/{path}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn fixture(name: &str) -> Vec<u8> {
    data(&format!("stored_runs/{name}.journal.jsonl"))
}

fn summary(name: &str) -> Value {
    let runs: Value = serde_json::from_slice(&data("stored_runs/runs.json")).unwrap();
    runs[name].clone()
}

/// What `node_stored_runs.mjs` compresses the fixtures' pieces with: deflate's stored blocks.
fn stored_deflate(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len() + 5);
    let chunks: Vec<&[u8]> = if raw.is_empty() { vec![raw] } else { raw.chunks(65535).collect() };
    for (index, chunk) in chunks.iter().enumerate() {
        let length = chunk.len() as u16;
        out.push(u8::from(index + 1 == chunks.len()));
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&(!length).to_le_bytes());
        out.extend_from_slice(chunk);
    }
    out
}

fn key(at: &Where, name: &str) -> String {
    format!("{}/{}/{}/{name}", at.project, at.run, at.branch)
}

/// A store in memory: each branch's journal and data.pack (with each piece's bytes before
/// compression by where it starts), the records by path, the locks held by others and taken, the
/// records written and the steps done.
#[derive(Default)]
struct Memory {
    files: BTreeMap<String, Vec<u8>>,
    raw: BTreeMap<(String, u64), Vec<u8>>,
    records: BTreeMap<String, Vec<u8>>,
    held: BTreeSet<String>,
    taken: BTreeSet<String>,
    written: Vec<BranchRecord>,
    steps: Vec<Step>,
}

impl Memory {
    /// A Run `p/r` whose record holds the engine's versions.
    fn new() -> Self {
        let mut memory = Self::default();
        let versions = serde_json::to_value(model_version::engine_signatures()).unwrap();
        memory.records.insert("p/r/run.json".to_owned(), serde_json::to_vec(&serde_json::json!({"model_versions": versions, "status": "queued"})).unwrap());
        memory
    }

    fn record(&self, at: &Where) -> Option<Value> {
        self.records.get(&key(at, "branch.json")).map(|bytes| serde_json::from_slice(bytes).unwrap())
    }

    fn set_status(&mut self, path: String, status: &str) {
        let mut record: Value = serde_json::from_slice(&self.records[&path]).unwrap();
        record["status"] = status.into();
        self.records.insert(path, serde_json::to_vec(&record).unwrap());
    }

    /// Does the procedure's steps up to `Compute` or `Done`, or to its failure.
    fn try_run(&mut self, procedure: &mut Procedure) -> Result<Step, Failure> {
        loop {
            let step = procedure.next()?;
            self.steps.push(step.clone());
            let given = match &step {
                Step::Compute | Step::Done => return Ok(step),
                Step::RunRecord { project, run } => Given::Text(self.records.get(&format!("{project}/{run}/run.json")).cloned()),
                Step::BranchRecord { at, .. } => Given::Text(self.records.get(&key(at, "branch.json")).cloned()),
                Step::Branches { project, run } => {
                    let prefix = format!("{project}/{run}/");
                    Given::Texts(
                        self.records
                            .iter()
                            .filter(|(path, _)| path.starts_with(&prefix) && path.ends_with("/branch.json"))
                            .map(|(_, bytes)| bytes.clone())
                            .collect(),
                    )
                }
                Step::Projects => Given::Ids(vec!["p".to_owned()]),
                Step::Runs { .. } => Given::Ids(vec!["r".to_owned()]),
                Step::Journal(at) => Given::Text(self.files.get(&key(at, "journal.jsonl")).cloned()),
                Step::Piece { at, stored } => Given::Text(Some(self.raw[&(key(at, "data.pack"), stored.offset)].clone())),
                Step::Create(at) => {
                    self.files.insert(key(at, "journal.jsonl"), Vec::new());
                    self.files.insert(key(at, "data.pack"), Vec::new());
                    Given::Nothing
                }
                Step::Truncate { at, journal, data } => {
                    self.files.get_mut(&key(at, "journal.jsonl")).unwrap().truncate(*journal as usize);
                    if let Some(data) = data {
                        self.files.get_mut(&key(at, "data.pack")).unwrap().truncate(*data as usize);
                    }
                    Given::Nothing
                }
                Step::WritePiece { at, .. } => {
                    let raw = procedure.raw().to_vec();
                    let stored = stored_deflate(&raw);
                    let file = self.files.entry(key(at, "data.pack")).or_default();
                    let offset = file.len() as u64;
                    file.extend_from_slice(&stored);
                    self.raw.insert((key(at, "data.pack"), offset), raw);
                    Given::Place { offset, length: stored.len() as u64 }
                }
                Step::AppendLine(at) => {
                    self.files.entry(key(at, "journal.jsonl")).or_default().extend_from_slice(procedure.line());
                    Given::Nothing
                }
                Step::WriteBranch { project, run, record } => {
                    let mut value = record.to_value();
                    if record.status.is_none() {
                        value.as_object_mut().unwrap().remove("status");
                    }
                    self.records.insert(format!("{project}/{run}/{}/branch.json", record.branch_id), serde_json::to_vec(&value).unwrap());
                    self.written.push(record.clone());
                    Given::Nothing
                }
                Step::SetStatus { at, status } => {
                    self.set_status(key(at, "branch.json"), status);
                    self.set_status(format!("{}/{}/run.json", at.project, at.run), status);
                    Given::Nothing
                }
                Step::SetRunStatus { project, run, status } => {
                    self.set_status(format!("{project}/{run}/run.json"), status);
                    Given::Nothing
                }
                Step::TryLock(at) => {
                    let name = key(at, "lock");
                    let free = !self.held.contains(&name);
                    if free {
                        self.taken.insert(name);
                    }
                    Given::Lock(free)
                }
                Step::Unlock(at) => {
                    assert!(self.taken.remove(&key(at, "lock")), "a lock let go that was not taken");
                    Given::Nothing
                }
            };
            procedure.give(given).unwrap();
        }
    }

    fn run(&mut self, procedure: &mut Procedure) -> Step {
        self.try_run(procedure).unwrap_or_else(|failure| panic!("{}", text(failure)))
    }

    /// Goes on with `at` to `target`, `ticks` at a time: its status.
    fn go_on(&mut self, at: &Where, target: i64, ticks: u64) -> &'static str {
        let mut procedure = Procedure::go_on(at.clone(), target);
        while self.run(&mut procedure) == Step::Compute {
            procedure.advance(ticks).unwrap();
        }
        procedure.status().unwrap()
    }

    /// The refusal a procedure ends with.
    fn refusal(&mut self, procedure: &mut Procedure) -> String {
        match self.try_run(procedure) {
            Err(Failure::Refused(refusal)) => refusal.error,
            Err(Failure::Error(error)) => panic!("an error, not a refusal: {error}"),
            Ok(step) => panic!("{step:?}, not refused"),
        }
    }
}

fn text(failure: Failure) -> String {
    match failure {
        Failure::Refused(refusal) => refusal.error,
        Failure::Error(error) => error.to_string(),
    }
}

fn tick_start(save_every: u64) -> Start {
    Start { export: data("tick_core.json"), world: Some("small_replay".to_owned()), save_every }
}

fn duration() -> i64 {
    sheltersim_core::compute::plain_run(&data("tick_core.json"), Some("small_replay")).unwrap().duration().unwrap()
}

#[test]
fn computed_from_the_start_it_writes_the_browsers_stored_runs() {
    for (name, every) in [("from_zero", 360), ("no_points", 0)] {
        let mut memory = Memory::new();
        let at = Where::new("p", "r", "main");
        let mut procedure = Procedure::from_start(at.clone(), tick_start(every), Some(model_version::engine_signatures()), None);
        while memory.run(&mut procedure) == Step::Compute {
            procedure.advance(50).unwrap();
        }
        assert_eq!(procedure.status(), Some("completed"));
        assert!(memory.files[&key(&at, "journal.jsonl")] == fixture(name), "{name}: the journal is not the browser's");
        let pack = &memory.files[&key(&at, "data.pack")];
        assert_eq!(
            (pack.len() as u64, sha256_hex(pack)),
            (summary(name)["data_bytes"].as_u64().unwrap(), summary(name)["data_sha256"].as_str().unwrap().to_owned())
        );
        // Without a lineage, no record is written.
        assert!(memory.written.is_empty(), "{name}");
    }
}

#[test]
fn going_on_in_chunks_moves_the_record_at_each_saved_point_and_writes_the_browsers_run() {
    let mut memory = Memory::new();
    let at = Where::new("p", "r", "main");
    let mut start = Procedure::start(at.clone(), tick_start(360));
    assert_eq!(memory.run(&mut start), Step::Done);
    let record = memory.record(&at).unwrap();
    assert_eq!((record["computed_until"].as_i64(), record.get("status")), (Some(0), None));
    memory.written.clear();

    assert_eq!(memory.go_on(&at, 360 + 140, 60), "paused");
    let moved: Vec<_> = memory.written.iter().map(|record| (record.computed_until, record.status.clone())).collect();
    assert_eq!(moved, [(Some(360), Some("running".to_owned())), (Some(500), Some("running".to_owned()))]);
    memory.written.clear();
    let duration = duration();
    assert_eq!(memory.go_on(&at, duration, 60), "completed");
    let last = memory.written.last().unwrap();
    assert_eq!(last.computed_until, Some(duration));
    // The fixture `computed` is this Run, whose start the browser copied from the sample.
    assert!(memory.files[&key(&at, "journal.jsonl")] == fixture("computed"), "the journal is not the browser's");
    let pack = &memory.files[&key(&at, "data.pack")];
    assert_eq!(sha256_hex(pack), summary("computed")["data_sha256"].as_str().unwrap());
    let journal = Journal::parse(&memory.files[&key(&at, "journal.jsonl")]).unwrap();
    assert_eq!(last.checkpoint_hash.as_deref(), Some(journal.entries().last().unwrap().head.as_str()));
}

#[test]
fn the_ticks_run_alone_and_the_steps_after_them_store_what_they_made() {
    let mut memory = Memory::new();
    let at = Where::new("p", "r", "main");
    memory.run(&mut Procedure::start(at.clone(), tick_start(360)));
    let mut procedure = Procedure::go_on(at.clone(), 120);
    assert_eq!(memory.run(&mut procedure), Step::Compute);
    // Nothing is given at `Compute`: the host runs the ticks or stops.
    assert!(procedure.give(Given::Nothing).is_err());
    let journal = memory.files[&key(&at, "journal.jsonl")].clone();
    assert_eq!(procedure.advance(60).unwrap(), 60);
    // The ticks write nothing: the files are the same until the steps after them are done.
    assert_eq!(memory.files[&key(&at, "journal.jsonl")], journal);
    assert!(procedure.advance(60).is_err(), "ticks again before the steps of the last are done");
    memory.steps.clear();
    assert_eq!(memory.run(&mut procedure), Step::Compute);
    assert!(memory.steps.iter().all(|step| matches!(step, Step::WritePiece { .. } | Step::AppendLine(_) | Step::Compute)), "{:?}", memory.steps);
}

#[test]
fn going_on_cuts_off_what_a_computation_left_after_the_head() {
    let mut memory = Memory::new();
    let at = Where::new("p", "r", "main");
    memory.run(&mut Procedure::start(at.clone(), tick_start(360)));
    let head = (memory.files[&key(&at, "journal.jsonl")].len() as u64, memory.files[&key(&at, "data.pack")].len() as u64);
    // A computation that wrote past the head and stopped before it moved the record: whole lines,
    // then a line cut short.
    let mut procedure = Procedure::go_on(at.clone(), 300);
    memory.run(&mut procedure);
    procedure.advance(60).unwrap();
    memory.run(&mut procedure);
    memory.files.get_mut(&key(&at, "journal.jsonl")).unwrap().extend_from_slice(b"{\"kind\":\"log_block\",\"off");
    assert!(memory.files[&key(&at, "journal.jsonl")].len() as u64 > head.0);
    memory.steps.clear();
    assert_eq!(memory.go_on(&at, 300, 60), "paused");
    let cut = memory.steps.iter().find_map(|step| match step {
        Step::Truncate { journal, data, .. } => Some((*journal, *data)),
        _ => None,
    });
    assert_eq!(cut, Some((head.0, Some(head.1))));
    // From the head, it reaches the target as a computation that was never cut short does.
    let mut clean = Memory::new();
    clean.run(&mut Procedure::start(at.clone(), tick_start(360)));
    assert_eq!(clean.go_on(&at, 300, 60), "paused");
    assert!(memory.files[&key(&at, "journal.jsonl")] == clean.files[&key(&at, "journal.jsonl")]);
}

#[test]
fn a_computation_stopped_early_saves_where_it_got_to() {
    let mut memory = Memory::new();
    let at = Where::new("p", "r", "main");
    memory.run(&mut Procedure::start(at.clone(), tick_start(360)));
    let mut procedure = Procedure::go_on(at.clone(), 1000);
    memory.run(&mut procedure);
    // Stopped at the head: nothing written.
    let mut at_head = Procedure::go_on(at.clone(), 1000);
    memory.run(&mut at_head);
    at_head.stop().unwrap();
    memory.written.clear();
    assert_eq!(memory.run(&mut at_head), Step::Done);
    assert_eq!(at_head.status(), Some("paused"));
    assert!(memory.written.is_empty());
    // Stopped after two chunks: a saved point at minute 120 and the record there.
    let mut procedure = Procedure::go_on(at.clone(), 1000);
    memory.run(&mut procedure);
    for _ in 0..2 {
        procedure.advance(60).unwrap();
        assert_eq!(memory.run(&mut procedure), Step::Compute);
    }
    procedure.stop().unwrap();
    assert_eq!(memory.run(&mut procedure), Step::Done);
    assert_eq!(procedure.status(), Some("paused"));
    let record = memory.written.last().unwrap();
    assert_eq!((record.computed_until, record.status.as_deref()), (Some(120), Some("running")));
    let journal = Journal::parse(&memory.files[&key(&at, "journal.jsonl")]).unwrap();
    assert_eq!(journal.saved(journal.saved_points()[0]).unwrap().minute, 120);
}

#[test]
fn what_is_refused_is_refused_with_the_stores_texts() {
    let mut memory = Memory::new();
    let at = Where::new("p", "r", "main");
    // A branch without a head.
    memory
        .records
        .insert(key(&at, "branch.json"), br#"{"branch_id":"main","parent_branch_id":null,"fork_minute":0,"computed_until":0,"checkpoint_hash":null}"#.to_vec());
    assert_eq!(memory.refusal(&mut Procedure::go_on(at.clone(), 60)), "branch has no computed minutes");
    memory.run(&mut Procedure::start(at.clone(), tick_start(360)));
    // A target outside what is left.
    assert_eq!(memory.refusal(&mut Procedure::go_on(at.clone(), duration() + 1)), OUT_OF_RANGE);
    assert_eq!(memory.refusal(&mut Procedure::go_on(at.clone(), 0)), OUT_OF_RANGE);
    // A child that is there, and a parent whose state is not the one the preview saw.
    let request = |child: &str, state_hash: Option<&str>| ForkRequest {
        parent: "main".to_owned(),
        child: child.to_owned(),
        minute: 0,
        program: "{}".to_owned(),
        approved_program_hash: "x".to_owned(),
        state_hash: state_hash.map(str::to_owned),
    };
    assert_eq!(memory.refusal(&mut Procedure::fork("p", "r", request("main", None))), "child branch already exists");
    assert_eq!(memory.refusal(&mut Procedure::fork("p", "r", request("c1", Some("not the state")))), "source state changed; preview the intervention again");
}

#[test]
fn a_computation_from_the_start_with_its_record_marks_it_running_then_completed_or_failed() {
    let mut memory = Memory::new();
    let at = Where::new("p", "r", "main");
    let lineage = Lineage::default();
    let mut procedure = Procedure::from_start(at.clone(), tick_start(360), None, Some(lineage.clone()));
    while memory.run(&mut procedure) == Step::Compute {
        procedure.advance(720).unwrap();
    }
    let statuses: Vec<_> = memory.written.iter().map(|record| record.status.clone().unwrap()).collect();
    assert_eq!(statuses, ["running", "completed"]);
    assert_eq!(memory.steps[0].name(), "run_record");
    // A start that is not a world: the record is marked failed.
    memory.written.clear();
    let mut failing = Procedure::from_start(at.clone(), Start { export: b"{}".to_vec(), world: None, save_every: 360 }, None, Some(lineage));
    assert!(memory.try_run(&mut failing).is_err());
    assert!(failing.fail());
    assert_eq!(memory.run(&mut failing), Step::Done);
    let statuses: Vec<_> = memory.written.iter().map(|record| (record.status.clone().unwrap(), record.checkpoint_hash.clone())).collect();
    assert_eq!(statuses, [("running".to_owned(), None), ("failed".to_owned(), None)]);
    assert_eq!(failing.status(), Some("failed"));
}

#[test]
fn a_turn_is_kept_after_the_end_line_a_line_cut_short_cut_off_first() {
    let mut memory = Memory::new();
    let at = Where::new("p", "r", "main");
    let mut procedure = Procedure::from_start(at.clone(), tick_start(360), None, Some(Lineage::default()));
    while memory.run(&mut procedure) == Step::Compute {
        procedure.advance(720).unwrap();
    }
    let end = memory.files[&key(&at, "journal.jsonl")].len();
    memory.files.get_mut(&key(&at, "journal.jsonl")).unwrap().extend_from_slice(b"{\"kind\":");
    assert_eq!(memory.run(&mut Procedure::keep_turn(at.clone(), br#"{"question":"q"}"#.to_vec())), Step::Done);
    let journal = Journal::parse(&memory.files[&key(&at, "journal.jsonl")]).unwrap();
    assert!(!journal.torn());
    assert_eq!(journal.entries().len(), Journal::parse(&memory.files[&key(&at, "journal.jsonl")][..end]).unwrap().entries().len() + 1);
    assert_eq!(journal.turns().len(), 1);
    // A Run not computed to its end keeps no turn.
    let other = Where::new("p", "r", "b");
    memory.run(&mut Procedure::start(other.clone(), tick_start(360)));
    let refused = memory.try_run(&mut Procedure::keep_turn(other, b"{}".to_vec()));
    assert!(matches!(refused, Err(Failure::Error(_))), "{refused:?}");
}

#[test]
fn a_branch_left_running_with_its_lock_free_is_interrupted() {
    let mut memory = Memory::new();
    for (branch, status) in [("a", "running"), ("b", "running"), ("c", "paused")] {
        let record =
            format!(r#"{{"branch_id":"{branch}","parent_branch_id":null,"fork_minute":0,"computed_until":0,"checkpoint_hash":null,"status":"{status}"}}"#);
        memory.records.insert(format!("p/r/{branch}/branch.json"), record.into_bytes());
    }
    // Another writer holds b.
    memory.held.insert("p/r/b/lock".to_owned());
    let mut procedure = Procedure::recover();
    assert_eq!(memory.run(&mut procedure), Step::Done);
    assert_eq!(procedure.recovered(), [Where::new("p", "r", "a")]);
    let status = |branch: &str| memory.record(&Where::new("p", "r", branch)).unwrap()["status"].as_str().unwrap().to_owned();
    assert_eq!((status("a"), status("b"), status("c")), ("interrupted".to_owned(), "running".to_owned(), "paused".to_owned()));
    let run: Value = serde_json::from_slice(&memory.records["p/r/run.json"]).unwrap();
    assert_eq!(run["status"], "interrupted");
    assert!(memory.taken.is_empty(), "a lock was kept");
}

#[test]
fn a_step_is_json_a_host_in_another_language_reads() {
    let at = Where::new("p", "r", "main");
    let piece = Step::Truncate { at: at.clone(), journal: 10, data: None };
    assert_eq!(piece.to_json(), r#"{"branch":"main","data":null,"do":"truncate","journal":10,"project":"p","run":"r"}"#);
    let record = BranchRecord { branch_id: "main".to_owned(), computed_until: Some(360), status: Some("running".to_owned()), ..BranchRecord::default() };
    let write = Step::WriteBranch { project: "p".to_owned(), run: "r".to_owned(), record };
    let value: Value = serde_json::from_str(&write.to_json()).unwrap();
    assert_eq!(value["record"]["computed_until"], 360);
    assert_eq!(value["record"]["parent_branch_id"], Value::Null);
    assert_eq!(Step::Compute.to_json(), r#"{"do":"compute"}"#);
}
