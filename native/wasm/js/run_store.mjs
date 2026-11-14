// A Run in the browser stored as it is computed, and read back (Issue #252, PR 6-5), with saved
// points to seek to a minute (Issue #251).
//
// The format is the core's (`sheltersim_core::run_store`, #368; its module documentation says what
// a Run's folder holds): which pieces are written and when, the journal's lines, the heads, where a
// seek starts and what it reads, and the checks of each piece. This file is the host's side: it
// opens the files (a backend), compresses each piece with CompressionStream("deflate-raw") before
// writing it and decompresses it after reading it, and passes the bytes to the core through WASM
// (`RunWriter`, `Journal`, `VerifyRun`, `Restore`). A piece is written to data.pack and flushed
// before its journal line is, and each line is flushed before the next is written.
//
// Storage is a backend: `opfsBackend` (the browser's origin private file system, through the
// synchronous access handles a dedicated Worker has) or `memoryBackend` (Node's tests). A backend
// opens a file by name; a file has size(), read(at, length), write(bytes, at), truncate(size),
// flush() and close(), all synchronous.
//
// `wasm` is the module wasm-bindgen made of sheltersim_wasm (any target): its Simulation,
// RunWriter, Journal, VerifyRun, Restore and the saved points' schedule (savedPointStep,
// savedPointDue).

/// The format written (the core's `run_store::Format::CURRENT`).
export const FORMAT = "sheltersim-browser-run-v4";
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
/// queues them, `flush` writes them. The writing methods return the sha256 of the last line
/// written (a branch's head is one, #267).
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
  /// are. Returns the sha256 of the last line written.
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
    return this.core.lastSha256();
  }

  /// The tables' new entries and the newly sealed blocks (every block when `last`) of the log and
  /// the samples of `simulation`. Call it between calls to `advance`.
  async take(simulation, last) {
    this.core.take(simulation, last);
    return this.flush();
  }

  /// A saved point of `simulation` now (#251), after the tables and blocks sealed so far. Call it
  /// between calls to `advance`. `saveMs` counts its time, its compression and writing included.
  /// With `seal` (#267) the blocks are sealed first, so every block the point needs is stored
  /// before its line and it can be restored, and gone on from, at once.
  async save(simulation, { seal = false } = {}) {
    const started = performance.now();
    this.core.save(simulation, seal);
    const sha = await this.flush();
    this.saveMs += performance.now() - started;
    return sha;
  }

  /// The last pieces of a Run stored to its end: the world at its last minute without its log, and
  /// the "end" line with the digests the Run gave. Call it after every block is stored. Returns the
  /// "end" line and its sha256.
  async finish(simulation) {
    this.core.finish(simulation);
    const sha = await this.flush();
    return { end: lineObject(this.lastLine), sha };
  }

  close() {
    this.data.close();
    this.journal.close();
    if (this.owned) this.core.free();
  }
}

