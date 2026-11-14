//! The format of a stored Run (`run_store`, #368): the journals the browser's writer wrote before
//! the format moved into the core (`tests/data/stored_runs/`, written by
//! `native/wasm/tests/node_stored_runs.mjs`, which also checks that the browser still writes them)
//! read back, each line encoded again to its bytes; formats v1 to v4; lines cut short, not
//! matching their sha256, or not lines of the format; the heads, the cuts, the seeks and the reads
//! a restore needs; the saved points' schedule; and the writer, which writes the same journals and
//! data.pack again from the same Runs (with the stand-in compression of the fixtures: deflate's
//! stored blocks), and whose pieces read back to the Run's digests.

use std::fs;

use serde_json::Value;
use sheltersim_core::archive::{Restore, SavedPoint};
use sheltersim_core::checkpoint::sha256_hex;
use sheltersim_core::plain::PlainRun;
use sheltersim_core::run_store::{
    ForkLine, ForkOrigin, Format, HeadKind, Journal, Line, Next, Piece, PieceLine, RunLine, RunWriter, Schedule, Stored, Verify, SAVE_EVERY,
};

const NAMES: [&str; 7] = ["from_zero", "no_points", "v1", "v2", "v3", "computed", "fork"];

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
    let program = "{\"text\":\"トイレを1基追加\"}\t\u{1}\u{1f}\u{7f}\u{2028}\"\\\u{8}\u{c}\n\r";
    let fork = Line::Fork(ForkLine {
        parent_branch_id: "main".to_owned(),
        minute: 400,
        origin: ForkOrigin { branch_id: "main".to_owned(), minute: 400, state_hash: "ab".to_owned(), checkpoint_hash: None },
        program: program.to_owned(),
        approved_program_hash: "cd".to_owned(),
        receipt: "{}".to_owned(),
    });
    let wanted = concat!(
        r#"{"kind":"fork","parent_branch_id":"main","minute":400,"fork_origin":{"branch_id":"main","minute":400,"state_hash":"ab","checkpoint_hash":null},"#,
        r#""program":"{\"text\":\"トイレを1基追加\"}\t\u0001\u001f"#,
        "\u{7f}\u{2028}",
        r#"\"\\\b\f\n\r","approved_program_hash":"cd","receipt":"{}"}"#,
    );
    assert_eq!(String::from_utf8(fork.to_json()).unwrap(), wanted);
    assert_eq!(Line::of_json(wanted.as_bytes()).unwrap(), fork);
    let encoded = fork.encode();
    assert_eq!(encoded.bytes, line(wanted));
    assert_eq!(encoded.sha256, sha256_hex(wanted.as_bytes()));
}

