// Computing a Run in the browser (Issue #267; js/compute.mjs through js/browser_api.mjs).
//
// Usage: node native/wasm/tests/node_compute.mjs PKG_DIR [BUNDLE]
//   PKG_DIR: the output of `wasm-bindgen --target nodejs` for sheltersim_wasm.wasm.
//   BUNDLE:  a sample bundle (scripts/build_project_sample.py --bundle); then its Run is computed again in
//            the browser's way instead of the fixture's (100 people, 7 days: about a minute).
//
// Without BUNDLE, a Project is made in a store in memory with one Run computed from the world
// `small_replay` of native/core/tests/data/tick_core.json (one day, saved points every 6 hours), as
// the sample. In both cases:
// - The Run to compute is made beside the sample's (compute_run.mjs): its records, the sample's
//   "export" piece as its bytes are, computed to minute 0.
// - `resume` and `cancel` answer Python's checks and JSON; the Run is computed in chunks while
//   the records, timeline and history answer between them; the screen's `timeline` of each new
//   head is the kept frame; cancelling stops at a chunk's end with a saved point there.
// - A computation stopped in the middle (the page closed: the Worker never comes back, a line cut
//   short at the journal's end) leaves a Run whose records read and whose head is readable; the
//   next start calls it `interrupted`, and resuming goes on from its head.
// - Computed so in pieces, the Run's stored world, log and samples read back to the digests of
//   the sample's Run (same Scenario, same seed), and its frames are the sample's.
// - A Run computed with another model version is view-only: resume is refused with Python's text;
//   its timeline and replay show its saved points, a minute between them rounded down as Python's
//   D2 says (#368).
// The store emulates OPFS's rule that a file has one access handle open at a time. Large bytes are
// compared by their sha256 (a failing deepEqual of large arrays takes all the memory).

import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const pkg = process.argv[2];
if (!pkg) {
  console.error("usage: node node_compute.mjs PKG_DIR [BUNDLE]");
  process.exit(2);
}
const bundlePath = process.argv[3] ?? null;
const require = createRequire(import.meta.url);
const wasm = require(join(resolve(pkg), "sheltersim_wasm.js"));
const here = dirname(fileURLToPath(import.meta.url));
const store = await import(join(here, "..", "js", "project_store.mjs"));
const api = await import(join(here, "..", "js", "browser_api.mjs"));
const compute = await import(join(here, "..", "js", "compute.mjs"));
const { COMPUTE_RUN, makeComputeRun } = await import(join(here, "compute_run.mjs"));
const runs = await import(join(here, "..", "js", "run_store.mjs"));
const encoder = new TextEncoder();
const decoder = new TextDecoder();
const sha = (bytes) => wasm.sha256Hex(typeof bytes === "string" ? encoder.encode(bytes) : bytes);
const RUN = COMPUTE_RUN;

let failures = 0;
const check = (ok, message) => {
  if (!ok) {
    console.log(`  ${message}`);
    failures += 1;
  }
};

/// A memory store whose files, as OPFS's, have one access handle open at a time.
function exclusive(tree) {
  const open = new Set();
  const folder = tree.folder.bind(tree);
  tree.folder = async (path) => {
    const backend = await folder(path);
    return {
      async open(name, options) {
        const key = [...path, name].join("/");
        if (open.has(key)) throw new Error(`NoModificationAllowedError: ${key} is open`);
        const file = await backend.open(name, options);
        open.add(key);
        return { ...file, close: () => open.delete(key) };
      },
    };
  };
  tree.openFiles = open;
  return tree;
}

/// Web Locks in memory: a page closed lets its locks go (`dropAll`).
function locks() {
  const held = new Set();
  return {
    held,
    branchLock: async (name) => {
      if (held.has(name)) return null;
      held.add(name);
      return () => held.delete(name);
    },
    dropAll: () => held.clear(),
  };
}

// ---- The sample --------------------------------------------------------------------------------

