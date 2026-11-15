//! The core of 避難所IF！ for the browser (Issue #244, PR 4-4; stage 4 of #132): a Run of a world
//! Python built and exported (`sheltersim.core.world_export.export_world`, route A), run on the
//! host without Python (`sheltersim_core::plain`), behind a JS boundary made with wasm-bindgen
//! (decision 8). It runs on one thread (decision 5: the core without its feature `parallel`).
//!
//! `Simulation` reads the export's JSON bytes, runs the ticks (`advance`) and gives the digests of
//! the world's parts as `tests/support/core_tick.py` writes them (`digests_json`) and of the
//! metric samples (`metrics_digest`): what the Node test (`tests/node_digests.mjs`) and the
//! measuring page of PR 4-5 compare with the native path's.
//!
//! It also gives the page the format of the Run it stores (#252, #251; `sheltersim_core::run_store`
//! since #368): `RunWriter` decides what to store and when and writes the journal's lines,
//! `Journal` reads the journal back and decides what a seek, a cut or a head is, and `VerifyRun`
//! reads the pieces back to the Run's digests. The page's side, `js/run_store.mjs`, opens the files
//! and compresses and decompresses the pieces. A Run is restored at a saved point with its stored
//! log and samples (`Restore`, `Simulation.restore`).
//!
//! A minute crosses the boundary as an `f64` (a JS number), not an `i64` (a `BigInt`): a world's
//! minutes are far below 2^53.

use sheltersim_core::error::Error;
use sheltersim_core::json::Encoder;
use sheltersim_core::plain::PlainRun;
use sheltersim_core::program::EffectProgram;
use sheltersim_core::value::V;
use sheltersim_core::world::WorldCell;
use sheltersim_core::{archive, checkpoint, frame, history, model_version, run_store};
use wasm_bindgen::prelude::*;

mod answers;

fn js_error(error: Error) -> JsError {
    JsError::new(&error.to_string())
}

/// A Run of an exported world.
#[wasm_bindgen]
pub struct Simulation {
    run: PlainRun,
}

#[wasm_bindgen]
impl Simulation {
    /// The Run of an exported entry, from its JSON bytes. An error for bytes that are not an
    /// exported world.
    #[wasm_bindgen(constructor)]
    pub fn new(entry: &[u8]) -> Result<Simulation, JsError> {
        Ok(Simulation { run: PlainRun::of_json(entry).map_err(js_error)? })
    }

    /// The Run of the world `name` of a file of named worlds (`{"worlds": {name: entry}}`, as the
    /// fixtures of `native/core/tests/data/` hold them), from the file's JSON bytes.
    #[wasm_bindgen(js_name = fromWorlds)]
    pub fn from_worlds(file: &[u8], name: &str) -> Result<Simulation, JsError> {
        Ok(Simulation { run: PlainRun::of_worlds_json(file, name).map_err(js_error)? })
    }

    /// The world's minute: the next tick's.
    pub fn minute(&self) -> Result<f64, JsError> {
        Ok(self.run.minute().map_err(js_error)? as f64)
    }

    /// The minute the world's clock is complete at.
    pub fn duration(&self) -> Result<f64, JsError> {
        Ok(self.run.duration().map_err(js_error)? as f64)
    }

    /// Run up to `ticks` ticks, stopping where the clock is complete: how many ran. An error is a
    /// tick's or the end-of-tick check's failure, with its minute.
    pub fn advance(&mut self, ticks: u32) -> Result<u32, JsError> {
        self.run.advance(u64::from(ticks)).map(|ran| ran as u32).map_err(|stopped| JsError::new(&stopped.to_string()))
    }

    /// The digests of the world's parts, as a JSON object in the order of `core_tick.py`'s
    /// `world_digests` (`PlainRun::run_digests_json`).
    pub fn digests_json(&self) -> Result<String, JsError> {
        self.run.run_digests_json().map_err(js_error)
    }

    /// The digest of each part of the world's canonical document (`canonical::digests`), as a JSON
    /// object in the order of the parts: what a stored Run's world piece and log read back to.
    pub fn part_digests_json(&self) -> Result<String, JsError> {
        self.run.part_digests_json().map_err(js_error)
    }

    /// The digest of the metric samples so far (`core_tick.py`'s `metrics_digest`).
    pub fn metrics_digest(&self) -> String {
        self.run.host.metrics_digest()
    }

    /// How many metric samples the Run took.
    pub fn metric_count(&self) -> u32 {
        self.run.host.samples.len() as u32
    }

    /// How many records the log holds.
    pub fn log_records(&self) -> Result<f64, JsError> {
        self.run.log_records().map(|count| count as f64).map_err(js_error)
    }

    /// The metric sample `index` as its JSON list (`MetricSnapshot`'s fields, the minute first),
    /// written again from its packed form; None past the last sample.
    pub fn metric_line(&self, index: u32) -> Result<Option<String>, JsError> {
        let line = self.run.host.samples.line(index as usize).map_err(js_error)?;
        line.map(|line| String::from_utf8(line).map_err(|error| JsError::new(&error.to_string()))).transpose()
    }
}

/// The frame of a minute (#264): the map and the indicators the API's `timeline` gives, made by the
/// core (`sheltersim_core::frame`), byte for byte the native path's.
#[wasm_bindgen]
impl Simulation {
    /// The frame of the world now, as its canonical JSON bytes (a `TimelineFrame`): `computedUntil`
    /// is how far the branch is computed, and `stateHash` the world's digest where the caller holds
    /// it (a saved world's); without it the frame's is the world's (`stateDigest`).
    pub fn frame(&mut self, run_id: &str, branch_id: &str, computed_until: f64, state_hash: Option<String>) -> Result<Vec<u8>, JsError> {
        let request = frame::Request { run_id, branch_id, computed_until: minute_of(computed_until)?, state_hash: state_hash.as_deref() };
        self.run.frame(&request).map_err(js_error)
    }

    /// `world.state_digest()` of the world now: the `state_hash` of a frame without one. The
    /// Simulation keeps the node of its log between calls (#266), so a call after `advance` reads
    /// only the records added since; the first call (after `new` or a restore) reads them all.
    #[wasm_bindgen(js_name = stateDigest)]
    pub fn state_digest(&self) -> Result<String, JsError> {
        self.run.state_digest().map_err(js_error)
    }
}

/// The summary of the world that the reading of an intervention's text is given (#302, under #292).
#[wasm_bindgen]
impl Simulation {
    /// The summary of the world now (`ScenarioSummary`, `sheltersim_core::summary`), as its
    /// canonical JSON bytes: those of `scenario_summary_from_world(world).model_dump(mode="json")`
    /// that the server's `intervention-preview` reads. It changes nothing.
    #[wasm_bindgen(js_name = scenarioSummary)]
    pub fn scenario_summary(&self) -> Result<Vec<u8>, JsError> {
        self.run.scenario_summary().map_err(js_error)
    }
}

