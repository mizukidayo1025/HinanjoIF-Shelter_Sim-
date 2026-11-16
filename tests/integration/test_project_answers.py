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
from support.core_interventions import mask
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


def test_a_run_record_of_either_schema_version_is_answered_as_it_is() -> None:
    """`run.json` of schema version 2 adds the Run's creation time (#447); one of version 1,
    written before, has none and is answered as it is. Version 2 without a creation time of the
    store's form, and another version, are refused."""
    run, branches = _fixture_run()
    old = json.loads(run)
    new = _record({**old, "schema_version": 2, "created_at": "2026-11-15T01:30:00Z"})
    for record in (run, new):
        assert json.loads(ANSWERS.run_answer("sample", "r", record, branches)) == {
            **json.loads(record),
            "branches": [json.loads(branches[0][1])],
        }
    for broken in (
        {**old, "schema_version": 2},
        {**old, "schema_version": 2, "created_at": "2026-11-15 10:30"},
    ):
        assert _refused(ANSWERS.run_answer, "sample", "r", _record(broken), branches)[1] == (
            '{"code":"storage_conflict","error":"saved artifact is missing or corrupt"}'
        )
    assert (
        "migration required"
        in _refused(
            ANSWERS.run_answer, "sample", "r", _record({**old, "schema_version": 3}), branches
        )[1]
    )


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


@pytest.fixture(scope="module")
def fixture_run() -> Any:
    """The fixture's Run computed to its end without Python (`PlainRun`), its blocks sealed."""
    run = native.rules("plain").PlainRun.from_worlds(
        (CORE_TEST_DATA / "api_answers.json").read_bytes(), "history"
    )
    assert run.advance(FIXTURE["run"]["minutes"]) == FIXTURE["run"]["minutes"]
    run.seal_blocks()
    return run


def _samples(run: Any) -> Any:
    """The Run's metric samples read back as a store reads its pieces: tables, then blocks."""
    table = ANSWERS.KpiTable()
    table.read_sample_tables(run.sample_tables(0, 0))
    for index in range(run.sample_block_counts()[0]):
        table.read_sample_block(run.sample_block(index))
    assert table.sample_count() == run.metric_count()
    return table


def _branch_file(branch_id: str) -> bytes | None:
    """The `branch.json` a store reads for a branch of the fixture's Run: `main`'s, or None."""
    _, [(name, branch)] = _fixture_run()
    return branch if branch_id == name else None


def _history(query: str, branch_id: str, table: Any) -> tuple[int, str]:
    """The core's answer to a history request of the fixture's Run: status and body."""
    run = FIXTURE["run"]
    branch = _branch_file(branch_id)
    try:
        plan = ANSWERS.history_plan(query, run["project_id"], run["run_id"], branch_id, branch)
        return 200, plan.answer(table).decode()
    except ANSWERS.Refused as refused:
        return refused.args[0], refused.args[3].decode()


def _same(got: tuple[int, str], expected: dict[str, Any]) -> bool:
    status, body = got
    if status != expected["status"]:
        return False
    if status != 200:
        return body == expected["body"]
    return hashlib.sha256(normalize(body, HEAD).encode()).hexdigest() == expected["sha256"]


def test_the_history_and_its_pages_are_the_server_s(fixture_run: Any) -> None:
    table = _samples(fixture_run)
    cases = [case for case in FIXTURE["cases"] if case["route"] == "history"]
    assert cases
    for case in cases:
        query = case["query"]
        for index, expected in enumerate(case["pages"]):
            got = _history(query, case["branch"], table)
            assert _same(got, expected), f"{query} page {index + 1}: {got[1][:300]}"
            if index + 1 < len(case["pages"]):
                query = f"{case['query']}&cursor={json.loads(got[1])['next_cursor']}"
    for foreign in FIXTURE["foreign_cursors"]:
        first = json.loads(_history(foreign["from"], "main", table)[1])
        got = _history(f"{foreign['to']}&cursor={first['next_cursor']}", "main", table)
        assert _same(got, foreign), foreign


