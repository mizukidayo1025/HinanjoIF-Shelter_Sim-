//! A writer killed in the middle of a line, and readers while a writer appends (Issue #426).
//!
//! A line cut short at the journal's end (a writer that stopped while writing it) is not read: the
//! stored Run reads back to the lines before it. Going on from the branch's head cuts it off, with
//! the pieces after the head, and the Run computed on reaches the same heads as one computed
//! straight through. A reader that opens the stored Run while a writer appends reads every piece
//! of every line it sees. The processes killed for real are `tests/integration/test_run_files.py`'s.

use std::fs::OpenOptions;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use sheltersim_core::run_store::HeadKind;
use sheltersim_store::compute::plain_run;
use sheltersim_store::records::{NewRun, ProjectFiles};
use sheltersim_store::stored::{StoredRun, DATA, JOURNAL};

use crate::support::{data, engine, journal_of, resume, TempDir};

/// A store with Project `p`, Run `r` of tick_core's `small_replay` and its branch `main` at minute
/// 0 (its stored Run's "run" and "export" lines).
fn store_with_run(dir: &TempDir) -> ProjectFiles {
    let files = ProjectFiles::new(&dir.path().join("store"), None).unwrap();
    let tick = data("tick_core.json");
    let duration = plain_run(&tick, Some("small_replay")).unwrap().duration().unwrap();
    files.create_project("p").unwrap();
    let scenario = format!("{{\"duration_minutes\":{duration},\"id\":\"s\",\"seed\":1}}").into_bytes();
    files.save_scenario("p", &scenario, &sheltersim_core::checkpoint::sha256_hex(&scenario)).unwrap();
    let engine = engine();
    files
        .create_run("p", &NewRun { run_id: "r", scenario_id: "s", scenario_version: 1, seed: 1, model_versions: &engine, evidence: br#"{"schema_version":1}"# })
        .unwrap();
    let lease = files.claim_branch("p", "r", "main").unwrap();
    let start = sheltersim_store::stored::FromStart { export: &tick, world: Some("small_replay"), ticks: 0, save_every: 360 };
    let record = files.start_branch(&lease, "p", "r", "main", &start).unwrap();
    assert!(record.get("status").is_none() && record["computed_until"] == 0);
    files
}

fn heads(files: &ProjectFiles) -> Vec<String> {
    let journal = journal_of(&files.branch_folder("p", "r", "main").unwrap());
    journal.heads().iter().map(|head| journal.entries()[head.index].head.clone()).collect()
}

#[test]
fn a_line_cut_short_is_not_read_and_going_on_cuts_it_off() {
    let straight_dir = TempDir::new("straight");
    let straight = store_with_run(&straight_dir);
    assert_eq!(resume(&straight, "p", "r", "main", 1440, 60), "completed");

    let dir = TempDir::new("torn");
    let files = store_with_run(&dir);
    assert_eq!(resume(&files, "p", "r", "main", 700, 60), "paused");
    let folder = files.branch_folder("p", "r", "main").unwrap();
    let whole = journal_of(&folder);
    let lines = whole.entries().len();
    // A writer killed while it wrote: a piece in data.pack, and its line half written.
    let last = std::fs::read(folder.join(JOURNAL)).unwrap();
    let start = whole.entries()[lines - 2].end as usize;
    let half = &last[start..start + (last.len() - start) / 2];
    OpenOptions::new().append(true).open(folder.join(DATA)).unwrap().write_all(b"garbage the killed writer flushed").unwrap();
    OpenOptions::new().append(true).open(folder.join(JOURNAL)).unwrap().write_all(half).unwrap();
    let torn = journal_of(&folder);
    assert!(torn.torn());
    assert_eq!(torn.entries().len(), lines);
    let reader = StoredRun::open(&folder, &|_| unreachable!()).unwrap();
    assert!(reader.journal().torn());
    let verified = reader.verify().unwrap();
    assert!(verified.torn && !verified.complete && verified.mismatches.is_empty(), "{:?}", verified.mismatches);
    drop(reader);

    // The record's head is the saved point at 700; going on cuts the rest off.
    let head = journal_of(&folder).heads().into_iter().rev().find(|head| head.kind == HeadKind::Saved).unwrap();
    assert_eq!(head.minute, 700);
    assert_eq!(resume(&files, "p", "r", "main", 1440, 60), "completed");
    let after = journal_of(&folder);
    assert!(!after.torn());
    // The saved points up to 700 are the ones before the cut, and the end is the one computed
    // straight through (the saved points after differ only in where the blocks were sealed).
    assert_eq!(heads(&files)[..2], heads(&straight)[..2]);
    let end = |files: &ProjectFiles| journal_of(&files.branch_folder("p", "r", "main").unwrap()).entries().last().unwrap().head.clone();
    assert_eq!(end(&files), end(&straight));
    let verified = StoredRun::open(&folder, &|_| unreachable!()).unwrap().verify().unwrap();
    assert!(verified.complete && verified.verified && !verified.torn);
    assert_eq!(files.branch_record("p", "r", "main").unwrap()["checkpoint_hash"], straight.branch_record("p", "r", "main").unwrap()["checkpoint_hash"]);
}

#[test]
fn a_reader_reads_every_piece_of_the_lines_it_sees_while_a_writer_appends() {
    let dir = TempDir::new("readers");
    let files = store_with_run(&dir);
    let folder = files.branch_folder("p", "r", "main").unwrap();
    let done = Arc::new(AtomicBool::new(false));
    let reader = {
        let (folder, done) = (folder.clone(), done.clone());
        thread::spawn(move || {
            let (mut reads, mut most) = (0, 0);
            while !done.load(Ordering::Acquire) {
                let stored = StoredRun::open(&folder, &|_| unreachable!()).unwrap();
                for position in 0..stored.chain().pieces().len() {
                    stored.raw_at(position).unwrap();
                }
                most = most.max(stored.journal().entries().len());
                reads += 1;
            }
            (reads, most)
        })
    };
    assert_eq!(resume(&files, "p", "r", "main", 1440, 10), "completed");
    done.store(true, Ordering::Release);
    let (reads, most) = reader.join().unwrap();
    assert!(reads > 1, "{reads} reads");
    assert!(most > 2, "the reader saw {most} lines");
}
