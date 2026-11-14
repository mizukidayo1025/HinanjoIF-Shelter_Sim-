//! What a reader reads of a stored Run: its heads and where to cut it back to one (a journal
//! alone), and through the chain of a fork's Run and its parents' (`Chain`), where to restore it
//! from to reach a minute, the pieces a restore needs, and its pieces read back to the digests its
//! "end" line holds (`Verify`).

use super::journal::{Journal, Line, Piece, PieceLine, RecordKind, SavedLine, Stored};
use super::{corrupt, records};
use crate::archive::{self, LogReader, Restore, SampleReader};
use crate::checkpoint::sha256_hex;
use crate::error::{Error, Result};
use crate::json::Encoder;

/// What a head is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeadKind {
    Export,
    Saved,
    End,
}

impl HeadKind {
    pub fn name(self) -> &'static str {
        match self {
            HeadKind::Export => "export",
            HeadKind::Saved => "saved",
            HeadKind::End => "end",
        }
    }
}

/// A line a branch's head may be: the journal's line `index`, at `minute`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Head {
    pub kind: HeadKind,
    pub minute: i64,
    pub index: usize,
}

/// A stored Run cut back to a head: the lines kept (up to the head's, `head.index` + 1), the
/// journal's bytes they take (`journal_end`) and the bytes of data.pack their pieces take
/// (`data_end`). The host truncates the files to these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cut {
    pub head: Head,
    pub journal_end: u64,
    pub data_end: u64,
}

/// Where a seek to `minute` starts: the leaf's line `base` (a "saved" line, or the "export" line of
/// a Run from minute 0) restored, then the ticks up to `minute` computed. `from` is the saved line
/// (None from the exported world), `export` the exported world's piece in the chain
/// (`Chain::pieces`), `from_minute` the minute it is computed from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seek {
    pub minute: i64,
    pub from: Option<usize>,
    pub base: usize,
    pub export: usize,
    pub from_minute: i64,
}

impl Journal {
    /// The minute the Run starts at: the fork's of a branch made by a fork, else 0.
    pub fn fork_minute(&self) -> Result<i64> {
        Ok(self.run()?.fork_minute())
    }

    /// The minute the Run's clock is complete at.
    pub fn duration(&self) -> Result<i64> {
        Ok(self.run()?.duration_minutes)
    }

    /// The lines a branch's head may be, in order: the "export" line of a Run from minute 0, every
    /// "saved" line (each is sealed: the blocks it needs are stored before it) and the "end" line.
    pub fn heads(&self) -> Vec<Head> {
        self.entries()
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| match &entry.line {
                Line::Piece(PieceLine { piece: Piece::Export, .. }) => Some(Head { kind: HeadKind::Export, minute: 0, index }),
                Line::Piece(PieceLine { piece: Piece::Saved(saved), .. }) => Some(Head { kind: HeadKind::Saved, minute: saved.minute, index }),
                Line::End(end) => Some(Head { kind: HeadKind::End, minute: end.minute, index }),
                _ => None,
            })
            .collect()
    }

    /// The head whose line's head hash is `hash`.
    pub fn head(&self, hash: &str) -> Option<Head> {
        self.heads().into_iter().find(|head| self.entries()[head.index].head == hash)
    }

    /// The stored Run cut back to the head `hash`: an error where no head's line has it, or where a
    /// branch forked from this one reads its pieces up to a later line (`kept_through`: the latest
    /// base line of its forks), which the cut would take away from it.
    pub fn cut(&self, hash: &str, kept_through: Option<usize>) -> Result<Cut> {
        let head = self.head(hash).ok_or_else(|| Error::Value("the branch's head is not a line its stored Run can go on from".to_owned()))?;
        if kept_through.is_some_and(|line| line > head.index) {
            return Err(Error::Value("a branch forked from this one reads its stored Run past the branch's head".to_owned()));
        }
        let mut data_end = 0u64;
        for entry in &self.entries()[..=head.index] {
            if let Line::Piece(line) = &entry.line {
                data_end = data_end.max(line.stored.offset.checked_add(line.stored.length).ok_or_else(|| corrupt("a piece past any file"))?);
            }
        }
        Ok(Cut { head, journal_end: self.entries()[head.index].end, data_end })
    }

    /// The "saved" lines, in order: each is a point a seek can restore.
    pub fn saved_points(&self) -> Vec<usize> {
        self.heads().into_iter().filter(|head| head.kind == HeadKind::Saved).map(|head| head.index).collect()
    }

    /// The "saved" line `index`'s fields.
    pub fn saved(&self, index: usize) -> Result<&SavedLine> {
        match &self.piece(index)?.piece {
            Piece::Saved(saved) => Ok(saved),
            other => Err(Error::Value(format!("the journal's line {index} is not a saved point but {}", other.kind()))),
        }
    }

    /// The conversation turns' pieces of this branch, in order.
    pub fn turns(&self) -> Vec<usize> {
        (0..self.entries().len())
            .filter(|index| {
                matches!(&self.entries()[*index].line, Line::Piece(PieceLine { piece: Piece::Records { kind: RecordKind::Conversations, .. }, .. }))
            })
            .collect()
    }
}