/// The EffectProgram of an intervention before it is scheduled (#304, under #292).
#[wasm_bindgen]
impl Simulation {
    /// `validate_effect_program(world, program)` on the world now, for a program's JSON bytes:
    /// the issues as a JSON list of strings, in Python's order and texts (`sheltersim_core::program`).
    /// An error for a program the contract refuses or the browser does not check. It changes
    /// nothing.
    #[wasm_bindgen(js_name = validateEffectProgram)]
    pub fn validate_effect_program(&mut self, program: &[u8]) -> Result<String, JsError> {
        self.run.validate_effect_program_json(program).map_err(js_error)
    }
}

/// A program scheduled on the world (#305, under #292): what a fork does to its child.
#[wasm_bindgen]
impl Simulation {
    /// `schedule_effect_program(world, program)` on the world now, for a program's JSON bytes, as
    /// `BranchRuntime.apply_effect_program` calls it on a fork's child: its record and events are
    /// written, all or none. The receipt is the `command_log` entry Python appends, as JSON
    /// (`{"command": "effect_program", "program_id", "program_hash", "event_ids", "minute"}`). An
    /// error, with nothing changed, for a program with issues (their text, as Python's
    /// `EffectProgram could not be scheduled: ...`), one the contract refuses, or one the browser
    /// does not check.
    #[wasm_bindgen(js_name = applyEffectProgram)]
    pub fn apply_effect_program(&mut self, program: &[u8]) -> Result<String, JsError> {
        self.run.apply_effect_program_json(program).map_err(js_error)
    }
}

/// `EffectProgram.model_validate(json).model_dump(mode="json")` of a program's JSON bytes, as its
/// canonical JSON (#304): the bytes whose sha256 is the approval's hash (`approved_program_hash`).
/// An error for a program the contract refuses (the text is the core's, not pydantic's).
#[wasm_bindgen(js_name = effectProgramDump)]
pub fn effect_program_dump(program: &[u8]) -> Result<Vec<u8>, JsError> {
    EffectProgram::of_json(program).and_then(|program| program.dump_json()).map_err(js_error)
}

/// A minute given as a JS number: a whole number of minutes, not negative.
fn minute_of(minute: f64) -> Result<i64, JsError> {
    if minute.fract() != 0.0 || !(0.0..=9_007_199_254_740_991.0).contains(&minute) {
        return Err(JsError::new("a minute is a whole number of minutes, not negative"));
    }
    Ok(minute as i64)
}

/// Saved points (#251; `sheltersim_core::archive::SavedPoint`): the Run now as a saved point's
/// bytes, and the Run at a saved point, restored with the stored log and samples (`Restore`).
#[wasm_bindgen]
impl Simulation {
    /// The Run now as a saved point's bytes (`archive::SavedPoint`): call it between calls to
    /// `advance`.
    pub fn saved_point(&self) -> Result<Vec<u8>, JsError> {
        Ok(self.run.saved_point().map_err(js_error)?.to_bytes())
    }

    /// The Run at the saved point `restore` was made of, which has read the stored log and samples
    /// up to its counts, with the exported entry the Run started from (its JSON bytes). `restore`
    /// is used up: it cannot restore again.
    pub fn restore(entry: &[u8], restore: &mut Restore) -> Result<Simulation, JsError> {
        let entry = PlainRun::entry_of_json(entry).map_err(js_error)?;
        Ok(Simulation { run: PlainRun::restore(entry, restore.inner.take().map_err(js_error)?).map_err(js_error)? })
    }

    /// `restore` with the world `name` of a file of named worlds as the exported entry.
    #[wasm_bindgen(js_name = restoreFromWorlds)]
    pub fn restore_from_worlds(file: &[u8], name: &str, restore: &mut Restore) -> Result<Simulation, JsError> {
        let entry = PlainRun::entry_of_worlds_json(file, name).map_err(js_error)?;
        Ok(Simulation { run: PlainRun::restore(entry, restore.inner.take().map_err(js_error)?).map_err(js_error)? })
    }

    /// Whether the Run collects its decisions' records (`PlainRun::collect_decisions`, #368): a
    /// stored Run keeps them (`RunWriter.take`). The world is the same either way.
    #[wasm_bindgen(js_name = collectDecisions)]
    pub fn collect_decisions(&mut self, on: bool) {
        self.run.collect_decisions(on);
    }
}

/// A saved point being restored (`archive::Restore`): made of the saved point's bytes, then given
/// the stored log's and samples' pieces its chain names (`Chain.nextRestoreRead`, `readEntry`),
/// then used by `Simulation.restore`.
#[wasm_bindgen]
pub struct Restore {
    inner: archive::RestoreOnce,
}

#[wasm_bindgen]
impl Restore {
    /// An error for bytes that are not a saved point of the version this core reads.
    #[wasm_bindgen(constructor)]
    pub fn new(point: &[u8]) -> Result<Restore, JsError> {
        Ok(Restore { inner: archive::RestoreOnce::new(point).map_err(js_error)? })
    }

    /// Reads the piece at `position` of `chain`, decompressed (`raw`), checked against its line
    /// (`run_store::Chain::restore_read`).
    #[wasm_bindgen(js_name = readEntry)]
    pub fn read_entry(&mut self, chain: &Chain, position: u32, raw: &[u8]) -> Result<(), JsError> {
        let restore = self.inner.get_mut().map_err(js_error)?;
        chain.inner.restore_read(position as usize, raw, restore).map_err(js_error)
    }
}

// ---- The stored Run (#368: `sheltersim_core::run_store`, format v5) ------------------------------
//
// The format of a Run the page stores (`journal.jsonl` and `data.pack`) is the core's: the page's
// `js/run_store.mjs` opens the files, compresses and decompresses the pieces (deflate-raw), and
// passes the bytes through these. A line's index in a journal, and a piece's position in a chain,
// cross as a u32; a count of bytes or a minute as an f64 (a JS number).

/// A count of bytes given as a JS number: a whole number, not negative.
fn count_of(count: f64) -> Result<u64, JsError> {
    if count.fract() != 0.0 || !(0.0..=9_007_199_254_740_991.0).contains(&count) {
        return Err(JsError::new("a count of bytes is a whole number, not negative"));
    }
    Ok(count as u64)
}

/// A JSON object of strings, as model versions are (`model_version::Versions`).
fn versions_of(json: &str) -> Result<model_version::Versions, JsError> {
    model_version::versions_of_json(json.as_bytes()).map_err(js_error)
}

pub(crate) fn versions_json(versions: &model_version::Versions) -> String {
    let mut encoder = Encoder::with_capacity(256);
    encoder.out.push(b'{');
    for (index, (key, value)) in versions.iter().enumerate() {
        if index > 0 {
            encoder.out.push(b',');
        }
        encoder.string(key);
        encoder.out.push(b':');
        encoder.string(value);
    }
    encoder.out.push(b'}');
    String::from_utf8_lossy(&encoder.out).into_owned()
}

/// The whole-model items this code records (`model_version::engine_signatures`), as JSON.
#[wasm_bindgen(js_name = engineSignatures)]
pub fn engine_signatures() -> String {
    versions_json(&model_version::engine_signatures())
}

