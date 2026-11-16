// The browser's answers to an intervention and its child branch are the Python server's (#306;
// js/interventions.mjs through js/browser_api.mjs).
//
// Usage: node native/wasm/tests/node_interventions.mjs PKG_DIR
//   PKG_DIR: the output of `wasm-bindgen --target nodejs` for sheltersim_wasm.wasm.
//
// native/core/tests/data/interventions.json (tests/support/core_interventions.py) holds a Run the
// Project screen made of a generated Scenario on the Python server (its worker a process, as the
// VM's), the world it started from and the server's answers to `intervention-preview`,
// `intervention-apply`, the Run's record with `last_fork`, and the child's frames and KPI history
// once computed. The Run is computed here in WASM and stored as the browser stores it, and the same
// requests go through BrowserApi: each body, masked as the builder masks it (`preview_id` and the
// branches' heads), has the server's sha256, and a refusal the server's body. Then: the core's
// catalog of operators (`GET {project}/contracts`) and a typed `program` built from it, as the
// screen's builder builds it (#584), are answered; the questions' turns are kept in the branch's
// stored Run, the child's stored Run is of format v6, starts at the fork and reads its parent's
// pieces up to its base (no copy of them), and it reads back to its digests.
//
// Then the questions, previews and analysis reports the server answered with an API key (#470,
// native/core/tests/data/interventions_llm.json) are asked with the relay's passphrase, the relay
// answered here with what the server's Provider wrote: the browser must send the relay the core's
// tasks the server's link got (their sha256) and answer as the server did, except that a preview
// without an accepted plan fails with the reason where the server (its error policy `fallback`)
// read the text with the rules: the browser stops (#580, #584).
// Then what only the browser does: a relay that does not let the passphrase in leaves a question
// without an API key and fails a preview, and one that refuses the call (the day's cap) gives a
// question the fallback with its reason and fails a preview with it.

import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import assert from "node:assert/strict";

const pkg = process.argv[2];
if (!pkg) {
  console.error("usage: node node_interventions.mjs PKG_DIR");
  process.exit(2);
}
const require = createRequire(import.meta.url);
const wasm = require(join(resolve(pkg), "sheltersim_wasm.js"));
const here = dirname(fileURLToPath(import.meta.url));
const store = await import(join(here, "..", "js", "project_store.mjs"));
const { BrowserApi } = await import(join(here, "..", "js", "browser_api.mjs"));
const runStore = await import(join(here, "..", "js", "run_store.mjs"));
const raw = readFileSync(join(here, "..", "..", "core", "tests", "data", "interventions.json"));
const fixture = JSON.parse(raw.toString("utf8"));
const withLlm = JSON.parse(readFileSync(join(here, "..", "..", "core", "tests", "data", "interventions_llm.json"), "utf8"));
const encoder = new TextEncoder();
const decoder = new TextDecoder();

let failures = 0;
function check(ok, message) {
  if (!ok) {
    failures += 1;
    console.error(`FAIL: ${message}`);
  }
}

/// tests/support/core_api_answers.py's `normalize`, the head replaced by "HEAD".
function normalize(body, anchor) {
  const document = JSON.parse(body);
  const cursor = document?.next_cursor;
  if (typeof cursor === "string") {
    const decoded = Buffer.from(cursor, "base64url").toString("utf8");
    const again = Buffer.from(decoded.replaceAll(anchor, "HEAD"), "utf8").toString("base64url");
    const padded = again + "=".repeat((4 - (again.length % 4)) % 4);
    body = body.replace(`"next_cursor":"${cursor}"`, `"next_cursor":"${padded}"`);
  }
  return body.replaceAll(anchor, "HEAD");
}

/// tests/support/core_interventions.py's `mask`.
async function mask(body) {
  let text = body
    .replace(/"preview_id":"[0-9a-f]{32}"/g, '"preview_id":"PREVIEW"')
    .replace(/"checkpoint_hash":"[0-9a-f]{64}"/g, '"checkpoint_hash":"HEAD"')
    .replace(/"context_id":"(c[0-9a-f]{32})-[0-9a-f]{24}"/g, '"context_id":"$1-RANDOM"');
  for (const branch of await store.branches(tree, projectId, runId)) {
    if (branch.checkpoint_hash) text = normalize(text, branch.checkpoint_hash);
  }
  return text;
}

