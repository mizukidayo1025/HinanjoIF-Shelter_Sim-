//! The computing procedure of a branch's stored Run (#476), without I/O: the steps the browser's
//! `js/compute.mjs` and the local and VM servers' `sheltersim_store` (#426) used to take each on
//! their own, now taken once here. A `Procedure` says what its host does next (`Procedure::next`, a
//! `Step`: read this record, read this journal, read this piece, write this piece, append this
//! line, write this record, set this status, compute…) and is given what the host read or where
//! it wrote (`Procedure::give`); it decides everything else. The host opens the files, compresses
//! and decompresses the pieces, flushes them, takes the locks and checks the store's limit: the
//! browser with OPFS (`js/procedure.mjs`), the servers with files on a disk
//! (`sheltersim_store::compute`).
//!
//! The procedures (each a constructor):
//! - `go_on` (resume): the branch cut back to the head its record names (the lines after it, then
//!   the bytes of data.pack no line up to it points at; refused where a branch forked from it reads
//!   past the head), its stored Run's chain opened (a fork's parents' journals), and the Run
//!   restored there (`seek`). Then, while it says `Step::Compute`,
//!   the host either runs a chunk of ticks (`advance`: at most the ticks it is given, not past the
//!   next saved point or the target) or ends it early (`stop`); the steps after each store the
//!   blocks sealed since, and at every `save_every` minutes, at the target and at the clock's end a
//!   saved point (at the end the "world" and "end" lines too), and move the branch's record to
//!   that line (`running` until the computation ends). It is `Done` at the target (`paused`), at
//!   the clock's end (`completed`) or once stopped. The record is written after the lines it names
//!   are flushed: the record is the commit.
//! - `fork`: a child branch made from its parent at a minute with an EffectProgram: the parent's
//!   Run there (checked against the state the preview saw), the program applied, the child's
//!   stored Run (reading the parent's pieces up to the line the parent's Run was restored from) and
//!   its record.
//! - `keep_turn`: a question's turn kept after the "end" line of a branch computed to its end.
//! - `start`: a branch's stored Run at minute 0 of an exported world (the "run" line and the
//!   "export" piece) and its record there, without a status.
//! - `from_start`: a Run computed from minute 0 to its end in one go and stored as it goes, with
//!   its record or without one (a stored Run of the tests and the measures).
//! - `recover`: the branches left `running` whose lock is free become `interrupted`, with their
//!   Runs.
//! - `set_status`: a branch's status and its Run's, as the host keeps them.
//!
//! The ticks run in `advance` only, and the steps after them are what the host does under the
//! store's transaction (#465): a Run's LLM link may take that transaction itself. A seek's ticks of
//! a Run that kept an LLM tape are answered from the tape's entries after the restored point's
//! count (`llm::Replay`, #428).
//!
//! A step the host cannot do is the host's error; the procedure is not used after an error (its
//! own or the host's), except `fail`, which gives the steps a computation from the start takes to
//! mark its branch failed.

use std::collections::VecDeque;

use serde_json::{json, Value};

use crate::answers::{Answer, Failure, Refusal};
use crate::archive::Restore;
use crate::error::Error;
use crate::llm::{Limits, Replay};
use crate::model_version::{versions_of_json, Versions};
use crate::plain::PlainRun;
use crate::run_store::{records, Chain, Fork, ForkOrigin, Head, HeadKind, Journal, Next, RecordKind, RunWriter, Schedule, Stored, Storing};

/// The compression of the pieces both hosts write, which the "run" line names.
pub const COMPRESSION: &str = "deflate-raw";
/// The status of a branch and its Run while a computation goes on.
pub const RUNNING: &str = "running";
/// The status of a branch left `running` that no writer holds.
pub const INTERRUPTED: &str = "interrupted";
/// The refusal of a target outside what is left to compute.
pub const OUT_OF_RANGE: &str = "target minute must be in the uncomputed remaining range";

fn refused(text: impl Into<String>) -> Failure {
    Failure::Refused(Refusal::storage(text))
}

fn error(text: impl Into<String>) -> Failure {
    Failure::Error(Error::Value(text.into()))
}

/// A branch of a Run of a Project: where a step reads or writes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Where {
    pub project: String,
    pub run: String,
    pub branch: String,
}

impl Where {
    pub fn new(project: &str, run: &str, branch: &str) -> Self {
        Self { project: project.to_owned(), run: run.to_owned(), branch: branch.to_owned() }
    }

    /// Another branch of the same Run.
    pub fn of(&self, branch: &str) -> Self {
        Self { branch: branch.to_owned(), ..self.clone() }
    }

    fn fields(&self, step: &str) -> serde_json::Map<String, Value> {
        let mut fields = serde_json::Map::new();
        fields.insert("do".to_owned(), step.into());
        fields.insert("project".to_owned(), self.project.as_str().into());
        fields.insert("run".to_owned(), self.run.as_str().into());
        fields.insert("branch".to_owned(), self.branch.as_str().into());
        fields
    }
}

/// A branch's record, as the hosts write it (`branch.json`): its lineage, how far it is computed,
/// its head (`checkpoint_hash`, a head hash of its journal) and its status (none for a branch not
/// computed yet).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BranchRecord {
    pub branch_id: String,
    pub parent_branch_id: Option<String>,
    pub fork_minute: i64,
    pub computed_until: Option<i64>,
    pub checkpoint_hash: Option<String>,
    pub status: Option<String>,
}

impl BranchRecord {
    /// The record of a branch's JSON, with the store's refusals.
    pub fn of_json(bytes: &[u8]) -> Answer<Self> {
        let value: Value = serde_json::from_slice(bytes).map_err(|_| refused("saved artifact is missing or corrupt"))?;
        let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
        Ok(Self {
            branch_id: text("branch_id").ok_or_else(|| refused("invalid artifact identifier"))?,
            parent_branch_id: text("parent_branch_id"),
            fork_minute: value.get("fork_minute").and_then(Value::as_i64).ok_or_else(|| refused("invalid fork minute"))?,
            computed_until: Some(value.get("computed_until").and_then(Value::as_i64).ok_or_else(|| refused("invalid computed minute"))?),
            checkpoint_hash: text("checkpoint_hash"),
            status: text("status"),
        })
    }

    /// As JSON, its fields by their names (a status that is none as null).
    pub fn to_value(&self) -> Value {
        json!({
            "branch_id": self.branch_id,
            "parent_branch_id": self.parent_branch_id,
            "fork_minute": self.fork_minute,
            "computed_until": self.computed_until.unwrap_or(self.fork_minute),
            "checkpoint_hash": self.checkpoint_hash,
            "status": self.status,
        })
    }
}

