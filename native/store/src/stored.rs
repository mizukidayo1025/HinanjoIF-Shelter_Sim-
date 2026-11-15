//! A branch's stored Run in its folder: `data.pack` and `journal.jsonl` (the core's
//! `run_store`, format v5), written by `StoredRunWriter` and read by `StoredRun`, as the browser's
//! `run_store.mjs` writes and reads them.
//!
//! Writing (`StoredRunWriter::flush`): for each item the core queued, a piece compressed (deflate-
//! raw), appended to data.pack and flushed to the disk, then its line appended to the journal and
//! flushed; a line without a piece appended and flushed. The store's limit is checked before each
//! write (`Quota::reserve`). After a write fails the writer refuses to go on: the journal may end
//! with a line cut short, which reading ignores and going on from the branch's head cuts off.
//!
//! Reading (`StoredRun::open`): the journal read once, up to its last whole line that matches its
//! sha256 (a line a writer is appending, or one a killed writer left cut short, is not read), the
//! "run" line's compression checked, then a fork's parents' journals read through the chain
//! (`sheltersim_core::run_store::Chain`). Each piece is read from its level's data.pack, inflated
//! and checked against the sha256 its line holds of its bytes before compression.

use std::fs::{self, File};
use std::path::{Path, PathBuf};

use sheltersim_core::archive::Restore;
use sheltersim_core::llm::{Limits, Replay};
use sheltersim_core::model_version::Versions;
use sheltersim_core::plain::PlainRun;
use sheltersim_core::run_store::{self, Chain, Journal, Next, RecordKind, RunWriter, Schedule, Storing, Verified, Verify};

use crate::compress::{deflate_raw, inflate_raw, Compress, COMPRESSION};
use crate::files::{read_at, sync_dir, AppendFile};
use crate::quota::Quota;
use crate::{Error, Result};

/// The journal's file in a branch's folder.
pub const JOURNAL: &str = run_store::JOURNAL;
/// The pieces' file in a branch's folder.
pub const DATA: &str = run_store::DATA;

/// What a writer wrote of each kind of piece: how many, their bytes before and after compression.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sizes {
    pub kinds: Vec<(&'static str, Size)>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Size {
    pub pieces: u64,
    pub raw: u64,
    pub stored: u64,
}

impl Sizes {
    fn add(&mut self, kind: &'static str, raw: u64, stored: u64) {
        let at = match self.kinds.iter().position(|(each, _)| *each == kind) {
            Some(at) => at,
            None => {
                self.kinds.push((kind, Size::default()));
                self.kinds.len() - 1
            }
        };
        let size = &mut self.kinds[at].1;
        size.pieces += 1;
        size.raw += raw;
        size.stored += stored;
    }
}

/// The writer of a branch's stored Run: the core's writer (`core`) and the branch's two files.
#[derive(Debug)]
pub struct StoredRunWriter {
    pub core: RunWriter,
    data: AppendFile,
    journal: AppendFile,
    compress: Compress,
    quota: Quota,
    last_line: Option<Vec<u8>>,
    last_head: Option<String>,
    failed: bool,
    pub sizes: Sizes,
}

impl StoredRunWriter {
    /// The writer of a new stored Run in `folder` (made where it is not there), whose files it
    /// empties.
    pub fn create(folder: &Path, quota: Quota) -> Result<Self> {
        fs::create_dir_all(folder)?;
        let data = AppendFile::open(&folder.join(DATA), true)?;
        let journal = AppendFile::open(&folder.join(JOURNAL), true)?;
        sync_dir(folder)?;
        Ok(Self::of(RunWriter::new(), data, journal, quota))
    }

    /// The writer that appends what `core` queues to the stored Run in `folder` as it is (`core`
    /// goes on after a line of its journal: `RunWriter::after`, `after_end`).
    pub fn append(folder: &Path, core: RunWriter, quota: Quota) -> Result<Self> {
        let data = AppendFile::open(&folder.join(DATA), false)?;
        let journal = AppendFile::open(&folder.join(JOURNAL), false)?;
        Ok(Self::of(core, data, journal, quota))
    }