const tree = exclusive(store.memoryTree());
let PROJECT;
let SAMPLE;
if (bundlePath === null) {
  PROJECT = "p1";
  SAMPLE = "r-sample";
  const raw = readFileSync(join(here, "..", "..", "core", "tests", "data", "tick_core.json"));
  const probe = wasm.Simulation.fromWorlds(raw, "small_replay");
  const duration = probe.duration();
  probe.free();
  await store.createProject(tree, PROJECT);
  const scenarioBytes = encoder.encode(`{"duration_minutes":${duration},"id":"trial","seed":7}`);
  await store.saveScenario(tree, PROJECT, { scenarioBytes, scenarioHash: await store.sha256Hex(scenarioBytes) });
  // The core cannot sign this Scenario: only the engine's items are compared (compute.mjs).
  const modelVersions = { ...JSON.parse(wasm.engineSignatures()), m: "1" };
  await store.createRun(tree, PROJECT, { runId: SAMPLE, scenarioId: "trial", scenarioVersion: 1, seed: 7, modelVersions, evidenceBytes: encoder.encode('{"schema_version":1,"weight":0.50}') });
  await store.computeBranch({ wasm, tree, projectId: PROJECT, runId: SAMPLE, branchId: "main", run: { exportBytes: raw, world: "small_replay", ticks: 120 } });
  await store.setRunStatus(tree, PROJECT, SAMPLE, "completed");
} else {
  const imported = await store.importProject({ wasm, tree, bundle: new Uint8Array(readFileSync(bundlePath)) });
  PROJECT = imported.projectId;
  SAMPLE = JSON.parse(await store.runsResponse(wasm, tree, PROJECT)).runs[0].run_id;
}
const sampleRecord = await store.branchRecord(tree, PROJECT, SAMPLE, "main");
const DURATION = sampleRecord.computed_until;
const sampleBackend = await store.branchBackend(tree, PROJECT, SAMPLE, "main");
const sampleEnd = await (async () => {
  const reader = await runs.RunReader.open({ wasm, backend: sampleBackend });
  try {
    return { end: reader.end(), run: reader.run, exportLine: reader.lines.find((line) => line.kind === "export") };
  } finally {
    reader.close();
  }
})();
const SAVE = sampleEnd.run.save_every;

// ---- The Run to compute ------------------------------------------------------------------------

const made = await makeComputeRun({ wasm, tree, projectId: PROJECT });
check(made?.runId === RUN, `the Run to compute was not made: ${JSON.stringify(made)}`);
{
  const sampleRun = JSON.parse(await store.runResponse(wasm, tree, PROJECT, SAMPLE));
  const run = JSON.parse(await store.runResponse(wasm, tree, PROJECT, RUN));
  const { branches, ...rest } = run;
  const { branches: _, ...sampleRest } = sampleRun;
  check(JSON.stringify({ ...rest, run_id: SAMPLE, status: sampleRest.status }) === JSON.stringify(sampleRest), `the Run's record is not the sample's with its id: ${JSON.stringify(rest)}`);
  check(run.status === "queued" && branches.length === 1, `the new Run: ${run.status}, ${branches.length} branches`);
  const [main] = branches;
  check(main.branch_id === "main" && main.computed_until === 0 && main.fork_minute === 0 && main.parent_branch_id === null && main.status === "queued" && /^[0-9a-f]{64}$/.test(main.checkpoint_hash), `the new branch: ${JSON.stringify(main)}`);
  check(sha(await tree.read([PROJECT, "runs", RUN, "evidence.json"])) === sha(await tree.read([PROJECT, "runs", SAMPLE, "evidence.json"])), "the evidence is not the sample's");
  const data = await tree.read([PROJECT, "runs", RUN, "branches", "main", "data.pack"]);
  const sampleData = await tree.read([PROJECT, "runs", SAMPLE, "branches", "main", "data.pack"]);
  const line = sampleEnd.exportLine;
  check(data.length === line.length && sha(data) === sha(sampleData.slice(line.offset, line.offset + line.length)), "the export piece is not the sample's bytes");
}

// ---- Answers -----------------------------------------------------------------------------------