/// What the host does next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Give the Run's record (run.json) as JSON (`Given::Text`); the host refuses one missing.
    RunRecord { project: String, run: String },
    /// Give the branch's record as JSON (`Given::Text`). A record missing is the host's refusal,
    /// or where `optional`, none.
    BranchRecord { at: Where, optional: bool },
    /// Give the records of the Run's branches (`Given::Texts`).
    Branches { project: String, run: String },
    /// Give the ids of the store's Projects (`Given::Ids`).
    Projects,
    /// Give the ids of the Project's Runs (`Given::Ids`), or `Given::Failed` where they cannot be read.
    Runs { project: String },
    /// Give the branch's journal as it is (`Given::Text`; none where there is no file).
    Journal(Where),
    /// Give the piece `stored` of the branch's data.pack decompressed (`Given::Text`): an error of
    /// the host where data.pack ends before it. The procedure checks its bytes.
    Piece { at: Where, stored: Stored },
    /// Make the branch's journal and data.pack, empty (`Given::Nothing`).
    Create(Where),
    /// Cut the branch's journal to `journal` bytes, and data.pack to `data` where it is given, each
    /// where it is longer, and flush them (`Given::Nothing`).
    Truncate { at: Where, journal: u64, data: Option<u64> },
    /// Compress the piece's bytes (`Procedure::raw`), append them to the branch's data.pack and
    /// flush it (`Given::Place`: where they start and how many bytes were written).
    WritePiece { at: Where, kind: &'static str },
    /// Append the line (`Procedure::line`) to the branch's journal and flush it (`Given::Nothing`).
    AppendLine(Where),
    /// Write the branch's record (`Given::Nothing`).
    WriteBranch { project: String, run: String, record: BranchRecord },
    /// Set the branch's status and its Run's, as the host keeps them (`Given::Nothing`).
    SetStatus { at: Where, status: String },
    /// Set the Run's status (`Given::Nothing`).
    SetRunStatus { project: String, run: String, status: String },
    /// Take the lock of the branch's writer where no one holds it (`Given::Lock`).
    TryLock(Where),
    /// Let the lock taken go (`Given::Nothing`).
    Unlock(Where),
    /// Run a chunk of ticks (`Procedure::advance`, outside the store's transaction) or end the
    /// computation (`Procedure::stop`).
    Compute,
    /// Nothing more: the procedure's outcome is there.
    Done,
}

impl Step {
    /// Its name, as the JSON of a step names it.
    pub fn name(&self) -> &'static str {
        match self {
            Step::RunRecord { .. } => "run_record",
            Step::BranchRecord { .. } => "branch_record",
            Step::Branches { .. } => "branches",
            Step::Projects => "projects",
            Step::Runs { .. } => "runs",
            Step::Journal(_) => "journal",
            Step::Piece { .. } => "piece",
            Step::Create(_) => "create",
            Step::Truncate { .. } => "truncate",
            Step::WritePiece { .. } => "write_piece",
            Step::AppendLine(_) => "append_line",
            Step::WriteBranch { .. } => "write_branch",
            Step::SetStatus { .. } => "set_status",
            Step::SetRunStatus { .. } => "set_run_status",
            Step::TryLock(_) => "try_lock",
            Step::Unlock(_) => "unlock",
            Step::Compute => "compute",
            Step::Done => "done",
        }
    }

    /// As JSON: `{"do": name, …its fields}` (a host in another language reads it).
    pub fn to_json(&self) -> String {
        let name = self.name();
        let run_fields = |project: &str, run: &str| {
            let mut fields = serde_json::Map::new();
            fields.insert("do".to_owned(), name.into());
            fields.insert("project".to_owned(), project.into());
            fields.insert("run".to_owned(), run.into());
            fields
        };
        let fields = match self {
            Step::RunRecord { project, run } | Step::Branches { project, run } => run_fields(project, run),
            Step::BranchRecord { at, optional } => {
                let mut fields = at.fields(name);
                fields.insert("optional".to_owned(), (*optional).into());
                fields
            }
            Step::Projects | Step::Compute | Step::Done => {
                let mut fields = serde_json::Map::new();
                fields.insert("do".to_owned(), name.into());
                fields
            }
            Step::Runs { project } => {
                let mut fields = serde_json::Map::new();
                fields.insert("do".to_owned(), name.into());
                fields.insert("project".to_owned(), project.as_str().into());
                fields
            }
            Step::Journal(at) | Step::Create(at) | Step::AppendLine(at) | Step::TryLock(at) | Step::Unlock(at) => at.fields(name),
            Step::Piece { at, stored } => {
                let mut fields = at.fields(name);
                fields.insert("offset".to_owned(), stored.offset.into());
                fields.insert("length".to_owned(), stored.length.into());
                fields.insert("raw_length".to_owned(), stored.raw_length.into());
                fields
            }
            Step::Truncate { at, journal, data } => {
                let mut fields = at.fields(name);
                fields.insert("journal".to_owned(), (*journal).into());
                fields.insert("data".to_owned(), (*data).into());
                fields
            }
            Step::WritePiece { at, kind } => {
                let mut fields = at.fields(name);
                fields.insert("kind".to_owned(), (*kind).into());
                fields
            }
            Step::WriteBranch { project, run, record } => {
                let mut fields = run_fields(project, run);
                fields.insert("record".to_owned(), record.to_value());
                fields
            }
            Step::SetStatus { at, status } => {
                let mut fields = at.fields(name);
                fields.insert("status".to_owned(), status.as_str().into());
                fields
            }
            Step::SetRunStatus { project, run, status } => {
                let mut fields = run_fields(project, run);
                fields.insert("status".to_owned(), status.as_str().into());
                fields
            }
        };
        Value::Object(fields).to_string()
    }
}

/// What the host gives back for a step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Given {
    /// The step is done.
    Nothing,
    /// Bytes read, or none.
    Text(Option<Vec<u8>>),
    /// Records read.
    Texts(Vec<Vec<u8>>),
    /// Ids listed.
    Ids(Vec<String>),
    /// Where a piece was written in data.pack and how many bytes it took.
    Place { offset: u64, length: u64 },
    /// Whether the lock was taken.
    Lock(bool),
    /// The read could not be done (only where a step says it may be given).
    Failed,
}

impl Given {
    /// Records given as a JSON list of them, as a host in another language lists them.
    pub fn texts_of_json(json: &[u8]) -> Result<Given, Error> {
        let listed: Vec<Value> = serde_json::from_slice(json).map_err(|_| Error::Value("records are given as a JSON list".to_owned()))?;
        Ok(Given::Texts(listed.iter().map(|record| record.to_string().into_bytes()).collect()))
    }

    /// Ids given as a JSON list of strings.
    pub fn ids_of_json(json: &[u8]) -> Result<Given, Error> {
        Ok(Given::Ids(serde_json::from_slice(json).map_err(|_| Error::Value("ids are given as a JSON list of strings".to_owned()))?))
    }
}

/// A fork to make (`Procedure::fork`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForkRequest {
    pub parent: String,
    pub child: String,
    pub minute: i64,
    /// The EffectProgram's JSON text, as approved.
    pub program: String,
    pub approved_program_hash: String,
    /// The state the preview saw: the fork is refused where the parent's is not this at the minute.
    pub state_hash: Option<String>,
}

/// A Run from minute 0: the exported world `export` (or the world `world` of a file of named
/// worlds, as the fixtures hold them), saved every `save_every` minutes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Start {
    pub export: Vec<u8>,
    pub world: Option<String>,
    pub save_every: u64,
}

/// The record a computation from the start keeps for its branch: its lineage.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lineage {
    pub parent_branch_id: Option<String>,
    pub fork_minute: i64,
}

/// The writer's item being written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Writing {
    Idle,
    /// A piece handed out (`Step::WritePiece`), its place awaited.
    Piece,
    /// A line handed out (`Step::AppendLine`).
    Line,
}

