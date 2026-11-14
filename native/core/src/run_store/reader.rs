//! What a reader reads of a stored Run: its heads, where to cut it back to one, where to restore
//! it from to reach a minute, and its pieces read back to the digests its "end" line holds.

use super::journal::{Journal, Line, Piece, PieceLine, SavedLine};
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

/// Where a seek to `minute` starts: the "saved" line `from` restored (with the stored log and
/// samples, `Journal::next_restore_read`), or the exported world (`export`) when `from` is None;
/// then the ticks up to `minute` are computed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seek {
    pub minute: i64,
    pub from: Option<usize>,
    pub export: usize,
    /// The minute the Run is computed from: the saved point's, or 0.
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

    /// The lines a branch's head may be, in order: the "export" line of a Run from minute 0, each
    /// "saved" line whose log and sample blocks are all stored before it, and the "end" line.
    pub fn heads(&self) -> Vec<Head> {
        let from_zero = self.run().map(|run| run.fork_minute() == 0).unwrap_or(false);
        let (mut log_blocks, mut sample_blocks) = (0u64, 0u64);
        let mut heads = Vec::new();
        for (index, entry) in self.entries().iter().enumerate() {
            match &entry.line {
                Line::Piece(PieceLine { piece: Piece::LogBlock { .. }, .. }) => log_blocks += 1,
                Line::Piece(PieceLine { piece: Piece::SampleBlock { .. }, .. }) => sample_blocks += 1,
                Line::Piece(PieceLine { piece: Piece::Export, .. }) if from_zero => heads.push(Head { kind: HeadKind::Export, minute: 0, index }),
                Line::Piece(PieceLine { piece: Piece::Saved(saved), .. }) if saved.log_blocks <= log_blocks && saved.sample_blocks <= sample_blocks => {
                    heads.push(Head { kind: HeadKind::Saved, minute: saved.minute, index })
                }
                Line::End(end) => heads.push(Head { kind: HeadKind::End, minute: end.minute, index }),
                _ => {}
            }
        }
        heads
    }

    /// The head whose line's sha256 is `sha256`.
    pub fn head(&self, sha256: &str) -> Option<Head> {
        self.heads().into_iter().find(|head| self.entries()[head.index].sha256 == sha256)
    }

    /// The stored Run cut back to the head `sha256`: an error where no head's line has it.
    pub fn cut(&self, sha256: &str) -> Result<Cut> {
        let head = self.head(sha256).ok_or_else(|| Error::Value("the branch's head is not a line its stored Run can go on from".to_owned()))?;
        let mut data_end = 0u64;
        for entry in &self.entries()[..=head.index] {
            if let Line::Piece(line) = &entry.line {
                let end = line.stored.offset.checked_add(line.stored.length).ok_or_else(|| super::corrupt("a piece past any file"))?;
                data_end = data_end.max(end);
            }
        }
        Ok(Cut { head, journal_end: self.entries()[head.index].end, data_end })
    }

    /// The "saved" lines a seek can restore, in order: those whose log and sample blocks are all
    /// stored (in a Run not stored to its end, the last ones may not be).
    pub fn saved_points(&self) -> Vec<usize> {
        let count =
            |wanted: fn(&Piece) -> bool| self.entries().iter().filter(|entry| matches!(&entry.line, Line::Piece(line) if wanted(&line.piece))).count() as u64;
        let log_blocks = count(|piece| matches!(piece, Piece::LogBlock { .. }));
        let sample_blocks = count(|piece| matches!(piece, Piece::SampleBlock { .. }));
        self.entries()
            .iter()
            .enumerate()
            .filter(|(_, entry)| matches!(&entry.line, Line::Piece(PieceLine { piece: Piece::Saved(saved), .. }) if saved.log_blocks <= log_blocks && saved.sample_blocks <= sample_blocks))
            .map(|(index, _)| index)
            .collect()
    }

    /// The "saved" line `index`'s fields.
    pub fn saved(&self, index: usize) -> Result<&SavedLine> {
        match &self.piece(index)?.piece {
            Piece::Saved(saved) => Ok(saved),
            other => Err(Error::Value(format!("the journal's line {index} is not a saved point but {}", other.kind()))),
        }
    }

    /// Where to start to reach `minute`: the nearest saved point at or before it, or the exported
    /// world at minute 0 when there is none. An error for a minute out of the Run (before its
    /// fork's, after its clock's end), for a Run without its exported world, and for a fork's Run
    /// without a saved point to start from (its exported world is the parent's).
    pub fn seek(&self, minute: i64) -> Result<Seek> {
        let (first, duration) = (self.fork_minute()?, self.duration()?);
        if minute < first || minute > duration {
            return Err(Error::Value(format!("minute {minute} is not one of the Run ({first} to {duration})")));
        }
        let export = self.find("export").ok_or_else(|| Error::Value("the stored Run has no exported world".to_owned()))?;
        let mut from = None;
        for index in self.saved_points() {
            let saved = self.saved(index)?;
            if saved.minute <= minute {
                from = Some((index, saved.minute));
            }
        }
        if from.is_none() && first > 0 {
            return Err(Error::Value(format!("the Run of a fork has no saved point at or before minute {minute} to start from")));
        }
        Ok(Seek { minute, from: from.map(|(index, _)| index), export, from_minute: from.map_or(0, |(_, minute)| minute) })
    }

    /// The next piece, from the line `start` on, that `restore` needs: a log piece while it needs
    /// records, a sample piece while it needs samples; None once it needs neither.
    pub fn next_restore_read(&self, start: usize, restore: &Restore) -> Option<usize> {
        let (log, samples) = (restore.log_needed(), restore.samples_needed());
        if !log && !samples {
            return None;
        }
        (start..self.entries().len())
            .find(|index| matches!(&self.entries()[*index].line, Line::Piece(line) if (log && line.piece.is_log()) || (samples && line.piece.is_samples())))
    }

    /// Gives `restore` the piece `index`, decompressed (`raw`): checked against its line, then read
    /// as the log's or the samples' tables or block.
    pub fn restore_read(&self, index: usize, raw: &[u8], restore: &mut Restore) -> Result<()> {
        self.check_raw(index, raw)?;
        match &self.piece(index)?.piece {
            Piece::LogTables(_) => restore.read_log_tables(raw),
            Piece::LogBlock { .. } => restore.read_log_block(raw).map(|_| ()),
            Piece::SampleTables(_) => restore.read_sample_tables(raw),
            Piece::SampleBlock { .. } => restore.read_sample_block(raw).map(|_| ()),
            other => Err(Error::Value(format!("a restore reads the log and the samples, not {}", other.kind()))),
        }
    }

    /// The samples' pieces, in order: what the KPI history reads.
    pub fn sample_pieces(&self) -> Vec<usize> {
        (0..self.entries().len()).filter(|index| matches!(&self.entries()[*index].line, Line::Piece(line) if line.piece.is_samples())).collect()
    }
}

