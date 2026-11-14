//! A host without Python (`PlainHost`) and a Run of an exported world on it (`PlainRun`): Issue
//! #244, PR 4-3, for the browser prototype of #132 (stage 4), which runs the ticks of a world Python
//! built and exported (route A). The WASM crate's `Simulation` and, from #368, the PyO3 crate's
//! `sheltersim_native.rules.plain.PlainRun` are its two wrappers; what both hand out is made here.
//!
//! What it does where the native host calls Python (decision 4 of #244, the rest left to stage 6):
//! - The Python-parsed sections of the configs are the exported world's (`load::Sections`), and
//!   the floor plan is built from them with the world's overlay now, again when the overlay's
//!   facility moves or partitions change (`load::FloorPlans`).
//! - A contract (policy E15) answers as the native host's pydantic does (`contracts::validate`,
//!   #346): the model's dump, or the `ValidationError` with pydantic's text.
//! - The whole world's validation (policy E6) is the core's (`validate`, #250), with Python's
//!   classes and texts. Each tick ends as `validate_after_tick` ends it
//!   (`engine::EndCheck::Whole`): the kernel's check with the core's validators of the namespaces
//!   (`rules::tick_checks`), and where it refuses, the validation without the event history.
//! - There is no LLM: the Run runs as one without (`Llm::default()`); no trace goes out
//!   (`TraceLevel::Off`), and no foreign handler runs. No decision record goes out either
//!   (`DecisionSink::Silent`) unless the Run collects them (`PlainRun::collect_decisions`, #368):
//!   then the sink is an indexed one, which keeps the JSON bytes the core writes of each decision
//!   (`Decision::Indexed`), the rows `_IndexedMetricSink.observations` holds as bytes on the native
//!   path (`Decisions`). Collecting changes nothing of the world.
//! - The records of the log are the world's (`load::find_record`). The log holds them in the binary
//!   form (`EventLog::pack_records`, `events::packed`, #252); their JSON is written when asked
//!   for (the digests, `find_record`), the same bytes as the native path's.
//! - The metric samples are kept packed (`MetricSamples`, #252), and their lines, which
//!   `engine::metric_line` writes, are written again when asked for; `metrics_digest` is the digest
//!   `tests/support/core_tick.py` writes of a Run's samples. Where the world has a resource ledger,
//!   each sample also holds its KPI columns then (`Balances`, #266), which the API's KPI history
//!   answers with (`history`).
//! - The core writes `world.resources` and the namespaces it starts itself (decision 2): the host
//!   is only told.
//! - A Run is saved at a minute and restored from there (`saved_point`, `restore`; #251): see
//!   `archive::SavedPoint`.

use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;
use std::sync::Arc;

use serde_json::Value;

use sha2::{Digest, Sha256};

use crate::archive::{self, Restore, SavedPoint};
use crate::canonical::ListNode;
use crate::checkpoint::hex;
use crate::config::{Configs, Filled, SectionId};
use crate::engine::{self, StepError, Stepping};
use crate::error::{Error, Result};
use crate::events::packed::{put_varint, put_zigzag, Blocks, Cursor, Packer};
use crate::events::EventRow;
use crate::geometry::Geo;
use crate::host::{Contract, Decision, DecisionSink, Foreign, Holding, Host, Llm, Phase, Stage, Trace, TraceLevel, TxnKind, NO_FOREIGN, NO_PYTHON_SINK};
use crate::json::Encoder;
use crate::load::{self, FloorPlans, Loaded, Sections};
use crate::rules::action_lifecycle::ActiveActions;
use crate::rules::behavior::Proposal;
use crate::rules::common_operations::Logged;
use crate::rules::decisions::Observed;
use crate::rules::metrics::MetricSample;
use crate::rules::world_view::ViewData;
use crate::state::resource_ledger::NAME as LEDGER;
use crate::validate;
use crate::value::V;
use crate::world::WorldCell;

/// The host of a loaded world without Python.
pub struct PlainHost {
    loaded: Rc<Loaded>,
    sections: Sections,
    plans: FloorPlans,
    /// The metric samples, packed (`MetricSamples`).
    pub samples: MetricSamples,
    /// The decision records, where the Run collects them (`PlainRun::collect_decisions`).
    pub decisions: Option<Decisions>,
}

/// The decision records a Run collects (#368, off unless asked for): the JSON bytes the core
/// writes of each decision for an indexed sink (`Decision::Indexed`, `rules::decisions::decision_bytes`),
/// which are the rows `_IndexedMetricSink.observations` holds as bytes on the native path, in the
/// same order; and how many decisions the core left to a sink in Python (`Sunk::Python`: an
/// observed state longer than the core writes, a household no row reads, ...), whose row,
/// `_IndexedMetricSink.record_decision`'s dict, a host without Python does not write.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Decisions {
    pub records: Vec<Vec<u8>>,
    pub missed: u64,
}

