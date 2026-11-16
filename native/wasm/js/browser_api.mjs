// The browser's answers to the API the screen `/demo-v2` calls (Issue #266, stage A of #263).
//
// The page wraps `fetch` (js/api_fetch.js) and hands each `/api/v1/...` request to a dedicated
// Worker (js/api_worker.mjs), which answers it here from the origin private file system: the
// Project, Run and Branch records and each branch's stored Run (js/project_store.mjs,
// js/run_store.mjs), and WASM (sheltersim_wasm). An answer is what the Python server
// (src/sheltersim/web/server.py, web/projects.py) answers: its status and its JSON, `{"error", "code"}`
// for an error.
//
// Routes (under /api/v1/projects):
//   GET  (list), POST (create), GET {project}, GET {project}/runs, GET runs/{run},
//   GET runs/{run}/evidence                  the records (project_store.mjs's response functions)
//   GET runs/{run}/branches/{branch}/timeline?minute=   the frame of the minute
//   GET runs/{run}/replay-frames?branch_ids=&start=&end=   the frames of 1 to 8 minutes of one branch,
//                                            or of two branches, each minute's comparison (#307)
//   GET runs/{run}/compare?branch_ids=&minute=   js/comparison.mjs (#307)
//   GET runs/{run}/branches/{branch}/history, report, llm-status   js/api_answers.mjs (or the
//                                            functions the caller gives, `extra`)
//   POST runs/{run}/resume, POST runs/{run}/cancel   computing in the Worker (js/compute.mjs, #267)
//   POST runs/{run}/branches/{branch}/intervention-preview, intervention-apply   js/interventions.mjs
//                                            (#306); GET runs/{run} then holds the fork's `last_fork`;
//                                            a preview's text is read with the LLM through the site's
//                                            relay when the request carries the passphrase (#470)
//   POST runs/{run}/branches/{branch}/questions   js/questions.mjs (#308): the answer without an API key,
//                                            or with the passphrase, the LLM's through the relay (#470)
//   POST runs/{run}/delete                   the Run removed (#376), as the server's
//   GET  runs/{run}/branches/{branch}/conversations   the questions and answers kept (#376)
//   POST runs/{run}/pdf-report               the server's checks, then its answer without an API key:
//                                            409 pdf_analysis_offline (#309); with the passphrase, the
//                                            report with the LLM's analysis through the relay (#470)
//   POST {project}/scenario-preview, scenario-approve, runs   js/scenario_routes.mjs (#301); a preview's
//                                            text is read with the LLM through the site's relay when the
//                                            request carries the passphrase (`llm`, #421)
//   anything else (the routes outside projects)   409 {"error": "ブラウザー版では未対応です…",
//                                            "code": "browser_unsupported"}
//
// A frame is the core's (`Simulation.frame`, #264) with the world's digest as its `state_hash`, as
// the server's is; a frame of `replay-frames` has none (`Simulation.replayFrame`, #500), as the
// digest of the whole world would cost most of each frame of a replay. The Run at a minute: a branch keeps one live Simulation; a later minute steps it
// forward when that is cheaper than restoring the nearest saved point, any other minute seeks
// (`RunReader.seek`, #251) and frees the Simulation it replaces. Replay therefore steps the kept
// Simulation and never restores. At most `keep` Simulations are kept (the least recently used one
// is freed).
//
// The sample (installSample): the site holds the precomputed sample Project as one bundle
// (project_store.mjs, #265) split into parts no larger than a static host takes, listed in a small
// manifest (SAMPLE_FORMAT): the parts are fetched, each checked against its sha256, joined, the
// whole checked, and imported (`importProject`). The site lists its examples (EXAMPLES_FORMAT,
// #411), and one is fetched only when the home's list is chosen from. The store keeps each
// example's sha256 beside the Projects (SAMPLE_MARKER), so a site with another example offers it
// again.

