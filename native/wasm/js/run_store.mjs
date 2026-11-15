// A Run in the browser stored as it is computed, and read back (Issue #252, PR 6-5), with saved
// points to seek to a minute (Issue #251).
//
// The format is the core's (`sheltersim_core::run_store`, format v6 of #478; its module
// documentation says what a Run's folder holds): which pieces are written and when, the journal's
// lines, the heads, where a seek starts and what it reads, the checks of each piece, the check of
// private data, and the chain of a fork's Run and its parents' it reads pieces of. This file is the host's side: it opens the files (a backend), compresses each
// piece with CompressionStream("deflate-raw") before writing it and decompresses it after reading it,
// and passes the bytes to the core through WASM (`RunWriter`, `Journal`, `Chain`, `VerifyRun`,
// `Seeker`). A piece is written to data.pack and flushed before its journal line is, and each line
// is flushed before the next is written.
//
// Storage is a backend: `opfsBackend` (the browser's origin private file system, through the
// synchronous access handles a dedicated Worker has) or `memoryBackend` (Node's tests). A backend
// opens a file by name; a file has size(), read(at, length), write(bytes, at), truncate(size),
// flush() and close(), all synchronous. A fork's Run reads its parent's pieces: a reader is given
// `parent(branchId)`, which gives the backend of a branch of the same Run.
//
// The computing procedure (going on from a branch's head in chunks, a fork, a question's turn, the
// start, the recovery of an interrupted branch, a Run computed from minute 0: #476) is the core's
// (`sheltersim_core::compute`, WASM's `Procedure`): it gives the steps, and `StepHost` does them
// with these files (and the store's records, js/project_store.mjs's `stepHost`), as the local and VM
// servers do them with theirs (`sheltersim_store::compute`).
//
// `wasm` is the module wasm-bindgen made of sheltersim_wasm (any target): its Simulation,
// RunWriter, Procedure, Seeker, Journal, Chain, VerifyRun and the saved points' schedule
// (savedPointStep, savedPointDue).

/// The format written and read (the core's `run_store::FORMAT`).
export const FORMAT = "sheltersim-run-v6";
/// The compression of the pieces, which the "run" line names.
export const COMPRESSION = "deflate-raw";
/// The code of the core's error for a Run stored in an older format: compute it again.
export const OUTDATED_CODE = "stored_run_outdated";
/// Minutes between saved points by default: 6 hours (#263; the core's `run_store::SAVE_EVERY`).
export const SAVE_EVERY = 360;
const DATA = "data.pack";
const JOURNAL = "journal.jsonl";
const encoder = new TextEncoder();
const decoder = new TextDecoder();

async function transform(bytes, stream) {
  const piped = new Blob([bytes]).stream().pipeThrough(stream);
  return new Uint8Array(await new Response(piped).arrayBuffer());
}

export const deflateRaw = (bytes) => transform(bytes, new CompressionStream("deflate-raw"));
export const inflateRaw = (bytes) => transform(bytes, new DecompressionStream("deflate-raw"));

/// Files held in memory, for tests: `files` maps a name to its bytes.
export function memoryBackend(files = new Map()) {
  return {
    files,
    async open(name, { truncate = false } = {}) {
      if (truncate || !files.has(name)) files.set(name, new Uint8Array(0));
      return {
        size: () => files.get(name).length,
        read(at, length) {
          return files.get(name).slice(at, at + length);
        },
        write(bytes, at) {
          const old = files.get(name);
          const grown = new Uint8Array(Math.max(old.length, at + bytes.length));
          grown.set(old);
          grown.set(bytes, at);
          files.set(name, grown);
          return bytes.length;
        },
        truncate(size) {
          files.set(name, files.get(name).slice(0, size));
        },
        flush() {},
        close() {},
      };
    },
  };
}