    fn of(core: RunWriter, data: AppendFile, journal: AppendFile, quota: Quota) -> Self {
        Self { core, data, journal, compress: deflate_raw, quota, last_line: None, last_head: None, failed: false, sizes: Sizes::default() }
    }

    /// The writer with another compressor of deflate-raw (the tests' stored blocks).
    pub fn with_compress(mut self, compress: Compress) -> Self {
        self.compress = compress;
        self
    }

    /// The last line written, and the head hash of the last line written.
    pub fn last_line(&self) -> Option<&[u8]> {
        self.last_line.as_deref()
    }

    pub fn last_head(&self) -> Option<&str> {
        self.last_head.as_deref()
    }

    /// What the store held when the writer started and what it wrote since.
    pub fn quota(&self) -> Quota {
        self.quota
    }

    /// Writes what the core queued, in order. `copied` is the compressed bytes of a piece copied
    /// from another stored Run, as they are (the core's `Next::Copy`). Returns the head hash of
    /// the last line written.
    pub fn flush(&mut self, mut copied: Option<&[u8]>) -> Result<Option<String>> {
        if self.failed {
            return Err(Error::refused("the stored Run's writer stopped at a write that failed"));
        }
        let wrote = self.flush_queue(&mut copied);
        if wrote.is_err() {
            self.failed = true;
        }
        wrote?;
        Ok(self.last_head.clone())
    }

    fn flush_queue(&mut self, copied: &mut Option<&[u8]>) -> Result<()> {
        while let Some(next) = self.core.next() {
            let encoded = match next {
                Next::Line => self.core.commit_line()?,
                Next::Piece { kind, raw } => {
                    let stored = (self.compress)(raw);
                    self.sizes.add(kind, raw.len() as u64, stored.len() as u64);
                    let offset = self.write_piece(&stored)?;
                    self.core.commit_piece(offset, stored.len() as u64)?
                }
                Next::Copy { kind, length } => {
                    let stored = copied.take().ok_or_else(|| Error::refused(format!("copy:{kind}: no bytes to copy")))?;
                    if stored.len() as u64 != length {
                        return Err(Error::refused(format!("{kind}: the copied piece is {} bytes, not {length}", stored.len())));
                    }
                    let offset = self.write_piece(stored)?;
                    self.core.commit_piece(offset, length)?
                }
            };
            self.quota.reserve(encoded.bytes.len() as u64)?;
            self.journal.append(&encoded.bytes)?;
            self.journal.sync()?;
            self.last_line = Some(encoded.bytes);
            self.last_head = Some(encoded.head);
        }
        Ok(())
    }

    fn write_piece(&mut self, stored: &[u8]) -> Result<u64> {
        self.quota.reserve(stored.len() as u64)?;
        let offset = self.data.append(stored)?;
        self.data.sync()?;
        Ok(offset)
    }

    /// The decision records `run` collected, the tables' new entries and the newly sealed blocks
    /// (every block when `last`), written. Call it between calls to `advance`.
    pub fn take(&mut self, run: &mut PlainRun, last: bool) -> Result<Option<String>> {
        self.core.take(run, last)?;
        self.flush(None)
    }

    /// A saved point of `run` now, its blocks sealed and stored and its decision records written
    /// first: the head hash of its line.
    pub fn save(&mut self, run: &mut PlainRun) -> Result<String> {
        self.core.save(run)?;
        self.flush(None)?.ok_or_else(|| Error::refused("a saved point wrote no line"))
    }

    /// The last pieces of a Run stored to its end (the decision records left, the world, the "end"
    /// line): the head hash of the "end" line.
    pub fn finish(&mut self, run: &mut PlainRun) -> Result<String> {
        self.core.finish(run)?;
        self.flush(None)?.ok_or_else(|| Error::refused("the end wrote no line"))
    }
}

