//! The files of the stored Runs of 避難所IF！ on a disk (Issue #426, part of #368): the core's
//! format v5 (`sheltersim_core::run_store`, which does no I/O) written to and read from the same
//! folders as the browser's (`native/wasm/js/project_store.mjs`, `run_store.mjs`), for the local
//! and VM servers through the PyO3 crate. The core stays free of I/O and builds for wasm32; this
//! crate is not part of that build.
//!
//! - `compress`: the pieces' compression, deflate-raw (flate2), as the browser's
//!   `CompressionStream("deflate-raw")`. The bytes it writes may differ from the browser's; the
//!   bytes they hold may not, and a branch's head hash is taken from those (#414).
//! - `files`: a file only appended to (`AppendFile`), reading at an offset, and the flushes.
//! - `stored`: a branch's stored Run written (`StoredRunWriter`: a piece written to data.pack and
//!   flushed to the disk before its journal line is, each line flushed before the next) and read
//!   (`StoredRun`: the journal read once, a fork's parents' journals and data.pack through the
//!   chain, a seek to a minute, the records, the check back to the digests). A reader in another
//!   process may read while a writer appends: it reads the journal up to its last whole line (a
//!   line cut short is ignored), and every line it reads points at bytes already on the disk.
//! - `quota`: what the store's files take and the limit checked before each write (the server's
//!   `--project-storage-gb`).
//! - `lock`: the locks of the Python server's `ProjectStore` (`claim_branch`, `claim_run`, its
//!   `transaction`), on the same files with the same kind of lock, so either side's holder keeps
//!   the other out on Linux, macOS and Windows.
//! - `layout` and `records`: the Project, Scenario, Run and Branch records in the browser's shapes
//!   and folders (`project_store.mjs`), with Python's errors.
//! - `compute`: what the browser's `compute.mjs` and `interventions.mjs` do with them: a branch
//!   computed from its head in chunks, its head moved at each saved point, and a branch made by a
//!   fork.

pub mod compress;
pub mod compute;
pub mod files;
pub mod layout;
pub mod lock;
pub mod quota;
pub mod records;
pub mod stored;

use std::fmt;

/// An error of the store.
#[derive(Debug)]
pub enum Error {
    /// A refusal, with the text of the Python server's `StorageError` where it has one (a leading
    /// `code: ` names the code, as Python's do).
    Refused(String),
    /// What the core refused or could not read.
    Core(sheltersim_core::error::Error),
    /// A file could not be read or written.
    Io(std::io::Error),
}

impl Error {
    pub fn refused(text: impl Into<String>) -> Self {
        Error::Refused(text.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Refused(text) => f.write_str(text),
            Error::Core(error) => write!(f, "{error}"),
            Error::Io(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<sheltersim_core::error::Error> for Error {
    fn from(error: sheltersim_core::error::Error) -> Self {
        Error::Core(error)
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Error::Io(error)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