import { ApiError, core, errorAnswer, historyCache, historyResponse, llmStatusResponse, readSamples, reportResponse } from "./api_answers.mjs";
import { CHUNK_TICKS, Computing, branchModelStatus, postDocument } from "./compute.mjs";
import * as store from "./project_store.mjs";
import { compareResponse, replayPair } from "./comparison.mjs";
import { interventionApply, interventionPreview } from "./interventions.mjs";
import { questionResponse } from "./questions.mjs";
import { SAVE_EVERY } from "./run_store.mjs";
import { askRelay, sameOriginFetch } from "./relay_link.mjs";
import { bodyDocument, ScenarioRoutes } from "./scenario_routes.mjs";

/// The manifest of the site's sample: {format, version, project_id, bytes, sha256, parts: [{path,
/// bytes, sha256}]}, the paths relative to the manifest.
export const SAMPLE_FORMAT = "sheltersim-site-sample";
export const SAMPLE_VERSION = 1;
/// What the store keeps of the examples it imported, beside the Projects' folders:
/// `{ examples: { <project id>: <sha256> } }` (before #411, `{ sha256, project_id }` of one).
export const SAMPLE_MARKER = "site-sample.json";
/// The site's list of examples: {format, version, examples: [{manifest, project_id, bytes, sha256,
/// runs, branches, municipality, disaster, people, duration_minutes}]}, the manifests' paths
/// relative to the list (scripts/build_static_site.py).
export const EXAMPLES_FORMAT = "sheltersim-site-examples";
export const EXAMPLES_VERSION = 1;
export const UNSUPPORTED = "ブラウザー版では未対応です";
const decoder = new TextDecoder();
const encoder = new TextEncoder();

// Errors are api_answers.mjs's: an `ApiError` with the server's status, or what the server's
// store refuses (a StorageError: 409, its code the message's leading word or "storage_conflict").
const refused = (message) => new store.ProjectStoreError(message);
const unsupported = (what) => new ApiError(409, `${UNSUPPORTED}（${what}）`, "browser_unsupported");

const json = (status, text) => ({ status, type: "application/json; charset=utf-8", body: text });

/// What the screen says when the browser does not let the site use its store (OPFS, #393): a
/// private window may refuse it (WebKit's answers `UnknownError`, "out of memory", which it is not).
export const STORAGE_UNAVAILABLE =
  "このブラウザーでは、計算の結果を保存する場所（OPFS）を使えません。" +
  "プライベートブラウズやシークレットのウインドウでは使えないことがあります。通常のウインドウで開いてください。";

/// The root folder of the browser's store (`navigator.storage`'s OPFS), or a 503
/// `browser_storage_unavailable` with STORAGE_UNAVAILABLE when there is none or it cannot be
/// opened. The Worker opens it before anything else, so the screen tells this from other failures.
export async function openStore(storage) {
  try {
    if (typeof storage?.getDirectory !== "function") throw new Error("navigator.storage.getDirectory is missing");
    return await storage.getDirectory();
  } catch (error) {
    throw Object.assign(new ApiError(503, STORAGE_UNAVAILABLE, "browser_storage_unavailable"), { cause: error });
  }
}

/// The query as Python's `parse_qs` reads it: blank values dropped, each name's values in order.
function queryOf(search) {
  const query = new Map();
  for (const [name, value] of new URLSearchParams(search)) {
    if (value === "") continue;
    if (!query.has(name)) query.set(name, []);
    query.get(name).push(value);
  }
  return query;
}

