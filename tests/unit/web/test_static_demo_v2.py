"""Exercise the real polling callback with stable/changed saved frame targets."""

import os
import shutil
import subprocess
from pathlib import Path

import pytest


def test_polling_reuses_only_the_same_immutable_frame():
    node = shutil.which("node")
    if node is None:
        pytest.skip("Node is required for the UI callback regression")
    source = Path("src/sheltersim/web/static/demo-v2.js").resolve()
    script = r"""
import fs from "node:fs";
import assert from "node:assert/strict";
const source = fs.readFileSync(process.argv[1], "utf8").replaceAll("\r\n", "\n");
const body = source.split("  async function refreshRun() {")[1].split("  async function seek(")[0];
const elements = new Map();
const $ = id => {
  if (!elements.has(id)) elements.set(id, {value:"",replaceChildren(){},options:[]});
  return elements.get(id);
};
const el = () => ({}), shortBranch = id => id;
let state, next, seeks, reads, controlled;
const api = async () => {reads++;return next;};
const runBase = () => "/run", base = () => "/project";
const reference = () => state.data.branches.find(b => b.branch_id === state.branch);
const upper = () => reference().computed_until, lower = () => reference().fork_minute;
const seek = async minute => {seeks.push(minute);};
const controls = () => {controlled++;}, remember = () => {}, note = () => {};
const clearTimeout = () => {}, setTimeout = () => 1;
const refresh = eval("(async function refreshRun() {" + body + ")");
const branch = {branch_id:"main",checkpoint_hash:"a",computed_until:30,fork_minute:0};
// While the branch computes, the page waits dimmed (#482): the minute does not follow the
// computed head and the frame is read again only once the computation ends.
for (const test of [
  {name:"unchanged",frameMinute:30,minute:30,hash:"a",head:30,expected:[]},
  {name:"new checkpoint while computing",frameMinute:30,minute:30,hash:"b",head:60,expected:[]},
  {name:"new checkpoint at the end",frameMinute:30,minute:30,hash:"b",head:60,
    status:"completed",expected:[30]},
  {name:"same past frame",frameMinute:13,minute:13,hash:"a",head:30,expected:[]},
  {name:"different minute while computing",frameMinute:0,minute:13,hash:"a",head:30,expected:[]},
  {name:"different minute",frameMinute:0,minute:13,hash:"a",head:30,status:"paused",
    expected:[13]},
  {name:"different run",frameMinute:30,minute:30,hash:"a",head:30,frameRun:"other",expected:[30]},
  {name:"different branch",frameMinute:30,minute:30,hash:"a",head:30,
    frameBranch:"child",expected:[30]},
  {name:"completion only",frameMinute:30,minute:30,hash:"a",head:30,status:"completed",expected:[]},
]) {
  elements.clear();seeks=[];reads=0;controlled=0;
  state = {run:"r",loadedRun:"r",branch:"main",scope:0,minute:test.minute,
    data:{branches:[{...branch}]},
    frame:{run_id:test.frameRun||"r",branch_id:test.frameBranch||"main",minute:test.frameMinute}};
  next = {status:test.status||"running",
    branches:[{...branch,checkpoint_hash:test.hash,computed_until:test.head}]};
  await refresh();
  assert.deepEqual(seeks,test.expected,test.name);
  assert.equal(reads,1,test.name+" still polls status");
  assert.equal(controlled,1,test.name+" updates controls");
}
"""
    subprocess.run([node, "--input-type=module", "-e", script, str(source)], check=True)


