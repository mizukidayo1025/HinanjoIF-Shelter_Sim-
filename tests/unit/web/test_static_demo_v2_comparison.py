"""The comparison page's script (demo-v2-comparison.js): building, previewing and applying an
intervention."""

import shutil
import subprocess
from pathlib import Path

import pytest


def test_applying_an_intervention_waits_for_the_fork_process_to_list_the_child():
    """The server forks in its own process, so intervention-apply answers before the
    child Branch exists. The screen must wait for it, as /demo does, or the comparison it
    opens next finds no Branch (「介入後の分岐を選択してください。」)."""
    node = shutil.which("node")
    if node is None:
        pytest.skip("Node is required for the UI callback regression")
    source = Path("src/sheltersim/web/static/demo-v2-comparison.js").resolve()
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


def _run_node(script: str) -> None:
    node = shutil.which("node")
    if node is None:
        pytest.skip("Node is required for the UI callback regression")
    source = Path("src/sheltersim/web/static/demo-v2-comparison.js").resolve()
    subprocess.run(
        [node, "--input-type=module", "-e", script, str(source)],
        check=True,
        capture_output=True,
        text=True,
    )


# The builder's functions (from its catalog to `builtProgram`), with a page of fake controls.
BUILDER = r"""
import fs from "node:fs";
import assert from "node:assert/strict";
const source = fs.readFileSync(process.argv[1], "utf8").replaceAll("\r\n", "\n");
const body = "  let catalog = null," + source
  .split("  let catalog = null,")[1]
  .split("  // What a preview was made of")[0];
const controls = {};
const control = (id, value = "") => (controls[id] ??= {
  id, value, options: [], children: [], dataset: {}, disabled: false, hidden: false,
  textContent: "",
  get selectedOptions() { return this.options.filter((o) => o.value === this.value); },
  replaceChildren(...items) {
    this.children = items;
    if (id === "intervention-target" || id === "intervention-op") this.options = items;
  },
  append(...items) { this.children.push(...items); },
  querySelector(selector) {
    const name = selector.match(/data-name="([^"]+)"/)[1];
    const walk = (items) => items.flatMap((item) => [item, ...walk(item.children || [])]);
    return walk(this.children).find((item) => item.dataset?.name === name);
  },
});
const $ = (id) => control(id);
const el = (tag, text) => ({ tag, textContent: text ?? "", value: "", children: [], dataset: {},
  append(...items) { this.children.push(...items); } });
const zoneNames = {};
const pad = (n) => String(n).padStart(2, "0");
const time = (m) =>
  `${Math.floor(m / 1440) + 1}日目 ${pad(Math.floor((m % 1440) / 60))}:${pad(m % 60)}`;
const comparing = () => false;
const base = () => "/p";
const state = { run: "r", frame: {
  zones: [{ id: "toilet", name: "トイレ" }, { id: "food", name: "食事・給水スペース" }],
  facilities: [
    { id: "toilet_01", kind: "toilet", zone_id: "toilet" },
    { id: "water_station_main", kind: "water_station", zone_id: "food" },
  ],
} };
const entries = [
  { op: "add_toilet", label: "トイレを増やす", target_kinds: ["zone"], target_facility_kinds: [],
    units: ["toilet_stall"], required_policies: ["facility_capacity"], timed: false,
    end_required: false, parameters: [{ name: "count", label: "増やす数", unit: "基",
      type: "integer", minimum: 1, maximum: 20, required: true }] },
  { op: "water_outage", label: "断水する", target_kinds: ["facility", "zone"],
    target_facility_kinds: ["water_station"], units: ["water_supply"],
    required_policies: ["operational_events_v1", "resource_service_v1"], timed: true,
    end_required: true, parameters: [] },
];
let asked = 0;
const api = async () => { asked++; return { executable_effect_primitives: entries }; };
const builder = eval(
  `(() => {${body}; return { renderBuilder, builtProgram, loaded: () => catalogLoading };})()`,
);
"""


def test_the_builder_makes_a_program_of_the_catalog_s_operator_target_and_amount():
    """The intervention builder (#584) offers the core catalog's operators, the targets of the
    world of the kinds each takes (a water outage: water stations and zones), the amounts within
    the catalog's ranges, and builds the program from the catalog's units and policies with the
    screen as its compiler (`manual`)."""
    _run_node(
        BUILDER
        + r"""
builder.renderBuilder();
await builder.loaded();
assert.equal(asked, 1);
const values = (id) => controls[id].options.map((o) => o.value);
assert.deepEqual(values("intervention-op"), ["add_toilet", "water_outage"]);
controls["intervention-op"].value = "add_toilet";
builder.renderBuilder();
assert.deepEqual(values("intervention-target"), ["zone:toilet", "zone:food"]);
controls["intervention-target"].value = "zone:toilet";
const count = controls["intervention-amounts"].querySelector('[data-name="count"]');
assert.equal(count.value, "1");
count.value = "3";
const { program, label } = builder.builtProgram({ branch: "main", minute: 600 });
assert.equal(label, "トイレを増やす · 区域：トイレ · 増やす数3基 · 1日目 10:00から");
const [effect] = program.effects;
assert.equal(effect.op, "add_toilet");
assert.deepEqual(effect.target_selector, { kind: "zone", target_id: "toilet", candidate_ids: [] });
assert.deepEqual(effect.parameters, { count: 3 });
assert.equal(effect.units, "toilet_stall");
assert.deepEqual(effect.policy_dependencies, ["facility_capacity"]);
assert.equal(effect.end_minute, null);
assert.equal(program.compiler_provenance.compiler_kind, "manual");
assert.equal(program.simulation_start_minute, 600);
const at600 = () => builder.builtProgram({ branch: "main", minute: 600 });
count.value = "21";
assert.throws(at600, /増やす数は1〜20基の整数/);

controls["intervention-op"].value = "water_outage";
builder.renderBuilder();
assert.deepEqual(values("intervention-target"),
  ["facility:water_station_main", "zone:toilet", "zone:food"]);
assert.equal(controls["intervention-duration-row"].hidden, false);
controls["intervention-target"].value = "facility:water_station_main";
controls["intervention-duration"].value = "";
assert.throws(at600, /続ける時間を入れてください/);
controls["intervention-duration"].value = "90";
const water = at600().program.effects[0];
assert.equal(water.end_minute, 690);
assert.equal(water.target_selector.kind, "facility");
"""
    )


def test_a_text_is_never_sent_while_the_api_is_off():
    """The text the AI reads is an option of the API (#580, #584): with the API OFF (or not yet
    known) the preview refuses it before any request."""
    _run_node(
        r"""
import fs from "node:fs";
import assert from "node:assert/strict";
const source = fs.readFileSync(process.argv[1], "utf8").replaceAll("\r\n", "\n");
const body = source
  .split("  async function previewIntervention(")[1]
  .split("  async function applyIntervention(")[0];
let sent = 0;
const api = async () => { sent++; return {}; };
const $ = () => ({ value: "トイレを2基追加" });
const interventionBase = () => ({ branch: "main", minute: 60 });
for (const llm of [null, { on: false }]) {
  const state = { data: { status: "paused" }, llm, scope: 1 };
  const previewIntervention = eval("(async function previewIntervention(" + body + ")");
  await assert.rejects(previewIntervention("text"), /API を ON にすると使えます/);
}
assert.equal(sent, 0);
"""
    )