def test_the_report_is_the_server_s(fixture_run: Any) -> None:
    run = FIXTURE["run"]
    _, [(_, branch)] = _fixture_run()
    ids = (run["project_id"], run["run_id"], run["branch_id"])
    for case in [case for case in FIXTURE["cases"] if case["route"] == "report"]:
        expected = case["pages"][0]
        try:
            stored = _branch_file(case["branch"])
            kept = ANSWERS.report_answer(case["query"], *ids[:2], case["branch"], stored, None)
        except ANSWERS.Refused as refused:
            assert (refused.args[0], refused.args[3].decode()) == (
                expected["status"],
                expected["body"],
            )
            continue
        assert kept is None, "a completed branch without a kept report makes it"
        scenario = FIXTURE["scenario_json"].encode()
        report = ANSWERS.build_report(fixture_run, scenario, case["branch"])
        assert _same((200, report.decode()), expected)
        assert ANSWERS.report_answer("", *ids, branch, report + b"\n") == report
    waiting = json.loads(branch)
    waiting["status"] = "paused"
    assert ANSWERS.report_answer("", *ids, _record(waiting), None) == b'{"status":"not_generated"}'


# ---- Frames: timeline, replay-frames and compare (native/core/tests/data/interventions.json) ----

INTERVENTIONS = json.loads((CORE_TEST_DATA / "interventions.json").read_text(encoding="utf-8"))
HEADS = {"main": "a" * 64, INTERVENTIONS["run"]["child"]: "b" * 64}


def _fork_files() -> tuple[bytes, list[tuple[str, bytes]]]:
    """The records of the interventions' Run: `main` and its child, both at the end."""
    run = INTERVENTIONS["run"]
    record = {
        "schema_version": 1,
        "project_id": run["project_id"],
        "scenario_id": "s",
        "scenario_version": 1,
        "scenario_hash": "0" * 64,
        "run_id": run["run_id"],
        "seed": run["seed"],
        "model_versions": run["model_versions"],
        "status": "completed",
        "archived": False,
    }

    def branch(branch_id: str, parent: str | None, fork: int) -> bytes:
        return _record(
            {
                "schema_version": 1,
                "run_id": run["run_id"],
                "branch_id": branch_id,
                "parent_branch_id": parent,
                "fork_minute": fork,
                "computed_until": run["minutes"],
                "checkpoint_hash": HEADS[branch_id],
                "status": "completed",
            }
        )

    child = run["child"]
    return _record(record), [
        ("main", branch("main", None, 0)),
        (child, branch(child, "main", run["fork_minute"])),
    ]


@pytest.fixture(scope="module")
def fork_frames() -> tuple[
    dict[str, dict[int, bytes]], dict[str, dict[int, bytes]], tuple[str, int, str, str]
]:
    """The frames of `main` and of the child at the minutes the cases ask for, made from Runs
    without Python: the child is `main` at the fork's minute with the approved program applied,
    as the browser's and the server's fork make it; the same frames as `replay-frames` gives them,
    without the world's digest (#500); and the child's fork receipt."""
    run = INTERVENTIONS["run"]
    applied = next(
        case
        for case in INTERVENTIONS["cases"]
        if case["route"] == "apply" and case["status"] == 200
    )
    approved = applied["document"]["approved_program_hash"]
    previews = [
        json.loads(case["text"])
        for case in INTERVENTIONS["cases"]
        if case["route"] == "preview" and case.get("text")
    ]
    program = next(
        preview["program"]
        for preview in previews
        if preview.get("approved_program_hash") == approved
    )
    program_text = json.dumps(
        program, sort_keys=True, ensure_ascii=False, separators=(",", ":")
    ).encode()
    assert hashlib.sha256(program_text).hexdigest() == approved
    raw = (CORE_TEST_DATA / "interventions.json").read_bytes()
    plain = native.rules("plain").PlainRun
    runs = {
        "main": plain.from_worlds(raw, "interventions"),
        run["child"]: plain.from_worlds(raw, "interventions"),
    }
    fork = run["fork_minute"]
    for each in runs.values():
        assert each.advance(fork - 1) == fork - 1
    # `main` a minute before the fork, for a question whose comparison is refused there.
    before = runs["main"].frame(run["run_id"], "main", run["minutes"])
    for each in runs.values():
        assert each.advance(1) == 1
    origin = runs[run["child"]].state_digest()
    runs[run["child"]].apply_effect_program(program_text)
    wanted = {
        fork,
        420,
        421,
        450,
        run["minutes"],
        *range(418, 426),
        *range(476, run["minutes"] + 1),
    }
    frames: dict[str, dict[int, bytes]] = {name: {} for name in runs}
    replays: dict[str, dict[int, bytes]] = {name: {} for name in runs}
    frames["main"][fork - 1] = before
    for minute in range(fork, run["minutes"] + 1):
        for name, each in runs.items():
            if each.minute() < minute:
                assert each.advance(1) == 1
            if minute in wanted:
                frames[name][minute] = each.frame(run["run_id"], name, run["minutes"])
                replays[name][minute] = each.replay_frame(run["run_id"], name, run["minutes"])
    return frames, replays, ("main", fork, origin, "c" * 64)