/// The files of a folder of the origin private file system, through synchronous access handles:
/// only a dedicated Worker can make them.
export function opfsBackend(directory) {
  return {
    directory,
    async open(name, { truncate = false } = {}) {
      const handle = await directory.getFileHandle(name, { create: true });
      const access = await handle.createSyncAccessHandle();
      if (truncate) access.truncate(0);
      return {
        size: () => access.getSize(),
        read(at, length) {
          const bytes = new Uint8Array(length);
          const read = access.read(bytes, { at });
          return bytes.subarray(0, read);
        },
        write: (bytes, at) => access.write(bytes, { at }),
        truncate: (size) => access.truncate(size),
        flush: () => access.flush(),
        close: () => access.close(),
      };
    },
  };
}

/// The JSON object of an encoded journal line (its JSON, a tab, its sha256, "\n").
const lineObject = (bytes) => JSON.parse(decoder.decode(bytes.subarray(0, bytes.lastIndexOf(9))));

/// Writes a Run's pieces and journal lines: `core` (the core's RunWriter, made here unless given)
/// queues them, `flush` writes them. The writing methods return the head hash of the last line
/// written (a branch's head is one, #267; the hash of what the line holds, whatever the compression
/// made of its piece, #368).
export class RunWriter {
  constructor(wasm, data, journal, core = null) {
    this.wasm = wasm;
    this.data = data;
    this.journal = journal;
    this.owned = core === null;
    this.core = core ?? new wasm.RunWriter();
    this.sizes = {};
    this.compressMs = 0;
    this.writeMs = 0;
    this.saveMs = 0;
    this.lastLine = null;
  }

  /// A writer of a new Run in the backend's files, which it empties.
  static async create(wasm, backend) {
    const data = await backend.open(DATA, { truncate: true });
    const journal = await backend.open(JOURNAL, { truncate: true });
    return new RunWriter(wasm, data, journal);
  }

  /// The saved points' lines written, as objects of their fields.
  get saved() {
    return JSON.parse(this.core.savedJson());
  }

  /// Writes what the core queued, in order: a piece compressed and appended to data.pack and
  /// flushed, then its journal line appended and flushed; a line without a piece appended and
  /// flushed. `copied` is the compressed bytes of a piece copied from another stored Run, as they
  /// are. Returns the head hash of the last line written.
  async flush(copied = null) {
    for (let kind = this.core.nextKind(); kind !== undefined; kind = this.core.nextKind()) {
      let line;
      if (kind === "line") {
        line = this.core.commitLine();
      } else {
        let stored;
        if (kind.startsWith("copy:")) {
          if (copied === null) throw new Error(`${kind}: no bytes to copy`);
          stored = copied;
          copied = null;
        } else {
          const raw = this.core.nextRaw();
          const started = performance.now();
          stored = await deflateRaw(raw);
          this.compressMs += performance.now() - started;
          const size = (this.sizes[kind] ??= { pieces: 0, raw: 0, stored: 0 });
          size.pieces += 1;
          size.raw += raw.length;
          size.stored += stored.length;
        }
        const started = performance.now();
        const offset = this.data.size();
        this.data.write(stored, offset);
        this.data.flush();
        this.writeMs += performance.now() - started;
        line = this.core.commitPiece(offset, stored.length);
      }
      const started = performance.now();
      this.journal.write(line, this.journal.size());
      this.journal.flush();
      this.writeMs += performance.now() - started;
      this.lastLine = line;
    }
    return this.core.lastHead();
  }

  /// The LLM tape's entries `simulation` kept, the tables' new entries and the newly sealed
  /// blocks (every block when `last`) of the log and the samples. Call it between calls to
  /// `advance`.
  async take(simulation, last) {
    this.core.take(simulation, last);
    return this.flush();
  }

  /// A saved point of `simulation` now (#251): the blocks are sealed and stored and the tape's
  /// entries written first, so it can be restored, and gone on from, at once. Call it between calls
  /// to `advance`. `saveMs` counts its time, its compression and writing included.
  async save(simulation) {
    const started = performance.now();
    this.core.save(simulation);
    const head = await this.flush();
    this.saveMs += performance.now() - started;
    return head;
  }

