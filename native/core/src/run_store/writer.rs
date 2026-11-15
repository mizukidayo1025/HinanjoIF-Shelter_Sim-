//! What a writer stores and when: the pieces and lines of a Run as it is computed, queued here and
//! written by the host in order (`RunWriter::next`, then `commit_piece` or `commit_line`).

use std::collections::VecDeque;

use super::journal::{
    Base, Encoded, EndLine, ForkLine, ForkOrigin, Journal, Line, Piece, PieceLine, RecordKind, RunLine, SavedLine, State, Stored, Storing, Tables,
};
use super::records::{self, Tokens};
use crate::archive::{self, SAVED_POINT_VERSION};
use crate::checkpoint::sha256_hex;
use crate::error::{Error, Result};
use crate::plain::PlainRun;
use crate::sensitive::Guard;

/// When saved points are due: every `every` minutes (none when 0), and at the end of the clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Schedule {
    pub every: u64,
}

impl Schedule {
    /// How many ticks to run from `minute`: at most `ticks`, not past the next saved point (when
    /// there are any) nor past `target`.
    pub fn step(&self, minute: i64, ticks: u64, target: Option<i64>) -> u64 {
        let mut step = ticks;
        if self.every > 0 {
            let every = i128::from(self.every);
            let next = (i128::from(minute).div_euclid(every) + 1) * every;
            step = step.min(u64::try_from(next - i128::from(minute)).unwrap_or(u64::MAX));
        }
        if let Some(target) = target {
            step = step.min(u64::try_from(i128::from(target) - i128::from(minute)).unwrap_or(0));
        }
        step
    }

    /// Whether `minute` is one of the saved points' minutes (a multiple of `every`).
    pub fn is_point(&self, minute: i64) -> bool {
        self.every > 0 && i128::from(minute).rem_euclid(i128::from(self.every)) == 0
    }

    /// Whether a saved point is due at `minute` of a Run whose clock ends at `duration`, before the
    /// end (the end's is stored with the last blocks).
    pub fn due(&self, minute: i64, duration: i64) -> bool {
        self.is_point(minute) && minute < duration
    }
}

/// What is queued: a line without a piece, a piece with its bytes before compression, or a piece
/// of another stored Run whose compressed bytes the host copies as they are.
#[derive(Clone, Debug)]
enum Pending {
    Line(Line),
    Piece { piece: Piece, raw: Vec<u8>, sha256: String },
    Copy { piece: Piece, stored: Stored },
}

/// What the host writes next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next<'a> {
    /// A line without a piece: `commit_line`.
    Line,
    /// A piece of `kind`: the host compresses `raw`, appends it to data.pack, flushes it, then
    /// `commit_piece` with where it put it and its length.
    Piece { kind: &'static str, raw: &'a [u8] },
    /// A piece of `kind` copied from another stored Run: the host appends its `length` compressed
    /// bytes, as they are, to data.pack, flushes them, then `commit_piece`.
    Copy { kind: &'static str, length: u64 },
}

/// A fork, as the host gives it: where it was made, the EffectProgram applied (its JSON text), the
/// approval's hash and the receipt (its JSON text).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fork {
    pub parent_branch_id: String,
    pub minute: i64,
    pub origin: ForkOrigin,
    pub program: String,
    pub approved_program_hash: String,
    pub receipt: String,
}

/// The writer of a stored Run. It decides what to store and when; the host writes it: for each
/// queued item (`next`), a piece's compressed bytes to data.pack (flushed) and then its line
/// (`commit_piece`), or a line alone (`commit_line`), each line appended to the journal and
/// flushed before the next is written.
///
/// What it queues has passed the check of private data (`sensitive::Guard`, #368 decision 5): the
/// exported world, each saved point's world, the world at the end, the log's records, the metric
/// samples, the decision records and the LLM tape's entries. A check that fails is the call's
/// error, and nothing of that call is queued.
#[derive(Clone, Debug, Default)]
pub struct RunWriter {
    state: State,
    queue: VecDeque<Pending>,
    saved: Vec<SavedLine>,
    decisions: Tokens,
    /// The LLM tape's entries taken and not queued yet (`PlainRun::take_tape`, #428).
    tape: Vec<Vec<u8>>,
    guard: Guard,
}

impl RunWriter {
    /// The writer of a new Run.
    pub fn new() -> Self {
        Self::default()
    }

    /// The writer that goes on after the journal's line `head`, from what the Run had stored then.
    pub fn after(journal: &Journal, head: usize) -> Result<Self> {
        Ok(Self { state: journal.state_at(head)?, ..Self::new() })
    }