def _masked(body: bytes) -> str:
    return mask(body.decode(), HEADS)


def test_compare_replay_and_timeline_are_the_server_s(fork_frames: Any) -> None:
    frames, replays, receipt = fork_frames
    run, branches = _fork_files()
    info = INTERVENTIONS["run"]
    ids = (info["project_id"], info["run_id"])
    receipts = {"main": None, info["child"]: receipt}

    def refused_or(call: Any) -> tuple[int, Any]:
        try:
            return 200, call()
        except ANSWERS.Refused as refused:
            return refused.args[0], refused.args[3].decode()

    checked = 0
    for case in INTERVENTIONS["cases"]:
        route = case["route"]
        if route == "compare":
            if (
                case.get("body")
                == '{"code":"storage_conflict","error":"unknown project route or query"}'
            ):
                continue  # the route's own refusal of another query, before the answer
            status, plan = refused_or(
                lambda: ANSWERS.compare_plan(case["query"], *ids, run, branches)
            )
            if status == 200:
                plan.receipts_of([receipts[id] for id in plan.branch_ids])
                plan_frames = [frames[id][plan.minute] for id in plan.branch_ids]
                body = _masked(plan.answer(info["run_id"], plan_frames))
                assert hashlib.sha256(body.encode()).hexdigest() == case["sha256"], case["name"]
            else:
                assert (status, plan) == (case["status"], case["body"]), case["name"]
        elif route == "replay-frames":
            status, plan = refused_or(
                lambda: ANSWERS.replay_plan(case["query"], *ids, run, branches)
            )
            if status != 200:
                assert (status, plan) == (case["status"], case["body"]), case["name"]
                continue
            plan.receipts_of(*ids, run, branches, [receipts[id] for id in plan.branch_ids])
            samples = [
                plan.compare(
                    info["run_id"], minute, [replays[id][minute] for id in plan.branch_ids]
                )
                for minute in range(plan.start, plan.end + 1)
            ]
            body = _masked(plan.answer(samples))
            assert hashlib.sha256(body.encode()).hexdigest() == case["sha256"], case["name"]
        elif route == "timeline":
            _, minute = ANSWERS.timeline_plan(
                str(case["minute"]), *ids, run, branches, info["child"]
            )
            body = _masked(frames[info["child"]][minute])
            assert hashlib.sha256(body.encode()).hexdigest() == case["sha256"], case["name"]
        else:
            continue
        checked += 1
    assert checked >= 15
    status, body = refused_or(lambda: ANSWERS.timeline_plan("x", *ids, run, branches, "main"))
    assert (status, body) == (
        409,
        '{"code":"storage_conflict","error":"minute must be an integer"}',
    )


