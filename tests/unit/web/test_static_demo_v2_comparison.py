"""The comparison page's script (demo-v2-comparison.js): applying an intervention."""

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