/// What a seek goes on to once the Run is restored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Then {
    GoOn,
    Fork,
}

/// Where a seek is.
#[derive(Debug)]
enum Seeking {
    Export,
    Point,
    Restore(usize),
    Tape(usize),
}

/// Where a procedure is.
#[derive(Debug)]
enum Phase {
    // go on
    GoRecord,
    GoBranches,
    GoChildren,
    GoJournal,
    GoCut,
    GoReady,
    GoTicked,
    GoStop,
    GoCommitted(Option<&'static str>),
    // the chain and the seek (go on, fork)
    Parents(Then),
    Seek(Then, Seeking),
    // fork
    ForkChild,
    ForkParent,
    ForkJournal,
    ForkCreated,
    ForkStarted,
    ForkSaved,
    ForkRecorded,
    // keep a turn
    TurnJournal,
    TurnCut,
    TurnWritten,
    // start
    StartRecord,
    StartCreated,
    StartWritten,
    StartRecorded,
    // from the start
    FromRecord,
    FromRunning,
    FromCreated,
    FromReady,
    FromTicked { last: bool },
    FromEnded,
    FromRecorded(&'static str),
    FromFailed,
    // recover
    RecoverProjects,
    RecoverRuns,
    RecoverBranches,
    RecoverLock,
    RecoverRecord,
    RecoverWritten,
    RecoverRunStatus,
    RecoverUnlocked,
    // set a status
    Status,
    Done,
}

/// A procedure of the stored Runs: what its host does next (`next`), what the host gives back
/// (`give`), the ticks (`advance`, `stop`) and its outcome. See the module's documentation.
pub struct Procedure {
    phase: Phase,
    step: Option<Step>,
    given: Option<Given>,
    broken: bool,
    /// The branch the procedure is about (a fork's child).
    at: Where,
    // ---- writing
    writer: RunWriter,
    writing: Writing,
    line: Vec<u8>,
    line_head: String,
    last_line: Vec<u8>,
    last_head: Option<String>,
    flushing: bool,
    // ---- reading
    /// The branch whose stored Run is read (a fork's parent), its journal, its chain and the branch
    /// of each level of the chain (the Run from minute 0's first).
    reading: String,
    journal_bytes: Vec<u8>,
    chain: Option<Chain>,
    levels: Vec<String>,
    seek: Option<crate::run_store::Seek>,
    seek_minute: i64,
    export: Vec<u8>,
    restore: Option<Restore>,
    tape_positions: Vec<usize>,
    tape: Vec<Vec<u8>>,
    sought_from: i64,
    sought_base: usize,
    // ---- the Run
    run: Option<PlainRun>,
    // ---- going on
    record: BranchRecord,
    target: i64,
    duration: i64,
    save_every: u64,
    head: Option<Head>,
    head_minute: i64,
    head_hash: String,
    children: VecDeque<String>,
    kept_through: Option<usize>,
    status: Option<&'static str>,
    // ---- fork
    fork: Option<ForkRequest>,
    parent_head: Option<String>,
    receipt: String,
    state_hash: String,
    // ---- a turn
    turn: Vec<u8>,
    // ---- from the start
    start: Option<Start>,
    versions: Option<Versions>,
    lineage: Option<Lineage>,
    marked_running: bool,
    // ---- recover
    projects: VecDeque<String>,
    runs: VecDeque<String>,
    branches: VecDeque<String>,
    project: String,
    run_id: String,
    recovered: Vec<Where>,
}

impl Procedure {
    fn new(phase: Phase, step: Step, at: Where) -> Self {
        Self {
            phase,
            step: Some(step),
            given: None,
            broken: false,
            reading: at.branch.clone(),
            at,
            writer: RunWriter::new(),
            writing: Writing::Idle,
            line: Vec::new(),
            line_head: String::new(),
            last_line: Vec::new(),
            last_head: None,
            flushing: false,
            journal_bytes: Vec::new(),
            chain: None,
            levels: Vec::new(),
            seek: None,
            seek_minute: 0,
            export: Vec::new(),
            restore: None,
            tape_positions: Vec::new(),
            tape: Vec::new(),
            sought_from: 0,
            sought_base: 0,
            run: None,
            record: BranchRecord::default(),
            target: 0,
            duration: 0,
            save_every: 0,
            head: None,
            head_minute: 0,
            head_hash: String::new(),
            children: VecDeque::new(),
            kept_through: None,
            status: None,
            fork: None,
            parent_head: None,
            receipt: String::new(),
            state_hash: String::new(),
            turn: Vec::new(),
            start: None,
            versions: None,
            lineage: None,
            marked_running: false,
            projects: VecDeque::new(),
            runs: VecDeque::new(),
            branches: VecDeque::new(),
            project: String::new(),
            run_id: String::new(),
            recovered: Vec::new(),
        }
    }

    /// Goes on with the branch `at` from its record's head toward `target`.
    pub fn go_on(at: Where, target: i64) -> Self {
        let step = Step::BranchRecord { at: at.clone(), optional: false };
        let mut procedure = Self::new(Phase::GoRecord, step, at);
        procedure.target = target;
        procedure
    }

    /// Makes the branch `request.child` of `request.parent` of the Run `run` of `project`.
    pub fn fork(project: &str, run: &str, request: ForkRequest) -> Self {
        let at = Where::new(project, run, &request.child);
        let step = Step::BranchRecord { at: at.clone(), optional: true };
        let mut procedure = Self::new(Phase::ForkChild, step, at);
        procedure.reading = request.parent.clone();
        procedure.fork = Some(request);
        procedure
    }

    /// Keeps a question's turn (its JSON text) after the "end" line of the branch `at`.
    pub fn keep_turn(at: Where, turn: Vec<u8>) -> Self {
        let step = Step::Journal(at.clone());
        let mut procedure = Self::new(Phase::TurnJournal, step, at);
        procedure.turn = turn;
        procedure
    }

    /// Starts the stored Run of the branch `at` at minute 0 of `start`, recording the Run's model
    /// versions (its record's), and writes the branch's record at that head without a status.
    pub fn start(at: Where, start: Start) -> Self {
        let step = Step::RunRecord { project: at.project.clone(), run: at.run.clone() };
        let mut procedure = Self::new(Phase::StartRecord, step, at);
        procedure.start = Some(start);
        procedure
    }

    /// Computes the Run of `start` from minute 0 to its end, `Step::Compute` asking for each chunk,
    /// into the branch `at`'s files: recording `versions` (the Run's record's where None), with
    /// the branch's record (`running`, then at the end line, `completed`) where `lineage` is given.
    pub fn from_start(at: Where, start: Start, versions: Option<Versions>, lineage: Option<Lineage>) -> Self {
        let mut procedure = Self::new(Phase::FromRecord, Step::Done, at);
        procedure.start = Some(start);
        procedure.versions = versions;
        procedure.lineage = lineage;
        procedure.step = None;
        procedure
    }

    /// Recovers the branches left `running` that no writer holds.
    pub fn recover() -> Self {
        Self::new(Phase::RecoverProjects, Step::Projects, Where::default())
    }

    /// Sets the branch `at`'s status, and its Run's.
    pub fn set_status(at: Where, status: &str) -> Self {
        let step = Step::SetStatus { at: at.clone(), status: status.to_owned() };
        Self::new(Phase::Status, step, at)
    }

    // ---- The host's side ---------------------------------------------------------------------

    /// What the host does next. The same step until it is given what it asked for.
    pub fn next(&mut self) -> Answer<Step> {
        if self.broken {
            return Err(error("the procedure stopped at an error"));
        }
        if self.step.is_none() {
            if let Err(failure) = self.proceed() {
                self.broken = true;
                return Err(failure);
            }
        }
        Ok(self.step.clone().expect("a step after proceeding"))
    }

    /// What the host did of the step `next` gave.
    pub fn give(&mut self, given: Given) -> Answer<()> {
        if self.broken {
            return Err(error("the procedure stopped at an error"));
        }
        match &self.step {
            None => return Err(error("the procedure was given what no step asked for")),
            Some(Step::Compute) => return Err(error("a computation's step is `advance` or `stop`")),
            Some(Step::Done) => return Err(error("the procedure is done")),
            Some(_) => {}
        }
        self.step = None;
        self.given = Some(given);
        Ok(())
    }

    /// Whether the step given out may be given `Given::Failed`.
    pub fn may_fail(&self) -> bool {
        matches!(self.step, Some(Step::Runs { .. } | Step::Branches { .. })) && matches!(self.phase, Phase::RecoverRuns | Phase::RecoverBranches)
    }

    /// The bytes of the piece `Step::WritePiece` asks to write, before compression.
    pub fn raw(&self) -> &[u8] {
        match self.writer.next() {
            Some(Next::Piece { raw, .. }) if self.writing == Writing::Piece => raw,
            _ => &[],
        }
    }

    /// The line `Step::AppendLine` asks to append.
    pub fn line(&self) -> &[u8] {
        &self.line
    }

    /// At `Step::Compute`: at most `ticks` ticks (at least one), not past the next saved point nor
    /// the target. They run here, and the steps after them store what they made.
    pub fn advance(&mut self, ticks: u64) -> Answer<u64> {
        if self.broken || self.step != Some(Step::Compute) {
            return Err(error("the procedure does not compute now"));
        }
        let result = self.ticks(ticks);
        if result.is_err() {
            self.broken = true;
        }
        result
    }

    fn ticks(&mut self, ticks: u64) -> Answer<u64> {
        let schedule = Schedule { every: self.save_every };
        let run = self.run.as_mut().expect("a Run computing");
        let minute = run.minute()?;
        let going_on = !matches!(self.phase, Phase::FromReady);
        let step = schedule.step(minute, ticks.max(1), going_on.then_some(self.target));
        let ran = run.advance(step).map_err(|stopped| refused(stopped.to_string()))?;
        if going_on {
            if ran != step {
                return Err(refused(format!("the Run stopped at minute {}", run.minute()?)));
            }
            self.phase = Phase::GoTicked;
        } else {
            self.phase = Phase::FromTicked { last: ran < step };
        }
        self.step = None;
        Ok(ran)
    }

    /// At `Step::Compute` of `go_on`: ends the computation where it got to (a cancel), with a saved
    /// point there where it is past the head.
    pub fn stop(&mut self) -> Answer<()> {
        if self.broken || self.step != Some(Step::Compute) || !matches!(self.phase, Phase::GoReady) {
            return Err(error("the procedure does not compute now"));
        }
        self.phase = Phase::GoStop;
        self.step = None;
        Ok(())
    }

    /// After an error of a computation from the start that wrote its branch's record `running`:
    /// the step that marks the branch failed (its record without a head), then `Done`. False where
    /// there is nothing to mark.
    pub fn fail(&mut self) -> bool {
        let Some(lineage) = self.lineage.clone().filter(|_| self.marked_running) else { return false };
        self.broken = false;
        self.writing = Writing::Idle;
        self.flushing = false;
        self.given = None;
        self.phase = Phase::FromFailed;
        self.step = Some(self.write_branch(BranchRecord {
            branch_id: self.at.branch.clone(),
            parent_branch_id: lineage.parent_branch_id,
            fork_minute: lineage.fork_minute,
            status: Some("failed".to_owned()),
            ..BranchRecord::default()
        }));
        true
    }

    // ---- The outcome ---------------------------------------------------------------------------

    /// The Run where the procedure got to (going on, a fork's child, from the start).
    pub fn run(&self) -> Option<&PlainRun> {
        self.run.as_ref()
    }

    pub fn run_mut(&mut self) -> Option<&mut PlainRun> {
        self.run.as_mut()
    }

    /// The branch the procedure is about.
    pub fn at(&self) -> &Where {
        &self.at
    }

    /// A computation's status once it is done: `completed` at the clock's end, `paused` at the
    /// target or stopped before it.
    pub fn status(&self) -> Option<&'static str> {
        self.status
    }

    /// The minute a computation's target is.
    pub fn target(&self) -> i64 {
        self.target
    }

    /// The minute the Run's clock is complete at.
    pub fn duration(&self) -> i64 {
        self.duration
    }

    /// The branch's head now: its minute and hash.
    pub fn head(&self) -> (i64, &str) {
        (self.head_minute, &self.head_hash)
    }

    /// The head hash of the last line written, and the line.
    pub fn last_head(&self) -> Option<&str> {
        self.last_head.as_deref()
    }

    pub fn last_line(&self) -> &[u8] {
        &self.last_line
    }

    /// The saved points' lines the procedure wrote.
    pub fn saved(&self) -> &[crate::run_store::SavedLine] {
        self.writer.saved()
    }

    /// A fork's receipt (its JSON text) and the child's state hash at the minute.
    pub fn receipt(&self) -> &str {
        &self.receipt
    }

    pub fn state_hash(&self) -> &str {
        &self.state_hash
    }

    /// The branches `recover` made `interrupted`.
    pub fn recovered(&self) -> &[Where] {
        &self.recovered
    }

    /// `recovered` as a JSON list of "project/run/branch".
    pub fn recovered_json(&self) -> String {
        Value::from(self.recovered.iter().map(|at| format!("{}/{}/{}", at.project, at.run, at.branch)).collect::<Vec<_>>()).to_string()
    }

    // ---- The steps ---------------------------------------------------------------------------

    /// Runs the procedure from what it was given to the next step the host does.
    fn proceed(&mut self) -> Answer<()> {
        let mut given = self.given.take();
        loop {
            match self.writing {
                Writing::Piece => {
                    let Some(Given::Place { offset, length }) = given.take() else {
                        return Err(error("a piece written is given where it was put"));
                    };
                    let encoded = self.writer.commit_piece(offset, length)?;
                    self.line = encoded.bytes;
                    self.line_head = encoded.head;
                    self.writing = Writing::Line;
                    self.step = Some(Step::AppendLine(self.at.clone()));
                    return Ok(());
                }
                Writing::Line => {
                    given = None;
                    self.last_head = Some(std::mem::take(&mut self.line_head));
                    self.last_line = std::mem::take(&mut self.line);
                    self.writing = Writing::Idle;
                }
                Writing::Idle => {}
            }
            if self.flushing {
                match self.writer.next() {
                    Some(Next::Line) => {
                        let encoded = self.writer.commit_line()?;
                        self.line = encoded.bytes;
                        self.line_head = encoded.head;
                        self.writing = Writing::Line;
                        self.step = Some(Step::AppendLine(self.at.clone()));
                        return Ok(());
                    }
                    Some(Next::Piece { kind, .. }) => {
                        self.writing = Writing::Piece;
                        self.step = Some(Step::WritePiece { at: self.at.clone(), kind });
                        return Ok(());
                    }
                    Some(Next::Copy { kind, .. }) => return Err(error(format!("copy:{kind}: a procedure copies no piece"))),
                    None => self.flushing = false,
                }
            }
            if let Some(step) = self.phase_step(given.take())? {
                self.step = Some(step);
                return Ok(());
            }
        }
    }

    /// Writes what the writer queued before the procedure goes on (`proceed`).
    fn flush(&mut self) {
        self.flushing = true;
    }

    fn run_ref(&mut self) -> &mut PlainRun {
        self.run.as_mut().expect("the Run")
    }

    fn chain(&self) -> &Chain {
        self.chain.as_ref().expect("the chain")
    }

    /// The bytes given for a read step.
    fn text(given: Option<Given>) -> Answer<Option<Vec<u8>>> {
        match given {
            Some(Given::Text(bytes)) => Ok(bytes),
            other => Err(error(format!("a read is given its bytes, not {other:?}"))),
        }
    }

    fn nothing(given: Option<Given>) -> Answer<()> {
        match given {
            Some(Given::Nothing) | None => Ok(()),
            other => Err(error(format!("a write is given nothing back, not {other:?}"))),
        }
    }

    fn write_branch(&self, record: BranchRecord) -> Step {
        Step::WriteBranch { project: self.at.project.clone(), run: self.at.run.clone(), record }
    }

    /// The procedure's phase from what it was given: the next step, or None to go on (after the
    /// writer's queue is written).
    fn phase_step(&mut self, given: Option<Given>) -> Answer<Option<Step>> {
        match std::mem::replace(&mut self.phase, Phase::Done) {
            // ---- go on
            Phase::GoRecord => {
                let bytes = Self::text(given)?.ok_or_else(|| refused("saved artifact is missing or corrupt"))?;
                self.record = BranchRecord::of_json(&bytes)?;
                if self.record.checkpoint_hash.is_none() {
                    return Err(refused("branch has no computed minutes"));
                }
                self.phase = Phase::GoBranches;
                Ok(Some(Step::Branches { project: self.at.project.clone(), run: self.at.run.clone() }))
            }
            Phase::GoBranches => {
                let Some(Given::Texts(records)) = given else { return Err(error("the branches are given as their records")) };
                for bytes in records {
                    let record: Value = serde_json::from_slice(&bytes).map_err(|_| refused("saved artifact is missing or corrupt"))?;
                    if record.get("parent_branch_id").and_then(Value::as_str) != Some(self.at.branch.as_str()) {
                        continue;
                    }
                    let child = record.get("branch_id").and_then(Value::as_str).ok_or_else(|| refused("invalid artifact identifier"))?;
                    self.children.push_back(child.to_owned());
                }
                Ok(Some(self.next_child()))
            }
            Phase::GoChildren => {
                // The latest line of the branch's stored Run a branch forked from it reads up to.
                if let Some(bytes) = Self::text(given)?.filter(|bytes| !bytes.is_empty()) {
                    if let Some(fork) = Journal::parse(&bytes)?.fork() {
                        let base = fork.base.line as usize;
                        self.kept_through = Some(self.kept_through.map_or(base, |kept| kept.max(base)));
                    }
                }
                Ok(Some(self.next_child()))
            }
            Phase::GoJournal => {
                self.journal_bytes = Self::text(given)?.unwrap_or_default();
                let journal = Journal::parse(&self.journal_bytes)?;
                let hash = self.record.checkpoint_hash.clone().expect("checked");
                let cut = journal.cut(&hash, self.kept_through)?;
                self.writer = RunWriter::after(&journal, cut.head.index)?;
                self.head = Some(cut.head);
                self.head_hash = hash;
                self.journal_bytes.truncate(cut.journal_end as usize);
                self.phase = Phase::GoCut;
                Ok(Some(Step::Truncate { at: self.at.clone(), journal: cut.journal_end, data: Some(cut.data_end) }))
            }
            Phase::GoCut => {
                Self::nothing(given)?;
                self.open(Then::GoOn)
            }
            Phase::GoReady => {
                self.phase = Phase::GoReady;
                Ok(Some(Step::Compute))
            }
            Phase::GoTicked => {
                let schedule = Schedule { every: self.save_every };
                let (target, duration) = (self.target, self.duration);
                let mut run = self.run.take().expect("the Run");
                let queued = Self::queue_chunk(&mut self.writer, &mut run, schedule, target, duration);
                self.run = Some(run);
                self.phase = match queued? {
                    Some(status) => Phase::GoCommitted(status),
                    None => Phase::GoReady,
                };
                self.flush();
                Ok(None)
            }
            Phase::GoStop => {
                let minute = self.run_ref().minute()?;
                let status = if minute == self.duration { "completed" } else { "paused" };
                if minute > self.head_minute {
                    let mut run = self.run.take().expect("the Run");
                    let saved = self.writer.save(&mut run);
                    self.run = Some(run);
                    saved?;
                    self.phase = Phase::GoCommitted(Some(status));
                    self.flush();
                    return Ok(None);
                }
                Ok(Some(self.done(Some(status))))
            }
            Phase::GoCommitted(status) => {
                if given.is_some() {
                    // The record is written: the computation goes on, or is done.
                    Self::nothing(given)?;
                    return Ok(Some(match status {
                        Some(status) => self.done(Some(status)),
                        None => {
                            self.phase = Phase::GoReady;
                            Step::Compute
                        }
                    }));
                }
                // The lines are written: the branch's record moves to the last.
                let minute = self.run_ref().minute()?;
                let head = self.last_head.clone().ok_or_else(|| error("a saved point wrote no line"))?;
                let record =
                    BranchRecord { computed_until: Some(minute), checkpoint_hash: Some(head.clone()), status: Some(RUNNING.to_owned()), ..self.record.clone() };
                self.head_minute = minute;
                self.head_hash = head;
                self.phase = Phase::GoCommitted(status);
                Ok(Some(self.write_branch(record)))
            }
            // ---- the chain and the seek
            Phase::Parents(then) => {
                let parent = self.chain().parent_needed().expect("a parent asked for").to_owned();
                let bytes = Self::text(given)?.ok_or_else(|| error(format!("the stored Run of a fork needs its parent's (branch {parent})")))?;
                self.chain.as_mut().expect("the chain").add_parent(Journal::parse(&bytes)?)?;
                self.levels.insert(0, parent);
                self.parents_or_seek(then)
            }
            Phase::Seek(then, seeking) => self.seek_step(then, seeking, given),
            // ---- fork
            Phase::ForkChild => {
                if Self::text(given)?.is_some() {
                    return Err(refused("child branch already exists"));
                }
                self.phase = Phase::ForkParent;
                Ok(Some(Step::BranchRecord { at: self.at.of(&self.reading), optional: false }))
            }
            Phase::ForkParent => {
                let bytes = Self::text(given)?.ok_or_else(|| refused("saved artifact is missing or corrupt"))?;
                let parent = BranchRecord::of_json(&bytes)?;
                self.parent_head = parent.checkpoint_hash;
                self.phase = Phase::ForkJournal;
                Ok(Some(Step::Journal(self.at.of(&self.reading))))
            }
            Phase::ForkJournal => {
                self.journal_bytes = Self::text(given)?.unwrap_or_default();
                self.open(Then::Fork)
            }
            Phase::ForkCreated => {
                Self::nothing(given)?;
                let request = self.fork.clone().expect("a fork");
                let fork = Fork {
                    parent_branch_id: request.parent.clone(),
                    minute: request.minute,
                    origin: ForkOrigin {
                        branch_id: request.parent.clone(),
                        minute: request.minute,
                        state_hash: std::mem::take(&mut self.state_hash),
                        checkpoint_hash: self.parent_head.clone(),
                    },
                    program: request.program,
                    approved_program_hash: request.approved_program_hash,
                    receipt: self.receipt.clone(),
                };
                self.writer = RunWriter::new();
                let parent = self.chain.as_ref().expect("the chain").leaf();
                self.writer.fork_start(parent, self.sought_base, fork)?;
                self.phase = Phase::ForkStarted;
                self.flush();
                Ok(None)
            }
            Phase::ForkStarted => {
                let mut run = self.run.take().expect("the Run");
                let saved = self.writer.save(&mut run);
                self.run = Some(run);
                saved?;
                self.phase = Phase::ForkSaved;
                self.flush();
                Ok(None)
            }
            Phase::ForkSaved => {
                let request = self.fork.as_ref().expect("a fork");
                let head = self.last_head.clone().ok_or_else(|| error("a saved point wrote no line"))?;
                let record = BranchRecord {
                    branch_id: request.child.clone(),
                    parent_branch_id: Some(request.parent.clone()),
                    fork_minute: request.minute,
                    computed_until: Some(request.minute),
                    checkpoint_hash: Some(head),
                    status: Some("paused".to_owned()),
                };
                self.phase = Phase::ForkRecorded;
                Ok(Some(self.write_branch(record)))
            }
            Phase::ForkRecorded => {
                Self::nothing(given)?;
                self.state_hash = self.run_ref().state_digest()?;
                Ok(Some(self.done(None)))
            }
            // ---- a question's turn
            Phase::TurnJournal => {
                let bytes = Self::text(given)?.unwrap_or_default();
                let journal = Journal::parse(&bytes)?;
                self.writer = RunWriter::after_end(&journal)?;
                let end = journal.entries().last().map_or(0, |entry| entry.end);
                self.phase = Phase::TurnCut;
                if bytes.len() as u64 > end {
                    // A line a writer left cut short after the last whole one goes first.
                    return Ok(Some(Step::Truncate { at: self.at.clone(), journal: end, data: None }));
                }
                Ok(None)
            }
            Phase::TurnCut => {
                Self::nothing(given)?;
                let turn = std::mem::take(&mut self.turn);
                self.writer.turns(&[turn])?;
                self.phase = Phase::TurnWritten;
                self.flush();
                Ok(None)
            }
            Phase::TurnWritten => Ok(Some(self.done(None))),
            // ---- the start
            Phase::StartRecord => {
                let bytes = Self::text(given)?.ok_or_else(|| refused("saved artifact is missing or corrupt"))?;
                self.versions = Some(Self::versions_of_record(&bytes)?);
                self.phase = Phase::StartCreated;
                Ok(Some(Step::Create(self.at.clone())))
            }
            Phase::StartCreated => {
                Self::nothing(given)?;
                let start = self.start.clone().expect("a start");
                let run = plain_run(&start.export, start.world.as_deref())?;
                let storing =
                    Storing { model_versions: self.versions.clone().expect("read"), compression: COMPRESSION.to_owned(), save_every: start.save_every };
                self.writer.start(&run, start.export, start.world, &storing)?;
                self.phase = Phase::StartWritten;
                self.flush();
                Ok(None)
            }
            Phase::StartWritten => {
                let head = self.last_head.clone().ok_or_else(|| error("the start wrote no line"))?;
                self.head_hash = head.clone();
                let record = BranchRecord { branch_id: self.at.branch.clone(), checkpoint_hash: Some(head), ..BranchRecord::default() };
                self.phase = Phase::StartRecorded;
                Ok(Some(self.write_branch(record)))
            }
            Phase::StartRecorded => {
                Self::nothing(given)?;
                Ok(Some(self.done(None)))
            }
            // ---- from the start
            Phase::FromRecord => {
                if let Some(given) = given {
                    let bytes = Self::text(Some(given))?.ok_or_else(|| refused("saved artifact is missing or corrupt"))?;
                    self.versions = Some(Self::versions_of_record(&bytes)?);
                }
                if self.versions.is_none() {
                    self.phase = Phase::FromRecord;
                    return Ok(Some(Step::RunRecord { project: self.at.project.clone(), run: self.at.run.clone() }));
                }
                match self.lineage.clone() {
                    Some(lineage) => {
                        self.phase = Phase::FromRunning;
                        Ok(Some(self.write_branch(BranchRecord {
                            branch_id: self.at.branch.clone(),
                            parent_branch_id: lineage.parent_branch_id,
                            fork_minute: lineage.fork_minute,
                            status: Some(RUNNING.to_owned()),
                            ..BranchRecord::default()
                        })))
                    }
                    None => self.from_create(),
                }
            }
            Phase::FromRunning => {
                Self::nothing(given)?;
                self.marked_running = true;
                self.from_create()
            }
            Phase::FromCreated => {
                Self::nothing(given)?;
                let start = self.start.take().expect("a start");
                let storing =
                    Storing { model_versions: self.versions.clone().expect("read"), compression: COMPRESSION.to_owned(), save_every: start.save_every };
                let run = self.run.as_ref().expect("the Run");
                self.writer.start(run, start.export, start.world, &storing)?;
                self.phase = Phase::FromReady;
                self.flush();
                Ok(None)
            }
            Phase::FromReady => {
                self.phase = Phase::FromReady;
                Ok(Some(Step::Compute))
            }
            Phase::FromTicked { last } => {
                let schedule = Schedule { every: self.save_every };
                let duration = self.duration;
                let mut run = self.run.take().expect("the Run");
                let queued = (|| -> Answer<bool> {
                    self.writer.take(&mut run, false)?;
                    if last {
                        self.writer.take(&mut run, true)?;
                        if schedule.every > 0 {
                            self.writer.save(&mut run)?;
                        }
                        self.writer.finish(&mut run)?;
                        return Ok(true);
                    }
                    if schedule.due(run.minute()?, duration) {
                        self.writer.save(&mut run)?;
                    }
                    Ok(false)
                })();
                self.run = Some(run);
                self.phase = if queued? { Phase::FromEnded } else { Phase::FromReady };
                self.flush();
                Ok(None)
            }
            Phase::FromEnded => {
                let minute = self.run_ref().minute()?;
                let head = self.last_head.clone().ok_or_else(|| error("the end wrote no line"))?;
                self.head_minute = minute;
                self.head_hash = head.clone();
                let status = if minute >= self.duration { "completed" } else { "paused" };
                let Some(lineage) = self.lineage.clone() else { return Ok(Some(self.done(Some(status)))) };
                self.phase = Phase::FromRecorded(status);
                Ok(Some(self.write_branch(BranchRecord {
                    branch_id: self.at.branch.clone(),
                    parent_branch_id: lineage.parent_branch_id,
                    fork_minute: lineage.fork_minute,
                    computed_until: Some(minute),
                    checkpoint_hash: Some(head),
                    status: Some(status.to_owned()),
                })))
            }
            Phase::FromRecorded(status) => {
                Self::nothing(given)?;
                Ok(Some(self.done(Some(status))))
            }
            Phase::FromFailed => {
                Self::nothing(given)?;
                Ok(Some(self.done(Some("failed"))))
            }
            // ---- recover
            Phase::RecoverProjects => {
                let Some(Given::Ids(ids)) = given else { return Err(error("the Projects are given as their ids")) };
                self.projects = ids.into();
                Ok(Some(self.next_project()))
            }
            Phase::RecoverRuns => {
                match given {
                    Some(Given::Ids(ids)) => self.runs = ids.into(),
                    Some(Given::Failed) => self.runs.clear(),
                    _ => return Err(error("the Runs are given as their ids")),
                }
                Ok(Some(self.next_run()))
            }
            Phase::RecoverBranches => {
                self.branches.clear();
                match given {
                    Some(Given::Texts(records)) => {
                        for bytes in records {
                            let Ok(record) = serde_json::from_slice::<Value>(&bytes) else { continue };
                            if record.get("status").and_then(Value::as_str) != Some(RUNNING) {
                                continue;
                            }
                            if let Some(id) = record.get("branch_id").and_then(Value::as_str) {
                                self.branches.push_back(id.to_owned());
                            }
                        }
                    }
                    Some(Given::Failed) => {}
                    _ => return Err(error("the branches are given as their records")),
                }
                Ok(Some(self.next_branch()))
            }
            Phase::RecoverLock => {
                let Some(Given::Lock(taken)) = given else { return Err(error("a lock is given as whether it was taken")) };
                if !taken {
                    return Ok(Some(self.next_branch()));
                }
                self.phase = Phase::RecoverRecord;
                Ok(Some(Step::BranchRecord { at: self.at.clone(), optional: false }))
            }
            Phase::RecoverRecord => {
                let bytes = Self::text(given)?.ok_or_else(|| refused("saved artifact is missing or corrupt"))?;
                let record = BranchRecord::of_json(&bytes)?;
                if record.status.as_deref() != Some(RUNNING) {
                    self.phase = Phase::RecoverUnlocked;
                    return Ok(Some(Step::Unlock(self.at.clone())));
                }
                self.phase = Phase::RecoverWritten;
                Ok(Some(self.write_branch(BranchRecord { status: Some(INTERRUPTED.to_owned()), ..record })))
            }
            Phase::RecoverWritten => {
                Self::nothing(given)?;
                self.phase = Phase::RecoverRunStatus;
                Ok(Some(Step::SetRunStatus { project: self.at.project.clone(), run: self.at.run.clone(), status: INTERRUPTED.to_owned() }))
            }
            Phase::RecoverRunStatus => {
                Self::nothing(given)?;
                self.recovered.push(self.at.clone());
                self.phase = Phase::RecoverUnlocked;
                Ok(Some(Step::Unlock(self.at.clone())))
            }
            Phase::RecoverUnlocked => {
                Self::nothing(given)?;
                Ok(Some(self.next_branch()))
            }
            // ---- a status
            Phase::Status => {
                Self::nothing(given)?;
                Ok(Some(self.done(None)))
            }
            Phase::Done => {
                self.phase = Phase::Done;
                Ok(Some(Step::Done))
            }
        }
    }

    /// Done, with a computation's status.
    fn done(&mut self, status: Option<&'static str>) -> Step {
        if status.is_some() {
            self.status = status;
        }
        self.phase = Phase::Done;
        Step::Done
    }

    /// The model versions of a Run's record (its `model_versions`).
    fn versions_of_record(bytes: &[u8]) -> Answer<Versions> {
        let record: Value = serde_json::from_slice(bytes).map_err(|_| refused("saved artifact is missing or corrupt"))?;
        Ok(versions_of_json(record.get("model_versions").unwrap_or(&Value::Null).to_string().as_bytes())?)
    }

    /// The writes of a chunk of `go_on` that ended at the Run's minute: the blocks sealed since, and
    /// at the clock's end, the target or a saved point's minute a saved point (at the end the
    /// "world" and "end" lines too). Some(the status once it ends) where the branch's record moves.
    fn queue_chunk(writer: &mut RunWriter, run: &mut PlainRun, schedule: Schedule, target: i64, duration: i64) -> Answer<Option<Option<&'static str>>> {
        let minute = run.minute()?;
        writer.take(run, false)?;
        if minute == duration {
            writer.save(run)?;
            writer.finish(run)?;
            return Ok(Some(Some("completed")));
        }
        if minute == target || schedule.is_point(minute) {
            writer.save(run)?;
            return Ok(Some((minute == target).then_some("paused")));
        }
        Ok(None)
    }

