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


def test_the_screen_s_files_receive_only_what_another_file_gives():
    """The screen's files share functions and values through window.HinanjoApp (#491,
    docs/ui-backup.md): each gives what others use in `Object.assign(app, { … })` (a changing
    variable through a getter), takes what an earlier file gives with `const { … } = app` and a
    later file's functions with `const { … } = app.later`. A name taken through `app.later` that
    no file gives fails only when it is called, so every name taken is checked here, and every
    name given is taken by some file."""
    import re

    static = Path("src/sheltersim/web/static")
    html = (static / "demo-v2.html").read_text(encoding="utf-8")
    order = [
        name
        for name in re.findall(r'<script src="/(demo-v2[^"]*\.js)"', html)
        if "window.HinanjoApp" in (static / name).read_text(encoding="utf-8")
    ]
    assert order[0] == "demo-v2-state.js" and order[-1] == "demo-v2.js"
    names = lambda block: re.findall(r"[\w$]+", block)  # noqa: E731
    given, taken = {}, []
    for index, name in enumerate(order):
        source = (static / name).read_text(encoding="utf-8")
        for block in re.findall(r"^  Object\.assign\(app, \{(.*?)\}\);$", source, re.S | re.M):
            for item in names(block):
                assert item not in given, f"{item} is given by {given[item]} and {name}"
                given[item] = index
        for item in re.findall(r'^  Object\.defineProperty\(app, "(\w+)"', source, re.M):
            given[item] = index
        for block, later in re.findall(
            r"^  const \{(.*?)\} = app(\.later)?;$", source, re.S | re.M
        ):
            taken += [(name, index, item, bool(later)) for item in names(block)]
        taken += [(name, index, item, None) for item in re.findall(r"\bapp\.(\w+)\b", source)]
    for name, index, item, later in taken:
        if item == "later":
            continue
        assert item in given, f"{name} takes {item}, which no file gives"
        if later is True:
            assert given[item] > index, (
                f"{name} takes {item} through app.later from an earlier file"
            )
        elif later is False:
            assert given[item] < index, f"{name} takes {item} from a file loaded after it"
    unused = set(given) - {item for _, _, item, _ in taken}
    assert not unused, f"given but taken by no file: {sorted(unused)}"
    node = shutil.which("node")
    if node is not None:
        for name in order:
            subprocess.run([node, "--check", str(static / name)], check=True)


def test_the_result_questions_need_the_api_but_their_history_does_not():
    """The question input and its suggestions are in a `[data-needs-api]` fieldset, disabled while
    the API is OFF (#585); the questions asked before (`#chat`) stay outside it, readable."""
    import re

    html = Path("src/sheltersim/web/static/demo-v2.html").read_text(encoding="utf-8")
    box = re.split(r'<fieldset class="needs-api" data-needs-api>\s*<form id="question-form">', html)
    assert len(box) == 2, "the question form opens its own needs-api fieldset"
    inside = box[1].split("</fieldset>")[0]
    assert 'id="question"' in inside and 'id="ask"' in inside
    assert inside.count("data-question=") == html.count("data-question=") == 4
    assert 'id="chat"' in box[0] and 'id="chat"' not in inside