/// A piece of a chain: the piece line `index` of the journal at `level` (0: the Run the chain starts
/// at, from minute 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct At {
    pub level: usize,
    pub index: usize,
}

/// A stored Run with the stored Runs it reads pieces of (#368, decision 6): a branch made by a fork
/// reads its parent's pieces up to its base line (its "fork" line's `base`), and the parent its
/// own parent's, down to a Run from minute 0, which holds the exported world. `Chain::new` takes the
/// branch's journal; while `parent_needed` names a branch, the host reads that branch's journal and
/// gives it (`add_parent`), which is checked to hold the base as it was. Then the chain's pieces are,
/// in order, each parent's up to its child's base (its conversation turns left out: they are its
/// own) and the branch's (`pieces`), and every reader's position is one of them.
#[derive(Clone, Debug)]
pub struct Chain {
    /// The journals, the Run from minute 0 first, the branch's last.
    levels: Vec<Journal>,
    pieces: Vec<At>,
}

impl Chain {
    /// The chain of `leaf`, the branch's journal: an error for a journal without its "run" line.
    pub fn new(leaf: Journal) -> Result<Self> {
        leaf.run()?;
        let mut chain = Self { levels: vec![leaf], pieces: Vec::new() };
        chain.index();
        Ok(chain)
    }

    /// The branch whose journal the chain needs next (the parent of its first level), or None once
    /// it reaches a Run from minute 0.
    pub fn parent_needed(&self) -> Option<&str> {
        self.levels[0].run().ok().and_then(|run| run.fork.as_ref()).map(|fork| fork.parent_branch_id.as_str())
    }

    /// Whether the chain reaches a Run from minute 0.
    pub fn complete(&self) -> bool {
        self.parent_needed().is_none()
    }

    /// Adds the journal of the branch `parent_needed` names: an error where it is not the Run the
    /// fork was made of (another computation), or no longer holds the fork's base line as it was:
    /// that line's head hash and what the parent had stored up to it.
    pub fn add_parent(&mut self, parent: Journal) -> Result<()> {
        let child = &self.levels[0];
        let (Some(_), Some(fork)) = (self.parent_needed(), child.fork()) else {
            return Err(Error::Value("the stored Run is not a fork's: it reads no parent's".to_owned()));
        };
        let missing = || Error::Value(format!("the stored Run of branch {} no longer holds the line a fork of it goes on from", fork.parent_branch_id));
        if !parent.run()?.same_run(child.run()?) {
            return Err(Error::Value(format!("the stored Run of branch {} is not the one the fork was made of", fork.parent_branch_id)));
        }
        let line = usize::try_from(fork.base.line).map_err(|_| missing())?;
        let entry = parent.entries().get(line).ok_or_else(missing)?;
        let minute = match &entry.line {
            Line::Piece(PieceLine { piece: Piece::Saved(saved), .. }) => saved.minute,
            Line::Piece(PieceLine { piece: Piece::Export, .. }) => 0,
            _ => return Err(missing()),
        };
        let mut state = parent.state_at(line)?;
        state.counts.conversations = 0;
        if entry.head != fork.base.head || minute != fork.base.minute || state != fork.base.state {
            return Err(missing());
        }
        self.levels.insert(0, parent);
        self.index();
        Ok(())
    }