    /// The writer that appends conversation turns (`turns`) to a Run stored to its end: a turn is
    /// kept after the branch's head, which only a computation going on cuts away, and a Run stored
    /// to its end is not gone on with.
    pub fn after_end(journal: &Journal) -> Result<Self> {
        if journal.end().is_none() {
            return Err(Error::Value("a conversation turn is kept by a Run stored to its end".to_owned()));
        }
        Self::after(journal, journal.entries().len() - 1)
    }

    /// What the Run has stored (or queued): the log's and the samples' places and the counts.
    pub fn state(&self) -> State {
        self.state
    }

    /// The saved points' lines committed, in order.
    pub fn saved(&self) -> &[SavedLine] {
        &self.saved
    }

    /// Whether `run`, restored at the head the writer goes on after (`after`), holds what is
    /// stored: its tables' sizes and its blocks are the stored ones, every block sealed.
    pub fn fits(&self, run: &PlainRun) -> Result<bool> {
        let same = |place: &super::Place, tables: [usize; 2], blocks: [usize; 2]| {
            [tables[0], tables[1], blocks[0], blocks[1]].map(|count| count as u64) == [place.strings, place.shapes, place.blocks, place.blocks]
        };
        Ok(same(&self.state.log, run.log_table_counts()?, run.log_block_counts()?)
            && same(&self.state.samples, run.host.samples.table_counts(), run.host.samples.block_counts()))
    }

    /// Queues the start of a new Run of the exported world `export` (the world `world` of a file of
    /// named worlds), which `run` was made of: its "run" line and its "export" piece. An error where
    /// a record of the exported world's log is not packed, or the world holds private data (of a
    /// file of named worlds, the world `world`).
    pub fn start(&mut self, run: &PlainRun, export: Vec<u8>, world: Option<String>, storing: &Storing) -> Result<()> {
        if !run.log_all_packed()? {
            return Err(Error::Value("the exported world's log is not all packed".to_owned()));
        }
        match &world {
            // A file of named worlds (the fixtures') holds more than the world: its world is checked.
            Some(name) => {
                let entry = PlainRun::entry_of_worlds_json(&export, name)?;
                self.guard.check(&[&serde_json::to_vec(&entry).map_err(|error| Error::Value(error.to_string()))?])?;
            }
            None => self.guard.check(&[&export])?,
        }
        self.run_line(RunLine::new(&export, world, run.duration()?, storing));
        self.piece(Piece::Export, export);
        Ok(())
    }

    /// Queues the start of a Run from minute 0 of the same exported world as the stored Run
    /// `source`, computed with the same versions and stored as it is: its "run" line
    /// (`RunLine::same_start`) and `source`'s "export" piece, copied.
    pub fn same_start(&mut self, source: &Journal) -> Result<()> {
        let export = source.find("export").ok_or_else(|| Error::Value("the stored Run has no exported world".to_owned()))?;
        self.run_line(RunLine::same_start(source.run()?));
        self.copy(source, export)
    }

    /// Queues the start of the Run of a branch made by `fork` from the parent's stored Run `parent`
    /// (#306), which reads the parent's pieces up to the line `base` (#368, decision 6): the
    /// parent's saved point the fork's world was restored from, or its exported world. Its "run"
    /// line (`RunLine::forked`) and its "fork" line; then a saved point at the fork's minute, after
    /// the program was applied (`save`), is where the Run goes on from.
    pub fn fork_start(&mut self, parent: &Journal, base: usize, fork: Fork) -> Result<()> {
        let entry = parent.entries().get(base).ok_or_else(|| Error::Value(format!("the parent's stored Run has no line {base}")))?;
        let minute = match &entry.line {
            Line::Piece(PieceLine { piece: Piece::Saved(saved), .. }) => saved.minute,
            Line::Piece(PieceLine { piece: Piece::Export, .. }) => 0,
            _ => return Err(Error::Value("a fork goes on from a saved point of its parent's stored Run, or its exported world".to_owned())),
        };
        if minute > fork.minute {
            return Err(Error::Value("a fork goes on from a line of its parent at or before its minute".to_owned()));
        }
        let mut state = parent.state_at(base)?;
        state.counts.conversations = 0;
        let base = Base { line: base as u64, head: entry.head.clone(), minute, state };
        self.run_line(RunLine::forked(parent.run()?, fork.minute, &fork.parent_branch_id));
        self.queue.push_back(Pending::Line(Line::Fork(ForkLine {
            parent_branch_id: fork.parent_branch_id,
            minute: fork.minute,
            origin: fork.origin,
            base,
            program: fork.program,
            approved_program_hash: fork.approved_program_hash,
            receipt: fork.receipt,
        })));
        self.state = state;
        Ok(())
    }

    /// Queues the "run" line.
    pub fn run_line(&mut self, line: RunLine) {
        self.queue.push_back(Pending::Line(Line::Run(line)));
    }