# ---- Interventions and questions (native/core/tests/data/interventions.json) --------------------


def _main_only() -> tuple[bytes, list[tuple[str, bytes]]]:
    """The records of the interventions' Run before the fork: `main` alone."""
    run, branches = _fork_files()
    return run, [entry for entry in branches if entry[0] == "main"]


def test_previews_and_applies_are_the_server_s() -> None:
    info = INTERVENTIONS["run"]
    ids = (info["project_id"], info["run_id"])
    raw = (CORE_TEST_DATA / "interventions.json").read_bytes()
    plain = native.rules("plain").PlainRun
    worlds: dict[int, Any] = {}
    drafts: dict[str, bytes] = {}
    applied_preview = None
    run, branches = _main_only()
    checked = 0
    for case in INTERVENTIONS["cases"]:
        if case["route"] == "preview":
            body = case["request"].encode() if isinstance(case["request"], str) else case["request"]
            try:
                plan = ANSWERS.preview_plan(body, *ids, run, branches, "main")
                plan.check_typed()
                if plan.minute not in worlds:
                    worlds[plan.minute] = plain.from_worlds(raw, "interventions")
                    worlds[plan.minute].advance(plan.minute)
                preview_id = f"{len(drafts) + 1:032x}"
                answer, draft = plan.answer(worlds[plan.minute], preview_id)
            except ANSWERS.Refused as refused:
                assert (refused.args[0], refused.args[3].decode()) == (
                    case["status"],
                    case["body"],
                ), case["name"]
                checked += 1
                continue
            assert case["status"] == 200, case["name"]
            text = _masked(answer)
            assert hashlib.sha256(text.encode()).hexdigest() == case["sha256"], case["name"]
            if draft is not None:
                drafts[preview_id] = draft
                if applied_preview is None and json.loads(answer)["can_apply"]:
                    applied_preview = preview_id
            checked += 1
        elif case["route"] == "apply":
            document = dict(case["document"])
            if document["preview_id"] == "$PREVIEW":
                document["preview_id"] = applied_preview
            body = json.dumps(document).encode()
            try:
                draft_id = ANSWERS.apply_draft_id(body)
                plan = ANSWERS.apply_plan(body, drafts.get(draft_id), *ids, "main", False)
                plan.fork_checks(run, branches)
            except ANSWERS.Refused as refused:
                assert (refused.args[0], refused.args[3].decode()) == (
                    case["status"],
                    case["body"],
                ), case["name"]
                checked += 1
                continue
            assert case["status"] == 200, case["name"]
            assert plan.answer().decode() == case["text"], case["name"]
            # The child is there from now on.
            branches = _fork_files()[1]
            checked += 1
    assert checked >= 25 and applied_preview is not None


def test_questions_are_the_server_s(fork_frames: Any) -> None:
    """Without an API key each question is refused: by its checks, or once it passes them as the
    question that needs the API (#585)."""
    frames, _, _ = fork_frames
    run, branches = _fork_files()
    info = INTERVENTIONS["run"]
    ids = (info["project_id"], info["run_id"])
    scenario = INTERVENTIONS["scenario_json"].encode()
    agent = json.loads(frames["main"][info["minutes"]])["people"][0]["id"]
    for case in [case for case in INTERVENTIONS["cases"] if case["route"] == "questions"]:
        document = {
            key: agent if value == "$AGENT" else value for key, value in case["document"].items()
        }
        with pytest.raises(ANSWERS.Refused) as refused:
            plan = ANSWERS.question_plan(json.dumps(document).encode(), *ids, case["branch"])
            plan.check_branch(run, branches)
            plan.evidence(scenario)
            compared = plan.check_frame(frames[case["branch"]][plan.minute])
            if compared:
                ANSWERS.comparison_range(compared, plan.minute, *ids, run, branches)
            plan.offline()
        assert (refused.value.args[0], refused.value.args[3].decode()) == (
            case["status"],
            case["body"],
        ), case["name"]