const lockSet = locks();
let halted = false;
const pause = () => (halted ? new Promise(() => {}) : new Promise((resolve) => setImmediate(resolve)));
const chunkTicks = bundlePath === null ? 60 : 120;
let browser = new api.BrowserApi({ wasm, tree, branchLock: lockSet.branchLock, chunkTicks, pause });
const base = `/api/v1/projects/${PROJECT}/runs/${RUN}`;
const get = (path, search = "") => browser.answer({ method: "GET", path: `${base}${path}`, search });
const post = (path, body) => browser.answer({ method: "POST", path: `${base}${path}`, body: typeof body === "string" ? body : JSON.stringify(body) });
const record = async () => JSON.parse((await get("")).body).branches.find((branch) => branch.branch_id === "main");
const runStatus = async () => JSON.parse((await get("")).body).status;
// Python's refusals (src/sheltersim/web/projects.py): a StorageError's text and code, 409.
const refusal = async (path, body, status, error, code = status === 409 ? "storage_conflict" : null) => {
  const answer = await post(path, body);
  const document = JSON.parse(answer.body);
  check(answer.status === status && document.error === error && (document.code ?? null) === code && (code !== null || !("code" in document)), `${path} ${JSON.stringify(body)}: ${answer.status} ${answer.body}`);
};
/// Waits (answering requests meanwhile) until the Run's status is not `running`.
async function settle(browserApi = browser) {
  for (let polls = 0; polls < 1_000_000; polls++) {
    const { body } = await browserApi.answer({ method: "GET", path: base, search: "" });
    if (JSON.parse(body).status !== "running") return JSON.parse(body);
    await new Promise((resolve) => setImmediate(resolve));
  }
  throw new Error("the computation did not end");
}

// The frames of the sample's Run (computed without stopping), for the Run to compute.
async function sampleFrame(minute, computedUntil) {
  const reader = await runs.RunReader.open({ wasm, backend: sampleBackend });
  try {
    const { simulation } = await reader.seek(minute);
    try {
      return sha(simulation.frame(RUN, "main", computedUntil, undefined));
    } finally {
      simulation.free();
    }
  } finally {
    reader.close();
  }
}

// Python's refusals, before anything computes.
const RANGE = "target minute must be in the uncomputed remaining range";
await refusal("/resume", { branch_id: "main", target_minute: DURATION + 1 }, 409, RANGE);
await refusal("/resume", { branch_id: "main", target_minute: 0 }, 409, RANGE);
await refusal("/resume", { branch_id: "main", target_minute: "10" }, 409, RANGE);
await refusal("/resume", { branch_id: "main", target_minute: 1.5 }, 409, RANGE);
await refusal("/resume", { branch_id: "main" }, 409, "request fields do not match this operation");
await refusal("/resume", { branch_id: "main", target_minute: 10, extra: 1 }, 409, "request fields do not match this operation");
await refusal("/resume", "not json", 400, "invalid project request");
await refusal("/resume", "[1]", 409, "project request must be an object");
await refusal("/resume", { branch_id: "nope", target_minute: 10 }, 409, "saved artifact is missing or corrupt");
await refusal("/cancel", { branch_id: "main", extra: 1 }, 409, "request fields do not match this operation");
{
  const answer = await post("/cancel", {});
  check(answer.status === 200 && answer.body === (await store.runResponse(wasm, tree, PROJECT, RUN)), `cancel with nothing computing: ${answer.status} ${answer.body}`);
}
// Another tab computes the branch: it holds the branch's lock and the records say running.
{
  const name = compute.lockName(PROJECT, RUN, "main");
  lockSet.held.add(name);
  await refusal("/resume", { branch_id: "main", target_minute: 10 }, 409, "run_worker_busy: another process writes this branch", "run_worker_busy");
  const before = await store.branchRecord(tree, PROJECT, RUN, "main");
  await store.writeBranch(tree, PROJECT, RUN, { ...before, status: "running" });
  await store.setRunStatus(tree, PROJECT, RUN, "running");
  await refusal("/cancel", {}, 409, "run_worker_busy: cancellation must reach the owning worker", "run_worker_busy");
  check((await compute.recoverInterrupted({ tree, branchLock: lockSet.branchLock })).length === 0, "a branch another tab computes was called interrupted");
  lockSet.held.delete(name);
  await store.writeBranch(tree, PROJECT, RUN, before);
  await store.setRunStatus(tree, PROJECT, RUN, "queued");
}
// Minute 0 reads back.
{
  const answer = await get("/branches/main/timeline", "?minute=0");
  check(answer.status === 200 && sha(answer.body) === (await sampleFrame(0, 0)), `timeline at 0: ${answer.status}`);
}

