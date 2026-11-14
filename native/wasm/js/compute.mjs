// Computing a Run in the browser (Issue #267, stage B of #263): the browser's answers to
// `POST runs/{run}/resume` and `POST runs/{run}/cancel` (js/browser_api.mjs routes them here), and
// what a page closed in the middle leaves. A Run to compute is made from a Scenario
// (js/scenario_routes.mjs, `POST runs`); the Run the site offered to compute beside the sample
// (#267) is gone since #411, and the tests make theirs with native/wasm/tests/compute_run.mjs.
//
// A branch's head. A branch's record (`computed_until`, `checkpoint_hash`) names a line of its
// stored Run's journal the Run can be read back at and gone on from: the "export" line (minute 0),
// a "saved" line whose blocks are all stored before it, or the "end" line. Its `checkpoint_hash`
// is that line's sha256 (project_store.mjs: the "end" line's for a Run stored to its end). The
// record is written after the line is flushed, so the record is the commit: lines after the head
// are a computation that did not reach its next head, and going on cuts them off first.
//
// Computing (Computing.resume). Python's checks and answers (src/sheltersim/web/projects.py
// `submit`, `cancel`), one computation at a time (the thread worker's max_jobs). The branch is cut
// back to its head, restored there (`RunReader.seek`, #251; the exported world at minute 0) and
// computed in chunks of at most `chunkTicks` ticks. Each chunk is a task of the store's queue (the
// requests' queue of browser_api.mjs, inside its Web Lock), so the Worker answers timeline,
// history and the records between chunks. At every `save_every` minutes (360, #263), at the
// target and when cancelled, the blocks are sealed (`RunWriter.save` with `seal`) and a saved point
// is stored: the record then moves its head there, the frame of that minute is kept for the
// screen's next `timeline` of it, and the Run's records stay `running` until the computation
// ends: `completed` at the end of the clock (with the "world" and "end" lines, as runAndStore
// stores them), `paused` at the target or when cancelled, `failed` on an error. The OPFS files
// are opened for each chunk only: a file has one synchronous access handle at a time, and a read
// between chunks opens its own.
//
// A page closed in the middle (recoverInterrupted): the Worker dies with the page; the record
// keeps the last head, the journal may hold lines after it (or a line cut short, which reading
// ignores). On the next start a branch left `running` whose lock no Worker holds becomes
// `interrupted`, as Python's server start does; resuming it cuts the lines after its head off.
// The lock of a branch (`branchLock`, a Web Lock in the page) is held while it computes, so
// another tab neither computes it at the same time nor calls it interrupted.

import { ApiError } from "./api_answers.mjs";
import * as store from "./project_store.mjs";
import { RunReader, RunWriter } from "./run_store.mjs";

/// Ticks per chunk by default: about a tenth of a second for 100 people.
export const CHUNK_TICKS = 60;
const DATA = "data.pack";
const JOURNAL = "journal.jsonl";
const decoder = new TextDecoder();
const refused = (message) => new store.ProjectStoreError(message);
/// Python's status of a Run that is computing, and of one a computation left (`_advance`).
const RUNNING = "running";

/// Python's `fields`: the document has the required keys and no others but the optional ones.
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
/// `{ kind, minute, index, sha }`. A journal whose lines cannot be read has none.
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

/// Cuts a branch's stored Run back to the head `hash` (the core's `Journal::cut`): the journal's
/// lines after it, then the bytes of data.pack no line up to it points at. Returns the head and the
/// core's writer that goes on after it.
async function cutToHead(wasm, backend, hash) {
  const file = await backend.open(JOURNAL);
  const data = await backend.open(DATA);
  try {
    const journal = new wasm.Journal(file.read(0, file.size()));
    try {
      const [index, journalEnd, dataEnd] = journal.cut(hash);
      const head = JSON.parse(journal.headsJson()).find((each) => each.index === index);
      if (file.size() > journalEnd) {
        file.truncate(journalEnd);
        file.flush();
      }
      if (data.size() > dataEnd) {
        data.truncate(dataEnd);
        data.flush();
      }
      return { head, writer: journal.writerAfter(index) };
    } finally {
      journal.free();
    }
  } finally {
    file.close();
    data.close();
  }
}

