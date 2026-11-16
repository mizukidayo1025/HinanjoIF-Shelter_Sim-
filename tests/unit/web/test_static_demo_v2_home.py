"""The home's script (demo-v2-home.js): the saved Scenarios' names and rows, and the
passphrase of the AI's API."""

import os
import shutil
import subprocess
from pathlib import Path

import pytest


def test_runs_are_named_as_scenarios_and_own_runs_of_the_same_conditions_by_when_made():
    """The screen calls a Run a シナリオ by its conditions (#373). One's own Runs of the same
    conditions are told apart by when they were made, in local time (#447); a Run made before
    #447 has no creation time and no suffix; an example's Runs have none either."""
    node = shutil.which("node")
    if node is None:
        pytest.skip("Node is required for the UI callback regression")
    source = Path("src/sheltersim/web/static/demo-v2-home.js").resolve()
    script = r"""
import fs from "node:fs";
import assert from "node:assert/strict";
const source = fs.readFileSync(process.argv[1], "utf8").replaceAll("\r\n", "\n");
const take = (name, next) => source.split(`  function ${name}(`)[1].split(`  ${next}`)[0];
const num = (n) => String(n);
const disasterName = (d) => ({earthquake: "地震"})[d] || d;
const runTitle = eval("(function runTitle(" + take("runTitle", "// The screen calls") + ")");
const runNames = eval("(function runNames(" + take("runNames", "// When a Run was made") + ")");
const createdLabel = eval("(function createdLabel(" + take("createdLabel", "// How far") + ")");
const scenario = (id, people) => ({scenario_id: id, scenario_version: 1, scenario: {
  duration_minutes: 10080, profiles: Array(people).fill({}),
  provenance: {natural_language_input: {municipality: "墨田区", disaster: "earthquake"}}}});
const scenarios = [scenario("s1", 100), scenario("s2", 20)];
const run = (id, sid, created_at) =>
  ({run_id: id, scenario_id: sid, scenario_version: 1, seed: 2026, created_at});
const runs = [run("run-aaaa1", "s1", "2026-11-15T01:30:00Z"),
  run("run-bbbb2", "s1", "2026-11-16T14:05:59Z"), run("run-cccc3", "s1", undefined),
  run("run-dddd4", "s2", "2026-11-15T01:30:00Z")];
assert.deepEqual([...runNames(runs, scenarios).values()], [
  "墨田区 · 地震 · 100人 · 7日間 · 11/15 10:30 作成",
  "墨田区 · 地震 · 100人 · 7日間 · 11/16 23:05 作成",
  "墨田区 · 地震 · 100人 · 7日間",
  "墨田区 · 地震 · 20人 · 7日間",
]);
assert.deepEqual([...runNames(runs, scenarios, {example: true}).values()], [
  "墨田区 · 地震 · 100人 · 7日間",
  "墨田区 · 地震 · 100人 · 7日間",
  "墨田区 · 地震 · 100人 · 7日間",
  "墨田区 · 地震 · 20人 · 7日間",
]);
for (const name of runNames(runs, scenarios).values()) {
  assert.doesNotMatch(name, /Run|Branch|seed|aaa1|bbb2/);
}
"""
    subprocess.run(
        [node, "--input-type=module", "-e", script, str(source)],
        check=True,
        capture_output=True,
        text=True,
        env={**os.environ, "TZ": "Asia/Tokyo"},
    )


