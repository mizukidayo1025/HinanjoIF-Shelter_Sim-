"""The PDF report with an API key (#463), through the Project API, with a fake Opus: the core
builds the report's data and facts from the branches' stored Runs and checks the analysis; the
server typesets the PDF and keeps it. No test calls a paid API.

Without an API key the route answers the core's refusal (409 `pdf_analysis_offline`), the
browser's, which the core's answer fixtures fix.

The report's data and the analysis task's cache key (its facts, versions and schema) are pinned:
a change that keeps the answers keeps them (#489).
"""

from __future__ import annotations

import copy
import hashlib
import json
from typing import Any

import pytest
from support.isolation import _isolate

from sheltersim.experiments.scenario import ScenarioSpec
from sheltersim.storage.projects import digest
from sheltersim.web.projects import ProjectAPI, answers

END = 30
FORK_MINUTE = 10
# The sha256 of the report's data as canonical JSON, and the cache key of the analysis task.
DATA_SHA256 = "14740a875a4615afb0d21ab0d799c55c39b4797542b7f6fbb6d3b048393db6b5"
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


def export(api: ProjectAPI, branch_ids: list[str], minute: int = END) -> bytes:
    body = json.dumps({"branch_ids": branch_ids, "minute": minute}).encode()
    return api.answer_post(["p", "runs", "r", "pdf-report"], body)


def kept(api: ProjectAPI) -> list[dict[str, Any]]:
    folder = api.store.root / "p" / "runs" / "r" / "pdf-reports"
    return [json.loads(path.read_text()) for path in sorted(folder.glob("*.json"))]


def test_a_report_of_two_branches_is_typeset_from_the_core_s_facts_and_kept(
    report_api, monkeypatch, report_provider
):
    """The analysis Opus wrote cites the core's facts; the PDF is kept, and the same report is
    answered again without asking."""
    provider = report_provider()
    install(monkeypatch, provider)
    pdf = export(report_api, ["main", "capacity"])
    assert pdf.startswith(b"%PDF-")
    assert provider.calls == 1
    [artifact] = [item for item in kept(report_api) if len(item["data"]["branches"]) == 2]
    data, facts = artifact["data"], artifact["facts"]
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
    assert artifact["analysis"]["model_id"].startswith("claude-opus-")
    assert export(report_api, ["main", "capacity"]) == pdf
    assert provider.calls == 1


def test_an_analysis_the_checks_refuse_makes_no_pdf(
    report_api, monkeypatch, report_provider, prose_document
):
    """A paragraph with a quantity is refused (409 `pdf_analysis_invalid`): no PDF is made, and
    the next try is a new request."""

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
