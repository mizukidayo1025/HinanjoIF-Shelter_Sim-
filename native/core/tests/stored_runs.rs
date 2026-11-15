//! The format of a stored Run (`run_store`, format v6 of #478): the journals the browser writes
//! (`tests/data/stored_runs/`, written by `native/wasm/tests/node_stored_runs.mjs`, which checks
//! that the browser still writes them) read back, each line encoded again to its bytes; the lines
//! of the format and the ones it refuses (an older format among them: compute it again); lines cut
//! short or not matching their sha256; the heads and their hash, the same whatever compressor wrote
//! the pieces; the cuts, the seeks and the reads a restore needs, through the chain of a fork and
//! its parent; the saved points' schedule; the append-only records and their counts; the check of
//! private data; the model versions; and the writer, which writes the same journals and data.pack
//! again from the same Runs (with the stand-in compression of the fixtures: deflate's stored
//! blocks), and whose pieces read back to the Run's digests.

use std::fs;

use serde_json::Value;
use sheltersim_core::archive::{Restore, SavedPoint};
use sheltersim_core::checkpoint::sha256_hex;
use sheltersim_core::model_version::{self, ModelStatus, Versions};
use sheltersim_core::plain::PlainRun;
use sheltersim_core::run_store::{
    records, Chain, Fork, ForkOrigin, HeadKind, Journal, Line, Next, Piece, PieceLine, RecordKind, RunLine, RunWriter, Schedule, Storing, Verified, Verify,
    FORMAT, OUTDATED_CODE, SAVE_EVERY,
};
use sheltersim_core::sensitive;

const NAMES: [&str; 5] = ["from_zero", "no_points", "computed", "fork_parent", "fork"];

