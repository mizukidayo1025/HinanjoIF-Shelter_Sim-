"""The report (#463), through the Project API: the core builds the report's data and facts from
the branches' stored Runs; the server answers them as JSON (the screen's print page lays them out,
#470). Without the analysis (#582) no LLM is asked and nothing is kept. With it, a fake Opus
writes the analysis, which the core checks, and the server keeps the answer. No test calls a paid
API.

An analysis asked without an API key is the core's refusal (409 `pdf_analysis_offline`), the
browser's, which the core's answer fixtures fix.

The report's data and the analysis task's cache key (its facts, versions and schema) are pinned:
a change that keeps the answers keeps them (#489).

The browser asks the same analysis through the site's relay (#470): the relay sends Anthropic what
the router's provider sends for it.
"""

from __future__ import annotations

import base64
import copy
import hashlib
import json
from typing import Any

import pytest
from support.isolation import _isolate
from support.llm_relay import relay_requests, router_request

from sheltersim.experiments.scenario import ScenarioSpec
from sheltersim.storage.projects import digest
from sheltersim.web.projects import ProjectAPI, answers

END = 30
FORK_MINUTE = 10
# The sha256 of the report's data as canonical JSON, and the cache key of the analysis task.
DATA_SHA256 = "e54238a34ce4a0213f04743db09b755f9f77fc33b91e049c9e647b20cbc26598"
ANALYSIS_KEY = "e7e292de94d0a4d4d414b5c12d58efbe1d7d4ece4ef5af304145f0847a0a2ade"


