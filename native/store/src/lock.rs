//! The locks of a store, the Python server's (`ProjectStore.transaction`, `claim_branch`,
//! `claim_run`, #48) on the same files:
//!
//! - the store's transaction: `.lock` under the root, held while waiting for it;
//! - a Run's gate: `runs/<run>/worker-lock`, held without waiting;
//! - a branch's writer: `runs/<run>/branches/<branch>/worker-lock`, held without waiting for as long
//!   as the branch is computed or made (one computation of a branch at a time).
//!
//! A branch's writer takes the Run's gate only while it locks its branch, so a whole-Run lease and
//! a branch's never start together; writers of different branches of a Run work at once.
//!
//! The locks are the standard library's `File::lock`: `flock` on Linux and macOS, as Python's
//! `fcntl.flock`, and `LockFileEx` from byte 0 on Windows, which overlaps the byte Python's
//! `msvcrt.locking` locks; so a holder in Python and one here keep each other out. Only exclusive
//! locks are used. Closing the file, or the process ending, releases a lock; the files stay.

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

use crate::layout::{listed, Layout};
use crate::{Error, Result};

/// Python's text of a Run whose gate another holder has.
pub const RUN_BUSY: &str = "run_worker_busy: another process owns this Run";
/// Python's text of a branch another writer holds.
pub const BRANCH_BUSY: &str = "run_worker_busy: another process writes this branch";
/// Python's text of a whole-Run lease refused while a branch is written.
pub const BRANCH_OF_RUN_BUSY: &str = "run_worker_busy: a branch of this Run is being written";

/// A lock held until it is dropped or released.
#[derive(Debug)]
pub struct Lease {
    file: File,
    path: PathBuf,
}

impl Lease {
    /// The lock's file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Releases the lock (dropping the lease does too).
    pub fn release(self) {
        drop(self.file);
    }
}

fn lock_file(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)?;
    // Python's msvcrt lock needs a byte in the file, and writes one where there is none: so does
    // this, so that Python never writes into a file this holds.
    #[cfg(windows)]
    if file.metadata()?.len() == 0 {
        use std::io::Write;
        (&file).write_all(b"\0")?;
    }
    Ok(file)
}

/// The lock of the file `path`, made where it is not there, or None where another holder has it.
pub fn try_lock(path: &Path) -> Result<Option<Lease>> {
    let file = lock_file(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(Lease { file, path: path.to_path_buf() })),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(error.into()),
    }
}

/// The lock of the file `path`, waiting for it.
pub fn lock(path: &Path) -> Result<Lease> {
    let file = lock_file(path)?;
    file.lock()?;
    Ok(Lease { file, path: path.to_path_buf() })
}

/// The store's transaction (Python's `ProjectStore.transaction`): held while waiting for it. Python
/// holds it while it changes records, claims a lease or recovers interrupted Runs. It is not
/// reentrant: a holder that takes it again in the same process waits for itself.
pub fn transaction(layout: &Layout) -> Result<Lease> {
    lock(&layout.store_lock()?)
}

/// The lease of a branch's writer (Python's `claim_branch`): the computation of the branch, or the
/// fork that makes it. `run_exists` is checked inside the store's transaction (Python reads the
/// Run's record there).
pub fn claim_branch(layout: &Layout, project: &str, run: &str, branch: &str, run_exists: impl FnOnce() -> Result<()>) -> Result<Lease> {
    let _transaction = transaction(layout)?;
    run_exists()?;
    let folder = layout.branch(project, run, branch)?;
    let gate = try_lock(&layout.run_file(project, run, "worker-lock")?)?.ok_or_else(|| Error::refused(RUN_BUSY))?;
    std::fs::create_dir_all(&folder)?;
    let lease = try_lock(&folder.join("worker-lock"))?.ok_or_else(|| Error::refused(BRANCH_BUSY));
    gate.release();
    lease
}

/// The lease of a whole Run (Python's `claim_run`): its gate, held, refused while a branch's writer
/// holds its branch.
pub fn claim_run(layout: &Layout, project: &str, run: &str, run_exists: impl FnOnce() -> Result<()>) -> Result<Lease> {
    let _transaction = transaction(layout)?;
    run_exists()?;
    let gate = try_lock(&layout.run_file(project, run, "worker-lock")?)?.ok_or_else(|| Error::refused(RUN_BUSY))?;
    let branches = layout.branches(project, run)?;
    for name in listed(&branches, true)? {
        let path = branches.join(&name).join("worker-lock");
        if !path.is_file() {
            continue;
        }
        match try_lock(&path)? {
            Some(branch) => branch.release(),
            None => return Err(Error::refused(BRANCH_OF_RUN_BUSY)),
        }
    }
    Ok(gate)
}

/// Whether another holder has the lock of `path` now (a file that is not there is free).
pub fn held(path: &Path) -> Result<bool> {
    if !path.is_file() {
        return Ok(false);
    }
    Ok(try_lock(path)?.is_none())
}