    /// The journal of the next branch forked from the one going on, or the branch's own.
    fn next_child(&mut self) -> Step {
        match self.children.pop_front() {
            Some(child) => {
                self.phase = Phase::GoChildren;
                Step::Journal(self.at.of(&child))
            }
            None => {
                self.phase = Phase::GoJournal;
                Step::Journal(self.at.clone())
            }
        }
    }

    /// Opens the chain of the stored Run of `reading` from its journal's bytes, then reads its
    /// parents' journals and seeks.
    fn open(&mut self, then: Then) -> Answer<Option<Step>> {
        let leaf = Journal::parse(&std::mem::take(&mut self.journal_bytes))?;
        let compression = &leaf.run()?.compression;
        if compression != COMPRESSION {
            return Err(refused(format!("the stored Run's pieces are compressed with {compression}, which this host does not read")));
        }
        self.levels = vec![self.reading.clone()];
        self.chain = Some(Chain::new(leaf)?);
        self.parents_or_seek(then)
    }

    fn parents_or_seek(&mut self, then: Then) -> Answer<Option<Step>> {
        if let Some(parent) = self.chain().parent_needed().map(str::to_owned) {
            self.phase = Phase::Parents(then);
            return Ok(Some(Step::Journal(self.at.of(&parent))));
        }
        let minute = match then {
            Then::GoOn => {
                let leaf = self.chain().leaf();
                let duration = leaf.duration()?;
                let save_every = leaf.run()?.save_every;
                let computed = self.record.computed_until.unwrap_or(self.record.fork_minute);
                if !(computed < self.target && self.target <= duration) {
                    return Err(refused(OUT_OF_RANGE));
                }
                self.duration = duration;
                self.save_every = save_every;
                self.head.expect("the head").minute
            }
            Then::Fork => self.fork.as_ref().expect("a fork").minute,
        };
        self.seek_minute = minute;
        let seek = self.chain().seek(minute)?;
        let export = self.chain().export()?;
        self.seek = Some(seek);
        self.phase = Phase::Seek(then, Seeking::Export);
        Ok(Some(self.piece_step(export)?))
    }

