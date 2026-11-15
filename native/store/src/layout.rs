//! Where a store keeps what (the browser's `project_store.mjs`):
//!
//! ```text
//!   <project>/project.json
//!   <project>/scenarios/<scenario>/v<n>.json, v<n>.scenario.json
//!   <project>/runs/<run>/run.json, evidence.json
//!   <project>/runs/<run>/branches/<branch>/branch.json, journal.jsonl, data.pack
//!   <project>/runs/<run>/worker-lock, <project>/runs/<run>/branches/<branch>/worker-lock
//!   .lock
//! ```
//!
//! The last three are the Python server's locks (`lock`). An id is one of Python's identifiers
//! (`ProjectStore._path`, the browser's `ID`), and no part of a path under the root may be a
//! symlink.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::{Error, Result};

// The ids and the folder of the usage accounts are the core's (`answers::records`), which the
// browser's store reads with.
pub use sheltersim_core::answers::records::{is_id, INVALID_ID, RESERVED};

/// `id`, or Python's error for one that is not an identifier.
pub fn check_id(id: &str) -> Result<&str> {
    if is_id(id) {
        Ok(id)
    } else {
        Err(Error::refused(INVALID_ID))
    }
}

/// A store's root and the paths under it.
#[derive(Clone, Debug)]
pub struct Layout {
    root: PathBuf,
}

impl Layout {
    /// The store at `root`, made where it is not there.
    pub fn new(root: &Path) -> Result<Self> {
        fs::create_dir_all(root)?;
        Ok(Self { root: std::path::absolute(root)? })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `parts` under the root, each an id or a fixed name, refused where a part of it is a symlink.
    fn path(&self, parts: &[&str]) -> Result<PathBuf> {
        let mut path = self.root.clone();
        for part in parts {
            path.push(part);
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.file_type().is_symlink() => return Err(Error::refused("artifact path cannot follow symlinks")),
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(path)
    }

    /// The lock of the store's transactions (Python's `ProjectStore.transaction`).
    pub fn store_lock(&self) -> Result<PathBuf> {
        self.path(&[".lock"])
    }

    pub fn project(&self, project: &str) -> Result<PathBuf> {
        self.path(&[check_id(project)?])
    }

    pub fn project_file(&self, project: &str, name: &str) -> Result<PathBuf> {
        self.path(&[check_id(project)?, name])
    }

    pub fn scenarios(&self, project: &str) -> Result<PathBuf> {
        self.path(&[check_id(project)?, "scenarios"])
    }

    pub fn scenario(&self, project: &str, scenario: &str) -> Result<PathBuf> {
        self.path(&[check_id(project)?, "scenarios", check_id(scenario)?])
    }

    pub fn runs(&self, project: &str) -> Result<PathBuf> {
        self.path(&[check_id(project)?, "runs"])
    }

    pub fn run(&self, project: &str, run: &str) -> Result<PathBuf> {
        self.path(&[check_id(project)?, "runs", check_id(run)?])
    }

    pub fn run_file(&self, project: &str, run: &str, name: &str) -> Result<PathBuf> {
        self.path(&[check_id(project)?, "runs", check_id(run)?, name])
    }

    pub fn branches(&self, project: &str, run: &str) -> Result<PathBuf> {
        self.path(&[check_id(project)?, "runs", check_id(run)?, "branches"])
    }

    /// The folder of a branch: its record and its stored Run.
    pub fn branch(&self, project: &str, run: &str, branch: &str) -> Result<PathBuf> {
        self.path(&[check_id(project)?, "runs", check_id(run)?, "branches", check_id(branch)?])
    }

    pub fn branch_file(&self, project: &str, run: &str, branch: &str, name: &str) -> Result<PathBuf> {
        self.path(&[check_id(project)?, "runs", check_id(run)?, "branches", check_id(branch)?, name])
    }
}

/// The names in a folder that are folders (`folders`) or files, sorted; none where it is not there.
pub fn listed(folder: &Path, folders: bool) -> Result<Vec<String>> {
    let entries = match fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry?;
        let kind = entry.file_type()?;
        if (folders && kind.is_dir()) || (!folders && kind.is_file()) {
            if let Ok(name) = entry.file_name().into_string() {
                names.push(name);
            }
        }
    }
    names.sort();
    Ok(names)
}
