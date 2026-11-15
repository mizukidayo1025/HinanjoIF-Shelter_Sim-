//! The core's computing procedure (`sheltersim_core::compute`, #476) done with a store's files: the
//! steps a `Procedure` gives are done here (the records with `ProjectFiles`; a branch's stored Run
//! with its folder's two files, each piece compressed, appended and flushed to the disk before its
//! line, each line flushed before the next; the store's limit checked before each write; the locks
//! of the branches' writers), and what they read or where they wrote is given back. The procedure
//! decides everything else, as it does for the browser (`js/procedure.mjs`), so both write the same
//! lines, pieces (before compression) and records.
//!
//! A branch is written by the one writer holding its lease (`ProjectFiles::claim_branch`): each
//! procedure that writes a branch's stored Run checks the lease first. After a step fails the host
//! refuses to go on: the journal may end with a line cut short, which reading ignores and going on
//! from the branch's head cuts off.
//!
//! Going on (`ProjectFiles::go_on`) gives a `Computation`: `advance` runs a chunk's ticks without
//! the files (outside the store's transaction, which the Run's LLM link may take, #465), `store`
//! does the chunk's steps (under it), `stop` ends it early with a saved point where it got to. The
//! caller sets the final status of the branch and the Run (`Computation::end`).

use std::collections::HashMap;
use std::fs::{self, File};
use std::path::PathBuf;

use serde_json::Value;
use sheltersim_core::answers::Failure;
use sheltersim_core::compute::{BranchRecord, Given, Lineage, Procedure, Start, Step, Where};
use sheltersim_core::model_version::Versions;
use sheltersim_core::plain::PlainRun;
use sheltersim_core::run_store::RecordKind;

use crate::compress::{inflate_raw, Compress};
use crate::files::{read_at, read_if_there, sync_dir, AppendFile};
use crate::lock::{self, Lease};
use crate::quota::Quota;
use crate::records::{record_bytes, Branch, ProjectFiles, Record};
use crate::stored::{python_stored, read_journal, FromStart, Sizes, Stored, StoredRun, DATA, JOURNAL};
use crate::{Error, Result};

pub use sheltersim_core::compute::{plain_run, ForkRequest, RUNNING};

impl From<Failure> for Error {
    fn from(failure: Failure) -> Self {
        match failure {
            Failure::Refused(refusal) => Error::Refused(refusal.error),
            Failure::Error(error) => Error::Core(error),
        }
    }
}

fn branch_of(record: &BranchRecord) -> Branch {
    Branch {
        branch_id: record.branch_id.clone(),
        parent_branch_id: record.parent_branch_id.clone(),
        fork_minute: record.fork_minute,
        computed_until: record.computed_until,
        checkpoint_hash: record.checkpoint_hash.clone(),
        status: record.status.clone(),
    }
}

/// Where a host's branches are: a store's folders, or one folder of a stored Run without records.
enum Folders {
    Store(ProjectFiles),
    One(PathBuf),
}

/// The files a procedure's steps are done with: the branches' folders (and a store's records), the
/// limit, the compressor, the files open and the locks taken.
pub struct Host {
    folders: Folders,
    quota: Quota,
    compress: Compress,
    writing: HashMap<PathBuf, AppendFile>,
    reading: HashMap<PathBuf, File>,
    locks: HashMap<PathBuf, Lease>,
    failed: bool,
    /// What was written of each kind of piece.
    pub sizes: Sizes,
}

impl Host {
    /// The host of a store's procedures, its limit measured now.
    pub fn of_store(files: &ProjectFiles) -> Result<Self> {
        Ok(Self::new(Folders::Store(files.clone()), files.quota()?, files.compress()))
    }

    /// The host of the stored Run in `folder`, which keeps no records.
    pub fn of_folder(folder: PathBuf, quota: Quota, compress: Compress) -> Self {
        Self::new(Folders::One(folder), quota, compress)
    }

    fn new(folders: Folders, quota: Quota, compress: Compress) -> Self {
        Self { folders, quota, compress, writing: HashMap::new(), reading: HashMap::new(), locks: HashMap::new(), failed: false, sizes: Sizes::default() }
    }

    fn files(&self) -> Result<&ProjectFiles> {
        match &self.folders {
            Folders::Store(files) => Ok(files),
            Folders::One(_) => Err(Error::refused("a stored Run without a store keeps no records")),
        }
    }