  /// The last pieces of a Run stored to its end: the tape's entries left, the world at its last
  /// minute without its log, and the "end" line with the digests the Run gave. Returns the "end"
  /// line and its head hash.
  async finish(simulation) {
    this.core.finish(simulation);
    const head = await this.flush();
    return { end: lineObject(this.lastLine), head };
  }

  close() {
    this.data.close();
    this.journal.close();
    if (this.owned) this.core.free();
  }
}

/// The model versions a Run records where its caller gives none: the engine's items.
const engineVersions = (wasm) => JSON.parse(wasm.engineSignatures());

/// What the message of a refusal of the core starts with (native/wasm/src/answers.rs).
export const CORE_REFUSED = "answer_refused ";

/// A refusal the core threw, as `{ status, error, code }`; null for another error.
export function coreRefusal(error) {
  const message = String(error?.message ?? "");
  return message.startsWith(CORE_REFUSED) ? JSON.parse(message.slice(CORE_REFUSED.length)) : null;
}

/// Does the steps of the core's computing procedure (WASM's `Procedure`, #476) with the browser's
/// files: a branch's journal and data.pack (a piece compressed, appended to data.pack and flushed,
/// then its line appended to the journal and flushed), the store's records (`records`,
/// js/project_store.mjs's `stepRecords`; none for a stored Run in a backend of its own), the
/// branches' locks (`branchLock(name)` gives a release, or null where another holds it), and
/// gives back what each read or where it wrote. `folder(project, run, branch)` gives a branch's
/// backend. The files a call opens are closed when it returns: a file has one access handle at a
/// time in OPFS, and a read between the chunks of a computation opens its own. `refused(message,
/// code)` makes the error of a refusal of the core; `onRecord(record, procedure)` is called after
/// a branch's record is written.
export class StepHost {
  constructor({ wasm, folder, records = null, branchLock = null, lockName = null, refused = (message) => new Error(message), onRecord = null }) {
    this.wasm = wasm;
    this.folder = folder;
    this.records = records;
    this.branchLock = branchLock;
    this.lockName = lockName;
    this.refused = refused;
    this.onRecord = onRecord;
    // name -> release, of the locks taken
    this.locks = new Map();
    this.failed = false;
    this.sizes = {};
    this.compressMs = 0;
    this.writeMs = 0;
    // Whether the last call wrote a saved point.
    this.saving = false;
    this.lastLine = null;
  }

  /// Runs a call of the procedure; a refusal it throws becomes `refused`'s error.
  call(call) {
    try {
      return call();
    } catch (error) {
      const refusal = coreRefusal(error);
      if (refusal !== null) throw this.refused(refusal.error, refusal.code);
      throw error;
    }
  }

  /// Does the procedure's steps up to its next "compute" or "done", which it returns.
  async execute(procedure) {
    if (this.failed) throw new Error("the stored Run's writer stopped at a write that failed");
    const open = new Map();
    this.saving = false;
    let done = false;
    try {
      for (;;) {
        const step = JSON.parse(this.call(() => procedure.next()));
        if (step.do === "compute" || step.do === "done") {
          done = step.do === "done";
          return step.do;
        }
        try {
          await this.take(step, procedure, open);
        } catch (error) {
          if (!procedure.mayFail()) {
            this.failed = true;
            throw error;
          }
          this.call(() => procedure.giveFailed());
        }
      }
    } finally {
      for (const file of open.values()) file.close();
      if (done || this.failed) this.releaseLocks();
    }
  }

  /// Does a procedure that ends without computing.
  async finish(procedure) {
    if ((await this.execute(procedure)) !== "done") throw new Error("the procedure asks for a computation");
  }

  releaseLocks() {
    for (const release of this.locks.values()) release();
    this.locks.clear();
  }