const { project_id: projectId, run_id: runId, child, minutes } = fixture.run;
const tree = store.memoryTree();
await store.createProject(tree, projectId);
const scenarioBytes = encoder.encode(fixture.scenario_json);
const scenario = await store.saveScenario(tree, projectId, { scenarioBytes, scenarioHash: await store.sha256Hex(scenarioBytes) });
await store.createRun(tree, projectId, {
  runId,
  scenarioId: scenario.scenario_id,
  scenarioVersion: scenario.scenario_version,
  seed: fixture.run.seed,
  modelVersions: fixture.run.model_versions,
  // The Run's evidence as the browser writes it, which the server's questions held.
  evidenceBytes: wasm.runEvidence(scenarioBytes),
  createdAt: "2026-11-15T01:30:00Z",
});
let started = performance.now();
const { record } = await store.computeBranch({ wasm, tree, projectId, runId, branchId: "main", run: { exportBytes: raw, world: "interventions", ticks: 60 } });
await store.setRunStatus(tree, projectId, runId, "completed");
console.log(`main computed to ${record.computed_until} in ${((performance.now() - started) / 1000).toFixed(2)} s`);

check(
  wasm.sha256Hex((await tree.read([projectId, "runs", runId, "evidence.json"])) ?? new Uint8Array(0)) === withLlm.evidence_sha256,
  "the Run's evidence.json is the server's",
);

// The site's relay (deploy/workers/relay.mjs): each call answers the next of `relayCalls` (a case's
// `llm`: the sha256 of the task the server's link got, the Provider's text and the server's reply),
// or `relayRefusal` where it is set.
const PASSPHRASE = "abcde-fghjk-mnpqr-stuvw";
let relayCalls = [];
let relayRefusal = null;
let relaySent = 0;
async function relay(path, init) {
  relaySent += 1;
  check(init.headers.Authorization === `Bearer ${PASSPHRASE}`, "the relay gets the passphrase");
  const body = init.body;
  check(body.startsWith('{"input":') && body.endsWith("}"), "the relay gets the core's task as it is");
  const task = body.slice('{"input":'.length, -1);
  const name = JSON.parse(task).task;
  check(path === `/llm/v1/tasks/${name}`, `the task's route: ${path}`);
  if (relayRefusal !== null) return new Response(JSON.stringify(relayRefusal.body), { status: relayRefusal.status, headers: { "content-type": "application/json" } });
  const call = relayCalls.shift();
  if (call === undefined) {
    check(false, `a call the server did not make: ${name}`);
    return new Response(JSON.stringify({ error: "no", code: "llm_relay_error" }), { status: 500 });
  }
  check(wasm.sha256Hex(encoder.encode(task)) === call.task_sha256, `the ${name} task is the one the server's link got`);
  const { reply } = call;
  return new Response(
    JSON.stringify({
      task: name,
      prompt_version: JSON.parse(task).prompt_version,
      model: reply.model_id,
      output_text: call.output_text,
      stop_reason: "end_turn",
      usage: { input_tokens: reply.input_tokens, output_tokens: reply.output_tokens, cost_usd: reply.estimated_cost_usd },
      remaining_usd: 4.99,
    }),
    { status: 200, headers: { "content-type": "application/json" } },
  );
}

const browser = new BrowserApi({ wasm, tree, relay });
const base = `/api/v1/projects/${projectId}/runs/${runId}`;
const post = (path, body, llm = null) => browser.answer({ method: "POST", path: `${base}${path}`, body, llm });
const get = (path, search = "") => browser.answer({ method: "GET", path: `${base}${path}`, search });

async function compare(name, got, expected) {
  if (got.status !== expected.status) {
    check(false, `${name}: status ${got.status} (${got.body.slice(0, 300)}), not ${expected.status} (${(expected.body ?? expected.text ?? "").slice(0, 300)})`);
    return;
  }
  if (expected.status !== 200) {
    check(got.body === expected.body, `${name}: ${got.body}, not ${expected.body}`);
    return;
  }
  const text = await mask(got.body);
  const ok = wasm.sha256Hex(encoder.encode(text)) === expected.sha256;
  check(ok, `${name}: the body is not the server's\n  browser: ${text.slice(0, 1500)}\n  server:  ${(expected.text ?? "(long)").slice(0, 1500)}`);
}

/// Waits until no computation runs.
async function settle() {
  for (const job of [...browser.computing.jobs.values()]) await job.finished;
}