/// A stored Run read back to its digests: its pieces before the "end" line, in order
/// (`Verify::reads`), each given decompressed (`read`), then compared with the "end" line
/// (`result`).
#[derive(Clone, Debug, Default)]
pub struct Verify {
    log: LogReader,
    samples: SampleReader,
    world: Option<Vec<(String, String)>>,
    export_sha256: Option<String>,
    saved_points: u64,
}

/// What reading a stored Run back gave: whether it is complete (it has its "end" line), whether
/// the journal was torn, whether everything matched its "end" line and what did not (`export`, a
/// part's name, `parts`, `records`, `metrics`), the digests read (each part's, the log's
/// `event_log` with them, and the samples'), the counts, and how many saved points it holds.
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
        for (name, count) in [("records", self.records), ("samples", self.samples), ("saved_points", self.saved_points)] {
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

    /// The pieces to read, in order: each piece line before the "end" line.
    pub fn reads(journal: &Journal) -> Vec<usize> {
        (0..journal.entries().len()).filter(|index| matches!(journal.entries()[*index].line, Line::Piece(_))).collect()
    }

    /// Reads the piece `index`, decompressed (`raw`): an error for bytes that are not the ones
    /// written, or that are not such a piece (a saved point not of its line's minute and counts).
    pub fn read(&mut self, journal: &Journal, index: usize, raw: &[u8]) -> Result<()> {
        journal.check_raw(index, raw)?;
        match &journal.piece(index)?.piece {
            Piece::Export => self.export_sha256 = Some(sha256_hex(raw)),
            Piece::LogTables(_) => self.log.read_tables(raw)?,
            Piece::LogBlock { .. } => {
                self.log.read_block(raw)?;
            }
            Piece::SampleTables(_) => self.samples.read_tables(raw)?,
            Piece::SampleBlock { .. } => {
                self.samples.read_block(raw)?;
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

    /// The digests read, compared with the "end" line's.
    pub fn result(&self, journal: &Journal) -> Result<Verified> {
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
        let end = journal.end();
        if let Some(end) = end {
            if self.export_sha256.as_deref() != Some(journal.run()?.export_sha256.as_str()) {
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
            torn: journal.torn(),
            verified: end.is_some() && mismatches.is_empty(),
            mismatches,
            parts,
            metrics: self.samples.digest(),
            records: self.log.records(),
            samples: self.samples.samples(),
            saved_points: self.saved_points,
        })
    }
}
