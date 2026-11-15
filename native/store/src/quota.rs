//! What a store's files take, and the limit checked before each write (the Python server's
//! `--project-storage-gb`, `ProjectStore.max_bytes`).
//!
//! The usage is measured as Python's `ProjectStore._measure_usage` measures it: the regular files
//! under the store's root, symlinks neither counted nor followed, a file or folder removed while it
//! is measured holding nothing. A writer measures it once when it starts (`Quota::measure`) and
//! counts its own writes on it; another process's writes count from the next measure.

use std::fs;
use std::io;
use std::path::Path;

use crate::{Error, Result};

/// Python's text of a write the limit refuses.
pub const CAPACITY_EXCEEDED: &str = "storage capacity exceeded; export then delete saved runs";

/// What the files under a root take.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub bytes: u64,
    pub files: u64,
}

/// The regular files under `root` now.
pub fn usage(root: &Path) -> io::Result<Usage> {
    let mut usage = Usage::default();
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        let entries = match fs::read_dir(&folder) {
            Ok(entries) => entries,
            Err(error) if matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied) => continue,
            Err(error) => return Err(error),
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            // The entry's own type: a symlink is neither a folder nor a file here.
            let kind = match entry.file_type() {
                Ok(kind) => kind,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            if kind.is_dir() {
                folders.push(entry.path());
            } else if kind.is_file() {
                match entry.metadata() {
                    Ok(metadata) => {
                        usage.bytes += metadata.len();
                        usage.files += 1;
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error),
                }
            }
        }
    }
    Ok(usage)
}

/// The limit of a store and what it held when measured, with the writes counted since.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quota {
    pub limit: Option<u64>,
    pub used: u64,
}

impl Quota {
    /// No limit: nothing is measured.
    pub fn unlimited() -> Self {
        Self { limit: None, used: 0 }
    }

    /// The store at `root` measured now, against `limit` bytes (none: `unlimited`).
    pub fn measure(root: &Path, limit: Option<u64>) -> Result<Self> {
        match limit {
            None => Ok(Self::unlimited()),
            Some(limit) => Ok(Self { limit: Some(limit), used: usage(root)?.bytes }),
        }
    }

    /// Counts `bytes` about to be written, or refuses them where the store would hold more than
    /// its limit (Python's check: what it holds and the bytes, past the limit).
    pub fn reserve(&mut self, bytes: u64) -> Result<()> {
        let after = self.used.saturating_add(bytes);
        if self.limit.is_some_and(|limit| after > limit) {
            return Err(Error::refused(CAPACITY_EXCEEDED));
        }
        self.used = after;
        Ok(())
    }

    /// Counts `bytes` removed (a file cut back).
    pub fn release(&mut self, bytes: u64) {
        self.used = self.used.saturating_sub(bytes);
    }
}
