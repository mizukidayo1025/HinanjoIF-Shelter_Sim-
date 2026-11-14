//! The check that keeps private data out of what a Run stores (#368, decision 5): Python's
//! `_reject_sensitive_checkpoint_content` (src/sheltersim/storage/projects.py), which every
//! Checkpoint's new chunks pass before they are written, with the same rule and texts. A stored
//! Run (`run_store`) passes its exported world, each saved point's world, the world at its end,
//! the records of its log, its metric samples and its decision records through it before the host
//! writes them (`run_store::RunWriter`).
//!
//! The rule: from the canonical JSON of what is stored, every object key and every string that
//! could be one Python's `_redact` changes (`checkpoint::sensitive_candidates`); a key that
//! `_private_field` finds private data in is refused with `SENSITIVE_FIELDS`, a string `_redact`
//! changes (an API key, a bearer token, an e-mail address, a phone number:
//! `interpret::redacts`) with `SENSITIVE_DATA`. A key or a string that passed once is not read
//! again (`Guard`), as Python checks each distinct one once.

use std::collections::HashSet;

use crate::checkpoint::sensitive_candidates;
use crate::error::{Error, Result};
use crate::interpret::{private_field, redacts};

/// Python's text for a key that names private data.
pub const SENSITIVE_FIELDS: &str = "sensitive fields cannot be persisted";
/// Python's text for a string that holds private data.
pub const SENSITIVE_DATA: &str = "checkpoint contains sensitive data";

/// The keys and strings that passed the check, so that each is read once.
#[derive(Clone, Debug, Default)]
pub struct Guard {
    keys: HashSet<String>,
    strings: HashSet<String>,
}

impl Guard {
    pub fn new() -> Self {
        Self::default()
    }

    /// An error (`SENSITIVE_FIELDS`, `SENSITIVE_DATA`) where a key or a string of the JSON texts
    /// `raws` holds private data; an error too for a text that is not JSON.
    pub fn check(&mut self, raws: &[&[u8]]) -> Result<()> {
        if raws.is_empty() {
            return Ok(());
        }
        let (keys, strings) = sensitive_candidates(raws).map_err(|error| Error::Value(format!("not JSON: {error}")))?;
        // In order, so that a check reads the same keys and strings first on every run.
        let mut keys: Vec<String> = keys.into_iter().collect();
        keys.sort_unstable();
        let mut strings: Vec<String> = strings.into_iter().collect();
        strings.sort_unstable();
        for key in keys {
            if self.keys.contains(&key) {
                continue;
            }
            if private_field(&key)? {
                return Err(Error::Value(SENSITIVE_FIELDS.to_owned()));
            }
            self.keys.insert(key);
        }
        for text in strings {
            if self.strings.contains(&text) {
                continue;
            }
            if redacts(&text)? {
                return Err(Error::Value(SENSITIVE_DATA.to_owned()));
            }
            self.strings.insert(text);
        }
        Ok(())
    }
}

/// `Guard::check` of the JSON texts `raws` alone.
pub fn check(raws: &[&[u8]]) -> Result<()> {
    Guard::new().check(raws)
}