/// The answers of one store. `wasm` is sheltersim_wasm, `tree` a project_store tree, `quota` what
/// the store may hold (the Project list's `storage.limit_bytes`), `extra` the routes another module
/// answers: `{ history, report, llmStatus }`, each `({ wasm, tree, projectId, runId, branchId,
/// query }) => JSON text`, `query` the request's query string as it came. By default they are
/// api_answers.mjs's (the history with a cache of the branches' samples). `locked(task)` runs each
/// task of the queue (a request or a chunk of a computation) holding the store's lock (the page:
/// a Web Lock), and `branchLock`, `chunkTicks` and `pause` are js/compute.mjs's `Computing`'s.
export class BrowserApi {
  constructor({ wasm, tree, quota = async () => null, extra = null, keep = 2, locked = (task) => task(), branchLock, chunkTicks = CHUNK_TICKS, pause, scenario = {}, relay = sameOriginFetch }) {
    this.wasm = wasm;
    this.tree = tree;
    // The site's LLM relay: `relay(path, init)` (relay_link.mjs; tests answer it).
    this.relay = relay;
    this.quota = quota;
    this.history = historyCache(2);
    this.extra = extra ?? {
      history: (asked) => historyResponse({ ...asked, cache: this.history }),
      report: async (asked) => {
        // A report not yet made is made from the Run at its end, in a Simulation of its own:
        // free the kept ones first, as WASM's memory does not shrink.
        const kept = await tree.read([asked.projectId, "runs", asked.runId, "branches", asked.branchId, "report.json"]);
        if (kept === null) this.freeLive();
        return reportResponse(asked);
      },
      llmStatus: llmStatusResponse,
    };
    this.keep = keep;
    // "project/run/branch" -> { simulation, head, savedMinutes, used }
    this.live = new Map();
    // "project/run/branch" -> { head, view }: whether the branch's Run is view-only (`viewOf`).
    this.views = new Map();
    this.used = 0;
    this.stats = { seeks: 0, steps: 0, restoreMs: 0, recomputeMs: 0, stepMs: 0, frameMs: 0, headFrames: 0 };
    this.queue = Promise.resolve();
    this.locked = locked;
    this.computing = new Computing({ api: this, branchLock, chunkTicks, pause });
    // The Scenario screen's routes (#301); `scenario` holds their `shelterIndex`, `shelterRows`,
    // `fetchJson`, `now` and `newId` (scenario_routes.mjs).
    this.scenarioRoutes = new ScenarioRoutes({ wasm, tree, relay, ...scenario });
    // "project/run" -> the last fork's outcome (the server's `_fork_outcomes`, #306)
    this.forkOutcomes = new Map();
  }

  /// Answers a request: `{ method, path, search, body, llm? }` (body the text of a POST; llm the
  /// passphrase of the site's relay, for a Scenario preview (#421), a question, an intervention's
  /// preview or the analysis report (#470)) to `{ status, type, body }`. Requests are answered one at a
  /// time, in the order they came.
  answer(request) {
    return this.enqueue(() => this.route(request));
  }

  /// Runs `task` after the requests and tasks queued before it, holding the store's lock.
  enqueue(task) {
    const done = this.queue.then(() => this.locked(task));
    this.queue = done.catch(() => {});
    return done;
  }

  async route({ method, path, search = "", body = null, llm = null }) {
    try {
      return json(200, await this.dispatch(method, path, search, body, llm));
    } catch (error) {
      if (!(error instanceof ApiError) && !(error instanceof store.ProjectStoreError)) console.error(error);
      const { status, body } = errorAnswer(error);
      return json(status, body);
    }
  }

  async dispatch(method, path, search, body, llm = null) {
    if (!path.startsWith("/api/v1/")) throw new ApiError(404, "page not found");
    const parts = path.split("/").filter((part) => part !== "").map((part) => decodeURIComponent(part));
    if (parts[2] !== "projects") throw unsupported(path);
    const rest = parts.slice(3);
    const query = queryOf(search);
    if (method === "POST") return this.post(rest, body, llm);
    if (method !== "GET") throw new ApiError(405, "inspector API is read-only");
    return this.get(rest, query, search);
  }

