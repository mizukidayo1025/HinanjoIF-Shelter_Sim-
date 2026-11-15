//! The locks of a store (Issue #426): one writer per branch, writers of different branches at once,
//! a whole-Run lease only while no branch is written, with Python's texts. Python's own holders of
//! the same locks, and other processes, are `tests/integration/test_run_files.py`'s.

use sheltersim_store::lock::{held, BRANCH_BUSY, BRANCH_OF_RUN_BUSY, RUN_BUSY};
use sheltersim_store::records::{NewRun, ProjectFiles, MISSING};

use crate::support::{engine, TempDir};

fn store(dir: &TempDir) -> ProjectFiles {
    let files = ProjectFiles::new(dir.path(), None).unwrap();
    files.create_project("p").unwrap();
    let scenario = br#"{"duration_minutes":60,"id":"s","seed":1}"#;
    files.save_scenario("p", scenario, &sheltersim_core::checkpoint::sha256_hex(scenario)).unwrap();
    let engine = engine();
    files
        .create_run(
            "p",
            &NewRun {
                run_id: "r",
                scenario_id: "s",
                scenario_version: 1,
                seed: 1,
                model_versions: &engine,
                evidence: br#"{"schema_version":1}"#,
                created_at: "2026-11-15T01:30:00Z",
            },
        )
        .unwrap();
    files
}

fn refusal(result: sheltersim_store::Result<sheltersim_store::lock::Lease>) -> String {
    result.unwrap_err().to_string()
}

#[test]
fn a_branch_has_one_writer_and_branches_are_written_at_once() {
    let dir = TempDir::new("locks-branch");
    let files = store(&dir);
    let main = files.claim_branch("p", "r", "main").unwrap();
    assert!(held(main.path()).unwrap());
    assert_eq!(refusal(files.claim_branch("p", "r", "main")), BRANCH_BUSY);
    let child = files.claim_branch("p", "r", "c1").unwrap();
    assert!(files.holds_branch(&child, "p", "r", "c1").unwrap() && !files.holds_branch(&child, "p", "r", "main").unwrap());
    assert_eq!(refusal(files.claim_run("p", "r")), BRANCH_OF_RUN_BUSY);
    main.release();
    assert_eq!(refusal(files.claim_run("p", "r")), BRANCH_OF_RUN_BUSY);
    child.release();
    let run = files.claim_run("p", "r").unwrap();
    assert_eq!(refusal(files.claim_branch("p", "r", "main")), RUN_BUSY);
    assert_eq!(refusal(files.claim_run("p", "r")), RUN_BUSY);
    drop(run);
    files.claim_branch("p", "r", "main").unwrap();
}

#[test]
fn a_lease_needs_the_runs_record_and_ids() {
    let dir = TempDir::new("locks-ids");
    let files = store(&dir);
    assert_eq!(refusal(files.claim_branch("p", "other", "main")), MISSING);
    assert_eq!(refusal(files.claim_run("p", "other")), MISSING);
    assert_eq!(refusal(files.claim_branch("p", "r", "../main")), "invalid artifact identifier");
    // A lease of another branch is not the writer's of this one.
    let lease = files.claim_branch("p", "r", "c1").unwrap();
    let error = files.go_on(&lease, "p", "r", "main", 10).err().unwrap();
    assert_eq!(error.to_string(), "the lease is not this branch's writer's");
}