    /// The step that reads the piece at `position` of the chain.
    fn piece_step(&self, position: usize) -> Answer<Step> {
        let (level, stored) = self.chain().stored(position)?;
        Ok(Step::Piece { at: self.at.of(&self.levels[level]), stored: stored.clone() })
    }

    /// The seek of the stored Run read (`StoredRun::seek_with` of the store, `RunReader.seek` of the
    /// browser): the nearest saved point at or before the minute restored with the stored log and
    /// samples, or the exported world at minute 0; the ticks to the
    /// minute answered from the LLM tape where the Run kept one.
    fn seek_step(&mut self, then: Then, seeking: Seeking, given: Option<Given>) -> Answer<Option<Step>> {
        let raw = Self::text(given)?.ok_or_else(|| error("a piece read is given its bytes"))?;
        let seek = self.seek.clone().expect("a seek");
        match seeking {
            Seeking::Export => {
                self.chain().check_raw(seek.export, &raw)?;
                self.export = raw;
                match seek.from {
                    Some(point) => {
                        let stored = self.chain().leaf().piece(point)?.stored.clone();
                        self.phase = Phase::Seek(then, Seeking::Point);
                        Ok(Some(Step::Piece { at: self.at.of(&self.reading), stored }))
                    }
                    None => {
                        let world = self.chain().leaf().run()?.world.clone();
                        self.run = Some(plain_run(&self.export, world.as_deref())?);
                        self.restored(then)
                    }
                }
            }
            Seeking::Point => {
                let point = seek.from.expect("a saved point");
                self.chain().leaf().check_raw(point, &raw)?;
                self.restore = Some(Restore::new(&raw)?);
                self.next_restore(then, 0)
            }
            Seeking::Restore(position) => {
                let chain = self.chain.as_ref().expect("the chain");
                chain.restore_read(position, &raw, self.restore.as_mut().expect("a restore"))?;
                self.next_restore(then, position + 1)
            }
            Seeking::Tape(index) => {
                let position = self.tape_positions[index];
                self.chain().check_raw(position, &raw)?;
                self.tape.extend(records::read(&raw)?);
                self.next_tape(then, index + 1)
            }
        }
    }

