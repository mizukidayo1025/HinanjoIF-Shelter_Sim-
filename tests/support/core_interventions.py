"""The Python server's answers to an intervention and its child branch (#306).

`native/wasm/tests/node_interventions.mjs` computes the same Run in WASM, stores it as the browser
does and asks the same routes through `native/wasm/js/browser_api.mjs`: each answer must be the
server's, byte for byte, once what may differ is masked (`normalize`).

The Run is the one the Project screen makes of a generated Scenario (`EVACUEES` people, `MINUTES`
minutes, seed 2026), computed to its end, as `core_api_answers.py` makes it, on a server whose
worker is a process (as the VM's). Then, at `FORK_MINUTE` of `main`: `intervention-preview` of
each of `PREVIEWS` (and the server's refusals), `intervention-apply` of the first one to the child
`CHILD`, the Run's record with its `last_fork`, the refusals of `APPLY_REFUSALS`, the child
computed to the end, and its frames and KPI history.

Then, with an API key (#470), on a server whose Provider is a fake answering each call with the
next of the case's answers (`LLM_QUESTIONS`, `LLM_PREVIEWS`, `LLM_REPORTS`): the questions,
previews and analysis reports the browser asks through the site's relay, in
`interventions_llm.json` (`interventions_llm_fixture`). The stored Runs `fork_parent` and `fork`
(native/core/tests/data/stored_runs/) keep `interventions.json`'s bytes as their world's export:
after writing it again, write them again too (`node_stored_runs.mjs PKG --write`). Each such case
holds,
for each call, the sha256 of the core's task (`llm::Task`'s JSON, which the browser must send the
relay as it is) and the reply the server's link gave (`llm::Reply`'s JSON: the text, the outcome,
the model, the tokens, the cost) with the text the Provider wrote, which the browser's fake relay
answers. A question's evidence holds the Run's `evidence.json`: the fixture holds its sha256,
which the browser's (the core's `runEvidence` of the Scenario) must have.

Masked (`normalize`): `preview_id` (the server's uuid4) and each branch's head (the server's
Checkpoint's hash, the browser's journal line's), in a Run's record and in a history's
`anchor_hash` and cursor, are replaced by fixed words. The fixture holds the world at minute 0, the
Scenario's canonical JSON and, for each request, its status and the sha256 of its body (a refusal's
body as it is).
"""

from __future__ import annotations

import functools
import hashlib
import json
import re
import runpy
import tempfile
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

from support.core_api_answers import _isolated, normalize
from support.paths import FIXTURES

EVACUEES = 12
MINUTES = 480
RUN_ID = "r"
CHILD = "c1"
FORK_MINUTE = 400
PREVIEW = "PREVIEW"
HEAD = "HEAD"
CREATED_AT = datetime(2026, 11, 15, 1, 30, tzinfo=UTC)

# Bodies of `intervention-preview` on main; the first one is applied.
PREVIEWS: list[str] = [
    json.dumps({"minute": FORK_MINUTE, "text": "7:00 toilet_01 outage"}),
    json.dumps({"minute": FORK_MINUTE, "text": "トイレを1基追加"}),
    json.dumps({"minute": FORK_MINUTE, "text": "7:00から7:30まで断水"}),
    json.dumps({"minute": FORK_MINUTE, "text": "7:00 断水"}),
    json.dumps({"minute": FORK_MINUTE, "text": "スタッフを2人追加"}),
    json.dumps({"minute": FORK_MINUTE, "text": "7:00 避難者が5人到着"}),
    json.dumps({"minute": FORK_MINUTE, "text": "雨が降る"}),
    json.dumps({"minute": FORK_MINUTE, "text": "Day 2 toilet outage"}),
    json.dumps({"minute": 0, "text": "toilet_01 outage"}),
    json.dumps({"minute": MINUTES, "text": "toilet_01 outage"}),
    json.dumps({"minute": FORK_MINUTE, "text": "連絡先 a@b.cc 7:00 toilet_01 outage"}),
    json.dumps({"minute": FORK_MINUTE, "text": ""}),
    json.dumps({"minute": FORK_MINUTE, "text": 5}),
    json.dumps({"minute": FORK_MINUTE}),
    json.dumps({"minute": MINUTES + 1, "text": "x"}),
    json.dumps({"minute": "1", "text": "x"}),
    json.dumps({"minute": FORK_MINUTE, "text": "x", "extra": 1}),
    json.dumps({"text": "x"}),
    "[1]",
    "{bad",
]

