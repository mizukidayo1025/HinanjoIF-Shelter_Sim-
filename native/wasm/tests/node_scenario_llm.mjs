// The Scenario text read with the LLM in the browser (Issue #421): js/scenario_routes.mjs's
// `scenario-preview` with the relay's passphrase asks the site's relay for the task
// `scenario_text` and reads its answer in the core (`parseScenarioTextAnswer`).
//
// Usage: node native/wasm/tests/node_scenario_llm.mjs PKG_DIR
//   PKG_DIR: the output of `wasm-bindgen --target nodejs` for sheltersim_wasm.wasm.
//
// The relay is answered here, with the LLM answers of native/core/tests/data/scenario_text_answers.json
// (tests/support/core_scenario_text.py): the preview's request and source must be what Python's
// parse made of the same answer (native/core/tests/scenario_text.rs compares every case in the
// core); an answer with a condition the browser does not compute leaves the offline read with a
// note first in `warnings`; a relay that refuses (the day's cap) and a relay out of reach fail the
// preview with the reason (#583), the text not read by the rules instead. The relay gets the
// passphrase and the text only; the call is recorded in the preparation account and the
// passphrase is stored nowhere.

import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import assert from "node:assert/strict";

const pkg = process.argv[2];
if (!pkg) {
  console.error("usage: node node_scenario_llm.mjs PKG_DIR");
  process.exit(2);
}
const require = createRequire(import.meta.url);
const wasm = require(join(resolve(pkg), "sheltersim_wasm.js"));
const here = dirname(fileURLToPath(import.meta.url));
const data = join(here, "..", "..", "core", "tests", "data");
const answers = JSON.parse(readFileSync(join(data, "scenario_text_answers.json"), "utf8"));
const recorded = JSON.parse(readFileSync(join(data, "research_cases.json"), "utf8"));
const { ScenarioRoutes } = await import(join(here, "..", "js", "scenario_routes.mjs"));
const store = await import(join(here, "..", "js", "project_store.mjs"));
const research = await import(join(here, "..", "js", "research.mjs"));

const PASSPHRASE = "abcde-fghjk-mnpqr-stuvw";
const decoder = new TextDecoder();
const sorted = (object) => Object.fromEntries(Object.entries(object).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)));
const caseOf = (name) => answers.cases.find((item) => item.name === name);

async function routesOf(relay) {
  const tree = store.memoryTree();
  await store.createProject(tree, "p");
  const sent = [];
  let ids = 0;
  const routes = new ScenarioRoutes({
    wasm,
    tree,
    shelterIndex: async () => recorded.indexes.corpus,
    shelterRows: async (code) => recorded.rows.corpus[code],
    fetchJson: (url) => research.gsiFetchJson(url, async () => ({ status: 200, arrayBuffer: async () => new TextEncoder().encode("[]").buffer })),
    now: () => new Date("2026-11-15T00:00:00+09:00"),
    newId: () => `${(ids += 1).toString(16).padStart(32, "0")}`,
    relay: async (path, init) => {
      sent.push({ path, init });
      return relay(path, init);
    },
  });
  return { routes, tree, sent };
}

async function preview(text, relay) {
  const { routes, tree, sent } = await routesOf(relay);
  const answer = JSON.parse(await routes.preview("p", { text }, { llm: PASSPHRASE }));
  const preparation = JSON.parse(decoder.decode(await tree.read(["p", "preparation.json"])));
  return { answer, sent, tree, preparation };
}

/// The preview's failure: the error, and the preparation account it left.
async function failed(text, relay) {
  const { routes, tree } = await routesOf(relay);
  const error = await routes.preview("p", { text }, { llm: PASSPHRASE }).then(
    () => assert.fail("the preview should fail"),
    (caught) => caught,
  );
  const preparation = JSON.parse(decoder.decode(await tree.read(["p", "preparation.json"])));
  return { error, preparation };
}