/// How the model versions a Run was computed with (`recorded`, JSON) compare with the installed
/// ones (`installed`, JSON, or undefined for configs the core cannot sign:
/// `ModelStatus::unreadable`), as JSON: `current`, `view_only`, `recorded_version`,
/// `installed_version`, `differences`, `message` (Python's text, or null) and `error` (the
/// refusal's text, `run_model_view_only: …`, or null).
#[wasm_bindgen(js_name = modelStatus)]
pub fn model_status(recorded: &str, installed: Option<String>) -> Result<String, JsError> {
    let recorded = versions_of(recorded)?;
    let status = match installed {
        Some(installed) => model_version::ModelStatus::of(&recorded, &versions_of(&installed)?),
        None => model_version::ModelStatus::unreadable(&recorded),
    };
    Ok(status_json(&status))
}

/// `modelStatus`'s JSON of a status.
pub(crate) fn status_json(status: &model_version::ModelStatus) -> String {
    let mut encoder = Encoder::with_capacity(512);
    encoder.out.extend_from_slice(b"{\"current\":");
    encoder.bool(status.current());
    encoder.out.extend_from_slice(b",\"view_only\":");
    encoder.bool(!status.current());
    encoder.out.extend_from_slice(b",\"recorded_version\":");
    encoder.string(&status.recorded_version);
    encoder.out.extend_from_slice(b",\"installed_version\":");
    encoder.string(&status.installed_version);
    encoder.out.extend_from_slice(b",\"differences\":[");
    for (index, difference) in status.differences.iter().enumerate() {
        if index > 0 {
            encoder.out.push(b',');
        }
        encoder.string(difference);
    }
    encoder.out.extend_from_slice(b"],\"message\":");
    if status.current() {
        encoder.null();
        encoder.out.extend_from_slice(b",\"error\":");
        encoder.null();
    } else {
        encoder.string(&status.message());
        encoder.out.extend_from_slice(b",\"error\":");
        encoder.string(&status.error_text());
    }
    encoder.out.push(b'}');
    String::from_utf8_lossy(&encoder.out).into_owned()
}

/// The fields an answer of a Run of other model versions adds (`ModelStatus::describe_json`, D2),
/// as JSON: `model_view` (the status of `recorded` against `installed`, as `modelStatus` takes
/// them) and, where `saved_minute` (the saved point shown) is not `requested_minute`,
/// `saved_point_rounding`.
#[wasm_bindgen(js_name = viewFields)]
pub fn view_fields(recorded: &str, installed: Option<String>, saved_minute: f64, requested_minute: f64) -> Result<String, JsError> {
    let recorded = versions_of(recorded)?;
    let status = match installed {
        Some(installed) => model_version::ModelStatus::of(&recorded, &versions_of(&installed)?),
        None => model_version::ModelStatus::unreadable(&recorded),
    };
    String::from_utf8(status.describe_json(minute_of(saved_minute)?, minute_of(requested_minute)?)).map_err(|error| JsError::new(&error.to_string()))
}

/// A stored Run's journal read back (`run_store::Journal::parse`): its lines up to the first that
/// is cut short or does not match its sha256, and what a reader decides of them alone.
#[wasm_bindgen]
pub struct Journal {
    inner: run_store::Journal,
}

#[wasm_bindgen]
impl Journal {
    /// The journal's bytes; an error for a line that matches its sha256 but is not one of this
    /// format (one of an older format: `stored_run_outdated: …`).
    #[wasm_bindgen(constructor)]
    pub fn new(bytes: &[u8]) -> Result<Journal, JsError> {
        Ok(Journal { inner: run_store::Journal::parse(bytes).map_err(js_error)? })
    }

    /// Whether reading stopped at a line cut short or not matching its sha256.
    pub fn torn(&self) -> bool {
        self.inner.torn()
    }

    /// The lines read, as a JSON list of their JSON texts as the journal holds them.
    #[wasm_bindgen(js_name = linesJson)]
    pub fn lines_json(&self) -> String {
        let entries = self.inner.entries();
        let mut text = String::with_capacity(entries.iter().map(|entry| entry.json.len() + 1).sum::<usize>() + 2);
        text.push('[');
        for (index, entry) in entries.iter().enumerate() {
            if index > 0 {
                text.push(',');
            }
            text.push_str(&entry.json);
        }
        text.push(']');
        text
    }

    /// The first line of `kind`'s index.
    pub fn find(&self, kind: &str) -> Option<u32> {
        self.inner.find(kind).map(|index| index as u32)
    }

    /// An error for a journal without its "run" line: not a stored Run.
    #[wasm_bindgen(js_name = checkRun)]
    pub fn check_run(&self) -> Result<(), JsError> {
        self.inner.run().map(|_| ()).map_err(js_error)
    }

    /// The minute the Run starts at: the fork's of a branch made by a fork, else 0.
    #[wasm_bindgen(js_name = forkMinute)]
    pub fn fork_minute(&self) -> Result<f64, JsError> {
        Ok(self.inner.fork_minute().map_err(js_error)? as f64)
    }

    /// The minute the Run's clock is complete at.
    pub fn duration(&self) -> Result<f64, JsError> {
        Ok(self.inner.duration().map_err(js_error)? as f64)
    }

    /// Minutes between saved points (0: at the end only).
    #[wasm_bindgen(js_name = saveEvery)]
    pub fn save_every(&self) -> Result<f64, JsError> {
        Ok(self.inner.run().map_err(js_error)?.save_every as f64)
    }

    /// The model versions the Run is computed with, as JSON.
    #[wasm_bindgen(js_name = modelVersionsJson)]
    pub fn model_versions_json(&self) -> Result<String, JsError> {
        Ok(versions_json(&self.inner.run().map_err(js_error)?.model_versions))
    }

    /// The line of the parent's journal a fork's Run reads the parent's pieces up to (its "fork"
    /// line's base), or undefined for a Run from minute 0.
    #[wasm_bindgen(js_name = forkBase)]
    pub fn fork_base(&self) -> Option<u32> {
        self.inner.fork().map(|fork| fork.base.line as u32)
    }

    /// The lines a branch's head may be (`run_store::Journal::heads`), as a JSON list of
    /// `{kind, minute, index, head}`: `head` is the line's head hash, a branch record's
    /// `checkpoint_hash`.
    #[wasm_bindgen(js_name = headsJson)]
    pub fn heads_json(&self) -> String {
        let mut encoder = Encoder::with_capacity(256);
        encoder.out.push(b'[');
        for (index, head) in self.inner.heads().iter().enumerate() {
            if index > 0 {
                encoder.out.push(b',');
            }
            encoder.out.extend_from_slice(b"{\"kind\":");
            encoder.string(head.kind.name());
            encoder.out.extend_from_slice(b",\"minute\":");
            encoder.int(head.minute);
            encoder.out.extend_from_slice(b",\"index\":");
            encoder.int(head.index as i64);
            encoder.out.extend_from_slice(b",\"head\":");
            encoder.string(&self.inner.entries()[head.index].head);
            encoder.out.push(b'}');
        }
        encoder.out.push(b']');
        String::from_utf8_lossy(&encoder.out).into_owned()
    }

