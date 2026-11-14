// The bytes a Run in the browser stores do not change (Issue #368): small Runs stored with each of
// the browser's ways of writing, compared with the journals and data.pack the committed fixtures
// hold, and the fixtures of the core's tests of the stored Run (native/core/tests/stored_runs.rs).
//
// Usage: node native/wasm/tests/node_stored_runs.mjs PKG_DIR [--write]
//   PKG_DIR: the output of `wasm-bindgen --target nodejs` for sheltersim_wasm.wasm.
//   --write: write the fixtures again instead of comparing (after a change that is meant to move
//            them: a world of the fixtures, the model, or the format).
//
// Compressing is the host's (js/run_store.mjs: CompressionStream("deflate-raw")), and its bytes may
// differ between zlib's versions, so here a stand-in writes each piece as deflate's stored blocks:
// deflate-raw any DecompressionStream reads, the same bytes everywhere. Then the journals (which
// hold the compressed pieces' offsets and lengths) and data.pack are the same on every machine.
//
// The Runs (native/core/tests/data/stored_runs/<name>.journal.jsonl, and runs.json with each Run's
// data.pack's length and sha256):
//   from_zero    runAndStore of tick_core's `small_replay` (a day), saved points every 360 minutes
//   no_points    the same without saved points (saveEvery 0)
//   v1, v2, v3   the formats still read: no_points's journal with the "run" line of format v1 (as
//                its writer wrote it, without `save_every`), from_zero's with format v2 and v3
//   computed     the Run the site offers to compute (js/compute.mjs): its "run" line and the
//                sample's "export" piece copied, then computed in chunks to a target between saved
//                points (paused) and on to the end
//   fork         a branch made by an intervention (js/interventions.mjs) on interventions.json's
//                world at minute 400, computed to the end
// Each Run also reads back to its digests (`readAndVerify`).

import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const pkg = process.argv[2];
if (!pkg) {
  console.error("usage: node node_stored_runs.mjs PKG_DIR [--write]");
  process.exit(2);
}
const write = process.argv.includes("--write");
const require = createRequire(import.meta.url);
const wasm = require(join(resolve(pkg), "sheltersim_wasm.js"));
const here = dirname(fileURLToPath(import.meta.url));
const data = join(here, "..", "..", "core", "tests", "data");
const out = join(data, "stored_runs");
const encoder = new TextEncoder();
const decoder = new TextDecoder();

/// deflate-raw of stored blocks (RFC 1951, BTYPE 00): at most 65535 bytes each, the last BFINAL.
function storedDeflate(bytes) {
  const blocks = Math.max(1, Math.ceil(bytes.length / 65535));
  const output = new Uint8Array(bytes.length + 5 * blocks);
  let at = 0;
  for (let block = 0; block < blocks; block++) {
    const chunk = bytes.subarray(block * 65535, Math.min(bytes.length, (block + 1) * 65535));
    output[at] = block === blocks - 1 ? 1 : 0;
    output[at + 1] = chunk.length & 0xff;
    output[at + 2] = chunk.length >> 8;
    output[at + 3] = ~chunk.length & 0xff;
    output[at + 4] = (~chunk.length >> 8) & 0xff;
    output.set(chunk, at + 5);
    at += 5 + chunk.length;
  }
  return output;
}

globalThis.CompressionStream = class extends TransformStream {
  constructor(format) {
    if (format !== "deflate-raw") throw new Error(`the stand-in compresses deflate-raw only, not ${format}`);
    const chunks = [];
    super({
      transform(chunk) {
        chunks.push(new Uint8Array(chunk));
      },
      flush(controller) {
        const all = new Uint8Array(chunks.reduce((sum, chunk) => sum + chunk.length, 0));
        let at = 0;
        for (const chunk of chunks) {
          all.set(chunk, at);
          at += chunk.length;
        }
        controller.enqueue(storedDeflate(all));
      },
    });
  }
};

const store = await import(join(here, "..", "js", "project_store.mjs"));
const runs = await import(join(here, "..", "js", "run_store.mjs"));
const api = await import(join(here, "..", "js", "browser_api.mjs"));
const compute = await import(join(here, "..", "js", "compute.mjs"));

