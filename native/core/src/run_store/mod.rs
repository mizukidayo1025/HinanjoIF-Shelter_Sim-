//! The format of a stored Run (#252, #251, #266, #306; moved from the browser's
//! `js/run_store.mjs` by #368, format v5 by #368, v6 by #478): the journal's lines, what a writer stores and
//! when, and what a reader reads of it. The core does no I/O and compresses nothing: a host (the
//! browser's `run_store.mjs`; later Python) opens the files, compresses each piece before it writes
//! it and decompresses it after it reads it, and passes the bytes in.
//!
//! A Run's folder holds two files that are only ever appended to:
//!   data.pack      the pieces, each compressed by the host (its "run" line names how)
//!   journal.jsonl  one line per piece or event: its JSON, a tab, the sha256 of that JSON, "\n"
//!
//! A piece is written to data.pack and flushed before its journal line is, so a line never points
//! at bytes that are not there. Reading stops at the first line that is cut short or whose sha256
//! is not its JSON's (a write the host did not finish, `Journal::parse`); bytes of data.pack no
//! line points at are ignored. Each piece's line holds the sha256 of its bytes before compression,
//! which reading checks (`Journal::check_raw`): compressed bytes may differ between compressors,
//! the bytes they hold may not.
//!
//! Format "sheltersim-run-v6" (#478) is the only one read. Its lines, in the order they are written
//! (`RunWriter`):
//!   run           first: the format, `engine_version` (`model_version::ENGINE_VERSION` of the
//!                 writer), the `model_versions` the Run is computed with, the host's
//!                 `compression`, `save_every` (minutes between saved points, 360 by default),
//!                 the exported world's sha256 and length, its name in a file of named worlds
//!                 (`world`, the fixtures'), the clock's end; a fork's also `fork_minute` and
//!                 `parent_branch_id`
//!   fork          a fork's second line (an intervention's child, #306): its origin, the EffectProgram
//!                 applied, the approval's hash and the receipt, and its `base`: the parent's line it
//!                 goes on from (its index, head hash and minute, and what the parent had stored up
//!                 to it), whose pieces it reads instead of a copy of them (`Chain`, decision 6)
//!   export        a Run from minute 0's second line: the exported world it starts from (its JSON)
//!   log_tables    the log's string and shape tables' new entries (`Packer::write_tables`)
//!   log_block     a sealed block of the packed log (`EventLog::packed_store`), by its index
//!   sample_tables, sample_block   the same of the metric samples (`MetricSamples::store`)
//!   conversations, llm_tape   append-only records (`records`, decision 4): the branch's
//!                 question turns (after the "end" line: the questions are about a Run computed to
//!                 its end), the LLM's calls (`PlainRun::take_tape`, #428: `llm::tape_entry`, which
//!                 `llm::Replay` answers from when ticks are computed again). The line holds
//!                 the count
//!   saved         a saved point (#251): the Run at a minute (`archive::SavedPoint`: the world without
//!                 its log and the counts of records and samples up to then), every `save_every`
//!                 minutes and at the end. Its line holds the minute, the counts of records,
//!                 samples, turns and tape entries, the point's version, and the log's and
//!                 samples' blocks; the blocks are always sealed before a saved point (decision 2),
//!                 so the counts are what is stored before it
//!   world         the world at the last minute without its log (`archive::world_parts`)
//!   end           the minute and the digests the Run gave, and its counts
//! `Journal::parse` keeps what the Run has stored after each line (`State`) and refuses a line
//! that does not follow it, and keys the format does not have.
//!
//! A Run of an older format ("sheltersim-browser-run-v1" to "-v4", "sheltersim-run-v5") is not
//! read: its "run" line is the error `stored_run_outdated` (`outdated`), and the Run is computed
//! again (decision 7). v5 also stored the records of the decisions (the old local version's
//! `_IndexedMetricSink`), which nothing read; v6 has no such piece (#478).
//!
//! The log and the samples are only appended to, so a saved point holds no copy of them: seeking
//! (`Chain::seek`) restores the nearest saved point at or before the minute from it and the stored
//! log's and samples' first records and samples up to its counts (`Chain::restore_read`), then
//! computes the ticks up to the minute (at most `save_every` - 1).
//!
//! A branch's head (#267) is a line its stored Run can be read back at and gone on from
//! (`Journal::heads`): the "export" line (minute 0, not in a fork's Run), a "saved" line, or the
//! "end" line. Its record's `checkpoint_hash` is that line's head hash: the sha256 of its JSON
//! without where its piece is in data.pack (`Line::head_json`, decision 1), the same whichever
//! compressor wrote it. Going on from a head cuts the lines after it off first (`Journal::cut`),
//! which a fork reading the branch's pieces past the head refuses.
//!
//! What a writer queues passes the check of private data first (`sensitive`, decision 5). Going on
//! with a Run computed with other model versions is the host's to refuse
//! (`model_version::ModelStatus`, decision 8), from the "run" line's `model_versions`.
//!
//! The journal's text is the browser's `JSON.stringify` of each line's object: the keys in the
//! order of this module's writer, no spaces, strings escaped as `json::write_json_string` does. A
//! line read back keeps its own JSON and sha256 (`Entry`); only the lines written are encoded here.

mod journal;
mod reader;
pub mod records;
mod writer;

pub use journal::{
    Base, Counts, Encoded, EndLine, Entry, ForkLine, ForkOrigin, ForkStart, Journal, Line, Piece, PieceLine, Place, RecordKind, RunLine, SavedLine, State,
    Stored, Storing, Tables,
};
pub use reader::{At, Chain, Cut, Head, HeadKind, Seek, Verified, Verify};
pub use writer::{Fork, Next, RunWriter, Schedule};

use crate::error::Error;

/// The journal's file in a Run's folder.
pub const JOURNAL: &str = "journal.jsonl";
/// The pieces' file in a Run's folder.
pub const DATA: &str = "data.pack";
/// Minutes between saved points by default: 6 hours (#263).
pub const SAVE_EVERY: u64 = 360;
/// The format written and read.
pub const FORMAT: &str = "sheltersim-run-v6";
/// The formats before it, which are not read (#368, decision 7).
pub const OUTDATED_FORMATS: [&str; 5] =
    ["sheltersim-browser-run-v1", "sheltersim-browser-run-v2", "sheltersim-browser-run-v3", "sheltersim-browser-run-v4", "sheltersim-run-v5"];
/// The code of the error of a Run of an older format.
pub const OUTDATED_CODE: &str = "stored_run_outdated";

/// The error of a stored Run of the older format `format`: its code, and what to do in Japanese,
/// as the browser's other refusals say it.
pub fn outdated(format: &str) -> Error {
    Error::Value(format!(
        "{OUTDATED_CODE}: このシナリオの計算結果は古い保存の形（{format}）で保存されています。今の形（{FORMAT}）では読めないので、計算し直してください。"
    ))
}

/// An error of a stored Run that is not what this format says.
pub(crate) fn corrupt(what: &str) -> Error {
    Error::Value(format!("not a stored Run: {what}"))
}