  async post(parts, body, llm = null) {
    if (parts.length === 0) {
      let document;
      try {
        document = JSON.parse(body ?? "");
      } catch {
        throw new ApiError(400, "invalid project request");
      }
      if (document === null || typeof document !== "object" || Array.isArray(document)) throw refused("project request must be an object");
      const keys = Object.keys(document);
      if (keys.length !== 1 || keys[0] !== "project_id") throw refused("request fields do not match this operation");
      if (typeof document.project_id !== "string") throw new ApiError(400, "invalid project request");
      return JSON.stringify(await store.createProject(this.tree, document.project_id));
    }
    if (parts.length === 2 && ["scenario-preview", "scenario-approve", "runs"].includes(parts[1])) {
      const document = bodyDocument(body);
      const routes = this.scenarioRoutes;
      if (parts[1] === "scenario-preview") return routes.preview(parts[0], document, { llm });
      if (parts[1] === "scenario-approve") return routes.approve(parts[0], document);
      return routes.runs(parts[0], document);
    }
    if (parts.length === 4 && parts[1] === "runs" && (parts[3] === "resume" || parts[3] === "cancel")) {
      const document = postDocument(body);
      return parts[3] === "resume" ? this.computing.resume(parts[0], parts[2], document) : this.computing.cancel(parts[0], parts[2], document);
    }
    if (parts.length === 6 && parts[1] === "runs" && parts[3] === "branches" && (parts[5] === "intervention-preview" || parts[5] === "intervention-apply")) {
      if (parts[5] === "intervention-apply") return interventionApply(this, parts[0], parts[2], parts[4], body);
      return interventionPreview(this, parts[0], parts[2], parts[4], body, llm);
    }
    if (parts.length === 6 && parts[1] === "runs" && parts[3] === "branches" && parts[5] === "questions") return questionResponse(this, parts[0], parts[2], parts[4], body, llm);
    if (parts.length === 4 && parts[1] === "runs" && parts[3] === "pdf-report") return this.pdfReport(parts[0], parts[2], body, llm);
    if (parts.length === 4 && parts[1] === "runs" && parts[3] === "delete") return this.deleteRun(parts[0], parts[2], body);
    const route = parts.length >= 6 && parts[1] === "runs" ? parts.slice(3).join("/") : parts.slice(1).join("/");
    throw unsupported(route || "POST");
  }

  async get(parts, query, search = "") {
    const tree = this.tree;
    const noQuery = query.size === 0;
    if (parts.length === 0) {
      if (!noQuery) throw refused("projects list takes no query parameters");
      return store.projectsResponse(this.wasm, tree, { limitBytes: await this.quota() });
    }
    const [projectId, section, runId, action, branchId, branchAction] = parts;
    if (parts.length === 1 && noQuery) return store.projectResponse(this.wasm, tree, projectId);
    if (parts.length === 2 && section === "runs" && noQuery) return store.runsResponse(this.wasm, tree, projectId);
    if (parts.length === 3 && section === "runs" && noQuery) return store.runResponse(this.wasm, tree, projectId, runId, this.forkOutcomes.get(`${projectId}/${runId}`) ?? null);
    if (parts.length === 4 && section === "runs" && action === "evidence" && noQuery) return store.evidenceResponse(this.wasm, tree, projectId, runId);
    if (parts.length === 4 && section === "runs" && action === "replay-frames") return this.replay(projectId, runId, search);
    if (parts.length === 4 && section === "runs" && action === "compare" && query.size === 2 && query.has("branch_ids") && query.has("minute")) {
      return compareResponse(this, projectId, runId, search);
    }
    if (parts.length === 4 && section === "runs" && action === "compare") throw refused("unknown project route or query");
    if (parts.length === 6 && section === "runs" && action === "branches") {
      if (branchAction === "timeline" && query.size === 1 && query.get("minute")?.length === 1) {
        const files = await store.runFiles(this.wasm, tree, projectId, runId);
        let plan;
        try {
          plan = JSON.parse(core(() => this.wasm.timelinePlan(query.get("minute")[0], projectId, runId, files, branchId)));
        } finally {
          files.free();
        }
        return this.frameText(projectId, runId, plan.record, plan.minute);
      }
      if (branchAction === "conversations" && noQuery) {
        await this.branch(projectId, runId, branchId);
        // The server's answer (web/projects.py): the core's of the turns kept.
        const pieces = await store.conversationPieces(this.wasm, tree, projectId, runId, branchId);
        try {
          return core(() => this.wasm.conversationsAnswer(pieces));
        } finally {
          pieces.free();
        }
      }
      const named = { history: "history", report: "report", "llm-status": "llmStatus" }[branchAction];
      if (named) {
        if (typeof this.extra[named] !== "function") throw unsupported(branchAction);
        return this.extra[named]({ wasm: this.wasm, tree, projectId, runId, branchId, query: search });
      }
      throw unsupported(branchAction);
    }
    if (parts.length >= 2 && section !== "runs") throw unsupported(section);
    if (parts.length >= 4 && section === "runs") throw unsupported(action);
    throw refused("unknown project route or query");
  }