def test_applying_an_intervention_waits_for_the_fork_process_to_list_the_child():
    """The server forks in its own process, so intervention-apply answers before the
    child Branch exists. The screen must wait for it, as /demo does, or the comparison it
    opens next finds no Branch (「介入後の分岐を選択してください。」)."""
    node = shutil.which("node")
    if node is None:
        pytest.skip("Node is required for the UI callback regression")
    source = Path("src/sheltersim/web/static/demo-v2.js").resolve()
    text = source.read_text(encoding="utf-8")
    apply = text.split("intervention-apply`, {")[1].split('show("comparison")')[0]
    assert apply.index("await waitForked(child);") < apply.index("state.branch = child;")
    script = r"""
import fs from "node:fs";
import assert from "node:assert/strict";
const source = fs.readFileSync(process.argv[1], "utf8").replaceAll("\r\n", "\n");
const body = source
  .split("  async function waitForked(child) {")[1]
  .split("  async function previewIntervention(")[0];
let state, answers, reads;
const api = async () => {
  reads++;
  const next = answers.shift();
  if (next.scope) state.scope++;
  return next.data;
};
const runBase = () => "/run";
const setTimeout = (resolve) => {resolve();return 1;};
const waitForked = eval("(async function waitForked(child) {" + body + ")");
const run = (status, branches, last_fork) => ({data:{status, branches, last_fork}});
const main = {branch_id:"main"}, child = {branch_id:"c"};
// Not listed while the fork computes, then listed while computing, then done.
state = {scope:1}; reads = 0;
answers = [run("running",[main]), run("running",[main,child]), run("paused",[main,child])];
await waitForked("c");
assert.equal(reads, 3);
assert.equal(state.data.branches.length, 2);
// Another branch computes meanwhile: a child with its own idle status has ended.
state = {scope:1}; reads = 0;
answers = [run("running",[{...main,status:"running"},{...child,status:"paused"}])];
await waitForked("c");
assert.equal(reads, 1);
// A failed fork says why instead of waiting forever.
state = {scope:1}; reads = 0;
answers = [run("paused",[main],{status:"failed",error:"storage capacity exceeded"})];
await assert.rejects(waitForked("c"), /分岐を作成できませんでした: storage capacity exceeded/);
// The fork ended without its child and gave no reason.
state = {scope:1}; reads = 0;
answers = [run("paused",[main])];
await assert.rejects(waitForked("c"), /理由を取得できませんでした/);
// Another Run was opened meanwhile.
state = {scope:1}; reads = 0;
answers = [{...run("running",[main]),scope:true}];
await assert.rejects(waitForked("c"), /表示するシナリオが変更されました/);
"""
    subprocess.run(
        [node, "--input-type=module", "-e", script, str(source)],
        check=True,
        capture_output=True,
        text=True,
    )


def test_timeline_events_scroll_to_the_newest_record():
    """The timeline's event box has a fixed height (#377), and records are listed oldest
    first, so each render scrolls the box to its end to keep the newest ones in view."""
    node = shutil.which("node")
    if node is None:
        pytest.skip("Node is required for the UI callback regression")
    source = Path("src/sheltersim/web/static/demo-v2.js").resolve()
    script = r"""
import fs from "node:fs";
import assert from "node:assert/strict";
const source = fs.readFileSync(process.argv[1], "utf8").replaceAll("\r\n", "\n");
const body = source
  .split("  function renderEvents(box, events, clickable = false) {")[1]
  .split("  function personSummary(")[0];
const el = (tag, text, cls) => ({tag, text, cls, append() {}, addEventListener() {}});
const eventNames = {}, eventTone = () => "record", time = (m) => String(m);
const lower = () => 0, upper = () => 100, action = () => {}, seek = () => {};
const renderEvents = eval(
  "(function renderEvents(box, events, clickable = false) {" + body + ")"
);
const box = () => ({
  children: [], scrollTop: 0,
  replaceChildren() {this.children = [];},
  append(n) {this.children.push(n);},
  get scrollHeight() {return 40 * this.children.length;},
});
const events = Array.from({length: 20}, (_, i) => ({
  minute: i, event_type: `t${i}`, status: "applied", event_id: `e${i}`,
}));
for (const clickable of [true, false]) {
  const b = box();
  renderEvents(b, events, clickable);
  assert.equal(b.children.length, 12);
  assert.equal(b.scrollTop, b.scrollHeight, `clickable=${clickable}`);
}
"""
    subprocess.run(
        [node, "--input-type=module", "-e", script, str(source)],
        check=True,
        capture_output=True,
        text=True,
    )


