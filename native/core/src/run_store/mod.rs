//! The format of a stored Run (#252, #251, #266, #306; moved from the browser's
//! `js/run_store.mjs` by #368): the journal's lines, what a writer stores and when, and what a
//! reader reads of it. The core does no I/O and compresses nothing: a host (the browser's
//! `run_store.mjs`; later Python) opens the files, compresses each piece before it writes it and
//! decompresses it after it reads it, and passes the bytes in.
//!
//! A Run's folder holds two files that are only ever appended to:
//!   data.pack      the pieces, each compressed (deflate-raw: the browser's `CompressionStream`)
//!   journal.jsonl  one line per piece or event: its JSON, a tab, the sha256 of that JSON, "\n"
//!
//! A piece is written to data.pack and flushed before its journal line is, so a line never points
//! at bytes that are not there. Reading stops at the first line that is cut short or whose sha256
//! is not its JSON's (a write the host did not finish, `Journal::parse`); bytes of data.pack no
//! line points at are ignored. Each piece's line holds the sha256 of its bytes before compression,
//! which reading checks (`Journal::check_raw`): compressed bytes may differ between compressors,
//! the bytes they hold may not.
//!
//! The pieces, in the order they are written (`RunWriter`):
//!   export        the exported world at minute 0 the Run started from (its JSON bytes)
//!   log_tables    the log's string and shape tables' new entries (`Packer::write_tables`)
//!   log_block     a sealed block of the packed log (`EventLog::packed_store`), by its index
//!   sample_tables, sample_block   the same of the metric samples (`MetricSamples::store`)
//!   saved         a saved point (#251): the Run at a minute (`archive::SavedPoint`, a versioned
//!                 binary: the world without its log and the counts of records and samples up to
//!                 then), every `save_every` minutes and at the end
//!   world         the world at the last minute without its log (`archive::world_parts`)
//! and the lines without a piece: "run" first (the format and what the Run is of) and "end" last
//! (the minute and the digests the Run gave). Tables come before the blocks that use them. Blocks
//! are taken as they seal, between two ticks, and every block at the end. A "saved" line has the
//! minute, the counts of records and samples, the saved point's version, and how many log and
//! sample blocks hold those records and samples (`log_blocks`, `sample_blocks`): the blocks there
//! were when it was saved, the last of them maybe not stored yet.
//!
//! Formats (`Format`): "sheltersim-browser-run-v4" (#306) is written. It is v3 with the Run of a
//! branch made by a fork (an intervention's child): such a Run starts at the fork's minute. Its
//! "run" line also holds `fork_minute` and `parent_branch_id`, a "fork" line after it holds the
//! fork (its origin in the parent, the EffectProgram applied and the receipt), and its pieces are
//! the parent's exported world at minute 0, the log's and samples' tables and blocks up to the
//! fork's minute, and a saved point at the fork's minute after the program was applied; the Run
//! goes on from there. A minute before the fork's is not one of it (`Journal::seek` refuses it). A
//! Run without `fork_minute` is a Run from minute 0, as in v3.
//! "sheltersim-browser-run-v3" (#266), still read and gone on from, is the same without forks: a
//! metric sample of a world with a resource ledger also holds the ledger's KPI columns
//! (`MetricSamples`), which the API's KPI history answers with.
//! "sheltersim-browser-run-v2" (#251) is v3 without those columns, and
//! "sheltersim-browser-run-v1" (#252) also without saved points (and without `save_every`); both
//! are still read (seeking in v1 computes from minute 0), but the KPI history refuses them. Any
//! other is an error.
//!
//! The log and the samples are only appended to, so a saved point holds no copy of them: seeking
//! (`Journal::seek`) restores the nearest saved point at or before the minute from it and the
//! stored log's and samples' first records and samples up to its counts (`Journal::restore_read`),
//! then computes the ticks up to the minute (at most `save_every` - 1).
//!
//! A branch's head (#267) is a line its stored Run can be read back at and gone on from
//! (`Journal::heads`): the "export" line (minute 0, not in a fork's Run), a "saved" line whose
//! blocks are all stored before it, or the "end" line. Its record's `checkpoint_hash` is that
//! line's sha256. Going on from a head cuts the lines after it off first (`Journal::cut`).
//!
//! The journal's text is what the browser's `JSON.stringify` wrote: the keys in the order of this
//! module's writer, no spaces, strings escaped as `json::write_json_string` does. A line read back
//! keeps its own JSON and sha256 (`Entry`); only the lines written are encoded here.

mod journal;
mod reader;
mod writer;

pub use journal::{Encoded, EndLine, Entry, ForkLine, ForkOrigin, ForkStart, Journal, Line, Piece, PieceLine, RunLine, SavedLine, Stored, Tables};
pub use reader::{Cut, Head, HeadKind, Seek, Verified, Verify};
pub use writer::{Next, Place, RunWriter, Schedule};

use crate::error::{Error, Result};

/// The journal's file in a Run's folder.
pub const JOURNAL: &str = "journal.jsonl";
/// The pieces' file in a Run's folder.
pub const DATA: &str = "data.pack";
/// Minutes between saved points by default: 6 hours (#263).
pub const SAVE_EVERY: u64 = 360;

/// The format of a stored Run, by the name its "run" line holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Format {
    /// #252: no saved points.
    V1,
    /// #251: saved points; samples without the ledger's columns.
    V2,
    /// #266: samples with the ledger's columns.
    V3,
    /// #306: a branch made by a fork. Written.
    V4,
}

impl Format {
    /// The format written.
    pub const CURRENT: Format = Format::V4;
    /// Every format read, oldest first.
    pub const ALL: [Format; 4] = [Format::V1, Format::V2, Format::V3, Format::V4];

    pub fn name(self) -> &'static str {
        match self {
            Format::V1 => "sheltersim-browser-run-v1",
            Format::V2 => "sheltersim-browser-run-v2",
            Format::V3 => "sheltersim-browser-run-v3",
            Format::V4 => "sheltersim-browser-run-v4",
        }
    }

    /// The format of `name`; an error for one this core does not read.
    pub fn of_name(name: &str) -> Result<Format> {
        Format::ALL.into_iter().find(|format| format.name() == name).ok_or_else(|| {
            let names: Vec<&str> = Format::ALL.iter().map(|format| format.name()).collect();
            Error::Value(format!("a stored Run of format {name}: this code reads {}", names.join(", ")))
        })
    }

    /// Whether its Runs may hold saved points (v2 on).
    pub fn has_saved_points(self) -> bool {
        self >= Format::V2
    }

    /// Whether its samples hold the ledger's columns (v3 on): the KPI history answers it, and a
    /// computation goes on in it.
    pub fn is_current(self) -> bool {
        self >= Format::V3
    }

    /// Whether its Runs may be a fork's (v4).
    pub fn has_forks(self) -> bool {
        self >= Format::V4
    }
}

/// An error of a stored Run that is not what this format says.
pub(crate) fn corrupt(what: &str) -> Error {
    Error::Value(format!("not a stored Run: {what}"))
}
