//! The core's computing procedure (`sheltersim_core::compute`, #476) for the browser: js/run_store.mjs's
//! `StepHost` does the steps a `Procedure` gives with OPFS (the records through
//! js/project_store.mjs, each piece compressed with CompressionStream before it is written), and
//! gives back what it read or where it wrote. The local and VM servers do the same steps with their
//! files (`sheltersim_store::compute`), so both write the same lines, pieces and records.
//!
//! `next` gives the step as JSON (`Step::to_json`); the bytes of a piece to write and of a line to
//! append are `raw` and `line`. A refusal is thrown as the answers' are (`answers::REFUSED` and the
//! refusal's JSON), any other error with the core's text.

use sheltersim_core::answers::Failure;
use sheltersim_core::compute::{self, ForkRequest, Given, Lineage, Start, Where};
use sheltersim_core::frame;
use wasm_bindgen::prelude::*;

use crate::answers::failed;
use crate::{count_of, js_error, minute_of, saved_json, versions_of};

fn step(result: Result<(), Failure>) -> Result<(), JsError> {
    result.map_err(failed)
}

/// A procedure of the stored Runs (`compute::Procedure`).
#[wasm_bindgen]
pub struct Procedure {
    inner: compute::Procedure,
}

#[wasm_bindgen]
impl Procedure {
    /// Goes on with a branch from its record's head toward `target`.
    #[wasm_bindgen(js_name = goOn)]
    pub fn go_on(project: &str, run: &str, branch: &str, target: f64) -> Result<Procedure, JsError> {
        Ok(Procedure { inner: compute::Procedure::go_on(Where::new(project, run, branch), minute_of(target)?) })
    }

    /// Makes the branch `child` of `parent` at `minute` with the EffectProgram `program` (its JSON
    /// text, approved with `approved_program_hash`), refused where the parent's state there is not
    /// `state_hash`.
    #[allow(clippy::too_many_arguments)]
    pub fn fork(
        project: &str,
        run: &str,
        parent: String,
        child: String,
        minute: f64,
        program: String,
        approved_program_hash: String,
        state_hash: Option<String>,
    ) -> Result<Procedure, JsError> {
        let request = ForkRequest { parent, child, minute: minute_of(minute)?, program, approved_program_hash, state_hash };
        Ok(Procedure { inner: compute::Procedure::fork(project, run, request) })
    }

    /// Keeps a question's turn (its JSON text) after the "end" line of a branch.
    #[wasm_bindgen(js_name = keepTurn)]
    pub fn keep_turn(project: &str, run: &str, branch: &str, turn: Vec<u8>) -> Procedure {
        Procedure { inner: compute::Procedure::keep_turn(Where::new(project, run, branch), turn) }
    }

    /// Starts a branch's stored Run at minute 0 of the exported world `export` (the world `world` of
    /// a file of named worlds), saved every `save_every` minutes, and its record there.
    pub fn start(project: &str, run: &str, branch: &str, export: Vec<u8>, world: Option<String>, save_every: f64) -> Result<Procedure, JsError> {
        let start = Start { export, world, save_every: count_of(save_every)? };
        Ok(Procedure { inner: compute::Procedure::start(Where::new(project, run, branch), start) })
    }

    /// Computes a Run from minute 0 to its end into a branch's files, recording `model_versions`
    /// (JSON; the Run's record's where undefined), with the branch's record where `record` (its
    /// parent and fork minute as given).
    #[allow(clippy::too_many_arguments)]
    #[wasm_bindgen(js_name = fromStart)]
    pub fn from_start(
        project: &str,
        run: &str,
        branch: &str,
        export: Vec<u8>,
        world: Option<String>,
        save_every: f64,
        model_versions: Option<String>,
        record: bool,
        parent_branch_id: Option<String>,
        fork_minute: f64,
    ) -> Result<Procedure, JsError> {
        let start = Start { export, world, save_every: count_of(save_every)? };
        let versions = model_versions.as_deref().map(versions_of).transpose()?;
        let fork_minute = minute_of(fork_minute)?;
        let lineage = record.then_some(Lineage { parent_branch_id, fork_minute });
        Ok(Procedure { inner: compute::Procedure::from_start(Where::new(project, run, branch), start, versions, lineage) })
    }

    /// Recovers the branches left `running` whose lock is free.
    pub fn recover() -> Procedure {
        Procedure { inner: compute::Procedure::recover() }
    }

    /// Sets a branch's status and its Run's.
    #[wasm_bindgen(js_name = setStatus)]
    pub fn set_status(project: &str, run: &str, branch: &str, status: &str) -> Procedure {
        Procedure { inner: compute::Procedure::set_status(Where::new(project, run, branch), status) }
    }

    /// The step the host does next, as JSON.
    pub fn next(&mut self) -> Result<String, JsError> {
        self.inner.next().map(|step| step.to_json()).map_err(failed)
    }

    /// The bytes of the piece `write_piece` asks to write, before compression.
    pub fn raw(&self) -> Vec<u8> {
        self.inner.raw().to_vec()
    }

    /// The line `append_line` asks to append.
    pub fn line(&self) -> Vec<u8> {
        self.inner.line().to_vec()
    }