    fn folder(&self, at: &Where) -> Result<PathBuf> {
        match &self.folders {
            Folders::Store(files) => files.branch_folder(&at.project, &at.run, &at.branch),
            Folders::One(folder) => Ok(folder.clone()),
        }
    }

    fn lock_path(&self, at: &Where) -> Result<PathBuf> {
        self.files()?.layout().branch_file(&at.project, &at.run, &at.branch, "worker-lock")
    }

    /// Does the procedure's steps up to its next `Step::Compute` or `Step::Done`, which it gives.
    pub fn execute(&mut self, procedure: &mut Procedure) -> Result<Step> {
        if self.failed {
            return Err(Error::refused("the stored Run's writer stopped at a write that failed"));
        }
        loop {
            let step = procedure.next()?;
            if matches!(step, Step::Compute | Step::Done) {
                return Ok(step);
            }
            let given = match self.take(&step, procedure) {
                Ok(given) => given,
                Err(_) if procedure.may_fail() => Given::Failed,
                Err(error) => {
                    self.failed = true;
                    return Err(error);
                }
            };
            procedure.give(given)?;
        }
    }

    /// Does one step: what it read, or where it wrote.
    fn take(&mut self, step: &Step, procedure: &Procedure) -> Result<Given> {
        Ok(match step {
            Step::RunRecord { project, run } => Given::Text(Some(record_bytes(&self.files()?.run_record(project, run)?))),
            Step::BranchRecord { at, optional } => {
                let files = self.files()?;
                if *optional {
                    let path = files.layout().branch_file(&at.project, &at.run, &at.branch, "branch.json")?;
                    Given::Text(if path.is_file() { Some(fs::read(&path)?) } else { None })
                } else {
                    Given::Text(Some(record_bytes(&files.branch_record(&at.project, &at.run, &at.branch)?)))
                }
            }
            Step::Branches { project, run } => Given::Texts(self.files()?.branches(project, run)?.iter().map(record_bytes).collect()),
            Step::Projects => Given::Ids(self.files()?.projects()?.iter().filter_map(|record| record.get("project_id")?.as_str().map(str::to_owned)).collect()),
            Step::Runs { project } => Given::Ids(self.files()?.run_ids(project)?),
            Step::Journal(at) => {
                let folder = self.folder(at)?;
                let bytes = read_if_there(&folder.join(JOURNAL))?;
                if bytes.is_none() && python_stored(&folder) {
                    // A branch of a Run the Python server stored before #427: of an older format.
                    read_journal(&folder)?;
                }
                Given::Text(bytes)
            }
            Step::Piece { at, stored } => {
                let path = self.folder(at)?.join(DATA);
                let bytes = match self.writing.get(&path) {
                    Some(file) => file.read_at(stored.offset, stored.length)?,
                    None => {
                        if !self.reading.contains_key(&path) {
                            let file = File::open(&path)?;
                            self.reading.insert(path.clone(), file);
                        }
                        read_at(&self.reading[&path], stored.offset, stored.length)?
                    }
                };
                if bytes.len() as u64 != stored.length {
                    return Err(Error::refused("data.pack ends before the piece"));
                }
                Given::Text(Some(inflate_raw(&bytes, stored.raw_length)?))
            }
            Step::Create(at) => {
                let folder = self.folder(at)?;
                fs::create_dir_all(&folder)?;
                for name in [DATA, JOURNAL] {
                    let path = folder.join(name);
                    self.reading.remove(&path);
                    let file = AppendFile::open(&path, true)?;
                    self.writing.insert(path, file);
                }
                sync_dir(&folder)?;
                Given::Nothing
            }
            Step::Truncate { at, journal, data } => {
                self.file(at, JOURNAL)?.truncate(*journal)?;
                if let Some(data) = data {
                    self.file(at, DATA)?.truncate(*data)?;
                }
                Given::Nothing
            }
            Step::WritePiece { at, kind } => {
                let raw = procedure.raw();
                let stored = (self.compress)(raw);
                self.sizes.add(kind, raw.len() as u64, stored.len() as u64);
                self.quota.reserve(stored.len() as u64)?;
                let file = self.file(at, DATA)?;
                let offset = file.append(&stored)?;
                file.sync()?;
                Given::Place { offset, length: stored.len() as u64 }
            }
            Step::AppendLine(at) => {
                let line = procedure.line();
                self.quota.reserve(line.len() as u64)?;
                let file = self.file(at, JOURNAL)?;
                file.append(line)?;
                file.sync()?;
                Given::Nothing
            }
            Step::WriteBranch { project, run, record } => {
                self.files()?.write_branch(project, run, &branch_of(record))?;
                Given::Nothing
            }
            Step::SetStatus { at, status } => {
                self.files()?.set_branch_status(&at.project, &at.run, &at.branch, status, None)?;
                Given::Nothing
            }
            Step::SetRunStatus { project, run, status } => {
                self.files()?.set_run_status(project, run, status)?;
                Given::Nothing
            }
            Step::TryLock(at) => {
                let path = self.lock_path(at)?;
                match lock::try_lock(&path)? {
                    Some(lease) => {
                        self.locks.insert(path, lease);
                        Given::Lock(true)
                    }
                    None => Given::Lock(false),
                }
            }
            Step::Unlock(at) => {
                if let Some(lease) = self.locks.remove(&self.lock_path(at)?) {
                    lease.release();
                }
                Given::Nothing
            }
            Step::Compute | Step::Done => unreachable!("the procedure's own steps"),
        })
    }

