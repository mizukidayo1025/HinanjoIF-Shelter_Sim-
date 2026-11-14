//! What a writer stores and when: the pieces and lines of a Run as it is computed, queued here and
//! written by the host in order (`RunWriter::next`, then `commit_piece` or `commit_line`).

use std::collections::VecDeque;

use super::journal::{Encoded, EndLine, ForkLine, Journal, Line, Piece, PieceLine, RunLine, SavedLine, Stored, Tables};
use crate::archive::SAVED_POINT_VERSION;
use crate::checkpoint::sha256_hex;
use crate::error::{Error, Result};
use crate::plain::PlainRun;

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

/// How much of a store (the log's or the samples') is stored: its tables' sizes the last tables
/// piece gave, and the blocks stored.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Place {
    pub strings: u64,
    pub shapes: u64,
    pub blocks: u64,
}

/// What is queued: a line without a piece, a piece with its bytes before compression, or a piece
/// of another stored Run whose compressed bytes the host copies as they are.
#[derive(Clone, Debug)]
enum Pending {
    Line(Line),
    Piece { piece: Piece, raw: Vec<u8>, sha256: String },
    Copy { piece: Piece, stored: super::Stored },
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

/// The writer of a stored Run. It decides what to store and when; the host writes it: for each
/// queued item (`next`), a piece's compressed bytes to data.pack (flushed) and then its line
/// (`commit_piece`), or a line alone (`commit_line`), each line appended to the journal and
/// flushed before the next is written.
#[derive(Clone, Debug, Default)]
pub struct RunWriter {
    log: Place,
    samples: Place,
    queue: VecDeque<Pending>,
    saved: Vec<SavedLine>,
}

impl RunWriter {
    /// The writer of a new Run.
    pub fn new() -> Self {
        Self::default()
    }

    /// The writer that goes on after the journal's lines up to `head` (its line's index): the
    /// tables' sizes the last tables lines give and the blocks stored, of the log and of the
    /// samples.
    pub fn after(journal: &Journal, head: usize) -> Result<Self> {
        let entries = journal.entries().get(..=head).ok_or_else(|| Error::Value(format!("the journal has no line {head}")))?;
        let mut writer = Self::new();
        for entry in entries {
            let Line::Piece(line) = &entry.line else { continue };
            match &line.piece {
                Piece::LogTables(tables) => (writer.log.strings, writer.log.shapes) = (tables.strings, tables.shapes),
                Piece::SampleTables(tables) => (writer.samples.strings, writer.samples.shapes) = (tables.strings, tables.shapes),
                Piece::LogBlock { .. } => writer.log.blocks += 1,
                Piece::SampleBlock { .. } => writer.samples.blocks += 1,
                _ => {}
            }
        }
        Ok(writer)
    }

    /// How much of the log and of the samples is stored (or queued).
    pub fn places(&self) -> (Place, Place) {
        (self.log, self.samples)
    }

    /// The saved points' lines committed, in order.
    pub fn saved(&self) -> &[SavedLine] {
        &self.saved
    }

    /// Whether `run`, restored at the head the writer goes on after (`after`), holds what is
    /// stored: its tables' sizes and its blocks are the stored ones, every block sealed.
    pub fn fits(&self, run: &PlainRun) -> Result<bool> {
        let same = |place: &Place, tables: [usize; 2], blocks: [usize; 2]| {
            [tables[0], tables[1], blocks[0], blocks[1]].map(|count| count as u64) == [place.strings, place.shapes, place.blocks, place.blocks]
        };
        Ok(same(&self.log, run.log_table_counts()?, run.log_block_counts()?)
            && same(&self.samples, run.host.samples.table_counts(), run.host.samples.block_counts()))
    }

    /// Queues the start of a new Run of the exported world `export` (the world `world` of a file of
    /// named worlds), which `run` was made of: its "run" line and its "export" piece. An error where
    /// a record of the exported world's log is not packed.
    pub fn start(&mut self, run: &PlainRun, export: Vec<u8>, world: Option<String>, save_every: u64) -> Result<()> {
        if !run.log_all_packed()? {
            return Err(Error::Value("the exported world's log is not all packed".to_owned()));
        }
        self.run_line(RunLine::new(&export, world, run.duration()?, save_every));
        self.export(export);
        Ok(())
    }

    /// Queues the start of a Run from minute 0 of the same exported world as the stored Run
    /// `source`, saved as often: its "run" line (`RunLine::same_start`) and `source`'s "export"
    /// piece, copied.
    pub fn same_start(&mut self, source: &Journal) -> Result<()> {
        let export = source.find("export").ok_or_else(|| Error::Value("the stored Run has no exported world".to_owned()))?;
        self.run_line(RunLine::same_start(source.run()?));
        self.copy(source, export)
    }