# `compare` and `replay-frames` of the two branches once the child is computed (#307).
COMPARISONS: list[tuple[str, str]] = [
    ("compare", f"branch_ids=main,{CHILD}&minute={FORK_MINUTE}"),
    ("compare", f"branch_ids=main,{CHILD}&minute=421"),
    ("compare", f"branch_ids={CHILD},main&minute={MINUTES}"),
    ("compare", f"branch_ids=main,{CHILD}&minute=4_21"),
    ("compare", f"branch_ids=main,{CHILD}&minute={FORK_MINUTE - 1}"),
    ("compare", "branch_ids=main&minute=420"),
    ("compare", "branch_ids=main,main&minute=420"),
    ("compare", "branch_ids=main,nope&minute=420"),
    ("compare", f"branch_ids=main,{CHILD}&minute=x"),
    ("compare", f"branch_ids=main,{CHILD}&minute=420&minute=421"),
    ("compare", f"branch_ids=main,{CHILD}"),
    ("replay-frames", f"branch_ids=main,{CHILD}&start=418&end=425"),
    ("replay-frames", f"branch_ids={CHILD},main&start=476&end={MINUTES}"),
    ("replay-frames", f"branch_ids=main,{CHILD}&start={FORK_MINUTE - 1}&end={FORK_MINUTE + 1}"),
    ("replay-frames", f"branch_ids=main,{CHILD},x&start=420&end=421"),
]

# `questions` once the child is computed (#308): (branch, body). "$AGENT" is a person at the end
# of main. Without an API key, a question that passes its checks is refused (#585).
QUESTIONS: list[tuple[str, dict[str, Any]]] = [
    ("main", {"question": "何が起きた?"}),
    ("main", {"question": "x", "context_id": "c" + "0" * 32 + "-" + "0" * 24}),
    ("main", {"question": "トイレの列は?", "minute": 420, "comparison_branch_ids": [CHILD]}),
    (CHILD, {"question": "分岐の後は?", "minute": 450, "comparison_branch_ids": ["main"]}),
    ("main", {"question": "この人は?", "agent_id": "$AGENT", "minute": MINUTES}),
    ("main", {"question": "連絡先 a@b.cc agent A001 は?", "run_id": "other"}),
    ("main", {"question": "x", "agent_id": "nope"}),
    ("main", {"question": ""}),
    ("main", {"question": "   "}),
    ("main", {"question": "a" * 2001}),
    ("main", {"question": "x", "extra": 1}),
    ("main", {"question": "x", "minute": -1}),
    ("main", {"question": "x", "minute": True}),
    ("main", {"question": "x", "comparison_branch_ids": CHILD}),
    ("main", {"question": "x", "comparison_branch_ids": ["a"] * 8}),
    ("main", {"question": "x", "comparison_branch_ids": ["nope"]}),
    ("main", {"question": "x", "comparison_branch_ids": ["main"]}),
    ("main", {"question": "x", "minute": FORK_MINUTE - 1, "comparison_branch_ids": [CHILD]}),
    ("main", {"question": "x", "minute": MINUTES + 1}),
    ("main", {"question": "x", "demo_branch_id": "d"}),
    ("main", {"question": "x", "context_id": 5}),
    ("main", {"question": "x", "context_id": "a" * 65}),
    ("nope", {"question": "x"}),
]

# `questions` asked with an API key (#470), after every other case: (branch, body, answers). Each
# answer is what the fake Provider writes for the next call (`_llm_answer`). "$LLM_CONTEXT" is the
# context_id of the first answer.
LLM_QUESTIONS: list[tuple[str, dict[str, Any], list[str]]] = [
    ("main", {"question": "記録された値は?"}, ["cite"]),
    ("main", {"question": "なぜそうなった?", "context_id": "$LLM_CONTEXT"}, ["cite"]),
    (
        "main",
        {"question": "分岐と比べると?", "minute": 420, "comparison_branch_ids": [CHILD]},
        ["cite"],
    ),
    ("main", {"question": "答えの形が違うと?"}, ["not json"]),
    # The question kept and asked with contact details and entity names redacted.
    ("main", {"question": "連絡先 a@b.cc agent A001 は?", "run_id": "other"}, ["cite"]),
]