/// What `run_and_store` stored.
#[derive(Debug)]
pub struct Stored {
    /// The head hash of the "end" line.
    pub head: String,
    /// The minute the Run reached.
    pub minute: i64,
    pub sizes: Sizes,
}

/// A Run computed from minute 0: the exported world `export` (or the world `world` of a file of
/// named worlds, as the fixtures hold them), `ticks` ticks at a time, saved every `save_every`
/// minutes and at the end.
#[derive(Clone, Copy, Debug)]
pub struct FromStart<'a> {
    pub export: &'a [u8],
    pub world: Option<&'a str>,
    pub ticks: u64,
    pub save_every: u64,
}

/// Runs `start` for its whole clock and stores it in `folder` as it goes, with its decision
/// records and saved points, its "run" line recording `model_versions` (the browser's
/// `runAndStore`).
pub fn run_and_store(folder: &Path, start: &FromStart<'_>, model_versions: Versions, quota: Quota, compress: Compress) -> Result<Stored> {
    let FromStart { export, world, ticks, save_every } = *start;
    let mut writer = StoredRunWriter::create(folder, quota)?.with_compress(compress);
    let mut run = match world {
        Some(name) => PlainRun::of_worlds_json(export, name)?,
        None => PlainRun::of_json(export)?,
    };
    run.collect_decisions(true);
    let duration = run.duration()?;
    let storing = Storing { model_versions, compression: COMPRESSION.to_owned(), save_every };
    writer.core.start(&run, export.to_vec(), world.map(str::to_owned), &storing)?;
    writer.flush(None)?;
    let schedule = Schedule { every: save_every };
    loop {
        let step = schedule.step(run.minute()?, ticks.max(1), None);
        let ran = run.advance(step).map_err(|stopped| Error::refused(stopped.to_string()))?;
        writer.take(&mut run, false)?;
        if ran < step {
            break;
        }
        if schedule.due(run.minute()?, duration) {
            writer.save(&mut run)?;
        }
    }
    writer.take(&mut run, true)?;
    if save_every > 0 {
        writer.save(&mut run)?;
    }
    let head = writer.finish(&mut run)?;
    Ok(Stored { head, minute: run.minute()?, sizes: writer.sizes })
}

/// A stored Run opened to read: the branch's journal and chain (with a fork's parents'), and each
/// level's data.pack (the Run from minute 0's first, the branch's last).
#[derive(Debug)]
pub struct StoredRun {
    journal: Journal,
    chain: Chain,
    datas: Vec<File>,
    export: Option<Vec<u8>>,
}

/// How a seek answers the LLM's calls of a Run that kept a tape: whether the Run was offline, and
/// the limits its rules asked within.
#[derive(Clone, Copy, Debug, Default)]
pub struct Replaying {
    pub offline: bool,
    pub limits: Limits,
}

/// Where a seek put the Run: `run` at the minute asked for, computed from `from` (a minute), the
/// branch's line `base` it was restored from.
pub struct Sought {
    pub run: PlainRun,
    pub from: i64,
    pub base: usize,
}

/// What the stored Runs the Python server wrote before #427 were kept as, named in the error of
/// reading one (`run_store::outdated`).
pub const PYTHON_CHECKPOINTS: &str = "Python の Checkpoint";

/// The journal of the stored Run in `folder`, as it is now. A branch of a Run the Python server
/// stored before #427 (its Checkpoints in the Run's folder, no journal) is of an older format:
/// `stored_run_outdated`, as a Run of format v1 to v4 is.
pub fn read_journal(folder: &Path) -> Result<Journal> {
    let bytes = match fs::read(folder.join(JOURNAL)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && python_stored(folder) => return Err(run_store::outdated(PYTHON_CHECKPOINTS).into()),
        Err(error) => return Err(error.into()),
    };
    Ok(Journal::parse(&bytes)?)
}

/// Whether the branch folder `folder` (`<run>/branches/<branch>`) is of a Run the Python server
/// stored before #427: its Run's folder holds the Checkpoints' folder.
pub fn python_stored(folder: &Path) -> bool {
    folder.parent().and_then(Path::parent).is_some_and(|run| run.join("checkpoints").is_dir())
}

