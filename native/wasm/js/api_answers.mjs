// The browser's answers to the API's KPI history, offline report and LLM status of a branch
// (Issue #266), in the JSON the Python server gives for the same Run:
//
//   GET /api/v1/projects/{p}/runs/{r}/branches/{b}/history?kind=kpis&start=&end=&step=&limit=&cursor=
//       historyResponse: from the branch's stored metric samples (js/run_store.mjs, #252), paged as
//       the server pages them (src/sheltersim/web/project_queries.py)
//   GET …/branches/{b}/report       reportResponse: what the server writes when an offline Run of
//       the screen completes (the evidence review's rule fallback; web/worker_process.py
//       `review` for a Run with an LLM), or {"status":"not_generated"} before
//   GET …/branches/{b}/llm-status   llmStatusResponse: the offline Run's answer (no router records)
//
// The answers, their checks and their refusals are the core's (`sheltersim_core::answers`, #449),
// which the local and VM versions call through PyO3: this module reads the files they need from
// the store (`tree`, js/project_store.mjs), the branch's samples, and the Run at its last minute
// for a report not made yet, and keeps a report once made. A refusal is thrown as an `ApiError`
// with the server's HTTP status, `error` and `code`; `errorAnswer` turns it, or a
// `ProjectStoreError`, into the status and body the server gives (StorageError: 409, a query the
// contracts refuse: 400).
//
// Where the browser differs from the server (docs/project-persistence.md lists these):
// - `anchor_hash` and the cursor's anchor are the branch's head (its record's `checkpoint_hash`,
//   the sha256 of its stored Run's last "end" line), not a Checkpoint's hash; a cursor is otherwise
//   the server's (the same scope, minute and key).
// - `first_indexed_checkpoint_minute` is the branch's fork minute, where its stored Run starts.
// - Only `kind=kpis` is answered; any other kind is refused (`browser_unsupported`).
// - A branch stored in an older format than run_store.mjs reads is refused, whatever the route
//   reads it for: 409 `stored_run_outdated` (the core's text: compute it again, #368).
// - The report is made the first time it is asked for, from the stored Run at its last minute,
//   and kept as the branch's `report.json`.
// - A number of the query takes ASCII digits only (Python's `int()` also takes other decimal
//   digits), and a cursor's minute written as 5.0 is taken as 5.

import { OUTDATED_CODE } from "./run_store.mjs";
import { ProjectStoreError, branchReader, branchRecord, coreRefusal, runScenarioBytes } from "./project_store.mjs";

const encoder = new TextEncoder();

/// A refusal with the server's HTTP status, message (`error`) and `code`.
export class ApiError extends Error {
  constructor(status, error, code = null) {
    super(error);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
  }
}

/// Runs a call of the core's answers (`sheltersim_core::answers`, #449); a refusal it throws
/// becomes an `ApiError` with the core's status, text and code.
export function core(call) {
  try {
    return call();
  } catch (error) {
    const refusal = coreRefusal(error);
    if (refusal !== null) throw new ApiError(refusal.status, refusal.error, refusal.code);
    throw error;
  }
}

/// StorageError's code: the given one, or the message's leading word where it is one, or
/// "storage_conflict" (src/sheltersim/storage/projects.py).
function storageCode(message, code = null) {
  if (code) return code;
  const candidate = message.split(":", 1)[0];
  return /^[a-z][a-z0-9_]{1,80}$/.test(candidate) ? candidate : "storage_conflict";
}

/// The status and body (JSON text) the server answers an error with: an `ApiError` as it is, a
/// `ProjectStoreError` as the StorageError it stands for, anything else as the server's 500.
export function errorAnswer(error) {
  // The server answers a request it cannot read with `{"error"}` alone (a 400 of the project
  // routes); a refusal with a code has both.
  if (error instanceof ApiError) return { status: error.status, body: refusalJson(error.message, error.code) };
  if (error instanceof ProjectStoreError) return { status: 409, body: refusalJson(error.message, storageCode(error.message, error.code)) };
  // A stored Run of an older format, wherever it was read (the core's error).
  if (String(error?.message ?? "").startsWith(`${OUTDATED_CODE}: `)) return { status: 409, body: refusalJson(error.message, OUTDATED_CODE) };
  return { status: 500, body: refusalJson("inspector request failed", null) };
}

/// The body of a refusal, as the server writes it: `{"code","error"}` (its keys sorted), or
/// `{"error"}` alone.
const refusalJson = (error, code) => (code === null ? JSON.stringify({ error }) : JSON.stringify({ code, error }));

/// The query's text (after "?"), as the route got it.
const queryText = (query) => (typeof query === "string" ? query : query instanceof URLSearchParams ? query.toString() : "");