# `intervention-preview` on main with an API key (#470): (body, answers). The second plan is
# refused (an end minute on a one-shot operation) and repaired once.
LLM_PREVIEWS: list[tuple[dict[str, Any], list[str]]] = [
    ({"minute": FORK_MINUTE, "text": "仮設トイレを2基置いてほしい"}, ["toilets"]),
    ({"minute": FORK_MINUTE, "text": "トイレを2基増やしてほしい"}, ["toilets until", "toilets"]),
    ({"minute": FORK_MINUTE, "text": "雨漏りの対策をしたい"}, ["not json"]),
]

# `pdf-report` with an API key (#470): (body, answers). "prose" is the grounded analysis of
# `support.reports`.
LLM_REPORTS: list[tuple[dict[str, Any], list[str]]] = [
    ({"branch_ids": ["main", CHILD], "minute": 420, "analysis": True}, ["prose"]),
    # Without the analysis (#582): no call, though there is a link.
    ({"branch_ids": ["main", CHILD], "minute": 420, "analysis": False}, []),
]


def _scalars(data: Any, prefix: str = "") -> Any:
    for key, value in data.items():
        path = f"{prefix}{key}"
        if isinstance(value, dict):
            yield from _scalars(value, path + ".")
        elif isinstance(value, (str, int, float, bool)):
            yield path, value


def _llm_answer(kind: str, task: Any) -> str:
    """The fake Provider's text for a task: `cite` a question's answer citing a recorded string
    and number of its evidence; `toilets` a plan adding two toilets to the first zone at the
    minute asked (`toilets until` with an end minute, which the checks refuse); `prose` the PDF
    report's grounded analysis; else `kind`."""
    if kind == "prose":
        from support.reports import _prose_document

        return json.dumps(_prose_document(), ensure_ascii=False)
    payload = json.loads(task.input_text)
    if kind == "cite":
        found = [
            (item["alias"], path, value)
            for item in payload["evidence"]
            for path, value in _scalars(item["data"])
        ]
        text = next(item for item in found if isinstance(item[2], str))
        number = next(
            item for item in found if isinstance(item[2], (int, float)) and item[2] is not True
        )
        cited = {"alias": number[0], "field": number[1], "value": json.dumps(number[2])}
        return json.dumps(
            {
                "claims": [
                    {"evidence": [{"alias": text[0], "field": text[1], "value": text[2]}]},
                    {"evidence": [cited]},
                ],
                "explanations": [
                    {
                        "kind": "hypothesis",
                        "text": "待ちの長さは処理能力に関係する可能性があります。",
                        "evidence": [cited],
                    }
                ],
            },
            ensure_ascii=False,
        )
    if kind.startswith("toilets"):
        effect = {
            "op": "add_toilet",
            "target_kind": "zone",
            "target_id": payload["target_ids_by_kind"]["zone"][0],
            "start_minute": payload["branch"]["selected_minute"],
            "parameters": {"count": 2},
        }
        if kind == "toilets until":
            effect["end_minute"] = payload["branch"]["selected_minute"] + 30
        return json.dumps(
            {
                "status": "compiled",
                "support_mode": "native",
                "mechanism_summary": "仮設トイレを2基追加する作用に対応付けました。",
                "effects": [effect],
                "assumptions": [],
                "limitations": [],
                "questions": [],
                "choices": [],
            },
            ensure_ascii=False,
        )
    return kind


class _ScriptedProvider:
    """A fake Provider: each call answers with the next of `answers` ("not json" once they run
    out), 10 input and 5 output tokens."""

    def __init__(self) -> None:
        self.answers: list[str] = []
        self.last: str | None = None

    def complete(self, *, model_id: str, task: Any, max_output_tokens: int, timeout_seconds: float):
        from sheltersim.llm.contracts import ProviderResponse

        del max_output_tokens, timeout_seconds
        kind = self.answers.pop(0) if self.answers else "not json"
        self.last = _llm_answer(kind, task)
        return ProviderResponse(
            text=self.last,
            model_id=model_id,
            input_tokens=10,
            output_tokens=5,
            elapsed_ms=0.0,
        )


