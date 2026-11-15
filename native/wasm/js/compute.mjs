// Computing a Run in the browser (Issue #267, stage B of #263): the browser's answers to
// `POST runs/{run}/resume` and `POST runs/{run}/cancel` (js/browser_api.mjs routes them here), and
// what a page closed in the middle leaves. A Run to compute is made from a Scenario
// (js/scenario_routes.mjs, `POST runs`); the Run the site offered to compute beside the sample
// (#267) is gone since #411, and the tests make theirs with native/wasm/tests/compute_run.mjs.
//
// A branch's head. A branch's record (`computed_until`, `checkpoint_hash`) names a line of its
// stored Run's journal the Run can be read back at and gone on from: the "export" line (minute 0),
// a "saved" line, or the "end" line. Its `checkpoint_hash` is that line's head hash (the core's
// `run_store::Line::head_json`; project_store.mjs: the "end" line's for a Run stored to its end).
// The record is written after the line is flushed, so the record is the commit: lines after the
// head are a computation that did not reach its next head, and going on cuts them off first, unless
// a branch forked from it reads them (it reads its parent's pieces up to its base: the core refuses
// the cut).
//
// A Run computed with other model versions is view-only (the core's `model_version`, #368): its
// stored Run's versions are compared with the ones the core records now for its Scenario
// (`modelVersions`, or only the engine's items for a Scenario the core cannot sign), and resume,
// intervention-preview and intervention-apply refuse it with the server's 409 `run_model_view_only`.
// Its frames show the saved point at or before the minute asked for (browser_api.mjs, D2 of the
// model versions).
//
// Computing (Computing.resume). The server's checks and answers (src/sheltersim/web/projects.py
// `submit`, `cancel`), one computation at a time (the thread worker's max_jobs). The procedure is
// the core's (`sheltersim_core::compute`, WASM's `Procedure.goOn`, #476), whose steps
// run_store.mjs's `StepHost` does with OPFS, as the local and VM servers do them with their files:
// the branch is cut back to its head and restored there (the exported world at minute 0), and
// computed in chunks of at most `chunkTicks` ticks. Each
// chunk (its ticks, then its steps) is a task of the store's queue (the requests' queue of
// browser_api.mjs, inside its Web Lock), so the Worker answers timeline, history and the records
// between chunks. At every `save_every` minutes (360, #263), at the target and when cancelled, the
// blocks are sealed and a saved point is stored: the record then moves its head there, the frame
// of that minute is kept for the screen's next `timeline` of it, and the Run's records stay
// `running` until the computation ends: `completed` at the end of the clock (with the "world" and
// "end" lines), `paused` at the target or when cancelled, `failed` on an error. The OPFS files are
// opened for each chunk only: a file has one synchronous access handle at a time, and a read
// between chunks opens its own.
//
// A page closed in the middle (recoverInterrupted): the Worker dies with the page; the record
// keeps the last head, the journal may hold lines after it (or a line cut short, which reading
// ignores). On the next start a branch left `running` whose lock no Worker holds becomes
// `interrupted`, as the server's start does (the core's `Procedure.recover`); resuming it cuts
// the lines after its head off.
// The lock of a branch (`branchLock`, a Web Lock in the page) is held while it computes, so
// another tab neither computes it at the same time nor calls it interrupted.

import { ApiError } from "./api_answers.mjs";
import * as store from "./project_store.mjs";

/// Ticks per chunk by default: about a tenth of a second for 100 people.
export const CHUNK_TICKS = 60;
const JOURNAL = "journal.jsonl";
const decoder = new TextDecoder();
const refused = (message) => new store.ProjectStoreError(message);
/// The status of a branch and its Run while it computes.
const RUNNING = "running";

/// web/projects.py's `fields`: the document has the required keys and no others but the optional
/// ones.
export function checkFields(document, required, optional = []) {
  const keys = Object.keys(document);
  if (keys.some((key) => !required.includes(key) && !optional.includes(key)) || required.some((key) => !keys.includes(key))) {
    throw refused("request fields do not match this operation");
  }
}

/// A POST's body as Python's server reads it: an error for an empty body (409), for one that is
/// not JSON (400) and for JSON that is not an object (409).
export function postDocument(body) {
  if (body === null || body === undefined || body === "") throw refused("project request exceeds the body limit");
  let document;
  try {
    document = JSON.parse(body);
  } catch {
    throw new ApiError(400, "invalid project request");
  }
  if (document === null || typeof document !== "object" || Array.isArray(document)) throw refused("project request must be an object");
  return document;
}

// ---- Journals and heads ------------------------------------------------------------------------