impl StoredRun {
    /// The stored Run in `folder`; a fork's reads its parents' pieces, whose folders `parent(branch
    /// id)` gives. An error for a journal without its "run" line, a Run of an older format
    /// (`stored_run_outdated`) or another compression.
    pub fn open(folder: &Path, parent: &dyn Fn(&str) -> Result<PathBuf>) -> Result<Self> {
        let journal = read_journal(folder)?;
        let compression = &journal.run()?.compression;
        if compression != COMPRESSION {
            return Err(Error::refused(format!("the stored Run's pieces are compressed with {compression}, which this host does not read")));
        }
        let mut chain = Chain::new(journal.clone())?;
        let mut datas = vec![File::open(folder.join(DATA))?];
        while let Some(branch) = chain.parent_needed().map(str::to_owned) {
            let folder = parent(&branch)?;
            chain.add_parent(read_journal(&folder)?)?;
            datas.insert(0, File::open(folder.join(DATA))?);
        }
        Ok(Self { journal, chain, datas, export: None })
    }

    /// The branch's journal as read.
    pub fn journal(&self) -> &Journal {
        &self.journal
    }

    pub fn chain(&self) -> &Chain {
        &self.chain
    }

    /// The compressed bytes of the piece at `position` of the chain, checked to be all there.
    pub fn stored(&self, position: usize) -> Result<Vec<u8>> {
        let (level, stored) = self.chain.stored(position)?;
        let bytes = read_at(&self.datas[level], stored.offset, stored.length)?;
        self.chain.check_stored(position, bytes.len() as u64)?;
        Ok(bytes)
    }

    /// The piece at `position` of the chain, inflated, not yet checked against its line.
    pub fn inflated(&self, position: usize) -> Result<Vec<u8>> {
        let raw_length = self.chain.stored(position)?.1.raw_length;
        inflate_raw(&self.stored(position)?, raw_length)
    }

    /// The bytes of the piece at `position` of the chain, checked against its line.
    pub fn raw_at(&self, position: usize) -> Result<Vec<u8>> {
        let raw = self.inflated(position)?;
        self.chain.check_raw(position, &raw)?;
        Ok(raw)
    }

    /// The compressed bytes of the branch's own line `index`'s piece, checked to hold the bytes
    /// written (a piece to copy into another stored Run).
    pub fn compressed(&self, index: usize) -> Result<Vec<u8>> {
        let stored = &self.journal.piece(index)?.stored;
        let bytes = read_at(self.datas.last().expect("a branch's data.pack"), stored.offset, stored.length)?;
        self.journal.check_stored(index, bytes.len() as u64)?;
        self.journal.check_raw(index, &inflate_raw(&bytes, stored.raw_length)?)?;
        Ok(bytes)
    }

    /// The pieces of the records of `kind`, each one's bytes before compression (what the core's
    /// answers read, `answers::routes::conversations_answer`).
    pub fn raw_records(&self, kind: RecordKind) -> Result<Vec<Vec<u8>>> {
        self.chain.records(kind)?.into_iter().map(|position| self.raw_at(position)).collect()
    }

    /// The JSON texts of the records of `kind`: the decision records and the tape's from the Run's
    /// start, the turns of the branch itself.
    pub fn records(&self, kind: RecordKind) -> Result<Vec<Vec<u8>>> {
        let mut records = Vec::new();
        for position in self.chain.records(kind)? {
            records.extend(run_store::records::read(&self.raw_at(position)?)?);
        }
        Ok(records)
    }

    /// The pieces of the stored metric samples, in order (a fork's from its parent's up to its
    /// base, then its own): each `(kind, bytes)`, "sample_tables" or "sample_block", checked against
    /// its line. What the KPI history reads (`answers::history::KpiTable`).
    pub fn sample_pieces(&self) -> Result<Vec<(&'static str, Vec<u8>)>> {
        let mut pieces = Vec::new();
        for position in self.chain.sample_pieces()? {
            let kind = self.chain.piece(position)?.0.piece.kind();
            pieces.push((kind, self.raw_at(position)?));
        }
        Ok(pieces)
    }