#[test]
fn each_stored_run_is_of_its_format() {
    for (name, format, save_every, current) in [
        ("v1", Format::V1, None, false),
        ("v2", Format::V2, Some(360), false),
        ("v3", Format::V3, Some(360), true),
        ("from_zero", Format::V4, Some(360), true),
        ("no_points", Format::V4, Some(0), true),
        ("computed", Format::V4, Some(360), true),
        ("fork", Format::V4, Some(360), true),
    ] {
        let journal = journal(name);
        let run = journal.run().unwrap();
        assert_eq!((run.format, run.save_every, run.format.is_current()), (format, save_every, current), "{name}");
        assert_eq!(run.save_every_or_default(), SAVE_EVERY, "{name}");
        assert_eq!(Format::of_name(format.name()).unwrap(), format);
        assert_eq!(journal.end().unwrap().minute, run.duration_minutes, "{name}");
        assert_eq!(journal.fork().is_some(), name == "fork", "{name}");
    }
    let fork = journal("fork");
    let run = fork.run().unwrap();
    assert_eq!((run.fork_minute(), run.fork.as_ref().unwrap().parent_branch_id.as_str()), (400, "main"));
    let line = fork.fork().unwrap();
    assert_eq!((line.minute, line.origin.branch_id.as_str(), line.origin.minute), (400, "main", 400));
    assert!(journal("no_points").saved_points().is_empty());
    assert!(journal("v1").saved_points().is_empty());
    let text = Format::of_name("sheltersim-browser-run-v9").unwrap_err().to_string();
    assert!(text.contains("a stored Run of format sheltersim-browser-run-v9: this code reads sheltersim-browser-run-v1, "), "{text}");
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

#[test]
fn a_whole_line_that_is_not_a_line_of_the_format_is_an_error() {
    let bytes = journal_bytes("from_zero");
    let read = journal("from_zero");
    let first = &bytes[..read.entries()[0].end as usize];
    let after_first = |json: &str| [first, &line(json)].concat();
    let fork_line = r#"{"kind":"fork","parent_branch_id":"main","minute":1,"fork_origin":{"branch_id":"main","minute":1,"state_hash":"x","checkpoint_hash":null},"program":"{}","approved_program_hash":"x","receipt":"{}"}"#;
    let cases: Vec<(Vec<u8>, &str)> = vec![
        (after_first("not json"), "a line that is not JSON"),
        (after_first("[1]"), "a line that is not an object"),
        (after_first(r#"{"kind":"nonsense"}"#), "a line of an unknown kind: nonsense"),
        (after_first(r#"{"offset":0}"#), "a line without kind"),
        (after_first(r#"{"kind":"export","offset":0,"length":1,"raw_length":1}"#), "a line without sha256"),
        (after_first(r#"{"kind":"export","offset":"0","length":1,"raw_length":1,"sha256":"x"}"#), "offset of a line is not a count"),
        (after_first(r#"{"kind":"export","offset":1.5,"length":1,"raw_length":1,"sha256":"x"}"#), "offset of a line is not a count"),
        (after_first(r#"{"kind":"export","offset":-1,"length":1,"raw_length":1,"sha256":"x"}"#), "offset of a line is not a count"),
        (after_first(r#"{"kind":"log_block","offset":0,"length":1,"raw_length":1,"sha256":"x"}"#), "a line without index"),
        (after_first(r#"{"kind":"end","minute":1,"records":0,"samples":0,"parts":[],"metrics":"x"}"#), "the end line's parts are not an object"),
        (after_first(r#"{"kind":"end","minute":1,"records":0,"samples":0,"parts":{"a":1},"metrics":"x"}"#), "a part's digest that is not text"),
        (after_first(&read.entries()[0].json), "a second run line"),
        (after_first(fork_line), "a fork line in a Run that is not a fork's"),
        (line(&read.entries()[1].json), "its first line is not the run line"),
        ([&bytes[..], &line(&read.entries()[1].json)].concat(), "a line after the end line"),
        (line(&read.entries()[0].json.replace("-v4", "-v9")), "a stored Run of format sheltersim-browser-run-v9"),
        (
            [
                &journal_bytes("v1")[..journal("v1").entries()[1].end as usize],
                &line(&read.entries().iter().find(|entry| entry.line.kind() == "saved").unwrap().json),
            ]
            .concat(),
            "a saved point in a Run of format v1",
        ),
    ];
    for (bytes, wanted) in cases {
        let text = error_text(&bytes);
        assert!(text.contains(wanted), "{wanted}: {text}");
    }
    // A second fork line in a fork's Run.
    let fork = journal_bytes("fork");
    let forked = journal("fork");
    let text = error_text(&[&fork[..forked.entries()[1].end as usize], &line(&forked.entries()[1].json)].concat());
    assert!(text.contains("a second fork line"), "{text}");
}

#[test]
fn the_heads_are_the_lines_a_branch_goes_on_from() {
    let from_zero = journal("from_zero");
    let heads = from_zero.heads();
    assert_eq!((heads[0].kind, heads[0].minute, heads[0].index), (HeadKind::Export, 0, 1));
    let last = heads.last().unwrap();
    assert_eq!((last.kind, last.minute, last.index), (HeadKind::End, 1440, from_zero.entries().len() - 1));
    // Each saved head's blocks are stored before it; the end's saved point is one.
    for head in heads.iter().filter(|head| head.kind == HeadKind::Saved) {
        let saved = from_zero.saved(head.index).unwrap();
        let before = &from_zero.entries()[..head.index];
        let count = |kind: &str| before.iter().filter(|entry| entry.line.kind() == kind).count() as u64;
        assert!(saved.log_blocks <= count("log_block") && saved.sample_blocks <= count("sample_block"));
    }
    assert!(heads.iter().any(|head| head.kind == HeadKind::Saved && head.minute == 1440));
    // Computed in chunks, every saved point is sealed: each is a head.
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
    // A fork's Run starts at its saved point at the fork: its exported world is the parent's.
    let fork = journal("fork");
    let kinds: Vec<(HeadKind, i64)> = fork.heads().iter().map(|head| (head.kind, head.minute)).collect();
    assert_eq!(kinds, [(HeadKind::Saved, 400), (HeadKind::Saved, 480), (HeadKind::End, 480)]);
    for head in computed.heads() {
        assert_eq!(computed.head(&computed.entries()[head.index].sha256), Some(head));
    }
    assert_eq!(computed.head(&"0".repeat(64)), None);
}

#[test]
fn a_cut_keeps_the_lines_up_to_the_head_and_the_bytes_they_point_at() {
    let bytes = journal_bytes("computed");
    let computed = journal("computed");
    for head in computed.heads() {
        let cut = computed.cut(&computed.entries()[head.index].sha256).unwrap();
        assert_eq!(cut.head, head);
        assert_eq!(cut.journal_end, computed.entries()[head.index].end);
        let kept = Journal::parse(&bytes[..cut.journal_end as usize]).unwrap();
        assert_eq!(kept.entries(), &computed.entries()[..=head.index]);
        let pieces = kept.entries().iter().filter_map(|entry| match &entry.line {
            Line::Piece(line) => Some(line.stored.offset + line.stored.length),
            _ => None,
        });
        assert_eq!(cut.data_end, pieces.max().unwrap());
    }
    let last = computed.entries().len() - 1;
    assert_eq!(computed.cut(&computed.entries()[last].sha256).unwrap().data_end, summary("computed")["data_bytes"].as_u64().unwrap());
    // A line that is no head (a block's) is no place to cut back to.
    let block = computed.find("log_block").unwrap();
    let text = computed.cut(&computed.entries()[block].sha256).unwrap_err().to_string();
    assert_eq!(text, "the branch's head is not a line its stored Run can go on from");
}

#[test]
fn a_seek_starts_at_the_nearest_saved_point_at_or_before_the_minute() {
    let from_zero = journal("from_zero");
    let points = from_zero.saved_points();
    assert_eq!(minutes(&from_zero, &points), [360, 720, 1080, 1440]);
    for (minute, from) in [(0, None), (1, None), (359, None), (360, Some(0)), (361, Some(0)), (1079, Some(1)), (1439, Some(2)), (1440, Some(3))] {
        let seek = from_zero.seek(minute).unwrap();
        assert_eq!(seek.from, from.map(|at: usize| points[at]), "minute {minute}");
        assert_eq!(seek.from_minute, from.map_or(0, |at| [360, 720, 1080, 1440][at]), "minute {minute}");
        assert_eq!((seek.minute, seek.export), (minute, 1));
    }
    for minute in [-1, 1441] {
        assert_eq!(from_zero.seek(minute).unwrap_err().to_string(), format!("minute {minute} is not one of the Run (0 to 1440)"));
    }
    // v1 has no saved points: from minute 0.
    assert_eq!(journal("v1").seek(700).unwrap().from, None);
    // A fork's Run: no minute before the fork's, and from its saved points.
    let fork = journal("fork");
    assert_eq!(fork.seek(399).unwrap_err().to_string(), "minute 399 is not one of the Run (400 to 480)");
    assert_eq!(fork.seek(400).unwrap().from_minute, 400);
    assert_eq!(fork.seek(479).unwrap().from_minute, 400);
    assert_eq!(fork.seek(480).unwrap().from_minute, 480);
    // Without its saved points, a fork's Run has nowhere to start from.
    let bytes = journal_bytes("fork");
    let first_saved = fork.find("saved").unwrap();
    let cut = Journal::parse(&bytes[..fork.entries()[first_saved - 1].end as usize]).unwrap();
    assert!(cut.seek(450).unwrap_err().to_string().contains("no saved point at or before minute 450"));
    // A Run without its exported world.
    let run_only = Journal::parse(&bytes[..fork.entries()[1].end as usize]).unwrap();
    assert_eq!(run_only.seek(450).unwrap_err().to_string(), "the stored Run has no exported world");
}

#[test]
fn a_restore_reads_the_pieces_it_needs_in_order() {
    let from_zero = journal("from_zero");
    let first = |kinds: &[&str]| from_zero.entries().iter().position(|entry| kinds.contains(&entry.line.kind()));
    let point = |records, samples| Restore::new(&SavedPoint { minute: 360, records, samples, world: Vec::new() }.to_bytes()).unwrap();
    let both = point(5, 5);
    assert_eq!(from_zero.next_restore_read(0, &both), first(&["log_tables", "log_block", "sample_tables", "sample_block"]));
    assert_eq!(from_zero.next_restore_read(0, &point(0, 5)), first(&["sample_tables", "sample_block"]));
    assert_eq!(from_zero.next_restore_read(0, &point(5, 0)), first(&["log_tables", "log_block"]));
    assert_eq!(from_zero.next_restore_read(0, &point(0, 0)), None);
    // Past the last piece, none.
    assert_eq!(from_zero.next_restore_read(from_zero.entries().len(), &both), None);
    // A piece that is not the bytes written, or not the log's or the samples', is an error.
    let mut restore = point(5, 5);
    let tables = first(&["log_tables"]).unwrap();
    assert_eq!(from_zero.restore_read(tables, b"other", &mut restore).unwrap_err().to_string(), "log_tables: the bytes are not the ones written");
    assert_eq!(from_zero.sample_pieces().len(), from_zero.entries().iter().filter(|entry| entry.line.kind().starts_with("sample_")).count());
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

/// deflate-raw of stored blocks: what `node_stored_runs.mjs` compresses the fixtures' pieces with.
fn stored_deflate(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len() + 5);
    let chunks: Vec<&[u8]> = if raw.is_empty() { vec![raw] } else { raw.chunks(65535).collect() };
    for (index, chunk) in chunks.iter().enumerate() {
        let length = chunk.len() as u16;
        out.push(u8::from(index + 1 == chunks.len()));
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&(!length).to_le_bytes());
        out.extend_from_slice(chunk);
    }
    out
}

/// A host in memory: data.pack, the journal, and each piece's bytes before compression.
#[derive(Default)]
struct Files {
    data: Vec<u8>,
    journal: Vec<u8>,
    raw: Vec<Option<Vec<u8>>>,
}

impl Files {
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
                    let stored = stored_deflate(&raw);
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
}

/// `runAndStore` of tick_core's `small_replay`, 50 ticks at a time, saved every `every` minutes.
fn run_and_store(every: u64) -> Files {
    let export = data("tick_core.json");
    let mut run = PlainRun::of_worlds_json(&export, "small_replay").unwrap();
    let schedule = Schedule { every };
    let duration = run.duration().unwrap();
    let mut files = Files::default();
    let mut writer = RunWriter::new();
    writer.start(&run, export.clone(), Some("small_replay".to_owned()), every).unwrap();
    files.flush(&mut writer);
    loop {
        let step = schedule.step(run.minute().unwrap(), 50, None);
        let ran = run.advance(step).unwrap();
        writer.take(&run, false).unwrap();
        files.flush(&mut writer);
        if ran < step {
            break;
        }
        if schedule.due(run.minute().unwrap(), duration) {
            writer.save(&mut run, false).unwrap();
            files.flush(&mut writer);
        }
    }
    writer.take(&run, true).unwrap();
    if every > 0 {
        writer.save(&mut run, false).unwrap();
    }
    writer.finish(&run).unwrap();
    files.flush(&mut writer);
    // The writer stands where a writer going on after the last piece would.
    let journal = Journal::parse(&files.journal).unwrap();
    let last = journal.entries().len() - 1;
    assert_eq!(RunWriter::after(&journal, last).unwrap().places(), writer.places());
    assert_eq!(minutes(&journal, &journal.saved_points()), writer.saved().iter().map(|saved| saved.minute).collect::<Vec<_>>());
    files
}

#[test]
fn the_writer_writes_the_stored_runs_of_the_browser_again() {
    for (name, every) in [("from_zero", 360), ("no_points", 0)] {
        let files = run_and_store(every);
        assert!(files.journal == journal_bytes(name), "{name}: the journal is not the one the browser wrote");
        let wanted = summary(name);
        assert_eq!(files.data.len() as u64, wanted["data_bytes"].as_u64().unwrap(), "{name}");
        assert_eq!(sha256_hex(&files.data), wanted["data_sha256"].as_str().unwrap(), "{name}");

        // Read back, each piece's bytes give the digests of the "end" line.
        let journal = Journal::parse(&files.journal).unwrap();
        let mut verify = Verify::new();
        for index in Verify::reads(&journal) {
            let raw = files.raw[index].as_ref().unwrap();
            journal.check_stored(index, journal.piece(index).unwrap().stored.length).unwrap();
            verify.read(&journal, index, raw).unwrap();
        }
        let result = verify.result(&journal).unwrap();
        assert!(result.complete && result.verified && !result.torn, "{name}: {:?}", result.mismatches);
        assert_eq!(result.saved_points, journal.saved_points().len() as u64);
        let end = journal.end().unwrap();
        assert_eq!((result.records, result.samples, &result.metrics), (end.records, end.samples, &end.metrics));
        assert_eq!(result.parts.as_ref().unwrap().len(), end.parts.len());

        // A piece that is not its bytes, and a saved point not of its line, are errors.
        let world = journal.find("world").unwrap();
        let mut changed = files.raw[world].clone().unwrap();
        changed[0] ^= 1;
        assert_eq!(Verify::new().read(&journal, world, &changed).unwrap_err().to_string(), "world: the bytes are not the ones written");
        assert_eq!(journal.check_stored(world, 3).unwrap_err().to_string(), "world: data.pack ends before the piece");
        if every > 0 {
            // A "saved" line that names the bytes of another saved point.
            let [first, second] = [journal.saved_points()[0], journal.saved_points()[1]];
            let mut lines: Vec<Line> = journal.entries().iter().map(|entry| entry.line.clone()).collect();
            let Line::Piece(PieceLine { stored: other, .. }) = lines[second].clone() else { panic!("not a piece") };
            let Line::Piece(PieceLine { stored, .. }) = &mut lines[first] else { panic!("not a piece") };
            *stored = other;
            let bytes: Vec<u8> = lines.iter().flat_map(|line| line.encode().bytes).collect();
            let lying = Journal::parse(&bytes).unwrap();
            let text = Verify::new().read(&lying, first, files.raw[second].as_ref().unwrap()).unwrap_err().to_string();
            assert_eq!(text, "saved: the point is not the one its line says (minute 360)");
        }

        // An end line whose digests are not the pieces' is a mismatch, not an error.
        let mut bytes = files.journal[..journal.entries()[journal.entries().len() - 2].end as usize].to_vec();
        let mut end = end.clone();
        end.records += 1;
        end.parts[0].1 = "0".repeat(64);
        bytes.extend_from_slice(&Line::End(end.clone()).encode().bytes);
        let changed = Journal::parse(&bytes).unwrap();
        let mut verify = Verify::new();
        for index in Verify::reads(&changed) {
            verify.read(&changed, index, files.raw[index].as_ref().unwrap()).unwrap();
        }
        let result = verify.result(&changed).unwrap();
        assert!(!result.verified, "{name}");
        assert_eq!(result.mismatches, [end.parts[0].0.clone(), "records".to_owned()], "{name}");
    }
}

#[test]
fn the_writer_commits_only_what_it_queued() {
    let mut writer = RunWriter::new();
    assert!(writer.next().is_none());
    assert!(writer.commit_line().is_err() && writer.commit_piece(0, 1).is_err());
    let run = RunLine::new(b"{}", None, 60, 360);
    writer.run_line(run.clone());
    writer.export(b"{}".to_vec());
    assert_eq!(writer.next(), Some(Next::Line));
    assert!(writer.commit_piece(0, 1).is_err());
    assert_eq!(writer.commit_line().unwrap().bytes, Line::Run(run).encode().bytes);
    assert_eq!(writer.next(), Some(Next::Piece { kind: "export", raw: b"{}" }));
    assert!(writer.commit_line().is_err());
    let encoded = writer.commit_piece(7, 9).unwrap();
    let wanted = Line::Piece(PieceLine { piece: Piece::Export, stored: Stored { offset: 7, length: 9, raw_length: 2, sha256: sha256_hex(b"{}") } });
    assert_eq!(encoded.bytes, wanted.encode().bytes);
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

    // A fork's start: its run line, its fork line, the parent's exported world.
    let mut forker = RunWriter::new();
    let origin = ForkOrigin { branch_id: "main".to_owned(), minute: 400, state_hash: "x".to_owned(), checkpoint_hash: Some("y".to_owned()) };
    let fork = ForkLine {
        parent_branch_id: "main".to_owned(),
        minute: 400,
        origin,
        program: "{}".to_owned(),
        approved_program_hash: "z".to_owned(),
        receipt: "{}".to_owned(),
    };
    forker.fork_start(&source, fork.clone()).unwrap();
    let run = forker.commit_line().unwrap();
    let forked = RunLine::forked(source.run().unwrap(), 400, "main");
    assert_eq!(run.bytes, Line::Run(forked.clone()).encode().bytes);
    assert_eq!((forked.fork_minute(), forked.save_every, forked.format), (400, Some(360), Format::CURRENT));
    assert_eq!(forker.commit_line().unwrap().bytes, Line::Fork(fork).encode().bytes);
    assert!(matches!(forker.next(), Some(Next::Copy { kind: "export", .. })));
}