impl PlainHost {
    /// The host of `loaded`, with the sections of the exported entry it was loaded from.
    pub fn new(entry: &Value, loaded: Rc<Loaded>) -> Result<Self> {
        Ok(Self { loaded, sections: Sections::of_entry(entry)?, plans: FloorPlans::default(), samples: MetricSamples::default(), decisions: None })
    }

    /// How many times the floor plan was built.
    pub fn plan_builds(&self) -> usize {
        self.plans.builds()
    }

    /// The sha256 of the metric lines joined by `\n`, in hex (`metrics_digest` of
    /// `tests/support/core_tick.py`).
    pub fn metrics_digest(&self) -> String {
        self.samples.digest()
    }
}

/// The metric samples of a Run in a binary form (#252, PR 6-4): each `MetricSample` and its minute,
/// its fields packed as they are, in blocks (`Blocks`). A sample's line (`engine::metric_line`) is
/// written again from them, byte for byte; the digest of the lines is kept as they are taken, so
/// it needs no line written again. After 7 days of 100 people the lines are 52 MiB of JSON, most of
/// it the IDs behind the counts (`source_ids_json`), and 7.5 MiB packed.
///
/// A sample is: its minute (a zigzag varint), each field in the order of `MetricSample` (a count a
/// varint, `toilet_working_units` a zigzag varint, a float its 8 bytes), and `source_ids_json`:
/// 0 then its JSON packed as a value (`Packer::pack_value`), or 1 then its length and its bytes,
/// for a text the packed value would not write again.
///
/// A sample of a world with a resource ledger (#266) also holds the ledger's KPI columns then
/// (`Balances`, the native path's `resource_snapshots`): it starts with the byte 1 (`WITH_BALANCES`),
/// and the twelve columns follow its last field as zigzag varints. A minute is never negative, so
/// the first byte of a sample without them, its minute's, is even: samples stored before (format v2
/// of `run_store.mjs`) read as they did. A Run's samples all have them or none do, as the native
/// path's recorder requires.
#[derive(Clone, Debug, Default)]
pub struct MetricSamples {
    packer: Packer,
    blocks: Blocks,
    starts: Vec<u64>,
    digest: Sha256,
    /// Whether the samples so far hold the ledger's columns; None before the first.
    balances: Option<bool>,
}

/// The resource ledger's KPI columns of a sample (`resource_metric_values`,
/// `rules::resources::metric_values`): initial, delivered, served, losses, remaining and reserved,
/// of water and then of meals.
pub type Balances = [[i64; 6]; 2];

/// The first byte of a packed sample that holds `Balances`.
const WITH_BALANCES: u8 = 1;

impl MetricSamples {
    /// Takes a sample, with the ledger's columns where the world has a ledger: its line goes into
    /// the digest, and the sample is held packed. An error for a negative minute, and where the
    /// ledger came or went after the first sample (the native path's recorder refuses it too).
    pub fn push(&mut self, sample: &MetricSample, minute: i64, balances: Option<&Balances>) -> Result<()> {
        if minute < 0 {
            return Err(Error::Value("a sample's minute is negative".to_owned()));
        }
        match self.balances {
            Some(true) if balances.is_none() => return Err(Error::unsupported("the ledger was removed")),
            Some(false) if balances.is_some() => return Err(Error::unsupported("the ledger came late")),
            _ => {}
        }
        let line = engine::metric_line(sample, minute)?;
        let mut packed = Vec::with_capacity(256);
        if balances.is_some() {
            packed.push(WITH_BALANCES);
        }
        put_zigzag(&mut packed, minute);
        for count in [sample.toilet_queue_length, sample.toilet_active_users] {
            put_varint(&mut packed, count as u64);
        }
        put_zigzag(&mut packed, sample.toilet_working_units);
        for value in
            [sample.toilet_wait_count, sample.toilet_wait_total_minutes, sample.toilet_wait_mean_minutes, sample.toilet_wait_max_minutes, sample.stress_mean]
        {
            packed.extend_from_slice(&value.to_bits().to_le_bytes());
        }
        for count in [sample.stress_high_count, sample.staff_assigned_task_count, sample.staff_busy_count] {
            put_varint(&mut packed, count as u64);
        }
        packed.extend_from_slice(&sample.staff_load_ratio.to_bits().to_le_bytes());
        put_varint(&mut packed, sample.unresolved_support_request_count as u64);
        self.put_ids(&mut packed, &sample.source_ids_json)?;
        put_varint(&mut packed, sample.support_no_qualified_worker_count as u64);
        for value in [
            sample.support_no_qualified_worker_request_minutes,
            sample.support_fairness_promoted_assignment_count,
            sample.support_wait_staff_busy_request_minutes,
            sample.support_wait_staff_off_shift_request_minutes,
        ] {
            packed.extend_from_slice(&value.to_bits().to_le_bytes());
        }
        for value in balances.into_iter().flatten().flatten() {
            put_zigzag(&mut packed, *value);
        }
        if !self.starts.is_empty() {
            self.digest.update(b"\n");
        }
        self.digest.update(&line);
        self.starts.push(self.blocks.push(&packed));
        self.balances = Some(balances.is_some());
        Ok(())
    }