/// A branch left `running` that no Worker computes (its lock is free: `branchLock(name)` gives a
/// release) was interrupted: its record and its Run's become `interrupted` (Python's
/// `recover_interrupted` at the server's start), its head kept. Every Project and Run is looked
/// at; one whose records cannot be read is left as it is. Returns the branches recovered, as
/// "project/run/branch".
export async function recoverInterrupted({ tree, branchLock }) {
  const recovered = [];
  for (const project of await store.projects(tree)) {
    let runs;
    try {
      ({ runs } = JSON.parse(await store.runsResponse(tree, project.project_id)));
    } catch {
      continue;
    }
    for (const run of runs) {
      for (const branch of run.branches) {
        if (branch.status !== RUNNING) continue;
        const release = await branchLock(lockName(project.project_id, run.run_id, branch.branch_id));
        if (!release) continue;
        try {
          const record = await store.branchRecord(tree, project.project_id, run.run_id, branch.branch_id);
          if (record.status !== RUNNING) continue;
          await store.writeBranch(tree, project.project_id, run.run_id, { ...record, status: "interrupted" });
          await store.setRunStatus(tree, project.project_id, run.run_id, "interrupted");
          recovered.push(`${project.project_id}/${run.run_id}/${branch.branch_id}`);
        } finally {
          release();
        }
      }
    }
  }
  return recovered;
}