let failures = 0;
const check = (ok, message) => {
  if (!ok) {
    console.log(`  ${message}`);
    failures += 1;
  }
};

/// A Run's files: its journal's text and its data.pack.
const made = new Map();

async function verified(name, backend) {
  const back = await runs.readAndVerify({ wasm, backend });
  check(back.complete && back.verified && !back.torn, `${name}: does not read back (${back.mismatches})`);
}

// from_zero, no_points and the formats derived from them.
const tick = readFileSync(join(data, "tick_core.json"));
for (const [name, saveEvery] of [["from_zero", 360], ["no_points", 0]]) {
  const backend = runs.memoryBackend();
  await runs.runAndStore({ wasm, exportBytes: tick, world: "small_replay", backend, ticks: 50, saveEvery });
  await verified(name, backend);
  made.set(name, { journal: decoder.decode(backend.files.get("journal.jsonl")), data: backend.files.get("data.pack") });
}

/// The journal `text` with its "run" line made by `edit` of its document, and its sha256 again.
function withRunLine(text, edit) {
  const [first, ...rest] = text.split("\n");
  const json = JSON.stringify(edit(JSON.parse(first.slice(0, first.lastIndexOf("\t")))));
  return [`${json}\t${wasm.sha256Hex(encoder.encode(json))}`, ...rest].join("\n");
}

made.set("v1", {
  journal: withRunLine(made.get("no_points").journal, ({ save_every: _, ...line }) => ({ ...line, format: "sheltersim-browser-run-v1" })),
  data: made.get("no_points").data,
});
for (const version of ["v2", "v3"]) {
  made.set(version, {
    journal: withRunLine(made.get("from_zero").journal, (line) => ({ ...line, format: `sheltersim-browser-run-${version}` })),
    data: made.get("from_zero").data,
  });
}

// computed: the Run to compute, beside a sample computed with runAndStore.
{
  const tree = store.memoryTree();
  const probe = wasm.Simulation.fromWorlds(tick, "small_replay");
  const duration = probe.duration();
  probe.free();
  await store.createProject(tree, "p1");
  const scenarioBytes = encoder.encode(`{"duration_minutes":${duration},"id":"trial","seed":7}`);
  await store.saveScenario(tree, "p1", { scenarioBytes, scenarioHash: await store.sha256Hex(scenarioBytes) });
  await store.createRun(tree, "p1", { runId: "r-sample", scenarioId: "trial", scenarioVersion: 1, seed: 7, modelVersions: { m: "1" }, evidenceBytes: encoder.encode('{"schema_version":1}') });
  await store.computeBranch({ wasm, tree, projectId: "p1", runId: "r-sample", branchId: "main", run: { exportBytes: tick, world: "small_replay", ticks: 120 } });
  await store.setRunStatus(tree, "p1", "r-sample", "completed");
  await compute.ensureComputeRun({ wasm, tree, projectId: "p1" });
  const browser = new api.BrowserApi({ wasm, tree, chunkTicks: 60 });
  const base = `/api/v1/projects/p1/runs/${compute.COMPUTE_RUN}`;
  for (const target of [360 + 140, duration]) {
    const answer = await browser.answer({ method: "POST", path: `${base}/resume`, body: JSON.stringify({ branch_id: "main", target_minute: target }) });
    check(answer.status === 200, `computed: resume to ${target}: ${answer.body}`);
    for (const job of [...browser.computing.jobs.values()]) await job.finished;
  }
  browser.close();
  const record = await store.branchRecord(tree, "p1", compute.COMPUTE_RUN, "main");
  check(record.status === "completed" && record.computed_until === duration, `computed: ${JSON.stringify(record)}`);
  const backend = await store.branchBackend(tree, "p1", compute.COMPUTE_RUN, "main");
  await verified("computed", backend);
  const folder = ["p1", "runs", compute.COMPUTE_RUN, "branches", "main"];
  made.set("computed", { journal: decoder.decode(await tree.read([...folder, "journal.jsonl"])), data: await tree.read([...folder, "data.pack"]) });
}