    /// A file of a branch's stored Run open to write, opened once.
    fn file(&mut self, at: &Where, name: &str) -> Result<&mut AppendFile> {
        let path = self.folder(at)?.join(name);
        if !self.writing.contains_key(&path) {
            self.reading.remove(&path);
            let file = AppendFile::open(&path, false)?;
            self.writing.insert(path.clone(), file);
        }
        Ok(self.writing.get_mut(&path).expect("opened above"))
    }
}

/// Does a procedure that does not compute to its end.
fn finish(host: &mut Host, procedure: &mut Procedure) -> Result<()> {
    match host.execute(procedure)? {
        Step::Done => Ok(()),
        _ => Err(Error::refused("the procedure asks for a computation")),
    }
}

/// Computes a procedure from the start to its end, `ticks` at a time.
fn compute_to_end(host: &mut Host, procedure: &mut Procedure, ticks: u64) -> Result<()> {
    while host.execute(procedure)? == Step::Compute {
        procedure.advance(ticks)?;
    }
    Ok(())
}

fn start_of(start: &FromStart<'_>) -> Start {
    Start { export: start.export.to_vec(), world: start.world.map(str::to_owned), save_every: start.save_every }
}

/// What a computation from the start stored: the "end" line's head hash, the minute reached, the
/// sizes.
fn stored_of(host: &mut Host, procedure: &Procedure) -> Result<Stored> {
    let minute = procedure.run().ok_or_else(|| Error::refused("the computation made no Run"))?.minute()?;
    let head = procedure.last_head().ok_or_else(|| Error::refused("the end wrote no line"))?.to_owned();
    Ok(Stored { head, minute, sizes: std::mem::take(&mut host.sizes) })
}

/// Runs `start` for its whole clock and stores it in `folder` as it goes (`Procedure::from_start`
/// without a record), with its saved points, its "run" line recording
/// `model_versions` (the browser's `runAndStore`).
pub fn run_and_store(folder: PathBuf, start: &FromStart<'_>, model_versions: Versions, quota: Quota, compress: Compress) -> Result<Stored> {
    let mut host = Host::of_folder(folder, quota, compress);
    let mut procedure = Procedure::from_start(Where::default(), start_of(start), Some(model_versions), None);
    compute_to_end(&mut host, &mut procedure, start.ticks)?;
    stored_of(&mut host, &procedure)
}

impl ProjectFiles {
    fn require_lease(&self, lease: &Lease, project: &str, run: &str, branch: &str) -> Result<()> {
        if self.holds_branch(lease, project, run, branch)? {
            Ok(())
        } else {
            Err(Error::refused("the lease is not this branch's writer's"))
        }
    }

    /// A branch's stored Run opened to read, with its parents' (the browser's `branchReader`).
    pub fn reader(&self, project: &str, run: &str, branch: &str) -> Result<StoredRun> {
        StoredRun::open(&self.branch_folder(project, run, branch)?, &|parent| self.branch_folder(project, run, parent))
    }