/// Runs the exported world `exportBytes` (or the world `world` of a file of named worlds, as the
/// fixtures hold them) for its whole clock, `ticks` ticks per call to `advance`, and stores it in
/// `backend` as it goes, with a saved point every `saveEvery` minutes and at the end (none when
/// `saveEvery` is 0). `progress(minute, duration)` is called after each call. Returns what the Run
/// gave and what storing it took.
export async function runAndStore({ wasm, exportBytes, world = null, backend, ticks = 1440, saveEvery = SAVE_EVERY, progress = () => {} }) {
  if (!Number.isInteger(saveEvery) || saveEvery < 0) throw new Error(`saveEvery is not a count of minutes: ${saveEvery}`);
  const writer = await RunWriter.create(wasm, backend);
  const simulation = world === null ? new wasm.Simulation(exportBytes) : wasm.Simulation.fromWorlds(exportBytes, world);
  try {
    const duration = simulation.duration();
    writer.core.start(simulation, exportBytes, world, saveEvery);
    await writer.flush();
    let tickMs = 0;
    for (;;) {
      const step = wasm.savedPointStep(simulation.minute(), ticks, saveEvery, undefined);
      const started = performance.now();
      const ran = simulation.advance(step);
      tickMs += performance.now() - started;
      progress(simulation.minute(), duration);
      await writer.take(simulation, false);
      if (ran < step) break;
      if (wasm.savedPointDue(simulation.minute(), duration, saveEvery)) await writer.save(simulation);
    }
    await writer.take(simulation, true);
    if (saveEvery > 0) await writer.save(simulation);
    const { end } = await writer.finish(simulation);
    return {
      end,
      digests: JSON.parse(simulation.digests_json()),
      tickMs,
      compressMs: writer.compressMs,
      writeMs: writer.writeMs,
      saveMs: writer.saveMs,
      saved: writer.saved,
      sizes: writer.sizes,
      memoryBytes: wasm.memoryBytes(),
    };
  } finally {
    simulation.free();
    writer.close();
  }
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

/// The piece `index` of `journal` as data.pack holds it, checked to be all there.
function storedPiece(journal, data, index) {
  const [offset, length] = journal.place(index);
  const stored = data.read(offset, length);
  journal.checkStored(index, stored.length);
  return stored;
}

/// Reads a stored Run back from `backend` and checks it (the core's `VerifyRun`): each piece's
/// bytes against its line, and the digests the log, the samples and the world read back to against
/// the ones the "end" line holds. Returns the digests read, whether the Run is complete (it has its
/// "end" line) and whether everything matched; an error for a piece whose bytes are not what its
/// line says.
export async function readAndVerify({ wasm, backend }) {
  const data = await backend.open(DATA);
  const journalFile = await backend.open(JOURNAL);
  const started = performance.now();
  try {
    const journal = new wasm.Journal(journalFile.read(0, journalFile.size()));
    const verify = new wasm.VerifyRun();
    let readMs = 0;
    let inflateMs = 0;
    try {
      journal.checkRun();
      for (const index of verify.reads(journal)) {
        let at = performance.now();
        const stored = storedPiece(journal, data, index);
        readMs += performance.now() - at;
        at = performance.now();
        const raw = await inflateRaw(stored);
        inflateMs += performance.now() - at;
        verify.read(journal, index, raw);
      }
      const { saved_points: savedPoints, ...result } = JSON.parse(verify.resultJson(journal));
      return { ...result, savedPoints, storedBytes: data.size() + journalFile.size(), readMs, inflateMs, totalMs: performance.now() - started };
    } finally {
      verify.free();
      journal.free();
    }
  } finally {
    data.close();
    journalFile.close();
  }
}

/// A stored Run opened to read it (#251): `RunReader.open`, then `seek(minute)` as often as needed,
/// then `close()`. It reads the journal once (`journal`, the core's Journal; `lines`, its lines as
/// objects) and keeps the exported world's bytes.
export class RunReader {
  constructor(wasm, data, journalFile, journal, lines) {
    this.wasm = wasm;
    this.data = data;
    this.journalFile = journalFile;
    this.journal = journal;
    this.lines = lines;
    this.torn = journal.torn();
    this.indexes = new Map(lines.map((line, index) => [line, index]));
    this.exported = null;
  }

  /// The stored Run in `backend`; an error for one of a format this code does not read.
  static async open({ wasm, backend }) {
    const data = await backend.open(DATA);
    const journalFile = await backend.open(JOURNAL);
    let journal = null;
    try {
      journal = new wasm.Journal(journalFile.read(0, journalFile.size()));
      journal.checkRun();
      return new RunReader(wasm, data, journalFile, journal, JSON.parse(journal.linesJson()));
    } catch (error) {
      journal?.free();
      data.close();
      journalFile.close();
      throw error;
    }
  }

  /// The "run" line.
  get run() {
    return this.lines[0];
  }

  /// Whether the Run's samples hold the ledger's columns (format v3 on): the KPI history answers
  /// it and a computation goes on in it.
  current() {
    return this.journal.isCurrent();
  }

  /// The minute the Run starts at: the fork's minute of a branch made by a fork, else 0.
  forkMinute() {
    return this.journal.forkMinute();
  }

  /// The minute the Run's clock is complete at.
  duration() {
    return this.journal.duration();
  }

  /// Minutes between saved points (the default where the "run" line has none).
  saveEvery() {
    return this.journal.saveEvery();
  }

  /// The "end" line, or null for a Run not stored to its end.
  end() {
    const index = this.journal.find("end");
    return index === undefined ? null : this.lines[index];
  }

  /// The "saved" lines a seek can restore, by minute: those whose log and sample blocks are all
  /// stored (in a Run not stored to its end, the last ones may not be).
  savedPoints() {
    return Array.from(this.journal.savedPoints(), (index) => this.lines[index]);
  }

  /// The compressed bytes of the piece `index`, checked to hold the bytes written.
  async compressed(index) {
    const stored = storedPiece(this.journal, this.data, index);
    this.journal.checkRaw(index, await inflateRaw(stored));
    return stored;
  }

  /// The piece `index` decompressed, not yet checked against its line.
  async inflated(index) {
    return inflateRaw(storedPiece(this.journal, this.data, index));
  }

  /// The piece `index`'s bytes, checked against its line: an error for bytes that are not the
  /// ones written.
  async rawAt(index) {
    const raw = await this.inflated(index);
    this.journal.checkRaw(index, raw);
    return raw;
  }

  /// A piece's bytes, by its line (one of `lines`).
  async raw(line) {
    const index = this.indexes.get(line);
    if (index === undefined) throw new Error("not a line of this stored Run");
    return this.rawAt(index);
  }

  /// The exported world the Run started from, read once.
  async exportBytes() {
    if (this.exported === null) {
      const index = this.journal.find("export");
      if (index === undefined) throw new Error("the stored Run has no exported world");
      this.exported = await this.rawAt(index);
    }
    return this.exported;
  }

  /// The Run at `minute` (an integer from the fork's minute to the duration): restored from the
  /// nearest saved point at or before it, or from the exported world when there is none, then
  /// computed to it. Returns { simulation, from, restoreMs, recomputeMs }: a `Simulation` the
  /// caller frees, the minute it was computed from, and the time restoring and computing took.
  async seek(minute) {
    const { wasm, journal } = this;
    const [point, , from] = journal.seek(minute);
    const started = performance.now();
    const exportBytes = await this.exportBytes();
    const world = this.run.world ?? null;
    let simulation;
    if (point < 0) {
      simulation = world === null ? new wasm.Simulation(exportBytes) : wasm.Simulation.fromWorlds(exportBytes, world);
    } else {
      const restore = new wasm.Restore(await this.rawAt(point));
      try {
        for (let index = journal.nextRestoreRead(0, restore); index !== undefined; index = journal.nextRestoreRead(index + 1, restore)) {
          restore.readEntry(journal, index, await this.inflated(index));
        }
        simulation = world === null ? wasm.Simulation.restore(exportBytes, restore) : wasm.Simulation.restoreFromWorlds(exportBytes, world, restore);
      } finally {
        restore.free();
      }
    }
    const restored = performance.now();
    try {
      const ticks = minute - simulation.minute();
      if (ticks < 0 || simulation.advance(ticks) !== ticks) throw new Error(`the Run does not reach minute ${minute}`);
    } catch (error) {
      simulation.free();
      throw error;
    }
    return { simulation, from, restoreMs: restored - started, recomputeMs: performance.now() - restored };
  }

  close() {
    this.journal.free();
    this.data.close();
    this.journalFile.close();
  }
}