// fork: a branch made by an intervention at minute 400, computed to the end.
{
  const raw = readFileSync(join(data, "interventions.json"));
  const fixture = JSON.parse(raw.toString("utf8"));
  const { project_id: projectId, run_id: runId, seed, model_versions: modelVersions } = fixture.run;
  const tree = store.memoryTree();
  await store.createProject(tree, projectId);
  const scenarioBytes = encoder.encode(fixture.scenario_json);
  const scenario = await store.saveScenario(tree, projectId, { scenarioBytes, scenarioHash: await store.sha256Hex(scenarioBytes) });
  await store.createRun(tree, projectId, { runId, scenarioId: scenario.scenario_id, scenarioVersion: scenario.scenario_version, seed, modelVersions, evidenceBytes: encoder.encode('{"schema_version":1}') });
  await store.computeBranch({ wasm, tree, projectId, runId, branchId: "main", run: { exportBytes: raw, world: "interventions", ticks: 60 } });
  await store.setRunStatus(tree, projectId, runId, "completed");
  const browser = new api.BrowserApi({ wasm, tree });
  const base = `/api/v1/projects/${projectId}/runs/${runId}`;
  const post = (path, body) => browser.answer({ method: "POST", path: `${base}${path}`, body: JSON.stringify(body) });
  const preview = JSON.parse((await post("/branches/main/intervention-preview", { minute: 400, text: "7:00 toilet_01 outage" })).body);
  check(preview.can_apply === true, `fork: the preview cannot be applied: ${JSON.stringify(preview).slice(0, 300)}`);
  const applied = await post("/branches/main/intervention-apply", { approved: true, approved_program_hash: preview.approved_program_hash, child_id: "c1", preview_id: preview.preview_id });
  check(applied.status === 200, `fork: apply: ${applied.body.slice(0, 300)}`);
  const resumed = await post("/resume", { branch_id: "c1", target_minute: fixture.run.minutes });
  check(resumed.status === 200, `fork: resume: ${resumed.body}`);
  for (const job of [...browser.computing.jobs.values()]) await job.finished;
  browser.close();
  const backend = await store.branchBackend(tree, projectId, runId, "c1");
  await verified("fork", backend);
  const folder = [projectId, "runs", runId, "branches", "c1"];
  made.set("fork", { journal: decoder.decode(await tree.read([...folder, "journal.jsonl"])), data: await tree.read([...folder, "data.pack"]) });
}

const summary = {};
for (const [name, { journal, data: pack }] of made) {
  summary[name] = { journal_sha256: wasm.sha256Hex(encoder.encode(journal)), data_bytes: pack.length, data_sha256: wasm.sha256Hex(pack) };
}
const summaryText = `${JSON.stringify(summary, null, 2)}\n`;
if (write) {
  mkdirSync(out, { recursive: true });
  for (const [name, { journal }] of made) writeFileSync(join(out, `${name}.journal.jsonl`), journal);
  writeFileSync(join(out, "runs.json"), summaryText);
  console.log(`wrote ${made.size} Runs to ${out}`);
} else {
  for (const [name, { journal }] of made) {
    let committed = null;
    try {
      committed = readFileSync(join(out, `${name}.journal.jsonl`), "utf8");
    } catch {
      check(false, `${name}: no committed journal`);
      continue;
    }
    if (committed !== journal) {
      const lines = journal.split("\n");
      const wanted = committed.split("\n");
      const at = lines.findIndex((line, index) => line !== wanted[index]);
      check(false, `${name}: the journal is not the committed one from line ${at + 1}\n    made:      ${(lines[at] ?? "").slice(0, 300)}\n    committed: ${(wanted[at] ?? "").slice(0, 300)}`);
    }
  }
  const committed = JSON.parse(readFileSync(join(out, "runs.json"), "utf8"));
  for (const [name, entry] of Object.entries(summary)) {
    check(JSON.stringify(committed[name]) === JSON.stringify(entry), `${name}: ${JSON.stringify(entry)}, not the committed ${JSON.stringify(committed[name])}`);
  }
  check(Object.keys(committed).length === Object.keys(summary).length, `runs.json names ${Object.keys(committed)}`);
}

if (failures > 0) {
  console.error(`node_stored_runs: ${failures} problem(s)`);
  process.exit(1);
}
console.log(`node_stored_runs: ok (${[...made.keys()].join(", ")}${write ? ", written" : ", the committed bytes"})`);