const answered = (output, cost = 0.0123) => async () =>
  new Response(JSON.stringify({ task: "scenario_text", prompt_version: "region-scenario-extraction-v1", model: "claude-opus-5-5", output_text: output, stop_reason: "end_turn", usage: { input_tokens: 9000, output_tokens: 300, cost_usd: cost }, remaining_usd: 4.9 }), { status: 200, headers: { "content-type": "application/json" } });

let checked = 0;
// Answers the browser reads as Python does: the request and the source are Python's.
for (const name of ["municipality-from-answer", "every-scalar", "stock-from-answer", "not-json", "blank-seed"]) {
  const item = caseOf(name);
  const { answer, sent, preparation } = await preview(item.text, answered(item.output));
  assert.equal(sent.length, 1, name);
  assert.equal(sent[0].path, "/llm/v1/tasks/scenario_text", name);
  assert.equal(sent[0].init.headers.Authorization, `Bearer ${PASSPHRASE}`, name);
  assert.deepEqual(JSON.parse(sent[0].init.body), { input: { text: item.text } }, name);
  const python = item.python.value;
  assert.deepEqual({ ...answer.request, field_origins: sorted(answer.request.field_origins) }, python.request, name);
  assert.equal(answer.scenario.provenance.scenario_input.extraction_source, python.source, name);
  assert.deepEqual(answer.warnings.slice(0, python.warnings.length), python.warnings, name);
  assert.deepEqual(answer.llm_usage, { mode: "record", api_request_count: 1, spent_usd: 0.0123 }, name);
  const [call] = preparation.llm_calls;
  assert.equal(call.model_id, "claude-opus-5-5", name);
  assert.equal(call.prompt_version, "region-scenario-extraction-v1", name);
  assert.equal(call.fallback_used, python.source !== "prepare_llm", name);
  checked += 1;
}

// A condition the browser does not compute: the offline read, said first.
{
  const item = caseOf("population-condition");
  const { answer, preparation } = await preview(item.text, answered(item.output));
  assert.equal(answer.scenario.provenance.scenario_input.extraction_source, "rules_fallback");
  assert.match(answer.warnings[0], /ブラウザー版ではまだ計算できない/);
  assert.equal(answer.llm_usage.api_request_count, 1);
  assert.equal(preparation.llm_calls[0].error_kind, "browser_unsupported");
  checked += 1;
}

// The day's cap reached, and a relay out of reach: the preview fails with the relay's reason,
// and the call is recorded.
{
  const text = caseOf("not-json").text;
  const capped = await failed(text, async () =>
    new Response(JSON.stringify({ error: "今日のAPIの上限（$5）に達しました。日本時間の0時に戻ります。", code: "llm_budget_exceeded", remaining_usd: 0.1 }), { status: 429 }),
  );
  assert.equal(capped.error.status, 429);
  assert.equal(capped.error.code, "llm_budget_exceeded");
  assert.match(capped.error.message, /^AIで文章を読み取れませんでした：今日のAPIの上限/);
  assert.equal(capped.preparation.llm_calls[0].outcome, "llm_budget_exceeded");
  const away = await failed(text, async () => {
    throw new TypeError("fetch failed");
  });
  assert.equal(away.error.code, "llm_relay_unreachable");
  assert.match(away.error.message, /中継に接続できませんでした/);
  assert.equal(away.preparation.llm_calls[0].outcome, "relay_unreachable");
  checked += 2;
}

// The passphrase is stored nowhere: not in the preparation account, a draft or any other file.
{
  const item = caseOf("municipality-from-answer");
  const { tree } = await preview(item.text, answered(item.output));
  const seen = [];
  async function visit(path) {
    for (const { name, kind } of await tree.list(path)) {
      if (kind === "directory") await visit([...path, name]);
      else seen.push(decoder.decode(await tree.read([...path, name])));
    }
  }
  await visit([]);
  assert.ok(seen.length >= 2);
  assert.ok(seen.every((text) => !text.includes(PASSPHRASE)));
  checked += 1;
}

console.log(`node_scenario_llm: ok (${checked} previews)`);