// 1. To a target between saved points, the screen polling the Run and its head's frame meanwhile.
const first = SAVE + 140;
{
  const answer = await post("/resume", { branch_id: "main", target_minute: first });
  check(answer.status === 200 && answer.body === `{"branch_id":"main","run_id":"${RUN}","status":"running","target_minute":${first}}`, `resume: ${answer.status} ${answer.body}`);
  const running = JSON.parse((await get("")).body);
  check(running.status === "running" && running.branches[0].status === "running", `while computing: ${running.status} / ${running.branches[0].status}`);
  await refusal("/resume", { branch_id: "main", target_minute: DURATION }, 409, "background worker is busy; wait or pause the active run");
  let heads = 0;
  for (;;) {
    const { status, branches } = JSON.parse((await get("")).body);
    const [main] = branches;
    if (main.computed_until > 0 && status === "running") {
      const frame = await get("/branches/main/timeline", `?minute=${main.computed_until}`);
      check(frame.status === 200, `timeline at the head ${main.computed_until}: ${frame.status} ${frame.body.slice(0, 200)}`);
      if ((await record()).computed_until === main.computed_until) {
        check(sha(frame.body) === (await sampleFrame(main.computed_until, main.computed_until)), `timeline at the head ${main.computed_until}: not the sample's frame`);
      }
      heads += 1;
    }
    if (status !== "running") break;
    await new Promise((resolve) => setImmediate(resolve));
  }
  const after = JSON.parse((await get("")).body);
  check(after.status === "paused" && after.branches[0].status === "paused" && after.branches[0].computed_until === first, `at the target: ${after.status} ${JSON.stringify(after.branches[0])}`);
  check(heads > 0 && browser.stats.headFrames > 0, `the head's frame was not answered while computing (${heads} polls, ${browser.stats.headFrames} kept frames)`);
  check(tree.openFiles.size === 0, `files left open: ${[...tree.openFiles]}`);
}
// Seek, step and history on the paused Run.
for (const minute of [first, 1, SAVE - 1, SAVE, SAVE + 1]) {
  const answer = await get("/branches/main/timeline", `?minute=${minute}`);
  check(answer.status === 200 && sha(answer.body) === (await sampleFrame(minute, first)), `paused, timeline at ${minute}: ${answer.status}`);
}
{
  const history = await get("/branches/main/history", `?kind=kpis&start=0&end=${first}&step=30&limit=100`);
  check(history.status === 200 && JSON.parse(history.body).items.length > 0, `paused, history: ${history.status} ${history.body.slice(0, 200)}`);
}

// 2. Resume to the end and cancel after a few heads: it stops at a chunk's end, a saved point there.
{
  const answer = await post("/resume", { branch_id: "main", target_minute: DURATION });
  check(answer.status === 200, `resume to the end: ${answer.body}`);
  while ((await record()).computed_until < 2 * SAVE) await new Promise((resolve) => setImmediate(resolve));
  const cancelled = await post("/cancel", { branch_id: "main" });
  check(cancelled.status === 200 && JSON.parse(cancelled.body).run_id === RUN, `cancel: ${cancelled.status} ${cancelled.body}`);
  const ended = await settle();
  const main = ended.branches[0];
  check(ended.status === "paused" && main.status === "paused" && main.computed_until >= 2 * SAVE && main.computed_until < DURATION, `cancelled: ${ended.status} ${JSON.stringify(main)}`);
  const at = await get("/branches/main/timeline", `?minute=${main.computed_until}`);
  check(at.status === 200 && sha(at.body) === (await sampleFrame(main.computed_until, main.computed_until)), `cancelled, timeline at ${main.computed_until}: ${at.status}`);
}

