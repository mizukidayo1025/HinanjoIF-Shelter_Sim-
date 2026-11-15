//! A Run of an exported world on the core's host (`sheltersim_core::plain::PlainRun`), for
//! Python callers (#368, step 2): the surface the browser's `Simulation` has (`native/wasm/src/lib.rs`),
//! so that the local and VM versions can run their ticks on the same host as the browser.
//!
//! `PlainRun` reads an entry's JSON bytes (what `sheltersim_core::initial::entry` writes, or a
//! fixture of `native/core/tests/data/`), a Scenario's canonical JSON, whose
//! minute-0 world the core builds (`from_scenario`, #429), or one world of a file of named worlds,
//! runs ticks (`advance`), and gives the digests, the metric samples, the frame, the scenario
//! summary, the EffectProgram's check and schedule, the pieces a Run stores and its saved points;
//! `Restore` builds a Run again at a saved point from the stored log and samples.
//!
//! An LLM is given as a link (`set_llm`, #428): a Python object with `offline` (a bool, read once)
//! and `complete(task: bytes) -> bytes`, the JSON of the core's `llm::Task` and `llm::Reply`
//! (`sheltersim.llm.link.RouterLink` puts Python's `LLMRouter` behind it; `core/llm_link.rs` calls
//! it). The core's rules ask it during the ticks; an exception it raises stops the tick, and
//! `advance` raises it as the cause of its `RuntimeError`. `set_llm_replay` answers from a stored tape instead (`llm::Replay`).
//!
//! Each method reads its arguments, calls one function of the core and converts what it gives:
//! bytes as `bytes`, JSON the core wrote as `str`, an error as its Python class
//! (`core::errors::to_py_err`). `advance` raises `RuntimeError` with the browser's text
//! (`plain::Stopped`), caused by the error of the tick or of the end-of-tick check as its own class.
//! A `PlainRun` holds `Rc`s, so it stays on the thread that made it (`unsendable`); it holds the
//! GIL while it runs ticks. The core keeps its feature `parallel` here; the browser's build has it off.

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use sheltersim_core::archive;
use sheltersim_core::error::Error;
use sheltersim_core::frame::Request;
use sheltersim_core::llm::{self, Limits};
use sheltersim_core::plain;

use crate::core::errors::{to_py_err, OrPy};
use crate::core::llm_link::{PyLink, Raised, LINK_RAISED};

/// A Run of an exported world on the core's host.
#[pyclass(unsendable, module = "sheltersim_native.rules.plain")]
pub struct PlainRun {
    run: plain::PlainRun,
    /// What the Python link raised last, the cause `advance` raises.
    raised: Raised,
}

impl PlainRun {
    /// The Python object of a Run: one made here, or one the store restored (`rules::run_files`).
    pub(crate) fn of_run(run: plain::PlainRun) -> Self {
        Self { run, raised: Raised::default() }
    }

    fn limits(limits: Option<&[u8]>) -> PyResult<Limits> {
        limits.map_or(Ok(Limits::default()), |raw| Limits::from_json(raw).or_py())
    }

    /// The core's Run, for the other modules' functions of a Run (`answers`).
    pub(crate) fn core(&self) -> &plain::PlainRun {
        &self.run
    }

    pub(crate) fn core_mut(&mut self) -> &mut plain::PlainRun {
        &mut self.run
    }
}

#[pymethods]
impl PlainRun {
    /// The Run of an exported entry, from its JSON bytes. ValueError for bytes that are not an
    /// exported world.
    #[new]
    fn new(entry: &[u8]) -> PyResult<Self> {
        Ok(Self::of_run(plain::PlainRun::of_json(entry).or_py()?))
    }

    /// The Run of a Scenario from minute 0, from its canonical JSON: the entry the core builds of
    /// its world (`sheltersim_core::initial::entry`, #429), which `sheltersim_native.initial_entry`
    /// gives as bytes. A Scenario the core refuses to build raises the core's error as its Python
    /// class.
    #[staticmethod]
    fn from_scenario(scenario: &[u8]) -> PyResult<Self> {
        let entry = sheltersim_core::initial::entry(scenario).or_py()?;
        Ok(Self::of_run(plain::PlainRun::of_json(&entry).or_py()?))
    }