    /// `source_ids_json` packed as a value when its value writes the same text again.
    fn put_ids(&mut self, out: &mut Vec<u8>, text: &str) -> Result<()> {
        let value = serde_json::from_str::<Value>(text).ok().as_ref().and_then(V::from_json);
        if let Some(value) = value {
            let mut again = Encoder::with_capacity(text.len());
            if value.write_json(&mut again).is_ok() && again.out == text.as_bytes() {
                out.push(0);
                return self.packer.pack_value(out, &value);
            }
        }
        out.push(1);
        put_varint(out, text.len() as u64);
        out.extend_from_slice(text.as_bytes());
        Ok(())
    }

    /// How many samples were taken.
    pub fn len(&self) -> usize {
        self.starts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.starts.is_empty()
    }

    /// The bytes the packed samples take.
    pub fn size(&self) -> usize {
        self.blocks.size()
    }

    /// The sample `index`, its minute and the ledger's columns it holds, or None past the last.
    pub fn sample(&self, index: usize) -> Result<Option<(MetricSample, i64, Option<Balances>)>> {
        let Some(&start) = self.starts.get(index) else {
            return Ok(None);
        };
        let unpacked = unpack_sample(&self.packer, self.blocks.at(start))?;
        Ok(Some((unpacked.sample, unpacked.minute, unpacked.balances)))
    }

    /// Seals the samples' blocks (`Blocks::seal`).
    pub fn seal(&mut self) {
        self.blocks.seal();
    }

    /// The packer and the blocks of the samples: what a Run in the browser stores (`archive`).
    pub fn store(&self) -> (&Packer, &Blocks) {
        (&self.packer, &self.blocks)
    }

    /// The sizes of the samples' string and shape tables: `[strings, shapes]`.
    pub fn table_counts(&self) -> [usize; 2] {
        [self.packer.strings().len(), self.packer.shapes().len()]
    }

    /// The tables' entries from `strings` and `shapes` on (`Packer::write_tables`).
    pub fn tables(&self, strings: usize, shapes: usize) -> Vec<u8> {
        let mut out = Vec::new();
        self.packer.write_tables(strings, shapes, &mut out);
        out
    }

    /// How many blocks there are, and how many of them are sealed: `[blocks, sealed]`.
    pub fn block_counts(&self) -> [usize; 2] {
        [self.blocks.blocks().len(), self.blocks.sealed()]
    }

    /// A copy of the block `index` (empty past the last).
    pub fn block(&self, index: usize) -> Vec<u8> {
        self.blocks.blocks().get(index).cloned().unwrap_or_default()
    }

    /// The line of the sample `index` (`engine::metric_line`), or None past the last.
    pub fn line(&self, index: usize) -> Result<Option<Vec<u8>>> {
        self.sample(index)?.map(|(sample, minute, _)| engine::metric_line(&sample, minute)).transpose()
    }

    /// Samples already packed against `packer` in `blocks`, each at its start in `starts`, with
    /// `digest` of their lines so far, and whether they hold the ledger's columns (a stored Run read
    /// back, `archive::Restore`, #251).
    pub(crate) fn adopt(packer: Packer, blocks: Blocks, starts: Vec<u64>, digest: Sha256, balances: Option<bool>) -> Self {
        Self { packer, blocks, starts, digest, balances }
    }

    /// The sha256 of the lines so far joined by `\n`, in hex.
    pub fn digest(&self) -> String {
        hex(&self.digest.clone().finalize())
    }
}

/// A sample read back (`unpack_sample`).
#[derive(Clone, Debug, PartialEq)]
pub struct UnpackedSample {
    pub sample: MetricSample,
    pub minute: i64,
    /// The ledger's columns, where the sample holds them.
    pub balances: Option<Balances>,
    /// How many bytes the sample took.
    pub used: usize,
}

