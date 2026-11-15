"""The screen's entry script (demo-v2.js) and the text all its scripts show."""

import shutil
import subprocess
from pathlib import Path

import pytest


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
