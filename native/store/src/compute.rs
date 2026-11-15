//! Computing a branch into its folder, as the browser does (`native/wasm/js/compute.mjs`,
//! `interventions.mjs`, `project_store.mjs`'s `computeBranch` and `keepTurn`), for a writer that
//! holds the branch's lease (`ProjectFiles::claim_branch`).
//!
//! A branch's head. A branch's record (`computed_until`, `checkpoint_hash`) names a line of its
//! stored Run's journal the Run can be read back at and gone on from: the "export" line (minute 0),
//! a "saved" line, or the "end" line; `checkpoint_hash` is that line's head hash. The record is
//! written after the line is flushed, so the record is the commit: lines after the head are a
//! computation that did not reach its next head, and going on cuts them off first, unless a branch
//! forked from it reads them (the core refuses that cut).
//!
//! Going on (`ProjectFiles::go_on`, compute.mjs's `start`): the branch cut back to its head and
//! restored there, collecting its decision records. Then `Computation::chunk` computes at most a
//! chunk of ticks, not past the next saved point or the target, stores the blocks sealed since, and
//! at every `save_every` minutes, at the target and at the clock's end stores a saved point and
//! moves the branch's record to it (`running` until the computation ends; at the end the "world"
//! and "end" lines too). `Computation::stop` ends it early with a saved point where it got to. The
//! caller sets the final status of the branch and the Run (`Computation::end`).

use std::fs;

use serde_json::Value;
use sheltersim_core::model_version::Versions;
use sheltersim_core::plain::PlainRun;
use sheltersim_core::run_store::{Fork, ForkOrigin, Head, HeadKind, Journal, RecordKind, RunWriter, Schedule, Storing};

use crate::compress::COMPRESSION;
use crate::files::AppendFile;
use crate::lock::{self, Lease};
use crate::records::{Branch, ProjectFiles, Record};
use crate::stored::{python_stored, read_journal, run_and_store, FromStart, Stored, StoredRun, StoredRunWriter, DATA, JOURNAL};
use crate::{Error, Result};

/// The status of a branch and its Run while a computation goes on.
pub const RUNNING: &str = "running";