def _llm_cases(api: Any, heads: Any) -> list[dict[str, Any]]:
    """The cases asked with an API key (see the head of this module)."""
    import pytest

    from sheltersim.llm.link import RouterLink

    branch = ["sample", "runs", RUN_ID, "branches"]
    provider = _ScriptedProvider()
    calls: list[dict[str, Any]] = []
    complete = RouterLink.complete

    def keep(self: RouterLink, task: bytes) -> bytes:
        provider.last = None
        reply = complete(self, task)
        calls.append(
            {
                "task_sha256": hashlib.sha256(task).hexdigest(),
                # What the Provider wrote (the reply's text is the fallback where it did not fit).
                "output_text": provider.last,
                "reply": json.loads(reply),
            }
        )
        return reply

    cases: list[dict[str, Any]] = []
    with pytest.MonkeyPatch.context() as monkeypatch:
        monkeypatch.setenv("SHELTERSIM_LLM_MODE", "record")
        monkeypatch.setattr("sheltersim.llm.provider.AnthropicProvider", lambda _key: provider)
        monkeypatch.setattr(RouterLink, "complete", keep)
        context = None
        for branch_id, question, answers in LLM_QUESTIONS:
            document = {
                key: context if value == "$LLM_CONTEXT" else value
                for key, value in question.items()
            }
            provider.answers, calls[:] = list(answers), []
            status, text = _post(api, [*branch, branch_id, "questions"], json.dumps(document))
            if context is None and status == 200:
                context = json.loads(text)["context_id"]
            name = f"question with the LLM {json.dumps(question, ensure_ascii=False)[:80]}"
            cases.append(
                {
                    "route": "questions",
                    "branch": branch_id,
                    "document": question,
                    "llm": list(calls),
                    **_case(name, status, text, heads()),
                }
            )
        for document, answers in LLM_PREVIEWS:
            provider.answers, calls[:] = list(answers), []
            body = json.dumps(document, ensure_ascii=False)
            status, text = _post(api, [*branch, "main", "intervention-preview"], body)
            cases.append(
                {
                    "route": "preview",
                    "request": body,
                    "llm": list(calls),
                    **_case(f"preview with the LLM {body}", status, text, heads()),
                }
            )
        for document, answers in LLM_REPORTS:
            provider.answers, calls[:] = list(answers), []
            status, text = _pdf(api, document)
            cases.append(
                {
                    "route": "pdf-report",
                    "document": document,
                    "llm": list(calls),
                    **_case(f"report with the LLM {json.dumps(document)}", status, text, heads()),
                }
            )
    return cases


# `pdf-report` of the child and of both branches (#309): the server's checks, then the report
# without the analysis (#582), and its 409 for an analysis asked without an API key.
PDF_REPORTS: list[dict[str, Any]] = [
    {"branch_ids": ["main", CHILD], "minute": 420},
    {"branch_ids": [CHILD, "main"], "minute": MINUTES},
    {"branch_ids": ["main", CHILD], "minute": FORK_MINUTE - 1},
    {"branch_ids": [CHILD], "minute": FORK_MINUTE - 1},
    {"branch_ids": [CHILD], "minute": FORK_MINUTE},
    {"branch_ids": ["main", CHILD], "minute": 420, "analysis": True},
]


def _pdf(api: Any, document: dict[str, Any]) -> tuple[int, str]:
    """The server's answer to `pdf-report`."""
    from support.web import server_answer

    return server_answer(
        api, "POST", ["sample", "runs", RUN_ID, "pdf-report"], body=_canonical(document)
    )


def _placeholder(document: dict[str, Any], preview_id: str) -> dict[str, Any]:
    """The document with the server's (random) preview_id written as "$PREVIEW"."""
    return {
        key: "$PREVIEW" if key == "preview_id" and value == preview_id else value
        for key, value in document.items()
    }


def _canonical(value: Any) -> str:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))


def _post(api: Any, parts: list[str], body: str) -> tuple[int, str]:
    """The server's status and body of a project POST (web/server.py's do_POST)."""
    from support.web import server_answer

    return server_answer(api, "POST", parts, body=body)


def _get(api: Any, parts: list[str], query: str = "") -> tuple[int, str]:
    from support.web import server_answer

    return server_answer(api, "GET", parts, query)