def test_the_api_passphrase_is_sent_only_to_the_relay_and_forgotten_when_refused():
    """「API を有効にする」 (#420): the passphrase goes to /llm/v1/session as a Bearer token; one
    the relay no longer lets in is forgotten, one kept while the relay is out of reach."""
    node = shutil.which("node")
    if node is None:
        pytest.skip("Node is required for the UI callback regression")
    source = Path("src/sheltersim/web/static/demo-v2-home.js").resolve()
    script = r"""
import fs from "node:fs";
import assert from "node:assert/strict";
const source = fs.readFileSync(process.argv[1], "utf8").replaceAll("\r\n", "\n");
const body = source.split("  const LLM_KEY =")[1].split("  async function enableApi(")[0];
const store = new Map();
const localStorage = {
  getItem: (k) => (store.has(k) ? store.get(k) : null),
  setItem: (k, v) => store.set(k, v),
  removeItem: (k) => store.delete(k),
};
const elements = new Map();
const $ = (id) => {
  if (!elements.has(id))
    elements.set(id, { textContent: "", hidden: false, classList: { toggle() {} } });
  return elements.get(id);
};
const num = (n) => String(n);
const state = {};
const window = { HinanjoBrowserApi: {} };
const api = async () => { throw new Error("the browser version does not ask the server"); };
let answer;
const sent = [];
const fetch = async (url, init) => {
  sent.push([url, init.headers.Authorization]);
  return answer();
};
const wrapped = "(() => { const LLM_KEY =" + body + "; return { refreshApi, keepPassphrase }; })";
const make = eval(wrapped);
const { refreshApi, keepPassphrase } = make();
const json = (status, value) => async () => ({ ok: status < 400, status, json: async () => value });

keepPassphrase("pass-1");
answer = json(200, { recipient: "審査員A", remaining_usd: 4.5, available: true });
await refreshApi();
assert.deepEqual(sent.at(-1), ["/llm/v1/session", "Bearer pass-1"]);
assert.match($("home-api-status").textContent, /審査員A/);
assert.equal($("home-api-enable").hidden, true);
assert.deepEqual(state.llm, { on: true });
assert.equal($("header-api").textContent, "API ON");

answer = async () => { throw new TypeError("offline"); };
await refreshApi();
assert.equal(store.get("hinanjo-if-llm-passphrase"), "pass-1");
assert.match($("home-api-status").textContent, /接続できません/);
assert.deepEqual(state.llm, { on: false });
assert.equal($("header-api").textContent, "API OFF");

answer = json(401, { error: "合言葉が違います。", code: "llm_passphrase_invalid" });
await refreshApi();
assert.equal(store.has("hinanjo-if-llm-passphrase"), false);
assert.equal($("home-api-status").textContent, "合言葉が違います。");
assert.equal($("home-api-enable").hidden, false);
"""
    result = subprocess.run(
        [node, "--input-type=module", "-e", script, str(source)],
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=False,
    )
    assert result.returncode == 0, result.stderr


def test_the_server_version_shows_its_llm_settings_as_on_or_off_without_a_passphrase():
    """The AI's ON/OFF (#581): the server version asks GET /api/v1/llm, shows its answer at the
    top and on the home, and offers no passphrase; a failed check is OFF."""
    node = shutil.which("node")
    if node is None:
        pytest.skip("Node is required for the UI callback regression")
    source = Path("src/sheltersim/web/static/demo-v2-home.js").resolve()
    script = r"""
import fs from "node:fs";
import assert from "node:assert/strict";
const source = fs.readFileSync(process.argv[1], "utf8").replaceAll("\r\n", "\n");
const body = source.split("  const LLM_KEY =")[1].split("  async function enableApi(")[0];
const elements = new Map();
const $ = (id) => {
  if (!elements.has(id)) {
    const classes = new Set();
    elements.set(id, { textContent: "", hidden: false, title: "", classes,
      classList: { toggle: (name, on) => (on ? classes.add(name) : classes.delete(name)) } });
  }
  return elements.get(id);
};
const localStorage = { getItem: () => null, setItem() {}, removeItem() {} };
const fetch = async () => { throw new Error("the server version sends no passphrase"); };
const num = (n) => String(n);
const state = {};
const window = {};
let answer;
const asked = [];
const api = async (path) => { asked.push(path); return answer(); };
const make = eval("(() => { const LLM_KEY =" + body + "; return { refreshApi }; })");
const { refreshApi } = make();

answer = async () => ({ available: true, mode: "record", reason: null });
await refreshApi();
assert.deepEqual(asked, ["/api/v1/llm"]);
assert.deepEqual(state.llm, { on: true });
assert.equal($("header-api").textContent, "API ON");
assert.equal($("header-api").hidden, false);
assert.ok($("header-api").classes.has("on"));
assert.match($("home-api-status").textContent, /^ON です/);
assert.equal($("home-api-enable").hidden, true);
assert.equal($("home-api-disable").hidden, true);
assert.match($("home-api-how").textContent, /APIキーつきで起動/);

answer = async () => ({ available: false, mode: "offline", reason: "理由。" });
await refreshApi();
assert.deepEqual(state.llm, { on: false });
assert.equal($("header-api").textContent, "API OFF");
assert.ok(!$("header-api").classes.has("on"));
assert.equal($("home-api-status").textContent, "OFF です。理由。");

answer = async () => { throw new Error("HTTP 500"); };
await refreshApi();
assert.deepEqual(state.llm, { on: false });
assert.match($("home-api-status").textContent, /確認できませんでした/);
"""
    result = subprocess.run(
        [node, "--input-type=module", "-e", script, str(source)],
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=False,
    )
    assert result.returncode == 0, result.stderr