started = performance.now();
let browserPreview = null;
let agent = null;
let firstContext = null;
let llmContext = null;
for (const item of [...fixture.cases, ...withLlm.cases]) {
  let got;
  // A case the server answered with an API key is asked with the passphrase.
  const llm = item.llm ? PASSPHRASE : null;
  relayCalls = item.llm ? [...item.llm] : [];
  relaySent = 0;
  if (item.route === "preview") {
    got = await post("/branches/main/intervention-preview", item.request, llm);
    if (browserPreview === null && item.status === 200 && item.text?.includes('"can_apply":true')) browserPreview = JSON.parse(got.body);
  } else if (item.route === "apply") {
    // The browser's own preview_id where the server's was ("$PREVIEW").
    const document = { ...item.document };
    if (document.preview_id === "$PREVIEW") document.preview_id = browserPreview.preview_id;
    got = await post("/branches/main/intervention-apply", JSON.stringify(document));
  } else if (item.route === "run") {
    got = await get("");
  } else if (item.route === "timeline") {
    // The child is computed to the end before its first frame is asked for.
    if (browser.computing.jobs.size === 0 && (await store.branchRecord(tree, projectId, runId, child)).computed_until < minutes) {
      const resumed = await post("/resume", JSON.stringify({ branch_id: child, target_minute: minutes }));
      check(resumed.status === 200, `resume the child: ${resumed.body}`);
      await settle();
    }
    got = await get(`/branches/${child}/timeline`, `minute=${item.minute}`);
  } else if (item.route === "history") {
    got = await get(`/branches/${child}/history`, item.query);
  } else if (item.route === "questions") {
    if (agent === null) agent = JSON.parse((await get(`/branches/main/timeline`, `minute=${minutes}`)).body).people[0].id;
    const placed = { $AGENT: agent, $CONTEXT: firstContext, $LLM_CONTEXT: llmContext };
    const document = Object.fromEntries(Object.entries(item.document).map(([key, value]) => [key, Object.hasOwn(placed, value) ? placed[value] : value]));
    got = await post(`/branches/${item.branch}/questions`, JSON.stringify(document), llm);
    if (firstContext === null && got.status === 200) firstContext = JSON.parse(got.body).context_id;
    if (llm && llmContext === null && got.status === 200) llmContext = JSON.parse(got.body).context_id;
  } else if (item.route === "pdf-report") {
    got = await post("/pdf-report", JSON.stringify(item.document), llm);
  } else if (item.route === "compare" || item.route === "replay-frames") {
    got = await get(`/${item.route}`, item.query);
  } else {
    throw new Error(`no route ${item.route}`);
  }
  if (item.llm && item.route === "preview" && item.text?.includes('"fallback_used":true')) {
    // The browser stops where the server fell back to the rules.
    const refused = JSON.parse(got.body);
    check(got.status === 409 && refused.code === "intervention_compiler_unavailable", `${item.name}: ${got.status} ${got.body}`);
  } else await compare(`${item.route} ${item.name}`, got, item);
  check(relayCalls.length === 0 && relaySent === (item.llm?.length ?? 0), `${item.name}: the relay got ${relaySent} calls, not ${item.llm?.length ?? 0}`);
}
console.log(`${fixture.cases.length + withLlm.cases.length} answers compared with the server's in ${((performance.now() - started) / 1000).toFixed(2)} s`);

// Each question answered is kept as a turn in its branch's stored Run, after its end, its question
// as the server keeps it.
{
  const reader = await store.branchReader(wasm, tree, projectId, runId, "main");
  let questions;
  try {
    questions = (await reader.records("conversations")).map((turn) => turn.question);
    check(reader.lines.at(-1).kind === "conversations" && reader.lines.findLastIndex((line) => line.kind === "end") < reader.lines.length - 1, "the turns follow the end line");
  } finally {
    reader.close();
  }
  check(JSON.stringify(questions) === JSON.stringify(withLlm.stored_questions), `the questions kept: ${JSON.stringify(questions)}, not ${JSON.stringify(withLlm.stored_questions)}`);
}

// A relay that does not let the passphrase in is no link: a question's answer without an API key,
// a preview that fails. One that refuses the call (the day's cap) gives a question the task's
// fallback with its reason, and fails a preview with it.
{
  relayRefusal = { status: 401, body: { error: "合言葉が違います。", code: "llm_passphrase_invalid" } };
  const question = { question: "合言葉が違うと?" };
  const asked = JSON.parse((await post("/branches/main/questions", JSON.stringify(question), PASSPHRASE)).body);
  const offline = JSON.parse((await post("/branches/main/questions", JSON.stringify(question))).body);
  check(asked.mode === offline.mode && asked.answer === offline.answer && asked.mode !== "record", `a passphrase not let in: ${JSON.stringify(asked).slice(0, 300)}`);
  const text = JSON.stringify({ minute: fixture.run.fork_minute, text: "仮設トイレを3基置いてほしい" });
  const previewed = await post("/branches/main/intervention-preview", text, PASSPHRASE);
  const notLetIn = JSON.parse(previewed.body);
  check(previewed.status === 409 && notLetIn.code === "intervention_compiler_unavailable" && notLetIn.error.includes("合言葉"), `a passphrase not let in: ${previewed.body}`);
  relayRefusal = { status: 429, body: { error: "今日のAPIの上限（$5）に達しました。", code: "llm_budget_exceeded", remaining_usd: 0.1 } };
  const capped = await post("/branches/main/intervention-preview", text, PASSPHRASE);
  const cap = JSON.parse(capped.body);
  check(capped.status === 409 && cap.code === "intervention_compiler_unavailable" && cap.error.includes("予算の上限を超えるため"), `the day's cap: ${capped.body}`);
  relayRefusal = null;
}