    /// Goes on with a branch from its record's head toward `target` (`Procedure::go_on`): its
    /// stored Run cut back to the head and restored there. `lease` is the branch's writer's.
    pub fn go_on(&self, lease: &Lease, project: &str, run: &str, branch: &str, target: i64) -> Result<Computation> {
        self.require_lease(lease, project, run, branch)?;
        let mut host = Host::of_store(self)?;
        let mut procedure = Procedure::go_on(Where::new(project, run, branch), target);
        if host.execute(&mut procedure)? != Step::Compute {
            return Err(Error::refused("the computation did not start"));
        }
        Ok(Computation { host, procedure })
    }

    /// Starts a branch's stored Run at minute 0 of `start`'s exported world (the browser's `POST
    /// runs`, `Procedure::start`): its "run" line, recording the Run's model versions, and its
    /// "export" piece; the branch's record at that head, without a status. `start.ticks` is not
    /// used. `lease` is the branch's writer's.
    pub fn start_branch(&self, lease: &Lease, project: &str, run: &str, branch: &str, start: &FromStart<'_>) -> Result<Record> {
        self.require_lease(lease, project, run, branch)?;
        let mut host = Host::of_store(self)?;
        finish(&mut host, &mut Procedure::start(Where::new(project, run, branch), start_of(start)))?;
        self.branch_record(project, run, branch)
    }

    /// Computes a branch from its stored Run's start in one go and stores it (the browser's
    /// `computeBranch`, `Procedure::from_start`): `running` while it computes, then computed to the
    /// minute the Run reached, its head the "end" line, `completed` (or `failed`, and the error,
    /// where it fails).
    pub fn compute_branch(&self, lease: &Lease, project: &str, run: &str, branch: &str, start: &FromStart<'_>) -> Result<(Record, Stored)> {
        self.require_lease(lease, project, run, branch)?;
        let mut host = Host::of_store(self)?;
        let mut procedure = Procedure::from_start(Where::new(project, run, branch), start_of(start), None, Some(Lineage::default()));
        if let Err(error) = compute_to_end(&mut host, &mut procedure, start.ticks) {
            if procedure.fail() {
                host.failed = false;
                finish(&mut host, &mut procedure)?;
            }
            return Err(error);
        }
        let stored = stored_of(&mut host, &procedure)?;
        Ok((self.branch_record(project, run, branch)?, stored))
    }

    /// Makes the branch `fork.child` of `fork.parent` at `fork.minute` (`Procedure::fork`): the
    /// parent at the minute, checked against `fork.state_hash` where given, the program applied,
    /// and the child's stored Run and record (`paused` at the minute). `lease` is the child's
    /// writer's.
    pub fn fork(&self, lease: &Lease, project: &str, run: &str, fork: &ForkRequest) -> Result<Forked> {
        self.require_lease(lease, project, run, &fork.child)?;
        let mut host = Host::of_store(self)?;
        let mut procedure = Procedure::fork(project, run, fork.clone());
        finish(&mut host, &mut procedure)?;
        Ok(Forked {
            record: self.branch_record(project, run, &fork.child)?,
            receipt: procedure.receipt().to_owned(),
            state_hash: procedure.state_hash().to_owned(),
        })
    }

    /// Keeps a question's turn (its JSON text) in a branch's stored Run computed to its end, after
    /// its "end" line (`Procedure::keep_turn`): the branch's head stays. `lease` is the branch's
    /// writer's.
    pub fn keep_turn(&self, lease: &Lease, project: &str, run: &str, branch: &str, turn: &[u8]) -> Result<()> {
        self.require_lease(lease, project, run, branch)?;
        let mut host = Host::of_store(self)?;
        finish(&mut host, &mut Procedure::keep_turn(Where::new(project, run, branch), turn.to_vec()))
    }

    /// The questions and answers kept in a branch's stored Run, oldest first; at most the last
    /// `limit`. None for a branch without a stored Run.
    pub fn conversations(&self, project: &str, run: &str, branch: &str, limit: usize) -> Result<Vec<Vec<u8>>> {
        let path = self.layout().branch_file(project, run, branch, JOURNAL)?;
        if fs::metadata(&path).map_or(true, |metadata| metadata.len() == 0) {
            return Ok(Vec::new());
        }
        let mut turns = self.reader(project, run, branch)?.records(RecordKind::Conversations)?;
        let skip = turns.len().saturating_sub(limit);
        Ok(turns.split_off(skip))
    }

