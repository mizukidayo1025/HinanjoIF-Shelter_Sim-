//! What the local and VM servers read and write through the store (#427): the files the core's
//! answers read, the statuses of a Run computed on several branches at once, a Run the Python
//! server stored before #427, and a Project carried in one bundle.

use std::fs;

use serde_json::Value;
use sheltersim_core::checkpoint::sha256_hex;
use sheltersim_store::bundle::{BUNDLE_FORMAT, BUNDLE_VERSION};
use sheltersim_store::records::{Branch, NewRun, ProjectFiles};

use crate::support::{engine, make_browser_runs, TempDir};

fn status(record: &serde_json::Map<String, Value>) -> Option<&str> {
    record.get("status").and_then(Value::as_str)
}

#[test]
fn the_files_an_answer_reads_are_listed_by_folder_and_a_journal_up_to_its_last_whole_line() {
    let dir = TempDir::new("server-listing");
    let made = make_browser_runs(dir.path(), ProjectFiles::new(dir.path(), None).unwrap());
    let files = &made.files;
    let projects: Vec<String> = files.projects_listing().unwrap().into_iter().map(|(name, _)| name).collect();
    assert!(projects.contains(&"p1".to_owned()));
    let (run, branches) = files.run_listing("p1", "r-sample").unwrap();
    assert_eq!(run.unwrap(), fs::read(dir.path().join("p1/runs/r-sample/run.json")).unwrap());
    assert_eq!(branches.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(), ["main"]);
    // An id that is not an identifier names no file.
    assert_eq!(files.run_listing("p1", "../r-sample").unwrap(), (None, Vec::new()));
    assert_eq!(files.run_file("p1", "nope", "run.json").unwrap(), None);
    let (project, scenarios, runs) = files.project_listing("p1", true).unwrap();
    assert!(project.is_some());
    assert_eq!(scenarios[0].0, "trial");
    assert_eq!(scenarios[0].1.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(), ["v1.json", "v1.scenario.json"]);
    assert_eq!(runs.iter().map(|(name, _, _)| name.as_str()).collect::<Vec<_>>(), ["r-sample", "run-browser-compute"]);
    assert!(files.project_listing("p1", false).unwrap().1.is_empty());

    // A writer may be appending: a line cut short is not read.
    let journal = dir.path().join("p1/runs/r-sample/branches/main/journal.jsonl");
    let whole = fs::read(&journal).unwrap();
    fs::write(&journal, [whole.as_slice(), b"{\"kind\":\"sav"].concat()).unwrap();
    assert_eq!(files.branch_file("p1", "r-sample", "main", "journal.jsonl").unwrap().unwrap(), whole);

    files.keep_report("p1", "r-sample", "main", b"{\"status\":\"x\"}\n").unwrap();
    assert_eq!(files.branch_file("p1", "r-sample", "main", "report.json").unwrap().unwrap(), b"{\"status\":\"x\"}\n");
    assert!(files.keep_report("p1", "r-sample", "nope", b"{}").is_err());
    files.keep_draft("p1", "d1", b"{}").unwrap();
    assert_eq!(files.draft("p1", "d1").unwrap().unwrap(), b"{}");
    assert_eq!(files.draft("p1", "d2").unwrap(), None);

    let reader = files.reader("p1", "r-sample", "main").unwrap();
    let pieces = reader.sample_pieces().unwrap();
    assert_eq!(pieces[0].0, "sample_tables");
    assert!(pieces[1..].iter().all(|(kind, _)| *kind == "sample_block" || *kind == "sample_tables"));
    assert!(reader.end_samples().unwrap() > 0);
}

#[test]
fn a_run_is_running_while_any_of_its_branches_computes() {
    let dir = TempDir::new("server-statuses");
    let made = make_browser_runs(dir.path(), ProjectFiles::new(dir.path(), None).unwrap());
    let files = &made.files;
    let fixture: Value = serde_json::from_slice(&crate::support::data("interventions.json")).unwrap();
    let (project, run) = (fixture["run"]["project_id"].as_str().unwrap(), fixture["run"]["run_id"].as_str().unwrap());
    files.set_branch_status(project, run, "main", "running", None).unwrap();
    files.set_branch_status(project, run, "c1", "running", None).unwrap();
    assert_eq!(status(&files.run_record(project, run).unwrap()), Some("running"));
    let record = files.set_branch_status(project, run, "c1", "paused", None).unwrap();
    assert_eq!(status(&record), Some("paused"));
    assert_eq!(status(&files.run_record(project, run).unwrap()), Some("running"));
    files.set_branch_status(project, run, "main", "completed", Some("paused")).unwrap();
    assert_eq!(status(&files.run_record(project, run).unwrap()), Some("paused"));
    assert_eq!(files.set_branch_status(project, run, "main", "lost", None).unwrap_err().to_string(), "unsupported run status");
}

