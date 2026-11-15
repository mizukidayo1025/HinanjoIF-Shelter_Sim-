"""The Project routes' answers of the core, from Python (#449): what the browser answers.

`sheltersim_native.rules.answers` gives the answers of `/api/v1/projects/...` that the browser
gives through WASM (`sheltersim_core::answers`, js/browser_api.mjs): the local and VM versions
read the same files and call the same functions (#427). The cases of
`native/core/tests/data/api_answers.json` hold the Python server's answers to a Run the Project
screen made (`support/core_api_answers.py`; `native/wasm/tests/node_api_answers.mjs` asks the
browser the same); here the core answers them from the Run's records, its head made `ANCHOR`
as there. The records' answers are the records as they are, and a refusal has the server's
status, text and code.
"""

from __future__ import annotations

import hashlib
import json
from typing import Any

import pytest
from support.core_api_answers import normalize
from support.paths import CORE_TEST_DATA

from sheltersim import native

ANSWERS = native.rules("answers")
FIXTURE = json.loads((CORE_TEST_DATA / "api_answers.json").read_text(encoding="utf-8"))
HEAD = "a" * 64


def _record(value: dict[str, Any]) -> bytes:
    """A record as the store writes it: sorted keys, no spaces, a newline."""
    return (
        json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":")) + "\n"
    ).encode()


def _fixture_run() -> tuple[bytes, list[tuple[str, bytes]]]:
    """The records of the fixture's Run: its `run.json` and its branch `main`, at its end."""
    run = FIXTURE["run"]
    record = {
        "schema_version": 1,
        "project_id": run["project_id"],
        "scenario_id": "s",
        "scenario_version": 1,
        "scenario_hash": "0" * 64,
        "run_id": run["run_id"],
        "seed": run["seed"],
        "model_versions": {},
        "status": "completed",
        "archived": False,
    }
    branch = {
        "schema_version": 1,
        "run_id": run["run_id"],
        "branch_id": run["branch_id"],
        "parent_branch_id": None,
        "fork_minute": 0,
        "computed_until": run["minutes"],
        "checkpoint_hash": HEAD,
        "status": "completed",
    }
    return _record(record), [(run["branch_id"], _record(branch))]


def _refused(call: Any, *arguments: Any) -> tuple[int, str]:
    with pytest.raises(ANSWERS.Refused) as raised:
        call(*arguments)
    status, error, code, body = raised.value.args
    assert json.loads(body) == (
        {"error": error} if code is None else {"code": code, "error": error}
    )
    return status, body.decode()


@pytest.mark.parametrize(
    "case", FIXTURE["pdf_cases"], ids=lambda case: f"{case['run_id']} {case['body']}"
)
def test_pdf_report_is_refused_as_the_server_refuses_it(case: dict[str, Any]) -> None:
    run, branches = _fixture_run()
    project = FIXTURE["run"]["project_id"]
    files = (run, branches) if case["run_id"] == FIXTURE["run"]["run_id"] else (None, [])
    body = case["body"].encode()
    status, answer = _refused(ANSWERS.pdf_report_answer, project, case["run_id"], body, *files)
    assert (status, answer) == (case["status"], case["answer"])


def test_llm_status_is_the_server_s() -> None:
    (case,) = [case for case in FIXTURE["cases"] if case["route"] == "llm-status"]
    run = FIXTURE["run"]
    _, [(_, branch)] = _fixture_run()
    body = ANSWERS.llm_status_answer(
        case["query"], run["project_id"], run["run_id"], case["branch"], branch
    )
    expected = case["pages"][0]
    assert hashlib.sha256(normalize(body.decode(), HEAD).encode()).hexdigest() == expected["sha256"]
    status, _ = _refused(
        ANSWERS.llm_status_answer, "x=1", run["project_id"], run["run_id"], case["branch"], branch
    )
    assert status == 409
    assert _refused(
        ANSWERS.llm_status_answer, "", run["project_id"], run["run_id"], "nope", None
    ) == (
        409,
        '{"code":"storage_conflict","error":"saved artifact is missing or corrupt"}',
    )