// The core's catalog of the operators it schedules (#584), and a typed program the screen's builder
// makes of it (demo-v2-comparison.js `builtProgram`): checked on the world, no text read, no relay
// asked even with the passphrase.
{
  const contracts = await browser.answer({ method: "GET", path: `/api/v1/projects/${projectId}/contracts`, search: "" });
  check(contracts.status === 200, `contracts: ${contracts.status} ${contracts.body}`);
  const catalog = JSON.parse(contracts.body);
  check(contracts.body === wasm.contractsAnswer() && catalog.executable_effect_primitives.length === 6, `the catalog: ${contracts.body.slice(0, 200)}`);
  const missing = await browser.answer({ method: "GET", path: "/api/v1/projects/nowhere/contracts", search: "" });
  check(missing.status === 409, `the catalog of no Project: ${missing.status} ${missing.body}`);
  const entry = catalog.executable_effect_primitives.find((item) => item.op === "add_toilet");
  const minute = fixture.run.fork_minute;
  const program = {
    schema_version: 1,
    program_id: "manual-toilets",
    narrative: entry.label,
    source_text: entry.label,
    simulation_start_minute: minute,
    effects: [
      {
        effect_id: "effect-1",
        op: entry.op,
        target_selector: { kind: "zone", target_id: "toilet", candidate_ids: [] },
        start_minute: minute,
        end_minute: null,
        guard: null,
        parameters: { count: 1 },
        units: entry.units[0],
        source_id: "screen-builder-v1",
        assumption_ids: [],
        policy_dependencies: entry.required_policies,
        causation_id: null,
        reversible: true,
      },
    ],
    assumptions: [],
    support_mode: "native",
    compiler_provenance: { compiler_kind: "manual", compiler_version: "screen-builder-v1", model_id: null, prompt_hash: null },
  };
  relaySent = 0;
  const got = await post("/branches/main/intervention-preview", JSON.stringify({ minute, program }), PASSPHRASE);
  const answer = JSON.parse(got.body);
  check(got.status === 200 && answer.status === "compiled" && answer.fallback_used === false && answer.program?.compiler_provenance?.compiler_kind === "manual", `a typed program: ${got.body.slice(0, 400)}`);
  check(relaySent === 0, `a typed program asked the relay ${relaySent} times`);
  const broken = await post("/branches/main/intervention-preview", JSON.stringify({ minute, program: { ...program, support_mode: "certain" } }));
  check(broken.status === 400 && JSON.parse(broken.body).code === "invalid_effect_program", `a program the contract refuses: ${broken.status} ${broken.body}`);
}

// The child's stored Run: format v6, its "run" line holds the fork, it starts at the fork's minute,
// it reads its parent's pieces up to its base (it holds no exported world of its own), and it reads
// back to its digests. Without its parent's, it is not read.
{
  const backend = await store.branchBackend(tree, projectId, runId, child);
  await assert.rejects(runStore.RunReader.open({ wasm, backend }), /needs its parent's \(branch main\)/);
  const reader = await store.branchReader(wasm, tree, projectId, runId, child);
  try {
    assert.equal(reader.run.format, runStore.FORMAT);
    assert.equal(reader.run.fork_minute, fixture.run.fork_minute);
    const forkLine = reader.lines.find((line) => line.kind === "fork");
    check(forkLine?.fork_origin?.branch_id === "main" && forkLine.minute === fixture.run.fork_minute, "the fork line names its origin");
    check(forkLine.base.minute <= fixture.run.fork_minute && reader.chain.levels() === 2 && !reader.lines.some((line) => line.kind === "export"), "the child reads its parent's pieces");
    await assert.rejects(reader.seek(fixture.run.fork_minute - 1));
  } finally {
    reader.close();
  }
  const back = await runStore.readAndVerify({ wasm, backend, parent: store.runBackends(tree, projectId, runId) });
  check(back.complete && back.verified, `the child's stored Run reads back to its digests: ${JSON.stringify(back).slice(0, 300)}`);
}

browser.close();
if (failures > 0) {
  console.error(`${failures} failure(s)`);
  process.exit(1);
}
console.log("the intervention's answers are the server's");