    /// The Run of the world `name` of a file of named worlds (`{"worlds": {name: entry}}`, as the
    /// fixtures of `native/core/tests/data/` hold them), from the file's JSON bytes.
    #[staticmethod]
    fn from_worlds(file: &[u8], name: &str) -> PyResult<Self> {
        Ok(Self::of_run(plain::PlainRun::of_worlds_json(file, name).or_py()?))
    }

    /// The Run at the saved point `restore` was made of, which has read the stored log and samples
    /// up to its counts, with the exported entry the Run started from (its JSON bytes). `restore` is
    /// used up.
    #[staticmethod]
    fn restore(entry: &[u8], restore: &mut Restore) -> PyResult<Self> {
        let entry = plain::PlainRun::entry_of_json(entry).or_py()?;
        Ok(Self::of_run(plain::PlainRun::restore(entry, restore.inner.take().or_py()?).or_py()?))
    }

    /// `restore` with the world `name` of a file of named worlds as the exported entry.
    #[staticmethod]
    fn restore_from_worlds(file: &[u8], name: &str, restore: &mut Restore) -> PyResult<Self> {
        let entry = plain::PlainRun::entry_of_worlds_json(file, name).or_py()?;
        Ok(Self::of_run(plain::PlainRun::restore(entry, restore.inner.take().or_py()?).or_py()?))
    }

    /// The world's minute: the next tick's.
    fn minute(&self) -> PyResult<i64> {
        self.run.minute().or_py()
    }

    /// The minute the world's clock is complete at.
    fn duration(&self) -> PyResult<i64> {
        self.run.duration().or_py()
    }

    /// Run up to `ticks` ticks, stopping where the clock is complete: how many ran. A tick's failure
    /// or the end-of-tick check's raises RuntimeError (`minute {m}: {step}: {error}`), caused by
    /// that error.
    fn advance(&mut self, py: Python<'_>, ticks: u64) -> PyResult<u64> {
        self.raised.borrow_mut().take();
        self.run.advance(ticks).map_err(|stopped| {
            let raised = self.raised.borrow_mut().take();
            let error = PyRuntimeError::new_err(stopped.to_string());
            match raised {
                Some(raised) if matches!(&stopped.error, Error::Runtime(text) if text == LINK_RAISED) => error.set_cause(py, Some(raised)),
                _ => error.set_cause(py, Some(to_py_err(stopped.error))),
            }
            error
        })
    }

    /// Gives the Run an LLM: `link` (`offline`, a bool read now, and `complete(task: bytes) ->
    /// bytes`), asked within `limits` (the JSON of `llm::Limits`; the model's defaults where None).
    /// ValueError for limits out of range.
    #[pyo3(signature = (link, limits=None))]
    fn set_llm(&mut self, py: Python<'_>, link: Py<PyAny>, limits: Option<&[u8]>) -> PyResult<()> {
        let link = PyLink::new(py, link, &self.raised)?;
        self.run.set_llm(Box::new(link), Self::limits(limits)?).or_py()
    }

    /// Gives the Run an LLM that answers from `entries`, a stored tape's entries from the Run's
    /// minute on (`llm::Replay`), and past them from `then` (a link as `set_llm` takes) or not at
    /// all: a replay never asks a provider. `offline` is whether the Run it replays was.
    #[pyo3(signature = (entries, offline, then=None, limits=None))]
    fn set_llm_replay(&mut self, py: Python<'_>, entries: Vec<Vec<u8>>, offline: bool, then: Option<Py<PyAny>>, limits: Option<&[u8]>) -> PyResult<()> {
        let mut replay = llm::Replay::new(&entries, offline).or_py()?;
        if let Some(link) = then {
            replay = replay.then(Box::new(PyLink::new(py, link, &self.raised)?));
        }
        self.run.set_llm(Box::new(replay), Self::limits(limits)?).or_py()
    }