    /// The stored Run cut back to the head `head` (`run_store::Journal::cut`), refused where a fork
    /// of the branch reads its lines up to `kept_through`, past the head: `[the head's line index,
    /// the journal's bytes to keep, data.pack's bytes to keep]`.
    pub fn cut(&self, head: &str, kept_through: Option<u32>) -> Result<Vec<f64>, JsError> {
        let cut = self.inner.cut(head, kept_through.map(|line| line as usize)).map_err(js_error)?;
        Ok(vec![cut.head.index as f64, cut.journal_end as f64, cut.data_end as f64])
    }

    /// The writer that goes on after the line `head` (`run_store::RunWriter::after`).
    #[wasm_bindgen(js_name = writerAfter)]
    pub fn writer_after(&self, head: u32) -> Result<RunWriter, JsError> {
        Ok(RunWriter { inner: run_store::RunWriter::after(&self.inner, head as usize).map_err(js_error)?, last_head: None })
    }

    /// The writer that appends conversation turns to a Run stored to its end
    /// (`run_store::RunWriter::after_end`).
    #[wasm_bindgen(js_name = writerAfterEnd)]
    pub fn writer_after_end(&self) -> Result<RunWriter, JsError> {
        Ok(RunWriter { inner: run_store::RunWriter::after_end(&self.inner).map_err(js_error)?, last_head: None })
    }

    /// The minute of the saved point at or before `minute` (`run_store::Journal::saved_minute_at`):
    /// what a Run of other model versions shows for `minute`.
    #[wasm_bindgen(js_name = savedMinuteAt)]
    pub fn saved_minute_at(&self, minute: f64) -> Result<f64, JsError> {
        Ok(self.inner.saved_minute_at(minute_of(minute)?).map_err(js_error)? as f64)
    }

    /// The "saved" lines a seek can restore, by their indexes.
    #[wasm_bindgen(js_name = savedPoints)]
    pub fn saved_points(&self) -> Vec<u32> {
        self.inner.saved_points().into_iter().map(|index| index as u32).collect()
    }

    /// Where the piece `index` is: `[offset, length]` of its compressed bytes in data.pack.
    pub fn place(&self, index: u32) -> Result<Vec<f64>, JsError> {
        let stored = &self.inner.piece(index as usize).map_err(js_error)?.stored;
        Ok(vec![stored.offset as f64, stored.length as f64])
    }

    /// An error where `raw`, the piece `index` decompressed, is not the bytes written.
    #[wasm_bindgen(js_name = checkRaw)]
    pub fn check_raw(&self, index: u32, raw: &[u8]) -> Result<(), JsError> {
        self.inner.check_raw(index as usize, raw).map_err(js_error)
    }
}

/// A stored Run with the stored Runs of its parents it reads pieces of (`run_store::Chain`): made
/// of the branch's journal, then given each journal `parentNeeded` names (`addParent`). A piece is
/// named by its position in the chain; `place` says which level's data.pack holds it (0: the Run
/// from minute 0, the branch's the last).
#[wasm_bindgen]
pub struct Chain {
    inner: run_store::Chain,
}

#[wasm_bindgen]
impl Chain {
    /// The chain of the branch's journal (a copy of it).
    #[wasm_bindgen(constructor)]
    pub fn new(journal: &Journal) -> Result<Chain, JsError> {
        Ok(Chain { inner: run_store::Chain::new(journal.inner.clone()).map_err(js_error)? })
    }

    /// The branch whose journal the chain needs next, or undefined once it is complete.
    #[wasm_bindgen(js_name = parentNeeded)]
    pub fn parent_needed(&self) -> Option<String> {
        self.inner.parent_needed().map(str::to_owned)
    }

    /// Adds the journal of the branch `parentNeeded` names (a copy of it): an error where it does
    /// not hold the fork's base as it was.
    #[wasm_bindgen(js_name = addParent)]
    pub fn add_parent(&mut self, journal: &Journal) -> Result<(), JsError> {
        self.inner.add_parent(journal.inner.clone()).map_err(js_error)
    }

    /// How many journals the chain holds.
    pub fn levels(&self) -> u32 {
        self.inner.levels().len() as u32
    }

    /// How many pieces the chain has.
    pub fn pieces(&self) -> u32 {
        self.inner.pieces().len() as u32
    }

    /// The kind of the piece at `position`.
    pub fn kind(&self, position: u32) -> Result<String, JsError> {
        Ok(self.inner.piece(position as usize).map_err(js_error)?.0.piece.kind().to_owned())
    }

    /// Where the piece at `position` is: `[level, offset, length]` of its compressed bytes.
    pub fn place(&self, position: u32) -> Result<Vec<f64>, JsError> {
        let (level, stored) = self.inner.stored(position as usize).map_err(js_error)?;
        Ok(vec![level as f64, stored.offset as f64, stored.length as f64])
    }

    /// An error where `length` bytes were read for the piece at `position`.
    #[wasm_bindgen(js_name = checkStored)]
    pub fn check_stored(&self, position: u32, length: f64) -> Result<(), JsError> {
        self.inner.check_stored(position as usize, count_of(length)?).map_err(js_error)
    }

    /// An error where `raw`, the piece at `position` decompressed, is not the bytes written.
    #[wasm_bindgen(js_name = checkRaw)]
    pub fn check_raw(&self, position: u32, raw: &[u8]) -> Result<(), JsError> {
        self.inner.check_raw(position as usize, raw).map_err(js_error)
    }

    /// The exported world's piece.
    pub fn export(&self) -> Result<u32, JsError> {
        Ok(self.inner.export().map_err(js_error)? as u32)
    }

    /// Where a seek to `minute` starts (`run_store::Chain::seek`): `[the branch's "saved" line's
    /// index or -1 for the exported world, the branch's line restored (the base a fork made there
    /// goes on from), the exported world's piece, the minute it is computed from]`.
    pub fn seek(&self, minute: f64) -> Result<Vec<f64>, JsError> {
        let seek = self.inner.seek(minute_of(minute)?).map_err(js_error)?;
        Ok(vec![seek.from.map_or(-1.0, |index| index as f64), seek.base as f64, seek.export as f64, seek.from_minute as f64])
    }

    /// The next piece from `start` on that `restore` needs, or undefined once it needs none.
    #[wasm_bindgen(js_name = nextRestoreRead)]
    pub fn next_restore_read(&self, start: u32, restore: &Restore) -> Result<Option<u32>, JsError> {
        let restore = restore.inner.get().map_err(js_error)?;
        Ok(self.inner.next_restore_read(start as usize, restore).map_err(js_error)?.map(|position| position as u32))
    }

    /// The samples' pieces: what the KPI history reads.
    #[wasm_bindgen(js_name = samplePieces)]
    pub fn sample_pieces(&self) -> Result<Vec<u32>, JsError> {
        Ok(self.inner.sample_pieces().map_err(js_error)?.into_iter().map(|position| position as u32).collect())
    }