/// The bytes of a file of a branch's folder, or undefined where it is missing or an id is not an
/// identifier (the core refuses the id first).
async function branchFile(wasm, tree, projectId, runId, branchId, name) {
  if (![projectId, runId, branchId].every((id) => wasm.isArtifactId(id))) return undefined;
  return (await tree.read([projectId, "runs", runId, "branches", branchId, name])) ?? undefined;
}

// ---- History ----------------------------------------------------------------------------------

/// A cache of the samples read back (`KpiHistory`), by branch head: pass the same one to each call
/// of `historyResponse` (a Worker keeps one) so that paging reads the stored samples once. It
/// keeps the last `size` branches and frees the others' WASM memory.
export function historyCache(size = 2) {
  const kept = new Map();
  return {
    get(key) {
      const table = kept.get(key);
      if (table !== undefined) {
        kept.delete(key);
        kept.set(key, table);
      }
      return table;
    },
    set(key, table) {
      kept.set(key, table);
      while (kept.size > size) {
        const [oldest, value] = kept.entries().next().value;
        kept.delete(oldest);
        value.free();
      }
    },
    clear() {
      for (const table of kept.values()) table.free();
      kept.clear();
    },
  };
}

/// The stored samples of a branch read back (`KpiHistory`, which the caller frees), checked
/// against its "end" line: a fork's from its parent's up to its base, then its own.
export async function readSamples(wasm, tree, projectId, runId, branchId) {
  const reader = await branchReader(wasm, tree, projectId, runId, branchId);
  try {
    const table = new wasm.KpiHistory();
    try {
      for (const position of reader.chain.samplePieces()) {
        if (reader.chain.kind(position) === "sample_tables") table.read_sample_tables(await reader.rawAt(position));
        else table.read_sample_block(await reader.rawAt(position));
      }
      const end = reader.end();
      if (end !== null && end.samples !== table.sample_count()) throw new Error("the stored samples are not the ones the Run took");
      return table;
    } catch (error) {
      table.free();
      throw error;
    }
  } finally {
    reader.close();
  }
}

/// `GET …/branches/{branchId}/history?{query}`: the answer's JSON text (the core's checks, then
/// the page of the branch's samples). `cache` (`historyCache()`) keeps the samples read between
/// calls.
export async function historyResponse({ wasm, tree, projectId, runId, branchId, query = "", cache = null }) {
  const branch = await branchFile(wasm, tree, projectId, runId, branchId, "branch.json");
  const plan = core(() => wasm.historyPlan(queryText(query), projectId, runId, branchId, branch));
  try {
    const anchor = plan.anchor();
    let table = null;
    let owned = false;
    if (anchor === undefined) {
      table = new wasm.KpiHistory();
      owned = true;
    } else {
      const key = JSON.stringify([projectId, runId, branchId, anchor]);
      table = cache?.get(key) ?? null;
      if (table === null) {
        table = await readSamples(wasm, tree, projectId, runId, branchId);
        if (cache) cache.set(key, table);
        else owned = true;
      }
    }
    try {
      return core(() => plan.answer(table));
    } finally {
      if (owned) table.free();
    }
  } finally {
    plan.free();
  }
}

// ---- Report -----------------------------------------------------------------------------------

/// `GET …/branches/{branchId}/report`: the branch's `report.json`; for a completed branch without
/// one, the report of its stored Run at its last minute (the core's `build_report`), kept as
/// `report.json`; else `{"status":"not_generated"}`.
export async function reportResponse({ wasm, tree, projectId, runId, branchId, query = "" }) {
  const branch = await branchFile(wasm, tree, projectId, runId, branchId, "branch.json");
  const kept = await branchFile(wasm, tree, projectId, runId, branchId, "report.json");
  const answer = core(() => wasm.reportAnswer(queryText(query), projectId, runId, branchId, branch, kept));
  if (answer !== undefined) return answer;
  const record = await branchRecord(tree, projectId, runId, branchId);
  const scenarioBytes = await runScenarioBytes(tree, projectId, runId);
  const reader = await branchReader(wasm, tree, projectId, runId, branchId);
  let text;
  try {
    const { simulation } = await reader.seek(record.computed_until);
    try {
      text = core(() => simulation.report(scenarioBytes, branchId));
    } finally {
      simulation.free();
    }
  } finally {
    reader.close();
  }
  await tree.write([projectId, "runs", runId, "branches", branchId, "report.json"], encoder.encode(`${text}\n`));
  return text;
}

// ---- LLM status -------------------------------------------------------------------------------

/// `GET …/branches/{branchId}/llm-status`: an offline Run's answer (the core's `llm_status_answer`).
export async function llmStatusResponse({ wasm, tree, projectId, runId, branchId, query = "" }) {
  const branch = await branchFile(wasm, tree, projectId, runId, branchId, "branch.json");
  return core(() => wasm.llmStatusAnswer(queryText(query), projectId, runId, branchId, branch));
}