fn text<'a>(record: &'a Record, key: &str) -> Option<&'a str> {
    record.get(key).and_then(Value::as_str)
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

    /// The latest line of a branch's stored Run that a branch forked from it reads its pieces up to
    /// (each child's base), or None where none is forked from it (compute.mjs's `forkedThrough`).
    pub fn kept_through(&self, project: &str, run: &str, branch: &str) -> Result<Option<usize>> {
        let mut latest: Option<usize> = None;
        for record in self.branches(project, run)? {
            if text(&record, "parent_branch_id") != Some(branch) {
                continue;
            }
            let child = text(&record, "branch_id").ok_or_else(|| Error::refused(crate::layout::INVALID_ID))?;
            let path = self.layout().branch_file(project, run, child, JOURNAL)?;
            let bytes = match fs::read(&path) {
                Ok(bytes) if !bytes.is_empty() => bytes,
                Ok(_) => continue,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            if let Some(fork) = Journal::parse(&bytes)?.fork() {
                let base = fork.base.line as usize;
                latest = Some(latest.map_or(base, |latest| latest.max(base)));
            }
        }
        Ok(latest)
    }

    /// Cuts a branch's stored Run back to the head `hash`: the journal's lines after it, then the
    /// bytes of data.pack no line up to it points at; refused where a branch forked from it reads
    /// its lines up to `kept_through`, past the head. The head and the core's writer after it.
    fn cut_to_head(&self, project: &str, run: &str, branch: &str, hash: &str) -> Result<(Head, RunWriter)> {
        let kept_through = self.kept_through(project, run, branch)?;
        let folder = self.branch_folder(project, run, branch)?;
        if python_stored(&folder) && !folder.join(JOURNAL).exists() {
            read_journal(&folder)?;
        }
        let mut journal_file = AppendFile::open(&folder.join(JOURNAL), false)?;
        let mut data_file = AppendFile::open(&folder.join(DATA), false)?;
        let journal = Journal::parse(&journal_file.read_all()?)?;
        let cut = journal.cut(hash, kept_through)?;
        journal_file.truncate(cut.journal_end)?;
        data_file.truncate(cut.data_end)?;
        Ok((cut.head, RunWriter::after(&journal, cut.head.index)?))
    }

    /// Goes on with a branch from its record's head toward `target` (compute.mjs's `start`): its
    /// stored Run cut back to the head and restored there. `lease` is the branch's writer's.
    pub fn go_on(&self, lease: &Lease, project: &str, run: &str, branch: &str, target: i64) -> Result<Computation> {
        self.require_lease(lease, project, run, branch)?;
        let record = self.branch_record(project, run, branch)?;
        let record = Branch::of_record(&record)?;
        let hash = record.checkpoint_hash.clone().ok_or_else(|| Error::refused("branch has no computed minutes"))?;
        let computed = record.computed_until.unwrap_or(record.fork_minute);
        let (head, core) = self.cut_to_head(project, run, branch, &hash)?;
        let mut reader = self.reader(project, run, branch)?;
        let duration = reader.journal().duration()?;
        if !(computed < target && target <= duration) {
            return Err(Error::refused("target minute must be in the uncomputed remaining range"));
        }
        let save_every = reader.journal().run()?.save_every;
        let sought = reader.seek(head.minute, true)?;
        drop(reader);
        if head.kind == HeadKind::Saved && !(sought.run.minute()? == head.minute && sought.from == head.minute && core.fits(&sought.run)?) {
            return Err(Error::refused("the Run restored at the branch's head does not hold what its stored Run holds there"));
        }
        let folder = self.branch_folder(project, run, branch)?;
        let writer = StoredRunWriter::append(&folder, core, self.quota()?)?.with_compress(self.compress());
        Ok(Computation {
            project: project.to_owned(),
            run_id: run.to_owned(),
            record,
            run: sought.run,
            writer,
            duration,
            save_every,
            target,
            head_minute: head.minute,
            head_hash: hash,
        })
    }

    /// Starts a branch's stored Run at minute 0 of `start`'s exported world (the browser's `POST
    /// runs`, scenario_routes.mjs): its "run" line, recording the Run's model versions, and its
    /// "export" piece; the branch's record at that head, without a status (not computed yet).
    /// `start.ticks` is not used. `lease` is the branch's writer's.
    pub fn start_branch(&self, lease: &Lease, project: &str, run: &str, branch: &str, start: &FromStart<'_>) -> Result<Record> {
        self.require_lease(lease, project, run, branch)?;
        let versions = versions_of(self.run_record(project, run)?.get("model_versions").unwrap_or(&Value::Null))?;
        let simulation = plain_run(start.export, start.world)?;
        let mut writer = StoredRunWriter::create(&self.branch_folder(project, run, branch)?, self.quota()?)?.with_compress(self.compress());
        let storing = Storing { model_versions: versions, compression: COMPRESSION.to_owned(), save_every: start.save_every };
        writer.core.start(&simulation, start.export.to_vec(), start.world.map(str::to_owned), &storing)?;
        let head = writer.flush(None)?.ok_or_else(|| Error::refused("the start wrote no line"))?;
        self.write_branch(project, run, &Branch { branch_id: branch.to_owned(), checkpoint_hash: Some(head), ..Branch::default() })
    }

    /// Computes a branch from its stored Run's start in one go and stores it (the browser's
    /// `computeBranch`): `running` while it computes, then computed to the minute the Run reached,
    /// its head the "end" line, `completed` (or `failed`, and the error, where it fails).
    pub fn compute_branch(&self, lease: &Lease, project: &str, run: &str, branch: &str, start: &FromStart<'_>) -> Result<(Record, Stored)> {
        self.require_lease(lease, project, run, branch)?;
        let versions = versions_of(self.run_record(project, run)?.get("model_versions").unwrap_or(&Value::Null))?;
        let mut record = Branch { branch_id: branch.to_owned(), status: Some(RUNNING.to_owned()), ..Branch::default() };
        self.write_branch(project, run, &record)?;
        let stored = match run_and_store(&self.branch_folder(project, run, branch)?, start, versions, self.quota()?, self.compress()) {
            Ok(stored) => stored,
            Err(error) => {
                record.status = Some("failed".to_owned());
                self.write_branch(project, run, &record)?;
                return Err(error);
            }
        };
        let duration = read_journal(&self.branch_folder(project, run, branch)?)?.duration()?;
        record.computed_until = Some(stored.minute);
        record.checkpoint_hash = Some(stored.head.clone());
        record.status = Some(if stored.minute >= duration { "completed" } else { "paused" }.to_owned());
        Ok((self.write_branch(project, run, &record)?, stored))
    }

    /// Makes the branch `fork.child` of `fork.parent` at `fork.minute` (interventions.mjs's `fork`):
    /// the parent at the minute, checked against `fork.state_hash` (the state the preview saw) where
    /// given, the program applied, and the child's stored Run and record (`paused` at the minute).
    /// The child's stored Run reads the parent's pieces up to the line the parent's world was
    /// restored from, and stores the rest from there. `lease` is the child's writer's.
    pub fn fork(&self, lease: &Lease, project: &str, run: &str, fork: &ForkRequest<'_>) -> Result<Forked> {
        self.require_lease(lease, project, run, fork.child)?;
        if self.layout().branch_file(project, run, fork.child, "branch.json")?.is_file() {
            return Err(Error::refused("child branch already exists"));
        }
        let parent = self.branch_record(project, run, fork.parent)?;
        let mut reader = self.reader(project, run, fork.parent)?;
        let mut sought = reader.seek(fork.minute, true)?;
        let origin = sought.run.state_digest()?;
        if fork.state_hash.is_some_and(|wanted| wanted != origin) {
            return Err(Error::refused("source state changed; preview the intervention again"));
        }
        let receipt = sought.run.apply_effect_program_json(fork.program.as_bytes())?;
        let folder = self.branch_folder(project, run, fork.child)?;
        let mut writer = StoredRunWriter::create(&folder, self.quota()?)?.with_compress(self.compress());
        writer.core.fork_start(
            reader.journal(),
            sought.base,
            Fork {
                parent_branch_id: fork.parent.to_owned(),
                minute: fork.minute,
                origin: ForkOrigin {
                    branch_id: fork.parent.to_owned(),
                    minute: fork.minute,
                    state_hash: origin,
                    checkpoint_hash: text(&parent, "checkpoint_hash").map(str::to_owned),
                },
                program: fork.program.to_owned(),
                approved_program_hash: fork.approved_program_hash.to_owned(),
                receipt: receipt.clone(),
            },
        )?;
        writer.flush(None)?;
        let head = writer.save(&mut sought.run)?;
        let record = self.write_branch(
            project,
            run,
            &Branch {
                branch_id: fork.child.to_owned(),
                parent_branch_id: Some(fork.parent.to_owned()),
                fork_minute: fork.minute,
                computed_until: Some(fork.minute),
                checkpoint_hash: Some(head),
                status: Some("paused".to_owned()),
            },
        )?;
        Ok(Forked { record, receipt, state_hash: sought.run.state_digest()? })
    }

    /// Keeps a question's turn (its JSON text) in a branch's stored Run computed to its end, after
    /// its "end" line (the browser's `keepTurn`): the branch's head stays. A line a writer left cut
    /// short after the last whole one is cut off first. `lease` is the branch's writer's.
    pub fn keep_turn(&self, lease: &Lease, project: &str, run: &str, branch: &str, turn: &[u8]) -> Result<()> {
        self.require_lease(lease, project, run, branch)?;
        let folder = self.branch_folder(project, run, branch)?;
        let mut journal_file = AppendFile::open(&folder.join(JOURNAL), false)?;
        let journal = Journal::parse(&journal_file.read_all()?)?;
        let mut core = RunWriter::after_end(&journal)?;
        journal_file.truncate(journal.entries().last().map_or(0, |entry| entry.end))?;
        drop(journal_file);
        core.turns(&[turn.to_vec()])?;
        StoredRunWriter::append(&folder, core, self.quota()?)?.with_compress(self.compress()).flush(None)?;
        Ok(())
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

    /// The branches left `running` that no writer holds (their lock is free) become `interrupted`,
    /// with their Runs, as the Python server's start does (`recover_interrupted`); their heads stay.
    /// Each is taken under its lock. Returns them as `(project, run, branch)`.
    pub fn recover_interrupted(&self) -> Result<Vec<(String, String, String)>> {
        let mut recovered = Vec::new();
        for project in self.projects()? {
            let Some(project) = text(&project, "project_id").map(str::to_owned) else { continue };
            for run in self.run_ids(&project)? {
                let Ok(branches) = self.branches(&project, &run) else { continue };
                for branch in branches {
                    if text(&branch, "status") != Some(RUNNING) {
                        continue;
                    }
                    let Some(id) = text(&branch, "branch_id").map(str::to_owned) else { continue };
                    let Some(lease) = lock::try_lock(&self.layout().branch_file(&project, &run, &id, "worker-lock")?)? else { continue };
                    let mut record = Branch::of_record(&self.branch_record(&project, &run, &id)?)?;
                    if record.status.as_deref() == Some(RUNNING) {
                        record.status = Some("interrupted".to_owned());
                        self.write_branch(&project, &run, &record)?;
                        self.set_run_status(&project, &run, "interrupted")?;
                        recovered.push((project.clone(), run.clone(), id));
                    }
                    lease.release();
                }
            }
        }
        Ok(recovered)
    }
}

/// A fork to make (`ProjectFiles::fork`).
#[derive(Clone, Debug)]
pub struct ForkRequest<'a> {
    pub parent: &'a str,
    pub child: &'a str,
    pub minute: i64,
    /// The EffectProgram's JSON text, as approved.
    pub program: &'a str,
    pub approved_program_hash: &'a str,
    /// The state the preview saw: the fork is refused where the parent's is not this at the minute.
    pub state_hash: Option<&'a str>,
}