/// The sample `MetricSamples` packed at the start of `bytes` against `packer`'s tables. An error,
/// never a panic, for bytes that are not such a sample.
pub fn unpack_sample(packer: &Packer, bytes: &[u8]) -> Result<UnpackedSample> {
    let bad = |what: &str| Error::Value(format!("not a packed sample: {what}"));
    let mut cursor = Cursor::new(bytes);
    let count = |cursor: &mut Cursor<'_>| -> Result<usize> { usize::try_from(cursor.varint()?).map_err(|_| bad("a count")) };
    let with_balances = match bytes.first() {
        Some(&WITH_BALANCES) => {
            cursor.skip(1)?;
            true
        }
        Some(first) if first & 1 == 1 => return Err(bad("its form")),
        _ => false,
    };
    let minute = cursor.zigzag()?;
    if minute < 0 {
        return Err(bad("its minute"));
    }
    let toilet_queue_length = count(&mut cursor)?;
    let toilet_active_users = count(&mut cursor)?;
    let toilet_working_units = cursor.zigzag()?;
    let toilet_wait_count = cursor.float()?;
    let toilet_wait_total_minutes = cursor.float()?;
    let toilet_wait_mean_minutes = cursor.float()?;
    let toilet_wait_max_minutes = cursor.float()?;
    let stress_mean = cursor.float()?;
    let stress_high_count = count(&mut cursor)?;
    let staff_assigned_task_count = count(&mut cursor)?;
    let staff_busy_count = count(&mut cursor)?;
    let staff_load_ratio = cursor.float()?;
    let unresolved_support_request_count = count(&mut cursor)?;
    let source_ids_json = match cursor.byte()? {
        0 => {
            let (value, used) = packer.unpack_value(cursor.rest())?;
            cursor.skip(used)?;
            let mut text = Encoder::with_capacity(1024);
            value.write_json(&mut text)?;
            String::from_utf8(text.out).map_err(|_| bad("its IDs"))?
        }
        1 => {
            let length = usize::try_from(cursor.varint()?).map_err(|_| bad("a length"))?;
            String::from_utf8(cursor.take(length)?.to_vec()).map_err(|_| bad("its IDs"))?
        }
        _ => return Err(bad("its IDs")),
    };
    let support_no_qualified_worker_count = count(&mut cursor)?;
    let sample = MetricSample {
        toilet_queue_length,
        toilet_active_users,
        toilet_working_units,
        toilet_wait_count,
        toilet_wait_total_minutes,
        toilet_wait_mean_minutes,
        toilet_wait_max_minutes,
        stress_mean,
        stress_high_count,
        staff_assigned_task_count,
        staff_busy_count,
        staff_load_ratio,
        unresolved_support_request_count,
        source_ids_json,
        support_no_qualified_worker_count,
        support_no_qualified_worker_request_minutes: cursor.float()?,
        support_fairness_promoted_assignment_count: cursor.float()?,
        support_wait_staff_busy_request_minutes: cursor.float()?,
        support_wait_staff_off_shift_request_minutes: cursor.float()?,
    };
    let balances = if with_balances {
        let mut balances: Balances = [[0; 6]; 2];
        for value in balances.iter_mut().flatten() {
            *value = cursor.zigzag()?;
        }
        Some(balances)
    } else {
        None
    };
    Ok(UnpackedSample { sample, minute, balances, used: cursor.position() })
}

impl Host for PlainHost {
    type Error = Error;

    fn validate_candidate(&mut self, _kind: TxnKind, stage: Stage) -> Result<()> {
        self.loaded.cell.with(|world| match stage {
            Stage::World => validate::world(world, &validate::Options::default()),
            Stage::Candidate => validate::candidate(world),
        })?
    }

    fn validate_world(&mut self) -> Result<()> {
        self.loaded.cell.with(|world| validate::world(world, &validate::Options::default()))?
    }

    fn validate_extensions(&mut self) -> Result<()> {
        self.loaded.cell.with(|world| validate::extensions(world, true))?
    }

    fn validate_in_flight(&mut self, event: &EventRow) -> Result<()> {
        self.loaded.cell.with(|world| validate::world(world, &validate::Options { history: true, in_flight: Some(event) }))?
    }

    fn find_record(&mut self, event_id: &str) -> Result<Option<Logged>> {
        self.loaded.cell.with(|world| load::find_record(world, event_id))?
    }

    fn floor_plan(&mut self) -> Result<Arc<Geo>> {
        let overlay = self.loaded.cell.with(|world| load::overlay(world))??;
        self.plans.plan(&self.sections, overlay.as_ref())
    }

    fn contract(&mut self, which: &Contract, input: &V) -> Result<V> {
        crate::contracts::validate(which, input)
    }

    fn llm(&self) -> Result<Llm> {
        Ok(Llm::default())
    }

    fn prioritize_support_request(&mut self, _request_id: &str, _agent_id: &str, _observed_state_hash: &str) -> Result<()> {
        Ok(())
    }

    fn consider_support_request(&mut self, _agent_id: &str, _request_id: &str) -> Result<()> {
        Ok(())
    }

    fn consider_complaint_response(&mut self, _agent_id: &str, _request_id: &str) -> Result<()> {
        Ok(())
    }

    fn consider_cooperation(&mut self, _initiator: &str, _participant: &str, _trigger_event_id: &str) -> Result<()> {
        Ok(())
    }

    fn consider_information_reaction(&mut self, _agent_id: &str, _participant: &str, _message: &str, _trigger_event_id: &str) -> Result<()> {
        Ok(())
    }