def test_the_home_lists_saved_scenarios_with_delete_but_not_the_server_s_example():
    """「保存済みのシナリオ」 (#440): one list; an example's rows are marked 「例」, and have
    「削除」 only where the example can be fetched again (the browser version); one's own always."""
    node = shutil.which("node")
    if node is None:
        pytest.skip("Node is required for the UI callback regression")
    source = Path("src/sheltersim/web/static/demo-v2-home.js").resolve()
    script = r"""
import fs from "node:fs";
import assert from "node:assert/strict";
const source = fs.readFileSync(process.argv[1], "utf8").replaceAll("\r\n", "\n");
const body = source.split("  function homeRows(")[1].split("\n  }\n")[0];
const node = (tag, text, cls) => ({
  tag, text, cls, children: [], attrs: {},
  append(...items) { this.children.push(...items); },
  setAttribute(k, v) { this.attrs[k] = v; },
  addEventListener(type, fn) { this.click = fn; },
  querySelector(sel) { return this.children.find((c) => c.tag === sel); },
});
const el = (tag, text, cls) => node(tag, text, cls);
const runNames = (runs) => new Map(runs.map((r) => [r.run_id, `name ${r.run_id}`]));
const runLink = () => { const a = node("a"); a.append(node("strong", "title")); return a; };
const deleted = [];
const action = (label, task) => task();
const deleteRun = (project, run, name) => deleted.push([project, run, name]);
const homeRows = eval("(function homeRows(" + body + "\n  })");
const project = { runs: [{ run_id: "r1" }, { run_id: "r2" }], scenarios: [] };
const button = (row) => row.children.find((c) => c.tag === "button");
const strong = (row) => row.children[0].querySelector("strong");
const tag = (row) => strong(row).children.find((c) => c.cls === "home-tag");

const server = homeRows("sample", project, { example: true, deletable: false });
assert.equal(server.length, 2);
assert.equal(tag(server[0]).text, "例");
assert.equal(button(server[0]), undefined);

const browser = homeRows("sample", project, { example: true, deletable: true });
assert.equal(button(browser[1]).text, "削除");
browser[1].children.find((c) => c.tag === "button").click();
assert.deepEqual(deleted, [["sample", "r2", "name r2"]]);

const own = homeRows("backup-1", project, { example: false, deletable: true });
assert.equal(tag(own[0]), undefined);
assert.equal(button(own[0]).attrs["aria-label"], "「name r1」を削除");
"""
    result = subprocess.run(
        [node, "--input-type=module", "-e", script, str(source)],
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=False,
    )
    assert result.returncode == 0, result.stderr