  /// `POST runs/{run}/pdf-report` (src/sheltersim/web/pdf_report.py `export`): the core's checks
  /// in the server's order; without the relay's passphrase, what the server answers without an API
  /// key: the analysis needs Opus, so no report is made
  /// (`sheltersim_core::answers::routes::pdf_report_answer`). With it (#470), the report as the
  /// server makes it with an API key: each branch's Run at the minute (a view-only one at a saved
  /// point only), its frame, stored samples and journal give the core the report's data and facts,
  /// the site's relay answers the analysis (Opus), and the core checks it and writes the answer
  /// (`{"analysis", "data", "facts"}`, which the screen lays out on its print page). A relay that
  /// does not let the passphrase in leaves the answer without an API key. The browser keeps no
  /// report: each request asks the relay again.
  async pdfReport(projectId, runId, body, llm = null) {
    const files = await store.runFiles(this.wasm, this.tree, projectId, runId);
    let plan;
    try {
      if (!llm) return core(() => this.wasm.pdfReport(projectId, runId, body ?? undefined, files));
      plan = core(() => this.wasm.pdfPlan(projectId, runId, body ?? undefined, files));
    } finally {
      files.free();
    }
    try {
      const minute = plan.minute();
      for (const record of JSON.parse(plan.recordsJson())) {
        const journal = (await this.tree.read([projectId, "runs", runId, "branches", record.branch_id, "journal.jsonl"])) ?? new Uint8Array(0);
        // A Run of other model versions is read at its saved points only (D2).
        const exact = journal.length > 0 && (await this.viewOf(projectId, runId, record)) !== null;
        if (exact) {
          const saved = await this.savedMinuteAt(projectId, runId, record, minute);
          core(() => plan.checkSavedPoint(saved));
        }
        const simulation = await this.simulationAt(projectId, runId, record, minute, { exact });
        const frame = decoder.decode(simulation.frame(runId, record.branch_id, record.computed_until, undefined));
        const table = await readSamples(this.wasm, this.tree, projectId, runId, record.branch_id);
        try {
          core(() => plan.addBranch(simulation, frame, table, journal));
        } finally {
          table.free();
        }
      }
      const scenario = await store.runScenarioBytes(this.tree, projectId, runId);
      const analysis = core(() => plan.analysis(scenario));
      try {
        const reply = await askRelay(this.relay, llm, core(() => analysis.task()));
        if (reply === null) {
          const again = await store.runFiles(this.wasm, this.tree, projectId, runId);
          try {
            return core(() => this.wasm.pdfReport(projectId, runId, body ?? undefined, again));
          } finally {
            again.free();
          }
        }
        return core(() => analysis.report(core(() => analysis.read(reply))));
      } finally {
        analysis.free();
      }
    } finally {
      plan.free();
    }
  }

  /// `POST runs/{run}/delete` (web/projects.py's `ProjectAPI.delete_run`): the core's
  /// checks (the Run's id as the confirmation, no computation of it running, a status that is not
  /// running); then its folder is removed and what this API keeps of it is let go.
  async deleteRun(projectId, runId, body) {
    const active = [...this.computing.jobs.values()].some((job) => job.projectId === projectId && job.runId === runId);
    const files = await store.runFiles(this.wasm, this.tree, projectId, runId);
    let answer;
    try {
      answer = core(() => this.wasm.deleteAnswer(projectId, runId, body ?? undefined, active, files));
    } finally {
      files.free();
    }
    for (const [key, { simulation }] of this.live) {
      if (key.startsWith(`${projectId}/${runId}/`)) {
        simulation.free();
        this.live.delete(key);
      }
    }
    for (const key of [...this.views.keys()]) if (key.startsWith(`${projectId}/${runId}/`)) this.views.delete(key);
    this.history.clear();
    this.forkOutcomes.delete(`${projectId}/${runId}`);
    await store.removeRun(this.tree, projectId, runId);
    return answer;
  }

