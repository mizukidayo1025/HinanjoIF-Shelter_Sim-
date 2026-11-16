"""The scenario page's script (demo-v2-scenario.js): the preview of the form's conditions."""

import shutil
import subprocess
from pathlib import Path

import pytest


def test_the_preview_is_made_from_the_form_and_reads_a_text_only_with_the_api_on():
    """The form's conditions are the Scenario's (#583): with the API off (or not yet known) the
    text is not sent at all, even when the textarea holds one; with it on, the text with the
    form's basic conditions is read first and its request completed by the form, which wins."""
    node = shutil.which("node")
    if node is None:
        pytest.skip("Node is required for the UI callback regression")
    source = Path("src/sheltersim/web/static/demo-v2-scenario.js").resolve()
    script = r"""
import fs from "node:fs";
import assert from "node:assert/strict";
const source = fs.readFileSync(process.argv[1], "utf8").replaceAll("\r\n", "\n");
const body = source
  .split("  async function previewScenario() {")[1]
  .split("  function sourceRows(")[0];
const form = {
  municipality: "墨田区", disaster: "地震", evacuees: 100, duration_days: 7,
  field_origins: {municipality: "user", disaster: "user", evacuees: "user", duration_days: "user"},
};
const TEXT = "高齢者が多い。職員は8人。";
async function preview(llm) {
  const calls = [];
  const state = {inputVersion: 1, llm};
  const nodes = {
    "scenario-form": {reportValidity: () => true},
    "scenario-text": {value: ` ${TEXT} `},
  };
  const $ = (id) => nodes[id];
  const scenarioRequest = () => structuredClone(form);
  const ensureProject = async () => {}, ownProject = () => "p";
  const ownBase = () => "/api/v1/projects/p";
  const renderPreview = () => {};
  const api = async (path, sent) => {
    calls.push([path, sent]);
    return sent.request ? {scenario: {}} : {request: {
      municipality: "横浜市", disaster: "地震", evacuees: 100, staff_count: 8,
      field_origins: {
        municipality: "rules", disaster: "rules", evacuees: "rules", staff_count: "llm_extracted",
      },
    }};
  };
  const previewScenario = eval("(async function previewScenario() {" + body + ")");
  await previewScenario();
  return calls;
}
const path = "/api/v1/projects/p/scenario-preview";
for (const llm of [null, {on: false}]) {
  assert.deepEqual(await preview(llm), [[path, {text: "", request: form}]]);
}
assert.deepEqual(await preview({on: true}), [
  [path, {text: `${TEXT}\n入力欄の指定: 墨田区で、地震、100人、7日間`}],
  [path, {text: TEXT, request: {
    ...form, staff_count: 8,
    field_origins: {...form.field_origins, staff_count: "llm_extracted"},
  }}],
]);
"""
    subprocess.run(
        [node, "--input-type=module", "-e", script, str(source)],
        check=True,
        capture_output=True,
        text=True,
    )