    /// The pieces of the records of `kind` ("decisions", "conversations" or "llm_tape").
    pub fn records(&self, kind: &str) -> Result<Vec<u32>, JsError> {
        let kind =
            run_store::RecordKind::ALL.into_iter().find(|each| each.name() == kind).ok_or_else(|| JsError::new(&format!("no records of kind {kind}")))?;
        Ok(self.inner.records(kind).map_err(js_error)?.into_iter().map(|position| position as u32).collect())
    }
}

/// The JSON texts of a records piece, decompressed (`run_store::records::read`), as a JSON list.
#[wasm_bindgen(js_name = recordsJson)]
pub fn records_json(raw: &[u8]) -> Result<String, JsError> {
    let records = run_store::records::read(raw).map_err(js_error)?;
    let mut text = Vec::with_capacity(records.iter().map(|record| record.len() + 1).sum::<usize>() + 2);
    text.push(b'[');
    for (index, record) in records.iter().enumerate() {
        if index > 0 {
            text.push(b',');
        }
        text.extend_from_slice(record);
    }
    text.push(b']');
    String::from_utf8(text).map_err(|error| JsError::new(&error.to_string()))
}

/// A stored Run read back to its digests (`run_store::Verify`).
#[wasm_bindgen]
pub struct VerifyRun {
    inner: run_store::Verify,
}

#[wasm_bindgen]
impl VerifyRun {
    #[wasm_bindgen(constructor)]
    pub fn new() -> VerifyRun {
        VerifyRun { inner: run_store::Verify::new() }
    }

    /// The pieces to read, in order, by their positions in `chain`.
    pub fn reads(&self, chain: &Chain) -> Result<Vec<u32>, JsError> {
        Ok(run_store::Verify::reads(&chain.inner).map_err(js_error)?.into_iter().map(|position| position as u32).collect())
    }

    /// Reads the piece at `position`, decompressed (`raw`).
    pub fn read(&mut self, chain: &Chain, position: u32, raw: &[u8]) -> Result<(), JsError> {
        self.inner.read(&chain.inner, position as usize, raw).map_err(js_error)
    }

    /// What reading back gave (`run_store::Verified::to_json`).
    #[wasm_bindgen(js_name = resultJson)]
    pub fn result_json(&self, chain: &Chain) -> Result<String, JsError> {
        let result = self.inner.result(&chain.inner).map_err(js_error)?;
        String::from_utf8(result.to_json()).map_err(|error| JsError::new(&error.to_string()))
    }
}

impl Default for VerifyRun {
    fn default() -> Self {
        Self::new()
    }
}

/// The writer of a stored Run (`run_store::RunWriter`): it queues what to store; the page writes
/// each item in order (`nextKind`, then `commitPiece` or `commitLine`) and appends the line each
/// commit gives to the journal.
#[wasm_bindgen]
pub struct RunWriter {
    inner: run_store::RunWriter,
    last_head: Option<String>,
}

#[wasm_bindgen]
impl RunWriter {
    /// The writer of a new Run.
    #[wasm_bindgen(constructor)]
    pub fn new() -> RunWriter {
        RunWriter { inner: run_store::RunWriter::new(), last_head: None }
    }

    /// Queues the "run" line and the "export" piece of a new Run of the exported world `export`
    /// (the world `world` of a file of named worlds) that `simulation` was made of, computed with
    /// the model versions `model_versions` (JSON), its pieces compressed by `compression`, saved
    /// every `save_every` minutes.
    pub fn start(
        &mut self,
        simulation: &Simulation,
        export: Vec<u8>,
        world: Option<String>,
        save_every: f64,
        model_versions: &str,
        compression: String,
    ) -> Result<(), JsError> {
        let storing = run_store::Storing { model_versions: versions_of(model_versions)?, compression, save_every: count_of(save_every)? };
        self.inner.start(&simulation.run, export, world, &storing).map_err(js_error)
    }

    /// Queues the start of a Run from minute 0 of the stored Run `source`'s exported world: its
    /// "run" line and `source`'s "export" piece, copied.
    #[wasm_bindgen(js_name = sameStart)]
    pub fn same_start(&mut self, source: &Journal) -> Result<(), JsError> {
        self.inner.same_start(&source.inner).map_err(js_error)
    }

    /// Queues the start of the Run of a branch made by a fork from `parent` at `minute` (#306),
    /// which reads `parent`'s pieces up to its line `base` (the line the fork's world was restored
    /// from): its "run" and "fork" lines.
    #[allow(clippy::too_many_arguments)]
    #[wasm_bindgen(js_name = forkStart)]
    pub fn fork_start(
        &mut self,
        parent: &Journal,
        base: u32,
        minute: f64,
        parent_branch_id: String,
        state_hash: String,
        checkpoint_hash: Option<String>,
        program: String,
        approved_program_hash: String,
        receipt: String,
    ) -> Result<(), JsError> {
        let minute = minute_of(minute)?;
        let fork = run_store::Fork {
            origin: run_store::ForkOrigin { branch_id: parent_branch_id.clone(), minute, state_hash, checkpoint_hash },
            parent_branch_id,
            minute,
            program,
            approved_program_hash,
            receipt,
        };
        self.inner.fork_start(&parent.inner, base as usize, fork).map_err(js_error)
    }

    /// Takes the decision records `simulation` collected, and queues the tables' new entries and the
    /// newly sealed blocks (every block when `last`).
    pub fn take(&mut self, simulation: &mut Simulation, last: bool) -> Result<(), JsError> {
        self.inner.take(&mut simulation.run, last).map_err(js_error)
    }

    /// Queues a saved point of `simulation` now, its blocks sealed and stored first.
    pub fn save(&mut self, simulation: &mut Simulation) -> Result<(), JsError> {
        self.inner.save(&mut simulation.run).map_err(js_error)
    }

    /// Queues the decision records left, the "world" piece and the "end" line.
    pub fn finish(&mut self, simulation: &mut Simulation) -> Result<(), JsError> {
        self.inner.finish(&mut simulation.run).map_err(js_error)
    }

    /// Whether `simulation`, restored at the head the writer goes on after, holds what is stored.
    pub fn fits(&self, simulation: &Simulation) -> Result<bool, JsError> {
        self.inner.fits(&simulation.run).map_err(js_error)
    }

    /// Queues a piece of one conversation turn (its JSON text), of a writer after the end
    /// (`Journal.writerAfterEnd`).
    pub fn turn(&mut self, turn: Vec<u8>) -> Result<(), JsError> {
        self.inner.turns(&[turn]).map_err(js_error)
    }

    /// The kind of the item to write next ("line" for a line without a piece, "copy:<kind>" for a
    /// piece whose compressed bytes are copied), or undefined when everything is written.
    #[wasm_bindgen(js_name = nextKind)]
    pub fn next_kind(&self) -> Option<String> {
        self.inner.next().map(|next| match next {
            run_store::Next::Line => "line".to_owned(),
            run_store::Next::Piece { kind, .. } => kind.to_owned(),
            run_store::Next::Copy { kind, .. } => format!("copy:{kind}"),
        })
    }