    fn index(&mut self) {
        let last = self.levels.len() - 1;
        self.pieces.clear();
        for (level, journal) in self.levels.iter().enumerate() {
            let end = match level == last {
                true => journal.entries().len(),
                false => self.levels[level + 1].fork().map_or(0, |fork| fork.base.line as usize + 1),
            };
            for (index, entry) in journal.entries()[..end.min(journal.entries().len())].iter().enumerate() {
                match &entry.line {
                    Line::Piece(PieceLine { piece: Piece::Records { kind: RecordKind::Conversations, .. }, .. }) if level != last => {}
                    Line::Piece(_) => self.pieces.push(At { level, index }),
                    _ => {}
                }
            }
        }
    }

    /// The branch's journal.
    pub fn leaf(&self) -> &Journal {
        &self.levels[self.levels.len() - 1]
    }

    /// The journals, the Run from minute 0 first.
    pub fn levels(&self) -> &[Journal] {
        &self.levels
    }

    /// The chain's pieces, in order (see `Chain`).
    pub fn pieces(&self) -> &[At] {
        &self.pieces
    }

    fn require_complete(&self) -> Result<()> {
        match self.parent_needed() {
            Some(parent) => Err(Error::Value(format!("the stored Run of a fork needs its parent's (branch {parent})"))),
            None => Ok(()),
        }
    }

    /// The piece at `position` of `pieces`, with where it is.
    pub fn piece(&self, position: usize) -> Result<(&PieceLine, At)> {
        let at = *self.pieces.get(position).ok_or_else(|| Error::Value(format!("the chain has no piece {position}")))?;
        Ok((self.levels[at.level].piece(at.index)?, at))
    }

    /// Where the piece at `position` is: its level and its place in that level's data.pack.
    pub fn stored(&self, position: usize) -> Result<(usize, &Stored)> {
        let (line, at) = self.piece(position)?;
        Ok((at.level, &line.stored))
    }

    /// An error where `length` bytes were read for the piece at `position`.
    pub fn check_stored(&self, position: usize, length: u64) -> Result<()> {
        let (_, at) = self.piece(position)?;
        self.levels[at.level].check_stored(at.index, length)
    }

    /// An error where `raw`, the piece at `position` decompressed, is not the bytes written.
    pub fn check_raw(&self, position: usize, raw: &[u8]) -> Result<()> {
        let (_, at) = self.piece(position)?;
        self.levels[at.level].check_raw(at.index, raw)
    }

    /// The exported world's piece: the first level's "export" piece.
    pub fn export(&self) -> Result<usize> {
        self.require_complete()?;
        self.pieces
            .iter()
            .position(|at| at.level == 0 && matches!(self.levels[0].entries()[at.index].line, Line::Piece(PieceLine { piece: Piece::Export, .. })))
            .ok_or_else(|| Error::Value("the stored Run has no exported world".to_owned()))
    }

    /// Where to start to reach `minute`: the branch's nearest saved point at or before it, or the
    /// exported world at minute 0 when there is none in a Run from minute 0. An error for a minute
    /// out of the Run (before its fork's, after its clock's end), and for a fork's Run without a
    /// saved point to start from.
    pub fn seek(&self, minute: i64) -> Result<Seek> {
        let leaf = self.leaf();
        let (first, duration) = (leaf.fork_minute()?, leaf.duration()?);
        if minute < first || minute > duration {
            return Err(Error::Value(format!("minute {minute} is not one of the Run ({first} to {duration})")));
        }
        let export = self.export()?;
        let mut from = None;
        for index in leaf.saved_points() {
            let saved = leaf.saved(index)?;
            if saved.minute <= minute {
                from = Some((index, saved.minute));
            }
        }
        let base = match from {
            Some((index, _)) => index,
            None if first == 0 => leaf.find("export").ok_or_else(|| Error::Value("the stored Run has no exported world".to_owned()))?,
            None => return Err(Error::Value(format!("the Run of a fork has no saved point at or before minute {minute} to start from"))),
        };
        Ok(Seek { minute, from: from.map(|(index, _)| index), base, export, from_minute: from.map_or(0, |(_, minute)| minute) })
    }

    /// The next piece, from `start` on, that `restore` needs: a log piece while it needs records, a
    /// sample piece while it needs samples; None once it needs neither.
    pub fn next_restore_read(&self, start: usize, restore: &Restore) -> Result<Option<usize>> {
        self.require_complete()?;
        let (log, samples) = (restore.log_needed(), restore.samples_needed());
        if !log && !samples {
            return Ok(None);
        }
        Ok((start..self.pieces.len()).find(|position| {
            let at = self.pieces[*position];
            matches!(&self.levels[at.level].entries()[at.index].line, Line::Piece(line) if (log && line.piece.is_log()) || (samples && line.piece.is_samples()))
        }))
    }