  /// The record of a branch with computed minutes (the core's `branch_of`); an error, as the
  /// server's, for one there is not.
  async branch(projectId, runId, branchId) {
    const files = await store.runFiles(this.wasm, this.tree, projectId, runId);
    try {
      return JSON.parse(core(() => this.wasm.branchOf(projectId, runId, files, branchId)));
    } finally {
      files.free();
    }
  }

  /// `replay-frames` of one branch (the core's checks, in the server's order, `ReplayPlan`); two
  /// branches are a comparison (stage C).
  async replay(projectId, runId, search) {
    const files = await store.runFiles(this.wasm, this.tree, projectId, runId);
    let plan;
    try {
      plan = core(() => this.wasm.replayPlan(search, projectId, runId, files));
    } finally {
      files.free();
    }
    try {
      if (plan.branchIds().length === 2) return await replayPair(this, projectId, runId, plan);
      const [record] = JSON.parse(plan.recordsJson());
      const samples = new this.wasm.Files();
      try {
        for (let minute = plan.start(); minute <= plan.end(); minute += 1) samples.addText("", await this.frameText(projectId, runId, record, minute, { replay: true }));
        return core(() => plan.answer(samples));
      } finally {
        samples.free();
      }
    } finally {
      plan.free();
    }
  }

  /// The frame of a branch at a minute, as JSON text; with `replay`, as `replay-frames` gives it,
  /// without the world's digest (`Simulation.replayFrame`, #500). A Run computed with other model
  /// versions is not computed again with the core's (D2 of the model versions): the frame is the
  /// saved point's at or before the minute, with `model_view` and, where the minute is not a saved
  /// point's, `saved_point_rounding` (the core's `ModelStatus::describe_json`).
  async frameText(projectId, runId, record, minute, { replay = false } = {}) {
    const frameOf = (simulation) =>
      replay ? simulation.replayFrame(runId, record.branch_id, record.computed_until) : simulation.frame(runId, record.branch_id, record.computed_until, undefined);
    const view = await this.viewOf(projectId, runId, record);
    if (view !== null) {
      const saved = await this.savedMinuteAt(projectId, runId, record, minute);
      const simulation = await this.simulationAt(projectId, runId, record, saved, { exact: true });
      const fields = this.wasm.viewFields(view.recorded, view.installed, saved, minute);
      return this.wasm.mergeJson(frameOf(simulation), encoder.encode(fields));
    }
    // The head's frame kept while it computes has the digest; a replay's has none.
    const kept = replay ? null : this.computing.frameAt(projectId, runId, record, minute);
    if (kept !== null) {
      this.stats.headFrames += 1;
      return kept;
    }
    const simulation = await this.simulationAt(projectId, runId, record, minute);
    const started = performance.now();
    const bytes = frameOf(simulation);
    this.stats.frameMs += performance.now() - started;
    return decoder.decode(bytes);
  }

  /// The model versions of a branch's stored Run against the core's, where they differ (the Run
  /// is view-only): `{ recorded, installed }` as `viewFields` takes them; null for a Run of the
  /// core's versions. Kept for the branch's head.
  async viewOf(projectId, runId, record) {
    const key = `${projectId}/${runId}/${record.branch_id}`;
    const kept = this.views.get(key);
    if (kept && kept.head === record.checkpoint_hash) return kept.view;
    const compared = await branchModelStatus(this.wasm, this.tree, projectId, runId, record.branch_id);
    const view = compared === null || compared.status.current ? null : { recorded: compared.recorded, installed: compared.installed };
    this.views.set(key, { head: record.checkpoint_hash, view });
    return view;
  }