    /// The sample, with the ledger's columns where the world has a ledger (the native path's
    /// `resource_snapshots`, #266), read as the native host reads them: after the sample, with the
    /// world as the sample saw it.
    fn capture_metric(&mut self, sample: &MetricSample, minute: i64) -> Result<()> {
        let balances = self.loaded.cell.with(|world| -> Result<Option<Balances>> {
            if Holding::of_world(world)?.is_absent(LEDGER).map_err(Error::unsupported)? {
                return Ok(None);
            }
            crate::rules::resources::metric_values(world).map(Some)
        })??;
        self.samples.push(sample, minute, balances.as_ref())
    }

    /// An indexed sink where the Run collects its decisions, else a silent one.
    fn decision_sink(&self) -> Result<DecisionSink> {
        Ok(if self.decisions.is_some() { DecisionSink::Indexed } else { DecisionSink::Silent })
    }

    /// The record's bytes, kept. An indexed sink is given no other kind of record, and a silent
    /// one none.
    fn decision(&mut self, decision: Decision) -> Result<()> {
        if let (Some(decisions), Decision::Indexed(raw)) = (&mut self.decisions, decision) {
            decisions.records.push(raw);
        }
        Ok(())
    }

    /// `policy_sources.get(policy_version)` of `_IndexedMetricSink`: the assumption source of the
    /// Model Catalog's model of that ID (`validate::catalog`), None for a version no model has.
    fn decision_source(&mut self, policy_version: &str) -> Result<Option<Option<Arc<str>>>> {
        let catalog = crate::validate::catalog::catalog()?;
        Ok(Some(catalog.models.get(policy_version).map(|model| model.assumption_source_id.clone())))
    }

    /// A decision the core leaves to a sink in Python. A collecting Run counts it (`Decisions`) and
    /// goes on, as a silent sink would, so that collecting changes nothing; a decision due no later
    /// than its own minute is refused whether or not the Run collects, as the silent sink refuses it.
    fn record_decision_python(
        &mut self,
        data: &ViewData,
        _proposal: &Proposal,
        _target_id: Option<&str>,
        next_decision_at: i64,
        _observed: &Observed<'_>,
    ) -> Result<()> {
        match &mut self.decisions {
            Some(decisions) if next_decision_at > data.minute_index => {
                decisions.missed += 1;
                Ok(())
            }
            _ => Err(Error::unsupported(NO_PYTHON_SINK)),
        }
    }

    fn trace_level(&self) -> Result<TraceLevel> {
        Ok(TraceLevel::Off)
    }

    fn trace(&mut self, _document: Trace) -> Result<()> {
        Ok(())
    }

    fn phase(&mut self, _phase: Phase, _before: bool) -> Result<()> {
        Ok(())
    }

    fn space_refreshed(&mut self) -> Result<()> {
        Ok(())
    }

    fn fill_section(&mut self, configs: &Arc<Configs>, which: SectionId) -> Result<Filled> {
        if !self.sections.fill(configs, which)? {
            return Err(Error::unsupported(format!("the export has no section {}", which.name())));
        }
        Ok(Filled::Kept)
    }

    fn apply_foreign(&mut self, _event: &EventRow) -> Result<Foreign> {
        Err(Error::unsupported(NO_FOREIGN))
    }
}

/// A Run of an exported world on a `PlainHost`: the ticks one at a time (`step`), the world's
/// digests and the samples' after any of them.
pub struct PlainRun {
    pub loaded: Rc<Loaded>,
    pub host: PlainHost,
    pub stepping: Stepping,
    /// The world document's `rng_state`, which the core's World does not hold: what the world's
    /// digest reads (`frame`).
    pub rng_state: V,
    /// The node of the log's records the last digest read (`canonical::state_digest_with`, #266):
    /// the next digest reads only the records added since. Built at the first digest.
    log_node: RefCell<ListNode>,
}

impl PlainRun {
    /// The Run of an exported entry (`export_world`'s document).
    pub fn of_entry(entry: &Value) -> Result<Self> {
        let loaded = Rc::new(load::loaded(entry)?);
        // The log holds its records in the binary form (#252): about a seventh of their JSON.
        loaded.cell.with(|world| world.log_mut().and_then(|mut log| log.pack_records()))??;
        let host = PlainHost::new(entry, Rc::clone(&loaded))?;
        let rng_state = load::value(&entry["world"]["rng_state"])?;
        Ok(Self { loaded, host, stepping: Stepping::whole(), rng_state, log_node: RefCell::default() })
    }

    /// `of_entry` of the entry's JSON text.
    pub fn of_json(raw: &[u8]) -> Result<Self> {
        Self::of_entry(&Self::entry_of_json(raw)?)
    }

    /// `of_entry` of the world `name` of a file of named worlds (`{"worlds": {name: entry}}`, as
    /// `benchmarks/core_fixtures.py` writes `native/core/tests/data/`), from its JSON text: the
    /// entry is read from the file's own bytes, its numbers as written (a JSON reader of another
    /// language may write `1.0` back as `1`).
    pub fn of_worlds_json(raw: &[u8], name: &str) -> Result<Self> {
        Self::of_entry(&Self::entry_of_worlds_json(raw, name)?)
    }