    /// The "conversations" pieces of a branch's stored Run, each one's bytes before compression
    /// (the browser's `conversationPieces`): none for a branch without a stored Run.
    pub fn conversation_pieces(&self, project: &str, run: &str, branch: &str) -> Result<Vec<Vec<u8>>> {
        let path = self.layout().branch_file(project, run, branch, JOURNAL)?;
        if fs::metadata(&path).map_or(true, |metadata| metadata.len() == 0) {
            return Ok(Vec::new());
        }
        self.reader(project, run, branch)?.raw_records(RecordKind::Conversations)
    }

    /// The branches left `running` that no writer holds (their lock is free) become `interrupted`,
    /// with their Runs, as the Python server's start does (`Procedure::recover`); their heads stay.
    /// Returns them as `(project, run, branch)`.
    pub fn recover_interrupted(&self) -> Result<Vec<(String, String, String)>> {
        let mut host = Host::of_store(self)?;
        let mut procedure = Procedure::recover();
        finish(&mut host, &mut procedure)?;
        Ok(procedure.recovered().iter().map(|at| (at.project.clone(), at.run.clone(), at.branch.clone())).collect())
    }
}

/// A fork made: the child's record, the program's receipt (its JSON text), and the child's state
/// hash at the minute.
#[derive(Debug)]
pub struct Forked {
    pub record: Record,
    pub receipt: String,
    pub state_hash: String,
}

/// A branch being computed (`ProjectFiles::go_on`): its procedure and the files it writes.
pub struct Computation {
    host: Host,
    procedure: Procedure,
}

impl Computation {
    /// The branch's id.
    pub fn branch(&self) -> &str {
        &self.procedure.at().branch
    }

    pub fn target(&self) -> i64 {
        self.procedure.target()
    }

    pub fn duration(&self) -> i64 {
        self.procedure.duration()
    }

    /// The branch's head now: its minute and hash.
    pub fn head(&self) -> (i64, &str) {
        self.procedure.head()
    }

    /// The Run where the computation got to.
    pub fn run(&self) -> &PlainRun {
        self.procedure.run().expect("the Run of a computation")
    }

    pub fn run_mut(&mut self) -> &mut PlainRun {
        self.procedure.run_mut().expect("the Run of a computation")
    }

    /// What was written of each kind of piece.
    pub fn sizes(&self) -> &Sizes {
        &self.host.sizes
    }

    /// One chunk: at most `ticks` ticks, not past the next saved point or the target, then the
    /// blocks sealed since stored, and at a saved point, the target or the clock's end the
    /// branch's new head. The computation's status once it ends (`completed` at the clock's end,
    /// `paused` at the target), else None. `advance` then `store`.
    pub fn chunk(&mut self, files: &ProjectFiles, ticks: u64) -> Result<Option<&'static str>> {
        self.advance(ticks)?;
        self.store(files)
    }

    /// A chunk's ticks, without the files: at most `ticks`, not past the next saved point or the
    /// target. The caller need not hold the store's transaction for them, and should not where the
    /// Run's LLM link takes it (the server's usage ledger, #465).
    pub fn advance(&mut self, ticks: u64) -> Result<()> {
        self.procedure.advance(ticks)?;
        Ok(())
    }

    /// A chunk's files, under the store's transaction: the blocks sealed since stored, and at a
    /// saved point, the target or the clock's end the branch's new head. The computation's status
    /// once it ends, else None. The files are the store's the computation was started in.
    pub fn store(&mut self, _files: &ProjectFiles) -> Result<Option<&'static str>> {
        Ok(match self.host.execute(&mut self.procedure)? {
            Step::Done => self.procedure.status(),
            _ => None,
        })
    }

    /// Ends the computation where it got to (a cancel): a saved point there, where it is past the
    /// head. Its status.
    pub fn stop(&mut self, files: &ProjectFiles) -> Result<&'static str> {
        self.procedure.stop()?;
        self.store(files)?.ok_or_else(|| Error::refused("the stopped computation did not end"))
    }

    /// Writes the final `status` to the branch's record (at its head), and the Run's: `running`
    /// while another of its branches computes, else `status` (`ProjectFiles::set_branch_status`).
    pub fn end(self, files: &ProjectFiles, status: &str) -> Result<Record> {
        let at = self.procedure.at();
        files.set_branch_status(&at.project, &at.run, &at.branch, status, None)
    }
}

/// The model versions of a JSON object of strings (a Run record's `model_versions`).
pub fn versions_of(value: &Value) -> Result<Versions> {
    Ok(sheltersim_core::model_version::versions_of_json(value.to_string().as_bytes())?)
}