  /// The minute of the saved point at or before `minute` of a branch's stored Run (the core's
  /// `Journal::saved_minute_at`).
  async savedMinuteAt(projectId, runId, record, minute) {
    const journal = new this.wasm.Journal(await this.tree.read([projectId, "runs", runId, "branches", record.branch_id, "journal.jsonl"]));
    try {
      return journal.savedMinuteAt(minute);
    } finally {
      journal.free();
    }
  }

  /// The branch's live Simulation at `minute`: stepped forward from the kept one when that does
  /// not cost more ticks than restoring the nearest saved point, else sought. With `exact` (a saved
  /// point of a Run of other model versions, which is not computed again) the kept one is used
  /// only when it is the saved point as restored, else the saved point is restored.
  async simulationAt(projectId, runId, record, minute, { exact = false } = {}) {
    const key = `${projectId}/${runId}/${record.branch_id}`;
    let live = this.live.get(key);
    if (live && (live.head !== record.checkpoint_hash || (exact && !(live.restoredAt === minute && live.simulation.minute() === minute)))) {
      live.simulation.free();
      this.live.delete(key);
      live = undefined;
    }
    this.used += 1;
    if (live) {
      live.used = this.used;
      const now = live.simulation.minute();
      const point = Math.max(0, ...live.savedMinutes.filter((saved) => saved <= minute));
      if (now === minute) return live.simulation;
      live.restoredAt = null;
      if (now < minute && (now >= point || minute - now <= SAVE_EVERY)) {
        const started = performance.now();
        if (live.simulation.advance(minute - now) !== minute - now) throw new Error(`the Run does not reach minute ${minute}`);
        this.stats.steps += minute - now;
        this.stats.stepMs += performance.now() - started;
        return live.simulation;
      }
      // Free the kept world before the sought one is made: WASM's memory does not shrink.
      live.simulation.free();
      this.live.delete(key);
    }
    while (this.live.size >= this.keep) {
      const [oldest] = [...this.live.entries()].sort((a, b) => a[1].used - b[1].used);
      oldest[1].simulation.free();
      this.live.delete(oldest[0]);
    }
    const reader = await store.branchReader(this.wasm, this.tree, projectId, runId, record.branch_id);
    try {
      const sought = await reader.seek(minute);
      this.stats.seeks += 1;
      this.stats.restoreMs += sought.restoreMs;
      this.stats.recomputeMs += sought.recomputeMs;
      this.live.set(key, {
        simulation: sought.simulation,
        head: record.checkpoint_hash,
        savedMinutes: reader.savedPoints().map((line) => line.minute),
        restoredAt: sought.from === minute ? minute : null,
        used: this.used,
      });
      return sought.simulation;
    } finally {
      reader.close();
    }
  }

  /// Frees every kept Simulation.
  freeLive() {
    for (const { simulation } of this.live.values()) simulation.free();
    this.live.clear();
  }

  /// Frees every kept Simulation and sample table.
  close() {
    this.freeLive();
    this.history.clear();
  }
}

// ---- The site's sample -------------------------------------------------------------------------

/// Checks a sample manifest's form; an error otherwise.
export function checkSampleManifest(manifest) {
  const bad = (message) => {
    throw new Error(`not a sample manifest: ${message}`);
  };
  if (manifest?.format !== SAMPLE_FORMAT || manifest.version !== SAMPLE_VERSION) bad("another format or version");
  if (typeof manifest.project_id !== "string" || !/^[a-f0-9]{64}$/.test(manifest.sha256 ?? "") || !Number.isSafeInteger(manifest.bytes)) bad("no project, sha256 or size");
  if (!Array.isArray(manifest.parts) || manifest.parts.length === 0) bad("no parts");
  let total = 0;
  for (const part of manifest.parts) {
    if (typeof part?.path !== "string" || !/^[A-Za-z0-9._-]+$/.test(part.path) || !/^[a-f0-9]{64}$/.test(part.sha256 ?? "") || !Number.isSafeInteger(part.bytes)) bad("a part without its name, size or sha256");
    total += part.bytes;
  }
  if (total !== manifest.bytes) bad("the parts' sizes do not add up to the bundle's");
  return manifest;
}