/// The lines of a branch's stored Run its head may be (the core's `Journal::heads`), in order:
/// `{ kind, minute, index, head }`. A journal whose lines cannot be read has none.
export async function readHeads(wasm, backend) {
  const file = await backend.open(JOURNAL);
  let journal;
  try {
    journal = new wasm.Journal(file.read(0, file.size()));
  } catch {
    return [];
  } finally {
    file.close();
  }
  try {
    return JSON.parse(journal.headsJson());
  } finally {
    journal.free();
  }
}

/// A branch left `running` that no Worker computes (its lock is free: `branchLock(name)` gives a
/// release) was interrupted: its record and its Run's become `interrupted` (the server's
/// `recover_interrupted` at its start; the core's `Procedure.recover`), its head kept.
/// Every Project and Run is looked at; one whose Runs or branches cannot be listed is left as it
/// is. Returns the branches recovered, as "project/run/branch".
export async function recoverInterrupted({ wasm, tree, branchLock }) {
  const procedure = wasm.Procedure.recover();
  try {
    await store.stepHost(wasm, tree, { branchLock, lockName }).finish(procedure);
    return JSON.parse(procedure.recoveredJson());
  } finally {
    procedure.free();
  }
}

/// The name of a branch's lock (a Web Lock in the page).
export const lockName = (projectId, runId, branchId) => `sheltersim-compute/${projectId}/${runId}/${branchId}`;

/// How the model versions a branch's stored Run was computed with compare with the ones the core
/// records now for its Scenario (the core's `answers::frames::model_view`): `{ recorded,
/// installed, status }`, the two as JSON text and the status as `modelStatus` answers it; null for
/// a branch without a stored Run or one whose journal cannot be read (a stored Run of an older
/// format is its error, `stored_run_outdated`).
export async function branchModelStatus(wasm, tree, projectId, runId, branchId) {
  const bytes = await tree.read([projectId, "runs", runId, "branches", branchId, JOURNAL]);
  if (bytes === null || bytes.length === 0) return null;
  const scenario = await store.runScenarioBytes(tree, projectId, runId);
  const view = store.answered(() => wasm.branchModelView(bytes, scenario));
  if (view === undefined) return null;
  const { recorded, installed, status } = JSON.parse(view);
  return { recorded: JSON.stringify(recorded), installed: JSON.stringify(installed), status };
}

/// The server's `_require_current_model` (web/workspace.py): a refusal (409 `run_model_view_only`,
/// the core's text) where the branch's stored Run was computed with other model versions than the
/// core records now. A branch without a stored Run, or whose head cannot be read, is left to the
/// computation (which fails the Run, as the server leaves a head it cannot read to the job); a
/// stored Run of an older
/// format is refused (`stored_run_outdated`).
export async function requireCurrentModel(wasm, tree, projectId, runId, branchId) {
  const compared = await branchModelStatus(wasm, tree, projectId, runId, branchId);
  if (compared !== null && !compared.status.current) throw new store.ProjectStoreError(compared.status.error, "run_model_view_only");
}

// ---- Computing ---------------------------------------------------------------------------------

/// The computations of one store (one at a time). `api` is the BrowserApi whose queue the chunks
/// go through (`api.enqueue(task)`), `branchLock(name)` gives a release function, or null when
/// another holder has the lock, and `pause()` lets the Worker take its messages between chunks.
export class Computing {
  constructor({ api, branchLock = async () => () => {}, chunkTicks = CHUNK_TICKS, pause = () => new Promise((resolve) => setTimeout(resolve, 0)) }) {
    this.api = api;
    this.branchLock = branchLock;
    this.chunkTicks = chunkTicks;
    this.pause = pause;
    // "project/run/branch" -> the job
    this.jobs = new Map();
    // The frame of the last head a computation reached: { key, head, minute, text }.
    this.headFrame = null;
    // Each computation's figures, for checks.
    this.done = [];
  }

  /// `POST runs/{run}/resume` (the server's `submit`, its checks in its order): the answer's text.
  async resume(projectId, runId, document) {
    const { wasm, tree } = this.api;
    checkFields(document, ["branch_id", "target_minute"]);
    const branchId = document.branch_id;
    const key = `${projectId}/${runId}/${branchId}`;
    if (this.jobs.has(key)) throw refused("background worker is busy; wait or pause the active run");
    if (this.jobs.size >= 1) throw refused("background workers are busy; wait until a running computation ends");
    const record = await store.branchRecord(tree, projectId, runId, branchId);
    // A Run of another model version is view-only; say so before any job starts.
    await requireCurrentModel(wasm, tree, projectId, runId, branchId);
    await store.runResponse(wasm, tree, projectId, runId);
    const scenario = JSON.parse(decoder.decode(await store.runScenarioBytes(tree, projectId, runId)));
    const target = document.target_minute;
    if (!Number.isSafeInteger(target) || !(record.computed_until < target && target <= scenario.duration_minutes)) {
      throw refused("target minute must be in the uncomputed remaining range");
    }
    if (record.checkpoint_hash === null) throw refused("branch has no computed minutes");
    const release = await this.branchLock(lockName(projectId, runId, branchId));
    if (!release) throw refused("run_worker_busy: another process writes this branch");
    try {
      // The kept worlds go first: WASM's memory does not shrink, and the computation holds one.
      this.api.freeLive();
      await setStatus(wasm, tree, projectId, runId, branchId, RUNNING);
    } catch (error) {
      release();
      throw error;
    }
    const job = { key, projectId, runId, branchId, target, cancel: false, release, from: record.computed_until };
    this.jobs.set(key, job);
    job.finished = this.run(job);
    return JSON.stringify({ branch_id: branchId, run_id: runId, status: RUNNING, target_minute: target });
  }