    /// The bytes of the piece to write next, before compression; undefined for a line or a copy.
    #[wasm_bindgen(js_name = nextRaw)]
    pub fn next_raw(&self) -> Option<Vec<u8>> {
        match self.inner.next() {
            Some(run_store::Next::Piece { raw, .. }) => Some(raw.to_vec()),
            _ => None,
        }
    }

    /// The line of the line without a piece queued next, to append to the journal.
    #[wasm_bindgen(js_name = commitLine)]
    pub fn commit_line(&mut self) -> Result<Vec<u8>, JsError> {
        let encoded = self.inner.commit_line().map_err(js_error)?;
        self.last_head = Some(encoded.head);
        Ok(encoded.bytes)
    }

    /// The line of the piece queued next, written at `offset` of data.pack, `length` bytes
    /// compressed.
    #[wasm_bindgen(js_name = commitPiece)]
    pub fn commit_piece(&mut self, offset: f64, length: f64) -> Result<Vec<u8>, JsError> {
        let encoded = self.inner.commit_piece(count_of(offset)?, count_of(length)?).map_err(js_error)?;
        self.last_head = Some(encoded.head);
        Ok(encoded.bytes)
    }

    /// The head hash of the last line committed (a branch's head is one).
    #[wasm_bindgen(js_name = lastHead)]
    pub fn last_head(&self) -> Option<String> {
        self.last_head.clone()
    }

    /// The saved points' lines committed, as a JSON list of their fields.
    #[wasm_bindgen(js_name = savedJson)]
    pub fn saved_json(&self) -> String {
        let mut encoder = Encoder::with_capacity(256);
        encoder.out.push(b'[');
        for (index, saved) in self.inner.saved().iter().enumerate() {
            if index > 0 {
                encoder.out.push(b',');
            }
            encoder.out.push(b'{');
            let fields = [
                ("records", saved.records),
                ("samples", saved.samples),
                ("decisions", saved.counts.decisions),
                ("point_version", saved.point_version),
                ("log_blocks", saved.log_blocks),
                ("sample_blocks", saved.sample_blocks),
            ];
            encoder.out.extend_from_slice(b"\"minute\":");
            encoder.int(saved.minute);
            for (name, value) in fields {
                encoder.out.push(b',');
                encoder.string(name);
                encoder.out.push(b':');
                encoder.int(value as i64);
            }
            encoder.out.push(b'}');
        }
        encoder.out.push(b']');
        String::from_utf8_lossy(&encoder.out).into_owned()
    }
}

impl Default for RunWriter {
    fn default() -> Self {
        Self::new()
    }
}

/// How many ticks to run from `minute`: at most `ticks`, not past the next saved point of a Run
/// saved every `every` minutes (none when 0) nor past `target` (`run_store::Schedule::step`).
#[wasm_bindgen(js_name = savedPointStep)]
pub fn saved_point_step(minute: f64, ticks: u32, every: f64, target: Option<f64>) -> Result<u32, JsError> {
    let target = target.map(minute_of).transpose()?;
    let step = run_store::Schedule { every: count_of(every)? }.step(minute_of(minute)?, u64::from(ticks), target);
    Ok(step.min(u64::from(ticks)) as u32)
}

/// Whether a saved point is due at `minute` of a Run whose clock ends at `duration`, saved every
/// `every` minutes (`run_store::Schedule::due`).
#[wasm_bindgen(js_name = savedPointDue)]
pub fn saved_point_due(minute: f64, duration: f64, every: f64) -> Result<bool, JsError> {
    Ok(run_store::Schedule { every: count_of(every)? }.due(minute_of(minute)?, minute_of(duration)?))
}

/// Whether `minute` is one of the saved points' minutes of a Run saved every `every` minutes.
#[wasm_bindgen(js_name = isSavedPointMinute)]
pub fn is_saved_point_minute(minute: f64, every: f64) -> Result<bool, JsError> {
    Ok(run_store::Schedule { every: count_of(every)? }.is_point(minute_of(minute)?))
}

/// What the server's `scenario-preview` reads of a Scenario text without an API key
/// (`sheltersim_core::scenario::text::parse`, #294), as JSON: `request` (its dump, or null),
/// `missing_fields`, `source`, `warnings` and `input_sha256`. An error whose message starts with
/// `invalid: ` is what Python refuses (the server's 400); `unsupported: `, what the browser does
/// not compute.
#[wasm_bindgen(js_name = parseScenarioText)]
pub fn parse_scenario_text(text: &str) -> Result<String, JsError> {
    use sheltersim_core::scenario::request::Refusal;
    match sheltersim_core::scenario::text::parse(text) {
        Ok(parsed) => {
            let bytes = parsed.to_value().to_json().map_err(js_error)?;
            String::from_utf8(bytes).map_err(|error| JsError::new(&error.to_string()))
        }
        Err(Refusal::Invalid(why)) => Err(JsError::new(&format!("invalid: {why}"))),
        Err(Refusal::Unsupported(why)) => Err(JsError::new(&format!("unsupported: {why}"))),
    }
}

/// What the server's `scenario-preview` reads of a Scenario text when the LLM answered `output`
/// (`sheltersim_core::scenario::text::parse_answer`, #421): JSON as `parseScenarioText`'s, its
/// `source` `prepare_llm` when the answer was used, `rules_fallback` when the task's schema refused
/// it, `rules` when it could not be read back. Errors as `parseScenarioText`'s; `unsupported: `
/// also when the answer holds a condition the browser does not compute.
#[wasm_bindgen(js_name = parseScenarioTextAnswer)]
pub fn parse_scenario_text_answer(text: &str, output: &str) -> Result<String, JsError> {
    use sheltersim_core::scenario::request::Refusal;
    match sheltersim_core::scenario::text::parse_answer(text, output) {
        Ok(parsed) => {
            let bytes = parsed.to_value().to_json().map_err(js_error)?;
            String::from_utf8(bytes).map_err(|error| JsError::new(&error.to_string()))
        }
        Err(Refusal::Invalid(why)) => Err(JsError::new(&format!("invalid: {why}"))),
        Err(Refusal::Unsupported(why)) => Err(JsError::new(&format!("unsupported: {why}"))),
    }
}

/// `InitialScenarioRequest.model_validate` of a request's JSON bytes and its
/// `model_dump(mode="json")` (`sheltersim_core::scenario::request`, #295), as JSON:
/// `{"request": dump, "input_sha256": sha256 of the dump's sorted JSON}` (the structured form's
/// input hash). Errors as `parseScenarioText`'s.
#[wasm_bindgen(js_name = scenarioRequest)]
pub fn scenario_request(raw: &[u8]) -> Result<String, JsError> {
    use sheltersim_core::scenario::request::{InitialScenarioRequest, Refusal};
    match InitialScenarioRequest::of_json(raw) {
        Ok(request) => {
            let mut out = b"{\"input_sha256\":\"".to_vec();
            out.extend_from_slice(request.input_sha256().as_bytes());
            out.extend_from_slice(b"\",\"request\":");
            out.extend_from_slice(&request.dump_json());
            out.push(b'}');
            String::from_utf8(out).map_err(|error| JsError::new(&error.to_string()))
        }
        Err(Refusal::Invalid(why)) => Err(JsError::new(&format!("invalid: {why}"))),
        Err(Refusal::Unsupported(why)) => Err(JsError::new(&format!("unsupported: {why}"))),
    }
}

