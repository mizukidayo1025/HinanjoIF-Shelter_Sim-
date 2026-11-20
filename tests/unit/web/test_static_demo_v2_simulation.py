"""The simulation page's script (demo-v2-simulation.js): polling the open Run with
stable/changed saved frame targets, and its timeline of events."""

import shutil
import subprocess
from pathlib import Path

import pytest


def test_polling_reuses_only_the_same_immutable_frame():
    node = shutil.which("node")
    if node is None:
        pytest.skip("Node is required for the UI callback regression")
    source = Path("src/sheltersim/web/static/demo-v2-simulation.js").resolve()
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


def test_timeline_brief_surfaces_recent_problems_and_allows_seeking():
    """A recorded problem survives routine events, with time jumps limited to this branch."""
    node = shutil.which("node")
    if node is None:
        pytest.skip("Node is required for the UI callback regression")
    source = Path("src/sheltersim/web/static/demo-v2-simulation.js").resolve()
    script = r"""
import fs from "node:fs";
import assert from "node:assert/strict";
const source = fs.readFileSync(process.argv[1], "utf8").replaceAll("\r\n", "\n");
const body = source
  .split("  function renderEvents(box, events, clickable = false) {")[1]
  .split("  function replayTime(")[0];
const toggle = {};
const $ = () => toggle;
const el = (tag, text, cls) => ({
  tag, text, cls, children: [],
  append(...nodes) { this.children.push(...nodes); },
  addEventListener() {},
});
const eventNames = {}, time = (m) => String(m);
const eventTone = (e) => e.minute === 12 ? "unresolved" : "record";
const lower = () => 13, upper = () => 100, action = () => {}, seek = () => {};
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
  if (clickable) {
    assert.equal(b.children[0].children[0].text, "12 · 課題の記録");
    assert.equal(b.children[0].disabled, true);
    assert.equal(b.children[1].children[0].text, "19 · 経過の記録");
    assert.equal(b.children[1].disabled, false);
    assert.equal(toggle.hidden, false);
    assert.equal(b.scrollTop, 0);
  } else {
    assert.equal(b.scrollTop, b.scrollHeight);
  }
}
"""
    subprocess.run(
        [node, "--input-type=module", "-e", script, str(source)],
        check=True,
        capture_output=True,
        text=True,
    )


def test_report_highlights_follow_the_displayed_frame():
    """The three prominent facts use the frame's values and do not invent missing data."""
    node = shutil.which("node")
    if node is None:
        pytest.skip("Node is required for the UI callback regression")
    source = Path("src/sheltersim/web/static/demo-v2-simulation.js").resolve()
    script = r"""
import fs from 'node:fs';
import assert from 'node:assert/strict';
const source = fs.readFileSync(process.argv[1], 'utf8').replaceAll('\r\n','\n');
const body = source.split('  function renderKpiStrip(container, kpis) {')[1]
  .split('  // The open Run\'s name')[0];
const node = (tag, text, cls) => ({
  tag, text, cls, children: [],
  append(...nodes) { this.children.push(...nodes); },
  replaceChildren(...nodes) { this.children = nodes; },
});
const highlight = node('div');
const $ = () => highlight, el = node, uiIcon = () => node('icon');
const state = {minute: 10}, time = () => '時刻', num = v => v == null ? '—' : String(v);
const metricDefs = [
 {key:'stress_mean', factor:100, unit:'/100', label:'平均', note:'平均'},
 {key:'unresolved_support_request_count', factor:1, unit:'件', label:'相談', note:'相談'},
 {key:'staff_load_ratio', factor:100, unit:'%', label:'負荷', note:'負荷'},
];
const render = eval('(function renderKpiStrip(container, kpis) {' + body + ')');
const strip = node('div');
render(strip, {stress_mean:.42, unresolved_support_request_count:3, staff_load_ratio:.5});
const highlightValues = () => highlight.children.map(card => card.children[1].children[1].text);
assert.deepEqual(highlightValues(), ['42 /100', '3 件', '50 %']);
render(strip, undefined);
assert.deepEqual(highlightValues(), ['— /100', '— 件', '— %']);
"""
    subprocess.run([node, "--input-type=module", "-e", script, str(source)], check=True)