    fn next_restore(&mut self, then: Then, start: usize) -> Answer<Option<Step>> {
        let next = self.chain().next_restore_read(start, self.restore.as_ref().expect("a restore"))?;
        if let Some(position) = next {
            self.phase = Phase::Seek(then, Seeking::Restore(position));
            return Ok(Some(self.piece_step(position)?));
        }
        let world = self.chain().leaf().run()?.world.clone();
        let entry = match &world {
            Some(name) => PlainRun::entry_of_worlds_json(&self.export, name)?,
            None => PlainRun::entry_of_json(&self.export)?,
        };
        self.run = Some(PlainRun::restore(entry, self.restore.take().expect("a restore"))?);
        self.restored(then)
    }

    /// The Run restored: then the tape read.
    fn restored(&mut self, then: Then) -> Answer<Option<Step>> {
        self.tape_positions = self.chain().records(RecordKind::Tape)?;
        self.tape.clear();
        self.next_tape(then, 0)
    }

    fn next_tape(&mut self, then: Then, index: usize) -> Answer<Option<Step>> {
        if let Some(&position) = self.tape_positions.get(index) {
            self.phase = Phase::Seek(then, Seeking::Tape(index));
            return Ok(Some(self.piece_step(position)?));
        }
        let seek = self.seek.clone().expect("a seek");
        if !self.tape.is_empty() {
            let from = match seek.from {
                Some(point) => self.chain().leaf().saved(point)?.counts.tape as usize,
                None => 0,
            };
            let entries = self.tape.get(from..).ok_or_else(|| refused("the stored Run's LLM tape is shorter than its saved point counts"))?;
            let replay = Replay::new(entries, false)?;
            self.run_ref().set_llm(Box::new(replay), Limits::default())?;
            self.tape.clear();
        }
        let minute = self.seek_minute;
        let run = self.run_ref();
        let ticks = minute - run.minute()?;
        let ran = if ticks < 0 { None } else { run.advance(ticks as u64).ok() };
        if ran != Some(ticks as u64) {
            return Err(refused(format!("the Run does not reach minute {minute}")));
        }
        self.sought_from = seek.from_minute;
        self.sought_base = seek.base;
        self.export = Vec::new();
        match then {
            Then::GoOn => self.go_sought(),
            Then::Fork => self.fork_sought(),
        }
    }