    /// Gives `restore` the piece at `position`, decompressed (`raw`): checked against its line, then
    /// read as the log's or the samples' tables or block.
    pub fn restore_read(&self, position: usize, raw: &[u8], restore: &mut Restore) -> Result<()> {
        self.check_raw(position, raw)?;
        match &self.piece(position)?.0.piece {
            Piece::LogTables(_) => restore.read_log_tables(raw),
            Piece::LogBlock { .. } => restore.read_log_block(raw).map(|_| ()),
            Piece::SampleTables(_) => restore.read_sample_tables(raw),
            Piece::SampleBlock { .. } => restore.read_sample_block(raw).map(|_| ()),
            other => Err(Error::Value(format!("a restore reads the log and the samples, not {}", other.kind()))),
        }
    }

    /// The samples' pieces, in order: what the KPI history reads.
    pub fn sample_pieces(&self) -> Result<Vec<usize>> {
        self.require_complete()?;
        Ok(self.positions(|piece| piece.is_samples()))
    }

    /// The pieces of the records of `kind`, in order: the decision records and the tape's entries
    /// from the Run's start, the conversation turns of the branch itself.
    pub fn records(&self, kind: RecordKind) -> Result<Vec<usize>> {
        self.require_complete()?;
        Ok(self.positions(|piece| matches!(piece, Piece::Records { kind: other, .. } if *other == kind)))
    }

    fn positions(&self, wanted: impl Fn(&Piece) -> bool) -> Vec<usize> {
        (0..self.pieces.len())
            .filter(|position| {
                let at = self.pieces[*position];
                matches!(&self.levels[at.level].entries()[at.index].line, Line::Piece(line) if wanted(&line.piece))
            })
            .collect()
    }
}

/// A stored Run read back to its digests: the chain's pieces (`Verify::reads`: the parents' saved
/// points left out), each given decompressed (`read`), then compared with the branch's "end" line
/// (`result`).
#[derive(Clone, Debug, Default)]
pub struct Verify {
    log: LogReader,
    samples: SampleReader,
    world: Option<Vec<(String, String)>>,
    export_sha256: Option<String>,
    saved_points: u64,
    records: [u64; 3],
}

/// What reading a stored Run back gave: whether it is complete (it has its "end" line), whether
/// the journal was torn, whether everything matched its "end" line and what did not (`export`, a
/// part's name, `parts`, `records`, `metrics`), the digests read (each part's, the log's
/// `event_log` with them, and the samples'), the counts (of records, samples, decision records,
/// conversation turns and the tape's entries), and how many of its saved points it read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verified {
    pub complete: bool,
    pub torn: bool,
    pub verified: bool,
    pub mismatches: Vec<String>,
    pub parts: Option<Vec<(String, String)>>,
    pub metrics: String,
    pub records: u64,
    pub samples: u64,
    pub decisions: u64,
    pub conversations: u64,
    pub tape: u64,
    pub saved_points: u64,
}

impl Verified {
    /// As a JSON object, its fields by their names.
    pub fn to_json(&self) -> Vec<u8> {
        let mut out = Encoder::with_capacity(2048);
        out.out.extend_from_slice(b"{\"complete\":");
        out.bool(self.complete);
        out.out.extend_from_slice(b",\"torn\":");
        out.bool(self.torn);
        out.out.extend_from_slice(b",\"verified\":");
        out.bool(self.verified);
        out.out.extend_from_slice(b",\"mismatches\":[");
        for (index, mismatch) in self.mismatches.iter().enumerate() {
            if index > 0 {
                out.out.push(b',');
            }
            out.string(mismatch);
        }
        out.out.extend_from_slice(b"],\"parts\":");
        match &self.parts {
            None => out.null(),
            Some(parts) => {
                out.out.push(b'{');
                for (index, (part, digest)) in parts.iter().enumerate() {
                    if index > 0 {
                        out.out.push(b',');
                    }
                    out.string(part);
                    out.out.push(b':');
                    out.string(digest);
                }
                out.out.push(b'}');
            }
        }
        out.out.extend_from_slice(b",\"metrics\":");
        out.string(&self.metrics);
        for (name, count) in [
            ("records", self.records),
            ("samples", self.samples),
            ("decisions", self.decisions),
            ("conversations", self.conversations),
            ("tape", self.tape),
            ("saved_points", self.saved_points),
        ] {
            out.out.push(b',');
            out.string(name);
            out.out.push(b':');
            out.out.extend_from_slice(itoa::Buffer::new().format(count).as_bytes());
        }
        out.out.push(b'}');
        out.out
    }
}