// 3. Resume, and close the page in the middle: the Worker stops between chunks and never comes
// back, and a line is cut short at the journal's end.
let stoppedAt;
{
  const answer = await post("/resume", { branch_id: "main", target_minute: DURATION });
  check(answer.status === 200, `resume again: ${answer.body}`);
  const from = (await record()).computed_until;
  while ((await record()).computed_until < from + SAVE) await new Promise((resolve) => setImmediate(resolve));
  halted = true;
  // The chunk in hand ends; then nothing more runs.
  for (let spin = 0; spin < 50; spin++) await new Promise((resolve) => setImmediate(resolve));
  const backend = await store.branchBackend(tree, PROJECT, RUN, "main");
  const journal = await backend.open("journal.jsonl");
  journal.write(encoder.encode('{"kind":"log_block","offset":1'), journal.size());
  journal.close();
  stoppedAt = (await store.branchRecord(tree, PROJECT, RUN, "main")).computed_until;
  // The page's locks go with it; a new page starts.
  lockSet.dropAll();
  halted = false;
  browser = new api.BrowserApi({ wasm, tree, branchLock: lockSet.branchLock, chunkTicks, pause });
  const recovered = await compute.recoverInterrupted({ tree, branchLock: lockSet.branchLock });
  check(recovered.join() === `${PROJECT}/${RUN}/main`, `recovered: ${recovered}`);
  const run = JSON.parse((await get("")).body);
  const [main] = run.branches;
  check(run.status === "interrupted" && main.status === "interrupted" && main.computed_until === stoppedAt, `after the page closed: ${run.status} ${JSON.stringify(main)}`);
  const at = await get("/branches/main/timeline", `?minute=${stoppedAt}`);
  check(at.status === 200 && sha(at.body) === (await sampleFrame(stoppedAt, stoppedAt)), `after the page closed, timeline at ${stoppedAt}: ${at.status}`);
  const history = await get("/branches/main/history", `?kind=kpis&start=0&end=${stoppedAt}&step=60&limit=100`);
  check(history.status === 200, `after the page closed, history: ${history.status} ${history.body.slice(0, 200)}`);
}
// 4. On to the end from the head, in the new page.
{
  const answer = await post("/resume", { branch_id: "main", target_minute: DURATION });
  check(answer.status === 200, `resume after the page closed: ${answer.body}`);
  const ended = await settle();
  check(ended.status === "completed" && ended.branches[0].status === "completed" && ended.branches[0].computed_until === DURATION, `at the end: ${ended.status} ${JSON.stringify(ended.branches[0])}`);
  await refusal("/resume", { branch_id: "main", target_minute: DURATION }, 409, RANGE);
  check(tree.openFiles.size === 0, `files left open: ${[...tree.openFiles]}`);
}