    /// The number of metric samples the Run's "end" line counts, where it has one.
    pub fn end_samples(&self) -> Option<u64> {
        self.journal.end().map(|end| end.samples)
    }

    /// The exported world the Run started from, read once.
    pub fn export_bytes(&mut self) -> Result<Vec<u8>> {
        if self.export.is_none() {
            self.export = Some(self.raw_at(self.chain.export()?)?);
        }
        Ok(self.export.clone().expect("read above"))
    }

    /// The Run at `minute` (from the fork's minute to the duration): restored from the nearest saved
    /// point at or before it, or built from the exported world where there is none, then computed to
    /// it; collecting its decision records from where it was restored with `decisions`.
    pub fn seek(&mut self, minute: i64, decisions: bool) -> Result<Sought> {
        self.seek_with(minute, decisions, &Replaying::default())
    }

    /// `seek`, and for a Run that kept an LLM tape (`llm_tape` pieces, #428) the ticks after the
    /// restored point answered from the tape's entries after that point's count (`llm::Replay`,
    /// within `replaying`'s limits): a seek never asks a provider, and reaches the world the Run
    /// reached with its link.
    pub fn seek_with(&mut self, minute: i64, decisions: bool, replaying: &Replaying) -> Result<Sought> {
        let seek = self.chain.seek(minute)?;
        let export = self.export_bytes()?;
        let world = self.journal.run()?.world.clone();
        let mut run = match seek.from {
            None => match &world {
                Some(name) => PlainRun::of_worlds_json(&export, name)?,
                None => PlainRun::of_json(&export)?,
            },
            Some(point) => {
                let stored = &self.journal.piece(point)?.stored;
                let bytes = read_at(self.datas.last().expect("a branch's data.pack"), stored.offset, stored.length)?;
                self.journal.check_stored(point, bytes.len() as u64)?;
                let raw = inflate_raw(&bytes, stored.raw_length)?;
                self.journal.check_raw(point, &raw)?;
                let mut restore = Restore::new(&raw)?;
                let mut next = self.chain.next_restore_read(0, &restore)?;
                while let Some(position) = next {
                    self.chain.restore_read(position, &self.inflated(position)?, &mut restore)?;
                    next = self.chain.next_restore_read(position + 1, &restore)?;
                }
                let entry = match &world {
                    Some(name) => PlainRun::entry_of_worlds_json(&export, name)?,
                    None => PlainRun::entry_of_json(&export)?,
                };
                PlainRun::restore(entry, restore)?
            }
        };
        if decisions {
            run.collect_decisions(true);
        }
        let tape = self.records(RecordKind::Tape)?;
        if !tape.is_empty() {
            let from = match seek.from {
                Some(point) => self.journal.saved(point)?.counts.tape as usize,
                None => 0,
            };
            let entries = tape.get(from..).ok_or_else(|| Error::refused("the stored Run's LLM tape is shorter than its saved point counts"))?;
            run.set_llm(Box::new(Replay::new(entries, replaying.offline)?), replaying.limits)?;
        }
        let ticks = minute - run.minute()?;
        let ran = if ticks < 0 { None } else { run.advance(ticks as u64).ok() };
        if ran != Some(ticks as u64) {
            return Err(Error::refused(format!("the Run does not reach minute {minute}")));
        }
        Ok(Sought { run, from: seek.from_minute, base: seek.base })
    }

    /// Reads every piece back and checks it (the core's `Verify`): each piece's bytes against its
    /// line, and the digests the log, the samples and the world read back to against the ones the
    /// "end" line holds.
    pub fn verify(&self) -> Result<Verified> {
        let mut verify = Verify::new();
        for position in Verify::reads(&self.chain)? {
            verify.read(&self.chain, position, &self.inflated(position)?)?;
        }
        Ok(verify.result(&self.chain)?)
    }
}