    /// The world's minute: the next tick's.
    pub fn minute(&self) -> Result<i64> {
        self.loaded.cell.with(|world| world.minute_index())?
    }

    /// The world's duration: its clock is complete at this minute.
    pub fn duration(&self) -> Result<i64> {
        self.loaded.cell.with(|world| world.duration_minutes())?
    }

    /// One tick (`engine::step`): the minute it ran at.
    pub fn step(&mut self) -> std::result::Result<i64, StepError<Error>> {
        engine::step(&self.loaded, &mut self.host, &mut self.stepping)
    }

    /// Up to `ticks` ticks, stopping where the clock is complete: how many ran. A tick's failure or
    /// the end-of-tick check's stops it (`Stopped`, with the ticks that ran before).
    pub fn advance(&mut self, ticks: u64) -> std::result::Result<u64, Stopped> {
        let mut ran = 0;
        while ran < ticks {
            match self.step() {
                Ok(_) => ran += 1,
                Err(StepError::Complete) => break,
                Err(StepError::Tick { minute, label, error }) => return Err(Stopped { ran, minute, at: At::Tick(label), error }),
                Err(StepError::Check { minute, error }) => return Err(Stopped { ran, minute, at: At::Check, error }),
            }
        }
        Ok(ran)
    }

    /// Whether the Run collects its decisions' records (#368): on, the host is an indexed sink and
    /// keeps them (`Decisions`); off (the default), it is a silent one, and what it kept and was
    /// not taken is dropped. The world is the same either way.
    pub fn collect_decisions(&mut self, on: bool) {
        match (on, self.host.decisions.is_some()) {
            (true, false) => self.host.decisions = Some(Decisions::default()),
            (false, _) => self.host.decisions = None,
            (true, true) => {}
        }
    }

    /// The decision records kept since the last call, taken out (none where the Run does not
    /// collect them).
    pub fn take_decisions(&mut self) -> Vec<Vec<u8>> {
        self.host.decisions.as_mut().map(|decisions| std::mem::take(&mut decisions.records)).unwrap_or_default()
    }

    /// How many decisions since the Run began to collect them were left to a sink in Python
    /// (`Decisions::missed`).
    pub fn decisions_missed(&self) -> u64 {
        self.host.decisions.as_ref().map_or(0, |decisions| decisions.missed)
    }

    /// The digest of each part of the world's canonical document (`canonical::digests`).
    pub fn digests(&self) -> Result<Vec<(&'static str, String)>> {
        self.loaded.cell.with(|world| crate::canonical::digests(world))?
    }

    /// `digests` as a JSON object in the order of the parts: what a stored Run's world piece and
    /// log read back to.
    pub fn part_digests_json(&self) -> Result<String> {
        utf8(crate::canonical::digests_json(&self.digests()?))
    }

    /// The digests of the world as `tests/support/core_tick.py` writes them
    /// (`canonical::run_digests`), as a JSON object in its order.
    pub fn run_digests_json(&self) -> Result<String> {
        let digests = self.loaded.cell.with(|world| crate::canonical::run_digests(world))??;
        utf8(crate::canonical::digests_json(&digests))
    }

    /// The frame of the world now (`frame::frame`, #264): its canonical JSON, what the API's
    /// timeline gives for this minute. It changes nothing.
    /// Without the request's `state_hash`, the frame's is `state_digest`.
    pub fn frame(&mut self, request: &crate::frame::Request<'_>) -> Result<Vec<u8>> {
        let digest = match request.state_hash {
            Some(_) => None,
            None => Some(self.state_digest()?),
        };
        let request = crate::frame::Request { state_hash: request.state_hash.or(digest.as_deref()), ..*request };
        crate::frame::frame(&self.loaded.cell, &mut self.host, &request, &self.rng_state)
    }

    /// The summary of the world now that the reading of an intervention's text is given
    /// (`summary::scenario_summary`, #302): its canonical JSON. It changes nothing.
    pub fn scenario_summary(&self) -> Result<Vec<u8>> {
        self.loaded.cell.with(|world| crate::summary::scenario_summary(world))?
    }

    /// `validate_effect_program(world, program)` on the world now (`program::validate`, #304): the
    /// issues, in Python's order and texts. It changes nothing: the toilets an addition would place
    /// are placed on a copy of the world, on the floor plan of that copy.
    pub fn validate_effect_program(&mut self, program: &crate::program::EffectProgram) -> Result<Vec<String>> {
        let enabled = self.placement_enabled()?;
        let host = &mut self.host;
        let mut plan = |world: &crate::world::World| -> Result<Arc<Geo>> {
            let overlay = load::overlay(world)?;
            host.plans.plan(&host.sections, overlay.as_ref())
        };
        let mut placement = crate::program::Placement { enabled, plan: &mut plan };
        self.loaded.cell.with(|world| crate::program::validate(world, program, &mut placement))?
    }