    /// Queues a copy of the piece `index` of another stored Run (`journal`), its compressed bytes
    /// written as they are (the host has checked them).
    pub fn copy(&mut self, journal: &Journal, index: usize) -> Result<()> {
        let line = journal.piece(index)?;
        self.queue.push_back(Pending::Copy { piece: line.piece.clone(), stored: line.stored.clone() });
        Ok(())
    }

    fn piece(&mut self, piece: Piece, raw: Vec<u8>) {
        let sha256 = sha256_hex(&raw);
        self.queue.push_back(Pending::Piece { piece, raw, sha256 });
    }

    /// Takes the decision records `run` collected since the last call (`PlainRun::take_decisions`;
    /// none where it does not collect them) and the LLM tape's entries (`PlainRun::take_tape`; none
    /// without an LLM), and queues the tables' new entries and the newly sealed blocks (every block
    /// when `last`) of the log and of the samples of `run`. Call it between two ticks. The decision
    /// records and the tape's entries wait for the next saved point or the end (`save`, `finish`),
    /// and go into one piece of each there.
    pub fn take(&mut self, run: &mut PlainRun, last: bool) -> Result<()> {
        let decisions = run.take_decisions();
        let tape = run.take_tape();
        let refs: Vec<&[u8]> = decisions.iter().chain(&tape).map(Vec::as_slice).collect();
        self.guard.check(&refs)?;
        for record in &decisions {
            self.decisions.push_json(record)?;
        }
        self.tape.extend(tape);
        let log_end = Self::end(run.log_block_counts()?, last);
        let sample_end = Self::end(run.host.samples.block_counts(), last);
        for index in self.state.log.blocks..log_end {
            let records = run.log_block_json(index as usize)?;
            self.guard.check(&records.iter().map(|record| &record[..]).collect::<Vec<_>>())?;
        }
        for index in self.state.samples.blocks..sample_end {
            let lines = run.host.samples.block_lines(index as usize)?;
            self.guard.check(&lines.iter().map(Vec::as_slice).collect::<Vec<_>>())?;
        }
        if log_end > self.state.log.blocks {
            let [strings, shapes] = run.log_table_counts()?.map(|count| count as u64);
            let tables = Tables { strings_from: self.state.log.strings, shapes_from: self.state.log.shapes, strings, shapes };
            let raw = run.log_tables(tables.strings_from as usize, tables.shapes_from as usize)?;
            self.piece(Piece::LogTables(tables), raw);
            (self.state.log.strings, self.state.log.shapes) = (strings, shapes);
            for index in self.state.log.blocks..log_end {
                let raw = run.log_block(index as usize)?;
                self.piece(Piece::LogBlock { index }, raw);
            }
            self.state.log.blocks = log_end;
        }
        let samples = &run.host.samples;
        if sample_end > self.state.samples.blocks {
            let [strings, shapes] = samples.table_counts().map(|count| count as u64);
            let tables = Tables { strings_from: self.state.samples.strings, shapes_from: self.state.samples.shapes, strings, shapes };
            let raw = samples.tables(tables.strings_from as usize, tables.shapes_from as usize);
            self.piece(Piece::SampleTables(tables), raw);
            (self.state.samples.strings, self.state.samples.shapes) = (strings, shapes);
            for index in self.state.samples.blocks..sample_end {
                let raw = samples.block(index as usize);
                self.piece(Piece::SampleBlock { index }, raw);
            }
            self.state.samples.blocks = sample_end;
        }
        Ok(())
    }

    /// The blocks to have stored: the sealed ones, or every one when `last`.
    fn end([blocks, sealed]: [usize; 2], last: bool) -> u64 {
        (if last { blocks } else { sealed }) as u64
    }

    /// Queues the decision records and the tape's entries taken since the last pieces of them, as
    /// one piece of each.
    fn queue_decisions(&mut self) -> Result<()> {
        let count = self.decisions.count();
        if count > 0 {
            let raw = std::mem::take(&mut self.decisions).finish();
            RecordKind::Decisions.add(&mut self.state.counts, count)?;
            self.piece(Piece::Records { kind: RecordKind::Decisions, count }, raw);
        }
        let tape = std::mem::take(&mut self.tape);
        self.tape(&tape)
    }