/// Checks the site's list of examples; an error otherwise.
export function checkExamples(list) {
  const bad = (message) => {
    throw new Error(`not a list of examples: ${message}`);
  };
  if (list?.format !== EXAMPLES_FORMAT || list.version !== EXAMPLES_VERSION || !Array.isArray(list.examples)) bad("another format or version");
  const seen = new Set();
  for (const example of list.examples) {
    if (typeof example?.manifest !== "string" || !/^[A-Za-z0-9._-]+$/.test(example.manifest)) bad("an example without its manifest");
    if (typeof example.project_id !== "string" || seen.has(example.project_id)) bad("an example without its own Project");
    if (!/^[a-f0-9]{64}$/.test(example.sha256 ?? "") || !Number.isSafeInteger(example.bytes)) bad("an example without its sha256 or size");
    seen.add(example.project_id);
  }
  return list;
}

/// The examples a store's marker names, as { <project id>: <sha256> }; the marker before #411
/// named one.
export function markerExamples(marker) {
  if (marker && typeof marker.examples === "object" && marker.examples !== null) return { ...marker.examples };
  if (typeof marker?.project_id === "string" && typeof marker.sha256 === "string") return { [marker.project_id]: marker.sha256 };
  return {};
}

/// Whether the store holds the site's example `example` ({ project_id, sha256 }) as the home can
/// open it: the marker (`held`, markerExamples) names its bundle, its Project is there and holds
/// a Run. An example whose Runs were all deleted on the home (#440) is not held: the home offers
/// to fetch it again, and fetching imports it again over its Project.
export async function exampleHeld(tree, example, held) {
  if (held[example.project_id] !== example.sha256) return false;
  if ((await tree.read([example.project_id, "project.json"])) === null) return false;
  return (await store.runIds(tree, example.project_id)).length > 0;
}

/// Makes sure the store holds the site's sample: when the store's marker does not name the
/// manifest's bundle (or the Project is missing), fetches the parts (`fetchBytes(path)` →
/// Uint8Array, the path relative to the manifest), checks each and the whole against their sha256,
/// and imports the bundle, replacing a Project of the same id. `progress({ phase, bytes, total })`
/// is told as it goes. Returns `{ installed, projectId, ms }`.
export async function installSample({ wasm, tree, manifest, fetchBytes, readMarker, writeMarker, progress = () => {} }) {
  checkSampleManifest(manifest);
  const marker = await readMarker();
  if (marker?.sha256 === manifest.sha256 && (await tree.read([manifest.project_id, "project.json"])) !== null) {
    return { installed: false, projectId: manifest.project_id, ms: 0 };
  }
  const started = performance.now();
  const parts = [];
  let bytes = 0;
  progress({ phase: "fetch", bytes, total: manifest.bytes });
  for (const part of manifest.parts) {
    const got = await fetchBytes(part.path);
    if (got.length !== part.bytes || (await store.sha256Hex(got)) !== part.sha256) throw new Error(`the sample's part ${part.path} is not the one its manifest lists`);
    parts.push(got);
    bytes += got.length;
    progress({ phase: "fetch", bytes, total: manifest.bytes });
  }
  const whole = new Uint8Array(bytes);
  let at = 0;
  for (const part of parts.splice(0)) {
    whole.set(part, at);
    at += part.length;
  }
  if ((await store.sha256Hex(whole)) !== manifest.sha256) throw new Error("the sample's parts do not join into the bundle its manifest lists");
  progress({ phase: "import", bytes, total: manifest.bytes });
  const replace = (await tree.read([manifest.project_id, "project.json"])) !== null;
  const imported = await store.importProject({ wasm, tree, bundle: whole, replace });
  if (imported.projectId !== manifest.project_id) throw new Error("the sample's bundle is of another Project");
  await writeMarker(encoder.encode(`${JSON.stringify({ sha256: manifest.sha256, project_id: manifest.project_id })}\n`));
  progress({ phase: "done", bytes, total: manifest.bytes });
  return { installed: true, projectId: manifest.project_id, ms: performance.now() - started };
}