/// The name of a branch's lock (a Web Lock in the page).
export const lockName = (projectId, runId, branchId) => `sheltersim-compute/${projectId}/${runId}/${branchId}`;

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

  /// `POST runs/{run}/resume` (Python's `submit`, its checks in its order): the answer's text.
  async resume(projectId, runId, document) {
    const { wasm, tree } = this.api;
    checkFields(document, ["branch_id", "target_minute"]);
    const branchId = document.branch_id;
    const key = `${projectId}/${runId}/${branchId}`;
    if (this.jobs.has(key)) throw refused("background worker is busy; wait or pause the active run");
    if (this.jobs.size >= 1) throw refused("background workers are busy; wait until a running computation ends");
    const record = await store.branchRecord(tree, projectId, runId, branchId);
    await store.runResponse(tree, projectId, runId);
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
      await store.writeBranch(tree, projectId, runId, { ...record, status: RUNNING });
      await store.setRunStatus(tree, projectId, runId, RUNNING);
    } catch (error) {
      release();
      throw error;
    }
    const job = { key, projectId, runId, branchId, target, cancel: false, release, from: record.computed_until };
    this.jobs.set(key, job);
    job.finished = this.run(job);
    return JSON.stringify({ branch_id: branchId, run_id: runId, status: RUNNING, target_minute: target });
  }

  /// `POST runs/{run}/cancel` (Python's): asks the Run's computations (of `branch_id` only, when
  /// given) to stop after their chunk; the Run's record as it is now.
  async cancel(projectId, runId, document) {
    checkFields(document, [], ["branch_id"]);
    const jobs = [...this.jobs.values()].filter((job) => job.projectId === projectId && job.runId === runId && (document.branch_id == null || document.branch_id === job.branchId));
    for (const job of jobs) job.cancel = true;
    const text = await store.runResponse(this.api.tree, projectId, runId);
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
    let state = null;
    let status = "failed";
    try {
      await this.pause();
      state = await this.api.enqueue(() => this.start(job));
      for (;;) {
        await this.pause();
        const ended = await this.api.enqueue(() => this.chunk(job, state));
        if (ended) {
          status = ended;
          break;
        }
      }
    } catch (error) {
      console.error(error);
      job.error = String(error?.message ?? error);
    }
    // The status, the end of the job and its lock go together, in one task of the queue (as
    // Python's `_advance` ends under its lock), so a request after it may start the next one.
    const end = () => {
      if (this.jobs.get(job.key) !== job) return;
      this.jobs.delete(job.key);
      job.release();
    };
    await this.api
      .enqueue(async () => {
        try {
          state?.simulation.free();
          state?.writer.free();
          const { tree } = this.api;
          const record = await store.branchRecord(tree, job.projectId, job.runId, job.branchId);
          await store.writeBranch(tree, job.projectId, job.runId, { ...record, status });
          await store.setRunStatus(tree, job.projectId, job.runId, status);
          this.done.push({
            branch: job.key,
            from: job.from,
            target: job.target,
            reached: record.computed_until,
            status,
            error: job.error ?? null,
            ms: performance.now() - started,
            tickMs: state?.tickMs ?? 0,
            saveMs: state?.saveMs ?? 0,
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

  /// Cuts the branch back to its head and restores the Run there.
  async start(job) {
    const { wasm, tree } = this.api;
    const record = await store.branchRecord(tree, job.projectId, job.runId, job.branchId);
    const backend = await store.branchBackend(tree, job.projectId, job.runId, job.branchId);
    const { head, writer } = await cutToHead(wasm, backend, record.checkpoint_hash);
    try {
      const reader = await RunReader.open({ wasm, backend });
      let sought;
      let duration;
      let saveEvery;
      try {
        if (!reader.current()) throw new Error(`browser_run_outdated: the branch was stored as ${reader.run.format}; compute it again`);
        sought = await reader.seek(head.minute);
        duration = reader.duration();
        saveEvery = reader.saveEvery();
      } finally {
        reader.close();
      }
      const simulation = sought.simulation;
      if (head.kind === "saved" && !(simulation.minute() === head.minute && sought.from === head.minute && writer.fits(simulation))) {
        simulation.free();
        throw new Error("the Run restored at the branch's head does not hold what its stored Run holds there");
      }
      return { simulation, backend, writer, record, duration, saveEvery, head: head.minute, tickMs: 0, saveMs: 0 };
    } catch (error) {
      writer.free();
      throw error;
    }
  }

  /// One chunk: at most `chunkTicks` ticks, not past the next saved point or the target, then
  /// the blocks sealed since stored, and at a saved point, the target or the clock's end the
  /// branch's new head. Returns the computation's status once it ends, else null.
  async chunk(job, state) {
    const { wasm } = this.api;
    const { simulation, duration, saveEvery } = state;
    const writer = new RunWriter(wasm, await state.backend.open(DATA), await state.backend.open(JOURNAL), state.writer);
    try {
      let minute = simulation.minute();
      if (job.cancel) {
        if (minute > state.head) await this.commit(job, state, minute, await this.save(writer, state));
        return minute === duration ? "completed" : "paused";
      }
      const step = wasm.savedPointStep(minute, this.chunkTicks, saveEvery, job.target);
      const started = performance.now();
      const ran = simulation.advance(step);
      state.tickMs += performance.now() - started;
      minute = simulation.minute();
      if (ran !== step) throw new Error(`the Run stopped at minute ${minute}`);
      await writer.take(simulation, false);
      if (minute === duration) {
        await this.save(writer, state);
        const { sha } = await writer.finish(simulation);
        await this.commit(job, state, minute, sha);
        return "completed";
      }
      if (minute === job.target || wasm.isSavedPointMinute(minute, saveEvery)) await this.commit(job, state, minute, await this.save(writer, state));
      return minute === job.target ? "paused" : null;
    } finally {
      writer.close();
    }
  }

  async save(writer, state) {
    const started = performance.now();
    const sha = await writer.save(state.simulation, { seal: true });
    state.saveMs += performance.now() - started;
    return sha;
  }

  /// Moves the branch's head to the line `sha` at `minute` and keeps the frame there.
  async commit(job, state, minute, sha) {
    const { tree } = this.api;
    await store.writeBranch(tree, job.projectId, job.runId, { ...state.record, computed_until: minute, checkpoint_hash: sha, status: RUNNING });
    state.head = minute;
    const text = decoder.decode(state.simulation.frame(job.runId, job.branchId, minute, undefined));
    this.headFrame = { key: job.key, head: sha, minute, text };
  }
}