    /// Queues a saved point of `run` now (#251), between two ticks: the blocks are sealed first and
    /// stored, and the decision records taken, so that everything the point needs is stored before
    /// its line (#267; always, #368 decision 2): the point can be restored, and gone on from, at
    /// once.
    pub fn save(&mut self, run: &mut PlainRun) -> Result<()> {
        run.seal_stores()?;
        self.take(run, false)?;
        let point = run.saved_point()?;
        self.guard.check(&archive::world_part_jsons(&point.world)?)?;
        self.queue_decisions()?;
        let saved = SavedLine {
            minute: point.minute,
            records: point.records,
            samples: point.samples,
            counts: self.state.counts,
            point_version: SAVED_POINT_VERSION,
            log_blocks: self.state.log.blocks,
            sample_blocks: self.state.samples.blocks,
        };
        if run.log_block_counts()?[0] as u64 != saved.log_blocks || run.host.samples.block_counts()[0] as u64 != saved.sample_blocks {
            return Err(Error::Value("a saved point whose blocks are not all stored".to_owned()));
        }
        self.piece(Piece::Saved(saved), point.to_bytes());
        Ok(())
    }

    /// Queues the last pieces of a Run stored to its end: the decision records not stored yet, the
    /// world at its last minute without its log, and the "end" line with the digests the Run gave.
    /// Call it after every block is taken (`take` with `last`). An error where a record of the log
    /// is not packed (the stored blocks would not hold the whole log), or a block is not stored.
    pub fn finish(&mut self, run: &mut PlainRun) -> Result<()> {
        if !run.log_all_packed()? {
            return Err(Error::Value("a record of the log is not packed".to_owned()));
        }
        self.take(run, true)?;
        let world = run.world_parts()?;
        self.guard.check(&archive::world_part_jsons(&world)?)?;
        self.queue_decisions()?;
        let end = EndLine {
            minute: run.minute()?,
            records: run.log_records()? as u64,
            samples: run.host.samples.len() as u64,
            counts: self.state.counts,
            parts: run.digests()?.into_iter().map(|(part, digest)| (part.to_owned(), digest)).collect(),
            metrics: run.host.metrics_digest(),
        };
        self.piece(Piece::World, world);
        self.queue.push_back(Pending::Line(Line::End(end)));
        Ok(())
    }

    /// Queues a piece of the LLM tape's entries `entries` (JSON texts), as they are: what `take`
    /// took of a Run with an LLM goes this way at a saved point and at the end.
    pub fn tape(&mut self, entries: &[Vec<u8>]) -> Result<()> {
        self.records(RecordKind::Tape, entries)
    }

    /// Queues a piece of conversation turns `turns` (JSON texts), as they are: of a writer
    /// `after_end`.
    pub fn turns(&mut self, turns: &[Vec<u8>]) -> Result<()> {
        self.records(RecordKind::Conversations, turns)
    }

    fn records(&mut self, kind: RecordKind, texts: &[Vec<u8>]) -> Result<()> {
        if texts.is_empty() {
            return Ok(());
        }
        for text in texts {
            serde_json::from_slice::<serde_json::Value>(text).map_err(|_| Error::Value(format!("{}: a record that is not JSON", kind.name())))?;
        }
        kind.add(&mut self.state.counts, texts.len() as u64)?;
        self.piece(Piece::Records { kind, count: texts.len() as u64 }, records::texts(texts));
        Ok(())
    }

    /// What the host writes next, or None when everything queued is written.
    pub fn next(&self) -> Option<Next<'_>> {
        self.queue.front().map(|pending| match pending {
            Pending::Line(_) => Next::Line,
            Pending::Piece { piece, raw, .. } => Next::Piece { kind: piece.kind(), raw },
            Pending::Copy { piece, stored } => Next::Copy { kind: piece.kind(), length: stored.length },
        })
    }

    /// The line of the line without a piece queued next, to append to the journal.
    pub fn commit_line(&mut self) -> Result<Encoded> {
        match self.queue.front() {
            Some(Pending::Line(line)) => {
                let encoded = line.encode();
                self.queue.pop_front();
                Ok(encoded)
            }
            _ => Err(Error::Value("the writer's next item is not a line without a piece".to_owned())),
        }
    }

    /// The line of the piece queued next, which the host wrote at `offset` of data.pack, `length`
    /// bytes compressed, to append to the journal.
    pub fn commit_piece(&mut self, offset: u64, length: u64) -> Result<Encoded> {
        let line = match self.queue.front() {
            Some(Pending::Piece { piece, raw, sha256 }) => {
                PieceLine { piece: piece.clone(), stored: Stored { offset, length, raw_length: raw.len() as u64, sha256: sha256.clone() } }
            }
            Some(Pending::Copy { piece, stored }) => {
                if stored.length != length {
                    return Err(Error::Value(format!("{}: the copied piece is {length} bytes, not {}", piece.kind(), stored.length)));
                }
                PieceLine { piece: piece.clone(), stored: Stored { offset, ..stored.clone() } }
            }
            _ => return Err(Error::Value("the writer's next item is not a piece".to_owned())),
        };
        self.queue.pop_front();
        if let Piece::Saved(saved) = &line.piece {
            self.saved.push(*saved);
        }
        Ok(Line::Piece(line).encode())
    }
}