/// Python's `str.casefold` (`sheltersim_core::text::casefold`): what the regional research compares
/// names with (`js/research.mjs`, #300).
#[wasm_bindgen]
pub fn casefold(text: &str) -> String {
    sheltersim_core::text::casefold(text).into_owned()
}

/// The entry of a Scenario's world at minute 0, from the Scenario's canonical JSON
/// (`sheltersim_core::initial::entry`, #249, #429): the bytes Python's `export_world` writes of it,
/// which `new Simulation(entry)` runs. A Scenario Python refuses to build is the error Python
/// raises; what the core leaves to a contract Python runs first (a config `ScenarioSpec` would not
/// have made canonical, and the like) is an error whose message starts with `unsupported: `; any
/// other error is input that is not a Scenario.
#[wasm_bindgen(js_name = initialEntry)]
pub fn initial_entry(scenario: &[u8]) -> Result<Vec<u8>, JsError> {
    sheltersim_core::initial::entry(scenario).map_err(initial_error)
}

/// The `model_versions` of a new Run of a Scenario (`sheltersim_core::initial::model_versions`,
/// #301), as JSON: `model_signatures(scenario.model_configs)`. Errors as `initialEntry`'s.
#[wasm_bindgen(js_name = modelVersions)]
pub fn model_versions(scenario: &[u8]) -> Result<String, JsError> {
    let value = sheltersim_core::initial::model_versions(scenario).map_err(initial_error)?;
    String::from_utf8(value.to_json().map_err(js_error)?).map_err(|error| JsError::new(&error.to_string()))
}

/// The evidence route's answer of a new Run of a Scenario (`sheltersim_core::initial::evidence`,
/// #301), as canonical JSON bytes: what the Run's `evidence.json` holds. Errors as
/// `initialEntry`'s.
#[wasm_bindgen(js_name = runEvidence)]
pub fn run_evidence(scenario: &[u8]) -> Result<Vec<u8>, JsError> {
    sheltersim_core::initial::evidence(scenario).map_err(initial_error)
}

fn initial_error(error: Error) -> JsError {
    match error {
        Error::Unsupported(reason) => JsError::new(&format!("unsupported: {reason}")),
        other => js_error(other),
    }
}

/// `hashlib.sha256(bytes).hexdigest()`: what the page's journal checks its lines and pieces by.
#[wasm_bindgen(js_name = sha256Hex)]
pub fn sha256_hex(bytes: &[u8]) -> String {
    checkpoint::sha256_hex(bytes)
}

/// The size of the WASM memory now, in bytes (`memory.buffer.byteLength`): it only grows, so read
/// after a Run it is the Run's peak. 0 off wasm32.
#[wasm_bindgen(js_name = memoryBytes)]
pub fn memory_bytes() -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        (core::arch::wasm32::memory_size(0) * 65536) as f64
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        0.0
    }
}

/// What the API's KPI history and offline report read (#266; `sheltersim_core::history`): a
/// stored Run's metric samples read back and paged as the history index pages them, and, of a
/// Run, its records and its samples' final metrics. The page's side is `js/api_answers.mjs`.
#[wasm_bindgen]
pub struct KpiHistory {
    table: history::KpiTable,
}

#[wasm_bindgen]
impl KpiHistory {
    #[wasm_bindgen(constructor)]
    pub fn new() -> KpiHistory {
        KpiHistory { table: history::KpiTable::new() }
    }

    pub fn read_sample_tables(&mut self, bytes: &[u8]) -> Result<(), JsError> {
        self.table.read_tables(bytes).map_err(js_error)
    }

    /// How many samples the block held.
    pub fn read_sample_block(&mut self, bytes: &[u8]) -> Result<u32, JsError> {
        self.table.read_block(bytes).map(|read| read as u32).map_err(js_error)
    }

    /// How many samples were read.
    pub fn sample_count(&self) -> f64 {
        self.table.len() as f64
    }

    /// The samples from `start` to `end`: `[count, first minute, last minute]`, or `[0]`.
    pub fn coverage(&self, start: f64, end: f64) -> Result<Vec<f64>, JsError> {
        Ok(match self.table.coverage(minute_of(start)?, minute_of(end)?) {
            Some((count, first, last)) => vec![count as f64, first as f64, last as f64],
            None => vec![0.0],
        })
    }

    /// A page of rows (`history::KpiQuery`): `afterMinute` and `afterKey` are the cursor's row
    /// (both or neither).
    #[allow(clippy::too_many_arguments)]
    pub fn page(
        &self,
        start: f64,
        end: f64,
        step: f64,
        limit: u32,
        after_minute: Option<f64>,
        after_key: Option<String>,
        filtered: bool,
        run_id: &str,
        branch_id: &str,
    ) -> Result<KpiPage, JsError> {
        let after = match (after_minute, after_key.as_deref()) {
            (Some(minute), Some(key)) => Some((minute as i64, key)),
            (None, None) => None,
            _ => return Err(JsError::new("a cursor's row has a minute and a key")),
        };
        let query = history::KpiQuery {
            start: minute_of(start)?,
            end: minute_of(end)?,
            step: minute_of(step)?,
            limit: limit as usize,
            after,
            filtered,
            run_id,
            branch_id,
        };
        let page = self.table.page(&query).map_err(js_error)?;
        Ok(KpiPage { items: String::from_utf8(page.items).map_err(|error| JsError::new(&error.to_string()))?, last: page.last, more: page.more })
    }
}

impl Default for KpiHistory {
    fn default() -> Self {
        Self::new()
    }
}

/// A page of KPI rows (`KpiHistory.page`).
#[wasm_bindgen]
pub struct KpiPage {
    items: String,
    last: Option<i64>,
    more: bool,
}

#[wasm_bindgen]
impl KpiPage {
    /// The rows, as a JSON list (canonical JSON, the floats as Python writes them).
    pub fn items(&self) -> String {
        self.items.clone()
    }

    /// The minute of the last row, or None for no rows.
    pub fn last(&self) -> Option<f64> {
        self.last.map(|minute| minute as f64)
    }

    /// Whether rows follow the page.
    pub fn more(&self) -> bool {
        self.more
    }
}

#[wasm_bindgen]
impl Simulation {
    /// The record `index` of the log, as its canonical JSON (`EventRecord.to_dict()`), or None past
    /// the last.
    pub fn log_record(&self, index: f64) -> Result<Option<String>, JsError> {
        let mut out = Vec::new();
        let written = self
            .run
            .loaded
            .cell
            .with(|world| {
                world.log().and_then(|log| if index >= 0.0 && index < log.len() as f64 { log.write_record(index as usize, &mut out) } else { Ok(false) })
            })
            .and_then(|written| written)
            .map_err(js_error)?;
        if !written {
            return Ok(None);
        }
        String::from_utf8(out).map(Some).map_err(|error| JsError::new(&error.to_string()))
    }