def mask(body: str, heads: dict[str, str]) -> str:
    """The body with `preview_id` and the branches' heads replaced, as the Node test does."""
    body = re.sub(r'"preview_id":"[0-9a-f]{32}"', f'"preview_id":"{PREVIEW}"', body)
    # A fork receipt names the parent's Checkpoint (the server's) or head (the browser's).
    body = re.sub(r'"checkpoint_hash":"[0-9a-f]{64}"', f'"checkpoint_hash":"{HEAD}"', body)
    # A question's context id ends with random digits.
    body = re.sub(r'"context_id":"(c[0-9a-f]{32})-[0-9a-f]{24}"', r'"context_id":"\1-RANDOM"', body)
    for head in heads.values():
        if head:
            body = normalize(body, head).replace("ANCHOR", HEAD)
    return body


def _case(name: str, status: int, body: str, heads: dict[str, str]) -> dict[str, Any]:
    text = mask(body, heads) if status == 200 else body
    case: dict[str, Any] = {"name": name, "status": status}
    if status == 200:
        case["sha256"] = hashlib.sha256(text.encode()).hexdigest()
        case["length"] = len(text.encode())
        case["text"] = text if len(text) < 4000 else None
    else:
        case["body"] = text
    return case


def interventions_fixture() -> dict[str, Any]:
    """`interventions.json`: the server's answers without an API key."""
    return _server_answers()[0]


def interventions_llm_fixture() -> dict[str, Any]:
    """`interventions_llm.json`: the server's answers with an API key, after the others."""
    return _server_answers()[1]