#[test]
fn a_run_the_python_server_stored_before_427_is_refused_as_an_older_format() {
    let dir = TempDir::new("server-python-run");
    let files = ProjectFiles::new(dir.path(), None).unwrap();
    files.create_project("p").unwrap();
    let scenario = br#"{"duration_minutes":60,"id":"s","seed":1}"#;
    files.save_scenario("p", scenario, &sha256_hex(scenario)).unwrap();
    let engine = engine();
    let run = NewRun {
        run_id: "r",
        scenario_id: "s",
        scenario_version: 1,
        seed: 1,
        model_versions: &engine,
        evidence: br#"{"schema_version":1}"#,
        created_at: "2026-11-15T01:30:00Z",
    };
    files.create_run("p", &run).unwrap();
    // The Python server's Run: its Checkpoints beside the records, a branch's pointer, no journal.
    fs::create_dir_all(dir.path().join("p/runs/r/checkpoints")).unwrap();
    let hash = "a".repeat(64);
    files.write_branch("p", "r", &Branch { branch_id: "main".to_owned(), computed_until: Some(30), checkpoint_hash: Some(hash), ..Branch::default() }).unwrap();
    let error = files.reader("p", "r", "main").err().unwrap().to_string();
    assert!(error.starts_with("stored_run_outdated: "), "{error}");
    assert!(error.contains("Python の Checkpoint"), "{error}");
    let lease = files.claim_branch("p", "r", "main").unwrap();
    assert!(files.go_on(&lease, "p", "r", "main", 60).err().unwrap().to_string().starts_with("stored_run_outdated: "));
}

/// A bundle's manifest and entries, read back.
fn read_bundle(bundle: &[u8]) -> (Value, Vec<(String, Vec<u8>)>) {
    assert_eq!(&bundle[..16], b"\x89SHELTERSIM-PRJ\n");
    assert_eq!(u32::from_be_bytes(bundle[16..20].try_into().unwrap()), BUNDLE_VERSION);
    let length = u32::from_be_bytes(bundle[20..24].try_into().unwrap()) as usize;
    let manifest = &bundle[56..56 + length];
    let digest: String = bundle[24..56].iter().map(|byte| format!("{byte:02x}")).collect();
    assert_eq!(digest, sha256_hex(manifest));
    let manifest: Value = serde_json::from_slice(manifest).unwrap();
    let mut at = 56 + length;
    let mut entries = Vec::new();
    for entry in manifest["entries"].as_array().unwrap() {
        let end = at + entry["length"].as_u64().unwrap() as usize;
        assert_eq!(sha256_hex(&bundle[at..end]), entry["sha256"].as_str().unwrap());
        entries.push((entry["path"].as_str().unwrap().to_owned(), bundle[at..end].to_vec()));
        at = end;
    }
    assert_eq!(at, bundle.len());
    (manifest, entries)
}

#[test]
fn a_bundle_carries_a_projects_records_and_stored_runs_but_not_the_servers_own_files() {
    let dir = TempDir::new("server-bundle");
    let made = make_browser_runs(dir.path(), ProjectFiles::new(dir.path(), None).unwrap());
    let files = &made.files;
    let folder = dir.path().join("p1");
    for own in ["preparation.json", "drafts/d1.json", "usage/a.json", "runs/r-sample/usage.json", "runs/r-sample/llm.json", "runs/r-sample/worker-lock"] {
        fs::create_dir_all(folder.join(own).parent().unwrap()).unwrap();
        fs::write(folder.join(own), b"{}").unwrap();
    }
    fs::write(folder.join("runs/r-sample/branches/main/worker-lock"), b"").unwrap();
    let source = serde_json::json!({"tool": "test"});
    let bundle = files.export_project("p1", &source).unwrap();
    let (manifest, entries) = read_bundle(&bundle);
    assert_eq!((manifest["format"].as_str(), manifest["version"].as_u64()), (Some(BUNDLE_FORMAT), Some(BUNDLE_VERSION.into())));
    assert_eq!((manifest["project_id"].as_str(), &manifest["source"]), (Some("p1"), &source));
    let paths: Vec<&str> = entries.iter().map(|(path, _)| path.as_str()).collect();
    assert_eq!(paths[0], "project.json");
    for path in &paths {
        assert!(!path.contains("worker-lock") && !path.contains("usage") && !path.contains("llm.json") && !path.starts_with("drafts"), "{path}");
        assert_eq!(entries.iter().find(|(each, _)| each == path).unwrap().1, fs::read(folder.join(path)).unwrap());
    }
    for path in [
        "scenarios/trial/v1.json",
        "scenarios/trial/v1.scenario.json",
        "runs/r-sample/run.json",
        "runs/r-sample/evidence.json",
        "runs/r-sample/branches/main/journal.jsonl",
        "runs/r-sample/branches/main/data.pack",
        "runs/r-sample/branches/main/branch.json",
    ] {
        assert!(paths.contains(&path), "{path}");
    }
    // A file a bundle does not hold is refused, as the browser refuses it.
    fs::write(folder.join("runs/r-sample/notes.txt"), b"").unwrap();
    let error = files.export_project("p1", &source).unwrap_err().to_string();
    assert_eq!(error, "bundle_invalid: runs/r-sample/notes.txt: a file a bundle does not hold");
}