    /// The LLM tape's entries since the last call, taken out: the JSON bytes of each, in order.
    fn take_tape<'py>(&mut self, py: Python<'py>) -> Vec<Bound<'py, PyBytes>> {
        self.run.take_tape().iter().map(|raw| PyBytes::new(py, raw)).collect()
    }

    /// How many calls the LLM's link answered since it was given.
    fn llm_calls(&self) -> u64 {
        self.run.llm_calls()
    }

    /// The digests of the world's parts (`sheltersim_core::canonical::run_digests`), as a JSON
    /// object in their order.
    fn digests_json(&self) -> PyResult<String> {
        self.run.run_digests_json().or_py()
    }

    /// The digest of each part of the world's canonical document, as a JSON object in the order of
    /// the parts.
    fn part_digests_json(&self) -> PyResult<String> {
        self.run.part_digests_json().or_py()
    }

    /// The digest of the metric samples so far (`sheltersim_core::plain::PlainHost::metrics_digest`).
    fn metrics_digest(&self) -> String {
        self.run.host.metrics_digest()
    }

    /// How many metric samples the Run took.
    fn metric_count(&self) -> usize {
        self.run.host.samples.len()
    }

    /// The metric sample `index` as its JSON list (`MetricSnapshot`'s fields, the minute first);
    /// None past the last.
    fn metric_line<'py>(&self, py: Python<'py>, index: usize) -> PyResult<Option<Bound<'py, PyBytes>>> {
        Ok(self.run.host.samples.line(index).or_py()?.map(|line| PyBytes::new(py, &line)))
    }

    /// The frame of the world now, as its canonical JSON bytes (a `TimelineFrame`); without
    /// `state_hash` the frame's is `state_digest()`.
    #[pyo3(signature = (run_id, branch_id, computed_until, state_hash=None))]
    fn frame<'py>(
        &mut self,
        py: Python<'py>,
        run_id: &str,
        branch_id: &str,
        computed_until: i64,
        state_hash: Option<&str>,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let request = Request { run_id, branch_id, computed_until, state_hash };
        Ok(PyBytes::new(py, &self.run.frame(&request).or_py()?))
    }

    /// The state digest of the world now (`sheltersim_core::plain::PlainRun::state_digest`).
    fn state_digest(&self) -> PyResult<String> {
        self.run.state_digest().or_py()
    }

    /// The summary of the world now that the reading of an intervention's text is given, as its
    /// canonical JSON bytes.
    fn scenario_summary<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        Ok(PyBytes::new(py, &self.run.scenario_summary().or_py()?))
    }

    /// The checks of a program on the world now (`sheltersim_core::program::validate`), for a
    /// program's JSON bytes: the issues as a JSON list of strings. It changes nothing.
    fn validate_effect_program(&mut self, program: &[u8]) -> PyResult<String> {
        self.run.validate_effect_program_json(program).or_py()
    }

    /// A program scheduled on the world now (`sheltersim_core::program::schedule`), for a program's
    /// JSON bytes: the receipt (the `command_log` entry) as JSON.
    fn apply_effect_program(&mut self, program: &[u8]) -> PyResult<String> {
        self.run.apply_effect_program_json(program).or_py()
    }

    /// The log's string and shape tables' sizes: `(strings, shapes)`.
    fn log_table_counts(&self) -> PyResult<(usize, usize)> {
        self.run.log_table_counts().map(|[strings, shapes]| (strings, shapes)).or_py()
    }

    /// The log's tables' entries from `strings` and `shapes` on.
    fn log_tables<'py>(&self, py: Python<'py>, strings: usize, shapes: usize) -> PyResult<Bound<'py, PyBytes>> {
        Ok(PyBytes::new(py, &self.run.log_tables(strings, shapes).or_py()?))
    }

    /// How many of the log's blocks there are, and how many of them are sealed: `(blocks, sealed)`.
    fn log_block_counts(&self) -> PyResult<(usize, usize)> {
        self.run.log_block_counts().map(|[blocks, sealed]| (blocks, sealed)).or_py()
    }

    /// A copy of the log's block `index` (empty past the last).
    fn log_block<'py>(&self, py: Python<'py>, index: usize) -> PyResult<Bound<'py, PyBytes>> {
        Ok(PyBytes::new(py, &self.run.log_block(index).or_py()?))
    }

    /// How many records the log holds.
    fn log_records(&self) -> PyResult<usize> {
        self.run.log_records().or_py()
    }

    /// Whether every record of the log is in its blocks.
    fn log_all_packed(&self) -> PyResult<bool> {
        self.run.log_all_packed().or_py()
    }

    /// The samples' string and shape tables' sizes: `(strings, shapes)`.
    fn sample_table_counts(&self) -> (usize, usize) {
        let [strings, shapes] = self.run.host.samples.table_counts();
        (strings, shapes)
    }

    /// The samples' tables' entries from `strings` and `shapes` on.
    fn sample_tables<'py>(&self, py: Python<'py>, strings: usize, shapes: usize) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.run.host.samples.tables(strings, shapes))
    }

    /// How many of the samples' blocks there are, and how many are sealed: `(blocks, sealed)`.
    fn sample_block_counts(&self) -> (usize, usize) {
        let [blocks, sealed] = self.run.host.samples.block_counts();
        (blocks, sealed)
    }

    /// A copy of the samples' block `index` (empty past the last).
    fn sample_block<'py>(&self, py: Python<'py>, index: usize) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.run.host.samples.block(index))
    }

    /// The world now without its log (`archive::world_parts`).
    fn world_parts<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        Ok(PyBytes::new(py, &self.run.world_parts().or_py()?))
    }

    /// The Run now as a saved point's bytes: call it between calls to `advance`.
    fn saved_point<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        Ok(PyBytes::new(py, &self.run.saved_point().or_py()?.to_bytes()))
    }

    /// Seals the log's and the samples' blocks: call it between calls to `advance`, before a saved
    /// point a stored Run goes on from.
    fn seal_blocks(&mut self) -> PyResult<()> {
        self.run.seal_stores().or_py()
    }
}