    /// The Run restored at the branch's head goes on from there: it must hold what the stored Run
    /// holds there.
    fn go_sought(&mut self) -> Answer<Option<Step>> {
        let head = self.head.expect("the head");
        let run = self.run.as_ref().expect("the Run");
        if head.kind == HeadKind::Saved && !(run.minute()? == head.minute && self.sought_from == head.minute && self.writer.fits(run)?) {
            return Err(refused("the Run restored at the branch's head does not hold what its stored Run holds there"));
        }
        self.head_minute = head.minute;
        self.phase = Phase::GoReady;
        Ok(Some(Step::Compute))
    }

    /// The parent's Run at the fork's minute: checked against the state the preview saw, the
    /// program applied, then the child's files made.
    fn fork_sought(&mut self) -> Answer<Option<Step>> {
        let request = self.fork.clone().expect("a fork");
        let run = self.run.as_mut().expect("the Run");
        let origin = run.state_digest()?;
        if request.state_hash.as_deref().is_some_and(|wanted| wanted != origin) {
            return Err(refused("source state changed; preview the intervention again"));
        }
        self.receipt = run.apply_effect_program_json(request.program.as_bytes())?;
        self.state_hash = origin;
        self.phase = Phase::ForkCreated;
        Ok(Some(Step::Create(self.at.clone())))
    }

