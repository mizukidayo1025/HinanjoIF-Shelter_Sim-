"""What is open and the address that names it (demo-v2-state.js)."""

import shutil
import subprocess
from pathlib import Path

import pytest


def test_the_address_names_what_is_open_and_the_bare_screen_is_the_home():
    """The home with nothing open is the bare / (#376; /demo-v2 before), which opens the home
    again (#371); a Run
    or another Project than one's own is in the query, and any other page in the hash."""
    node = shutil.which("node")
    if node is None:
        pytest.skip("Node is required for the UI callback regression")
    source = Path("src/sheltersim/web/static/demo-v2-state.js").resolve()
    script = r"""
import fs from "node:fs";
import assert from "node:assert/strict";
const source = fs.readFileSync(process.argv[1], "utf8").replaceAll("\r\n", "\n");
const body = source.split("  function remember() {")[1].split("  Object.assign(app, {")[0];
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


def test_opening_a_run_uses_no_random_id():
    """The own Project's id is made only when a Scenario is first made (#371): opening a Run of
    the example uses no crypto.randomUUID, whose sequence the browser checks pin for the Run and
    branch ids (benchmarks/static_site_e2e.py)."""
    node = shutil.which("node")
    if node is None:
        pytest.skip("Node is required for the UI callback regression")
    source = Path("src/sheltersim/web/static/demo-v2-state.js").resolve()
    script = r"""
import fs from "node:fs";
import assert from "node:assert/strict";
const source = fs.readFileSync(process.argv[1], "utf8").replaceAll("\r\n", "\n");
const body = source.split("  const askedProject = ")[1].split("  const state = {")[0];
const run = (search, saved) => {
  let made = 0;
  const crypto = {randomUUID: () => `u${++made}`};
  const query = new URLSearchParams(search), isExample = (id) => /^sample(-|$)/.test(id);
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
// Every example's Project (`sample-…`) is an example's, not one's own.
assert.equal(run("?project=sample-2&run=r2", {}).ownId(), null);
assert.equal(run("", {project: "sample-3"}).ownId(), null);
assert.equal(run("?project=samples", {}).ownId(), "samples");
"""
    subprocess.run(
        [node, "--input-type=module", "-e", script, str(source)],
        check=True,
        capture_output=True,
        text=True,
    )
