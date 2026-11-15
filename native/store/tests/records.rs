//! The records of a store (Issue #426): the browser's shapes (`project_store.mjs`) and Python's
//! refusals. That the records of the browser's Runs are written byte for byte is `browser_runs`'.

use std::fs;

use serde_json::json;
use sheltersim_store::records::{parse_record, parse_run_record, Branch, NewRun, ProjectFiles, MISSING, UNSUPPORTED};

use crate::support::{engine, TempDir};

fn text(result: sheltersim_store::Result<impl std::fmt::Debug>) -> String {
    result.unwrap_err().to_string()
}

#[test]
fn the_records_are_sorted_json_with_a_newline_and_read_back() {
    let dir = TempDir::new("records");
    let files = ProjectFiles::new(dir.path(), None).unwrap();
    files.create_project("p").unwrap();
    assert_eq!(fs::read_to_string(dir.path().join("p/project.json")).unwrap(), "{\"archived\":false,\"project_id\":\"p\",\"schema_version\":1}\n");
    // Written again with the same bytes.
    files.create_project("p").unwrap();
    assert_eq!(files.projects().unwrap(), vec![files.project("p").unwrap()]);
    let scenario = br#"{"duration_minutes":60,"id":"s","seed":1}"#;
    let hash = sheltersim_core::checkpoint::sha256_hex(scenario);
    assert_eq!(text(files.save_scenario("p", scenario, &"0".repeat(64))), "scenario approval hash does not match content");
    let first = files.save_scenario("p", scenario, &hash).unwrap();
    let second = files.save_scenario("p", scenario, &hash).unwrap();
    assert_eq!((first["scenario_version"].as_i64(), second["scenario_version"].as_i64()), (Some(1), Some(2)));
    assert_eq!(
        files.scenarios("p").unwrap().iter().map(|(record, text)| (record["scenario_version"].as_i64().unwrap(), text.clone())).collect::<Vec<_>>(),
        [(1, scenario.to_vec()), (2, scenario.to_vec())]
    );
    let engine = engine();
    let new = NewRun {
        run_id: "r",
        scenario_id: "s",
        scenario_version: 1,
        seed: 1,
        model_versions: &engine,
        evidence: br#"{"schema_version":1}"#,
        created_at: "2026-11-15T01:30:00Z",
    };
    let run = files.create_run("p", &new).unwrap();
    assert_eq!(run["status"], "queued");
    // Schema version 2 keeps the creation time the caller gives (#447).
    assert_eq!((run["schema_version"].as_i64(), run["created_at"].as_str()), (Some(2), Some("2026-11-15T01:30:00Z")));
    for created_at in ["2026-11-15T01:30:00", "2026-11-15 01:30:00Z", "2026-11-15T01:30:00.000Z", ""] {
        assert_eq!(text(files.create_run("p", &NewRun { run_id: "r2", created_at, ..new.clone() })), "invalid creation time");
    }
    assert_eq!(files.create_run("p", &new).unwrap(), run);
    assert_eq!(text(files.create_run("p", &NewRun { scenario_version: 2, ..new.clone() })), "run_id_conflict");
    assert_eq!(text(files.create_run("p", &NewRun { run_id: "r2", seed: -1, ..new.clone() })), "invalid seed");
    assert_eq!(text(files.create_run("p", &NewRun { run_id: "r2", model_versions: &json!({"a": 1}), ..new.clone() })), "invalid model versions");
    assert_eq!(text(files.create_run("p", &NewRun { run_id: "r2", evidence: b"{}", ..new.clone() })), UNSUPPORTED);
    assert_eq!(text(files.set_run_status("p", "r", "done")), "unsupported run status");
    assert_eq!(files.set_run_status("p", "r", "paused").unwrap()["status"], "paused");
    assert_eq!(files.run_ids("p").unwrap(), ["r"]);
    assert_eq!(files.run_scenario_bytes("p", "r").unwrap(), scenario);
    assert_eq!(files.evidence_bytes("p", "r").unwrap(), br#"{"schema_version":1}"#);

    // A branch without a status, its child, and the lineage that does not change.
    let main = Branch { branch_id: "main".to_owned(), ..Branch::default() };
    let record = files.write_branch("p", "r", &main).unwrap();
    assert!(record.get("status").is_none());
    assert_eq!(
        fs::read_to_string(dir.path().join("p/runs/r/branches/main/branch.json")).unwrap(),
        "{\"branch_id\":\"main\",\"checkpoint_hash\":null,\"computed_until\":0,\"fork_minute\":0,\"parent_branch_id\":null,\"run_id\":\"r\",\"schema_version\":1}\n"
    );
    let child = Branch { branch_id: "c1".to_owned(), parent_branch_id: Some("main".to_owned()), fork_minute: 30, ..Branch::default() };
    assert_eq!(files.write_branch("p", "r", &child).unwrap()["computed_until"].as_i64(), Some(30));
    assert_eq!(text(files.write_branch("p", "r", &Branch { fork_minute: 20, ..child.clone() })), "branch lineage is immutable");
    assert_eq!(text(files.write_branch("p", "r", &Branch { parent_branch_id: Some("gone".to_owned()), ..child.clone() })), MISSING);
    assert_eq!(text(files.write_branch("p", "r", &Branch { computed_until: Some(10), ..child.clone() })), "invalid computed minute");
    assert_eq!(text(files.write_branch("p", "r", &Branch { checkpoint_hash: Some("x".to_owned()), ..child.clone() })), "invalid checkpoint hash");
    assert_eq!(text(files.write_branch("p", "r", &Branch { status: Some("x".to_owned()), ..child.clone() })), "unsupported run status");
    assert_eq!(files.branches("p", "r").unwrap().iter().map(|record| record["branch_id"].as_str().unwrap().to_owned()).collect::<Vec<_>>(), ["c1", "main"]);
    files.remove_run("p", "r").unwrap();
    assert_eq!(text(files.remove_run("p", "r")), MISSING);
    files.remove_project("p").unwrap();
    assert!(files.projects().unwrap().is_empty());
}

#[test]
fn ids_records_and_paths_are_refused_as_python_refuses_them() {
    let dir = TempDir::new("records-refused");
    let files = ProjectFiles::new(dir.path(), None).unwrap();
    for id in ["", "-p", "a/b", "..", &"a".repeat(65)] {
        assert_eq!(text(files.create_project(id)), "invalid artifact identifier", "{id:?}");
    }
    assert_eq!(text(files.create_project("runtime-accounts")), "project_id_reserved_for_usage_accounts");
    files.create_project("p").unwrap();
    fs::write(dir.path().join("p/project.json"), "{\"schema_version\":1}\n").unwrap();
    assert_eq!(text(files.create_project("p")), "immutable saved artifact already exists");
    assert_eq!(text(parse_record(b"{")), MISSING);
    assert_eq!(text(parse_record(b"{\"schema_version\":2}")), UNSUPPORTED);
    assert_eq!(text(parse_record(b"[1]")), UNSUPPORTED);
    // A Run's record: version 1 (before #447, no creation time) is read as it is; version 2 must
    // have its creation time; another version is refused.
    assert!(parse_run_record(b"{\"schema_version\":1}").is_ok());
    assert!(parse_run_record(b"{\"created_at\":\"2026-11-15T01:30:00Z\",\"schema_version\":2}").is_ok());
    assert_eq!(text(parse_run_record(b"{\"schema_version\":2}")), MISSING);
    assert_eq!(text(parse_run_record(b"{\"created_at\":\"11/15\",\"schema_version\":2}")), MISSING);
    assert_eq!(text(parse_run_record(b"{\"schema_version\":3}")), UNSUPPORTED);
    assert_eq!(text(files.run_record("p", "r")), MISSING);
    assert_eq!(text(ProjectFiles::new(dir.path(), Some(0))), "max_bytes must be positive");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(dir.path().join("p"), dir.path().join("q")).unwrap();
        assert_eq!(text(files.project("q")), "artifact path cannot follow symlinks");
    }
}