  /// A file of a branch, open for the rest of the call.
  async file(open, step, name, options = {}) {
    const key = `${step.project}/${step.run}/${step.branch}/${name}`;
    if (!open.has(key)) {
      const backend = await this.folder(step.project, step.run, step.branch);
      open.set(key, await backend.open(name, options));
    }
    return open.get(key);
  }

  /// Does one step and gives back what it read or where it wrote.
  async take(step, procedure, open) {
    const records = this.records;
    const json = (value) => encoder.encode(JSON.stringify(value));
    switch (step.do) {
      case "run_record": {
        const record = await records.runRecord(step.project, step.run);
        return this.call(() => procedure.giveText(json(record)));
      }
      case "branch_record": {
        const bytes = step.optional ? await records.branchFile(step.project, step.run, step.branch, "branch.json") : json(await records.branchRecord(step.project, step.run, step.branch));
        return this.call(() => procedure.giveText(bytes ?? undefined));
      }
      case "branches": {
        const listed = await records.branches(step.project, step.run);
        return this.call(() => procedure.giveRecords(JSON.stringify(listed)));
      }
      case "projects": {
        const ids = (await records.projects()).map((project) => project.project_id);
        return this.call(() => procedure.giveIds(JSON.stringify(ids)));
      }
      case "runs": {
        const ids = await records.runIds(step.project);
        return this.call(() => procedure.giveIds(JSON.stringify(ids)));
      }
      case "journal": {
        // Read through the file where this call has it open; else without making it.
        const key = `${step.project}/${step.run}/${step.branch}/${JOURNAL}`;
        const file = open.get(key);
        const bytes = file ? file.read(0, file.size()) : records ? await records.branchFile(step.project, step.run, step.branch, JOURNAL) : null;
        return this.call(() => procedure.giveText(bytes ?? undefined));
      }
      case "piece": {
        const file = await this.file(open, step, DATA);
        const stored = file.read(step.offset, step.length);
        if (stored.length !== step.length) throw new Error("data.pack ends before the piece");
        const raw = await inflateRaw(stored);
        return this.call(() => procedure.giveText(raw));
      }
      case "create": {
        for (const name of [DATA, JOURNAL]) {
          const key = `${step.project}/${step.run}/${step.branch}/${name}`;
          open.get(key)?.close();
          open.delete(key);
          await this.file(open, step, name, { truncate: true });
        }
        return this.call(() => procedure.done());
      }
      case "truncate": {
        for (const [name, size] of [[JOURNAL, step.journal], [DATA, step.data]]) {
          if (size === null) continue;
          const file = await this.file(open, step, name);
          if (file.size() > size) {
            file.truncate(size);
            file.flush();
          }
        }
        return this.call(() => procedure.done());
      }
      case "write_piece": {
        const raw = procedure.raw();
        let started = performance.now();
        const stored = await deflateRaw(raw);
        this.compressMs += performance.now() - started;
        const size = (this.sizes[step.kind] ??= { pieces: 0, raw: 0, stored: 0 });
        size.pieces += 1;
        size.raw += raw.length;
        size.stored += stored.length;
        if (step.kind === "saved") this.saving = true;
        const file = await this.file(open, step, DATA);
        started = performance.now();
        const offset = file.size();
        file.write(stored, offset);
        file.flush();
        this.writeMs += performance.now() - started;
        return this.call(() => procedure.givePlace(offset, stored.length));
      }
      case "append_line": {
        const line = procedure.line();
        const file = await this.file(open, step, JOURNAL);
        const started = performance.now();
        file.write(line, file.size());
        file.flush();
        this.writeMs += performance.now() - started;
        this.lastLine = line;
        return this.call(() => procedure.done());
      }
      case "write_branch": {
        await records.writeBranch(step.project, step.run, step.record);
        this.onRecord?.(step.record, procedure);
        return this.call(() => procedure.done());
      }
      case "set_status":
        await records.setStatus(step.project, step.run, step.branch, step.status);
        return this.call(() => procedure.done());
      case "set_run_status":
        await records.setRunStatus(step.project, step.run, step.status);
        return this.call(() => procedure.done());
      case "try_lock": {
        const name = this.lockName(step.project, step.run, step.branch);
        const release = await this.branchLock(name);
        if (release) this.locks.set(name, release);
        return this.call(() => procedure.giveLock(Boolean(release)));
      }
      case "unlock": {
        const name = this.lockName(step.project, step.run, step.branch);
        this.locks.get(name)?.();
        this.locks.delete(name);
        return this.call(() => procedure.done());
      }
      default:
        throw new Error(`a step this host does not take: ${step.do}`);
    }
  }
}