fn data(path: &str) -> Vec<u8> {
    fs::read(format!("{}/tests/data/{path}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn journal_bytes(name: &str) -> Vec<u8> {
    data(&format!("stored_runs/{name}.journal.jsonl"))
}

fn journal(name: &str) -> Journal {
    Journal::parse(&journal_bytes(name)).unwrap()
}

fn summary(name: &str) -> Value {
    let runs: Value = serde_json::from_slice(&data("stored_runs/runs.json")).unwrap();
    runs[name].clone()
}

/// A journal line of `json`, with its sha256: written whole.
fn line(json: &str) -> Vec<u8> {
    format!("{json}\t{}\n", sha256_hex(json.as_bytes())).into_bytes()
}

fn error_text(bytes: &[u8]) -> String {
    match Journal::parse(bytes) {
        Ok(journal) => panic!("read {} lines of a journal that is not one", journal.entries().len()),
        Err(error) => error.to_string(),
    }
}

/// The minutes of the "saved" lines `indexes` name.
fn minutes(journal: &Journal, indexes: &[usize]) -> Vec<i64> {
    indexes.iter().map(|index| journal.saved(*index).unwrap().minute).collect()
}

/// How the browser's fixtures are stored: the engine's items as the versions (`engineSignatures`).
fn storing(save_every: u64) -> Storing {
    Storing { model_versions: model_version::engine_signatures(), compression: "deflate-raw".to_owned(), save_every }
}

#[test]
fn every_line_of_each_stored_run_is_encoded_again_byte_for_byte() {
    for name in NAMES {
        let bytes = journal_bytes(name);
        let journal = Journal::parse(&bytes).unwrap();
        assert!(!journal.torn(), "{name}");
        assert_eq!(sha256_hex(&bytes), summary(name)["journal_sha256"].as_str().unwrap(), "{name}");
        let mut at = 0;
        for entry in journal.entries() {
            let end = entry.end as usize;
            let encoded = entry.line.encode();
            assert_eq!(encoded.bytes, bytes[at..end], "{name}: {}", entry.json);
            assert_eq!(encoded.sha256, entry.sha256, "{name}");
            assert_eq!(encoded.head, entry.head, "{name}");
            assert_eq!(Line::of_json(entry.json.as_bytes()).unwrap(), entry.line, "{name}");
            at = end;
        }
        assert_eq!(at, bytes.len(), "{name}: every byte is a line");
    }
}

#[test]
fn a_line_is_written_as_json_stringify_writes_its_object() {
    // Node's JSON.stringify of the same object: control characters as \u00XX, DEL and U+2028 as
    // they are, the text after them as UTF-8.
    let fork = journal("fork");
    let mut line = fork.fork().unwrap().clone();
    line.program = "{\"text\":\"トイレを1基追加\"}\t\u{1}\u{1f}\u{7f}\u{2028}\"\\\u{8}\u{c}\n\r".to_owned();
    let json = String::from_utf8(Line::Fork(line.clone()).to_json()).unwrap();
    let wanted = concat!(r#""program":"{\"text\":\"トイレを1基追加\"}\t\u0001\u001f"#, "\u{7f}\u{2028}", r#"\"\\\b\f\n\r","approved_program_hash""#);
    assert!(json.contains(wanted), "{json}");
    assert!(json.starts_with(r#"{"kind":"fork","parent_branch_id":"main","minute":400,"fork_origin":{"branch_id":"main","minute":400,"#), "{json}");
    assert!(json.contains(r#""base":{"line":"#), "{json}");
    assert_eq!(Line::of_json(json.as_bytes()).unwrap(), Line::Fork(line));
}

#[test]
fn each_stored_run_is_of_this_format_and_says_how_it_was_computed_and_stored() {
    for name in NAMES {
        let journal = journal(name);
        let run = journal.run().unwrap();
        assert_eq!(run.engine_version, model_version::ENGINE_VERSION, "{name}");
        assert_eq!(run.compression, "deflate-raw", "{name}");
        assert_eq!(run.save_every, if name == "no_points" { 0 } else { SAVE_EVERY }, "{name}");
        assert_eq!(run.model_versions.get(model_version::ENGINE_VERSION_KEY).map(String::as_str), Some(model_version::ENGINE_VERSION), "{name}");
        assert!(journal.entries()[0].json.contains(&format!(r#""format":"{FORMAT}""#)));
        assert_eq!(journal.end().unwrap().minute, run.duration_minutes, "{name}");
        assert_eq!(journal.fork().is_some(), name == "fork", "{name}");
    }
    let fork = journal("fork");
    let run = fork.run().unwrap();
    assert_eq!((run.fork_minute(), run.fork.as_ref().unwrap().parent_branch_id.as_str()), (400, "main"));
    let line = fork.fork().unwrap();
    assert_eq!((line.minute, line.origin.branch_id.as_str(), line.origin.minute, line.base.minute), (400, "main", 400, 360));
    assert!(journal("no_points").saved_points().is_empty());
}

#[test]
fn a_run_of_an_older_format_is_computed_again() {
    let v4 = r#"{"kind":"run","format":"sheltersim-browser-run-v4","export_sha256":"x","export_bytes":2,"world":null,"duration_minutes":60,"save_every":360}"#;
    let text = error_text(&line(v4));
    assert!(text.starts_with(&format!("{OUTDATED_CODE}: ")), "{text}");
    assert!(text.contains("sheltersim-browser-run-v4") && text.contains("計算し直してください"), "{text}");
    for old in ["sheltersim-browser-run-v1", "sheltersim-browser-run-v2", "sheltersim-browser-run-v3"] {
        assert!(error_text(&line(&v4.replace("sheltersim-browser-run-v4", old))).starts_with(OUTDATED_CODE));
    }
    let text = error_text(&line(&v4.replace("sheltersim-browser-run-v4", "sheltersim-run-v9")));
    assert_eq!(text, format!("a stored Run of format sheltersim-run-v9: this code reads {FORMAT}"));
}

#[test]
fn a_journal_cut_anywhere_reads_back_to_the_whole_lines_before_the_cut() {
    for name in ["from_zero", "fork"] {
        let bytes = journal_bytes(name);
        let ends: Vec<usize> = journal(name).entries().iter().map(|entry| entry.end as usize).collect();
        for cut in 0..=bytes.len() {
            let read = Journal::parse(&bytes[..cut]).unwrap();
            let whole = ends.iter().filter(|end| **end <= cut).count();
            assert_eq!(read.entries().len(), whole, "{name} cut at {cut}");
            assert_eq!(read.torn(), cut > 0 && !ends.contains(&cut), "{name} cut at {cut}");
        }
    }
    let empty = Journal::parse(b"").unwrap();
    assert!(empty.entries().is_empty() && !empty.torn());
    assert_eq!(empty.run().unwrap_err().to_string(), "not a stored Run");
}

#[test]
fn a_line_that_does_not_match_its_sha256_ends_the_journal() {
    let bytes = journal_bytes("from_zero");
    let ends: Vec<usize> = journal("from_zero").entries().iter().map(|entry| entry.end as usize).collect();
    // A byte of the fourth line's JSON, then of its sha256.
    for at in [ends[2] + 10, ends[3] - 3] {
        let mut changed = bytes.clone();
        changed[at] = if changed[at] == b'0' { b'1' } else { b'0' };
        let read = Journal::parse(&changed).unwrap();
        assert_eq!((read.entries().len(), read.torn()), (3, true), "byte {at}");
    }
    // A line without its tab, and an empty line.
    for extra in [&b"{\"kind\":\"export\"}\n"[..], b"\n"] {
        let read = Journal::parse(&[&bytes[..ends[0]], extra].concat()).unwrap();
        assert_eq!((read.entries().len(), read.torn()), (1, true));
    }
}

/// The JSON of `name`'s line `index` with `edit` made to its object (its keys then sorted, which
/// the reader does not mind).
fn edited(name: &str, index: usize, edit: impl FnOnce(&mut serde_json::Map<String, Value>)) -> String {
    let mut value: Value = serde_json::from_str(&journal(name).entries()[index].json).unwrap();
    edit(value.as_object_mut().unwrap());
    serde_json::to_string(&value).unwrap()
}

#[test]
fn a_whole_line_that_is_not_a_line_of_the_format_is_an_error() {
    let bytes = journal_bytes("from_zero");
    let read = journal("from_zero");
    let upto = |index: usize| bytes[..read.entries()[index].end as usize].to_vec();
    let after = |index: usize, json: &str| [upto(index), line(json)].concat();
    let json = |index: usize| read.entries()[index].json.clone();
    let saved = read.find("saved").unwrap();
    let block = read.find("log_block").unwrap();
    let end = read.end_index().unwrap();
    let fork = journal("fork");
    let fork_bytes = journal_bytes("fork");
    let fork_run = &fork_bytes[..fork.entries()[0].end as usize];
    let cases: Vec<(Vec<u8>, &str)> = vec![
        (after(1, "not json"), "a line that is not JSON"),
        (after(1, "[1]"), "a line that is not an object"),
        (after(1, r#"{"kind":"nonsense"}"#), "a line of an unknown kind: nonsense"),
        (after(1, r#"{"offset":0}"#), "a line without kind"),
        (after(1, r#"{"kind":"log_block","offset":0,"length":1,"raw_length":1,"index":0}"#), "a line without sha256"),
        (after(1, r#"{"kind":"log_block","offset":"0","length":1,"raw_length":1,"sha256":"x","index":0}"#), "offset of a line is not a count"),
        (after(1, r#"{"kind":"log_block","offset":1.5,"length":1,"raw_length":1,"sha256":"x","index":0}"#), "offset of a line is not a count"),
        (after(1, r#"{"kind":"log_block","offset":-1,"length":1,"raw_length":1,"sha256":"x","index":0}"#), "offset of a line is not a count"),
        (after(1, r#"{"kind":"log_block","offset":0,"length":1,"raw_length":1,"sha256":"x"}"#), "a line without index"),
        (after(1, r#"{"kind":"log_block","offset":0,"length":1,"raw_length":1,"sha256":"x","index":0,"more":1}"#), "a key this format does not have: more"),
        (after(1, r#"{"kind":"log_block","offset":0,"length":1,"raw_length":1,"sha256":"x","index":3}"#), "a block that is not the next one"),
        (
            after(1, r#"{"kind":"log_tables","offset":0,"length":1,"raw_length":1,"sha256":"x","strings_from":5,"shapes_from":0,"strings":6,"shapes":0}"#),
            "tables that do not follow",
        ),
        (after(1, &json(0)), "a second run line"),
        (after(1, &json(1)), "an exported world that is not the second line"),
        (after(0, &json(2)), "a Run from minute 0 without its exported world"),
        (after(1, &json(saved)), "a saved point whose counts are not the blocks and records stored before it"),
        (after(saved, &json(saved)), "out of order"),
        (
            after(
                saved - 1,
                &edited("from_zero", saved, |line| {
                    line.insert("point_version".into(), 1.into());
                }),
            ),
            "a saved point of version 1",
        ),
        (after(1, &json(end)), "an end line that does not follow the world"),
        (
            after(
                end - 1,
                &edited("from_zero", end, |line| {
                    line.insert("tape".into(), 1.into());
                }),
            ),
            "an end line whose counts are not the records stored",
        ),
        (
            after(
                block - 1,
                &edited("from_zero", block, |line| {
                    line.insert("index".into(), 1.into());
                }),
            ),
            "a block that is not the next one",
        ),
        (line(&json(1)), "its first line is not the run line"),
        ([&bytes[..], &line(&json(block))].concat(), "a line after the end line"),
        (after(1, &fork.entries()[1].json), "a fork line that is not the second line of a fork's Run"),
        ([fork_run, &line(&json(1))].concat(), "an exported world that is not the second line"),
        ([fork_run, &line(&fork.entries()[2].json)].concat(), "a fork's Run without its fork line"),
        (
            [
                fork_run,
                &line(&edited("fork", 1, |line| {
                    line.insert("minute".into(), 401.into());
                })),
            ]
            .concat(),
            "a fork line of another fork",
        ),
        ([&upto(end - 1)[..], &line(&json(saved))].concat(), "a line between the world and the end line"),
    ];
    for (bytes, wanted) in cases {
        let text = error_text(&bytes);
        assert!(text.contains(wanted), "{wanted}: {text}");
    }
}

#[test]
fn the_heads_are_the_export_every_saved_point_and_the_end() {
    let from_zero = journal("from_zero");
    let heads = from_zero.heads();
    assert_eq!((heads[0].kind, heads[0].minute, heads[0].index), (HeadKind::Export, 0, 1));
    let last = heads.last().unwrap();
    assert_eq!((last.kind, last.minute, last.index), (HeadKind::End, 1440, from_zero.end_index().unwrap()));
    assert_eq!(heads.iter().filter(|head| head.kind == HeadKind::Saved).map(|head| head.minute).collect::<Vec<_>>(), [360, 720, 1080, 1440]);
    // Computed in chunks to a target between saved points, and on.
    let computed = journal("computed");
    let kinds: Vec<(HeadKind, i64)> = computed.heads().iter().map(|head| (head.kind, head.minute)).collect();
    assert_eq!(
        kinds,
        [
            (HeadKind::Export, 0),
            (HeadKind::Saved, 360),
            (HeadKind::Saved, 500),
            (HeadKind::Saved, 720),
            (HeadKind::Saved, 1080),
            (HeadKind::Saved, 1440),
            (HeadKind::End, 1440)
        ]
    );
    // A fork's Run starts at its saved point at the fork: its exported world is its parent's.
    let fork = journal("fork");
    let kinds: Vec<(HeadKind, i64)> = fork.heads().iter().map(|head| (head.kind, head.minute)).collect();
    assert_eq!(kinds, [(HeadKind::Saved, 400), (HeadKind::Saved, 480), (HeadKind::End, 480)]);
    // A head's hash is of its line without the piece's place.
    for head in computed.heads() {
        let entry = &computed.entries()[head.index];
        assert_eq!(computed.head(&entry.head), Some(head));
        assert_eq!(entry.head, sha256_hex(&entry.line.head_json()));
        let mut whole: Value = serde_json::from_str(&entry.json).unwrap();
        whole.as_object_mut().unwrap().remove("offset");
        whole.as_object_mut().unwrap().remove("length");
        assert_eq!(serde_json::from_slice::<Value>(&entry.line.head_json()).unwrap(), whole);
    }
    assert_eq!(computed.head(&"0".repeat(64)), None);
    // A line's sha256 is no head's hash.
    let saved = computed.saved_points()[0];
    assert_eq!(computed.head(&computed.entries()[saved].sha256), None);
}

#[test]
fn a_cut_keeps_the_lines_up_to_the_head_and_the_bytes_they_point_at() {
    let bytes = journal_bytes("computed");
    let computed = journal("computed");
    for head in computed.heads() {
        let hash = &computed.entries()[head.index].head;
        let cut = computed.cut(hash, None).unwrap();
        assert_eq!(cut.head, head);
        assert_eq!(cut.journal_end, computed.entries()[head.index].end);
        let kept = Journal::parse(&bytes[..cut.journal_end as usize]).unwrap();
        assert_eq!(kept.entries(), &computed.entries()[..=head.index]);
        let pieces = kept.entries().iter().filter_map(|entry| match &entry.line {
            Line::Piece(line) => Some(line.stored.offset + line.stored.length),
            _ => None,
        });
        assert_eq!(cut.data_end, pieces.max().unwrap());
        // A fork that reads the branch up to a later line keeps the cut from taking it away.
        assert!(computed.cut(hash, Some(head.index)).is_ok());
        let text = computed.cut(hash, Some(head.index + 1)).unwrap_err().to_string();
        assert_eq!(text, "a branch forked from this one reads its stored Run past the branch's head");
    }
    let last = computed.end_index().unwrap();
    assert_eq!(computed.cut(&computed.entries()[last].head, None).unwrap().data_end, summary("computed")["data_bytes"].as_u64().unwrap());
    // A line that is no head (a block's) is no place to cut back to.
    let block = computed.find("log_block").unwrap();
    let text = computed.cut(&computed.entries()[block].head, None).unwrap_err().to_string();
    assert_eq!(text, "the branch's head is not a line its stored Run can go on from");
}

/// The chain of the fixture `fork` and its parent.
fn fork_chain() -> Chain {
    let mut chain = Chain::new(journal("fork")).unwrap();
    assert_eq!(chain.parent_needed(), Some("main"));
    assert!(!chain.complete());
    assert!(chain.seek(450).unwrap_err().to_string().contains("needs its parent's (branch main)"));
    chain.add_parent(journal("fork_parent")).unwrap();
    assert!(chain.complete() && chain.parent_needed().is_none());
    chain
}

#[test]
fn a_seek_starts_at_the_nearest_saved_point_at_or_before_the_minute() {
    let from_zero = Chain::new(journal("from_zero")).unwrap();
    let leaf = from_zero.leaf();
    let points = leaf.saved_points();
    assert_eq!(minutes(leaf, &points), [360, 720, 1080, 1440]);
    let export = from_zero.export().unwrap();
    for (minute, from) in [(0, None), (1, None), (359, None), (360, Some(0)), (361, Some(0)), (1079, Some(1)), (1439, Some(2)), (1440, Some(3))] {
        let seek = from_zero.seek(minute).unwrap();
        assert_eq!(seek.from, from.map(|at: usize| points[at]), "minute {minute}");
        assert_eq!(seek.base, seek.from.unwrap_or(1), "minute {minute}");
        assert_eq!(seek.from_minute, from.map_or(0, |at| [360, 720, 1080, 1440][at]), "minute {minute}");
        assert_eq!((seek.minute, seek.export), (minute, export));
    }
    for minute in [-1, 1441] {
        assert_eq!(from_zero.seek(minute).unwrap_err().to_string(), format!("minute {minute} is not one of the Run (0 to 1440)"));
    }
    // A fork's Run: no minute before the fork's, from its saved points, its exported world its
    // parent's.
    let fork = fork_chain();
    assert_eq!(fork.seek(399).unwrap_err().to_string(), "minute 399 is not one of the Run (400 to 480)");
    assert_eq!(fork.seek(400).unwrap().from_minute, 400);
    assert_eq!(fork.seek(479).unwrap().from_minute, 400);
    assert_eq!(fork.seek(480).unwrap().from_minute, 480);
    assert_eq!(fork.pieces()[fork.export().unwrap()].level, 0);
    // What a Run of other model versions shows for a minute (D2): the saved point at or before it.
    let leaf = journal("from_zero");
    for (minute, saved) in [(0, 0), (359, 0), (360, 360), (1000, 720), (1440, 1440)] {
        assert_eq!(leaf.saved_minute_at(minute).unwrap(), saved, "minute {minute}");
    }
    assert!(leaf.saved_minute_at(1441).is_err());
    let fork_leaf = journal("fork");
    assert_eq!((fork_leaf.saved_minute_at(400).unwrap(), fork_leaf.saved_minute_at(479).unwrap()), (400, 400));
    assert!(fork_leaf.saved_minute_at(399).is_err());
    // Without its saved points, a fork's Run has nowhere to start from.
    let bytes = journal_bytes("fork");
    let leaf = journal("fork");
    let first_saved = leaf.find("saved").unwrap();
    let mut cut = Chain::new(Journal::parse(&bytes[..leaf.entries()[first_saved - 1].end as usize]).unwrap()).unwrap();
    cut.add_parent(journal("fork_parent")).unwrap();
    assert!(cut.seek(450).unwrap_err().to_string().contains("no saved point at or before minute 450"));
}

#[test]
fn a_fork_reads_its_parents_pieces_up_to_its_base_and_refuses_another_parent() {
    let chain = fork_chain();
    let parent = journal("fork_parent");
    let base = journal("fork").fork().unwrap().base.clone();
    assert_eq!(base.head, parent.entries()[base.line as usize].head);
    // The parent's pieces up to its base, then the fork's own.
    let levels: Vec<usize> = chain.pieces().iter().map(|at| at.level).collect();
    let parent_pieces = levels.iter().filter(|level| **level == 0).count();
    assert!(levels[..parent_pieces].iter().all(|level| *level == 0) && levels[parent_pieces..].iter().all(|level| *level == 1));
    assert_eq!(chain.pieces()[parent_pieces - 1].index, base.line as usize);
    // The fork's blocks follow the parent's, which it does not copy.
    let own_blocks: Vec<u64> = journal("fork")
        .entries()
        .iter()
        .filter_map(|entry| match &entry.line {
            Line::Piece(PieceLine { piece: Piece::LogBlock { index }, .. }) => Some(*index),
            _ => None,
        })
        .collect();
    assert_eq!(own_blocks[0], base.state.log.blocks);
    // Another Run, or the parent without its base line as it was, is refused.
    let mut wrong = Chain::new(journal("fork")).unwrap();
    let text = wrong.add_parent(journal("from_zero")).unwrap_err().to_string();
    assert_eq!(text, "the stored Run of branch main is not the one the fork was made of");
    let bytes = journal_bytes("fork_parent");
    let short = Journal::parse(&bytes[..parent.entries()[base.line as usize - 1].end as usize]).unwrap();
    let text = wrong.add_parent(short).unwrap_err().to_string();
    assert_eq!(text, "the stored Run of branch main no longer holds the line a fork of it goes on from");
    assert!(Chain::new(journal("from_zero")).unwrap().add_parent(journal("from_zero")).is_err());
}

#[test]
fn a_restore_reads_the_pieces_it_needs_in_order() {
    let chain = Chain::new(journal("from_zero")).unwrap();
    let kind = |position: usize| chain.piece(position).unwrap().0.piece.kind();
    let first = |kinds: &[&str]| (0..chain.pieces().len()).find(|position| kinds.contains(&kind(*position)));
    let point = |records, samples| Restore::new(&SavedPoint { minute: 360, records, samples, world: Vec::new() }.to_bytes()).unwrap();
    let both = point(5, 5);
    assert_eq!(chain.next_restore_read(0, &both).unwrap(), first(&["log_tables", "log_block", "sample_tables", "sample_block"]));
    assert_eq!(chain.next_restore_read(0, &point(0, 5)).unwrap(), first(&["sample_tables", "sample_block"]));
    assert_eq!(chain.next_restore_read(0, &point(5, 0)).unwrap(), first(&["log_tables", "log_block"]));
    assert_eq!(chain.next_restore_read(0, &point(0, 0)).unwrap(), None);
    // Past the last piece, none.
    assert_eq!(chain.next_restore_read(chain.pieces().len(), &both).unwrap(), None);
    // A piece that is not the bytes written, or not the log's or the samples', is an error.
    let mut restore = point(5, 5);
    let tables = first(&["log_tables"]).unwrap();
    assert_eq!(chain.restore_read(tables, b"other", &mut restore).unwrap_err().to_string(), "log_tables: the bytes are not the ones written");
    assert_eq!(chain.sample_pieces().unwrap().len(), (0..chain.pieces().len()).filter(|position| kind(*position).starts_with("sample_")).count());
}

#[test]
fn the_saved_points_are_due_every_save_every_minutes_and_at_the_end() {
    let every = Schedule { every: 360 };
    assert_eq!(every.step(0, 50, None), 50);
    assert_eq!(every.step(340, 50, None), 20);
    assert_eq!(every.step(360, 50, None), 50);
    assert_eq!(every.step(480, 60, Some(500)), 20);
    assert_eq!(every.step(500, 60, Some(500)), 0);
    assert_eq!(every.step(700, 600, None), 20);
    assert!(every.is_point(0) && every.is_point(360) && every.is_point(1440) && !every.is_point(361));
    assert!(every.due(1080, 1440) && !every.due(1440, 1440) && !every.due(1000, 1440));
    let none = Schedule { every: 0 };
    assert_eq!(none.step(340, 50, None), 50);
    assert!(!none.is_point(0) && !none.due(360, 1440));
}

/// deflate-raw of stored blocks of at most `size` bytes.
fn stored_blocks(raw: &[u8], size: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len() + 5);
    let chunks: Vec<&[u8]> = if raw.is_empty() { vec![raw] } else { raw.chunks(size).collect() };
    for (index, chunk) in chunks.iter().enumerate() {
        let length = chunk.len() as u16;
        out.push(u8::from(index + 1 == chunks.len()));
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&(!length).to_le_bytes());
        out.extend_from_slice(chunk);
    }
    out
}

/// What `node_stored_runs.mjs` compresses the fixtures' pieces with: deflate's stored blocks.
fn stored_deflate(raw: &[u8]) -> Vec<u8> {
    stored_blocks(raw, 65535)
}

/// Another compressor: stored blocks of at most 1000 bytes. Its bytes, and so where each piece is
/// and how long, differ from `stored_deflate`'s; what they hold does not.
fn small_blocks(raw: &[u8]) -> Vec<u8> {
    stored_blocks(raw, 1000)
}

/// A host in memory: data.pack, the journal, each piece's bytes before compression by its line,
/// and the compressor.
struct Files {
    data: Vec<u8>,
    journal: Vec<u8>,
    raw: Vec<Option<Vec<u8>>>,
    compress: fn(&[u8]) -> Vec<u8>,
}

impl Files {
    fn new(compress: fn(&[u8]) -> Vec<u8>) -> Self {
        Self { data: Vec::new(), journal: Vec::new(), raw: Vec::new(), compress }
    }

    /// Writes what `writer` queued, as the browser's `RunWriter.flush` does.
    fn flush(&mut self, writer: &mut RunWriter) {
        while let Some(next) = writer.next() {
            let encoded = match next {
                Next::Line => {
                    self.raw.push(None);
                    writer.commit_line().unwrap()
                }
                Next::Piece { raw, .. } => {
                    let raw = raw.to_vec();
                    let stored = (self.compress)(&raw);
                    let offset = self.data.len() as u64;
                    self.data.extend_from_slice(&stored);
                    self.raw.push(Some(raw));
                    writer.commit_piece(offset, stored.len() as u64).unwrap()
                }
                Next::Copy { .. } => panic!("no copies here"),
            };
            self.journal.extend_from_slice(&encoded.bytes);
        }
    }

    fn journal(&self) -> Journal {
        Journal::parse(&self.journal).unwrap()
    }
}

/// The bytes of the piece at `position` of `chain`, from the files of its level.
fn chain_raw<'a>(chain: &Chain, levels: &[&'a Files], position: usize) -> &'a [u8] {
    let at = chain.pieces()[position];
    levels[at.level].raw[at.index].as_deref().unwrap()
}

/// `runAndStore` of tick_core's `small_replay`, 50 ticks at a time, saved every `every` minutes.
fn run_and_store(every: u64, compress: fn(&[u8]) -> Vec<u8>) -> Files {
    let export = data("tick_core.json");
    let mut run = PlainRun::of_worlds_json(&export, "small_replay").unwrap();
    let schedule = Schedule { every };
    let duration = run.duration().unwrap();
    let mut files = Files::new(compress);
    let mut writer = RunWriter::new();
    writer.start(&run, export.clone(), Some("small_replay".to_owned()), &storing(every)).unwrap();
    files.flush(&mut writer);
    loop {
        let step = schedule.step(run.minute().unwrap(), 50, None);
        let ran = run.advance(step).unwrap();
        writer.take(&mut run, false).unwrap();
        files.flush(&mut writer);
        if ran < step {
            break;
        }
        if schedule.due(run.minute().unwrap(), duration) {
            writer.save(&mut run).unwrap();
            files.flush(&mut writer);
        }
    }
    writer.take(&mut run, true).unwrap();
    if every > 0 {
        writer.save(&mut run).unwrap();
    }
    writer.finish(&mut run).unwrap();
    files.flush(&mut writer);
    // The writer stands where a writer going on after the last piece would.
    let journal = files.journal();
    let last = journal.entries().len() - 1;
    assert_eq!(RunWriter::after(&journal, last).unwrap().state(), writer.state());
    assert_eq!(minutes(&journal, &journal.saved_points()), writer.saved().iter().map(|saved| saved.minute).collect::<Vec<_>>());
    files
}

/// Reads `chain` back from the files of its levels.
fn verified(chain: &Chain, levels: &[&Files]) -> Verified {
    let mut verify = Verify::new();
    for position in Verify::reads(chain).unwrap() {
        let (level, stored) = chain.stored(position).unwrap();
        chain.check_stored(position, stored.length).unwrap();
        assert!(levels[level].data.len() as u64 >= stored.offset + stored.length);
        verify.read(chain, position, chain_raw(chain, levels, position)).unwrap();
    }
    verify.result(chain).unwrap()
}

#[test]
fn the_writer_writes_the_stored_runs_of_the_browser_again() {
    for (name, every) in [("from_zero", 360), ("no_points", 0)] {
        let files = run_and_store(every, stored_deflate);
        assert!(files.journal == journal_bytes(name), "{name}: the journal is not the one the browser wrote");
        let wanted = summary(name);
        assert_eq!(files.data.len() as u64, wanted["data_bytes"].as_u64().unwrap(), "{name}");
        assert_eq!(sha256_hex(&files.data), wanted["data_sha256"].as_str().unwrap(), "{name}");

        // Read back, each piece's bytes give the digests of the "end" line.
        let journal = files.journal();
        let chain = Chain::new(journal.clone()).unwrap();
        let result = verified(&chain, &[&files]);
        assert!(result.complete && result.verified && !result.torn, "{name}: {:?}", result.mismatches);
        assert_eq!(result.saved_points, journal.saved_points().len() as u64);
        let end = journal.end().unwrap();
        assert_eq!((result.records, result.samples, &result.metrics), (end.records, end.samples, &end.metrics));
        assert_eq!((result.conversations, result.tape), (0, 0));
        assert_eq!(result.parts.as_ref().unwrap().len(), end.parts.len());

        // A piece that is not its bytes is an error.
        let world = (0..chain.pieces().len()).find(|position| chain.piece(*position).unwrap().0.piece.kind() == "world").unwrap();
        let mut changed = chain_raw(&chain, &[&files], world).to_vec();
        changed[0] ^= 1;
        assert_eq!(Verify::new().read(&chain, world, &changed).unwrap_err().to_string(), "world: the bytes are not the ones written");
        assert_eq!(chain.check_stored(world, 3).unwrap_err().to_string(), "world: data.pack ends before the piece");

        // An end line whose digests are not the pieces' is a mismatch, not an error.
        let mut bytes = files.journal[..journal.entries()[journal.entries().len() - 2].end as usize].to_vec();
        let mut end = end.clone();
        end.records += 1;
        end.parts[0].1 = "0".repeat(64);
        bytes.extend_from_slice(&Line::End(end.clone()).encode().bytes);
        let changed = Chain::new(Journal::parse(&bytes).unwrap()).unwrap();
        let result = verified(&changed, &[&files]);
        assert!(!result.verified, "{name}");
        assert_eq!(result.mismatches, [end.parts[0].0.clone(), "records".to_owned()], "{name}");
    }
}

#[test]
fn the_heads_are_the_same_whatever_compressor_wrote_the_pieces() {
    let (one, other) = (run_and_store(360, stored_deflate), run_and_store(360, small_blocks));
    assert_ne!(one.data, other.data);
    assert_ne!(one.journal, other.journal);
    let (one, other) = (one.journal(), other.journal());
    let heads = |journal: &Journal| journal.heads().iter().map(|head| journal.entries()[head.index].head.clone()).collect::<Vec<_>>();
    assert_eq!(heads(&one), heads(&other));
    // The lines differ where the pieces are, and hold the same otherwise.
    for (a, b) in one.entries().iter().zip(other.entries()) {
        assert_eq!(a.head, b.head);
        assert_eq!(a.sha256 == b.sha256, !matches!(a.line, Line::Piece(_)), "{}", a.json);
    }
}

#[test]
fn a_fork_writes_its_own_pieces_after_its_parents_base_and_reads_back_through_it() {
    let parent = run_and_store(360, stored_deflate);
    let parent_journal = parent.journal();
    let chain = Chain::new(parent_journal.clone()).unwrap();
    assert!(chain.complete());
    // The parent at minute 400: its saved point at 360 restored, 40 ticks computed.
    let seek = chain.seek(400).unwrap();
    assert_eq!((seek.from_minute, seek.base), (360, seek.from.unwrap()));
    let mut restore = Restore::new(parent.raw[seek.base].as_deref().unwrap()).unwrap();
    let mut position = chain.next_restore_read(0, &restore).unwrap();
    while let Some(at) = position {
        chain.restore_read(at, chain_raw(&chain, &[&parent], at), &mut restore).unwrap();
        position = chain.next_restore_read(at + 1, &restore).unwrap();
    }
    let entry = PlainRun::entry_of_worlds_json(&data("tick_core.json"), "small_replay").unwrap();
    let mut run = PlainRun::restore(entry, restore).unwrap();
    run.advance(40).unwrap();
    // A fork without a program: the Run goes on as the parent's did.
    let fork = Fork {
        parent_branch_id: "main".to_owned(),
        minute: 400,
        origin: ForkOrigin { branch_id: "main".to_owned(), minute: 400, state_hash: run.state_digest().unwrap(), checkpoint_hash: None },
        program: "{}".to_owned(),
        approved_program_hash: "x".to_owned(),
        receipt: "{}".to_owned(),
    };
    let mut files = Files::new(stored_deflate);
    let mut writer = RunWriter::new();
    assert!(writer.fork_start(&parent_journal, parent_journal.find("log_block").unwrap(), fork.clone()).is_err());
    writer.fork_start(&parent_journal, seek.base, fork).unwrap();
    writer.save(&mut run).unwrap();
    files.flush(&mut writer);
    let schedule = Schedule { every: 360 };
    loop {
        let step = schedule.step(run.minute().unwrap(), 60, None);
        let ran = run.advance(step).unwrap();
        writer.take(&mut run, false).unwrap();
        if ran < step {
            break;
        }
        if schedule.due(run.minute().unwrap(), run.duration().unwrap()) {
            writer.save(&mut run).unwrap();
        }
        files.flush(&mut writer);
    }
    writer.save(&mut run).unwrap();
    writer.finish(&mut run).unwrap();
    files.flush(&mut writer);
    let child = files.journal();
    // Its own pieces: no exported world, no block of its parent's.
    assert!(child.find("export").is_none());
    let base = child.fork().unwrap().base.clone();
    assert_eq!(child.state_at(1).unwrap(), base.state);
    assert!(files.data.len() < parent.data.len(), "{} of {}", files.data.len(), parent.data.len());
    // Read back through the parent, to the parent's own digests at the end.
    let mut chain = Chain::new(child.clone()).unwrap();
    chain.add_parent(parent_journal.clone()).unwrap();
    let result = verified(&chain, &[&parent, &files]);
    assert!(result.complete && result.verified, "{:?}", result.mismatches);
    let (child_end, parent_end) = (child.end().unwrap(), parent_journal.end().unwrap());
    assert_eq!((&child_end.parts, &child_end.metrics, child_end.records), (&parent_end.parts, &parent_end.metrics, parent_end.records));
    // The parent cannot be cut back before the fork's base.
    let saved = parent_journal.saved_points();
    assert!(parent_journal.cut(&parent_journal.entries()[saved[0]].head, Some(base.line as usize)).is_ok());
    assert!(parent_journal.cut(&parent_journal.entries()[1].head, Some(base.line as usize)).is_err());
}

#[test]
fn records_are_stored_as_texts_and_read_back_byte_for_byte() {
    let texts = vec![
        br#"{"question":"q1","turn_id":"t-1"}"#.to_vec(),
        br#"{"a":1.50,"b":"\u0041\/"}"#.to_vec(),
        b"[]".to_vec(),
        "{\"q\":\"\u{3042}\"}".as_bytes().to_vec(),
    ];
    let bytes = records::texts(&texts);
    assert_eq!(records::read(&bytes).unwrap(), texts);
    assert_eq!(records::count(&bytes).unwrap(), texts.len() as u64);
    // Bytes that are not a records piece are an error, never a panic: cut short, with bytes after
    // the records, of another encoding (1 was the tokens of the decision records of format v5), a
    // record that is not UTF-8.
    for end in [0, 1, 2, bytes.len() / 2, bytes.len() - 1] {
        assert!(records::read(&bytes[..end]).is_err(), "cut at {end}");
    }
    assert!(records::read(&[bytes.clone(), vec![0]].concat()).is_err());
    assert!(records::read(&[1, 0]).is_err());
    assert!(records::read(&[7, 0]).is_err());
    assert!(records::read(&[0, 1, 1, 0xff]).is_err());
}

#[test]
fn turns_and_tape_entries_are_counted_and_turns_come_after_the_end() {
    let mut files = run_and_store(360, stored_deflate);
    let journal = files.journal();
    // A Run not stored to its end keeps no turn.
    let saved = journal.saved_points()[0];
    let partial = Journal::parse(&files.journal[..journal.entries()[saved].end as usize]).unwrap();
    assert_eq!(RunWriter::after_end(&partial).unwrap_err().to_string(), "a conversation turn is kept by a Run stored to its end");
    let mut writer = RunWriter::after_end(&journal).unwrap();
    let turns = vec![br#"{"question":"q1","turn_id":"t-1"}"#.to_vec(), br#"{"question":"q2","turn_id":"t-2"}"#.to_vec()];
    writer.turns(&turns).unwrap();
    assert!(writer.turns(&[b"not json".to_vec()]).is_err());
    files.flush(&mut writer);
    let mut again = RunWriter::after_end(&files.journal()).unwrap();
    again.turns(&turns[..1]).unwrap();
    files.flush(&mut again);
    let journal = files.journal();
    let chain = Chain::new(journal.clone()).unwrap();
    let read: Vec<Vec<u8>> = chain
        .records(RecordKind::Conversations)
        .unwrap()
        .into_iter()
        .flat_map(|position| records::read(chain_raw(&chain, &[&files], position)).unwrap())
        .collect();
    assert_eq!(read, [turns.clone(), turns[..1].to_vec()].concat());
    assert_eq!(journal.state_at(journal.entries().len() - 1).unwrap().counts.conversations, 3);
    // The head stays the end line; the Run still reads back to it.
    assert_eq!(journal.heads().last().unwrap().kind, HeadKind::End);
    let result = verified(&chain, &[&files]);
    assert!(result.verified && result.conversations == 3);
    // A tape piece counts its entries; the saved point after it holds the count.
    let mut writer = RunWriter::new();
    let export = data("tick_core.json");
    let mut run = PlainRun::of_worlds_json(&export, "small_replay").unwrap();
    let mut tape = Files::new(stored_deflate);
    writer.start(&run, export.clone(), Some("small_replay".to_owned()), &storing(360)).unwrap();
    writer.tape(&[br#"{"response":"a"}"#.to_vec()]).unwrap();
    run.advance(360).unwrap();
    writer.save(&mut run).unwrap();
    tape.flush(&mut writer);
    let journal = tape.journal();
    assert_eq!(journal.saved(journal.saved_points()[0]).unwrap().counts.tape, 1);
    assert!(journal.find("llm_tape").is_some());
}

#[test]
fn the_check_of_private_data_refuses_what_pythons_checkpoint_check_refuses() {
    // tests/unit/storage/test_projects.py `_PRIVATE_DOCUMENTS` and `_CLEAN_DOCUMENTS`, as canonical
    // JSON, and keys `_PRIVATE` finds with `re.IGNORECASE`'s letters.
    let private = [
        r#"{"api_key":"x"}"#,
        r#"{"nested":[{"Full Name":1}]}"#,
        r#"["call 090-1234-5678"]"#,
        r#"["+81 90-1234-5678"]"#,
        r#"["０９０-１２３４-５６７８"]"#,
        r#"{"note":"write to person@example.org today"}"#,
        r#"["token sk-abcdefghijkl0123"]"#,
        r#"["token sk-ant-abcdefgh"]"#,
        r#"["Token SK-ABCDEFGHIJKL"]"#,
        r#"["token ſk-abcdefghijkl"]"#,
        r#"["token sK-abcdefghijkl"]"#,
        r#"["BEARER abc"]"#,
        r#"["line\nBearer\tabc"]"#,
        r#"["quoted \"person@example.org\""]"#,
        r#"["あ 090-1234-5678"]"#,
        r#"{"APİ_KEY":1}"#,
        r#"{"apı_key":1}"#,
        r#"{"ſecret":1}"#,
    ];
    let clean = [
        r#"{"id":"task-abcdefghijk","task-1":"task-1","text":"あいう"}"#,
        r#"[1.5,-2,null,true,"2026-10-31","a \"quoted\" \\ \n string"]"#,
        r#"["bearing","1-2","١","minute 12-34"]"#,
        r#"{"a":{"b":[{"c":"no-digits - here"}]}}"#,
        "[]",
        "{}",
        r#"{"a\nddress":1}"#,
    ];
    for document in private {
        assert!(sensitive::check(&[document.as_bytes()]).is_err(), "{document}");
    }
    for document in clean {
        sensitive::check(&[document.as_bytes()]).unwrap_or_else(|error| panic!("{document}: {error}"));
    }
    assert_eq!(sensitive::check(&[br#"{"phone":1}"#]).unwrap_err().to_string(), sensitive::SENSITIVE_FIELDS);
    assert_eq!(sensitive::check(&[br#"["090-1234-5678"]"#]).unwrap_err().to_string(), sensitive::SENSITIVE_DATA);
    // A string checked once passes again without being read; a new one is read.
    let mut guard = sensitive::Guard::new();
    guard.check(&[br#"["a@b"]"#]).unwrap();
    guard.check(&[br#"["a@b"]"#]).unwrap();
    assert!(guard.check(&[br#"["a@b.example"]"#]).is_err());

    // The writer refuses an exported world that holds private data, and queues nothing of it.
    let mut entry = PlainRun::entry_of_worlds_json(&data("tick_core.json"), "small_replay").unwrap();
    let run = PlainRun::of_entry(&entry).unwrap();
    entry.as_object_mut().unwrap().insert("leak".to_owned(), "call 090-1234-5678".into());
    let mut writer = RunWriter::new();
    let refused = writer.start(&run, serde_json::to_vec(&entry).unwrap(), None, &storing(360)).unwrap_err().to_string();
    assert_eq!(refused, sensitive::SENSITIVE_DATA);
    assert!(writer.next().is_none());
}

#[test]
fn a_run_of_other_model_versions_is_view_only_with_pythons_rule_and_text() {
    let installed: Versions = [("engine:model", "3"), ("engine:rng_algorithm", "sha256-keyed-v1"), ("policy:behavior", "b@1"), ("state:x", "x@1")]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect();
    let status = ModelStatus::of(&installed, &installed);
    assert!(status.current() && status.require_current().is_ok());
    // Without the engine's items, a save was computed by engine 1.
    let unrecorded: Versions = installed.iter().filter(|(key, _)| !key.starts_with("engine:")).map(|(key, value)| (key.clone(), value.clone())).collect();
    let status = ModelStatus::of(&unrecorded, &installed);
    assert_eq!((status.recorded_version.as_str(), status.installed_version.as_str()), ("1", "3"));
    assert_eq!(status.differences, ["engine:model"]);
    assert!(status.message().starts_with("このシナリオは旧モデル（版 1）で計算されました。今のモデルは版 3 です。閲覧のみです。"), "{}", status.message());
    let error = status.require_current().unwrap_err().to_string();
    assert!(error.starts_with("run_model_view_only: このシナリオは旧モデル（版 1）"), "{error}");
    // A later version, and other policy signatures.
    let mut later = installed.clone();
    later.insert("engine:model".to_owned(), "4".to_owned());
    assert!(ModelStatus::of(&later, &installed).message().starts_with("このシナリオは別の版のモデル（版 4）で計算されました。"));
    let mut policies = installed.clone();
    for key in ["policy:behavior", "policy:queue", "model_suite", "state:x"] {
        policies.insert(key.to_owned(), "other".to_owned());
    }
    let status = ModelStatus::of(&policies, &installed);
    assert_eq!(status.differences, ["model_suite", "policy:behavior", "policy:queue", "state:x"]);
    assert!(status.message().starts_with("このシナリオは今と異なるモデルの版（model_suite、policy:behavior、policy:queue ほか）"), "{}", status.message());
    // A view (D2): `model_view` for a Run of other versions, `saved_point_rounding` where the minute
    // shown is not the one asked for.
    let old = ModelStatus::of(&unrecorded, &installed);
    let described: Value = serde_json::from_slice(&old.describe_json(30, 45)).unwrap();
    assert_eq!(
        described,
        serde_json::json!({
            "model_view": {
                "current": false,
                "view_only": true,
                "recorded_version": "1",
                "installed_version": "3",
                "differences": ["engine:model"],
                "message": old.message(),
            },
            "saved_point_rounding": {
                "requested_minute": 45,
                "saved_minute": 30,
                "message": "旧モデルのRunのため、45分は計算し直さず、保存した時点（30分）の状態を表示しています。",
            },
        })
    );
    // The bytes the frame gets, the whole message and the order of the keys with them (#489).
    assert_eq!(sha256_hex(&old.describe_json(30, 45)), "74cb82e11e81a523ac1a3626757f53c75f44f0f27d6caf50d18d58c2fbb89ec7");
    let at_save: Value = serde_json::from_slice(&old.describe_json(30, 30)).unwrap();
    assert!(at_save.get("saved_point_rounding").is_none() && at_save["model_view"]["view_only"] == true);
    assert_eq!(ModelStatus::of(&installed, &installed).describe_json(30, 30), b"{}");
    let current: Value = serde_json::from_slice(&ModelStatus::of(&installed, &installed).to_json()).unwrap();
    assert_eq!(current["message"], Value::Null);
    // Configs the installed code cannot sign.
    let status = ModelStatus::unreadable(&installed);
    assert_eq!(status.differences, ["model_signatures"]);
    assert!(!status.current());
}

/// The RNG's algorithm is the one the Scenario authoring of Python draws with (`scenario_authoring/rng.py`).
#[test]
fn the_rng_algorithm_is_the_scenario_authoring_s() {
    let python = fs::read_to_string(format!("{}/../../src/sheltersim/scenario_authoring/rng.py", env!("CARGO_MANIFEST_DIR"))).unwrap();
    assert_eq!(model_version::RNG_ALGORITHM, "sha256-keyed-v1");
    assert!(python.contains("\"sha256-keyed-v1\""), "scenario_authoring/rng.py's algorithm is not the core's");
}

#[test]
fn the_writer_commits_only_what_it_queued() {
    let mut writer = RunWriter::new();
    assert!(writer.next().is_none());
    assert!(writer.commit_line().is_err() && writer.commit_piece(0, 1).is_err());
    let run = RunLine::new(b"{}", None, 60, &storing(360));
    writer.run_line(run.clone());
    assert_eq!(writer.next(), Some(Next::Line));
    assert!(writer.commit_piece(0, 1).is_err());
    let encoded = writer.commit_line().unwrap();
    assert_eq!(encoded.bytes, Line::Run(run.clone()).encode().bytes);
    assert_eq!(encoded.head, sha256_hex(&Line::Run(run).to_json()));
    assert!(writer.next().is_none());

    // A copy of another Run's piece keeps its bytes' length and sha256, at its new place.
    let source = journal("from_zero");
    let export = source.find("export").unwrap();
    let mut copier = RunWriter::new();
    copier.same_start(&source).unwrap();
    copier.commit_line().unwrap();
    let length = source.piece(export).unwrap().stored.length;
    assert_eq!(copier.next(), Some(Next::Copy { kind: "export", length }));
    assert!(copier.commit_piece(0, length + 1).is_err());
    let line = copier.commit_piece(0, length).unwrap();
    let copied = Line::of_json(&line.bytes[..line.bytes.len() - 66]).unwrap();
    assert_eq!(copied, source.entries()[export].line);
    assert_eq!(line.head, source.entries()[export].head);
}
