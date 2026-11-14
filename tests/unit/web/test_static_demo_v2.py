"""Exercise the real polling callback with stable/changed saved frame targets."""

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
const source = fs.readFileSync(process.argv[1], "utf8");
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
for (const test of [
  {name:"unchanged",frameMinute:30,minute:30,hash:"a",head:30,expected:[]},
  {name:"new checkpoint",frameMinute:30,minute:30,hash:"b",head:60,expected:[60]},
  {name:"same past frame",frameMinute:13,minute:13,hash:"a",head:30,expected:[]},
  {name:"different minute",frameMinute:0,minute:13,hash:"a",head:30,expected:[13]},
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
const source = fs.readFileSync(process.argv[1], "utf8");
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
const source = fs.readFileSync(process.argv[1], "utf8");
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
const source = fs.readFileSync(process.argv[1], "utf8");
const body = source.split("  function loadFailure(error) {")[1].split("  init().catch(")[0];
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
    """The home with nothing open is the bare /demo-v2, which opens the home again (#371); a Run
    or another Project than one's own is in the query, and any other page in the hash."""
    node = shutil.which("node")
    if node is None:
        pytest.skip("Node is required for the UI callback regression")
    source = Path("src/sheltersim/web/static/demo-v2.js").resolve()
    script = r"""
import fs from "node:fs";
import assert from "node:assert/strict";
const source = fs.readFileSync(process.argv[1], "utf8");
const body = source.split("  function remember() {")[1].split("  async function api(")[0];
let stored = null, address = null;
const localStorage = {setItem: (key, value) => {stored = [key, JSON.parse(value)];}};
const history = {replaceState: (_s, _t, url) => {address = url;}};
const ownProject = "backup-1";
let state;
const remember = eval("(function remember() {" + body + ")");
for (const [given, url] of [
  [{project: "backup-1", run: null, branch: "main", page: "home"}, "/demo-v2"],
  [{project: "backup-1", run: null, branch: "main", page: "scenario"}, "/demo-v2#scenario"],
  [{project: "sample", run: null, branch: "main", page: "home"}, "/demo-v2?project=sample#home"],
  [{project: "sample", run: "r1", branch: "b2", page: "simulation"},
    "/demo-v2?project=sample&run=r1&branch=b2#simulation"],
  [{project: "backup-1", run: "r2", branch: "main", page: "home"},
    "/demo-v2?project=backup-1&run=r2&branch=main#home"],
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


def test_runs_are_named_as_scenarios_and_runs_of_the_same_conditions_apart():
    """The screen calls a Run a シナリオ by its conditions (#373); Runs of the same conditions
    are told apart by their seed, then by the end of their id."""
    node = shutil.which("node")
    if node is None:
        pytest.skip("Node is required for the UI callback regression")
    source = Path("src/sheltersim/web/static/demo-v2.js").resolve()
    script = r"""
import fs from "node:fs";
import assert from "node:assert/strict";
const source = fs.readFileSync(process.argv[1], "utf8");
const take = (name, next) => source.split(`  function ${name}(`)[1].split(`  ${next}`)[0];
const num = (n) => String(n);
const disasterName = (d) => ({earthquake: "地震"})[d] || d;
const runTitle = eval("(function runTitle(" + take("runTitle", "// The screen calls") + ")");
const runNames = eval("(function runNames(" + take("runNames", "// How far") + ")");
const scenario = (id, people) => ({scenario_id: id, scenario_version: 1, scenario: {
  duration_minutes: 10080, profiles: Array(people).fill({}),
  provenance: {natural_language_input: {municipality: "墨田区", disaster: "earthquake"}}}});
const scenarios = [scenario("s1", 100), scenario("s2", 20)];
const run = (id, sid, seed) => ({run_id: id, scenario_id: sid, scenario_version: 1, seed});
const names = runNames([run("run-aaaa1", "s1", 1), run("run-bbbb2", "s1", 1),
  run("run-cccc3", "s1", 2), run("run-dddd4", "s2", 1)], scenarios);
assert.deepEqual([...names.values()], [
  "墨田区 · 地震 · 100人 · 7日間 · seed 1 · aaa1",
  "墨田区 · 地震 · 100人 · 7日間 · seed 1 · bbb2",
  "墨田区 · 地震 · 100人 · 7日間 · seed 2",
  "墨田区 · 地震 · 20人 · 7日間",
]);
for (const name of names.values()) assert.doesNotMatch(name, /Run|Branch/);
"""
    subprocess.run(
        [node, "--input-type=module", "-e", script, str(source)],
        check=True,
        capture_output=True,
        text=True,
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