@functools.cache
def _server_answers() -> tuple[dict[str, Any], dict[str, Any]]:
    import sheltersim_native

    from sheltersim.experiments.scenario import ScenarioSpec
    from sheltersim.storage.projects import ProjectStore, canonical, digest
    from sheltersim.web.projects import ProjectAPI
    from support.generated import _generated_request

    sample = runpy.run_path(str(FIXTURES.parent / "scripts" / "build_project_sample.py"))
    request = _generated_request(evacuees=EVACUEES, duration_days=MINUTES / 1440)
    branch = ["sample", "runs", RUN_ID, "branches"]
    with tempfile.TemporaryDirectory(prefix="interventions-") as scratch, _isolated(Path(scratch)):
        store = ProjectStore(Path(scratch) / "projects")
        # The Run's creation time the browser's test gives its Run (#447).
        api = ProjectAPI(store, worker="process", recover=False, clock=lambda: CREATED_AT)
        api.workspace.research.research = sample["_no_research"]

        def heads() -> dict[str, str]:
            return {
                item["branch_id"]: item["checkpoint_hash"]
                for item in json.loads(api.files.branches("sample", RUN_ID))
            }

        try:
            draft, _ = sample["_preview"](api, "", request)
            api.post(
                ["sample", "scenario-approve"],
                {
                    "preview_id": draft["preview_id"],
                    "approved_hash": draft["approved_hash"],
                    "approved": True,
                    "run_id": RUN_ID,
                },
            )
            api.post(
                ["sample", "runs", RUN_ID, "resume"],
                {"branch_id": "main", "target_minute": MINUTES},
            )
            api.wait_idle(timeout=600)
            cases = []
            previews = []
            for body in PREVIEWS:
                status, text = _post(api, [*branch, "main", "intervention-preview"], body)
                previews.append((status, text))
                cases.append(
                    {"route": "preview", "request": body, **_case(body, status, text, heads())}
                )
            applied = json.loads(previews[0][1])
            if not applied.get("can_apply"):
                raise RuntimeError(f"the first preview cannot be applied: {previews[0][1][:400]}")
            apply = {
                "preview_id": applied["preview_id"],
                "approved_program_hash": applied["approved_program_hash"],
                "approved": True,
                "child_id": CHILD,
            }
            refusals = [
                {**apply, "approved": False},
                {**apply, "approved_program_hash": "0" * 64},
                {**apply, "preview_id": "0" * 32},
                {**apply, "child_id": "../x"},
                {key: value for key, value in apply.items() if key != "child_id"},
            ]
            for document in refusals:
                status, text = _post(
                    api, [*branch, "main", "intervention-apply"], json.dumps(document)
                )
                cases.append(
                    {
                        "route": "apply",
                        "document": _placeholder(document, apply["preview_id"]),
                        **_case("refused apply", status, text, heads()),
                    }
                )
            status, text = _post(api, [*branch, "main", "intervention-apply"], json.dumps(apply))
            cases.append(
                {
                    "route": "apply",
                    "document": _placeholder(apply, apply["preview_id"]),
                    **_case("apply", status, text, heads()),
                }
            )
            api.wait_idle(timeout=600)
            status, text = _get(api, ["sample", "runs", RUN_ID])
            cases.append({"route": "run", **_case("run after the fork", status, text, heads())})
            # The same preview again: the child exists now.
            status, text = _post(api, [*branch, "main", "intervention-apply"], json.dumps(apply))
            cases.append(
                {
                    "route": "apply",
                    "document": _placeholder(apply, apply["preview_id"]),
                    **_case("apply again", status, text, heads()),
                }
            )
            api.wait_idle(timeout=600)
            status, text = _get(api, ["sample", "runs", RUN_ID])
            cases.append(
                {"route": "run", **_case("run after a refused fork", status, text, heads())}
            )
            api.post(
                ["sample", "runs", RUN_ID, "resume"], {"branch_id": CHILD, "target_minute": MINUTES}
            )
            api.wait_idle(timeout=600)
            for minute in (FORK_MINUTE, 420, 421, MINUTES):
                status, text = _get(api, [*branch, CHILD, "timeline"], f"minute={minute}")
                cases.append(
                    {
                        "route": "timeline",
                        "minute": minute,
                        **_case(f"child at {minute}", status, text, heads()),
                    }
                )
            for query in (
                f"kind=kpis&start={FORK_MINUTE}&end={MINUTES}&limit=100",
                f"kind=kpis&end={MINUTES}&step=7",
            ):
                status, text = _get(api, [*branch, CHILD, "history"], query)
                cases.append(
                    {
                        "route": "history",
                        "query": query,
                        **_case(f"child history {query}", status, text, heads()),
                    }
                )
            status, text = _get(api, ["sample", "runs", RUN_ID])
            cases.append({"route": "run", **_case("run after the child", status, text, heads())})
            _, frame = _get(api, [*branch, "main", "timeline"], f"minute={MINUTES}")
            agent = json.loads(frame)["people"][0]["id"]
            for branch_id, question in QUESTIONS:
                document = {
                    key: agent if value == "$AGENT" else value for key, value in question.items()
                }
                status, text = _post(api, [*branch, branch_id, "questions"], json.dumps(document))
                name = f"question {branch_id} {json.dumps(question)[:80]}"
                cases.append(
                    {
                        "route": "questions",
                        "branch": branch_id,
                        "document": question,
                        **_case(name, status, text, heads()),
                    }
                )
            for document in PDF_REPORTS:
                status, text = _pdf(api, document)
                cases.append(
                    {
                        "route": "pdf-report",
                        "document": document,
                        **_case("pdf-report", status, text, heads()),
                    }
                )
            for route, query in COMPARISONS:
                status, text = _get(api, ["sample", "runs", RUN_ID, route], query)
                cases.append(
                    {
                        "route": route,
                        "query": query,
                        **_case(f"{route} {query}", status, text, heads()),
                    }
                )
            llm_cases = _llm_cases(api, heads)
            turns = api.get([*branch, "main", "conversations"])["turns"]
            llm_questions = [turn["question"] for turn in turns]
            evidence = api.files.run_file("sample", RUN_ID, "evidence.json")
            run = json.loads(api.files.run_record("sample", RUN_ID))
            text = api.files.run_scenario_bytes("sample", RUN_ID).decode()
            scenario = ScenarioSpec.from_json(text)
        finally:
            api.close()
        scenario_text = canonical(json.loads(scenario.canonical_json()))
        if digest(json.loads(scenario_text)) != run["scenario_hash"]:
            raise RuntimeError("the Scenario's canonical JSON is not the one its hash is of")
        # The world at minute 0 as a Run sets it up: the entry the core builds (#429).
        entry = json.loads(sheltersim_native.initial_entry(scenario.canonical_json().encode()))
    with_llm = {
        "evidence_sha256": hashlib.sha256(evidence).hexdigest(),
        "cases": llm_cases,
        "stored_questions": llm_questions,
    }
    return {
        "worlds": {"interventions": entry},
        "run": {
            "project_id": "sample",
            "run_id": RUN_ID,
            "seed": run["seed"],
            "minutes": MINUTES,
            "evacuees": EVACUEES,
            "fork_minute": FORK_MINUTE,
            "child": CHILD,
            "model_versions": run["model_versions"],
        },
        "scenario_json": scenario_text,
        "cases": cases,
    }, with_llm