def test_a_browser_failure_does_not_send_the_reader_to_the_url():
    """When the browser version cannot open its store (OPFS) or start its Worker, the API answers
    a `browser_*` code with a text that says what to do (#393); the screen shows that text alone,
    not 「URLのproject/run指定を確認してください」, which is for a Project or Run it cannot find."""
    node = shutil.which("node")
    if node is None:
        pytest.skip("Node is required for the UI callback regression")
    source = Path("src/sheltersim/web/static/demo-v2.js").resolve()
    script = r"""
import fs from "node:fs";
import assert from "node:assert/strict";
const source = fs.readFileSync(process.argv[1], "utf8").replaceAll("\r\n", "\n");
const body = source.split("  function loadFailure(error) {")[1].split("  loading(init).catch(")[0];
const loadFailure = eval("(function loadFailure(error) {" + body + ")");
const opfs = "このブラウザーでは、計算の結果を保存する場所（OPFS）を使えません。";
for (const code of ["browser_storage_unavailable", "browser_unavailable"])
  assert.equal(loadFailure(Object.assign(new Error(opfs), {code})), opfs, code);
const missing = loadFailure(Object.assign(new Error("project not found"), {code: "not_found"}));
assert.match(missing, /^読み込みに失敗しました：project not found。URLのproject\/run指定/);
assert.match(loadFailure(new Error("HTTP 500")), /URLのproject\/run指定/);
"""
    subprocess.run(
        [node, "--input-type=module", "-e", script, str(source)],
        check=True,
        capture_output=True,
        text=True,
    )


def test_the_address_names_what_is_open_and_the_bare_screen_is_the_home():
    """The home with nothing open is the bare / (#376; /demo-v2 before), which opens the home
    again (#371); a Run
    or another Project than one's own is in the query, and any other page in the hash."""
    node = shutil.which("node")
    if node is None:
        pytest.skip("Node is required for the UI callback regression")
    source = Path("src/sheltersim/web/static/demo-v2.js").resolve()
    script = r"""
import fs from "node:fs";
import assert from "node:assert/strict";
const source = fs.readFileSync(process.argv[1], "utf8").replaceAll("\r\n", "\n");
const body = source.split("  function remember() {")[1].split("  async function api(")[0];
let stored = null, address = null;
const localStorage = {setItem: (key, value) => {stored = [key, JSON.parse(value)];}};
const history = {replaceState: (_s, _t, url) => {address = url;}};
const ownId = "backup-1";
let state;
const remember = eval("(function remember() {" + body + ")");
for (const [given, url] of [
  [{project: "backup-1", run: null, branch: "main", page: "home"}, "/"],
  [{project: "backup-1", run: null, branch: "main", page: "scenario"}, "/#scenario"],
  [{project: "sample", run: null, branch: "main", page: "home"}, "/?project=sample#home"],
  [{project: "sample", run: "r1", branch: "b2", page: "simulation"},
    "/?project=sample&run=r1&branch=b2#simulation"],
  [{project: "backup-1", run: "r2", branch: "main", page: "home"},
    "/?project=backup-1&run=r2&branch=main#home"],
]) {
  state = given;
  remember();
  assert.equal(address, url);
  assert.deepEqual(stored, ["hinanjo-if-backup",
    {own: "backup-1", project: given.project, run: given.run, branch: given.branch}]);
}
"""
    subprocess.run(
        [node, "--input-type=module", "-e", script, str(source)],
        check=True,
        capture_output=True,
        text=True,
    )


def test_runs_are_named_as_scenarios_and_own_runs_of_the_same_conditions_by_when_made():
    """The screen calls a Run a シナリオ by its conditions (#373). One's own Runs of the same
    conditions are told apart by when they were made, in local time (#447); a Run made before
    #447 has no creation time and no suffix; an example's Runs have none either."""
    node = shutil.which("node")
    if node is None:
        pytest.skip("Node is required for the UI callback regression")
    source = Path("src/sheltersim/web/static/demo-v2.js").resolve()
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