/// Runs a procedure that computes from minute 0 (`Procedure.fromStart`) to its end with `host`,
/// `ticks` ticks per chunk, `progress(minute, duration)` after each; then frees it. Returns what
/// the Run gave and what storing it took. After an error, the steps that mark its branch failed
/// are done, then the error is thrown.
export async function computeFromStart({ wasm, host, procedure, ticks = 1440, progress = () => {} }) {
  let tickMs = 0;
  let saveMs = 0;
  try {
    for (;;) {
      const started = performance.now();
      const at = await host.execute(procedure);
      if (host.saving) saveMs += performance.now() - started;
      if (at === "done") break;
      const ticking = performance.now();
      host.call(() => procedure.advance(ticks));
      tickMs += performance.now() - ticking;
      progress(procedure.minute(), procedure.duration());
    }
    return {
      end: lineObject(procedure.lastLine()),
      digests: JSON.parse(procedure.digestsJson()),
      tickMs,
      compressMs: host.compressMs,
      writeMs: host.writeMs,
      saveMs,
      saved: JSON.parse(procedure.savedJson()),
      sizes: host.sizes,
      memoryBytes: wasm.memoryBytes(),
    };
  } catch (error) {
    if (procedure.fail()) {
      host.failed = false;
      await host.finish(procedure);
    }
    throw error;
  } finally {
    procedure.free();
  }
}

/// Runs the exported world `exportBytes` (or the world `world` of a file of named worlds, as the
/// fixtures hold them) for its whole clock, `ticks` ticks per chunk, and stores it in `backend` as
/// it goes (the core's `Procedure.fromStart`), with a saved point every
/// `saveEvery` minutes and at the end, its "run" line recording `modelVersions`.
/// `progress(minute, duration)` is called after each chunk. Returns what the Run gave and what
/// storing it took.
export async function runAndStore({ wasm, exportBytes, world = null, backend, ticks = 1440, saveEvery = SAVE_EVERY, modelVersions = null, progress = () => {} }) {
  if (!Number.isInteger(saveEvery) || saveEvery < 0) throw new Error(`saveEvery is not a count of minutes: ${saveEvery}`);
  const host = new StepHost({ wasm, folder: async () => backend });
  const versions = JSON.stringify(modelVersions ?? engineVersions(wasm));
  const procedure = host.call(() => wasm.Procedure.fromStart("", "", "", exportBytes, world ?? undefined, saveEvery, versions, false, undefined, 0));
  return computeFromStart({ wasm, host, procedure, ticks, progress });
}

/// The journal's lines up to the first that is cut short or does not match its sha256, as
/// objects: `{ lines, torn }`.
export function journalLines(wasm, text) {
  const journal = new wasm.Journal(encoder.encode(text));
  try {
    return { lines: JSON.parse(journal.linesJson()), torn: journal.torn() };
  } finally {
    journal.free();
  }
}

/// The journal of a branch's stored Run, read once.
function journalOf(wasm, file) {
  return new wasm.Journal(file.read(0, file.size()));
}