    /// `schedule_effect_program(world, program)` on the world now, as a fork's
    /// `BranchRuntime.apply_effect_program` calls it (`program::schedule`, #305): the program's
    /// record and events are written, all or none, and the receipt says what was scheduled.
    pub fn apply_effect_program(&mut self, program: &crate::program::EffectProgram) -> Result<crate::program::Receipt> {
        let enabled = self.placement_enabled()?;
        let host = &mut self.host;
        let mut plan = |world: &crate::world::World| -> Result<Arc<Geo>> {
            let overlay = load::overlay(world)?;
            host.plans.plan(&host.sections, overlay.as_ref())
        };
        let mut placement = crate::program::Placement { enabled, plan: &mut plan };
        self.loaded.cell.with(|world| crate::program::schedule(world, program, &mut placement))?
    }

    /// `validate_effect_program` of a program's JSON bytes (`EffectProgram::of_json`): the issues as
    /// a JSON list of strings. An error for a program the contract refuses or the core does not
    /// check.
    pub fn validate_effect_program_json(&mut self, program: &[u8]) -> Result<String> {
        let program = crate::program::EffectProgram::of_json(program)?;
        let issues = self.validate_effect_program(&program)?;
        utf8(V::List(issues.into_iter().map(|issue| V::Str(issue.into())).collect()).to_json()?)
    }

    /// `apply_effect_program` of a program's JSON bytes: the receipt's `command_log` entry as JSON
    /// (`{"command": "effect_program", "program_id", "program_hash", "event_ids", "minute"}`).
    pub fn apply_effect_program_json(&mut self, program: &[u8]) -> Result<String> {
        let program = crate::program::EffectProgram::of_json(program)?;
        let receipt = self.apply_effect_program(&program)?;
        utf8(receipt.command().to_json()?)
    }

    /// `installed_facility_placement_enabled(world)`.
    fn placement_enabled(&mut self) -> Result<bool> {
        let holding = crate::rules::kit::holding(&self.loaded.cell, &mut self.host)?;
        let schema = crate::rules::kit::kept_section_value(
            &self.loaded.cell,
            &mut self.host,
            &holding,
            SectionId::OverlaySchema,
            |configs| &configs.overlay_schema,
            crate::config::overlay::overlay_schema,
        )?;
        let overlay = holding.held(crate::rules::overlay::OVERLAY_KEY, "the overlay is not held as its row");
        crate::rules::overlay::placement_enabled(&schema, &overlay)
    }

    /// `world.state_digest()` (`canonical::state_digest`) of the world now. The log's node is kept
    /// between calls, so a call reads only the records added since the last
    /// (`canonical::state_digest_with`).
    pub fn state_digest(&self) -> Result<String> {
        let mut log = self.log_node.try_borrow_mut().map_err(|_| Error::unsupported("the log's node is borrowed"))?;
        self.loaded.cell.with(|world| crate::canonical::state_digest_with(world, &self.rng_state, &mut log))?
    }

    /// Seals the blocks of the packed log and of the samples (`Blocks::seal`), between two ticks:
    /// the records and samples taken so far are then all in sealed blocks, which a Run in the
    /// browser stores before a saved point, so that the point can be restored from what is stored
    /// up to it (#267). The records, the samples and every digest stay as they were.
    pub fn seal_stores(&mut self) -> Result<()> {
        self.loaded.cell.with(|world| world.log_mut().map(|mut log| log.seal_packed()))??;
        self.host.samples.seal();
        Ok(())
    }

    /// The packed log's packer and blocks (`EventLog::packed_store`), given to `f`.
    fn log_store<R>(&self, f: impl FnOnce(&Packer, &Blocks) -> R) -> Result<R> {
        self.loaded.cell.with(|world| -> Result<R> {
            let log = world.log()?;
            let (packer, blocks) = log.packed_store().ok_or_else(|| Error::unsupported("the log is not packed"))?;
            Ok(f(packer, blocks))
        })?
    }

    /// The sizes of the log's string and shape tables: `[strings, shapes]`.
    pub fn log_table_counts(&self) -> Result<[usize; 2]> {
        self.log_store(|packer, _| [packer.strings().len(), packer.shapes().len()])
    }

    /// The log's tables' entries from `strings` and `shapes` on (`Packer::write_tables`).
    pub fn log_tables(&self, strings: usize, shapes: usize) -> Result<Vec<u8>> {
        self.log_store(|packer, _| {
            let mut out = Vec::new();
            packer.write_tables(strings, shapes, &mut out);
            out
        })
    }

    /// How many of the log's blocks there are, and how many of them are sealed: `[blocks, sealed]`.
    pub fn log_block_counts(&self) -> Result<[usize; 2]> {
        self.log_store(|_, blocks| [blocks.blocks().len(), blocks.sealed()])
    }