    /// The Run from minute 0, and its branch's files made.
    fn from_create(&mut self) -> Answer<Option<Step>> {
        let start = self.start.as_ref().expect("a start");
        let run = plain_run(&start.export, start.world.as_deref())?;
        self.duration = run.duration()?;
        self.save_every = start.save_every;
        self.run = Some(run);
        self.phase = Phase::FromCreated;
        Ok(Some(Step::Create(self.at.clone())))
    }

    fn next_project(&mut self) -> Step {
        match self.projects.pop_front() {
            Some(project) => {
                self.project = project.clone();
                self.phase = Phase::RecoverRuns;
                Step::Runs { project }
            }
            None => self.done(None),
        }
    }

    fn next_run(&mut self) -> Step {
        match self.runs.pop_front() {
            Some(run) => {
                self.run_id = run.clone();
                self.phase = Phase::RecoverBranches;
                Step::Branches { project: self.project.clone(), run }
            }
            None => self.next_project(),
        }
    }

    fn next_branch(&mut self) -> Step {
        match self.branches.pop_front() {
            Some(branch) => {
                self.at = Where::new(&self.project, &self.run_id, &branch);
                self.phase = Phase::RecoverLock;
                Step::TryLock(self.at.clone())
            }
            None => self.next_run(),
        }
    }
}

/// A Run of the exported world `export` from minute 0 (of the world `world` of a file of named
/// worlds).
pub fn plain_run(export: &[u8], world: Option<&str>) -> Result<PlainRun, Error> {
    match world {
        Some(name) => PlainRun::of_worlds_json(export, name),
        None => PlainRun::of_json(export),
    }
}