/// A stored Run's files opened to read it, with its parents' a fork's Run reads pieces of (the
/// core's `Chain`): `journal` the branch's journal, `chain` the chain, `datas` each level's
/// data.pack (the Run from minute 0's first). `parent(branchId)` gives the backend of a branch of
/// the same Run; without it, a fork's Run is an error. Close it with `close()`.
class Opened {
  static async of({ wasm, backend, parent = null }) {
    const opened = new Opened();
    try {
      const file = await backend.open(JOURNAL);
      opened.files.push(file);
      opened.datas.push(await backend.open(DATA));
      opened.files.push(opened.datas[0]);
      opened.journal = journalOf(wasm, file);
      opened.journal.checkRun();
      opened.chain = new wasm.Chain(opened.journal);
      for (let branchId = opened.chain.parentNeeded(); branchId !== undefined; branchId = opened.chain.parentNeeded()) {
        if (parent === null) throw new Error(`the stored Run of a fork needs its parent's (branch ${branchId})`);
        const parentBackend = await parent(branchId);
        const parentFile = await parentBackend.open(JOURNAL);
        opened.files.push(parentFile);
        const data = await parentBackend.open(DATA);
        opened.files.push(data);
        const journal = journalOf(wasm, parentFile);
        try {
          opened.chain.addParent(journal);
        } finally {
          journal.free();
        }
        opened.datas.unshift(data);
      }
      return opened;
    } catch (error) {
      opened.close();
      throw error;
    }
  }

  constructor() {
    this.files = [];
    this.datas = [];
    this.journal = null;
    this.chain = null;
  }

  /// The compressed bytes of the piece at `position` of the chain, checked to be all there.
  stored(position) {
    const [level, offset, length] = this.chain.place(position);
    const stored = this.datas[level].read(offset, length);
    this.chain.checkStored(position, stored.length);
    return stored;
  }

  close() {
    this.chain?.free();
    this.journal?.free();
    for (const file of this.files) file.close();
    this.chain = null;
    this.journal = null;
    this.files = [];
  }
}

/// Reads a stored Run back from `backend` (a fork's through its parents': `parent`) and checks it
/// (the core's `VerifyRun`): each piece's bytes against its line, and the digests the log, the
/// samples and the world read back to against the ones the "end" line holds. Returns the digests
/// read, the counts, whether the Run is complete (it has its "end" line) and whether everything
/// matched; an error for a piece whose bytes are not what its line says.
export async function readAndVerify({ wasm, backend, parent = null }) {
  const started = performance.now();
  const opened = await Opened.of({ wasm, backend, parent });
  const verify = new wasm.VerifyRun();
  let readMs = 0;
  let inflateMs = 0;
  try {
    for (const position of verify.reads(opened.chain)) {
      let at = performance.now();
      const stored = opened.stored(position);
      readMs += performance.now() - at;
      at = performance.now();
      const raw = await inflateRaw(stored);
      inflateMs += performance.now() - at;
      verify.read(opened.chain, position, raw);
    }
    const { saved_points: savedPoints, ...result } = JSON.parse(verify.resultJson(opened.chain));
    const storedBytes = opened.files.slice(0, 2).reduce((sum, file) => sum + file.size(), 0);
    return { ...result, savedPoints, storedBytes, readMs, inflateMs, totalMs: performance.now() - started };
  } finally {
    verify.free();
    opened.close();
  }
}

/// A stored Run opened to read it (#251): `RunReader.open`, then `seek(minute)` as often as needed,
/// then `close()`. It reads the journals once (`journal`, the branch's, the core's Journal; `lines`,
/// its lines as objects; `chain`, the core's Chain with a fork's parents) and keeps the exported
/// world's bytes.
export class RunReader {
  constructor(wasm, opened) {
    this.wasm = wasm;
    this.opened = opened;
    this.journal = opened.journal;
    this.chain = opened.chain;
    this.lines = JSON.parse(opened.journal.linesJson());
    this.torn = opened.journal.torn();
    this.indexes = new Map(this.lines.map((line, index) => [line, index]));
    this.exported = null;
  }