def test_the_records_answers_are_the_records_as_they_are() -> None:
    run, branches = _fixture_run()
    project = _record({"schema_version": 1, "project_id": "sample", "archived": False})
    usage = ANSWERS.projects_answer(
        [("sample", project), ("runtime-accounts", project), ("../x", project)], 10, 2
    )
    assert (
        usage
        == b'{"projects":['
        + project.strip()
        + b'],"storage":{"bytes":10,"files":2,"limit_bytes":null}}'
    )
    run_document = run.strip()[:-1] + b',"branches":[' + branches[0][1].strip() + b"]}"
    assert ANSWERS.run_answer("sample", "r", run, branches) == run_document
    assert (
        ANSWERS.runs_answer("sample", project, [("r", run, branches), ("other", None, [])])
        == b'{"runs":[' + run_document + b"]}"
    )
    scenario = '{"id":"s","share":1.0}\n'.encode()
    revision = _record(
        {
            "schema_version": 1,
            "scenario_id": "s",
            "scenario_version": 1,
            "scenario_hash": "0" * 64,
            "approved": True,
        }
    )
    answer = ANSWERS.project_answer(
        "sample",
        project,
        [("s", [("v1.json", revision), ("v1.scenario.json", scenario)])],
        [("r", run, branches)],
    )
    expected = (
        project.strip()[:-1]
        + b',"scenarios":['
        + revision.strip()[:-1]
        + b',"scenario":{"id":"s","share":1.0}}],"runs":['
        + run_document
        + b"]}"
    )
    assert answer == expected
    assert (
        ANSWERS.evidence_answer("sample", "r", run, b'{"schema_version":1,"weight":0.50}\n')
        == b'{"schema_version":1,"weight":0.50}'
    )
    fork = b'{"status":"created","branch_id":"b","minute":3,"state_hash":"h"}'
    merged = json.loads(ANSWERS.run_answer("sample", "r", run, branches, fork))
    assert merged["last_fork"] == {
        "status": "created",
        "branch_id": "b",
        "minute": 3,
        "state_hash": "h",
    }
    assert (
        json.loads(ANSWERS.branch_of("sample", "r", run, branches, "main"))["checkpoint_hash"]
        == HEAD
    )
    assert (
        _refused(ANSWERS.run_answer, "sample", "../r", run, branches)[1]
        == '{"code":"storage_conflict","error":"invalid artifact identifier"}'
    )
    assert _refused(ANSWERS.branch_of, "sample", "r", run, branches, "nope")[0] == 409


def _conversations_piece(turns: list[bytes]) -> bytes:
    """A "conversations" piece of the texts' encoding: 0, the count, then each length and text."""

    def varint(number: int) -> bytes:
        out = bytearray()
        while True:
            byte = number & 0x7F
            number >>= 7
            out.append(byte | (0x80 if number else 0))
            if not number:
                return bytes(out)

    return varint(0) + varint(len(turns)) + b"".join(varint(len(turn)) + turn for turn in turns)


def test_conversations_and_delete_are_answered_as_the_browser_answers_them() -> None:
    turns = [b'{"answer":"a","claims":[],"question":"q1"}', b'{"answer":"b","question":"q2"}']
    answer = json.loads(ANSWERS.conversations_answer([_conversations_piece(turns)]))
    assert answer["turns"] == [
        {"schema_version": 2, "answer": "a", "claims": [], "question": "q1"},
        {"schema_version": 1, "answer": "b", "question": "q2"},
    ]
    assert (answer["coverage"], answer["history_route"]) == (
        "latest_100_turns",
        "history?kind=conversations",
    )
    many = json.loads(
        ANSWERS.conversations_answer([_conversations_piece([b'{"n":%d}' % n for n in range(120)])])
    )
    assert [turn["n"] for turn in many["turns"]] == list(range(20, 120))
    run, branches = _fixture_run()
    assert (
        ANSWERS.delete_answer("sample", "r", b'{"confirmation":"r"}', False, run, branches)
        == b'{"deleted":"r"}'
    )
    assert _refused(
        ANSWERS.delete_answer, "sample", "r", b'{"confirmation":"r"}', True, run, branches
    )[1] == ('{"code":"storage_conflict","error":"cannot delete an active run"}')
    assert _refused(ANSWERS.delete_answer, "sample", "r", b"{bad", False, run, branches) == (
        400,
        '{"error":"invalid project request"}',
    )
