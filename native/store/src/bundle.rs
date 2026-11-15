//! One file that carries a Project (the browser's bundle, `project_store.mjs`'s `exportProject`,
//! #265): its records and each branch's stored files as they are, for the site's examples and to
//! open a Project of the local or VM version in the browser (#427).
//!
//! ```text
//!   16 bytes   magic: 0x89 "SHELTERSIM-PRJ" 0x0a
//!   4 bytes    the bundle's version, big-endian (2)
//!   4 bytes    the manifest's length in bytes, big-endian
//!   32 bytes   the manifest's sha256
//!   manifest   JSON: {format, version, project_id, source, entries: [{path, length, sha256}]}
//!   entries    each entry's bytes in the manifest's order, nothing after the last
//! ```
//!
//! The entries are the files of the Project's folder a bundle holds, depth first, each folder's
//! names in order, their paths relative to the folder. The browser reads them back
//! (`importProject`): it checks the records and reads each branch's stored Run back to its digests.
//! What the Python server keeps beside them for itself (its locks, the drafts of previews, the LLM
//! usage accounts, tapes and settings) is not carried; any other file is refused, as the browser
//! refuses it.

use std::fs;
use std::path::Path;

use serde_json::{json, Value};
use sheltersim_core::checkpoint::sha256_hex;

use crate::layout::{is_id, listed};
use crate::records::ProjectFiles;
use crate::{Error, Result};

pub const BUNDLE_FORMAT: &str = "sheltersim-project-bundle";
pub const BUNDLE_VERSION: u32 = 2;
pub const BUNDLE_EXTENSION: &str = ".shelterproj";
const MAGIC: &[u8; 16] = b"\x89SHELTERSIM-PRJ\n";

/// The files of a Project's folder the Python server keeps for itself, which a bundle leaves out:
/// at the top, in a Run's folder and in a branch's folder.
const PROJECT_OWN: [&str; 4] = ["drafts", "preparation.json", "usage", "usage-tapes"];
const RUN_OWN: [&str; 6] = ["worker-lock", "usage.json", "analysis.json", "analysis-tape.json", "compiler-tape.json", "llm.json"];
const BRANCH_OWN: [&str; 1] = ["worker-lock"];

/// Whether a bundle holds the file at `parts` (relative to the Project's folder): the browser's
/// `entryKind`.
fn held(parts: &[&str]) -> bool {
    let scenario_file = |name: &str| {
        let Some(rest) = name.strip_prefix('v') else { return false };
        let number = rest.strip_suffix(".scenario.json").or_else(|| rest.strip_suffix(".json")).unwrap_or("");
        (1..=9).contains(&number.len()) && !number.starts_with('0') && number.bytes().all(|byte| byte.is_ascii_digit())
    };
    let run_file = |name: &str| {
        let bytes = name.as_bytes();
        !bytes.is_empty()
            && bytes.len() <= 64
            && bytes[0].is_ascii_alphanumeric()
            && bytes[1..].iter().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    };
    match parts {
        ["project.json"] => true,
        ["scenarios", scenario, name] => is_id(scenario) && scenario_file(name),
        ["runs", run, "run.json" | "evidence.json"] => is_id(run),
        ["runs", run, "branches", branch, name] => is_id(run) && is_id(branch) && run_file(name),
        _ => false,
    }
}

/// Whether the file or folder at `parts` is one the Python server keeps for itself.
fn own(parts: &[&str]) -> bool {
    match parts {
        [name] => PROJECT_OWN.contains(name),
        ["runs", _, name] => RUN_OWN.contains(name),
        ["runs", _, "branches", _, name] => BRANCH_OWN.contains(name),
        _ => false,
    }
}

/// The files of the Project's folder a bundle holds, depth first: `(path, bytes)`.
fn entries(folder: &Path) -> Result<Vec<(String, Vec<u8>)>> {
    fn visit(folder: &Path, at: &mut Vec<String>, out: &mut Vec<(String, Vec<u8>)>) -> Result<()> {
        let mut names: Vec<(String, bool)> = listed(folder, true)?.into_iter().map(|name| (name, true)).collect();
        names.extend(listed(folder, false)?.into_iter().map(|name| (name, false)));
        names.sort();
        for (name, is_folder) in names {
            at.push(name.clone());
            let parts: Vec<&str> = at.iter().map(String::as_str).collect();
            // A file being written (`.pending-…`) is not one yet.
            if !(own(&parts) || name.starts_with('.')) {
                let path = folder.join(&name);
                if is_folder {
                    visit(&path, at, out)?;
                } else if held(&parts) {
                    out.push((parts.join("/"), fs::read(&path)?));
                } else {
                    return Err(Error::refused(format!("bundle_invalid: {}: a file a bundle does not hold", parts.join("/"))));
                }
            }
            at.pop();
        }
        Ok(())
    }
    let mut out = Vec::new();
    visit(folder, &mut Vec::new(), &mut out)?;
    Ok(out)
}

/// Packs `entries` (`(path, bytes)`) into a bundle of the Project `project`; `source` is what made
/// it (any JSON), kept in its manifest.
pub fn pack(project: &str, source: &Value, entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let listed: Vec<Value> = entries.iter().map(|(path, bytes)| json!({"path": path, "length": bytes.len(), "sha256": sha256_hex(bytes)})).collect();
    // The keys in the order the browser writes them (`JSON.stringify` of its object).
    let manifest = format!(
        "{{\"format\":{},\"version\":{BUNDLE_VERSION},\"project_id\":{},\"source\":{},\"entries\":[{}]}}",
        Value::from(BUNDLE_FORMAT),
        Value::from(project),
        source,
        listed
            .iter()
            .map(|entry| format!("{{\"path\":{},\"length\":{},\"sha256\":{}}}", entry["path"], entry["length"], entry["sha256"]))
            .collect::<Vec<_>>()
            .join(",")
    );
    let manifest = manifest.into_bytes();
    let digest = hex_bytes(&sha256_hex(&manifest));
    let total: usize = entries.iter().map(|(_, bytes)| bytes.len()).sum();
    let mut out = Vec::with_capacity(MAGIC.len() + 8 + 32 + manifest.len() + total);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&BUNDLE_VERSION.to_be_bytes());
    out.extend_from_slice(&u32::try_from(manifest.len()).expect("a manifest under 4 GiB").to_be_bytes());
    out.extend_from_slice(&digest);
    out.extend_from_slice(&manifest);
    for (_, bytes) in entries {
        out.extend_from_slice(bytes);
    }
    out
}

fn hex_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len()).step_by(2).map(|at| u8::from_str_radix(&hex[at..at + 2], 16).expect("hex")).collect()
}

impl ProjectFiles {
    /// The bundle of the Project `project` (the browser's `exportProject`); `source` is kept in its
    /// manifest. The caller holds the store's transaction, so no record changes meanwhile.
    pub fn export_project(&self, project: &str, source: &Value) -> Result<Vec<u8>> {
        self.project(project)?;
        let folder = self.layout().project(project)?;
        Ok(pack(project, source, &entries(&folder)?))
    }
}