  /// The stored Run in `backend` (a fork's with its parents', `parent(branchId)` giving their
  /// backends); an error for one of a format this code does not read (an older one:
  /// `stored_run_outdated`).
  static async open({ wasm, backend, parent = null }) {
    return new RunReader(wasm, await Opened.of({ wasm, backend, parent }));
  }

  /// The "run" line.
  get run() {
    return this.lines[0];
  }

  /// The model versions the Run was computed with.
  modelVersions() {
    return JSON.parse(this.journal.modelVersionsJson());
  }

  /// The minute the Run starts at: the fork's minute of a branch made by a fork, else 0.
  forkMinute() {
    return this.journal.forkMinute();
  }

  /// The minute the Run's clock is complete at.
  duration() {
    return this.journal.duration();
  }

  /// Minutes between saved points.
  saveEvery() {
    return this.journal.saveEvery();
  }

  /// The "end" line, or null for a Run not stored to its end.
  end() {
    const index = this.journal.find("end");
    return index === undefined ? null : this.lines[index];
  }

  /// The "saved" lines of the branch a seek can restore, by minute.
  savedPoints() {
    return Array.from(this.journal.savedPoints(), (index) => this.lines[index]);
  }

  /// The compressed bytes of the branch's line `index`'s piece, checked to hold the bytes written.
  async compressed(index) {
    const [offset, length] = this.journal.place(index);
    const stored = this.opened.datas.at(-1).read(offset, length);
    if (stored.length !== length) throw new Error("data.pack ends before the piece");
    this.journal.checkRaw(index, await inflateRaw(stored));
    return stored;
  }

  /// The piece at `position` of the chain decompressed, not yet checked against its line.
  async inflated(position) {
    return inflateRaw(this.opened.stored(position));
  }

  /// The bytes of the piece at `position` of the chain, checked against its line: an error for
  /// bytes that are not the ones written.
  async rawAt(position) {
    const raw = await this.inflated(position);
    this.chain.checkRaw(position, raw);
    return raw;
  }

  /// The JSON texts of the records of `kind` ("conversations", "llm_tape"), parsed: the tape's
  /// from the Run's start, the turns of the branch itself.
  async records(kind) {
    const records = [];
    for (const raw of await this.rawRecords(kind)) records.push(...JSON.parse(this.wasm.recordsJson(raw)));
    return records;
  }

  /// The pieces of the records of `kind`, decompressed, in the order `records` reads them.
  async rawRecords(kind) {
    const pieces = [];
    for (const position of this.chain.records(kind)) pieces.push(await this.rawAt(position));
    return pieces;
  }

  /// The exported world the Run started from, read once.
  async exportBytes() {
    if (this.exported === null) this.exported = await this.rawAt(this.chain.export());
    return this.exported;
  }

  /// The Run at `minute` (an integer from the fork's minute to the duration): restored from the
  /// nearest saved point at or before it, or from the exported world when there is none, then
  /// computed to it. The seek is the core's (WASM's `Seeker`, #501), as the servers' and the
  /// computing procedure's are: it names each piece of the chain to read, and here it is read and
  /// decompressed. Returns { simulation, from, base, restoreMs, recomputeMs }: a `Simulation` the
  /// caller frees, the minute it was computed from, the branch's line it was restored from, and
  /// the time restoring and computing took.
  async seek(minute) {
    const { wasm, chain } = this;
    const started = performance.now();
    const seeker = new wasm.Seeker(chain, minute);
    try {
      seeker.giveExport(await this.exportBytes());
      for (let position = seeker.next(chain); position !== undefined; position = seeker.next(chain)) {
        seeker.give(chain, await this.inflated(position));
      }
      const restored = performance.now();
      const simulation = seeker.finish();
      return { simulation, from: seeker.from(), base: seeker.base(), restoreMs: restored - started, recomputeMs: performance.now() - restored };
    } finally {
      seeker.free();
    }
  }

  close() {
    this.opened.close();
  }
}