    /// Bytes read (a record's JSON, a journal, a piece decompressed), or undefined.
    #[wasm_bindgen(js_name = giveText)]
    pub fn give_text(&mut self, bytes: Option<Vec<u8>>) -> Result<(), JsError> {
        step(self.inner.give(Given::Text(bytes)))
    }

    /// Records read, as a JSON list of them.
    #[wasm_bindgen(js_name = giveRecords)]
    pub fn give_records(&mut self, records: &str) -> Result<(), JsError> {
        step(self.inner.give(Given::texts_of_json(records.as_bytes()).map_err(js_error)?))
    }

    /// Ids listed, as a JSON list of strings.
    #[wasm_bindgen(js_name = giveIds)]
    pub fn give_ids(&mut self, ids: &str) -> Result<(), JsError> {
        step(self.inner.give(Given::ids_of_json(ids.as_bytes()).map_err(js_error)?))
    }

    /// Where a piece was written in data.pack and how many bytes it took.
    #[wasm_bindgen(js_name = givePlace)]
    pub fn give_place(&mut self, offset: f64, length: f64) -> Result<(), JsError> {
        step(self.inner.give(Given::Place { offset: count_of(offset)?, length: count_of(length)? }))
    }

    /// Whether the lock was taken.
    #[wasm_bindgen(js_name = giveLock)]
    pub fn give_lock(&mut self, taken: bool) -> Result<(), JsError> {
        step(self.inner.give(Given::Lock(taken)))
    }

    /// The read could not be done (where `mayFail`).
    #[wasm_bindgen(js_name = giveFailed)]
    pub fn give_failed(&mut self) -> Result<(), JsError> {
        step(self.inner.give(Given::Failed))
    }

    /// The step is done.
    pub fn done(&mut self) -> Result<(), JsError> {
        step(self.inner.give(Given::Nothing))
    }

    /// Whether the step given out may be given `giveFailed`.
    #[wasm_bindgen(js_name = mayFail)]
    pub fn may_fail(&self) -> bool {
        self.inner.may_fail()
    }

    /// At `compute`: at most `ticks` ticks, not past the next saved point nor the target: how many
    /// ran.
    pub fn advance(&mut self, ticks: u32) -> Result<u32, JsError> {
        self.inner.advance(u64::from(ticks)).map(|ran| ran as u32).map_err(failed)
    }

    /// At `compute` of `goOn`: ends the computation where it got to.
    pub fn stop(&mut self) -> Result<(), JsError> {
        step(self.inner.stop())
    }

    /// After an error of a computation from the start with its record: whether there are steps
    /// that mark the branch failed.
    pub fn fail(&mut self) -> bool {
        self.inner.fail()
    }

    /// A computation's status once it is done.
    pub fn status(&self) -> Option<String> {
        self.inner.status().map(str::to_owned)
    }

    /// The Run's minute now.
    pub fn minute(&self) -> Result<f64, JsError> {
        let run = self.inner.run().ok_or_else(|| JsError::new("the procedure has no Run"))?;
        Ok(run.minute().map_err(js_error)? as f64)
    }

    /// The minute the Run's clock is complete at.
    pub fn duration(&self) -> Result<f64, JsError> {
        let run = self.inner.run().ok_or_else(|| JsError::new("the procedure has no Run"))?;
        Ok(run.duration().map_err(js_error)? as f64)
    }

    /// The frame of the Run now (`Simulation.frame`).
    pub fn frame(&mut self, run_id: &str, branch_id: &str, computed_until: f64) -> Result<Vec<u8>, JsError> {
        let request = frame::Request { run_id, branch_id, computed_until: minute_of(computed_until)?, state_hash: None };
        let run = self.inner.run_mut().ok_or_else(|| JsError::new("the procedure has no Run"))?;
        run.frame(&request).map_err(js_error)
    }

    /// The digests of the Run's world (`Simulation.digests_json`).
    #[wasm_bindgen(js_name = digestsJson)]
    pub fn digests_json(&self) -> Result<String, JsError> {
        let run = self.inner.run().ok_or_else(|| JsError::new("the procedure has no Run"))?;
        run.run_digests_json().map_err(js_error)
    }

    /// The saved points' lines written, as `RunWriter.savedJson` gives them.
    #[wasm_bindgen(js_name = savedJson)]
    pub fn saved_json(&self) -> String {
        saved_json(self.inner.saved())
    }

    /// The head hash of the last line written.
    #[wasm_bindgen(js_name = lastHead)]
    pub fn last_head(&self) -> Option<String> {
        self.inner.last_head().map(str::to_owned)
    }

    /// The last line written.
    #[wasm_bindgen(js_name = lastLine)]
    pub fn last_line(&self) -> Vec<u8> {
        self.inner.last_line().to_vec()
    }

    /// A fork's receipt (its JSON text) and the child's state hash.
    pub fn receipt(&self) -> String {
        self.inner.receipt().to_owned()
    }

    #[wasm_bindgen(js_name = stateHash)]
    pub fn state_hash(&self) -> String {
        self.inner.state_hash().to_owned()
    }

    /// The branches `recover` made `interrupted`, as a JSON list of "project/run/branch".
    #[wasm_bindgen(js_name = recoveredJson)]
    pub fn recovered_json(&self) -> String {
        self.inner.recovered_json()
    }
}