  /// `POST runs/{run}/cancel` (the server's `cancel`): asks the Run's computations (of `branch_id` only, when
  /// given) to stop after their chunk; the Run's record as it is now.
  async cancel(projectId, runId, document) {
    checkFields(document, [], ["branch_id"]);
    const jobs = [...this.jobs.values()].filter((job) => job.projectId === projectId && job.runId === runId && (document.branch_id == null || document.branch_id === job.branchId));
    for (const job of jobs) job.cancel = true;
    const text = await store.runResponse(this.api.wasm, this.api.tree, projectId, runId);
    if (jobs.length === 0 && JSON.parse(text).status === RUNNING) throw refused("run_worker_busy: cancellation must reach the owning worker");
    return text;
  }

  /// The frame text of a branch's head a computation reached, when `record` is still at it.
  frameAt(projectId, runId, record, minute) {
    const kept = this.headFrame;
    if (!kept || kept.key !== `${projectId}/${runId}/${record.branch_id}`) return null;
    return kept.head === record.checkpoint_hash && kept.minute === minute && record.computed_until === minute ? kept.text : null;
  }

  /// A computation, a chunk at a time through the store's queue, to its status.
  async run(job) {
    const started = performance.now();
    const { wasm, tree } = this.api;
    const host = store.stepHost(wasm, tree, { onRecord: (record, procedure) => this.keepFrame(job, record, procedure) });
    let procedure = null;
    let status = "failed";
    let tickMs = 0;
    let saveMs = 0;
    try {
      await this.pause();
      procedure = host.call(() => wasm.Procedure.goOn(job.projectId, job.runId, job.branchId, job.target));
      // Cut back to the head and restored there.
      if ((await this.api.enqueue(() => host.execute(procedure))) !== "compute") throw new Error("the computation did not start");
      for (;;) {
        await this.pause();
        const ended = await this.api.enqueue(async () => {
          if (job.cancel) {
            host.call(() => procedure.stop());
          } else {
            const ticking = performance.now();
            host.call(() => procedure.advance(this.chunkTicks));
            tickMs += performance.now() - ticking;
          }
          const storing = performance.now();
          const at = await host.execute(procedure);
          if (host.saving) saveMs += performance.now() - storing;
          return at === "done" ? procedure.status() : null;
        });
        if (ended) {
          status = ended;
          break;
        }
      }
    } catch (error) {
      console.error(error);
      job.error = String(error?.message ?? error);
    }
    // The status, the end of the job and its lock go together, in one task of the queue (as the
    // server's `finish` in web/worker_process.py ends under its lock), so a request after it may
    // start the next one.
    const end = () => {
      if (this.jobs.get(job.key) !== job) return;
      this.jobs.delete(job.key);
      job.release();
    };
    await this.api
      .enqueue(async () => {
        try {
          procedure?.free();
          await setStatus(wasm, tree, job.projectId, job.runId, job.branchId, status);
          const record = await store.branchRecord(tree, job.projectId, job.runId, job.branchId);
          this.done.push({
            branch: job.key,
            from: job.from,
            target: job.target,
            reached: record.computed_until,
            status,
            error: job.error ?? null,
            ms: performance.now() - started,
            tickMs,
            saveMs,
            memoryBytes: this.api.wasm.memoryBytes(),
          });
        } finally {
          end();
        }
      })
      .catch((error) => console.error(error));
    end();
    return status;
  }

  /// Keeps the frame of the branch's new head, once its record moved there.
  keepFrame(job, record, procedure) {
    if (record.branch_id !== job.branchId) return;
    const minute = record.computed_until;
    const text = decoder.decode(procedure.frame(job.runId, job.branchId, minute));
    this.headFrame = { key: job.key, head: record.checkpoint_hash, minute, text };
  }
}

/// Sets a branch's status and its Run's (the core's `Procedure.setStatus`).
async function setStatus(wasm, tree, projectId, runId, branchId, status) {
  const procedure = wasm.Procedure.setStatus(projectId, runId, branchId, status);
  try {
    await store.stepHost(wasm, tree).finish(procedure);
  } finally {
    procedure.free();
  }
}
