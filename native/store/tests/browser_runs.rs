//! The browser and the store write the same Runs (Issue #426): the six stored Runs of
//! `native/wasm/tests/node_stored_runs.mjs` made again through the store's files, locks and records.
//!
//! - With the fixtures' compressor (deflate's stored blocks, the same bytes everywhere), each
//!   branch's journal and data.pack are the browser's byte for byte, and the Projects' records are
//!   the ones `records.json` holds.
//! - With flate2's deflate-raw, the store's own, each journal has the same lines but where each piece
//!   is in data.pack and how long it is: each line's head hash and each piece's bytes before
//!   compression (its sha256 and length) are the browser's, so are the branches' records (their
//!   heads), and each stored Run reads back to its digests.

use std::fs;

use serde_json::Value;
use sheltersim_core::checkpoint::sha256_hex;
use sheltersim_core::run_store::Line;
use sheltersim_store::compress::stored_deflate;
use sheltersim_store::records::ProjectFiles;
use sheltersim_store::stored::{StoredRun, DATA, JOURNAL};

use crate::support::{data, data_fixture, journal_fixture, journal_of, make_browser_runs, TempDir};

const NAMES: [&str; 6] = ["from_zero", "no_points", "computed", "stopped", "fork_parent", "fork"];

/// The records of `records.json`, by path under the store's root.
fn record_fixtures() -> Vec<(String, String)> {
    let records: serde_json::Map<String, Value> = serde_json::from_slice(&data("stored_runs/records.json")).unwrap();
    records.into_iter().map(|(path, text)| (path, text.as_str().unwrap().to_owned())).collect()
}

/// A stored Run's reader with its parents' in the same Run's folder.
fn open(folder: &std::path::Path) -> StoredRun {
    let branches = folder.parent().unwrap().to_path_buf();
    StoredRun::open(folder, &|parent| Ok(branches.join(parent))).unwrap()
}

#[test]
fn the_store_writes_the_browsers_stored_runs_and_records_byte_for_byte() {
    let dir = TempDir::new("browser-bytes");
    let made = make_browser_runs(dir.path(), ProjectFiles::new(&dir.path().join("store"), None).unwrap().with_compress(stored_deflate));
    for name in NAMES {
        let folder = made.folder(name);
        let journal = fs::read(folder.join(JOURNAL)).unwrap();
        assert!(journal == journal_fixture(name), "{name}: the journal is not the browser's");
        let pack = fs::read(folder.join(DATA)).unwrap();
        assert_eq!((pack.len() as u64, sha256_hex(&pack)), data_fixture(name), "{name}: data.pack is not the browser's");
        let verified = open(folder).verify().unwrap();
        assert!(verified.complete && verified.verified && !verified.torn, "{name}: {:?}", verified.mismatches);
    }
    for (path, text) in record_fixtures() {
        let written = fs::read_to_string(dir.path().join("store").join(&path)).unwrap();
        assert_eq!(written, text, "{path}");
    }
}

#[test]
fn with_flate2_the_lines_hold_the_browsers_pieces_and_heads_where_the_bytes_differ() {
    let dir = TempDir::new("browser-flate2");
    let made = make_browser_runs(dir.path(), ProjectFiles::new(&dir.path().join("store"), None).unwrap());
    for name in NAMES {
        let folder = made.folder(name);
        let browser = sheltersim_core::run_store::Journal::parse(&journal_fixture(name)).unwrap();
        let ours = journal_of(folder);
        assert!(!ours.torn(), "{name}");
        assert_eq!(ours.entries().len(), browser.entries().len(), "{name}");
        let mut pieces = 0;
        for (mine, theirs) in ours.entries().iter().zip(browser.entries()) {
            assert_eq!(mine.head, theirs.head, "{name}: {}", theirs.json);
            match (&mine.line, &theirs.line) {
                (Line::Piece(mine), Line::Piece(theirs)) => {
                    assert_eq!((&mine.piece, mine.stored.raw_length, &mine.stored.sha256), (&theirs.piece, theirs.stored.raw_length, &theirs.stored.sha256));
                    pieces += 1;
                }
                (mine, theirs) => assert_eq!(mine, theirs, "{name}"),
            }
        }
        assert!(pieces > 0, "{name}");
        // flate2 compresses: data.pack is not the fixtures' stored blocks, and smaller.
        let pack = fs::read(folder.join(DATA)).unwrap();
        assert!((pack.len() as u64) < data_fixture(name).0, "{name}: {} bytes", pack.len());
        let verified = open(folder).verify().unwrap();
        assert!(verified.complete && verified.verified && !verified.torn, "{name}: {:?}", verified.mismatches);
    }
    // The records hold the same heads: the same bytes.
    for (path, text) in record_fixtures() {
        assert_eq!(fs::read_to_string(dir.path().join("store").join(&path)).unwrap(), text, "{path}");
    }
}