    /// The final metrics of the offline report's evidence (`history::review_metrics`) of the
    /// sample `index`, as canonical JSON, or None past the last sample.
    pub fn review_metrics(&self, index: u32) -> Result<Option<String>, JsError> {
        let Some((sample, _, _)) = self.run.host.samples.sample(index as usize).map_err(js_error)? else {
            return Ok(None);
        };
        let metrics = history::review_metrics(&sample).map_err(js_error)?;
        String::from_utf8(metrics).map(Some).map_err(|error| JsError::new(&error.to_string()))
    }
}

/// A Scenario the screen's request builds (`buildScenario`, #299): what `scenario-preview`
/// answers of it, but the text parser's warnings, the research and the IDs, which the route adds.
#[wasm_bindgen]
pub struct BuiltScenario {
    preview: sheltersim_core::scenario::builder::Preview,
}

#[wasm_bindgen]
impl BuiltScenario {
    /// The Scenario's canonical JSON (`ScenarioSpec.canonical_json()`); its sha256 is
    /// `approvedHash`. Hand it on as bytes: a `JSON.parse` and `stringify` changes `1.0` to `1`.
    pub fn scenario(&self) -> Vec<u8> {
        self.preview.built.canonical.clone()
    }

    #[wasm_bindgen(getter, js_name = approvedHash)]
    pub fn approved_hash(&self) -> String {
        self.preview.built.approved_hash.clone()
    }

    /// `FacilityDesign.model_dump(mode="json")`, as canonical JSON.
    #[wasm_bindgen(js_name = facilityDesign)]
    pub fn facility_design(&self) -> Result<Vec<u8>, JsError> {
        self.preview.facility_design.to_json().map_err(js_error)
    }

    #[wasm_bindgen(getter, js_name = geometryHash)]
    pub fn geometry_hash(&self) -> String {
        self.preview.geometry_hash.clone()
    }

    /// `InitialScenarioRequest.model_dump(mode="json")`, as canonical JSON (the answer's
    /// `request`).
    pub fn request(&self) -> Result<Vec<u8>, JsError> {
        self.preview.request.to_json().map_err(js_error)
    }

    /// The builder's warnings, as a JSON list (the route puts the text parser's first).
    pub fn warnings(&self) -> Result<String, JsError> {
        texts_json(&self.preview.built.warnings)
    }

    /// The approval blockers, as a JSON list.
    #[wasm_bindgen(js_name = approvalBlockers)]
    pub fn approval_blockers(&self) -> Result<String, JsError> {
        texts_json(&self.preview.built.approval_blockers)
    }

    #[wasm_bindgen(getter, js_name = canStart)]
    pub fn can_start(&self) -> bool {
        self.preview.built.can_start
    }
}

fn texts_json(items: &[String]) -> Result<String, JsError> {
    let list = V::List(items.iter().map(|item| V::Str(item.as_str().into())).collect());
    String::from_utf8(list.to_json().map_err(js_error)?).map_err(|error| JsError::new(&error.to_string()))
}

/// `scenario-preview`'s build (`sheltersim_core::scenario::builder::build`, #299): the request's
/// JSON bytes (the form's request as posted, or the text parser's), the research's JSON bytes
/// (`RegionResearchResult.model_dump(mode="json")`), and the input's sha256 and extraction source
/// (`provenance.scenario_input`); an empty `input_sha256` is the request's own, the sha256 of its
/// dump's canonical JSON, as the structured form's (`structured_form`). A refusal is an Error whose message starts with
/// `browser_unsupported: ` (a field or value the browser does not compute) or `builder_refused: `
/// (the server answers 400 `invalid project request`).
#[wasm_bindgen(js_name = buildScenario)]
pub fn build_scenario(request: &[u8], research: &[u8], input_sha256: &str, extraction_source: &str) -> Result<BuiltScenario, JsError> {
    use sheltersim_core::scenario::builder::{build, Refusal, ScenarioInput};
    let sha256 = if input_sha256.is_empty() {
        match sheltersim_core::scenario::request::InitialScenarioRequest::of_json(request) {
            Ok(parsed) => parsed.input_sha256(),
            Err(_) => String::new(),
        }
    } else {
        input_sha256.to_owned()
    };
    let input = ScenarioInput { sha256, extraction_source: extraction_source.to_owned() };
    match build(request, research, &input) {
        Ok(preview) => Ok(BuiltScenario { preview }),
        Err(Refusal::Unsupported(field)) => Err(JsError::new(&format!("browser_unsupported: {field}"))),
        Err(Refusal::Invalid(text)) => Err(JsError::new(&format!("builder_refused: {text}"))),
    }
}

/// What the offline intervention compiler answers (#303, `sheltersim_core::interpret`): the
/// canonical JSON of the `CompilerResult` for the JSON of a `ParseRequest` (`text`, `branch_id`,
/// `selected_minute`, `scenario_summary`), or None where Python refuses the request with a
/// ValueError (the server's 400 "invalid project request").
#[wasm_bindgen(js_name = compileInterventionOffline)]
pub fn compile_intervention_offline(request: &[u8]) -> Result<Option<String>, JsError> {
    match sheltersim_core::interpret::compile_offline(request) {
        Ok(body) => String::from_utf8(body).map(Some).map_err(|error| JsError::new(&error.to_string())),
        Err(Error::Value(_)) => Ok(None),
        Err(error) => Err(js_error(error)),
    }
}

/// The canonical JSON of the object `base` with the fields of the object `extra` put in
/// (`sheltersim_core::interpret::merge_objects`): an answer the core wrote, with a route's fields
/// added, whose floats keep their digits (#306).
#[wasm_bindgen(js_name = mergeJson)]
pub fn merge_json(base: &[u8], extra: &[u8]) -> Result<String, JsError> {
    let bytes = sheltersim_core::interpret::merge_objects(base, extra).map_err(js_error)?;
    String::from_utf8(bytes).map_err(|error| JsError::new(&error.to_string()))
}

/// Whether the server's `safe_document` would redact a string of the JSON document (an API key,
/// a bearer token, an e-mail address or a phone number): a draft holding one is refused (#306).
#[wasm_bindgen(js_name = documentRedacts)]
pub fn document_redacts(document: &[u8]) -> Result<bool, JsError> {
    sheltersim_core::interpret::document_redacts(document).map_err(js_error)
}

/// The text a result question stores of its question, before the route's own IDs are replaced
/// (`sheltersim_core::interpret::question_text`, #308).
#[wasm_bindgen(js_name = questionText)]
pub fn question_text(question: &str) -> Result<String, JsError> {
    sheltersim_core::interpret::question_text(question).map_err(js_error)
}

/// The KPI differences of the frame `other` from the frame `base` in a comparison, as canonical
/// JSON (`sheltersim_core::interpret::kpi_differences`, #307): ints stay ints, floats are Python's.
#[wasm_bindgen(js_name = kpiDifferences)]
pub fn kpi_differences(base: &[u8], other: &[u8]) -> Result<String, JsError> {
    let bytes = sheltersim_core::interpret::kpi_differences(base, other).map_err(js_error)?;
    String::from_utf8(bytes).map_err(|error| JsError::new(&error.to_string()))
}