    /// A copy of the log's block `index` (empty past the last).
    pub fn log_block(&self, index: usize) -> Result<Vec<u8>> {
        self.log_store(|_, blocks| blocks.blocks().get(index).cloned().unwrap_or_default())
    }

    /// How many records the log holds.
    pub fn log_records(&self) -> Result<usize> {
        self.loaded.cell.with(|world| world.log().map(|log| log.len()))?
    }

    /// Whether every record of the log is in its blocks: none kept its bytes instead, so the
    /// stored blocks hold the whole log.
    pub fn log_all_packed(&self) -> Result<bool> {
        self.loaded.cell.with(|world| world.log().map(|log| log.all_packed()))?
    }

    /// The world now without its log (`archive::world_parts`).
    pub fn world_parts(&self) -> Result<Vec<u8>> {
        self.loaded.cell.with(|world| archive::world_parts(world))?
    }

    /// The Run now as a saved point (`archive::SavedPoint`, #251), between two ticks.
    pub fn saved_point(&self) -> Result<SavedPoint> {
        let (world, records) = self.loaded.cell.with(|world| -> Result<_> { Ok((archive::world_parts(world)?, world.log()?.len() as u64)) })??;
        Ok(SavedPoint { minute: self.minute()?, records, samples: self.host.samples.len() as u64, world })
    }

    /// The Run at a saved point (#251): its world, the log and the samples `restore` took, and
    /// the exported `entry` the Run started from, whose controller (`run`), slots and sections it
    /// runs with (its world is not read). It goes on as the Run went on from the saved point. An
    /// error where `restore` holds fewer records or samples than the saved point, or the world is
    /// not of the saved point's minute.
    pub fn restore(mut entry: Value, restore: Restore) -> Result<Self> {
        let (point, log, samples) = restore.into_parts()?;
        // The world document's `rng_state` is the Run's from its start (the algorithm and the seed).
        let rng_state = load::value(&entry["world"]["rng_state"])?;
        let entry_object = entry.as_object_mut().ok_or_else(|| Error::Value("not an exported world: the entry is not a dict".to_owned()))?;
        entry_object.insert("world".to_owned(), Value::Object(archive::world_document(&point.world)?));
        let loaded = Rc::new(load::loaded(&entry)?);
        loaded.cell.with(|world| -> Result<()> {
            if world.minute_index()? != point.minute {
                return Err(Error::Value("not a saved point: its world is of another minute".to_owned()));
            }
            *world.log_mut()? = log;
            Ok(())
        })??;
        let actions = loaded.cell.with(|world| world.log().and_then(|log| ActiveActions::of_log(&log)))??;
        loaded.run.try_borrow_mut().map_err(|_| Error::unsupported("the Run is borrowed"))?.actions = actions;
        let mut host = PlainHost::new(&entry, Rc::clone(&loaded))?;
        host.samples = samples;
        Ok(Self { loaded, host, stepping: Stepping::whole(), rng_state, log_node: RefCell::default() })
    }

    /// The exported entry of `of_json`'s JSON text, for `restore`.
    pub fn entry_of_json(raw: &[u8]) -> Result<Value> {
        serde_json::from_slice(raw).map_err(|error| Error::Value(format!("not an exported world: {error}")))
    }

    /// The entry `name` of `of_worlds_json`'s file of named worlds, for `restore`.
    pub fn entry_of_worlds_json(raw: &[u8], name: &str) -> Result<Value> {
        let mut file: Value = serde_json::from_slice(raw).map_err(|error| Error::Value(format!("not a file of worlds: {error}")))?;
        let entry = file.get_mut("worlds").and_then(|worlds| worlds.get_mut(name)).ok_or_else(|| Error::Value(format!("the file has no world {name}")))?;
        Ok(entry.take())
    }
}

/// Where an `advance` stopped (`Stopped`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum At {
    /// The tick, at the step of this label (`tick::Failed`), or None.
    Tick(Option<&'static str>),
    /// The end-of-tick check.
    Check,
}

/// Why `PlainRun::advance` stopped before its ticks ran out: a tick's failure or the end-of-tick
/// check's, at its minute. Its text (`Display`) is what the browser's `Simulation.advance` throws
/// and the PyO3 crate's `PlainRun.advance` raises: `minute {minute}: {the step}: {error}`.
#[derive(Clone, Debug, PartialEq)]
pub struct Stopped {
    /// How many ticks ran before.
    pub ran: u64,
    pub minute: i64,
    pub at: At,
    pub error: Error,
}

impl fmt::Display for Stopped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let at = match self.at {
            At::Tick(label) => label.unwrap_or("the tick"),
            At::Check => "the end-of-tick check",
        };
        write!(f, "minute {}: {at}: {}", self.minute, self.error)
    }
}

impl std::error::Error for Stopped {}

/// JSON the core wrote, as a `String`.
fn utf8(raw: Vec<u8>) -> Result<String> {
    String::from_utf8(raw).map_err(|_| Error::Value("the JSON is not UTF-8".to_owned()))
}