/// A saved point being restored (`archive::Restore`): made of the saved point's bytes, then given
/// the stored log's and samples' tables and blocks in the order they were written until it needs
/// no more, then used by `PlainRun.restore`.
#[pyclass(unsendable, module = "sheltersim_native.rules.plain")]
pub struct Restore {
    inner: archive::RestoreOnce,
}

#[pymethods]
impl Restore {
    /// ValueError for bytes that are not a saved point of the version this core reads.
    #[new]
    fn new(point: &[u8]) -> PyResult<Self> {
        Ok(Self { inner: archive::RestoreOnce::new(point).or_py()? })
    }

    /// The saved point's minute.
    fn minute(&self) -> PyResult<i64> {
        Ok(self.inner.get().or_py()?.point.minute)
    }

    /// How many records the log held at the saved point.
    fn records(&self) -> PyResult<u64> {
        Ok(self.inner.get().or_py()?.point.records)
    }

    /// How many metric samples the Run had taken at the saved point.
    fn samples(&self) -> PyResult<u64> {
        Ok(self.inner.get().or_py()?.point.samples)
    }

    fn log_needed(&self) -> PyResult<bool> {
        Ok(self.inner.get().or_py()?.log_needed())
    }

    fn samples_needed(&self) -> PyResult<bool> {
        Ok(self.inner.get().or_py()?.samples_needed())
    }

    fn read_log_tables(&mut self, raw: &[u8]) -> PyResult<()> {
        self.inner.get_mut().or_py()?.read_log_tables(raw).or_py()
    }

    /// How many of the block's records were taken.
    fn read_log_block(&mut self, raw: &[u8]) -> PyResult<usize> {
        self.inner.get_mut().or_py()?.read_log_block(raw).or_py()
    }

    fn read_sample_tables(&mut self, raw: &[u8]) -> PyResult<()> {
        self.inner.get_mut().or_py()?.read_sample_tables(raw).or_py()
    }

    /// How many of the block's samples were taken.
    fn read_sample_block(&mut self, raw: &[u8]) -> PyResult<usize> {
        self.inner.get_mut().or_py()?.read_sample_block(raw).or_py()
    }
}

/// The version of the saved points this core writes and reads (`archive::SAVED_POINT_VERSION`).
#[pyfunction]
fn saved_point_version() -> u64 {
    archive::SAVED_POINT_VERSION
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PlainRun>()?;
    m.add_class::<Restore>()?;
    m.add_function(wrap_pyfunction!(saved_point_version, m)?)?;
    Ok(())
}