impl Verify {
    pub fn new() -> Self {
        Self::default()
    }

    /// The pieces to read, in order: the chain's, but its parents' saved points.
    pub fn reads(chain: &Chain) -> Result<Vec<usize>> {
        chain.require_complete()?;
        let last = chain.levels.len() - 1;
        Ok((0..chain.pieces.len())
            .filter(|position| {
                let at = chain.pieces[*position];
                at.level == last || !matches!(chain.levels[at.level].entries()[at.index].line, Line::Piece(PieceLine { piece: Piece::Saved(_), .. }))
            })
            .collect())
    }

    /// Reads the piece at `position`, decompressed (`raw`): an error for bytes that are not the ones
    /// written, or that are not such a piece (a saved point not of its line's minute and counts, a
    /// records piece of another count).
    pub fn read(&mut self, chain: &Chain, position: usize, raw: &[u8]) -> Result<()> {
        chain.check_raw(position, raw)?;
        match &chain.piece(position)?.0.piece {
            Piece::Export => self.export_sha256 = Some(sha256_hex(raw)),
            Piece::LogTables(_) => self.log.read_tables(raw)?,
            Piece::LogBlock { .. } => {
                self.log.read_block(raw)?;
            }
            Piece::SampleTables(_) => self.samples.read_tables(raw)?,
            Piece::SampleBlock { .. } => {
                self.samples.read_block(raw)?;
            }
            Piece::Records { kind, count } => {
                if records::count(raw)? != *count {
                    return Err(Error::Value(format!("{}: the piece does not hold the records its line says", kind.name())));
                }
                let slot = RecordKind::ALL.iter().position(|each| each == kind).unwrap_or_default();
                self.records[slot] += count;
            }
            Piece::World => self.world = Some(archive::world_part_digests(raw)?),
            Piece::Saved(saved) => {
                let point = archive::SavedPoint::of_bytes(raw)?;
                if point.minute != saved.minute || point.records != saved.records || point.samples != saved.samples {
                    return Err(Error::Value(format!("saved: the point is not the one its line says (minute {})", saved.minute)));
                }
                self.saved_points += 1;
            }
        }
        Ok(())
    }

    /// The digests read, compared with the branch's "end" line's.
    pub fn result(&self, chain: &Chain) -> Result<Verified> {
        let leaf = chain.leaf();
        let parts = self.world.as_ref().map(|world| {
            let mut parts = world.clone();
            let log = self.log.digest();
            match parts.iter_mut().find(|(part, _)| part == "event_log") {
                Some(part) => part.1 = log,
                None => parts.push(("event_log".to_owned(), log)),
            }
            parts
        });
        let mut mismatches = Vec::new();
        let end = leaf.end();
        if let Some(end) = end {
            if self.export_sha256.as_deref() != Some(leaf.run()?.export_sha256.as_str()) {
                mismatches.push("export".to_owned());
            }
            for (part, digest) in &end.parts {
                if parts.as_ref().and_then(|parts| parts.iter().find(|(name, _)| name == part)).map(|(_, read)| read) != Some(digest) {
                    mismatches.push(part.clone());
                }
            }
            if parts.as_ref().is_some_and(|parts| parts.len() != end.parts.len()) {
                mismatches.push("parts".to_owned());
            }
            if self.log.records() != end.records {
                mismatches.push("records".to_owned());
            }
            if self.samples.digest() != end.metrics || self.samples.samples() != end.samples {
                mismatches.push("metrics".to_owned());
            }
        }
        Ok(Verified {
            complete: end.is_some(),
            torn: leaf.torn(),
            verified: end.is_some() && mismatches.is_empty(),
            mismatches,
            parts,
            metrics: self.samples.digest(),
            records: self.log.records(),
            samples: self.samples.samples(),
            decisions: self.records[0],
            conversations: self.records[1],
            tape: self.records[2],
            saved_points: self.saved_points,
        })
    }
}