def test_the_screen_s_text_says_scenario_and_branch_not_run_or_branch():
    """The screen's words are シナリオ and 分岐 (#373): no Run, Branch, 世界線 or Checkpoint in
    the text the page shows (its HTML text and attributes, and the strings its scripts show)."""
    import re

    static = Path("src/sheltersim/web/static")
    html = (static / "demo-v2.html").read_text(encoding="utf-8")
    shown = re.sub(r"<(script|style)[^>]*>.*?</\1>", "", html, flags=re.S)
    texts = re.findall(r">([^<]+)<", shown) + re.findall(
        r'(?:title|aria-label|placeholder|aria-description)="([^"]*)"', shown
    )
    for script in sorted(static.glob("demo-v2*.js")):
        source = script.read_text(encoding="utf-8")
        source = re.sub(r"^\s*//.*$|/\*.*?\*/", "", source, flags=re.S | re.M)
        # The strings a script writes: those holding Japanese are shown to the person.
        for literal in re.findall(r'"([^"\n]*)"|`([^`]*)`', source):
            text = "".join(literal)
            if re.search(r"[ぁ-んァ-ヶ一-龥]", text):
                texts.append(text)
    bad = [t for t in texts if re.search(r"\bRun\b|\bBranch\b|世界線|Checkpoint", t)]
    assert bad == []


def test_opening_a_run_uses_no_random_id():
    """The own Project's id is made only when a Scenario is first made (#371): opening a Run of
    the example uses no crypto.randomUUID, whose sequence the browser checks pin for the Run and
    branch ids (benchmarks/static_site_e2e.py)."""
    node = shutil.which("node")
    if node is None:
        pytest.skip("Node is required for the UI callback regression")
    source = Path("src/sheltersim/web/static/demo-v2.js").resolve()
    script = r"""
import fs from "node:fs";
import assert from "node:assert/strict";
const source = fs.readFileSync(process.argv[1], "utf8").replaceAll("\r\n", "\n");
const body = source.split("  const askedProject = ")[1].split("  const state = {")[0];
const run = (search, saved) => {
  let made = 0;
  const crypto = {randomUUID: () => `u${++made}`};
  const query = new URLSearchParams(search), exampleProject = "sample";
  const own = eval(`(() => { const askedProject = ${body}; return [() => ownId, ownProject]; })()`);
  return {made: () => made, ownId: own[0], ownProject: own[1]};
};
const example = run("?project=sample&run=r1", {});
assert.equal(example.made(), 0);
assert.equal(example.ownId(), null);
assert.equal(example.ownProject(), "backup-u1");
assert.equal(example.ownProject(), "backup-u1");
assert.equal(example.made(), 1);
assert.equal(run("?project=e2e", {}).ownId(), "e2e");
assert.equal(run("", {own: "backup-x"}).ownId(), "backup-x");
assert.equal(run("?project=sample", {project: "mine"}).ownId(), "mine");
"""
    subprocess.run(
        [node, "--input-type=module", "-e", script, str(source)],
        check=True,
        capture_output=True,
        text=True,
    )


def test_the_api_passphrase_is_sent_only_to_the_relay_and_forgotten_when_refused():
    """「API を有効にする」 (#420): the passphrase goes to /llm/v1/session as a Bearer token; one
    the relay no longer lets in is forgotten, one kept while the relay is out of reach."""
    node = shutil.which("node")
    if node is None:
        pytest.skip("Node is required for the UI callback regression")
    source = Path("src/sheltersim/web/static/demo-v2.js").resolve()
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

answer = async () => { throw new TypeError("offline"); };
await refreshApi();
assert.equal(store.get("hinanjo-if-llm-passphrase"), "pass-1");
assert.match($("home-api-status").textContent, /接続できません/);

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


def test_the_home_lists_saved_scenarios_with_delete_but_not_the_server_s_example():
    """「保存済みのシナリオ」 (#440): one list; an example's rows are marked 「例」, and have
    「削除」 only where the example can be fetched again (the browser version); one's own always."""
    node = shutil.which("node")
    if node is None:
        pytest.skip("Node is required for the UI callback regression")
    source = Path("src/sheltersim/web/static/demo-v2.js").resolve()
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