/// A fork made: the child's record, the program's receipt (its JSON text), and the child's state
/// hash at the minute.
#[derive(Debug)]
pub struct Forked {
    pub record: Record,
    pub receipt: String,
    pub state_hash: String,
}

/// A branch being computed (`ProjectFiles::go_on`).
pub struct Computation {
    project: String,
    run_id: String,
    record: Branch,
    /// The Run where the computation got to.
    pub run: PlainRun,
    writer: StoredRunWriter,
    duration: i64,
    save_every: u64,
    target: i64,
    head_minute: i64,
    head_hash: String,
}

impl Computation {
    /// The branch's id.
    pub fn branch(&self) -> &str {
        &self.record.branch_id
    }

    pub fn target(&self) -> i64 {
        self.target
    }

    pub fn duration(&self) -> i64 {
        self.duration
    }

    /// The branch's head now: its minute and hash.
    pub fn head(&self) -> (i64, &str) {
        (self.head_minute, &self.head_hash)
    }

    /// The writer, with its figures.
    pub fn writer(&self) -> &StoredRunWriter {
        &self.writer
    }

    /// One chunk (compute.mjs's `chunk`): at most `ticks` ticks, not past the next saved point or
    /// the target, then the blocks sealed since stored, and at a saved point, the target or the
    /// clock's end the branch's new head. The computation's status once it ends (`completed` at the
    /// clock's end, `paused` at the target), else None.
    pub fn chunk(&mut self, files: &ProjectFiles, ticks: u64) -> Result<Option<&'static str>> {
        let schedule = Schedule { every: self.save_every };
        let step = schedule.step(self.run.minute()?, ticks.max(1), Some(self.target));
        let ran = self.run.advance(step).map_err(|stopped| Error::refused(stopped.to_string()))?;
        let minute = self.run.minute()?;
        if ran != step {
            return Err(Error::refused(format!("the Run stopped at minute {minute}")));
        }
        self.writer.take(&mut self.run, false)?;
        if minute == self.duration {
            self.writer.save(&mut self.run)?;
            let head = self.writer.finish(&mut self.run)?;
            self.commit(files, minute, head)?;
            return Ok(Some("completed"));
        }
        if minute == self.target || schedule.is_point(minute) {
            let head = self.writer.save(&mut self.run)?;
            self.commit(files, minute, head)?;
        }
        Ok((minute == self.target).then_some("paused"))
    }

    /// Ends the computation where it got to (a cancel): a saved point there, where it is past the
    /// head. Its status.
    pub fn stop(&mut self, files: &ProjectFiles) -> Result<&'static str> {
        let minute = self.run.minute()?;
        if minute > self.head_minute {
            let head = self.writer.save(&mut self.run)?;
            self.commit(files, minute, head)?;
        }
        Ok(if minute == self.duration { "completed" } else { "paused" })
    }

    /// Moves the branch's head to the line of the head hash `head` at `minute`.
    fn commit(&mut self, files: &ProjectFiles, minute: i64, head: String) -> Result<()> {
        let record = Branch { computed_until: Some(minute), checkpoint_hash: Some(head.clone()), status: Some(RUNNING.to_owned()), ..self.record.clone() };
        files.write_branch(&self.project, &self.run_id, &record)?;
        self.head_minute = minute;
        self.head_hash = head;
        Ok(())
    }

    /// Writes the final `status` to the branch's record (at its head), and the Run's: `running`
    /// while another of its branches computes, else `status` (`ProjectFiles::set_branch_status`).
    pub fn end(self, files: &ProjectFiles, status: &str) -> Result<Record> {
        files.set_branch_status(&self.project, &self.run_id, &self.record.branch_id, status, None)
    }
}

/// The model versions of a JSON object of strings (a Run record's `model_versions`).
pub fn versions_of(value: &Value) -> Result<Versions> {
    Ok(sheltersim_core::model_version::versions_of_json(value.to_string().as_bytes())?)
}

/// A Run of the exported world `export` from minute 0 (for a Run without one stored yet): the
/// browser's `Simulation`.
pub fn plain_run(export: &[u8], world: Option<&str>) -> Result<PlainRun> {
    Ok(match world {
        Some(name) => PlainRun::of_worlds_json(export, name)?,
        None => PlainRun::of_json(export)?,
    })
}
