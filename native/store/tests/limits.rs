//! The store's limit (the server's `--project-storage-gb`, Issue #426): checked before each write,
//! with Python's text; a computation stopped by it leaves the lines it wrote whole and its record at
//! its last head.

use sheltersim_store::quota::{usage, Quota, CAPACITY_EXCEEDED};
use sheltersim_store::records::{NewRun, ProjectFiles};
use sheltersim_store::stored::{FromStart, StoredRun};

use crate::support::{data, engine, journal_of, TempDir};

#[test]
fn a_quota_counts_writes_on_what_it_measured() {
    let dir = TempDir::new("quota");
    std::fs::create_dir_all(dir.path().join("a/b")).unwrap();
    std::fs::write(dir.path().join("a/b/one"), [0u8; 10]).unwrap();
    std::fs::write(dir.path().join("two"), [0u8; 5]).unwrap();
    assert_eq!(usage(dir.path()).unwrap(), sheltersim_store::quota::Usage { bytes: 15, files: 2 });
    let mut quota = Quota::measure(dir.path(), Some(20)).unwrap();
    quota.reserve(5).unwrap();
    assert_eq!(quota.reserve(1).unwrap_err().to_string(), CAPACITY_EXCEEDED);
    quota.release(3);
    quota.reserve(3).unwrap();
    assert_eq!(Quota::measure(dir.path(), None).unwrap(), Quota::unlimited());
}

#[test]
fn a_computation_past_the_limit_stops_with_its_lines_whole() {
    let dir = TempDir::new("limits");
    let open = ProjectFiles::new(dir.path(), None).unwrap();
    open.create_project("p").unwrap();
    let scenario = br#"{"duration_minutes":1440,"id":"s","seed":1}"#;
    open.save_scenario("p", scenario, &sheltersim_core::checkpoint::sha256_hex(scenario)).unwrap();
    let engine = engine();
    open.create_run(
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
    // Room for half the stored Run.
    let whole = TempDir::new("limits-whole");
    let unlimited = ProjectFiles::new(whole.path(), None).unwrap();
    let tick = data("tick_core.json");
    let start = FromStart { export: &tick, world: Some("small_replay"), ticks: 60, save_every: 360 };
    sheltersim_store::stored::run_and_store(
        whole.path(),
        &start,
        sheltersim_core::model_version::engine_signatures(),
        unlimited.quota().unwrap(),
        unlimited.compress(),
    )
    .unwrap();
    let limit = usage(dir.path()).unwrap().bytes + usage(whole.path()).unwrap().bytes / 2;
    let files = ProjectFiles::new(dir.path(), Some(limit)).unwrap();
    let lease = files.claim_branch("p", "r", "main").unwrap();
    let error = files.compute_branch(&lease, "p", "r", "main", &start).unwrap_err();
    assert_eq!(error.to_string(), CAPACITY_EXCEEDED);
    assert!(usage(dir.path()).unwrap().bytes <= limit);
    let record = files.branch_record("p", "r", "main").unwrap();
    assert_eq!(record["status"], "failed");
    let folder = files.branch_folder("p", "r", "main").unwrap();
    let journal = journal_of(&folder);
    assert!(!journal.torn() && journal.entries().len() > 2);
    assert!(StoredRun::open(&folder, &|_| unreachable!()).unwrap().verify().unwrap().mismatches.is_empty());
    // A record past the limit is refused too.
    let big = vec![b' '; 50_000];
    let mut text = br#"{"duration_minutes":1440,"id":"t","seed":1}"#.to_vec();
    text.extend_from_slice(&big);
    assert_eq!(files.save_scenario("p", &text, &sheltersim_core::checkpoint::sha256_hex(&text)).unwrap_err().to_string(), CAPACITY_EXCEEDED);
}