// The stored Run reads back to the sample's digests, and its frames are the sample's.
{
  const backend = await store.branchBackend(tree, PROJECT, RUN, "main");
  const back = await runs.readAndVerify({ wasm, backend });
  check(back.complete && back.verified && !back.torn, `the computed Run does not read back: ${JSON.stringify(back.mismatches)} torn ${back.torn}`);
  const reader = await runs.RunReader.open({ wasm, backend });
  const end = reader.end();
  const saved = reader.savedPoints().map((line) => line.minute);
  reader.close();
  check(JSON.stringify(end.parts) === JSON.stringify(sampleEnd.end.parts), "the world's digests at the end are not the sample's");
  check(end.metrics === sampleEnd.end.metrics && end.records === sampleEnd.end.records && end.samples === sampleEnd.end.samples, "the metrics' digest or the counts are not the sample's");
  const head = (await record()).checkpoint_hash;
  check(head === store.journalHead(wasm, decoder.decode(await tree.read([PROJECT, "runs", RUN, "branches", "main", "journal.jsonl"]))).checkpointHash, "the head is not the end line");
  for (let minute = SAVE; minute < DURATION; minute += SAVE) check(saved.includes(minute), `no saved point at ${minute}: ${saved}`);
  check(saved.includes(first) && saved.includes(DURATION), `saved points: ${saved}`);
  for (const minute of [0, 1, SAVE, first, stoppedAt, stoppedAt + 1, DURATION - 1, DURATION]) {
    const answer = await get("/branches/main/timeline", `?minute=${minute}`);
    check(answer.status === 200 && sha(answer.body) === (await sampleFrame(minute, DURATION)), `completed, timeline at ${minute}: ${answer.status}`);
  }
  const ours = JSON.parse((await get("/branches/main/history", `?kind=kpis&start=0&end=${DURATION}&step=60&limit=100`)).body);
  const theirs = JSON.parse((await browser.answer({ method: "GET", path: `/api/v1/projects/${PROJECT}/runs/${SAMPLE}/branches/main/history`, search: `?kind=kpis&start=0&end=${DURATION}&step=60&limit=100` })).body);
  const items = (page, run) => JSON.stringify(page.items).replaceAll(`"${run}"`, '"RUN"');
  check(items(ours, RUN) === items(theirs, SAMPLE) && ours.coverage?.count === theirs.coverage?.count, "the history is not the sample's");
  console.log(`saved points ${saved.length}; computed ${JSON.stringify(browser.computing.done.map(({ from, target, reached, status, ms }) => ({ from, target, reached, status, s: +(ms / 1000).toFixed(2) })))}`);
}
// A Run computed with another model version (engine 1: no engine items recorded) is view-only.
{
  const raw = bundlePath === null ? readFileSync(join(here, "..", "..", "core", "tests", "data", "tick_core.json")) : null;
  if (raw !== null) {
    const old = "r-old-model";
    await store.createRun(tree, PROJECT, { runId: old, scenarioId: "trial", scenarioVersion: 1, seed: 7, modelVersions: { m: "1" }, evidenceBytes: encoder.encode('{"schema_version":1}') });
    await store.computeBranch({ wasm, tree, projectId: PROJECT, runId: old, branchId: "main", run: { exportBytes: raw, world: "small_replay", ticks: 720, saveEvery: 720 } });
    const branch = await store.writeBranch(tree, PROJECT, old, { ...(await store.branchRecord(tree, PROJECT, old, "main")), status: "paused" });
    const at = `/api/v1/projects/${PROJECT}/runs/${old}`;
    const answer = await browser.answer({ method: "POST", path: `${at}/resume`, body: JSON.stringify({ branch_id: "main", target_minute: branch.computed_until + 1 }) });
    const wanted = "run_model_view_only: このシナリオは旧モデル（版 1）で計算されました。今のモデルは版 3 です。閲覧のみです。保存した時点だけを表示し、続きの計算、分岐、介入の適用はできません。";
    check(answer.status === 409 && answer.body === JSON.stringify({ code: "run_model_view_only", error: wanted }), `a Run of another model version: ${answer.status} ${answer.body}`);
    // It is viewed as Python views it (D2): a minute between saved points shows the saved point at
    // or before it, not computed again, and says so; a saved point's minute is shown as it is.
    const frameOf = async (minute) => {
      const answer = await browser.answer({ method: "GET", path: `${at}/branches/main/timeline`, search: `?minute=${minute}` });
      check(answer.status === 200, `a Run of another model version at ${minute}: ${answer.status} ${answer.body.slice(0, 200)}`);
      return JSON.parse(answer.body);
    };
    const between = await frameOf(1000);
    const view = between.model_view;
    check(
      view?.view_only === true && view.current === false && view.recorded_version === "1" && view.installed_version === "3" && JSON.stringify(view.differences) === '["engine:model"]' && view.message === wanted.slice("run_model_view_only: ".length),
      `model_view: ${JSON.stringify(view)}`,
    );
    check(
      JSON.stringify(between.saved_point_rounding) ===
        JSON.stringify({ message: "旧モデルのRunのため、1000分は計算し直さず、保存した時点（720分）の状態を表示しています。", requested_minute: 1000, saved_minute: 720 }),
      `saved_point_rounding: ${JSON.stringify(between.saved_point_rounding)}`,
    );
    const atSaved = await frameOf(720);
    check(between.minute === 720 && atSaved.minute === 720 && between.state_hash === atSaved.state_hash && !("saved_point_rounding" in atSaved) && atSaved.model_view?.view_only === true, "the saved point's frame");
    const beforeFirst = await frameOf(100);
    check(beforeFirst.minute === 0 && beforeFirst.saved_point_rounding?.saved_minute === 0, `before the first saved point: ${beforeFirst.minute}`);
    // The world shown is the saved one: the sample's (same world, computed straight through) there.
    const sample = JSON.parse((await browser.answer({ method: "GET", path: `/api/v1/projects/${PROJECT}/runs/${SAMPLE}/branches/main/timeline`, search: "?minute=720" })).body);
    check(sample.state_hash === atSaved.state_hash && !("model_view" in sample), "the saved point is the sample's world at 720");
    const replay = await browser.answer({ method: "GET", path: `${at}/replay-frames`, search: "?branch_ids=main&start=718&end=721" });
    check(replay.status === 200, `replay of a Run of another model version: ${replay.status} ${replay.body.slice(0, 300)}`);
    const minutes = (JSON.parse(replay.body).samples ?? []).map((frame) => [frame.minute, frame.saved_point_rounding?.saved_minute ?? null]);
    check(JSON.stringify(minutes) === "[[0,0],[0,0],[720,null],[720,720]]", `replay of a Run of another model version: ${JSON.stringify(minutes)}`);
  }
}
browser.close();

if (failures > 0) {
  console.log(`node_compute: ${failures} failures`);
  process.exit(1);
}
console.log(`node_compute: ok (${bundlePath === null ? "small_replay" : bundlePath}, ${DURATION} minutes)`);