    /// Queues the start of the Run of a branch made by `fork` from the parent's stored Run
    /// `parent` (#306): its "run" line (`RunLine::forked`), the "fork" line and the parent's
    /// "export" piece, copied. Then a saved point at the fork's minute, after the program was
    /// applied (`save` with `seal`), is where the Run goes on from.
    pub fn fork_start(&mut self, parent: &Journal, fork: ForkLine) -> Result<()> {
        let export = parent.find("export").ok_or_else(|| Error::Value("the parent's stored Run has no exported world".to_owned()))?;
        self.run_line(RunLine::forked(parent.run()?, fork.minute, &fork.parent_branch_id));
        self.fork_line(fork);
        self.copy(parent, export)
    }

    /// Queues the "run" line.
    pub fn run_line(&mut self, line: RunLine) {
        self.queue.push_back(Pending::Line(Line::Run(line)));
    }

    /// Queues the "fork" line of a branch made by a fork (#306), after its "run" line.
    pub fn fork_line(&mut self, line: ForkLine) {
        self.queue.push_back(Pending::Line(Line::Fork(line)));
    }

    /// Queues the "export" piece: the exported world's bytes.
    pub fn export(&mut self, raw: Vec<u8>) {
        self.piece(Piece::Export, raw);
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

    /// Queues the tables' new entries and the newly sealed blocks (every block when `last`) of the
    /// log and of the samples of `run`. Call it between two ticks.
    pub fn take(&mut self, run: &PlainRun, last: bool) -> Result<()> {
        let log_end = Self::end(run.log_block_counts()?, last);
        if log_end > self.log.blocks {
            let [strings, shapes] = run.log_table_counts()?.map(|count| count as u64);
            let tables = Tables { strings_from: self.log.strings, shapes_from: self.log.shapes, strings, shapes };
            let raw = run.log_tables(tables.strings_from as usize, tables.shapes_from as usize)?;
            self.piece(Piece::LogTables(tables), raw);
            (self.log.strings, self.log.shapes) = (strings, shapes);
            for index in self.log.blocks..log_end {
                let raw = run.log_block(index as usize)?;
                self.piece(Piece::LogBlock { index }, raw);
            }
            self.log.blocks = log_end;
        }
        let samples = &run.host.samples;
        let sample_end = Self::end(samples.block_counts(), last);
        if sample_end > self.samples.blocks {
            let [strings, shapes] = samples.table_counts().map(|count| count as u64);
            let tables = Tables { strings_from: self.samples.strings, shapes_from: self.samples.shapes, strings, shapes };
            let raw = samples.tables(tables.strings_from as usize, tables.shapes_from as usize);
            self.piece(Piece::SampleTables(tables), raw);
            (self.samples.strings, self.samples.shapes) = (strings, shapes);
            for index in self.samples.blocks..sample_end {
                let raw = samples.block(index as usize);
                self.piece(Piece::SampleBlock { index }, raw);
            }
            self.samples.blocks = sample_end;
        }
        Ok(())
    }

    /// The blocks to have stored: the sealed ones, or every one when `last`.
    fn end([blocks, sealed]: [usize; 2], last: bool) -> u64 {
        (if last { blocks } else { sealed }) as u64
    }

    /// Queues a saved point of `run` now (#251), after the tables and blocks sealed so far. Call it
    /// between two ticks. With `seal` (#267) the blocks are sealed first, so every block the point
    /// needs is stored before its line, and it can be restored, and gone on from, at once.
    pub fn save(&mut self, run: &mut PlainRun, seal: bool) -> Result<()> {
        if seal {
            run.seal_stores()?;
        }
        self.take(run, false)?;
        let point = run.saved_point()?;
        let saved = SavedLine {
            minute: point.minute,
            records: point.records,
            samples: point.samples,
            point_version: SAVED_POINT_VERSION,
            log_blocks: run.log_block_counts()?[0] as u64,
            sample_blocks: run.host.samples.block_counts()[0] as u64,
        };
        self.piece(Piece::Saved(saved), point.to_bytes());
        Ok(())
    }

    /// Queues the last pieces of a Run stored to its end: the world at its last minute without its
    /// log, and the "end" line with the digests the Run gave. Call it after every block is taken
    /// (`take` with `last`). An error where a record of the log is not packed (the stored blocks
    /// would not hold the whole log).
    pub fn finish(&mut self, run: &PlainRun) -> Result<()> {
        if !run.log_all_packed()? {
            return Err(Error::Value("a record of the log is not packed".to_owned()));
        }
        let end = EndLine {
            minute: run.minute()?,
            records: run.log_records()? as u64,
            samples: run.host.samples.len() as u64,
            parts: run.digests()?.into_iter().map(|(part, digest)| (part.to_owned(), digest)).collect(),
            metrics: run.host.metrics_digest(),
        };
        self.piece(Piece::World, run.world_parts()?);
        self.queue.push_back(Pending::Line(Line::End(end)));
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
