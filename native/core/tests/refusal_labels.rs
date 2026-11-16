//! A refusal's label (`Error::Refused`, "{label} refused: {reason}") names the core's function that
//! refused: `module.name`, where `module` is a file or folder of `native/core/src` and `name` is a
//! function in it (#569). A label that names a function the core no longer has, as the labels of
//! the Python rules did after the port, fails here.
//!
//! Every string literal of the form `word.word` in the source is read as a label, but the names
//! below, which are not labels: the fields of an EffectProgram's guard and the paths of the model's
//! parameters, and file names (`EXTENSIONS`).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

/// The literals of the form `word.word` that are not labels.
const NOT_LABELS: [&str; 11] = [
    // The fields of an EffectProgram's guard (`program::GUARD_FIELDS`).
    "simulation.minute",
    "target.capacity",
    "target.working_units",
    "target.queue_length",
    "zone.occupancy",
    "resource.available_quantity",
    "staff.available",
    // The paths of the model's parameters.
    "behavior.sleep_debt_utility_weight",
    "facility_queues.retry_delay_minutes",
    "facility_queues.persistence_urgency_threshold",
    "staff.support_fairness_minutes",
];

/// The extensions of the file names in the source (`journal.jsonl`, `records.json`).
const EXTENSIONS: [&str; 4] = ["json", "jsonl", "pack", "rs"];

fn sources(dir: &Path, found: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = fs::read_dir(dir).unwrap().map(|entry| entry.unwrap().path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            sources(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}

/// The modules a file belongs to: its own name (its folder's for `mod.rs`) and each folder above it.
fn modules_of(root: &Path, path: &Path) -> Vec<String> {
    let relative = path.strip_prefix(root).unwrap();
    let mut names: Vec<String> = relative.parent().into_iter().flat_map(|parent| parent.iter()).map(|part| part.to_string_lossy().into_owned()).collect();
    let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
    if stem != "mod" {
        names.push(stem);
    }
    names
}

/// The names of the functions a file defines.
fn functions(text: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for (at, _) in text.match_indices("fn ") {
        if at > 0 && text[..at].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_') {
            continue;
        }
        let name: String = text[at + 3..].chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        let next = text[at + 3 + name.len()..].chars().next();
        if !name.is_empty() && matches!(next, Some('(' | '<')) {
            names.insert(name);
        }
    }
    names
}

fn is_word(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| c.is_ascii_lowercase() || c == '_')
}

/// The string literals of a line's code (before a `//` comment) of the form `word.word`.
fn dotted_literals(line: &str) -> Vec<&str> {
    let code = line.split("//").next().unwrap_or("");
    code.split('"')
        .skip(1)
        .step_by(2)
        .filter(|literal| literal.split_once('.').is_some_and(|(module, name)| is_word(module) && is_word(name) && !EXTENSIONS.contains(&name)))
        .collect()
}

#[test]
fn a_refusal_label_names_a_function_of_the_core() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&root, &mut files);
    let mut defined: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut texts = Vec::new();
    for path in files {
        let text = fs::read_to_string(&path).unwrap();
        for module in modules_of(&root, &path) {
            defined.entry(module).or_default().extend(functions(&text));
        }
        texts.push((path, text));
    }
    let mut labels = 0;
    let mut wrong = Vec::new();
    for (path, text) in &texts {
        for (number, line) in text.lines().enumerate() {
            for literal in dotted_literals(line) {
                if NOT_LABELS.contains(&literal) {
                    continue;
                }
                labels += 1;
                let (module, name) = literal.split_once('.').unwrap();
                if !defined.get(module).is_some_and(|names| names.contains(name)) {
                    wrong.push(format!("{}:{}: {literal}", path.strip_prefix(&root).unwrap().display(), number + 1));
                }
            }
        }
    }
    assert!(labels > 50, "{labels} labels");
    assert!(wrong.is_empty(), "labels that name no function of the core:\n{}", wrong.join("\n"));
}

#[test]
fn a_name_that_is_not_a_label_is_still_in_the_source() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&root, &mut files);
    let found: BTreeSet<String> = files
        .iter()
        .flat_map(|path| {
            fs::read_to_string(path)
                .unwrap()
                .lines()
                .flat_map(|line| dotted_literals(line).into_iter().map(str::to_owned).collect::<Vec<_>>())
                .collect::<Vec<_>>()
        })
        .collect();
    let stale: Vec<&str> = NOT_LABELS.iter().copied().filter(|name| !found.contains(*name)).collect();
    assert!(stale.is_empty(), "names the source no longer has: {stale:?}");
}