@pytest.fixture(scope="module")
def report_api(tmp_path_factory, generated_previews, add_toilets_program, saved_project):
    """A ProjectAPI over Run r of the generated Scenario computed to its end without an LLM, with
    a branch `capacity` that adds toilets at minute 10."""
    preview = generated_previews(duration_days=END / 1440)
    scenario = ScenarioSpec.model_validate(copy.deepcopy(preview["scenario"]))
    with pytest.MonkeyPatch.context() as monkeypatch:
        _isolate(monkeypatch, tmp_path_factory.mktemp("pdf-state"))
        monkeypatch.setenv("SHELTERSIM_LLM_MODE", "offline")
        store = saved_project(tmp_path_factory.mktemp("pdf") / "projects", scenario)
        api = ProjectAPI(store)
        try:
            api.submit("p", "r", "main", END)
            api.wait_idle(timeout=120)
            program = add_toilets_program(2, start_minute=FORK_MINUTE)
            text = json.dumps(program, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
            api.fork("p", "r", "main", "capacity", FORK_MINUTE, text, digest(program))
            api.submit("p", "r", "capacity", END)
            api.wait_idle(timeout=120)
            yield api
        finally:
            api.close()


def install(monkeypatch: pytest.MonkeyPatch, provider: Any) -> None:
    monkeypatch.setenv("SHELTERSIM_LLM_MODE", "record")
    monkeypatch.setattr("sheltersim.llm.provider.AnthropicProvider", lambda _key: provider)


def export(
    api: ProjectAPI, branch_ids: list[str], minute: int = END, *, analysis: bool = True
) -> bytes:
    body = json.dumps({"branch_ids": branch_ids, "minute": minute, "analysis": analysis}).encode()
    return api.answer_post(["p", "runs", "r", "pdf-report"], body)


def report_of(answer: bytes) -> dict[str, Any]:
    """The route's answer read: the core's data, facts and checked analysis, nothing else."""
    report = json.loads(answer)
    assert set(report) == {"analysis", "data", "facts"}
    return report


def kept(api: ProjectAPI) -> list[dict[str, Any]]:
    """The kept reports, each with its answer decoded (`report`)."""
    folder = api.store.root / "p" / "runs" / "r" / "pdf-reports"
    reports = []
    for path in sorted(folder.glob("*.json")):
        item = json.loads(path.read_text())
        if "answer_base64" in item:
            item["report"] = json.loads(base64.b64decode(item["answer_base64"]))
        reports.append({**item, "path": path})
    return reports


def test_a_report_of_two_branches_is_answered_from_the_core_s_facts_and_kept(
    report_api, monkeypatch, report_provider
):
    """The analysis Opus wrote cites the core's facts; the answer is kept, and the same report is
    answered again without asking."""
    provider = report_provider()
    install(monkeypatch, provider)
    answer = export(report_api, ["main", "capacity"])
    report = report_of(answer)
    assert provider.calls == 1
    [artifact] = [item for item in kept(report_api) if len(item["report"]["data"]["branches"]) == 2]
    assert artifact["schema_version"] == 2 and base64.b64decode(artifact["answer_base64"]) == answer
    assert artifact["identity"]["heads"] == report["data"]["heads"]
    data, facts = report["data"], report["facts"]
    canonical = json.dumps(data, sort_keys=True, ensure_ascii=False, separators=(",", ":"))
    assert hashlib.sha256(canonical.encode()).hexdigest() == DATA_SHA256
    assert provider.keys == [ANALYSIS_KEY]
    assert data["range"] == [FORK_MINUTE, END]
    assert [branch["branch_id"] for branch in data["branches"]] == ["main", "capacity"]
    assert data["paired"] is True
    assert {"S", "L", "B0.toilet_queue_length", "B1.stress_mean", "D1.stress_mean"} <= set(facts)
    capacity = data["branches"][1]
    assert any(item["operator"] == "add_toilet" for item in capacity["interventions"])
    assert facts["B1.interventions"]["recorded_commands"] == 1
    assert report["analysis"]["model_id"].startswith("claude-opus-")
    assert report["analysis"]["sections"] == provider.document()
    assert export(report_api, ["main", "capacity"]) == answer
    assert provider.calls == 1


def test_a_report_without_the_analysis_asks_no_llm_and_is_not_kept(
    report_api, monkeypatch, report_provider
):
    """#582: without the analysis, the answer is the core's data and facts with `analysis: null`,
    with an API key or without one; no LLM is asked, nothing is kept, and a kept report with the
    analysis is not answered for it."""
    provider = report_provider()
    install(monkeypatch, provider)
    with_analysis = report_of(export(report_api, ["main", "capacity"]))
    calls, before = provider.calls, kept(report_api)
    answer = export(report_api, ["main", "capacity"], analysis=False)
    report = report_of(answer)
    assert provider.calls == calls and kept(report_api) == before
    assert report["analysis"] is None
    assert (report["data"], report["facts"]) == (with_analysis["data"], with_analysis["facts"])
    monkeypatch.setenv("SHELTERSIM_LLM_MODE", "offline")
    assert export(report_api, ["main", "capacity"], analysis=False) == answer
    with pytest.raises(answers().Refused) as refused:
        export(report_api, ["main", "capacity"])
    assert (refused.value.args[0], refused.value.args[2]) == (409, "pdf_analysis_offline")
    assert provider.calls == calls


def test_a_report_kept_in_another_form_is_asked_again(report_api, monkeypatch, report_provider):
    """A report kept before #470 (a typeset PDF, form 1) is not answered: the report is made
    again, the Run's tape answering the same analysis without a call, and kept in form 2."""
    provider = report_provider()
    install(monkeypatch, provider)
    first = report_of(export(report_api, ["main"], 15))
    [saved] = [item for item in kept(report_api) if item["report"]["data"]["range"] == [0, 15]]
    path = saved["path"]
    path.write_text(
        json.dumps({"schema_version": 1, "identity": saved["identity"], "pdf_base64": "JVBERi0="})
    )
    again = report_of(export(report_api, ["main"], 15))
    assert provider.calls == 1
    assert again["analysis"]["record"]["cache_hit"] is True
    assert {**again, "analysis": {**again["analysis"], "record": None}} == {
        **first,
        "analysis": {**first["analysis"], "record": None},
    }
    assert json.loads(path.read_text())["schema_version"] == 2


def test_an_analysis_the_checks_refuse_makes_no_report(
    report_api, monkeypatch, report_provider, prose_document
):
    """A paragraph with a quantity is refused (409 `pdf_analysis_invalid`): no report is made,
    and the next try is a new request."""

    def counted() -> dict[str, Any]:
        document = prose_document()
        document["executive"][0]["text"] = "待機が3人を超えた可能性があります。"
        return document

    provider = report_provider(counted)
    install(monkeypatch, provider)
    with pytest.raises(answers().Refused) as refused:
        export(report_api, ["main"], 20)
    assert (refused.value.args[0], refused.value.args[2]) == (409, "pdf_analysis_invalid")
    assert provider.calls == 1
    attempts = report_api.store.root / "p" / "runs" / "r" / "pdf-attempts"
    assert [json.loads(path.read_text())["next_attempt"] for path in attempts.glob("*.json")] == [1]


def test_the_browser_s_relay_sends_the_analysis_task_as_the_router_does(
    report_api, monkeypatch, report_provider
):
    """#470: the analysis task the core asks, sent to the relay as the browser sends it, is the
    request the router's provider makes: Opus, the answer's size, the system prompt, the facts."""
    from sheltersim.llm.link import RouterLink

    tasks: list[bytes] = []
    complete = RouterLink.complete

    def keep(self: RouterLink, task: bytes) -> bytes:
        tasks.append(task)
        return complete(self, task)

    requests: list[dict[str, Any]] = []
    answering = report_provider()

    class Recording:
        def complete(self, *, model_id, task, max_output_tokens, timeout_seconds):
            requests.append(
                router_request(model_id=model_id, task=task, max_output_tokens=max_output_tokens)
            )
            return answering.complete(model_id=model_id, task=task)

    monkeypatch.setattr(RouterLink, "complete", keep)
    install(monkeypatch, Recording())
    # A scope the other tests do not export: no kept report answers it.
    report_of(export(report_api, ["capacity"], minute=25))
    assert [json.loads(task)["task"] for task in tasks] == ["pdf_analysis"]
    [sent] = relay_requests(tasks)
    [request] = requests
    assert {key: sent[key] for key in request} == request
    assert request["model"].startswith("claude-opus-")
