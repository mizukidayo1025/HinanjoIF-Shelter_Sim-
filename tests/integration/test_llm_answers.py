"""The answers made with an LLM (#463) through the Project API, with a fake Provider: a question
about a completed branch, the review of a branch the Run's LLM completes, and its llm-status.

The core builds the evidence and the task and checks the answer; the router behind the core's
port (`RouterLink`) only answers. No test calls a paid API: the Provider is a fake installed in
place of Anthropic's, and a replay answers from the Run's analysis tape without any Provider.
The answers without an API key are the browser's, fixed by the core's answer fixtures.
"""

from __future__ import annotations

import copy
import json
from typing import Any

import pytest
from support.isolation import _isolate

from sheltersim.experiments.scenario import ScenarioSpec
from sheltersim.web.projects import ProjectAPI

END = 30
QUESTION_VERSION = "read-only-results-chat-v2-grounded-explanations"
REVIEW_VERSION = "evidence-review-v1"


class FakeProvider:
    """Answers each task with `answers[prompt_version](task)`, counting the calls; a task it has
    no answer for gets text that is not JSON (the core then falls back)."""

    def __init__(self, answers: dict[str, Any]) -> None:
        self.answers = answers
        self.calls: list[str] = []

    def complete(self, *, model_id, task, max_output_tokens, timeout_seconds):
        from sheltersim.llm.contracts import ProviderResponse

        del max_output_tokens, timeout_seconds
        self.calls.append(task.prompt_version)
        answer = self.answers.get(task.prompt_version)
        text = answer(task) if answer is not None else "not json"
        return ProviderResponse(
            text=text, model_id=model_id, input_tokens=10, output_tokens=5, elapsed_ms=0.0
        )


def install(monkeypatch: pytest.MonkeyPatch, provider: Any, mode: str = "record") -> None:
    """The configured LLM mode, with `provider` in place of Anthropic's (no key is read)."""
    monkeypatch.setenv("SHELTERSIM_LLM_MODE", mode)
    monkeypatch.setattr("sheltersim.llm.provider.AnthropicProvider", lambda _key: provider)


def scalars(data: Any, prefix: str = ""):
    for key, value in data.items():
        path = f"{prefix}{key}"
        if isinstance(value, dict):
            yield from scalars(value, path + ".")
        elif isinstance(value, (str, int, float, bool)):
            yield path, value


def evidence_of(task: Any) -> list[dict[str, Any]]:
    return json.loads(task.input_text)["evidence"]


@pytest.fixture(scope="module")
def completed_api(tmp_path_factory, generated_previews, saved_project):
    """A ProjectAPI over Run r of the generated Scenario, its branch main computed to the end
    without an LLM (format v5)."""
    preview = generated_previews(duration_days=END / 1440)
    scenario = ScenarioSpec.model_validate(copy.deepcopy(preview["scenario"]))
    with pytest.MonkeyPatch.context() as monkeypatch:
        monkeypatch.setenv("SHELTERSIM_LLM_MODE", "offline")
        store = saved_project(tmp_path_factory.mktemp("answers") / "projects", scenario)
        api = ProjectAPI(store)
        try:
            api.submit("p", "r", "main", END)
            api.wait_idle(timeout=120)
            yield api
        finally:
            api.close()


def ask(api: ProjectAPI, question: str, **fields: Any) -> dict[str, Any]:
    return api.post(
        ["p", "runs", "r", "branches", "main", "questions"], {"question": question, **fields}
    )


def test_a_question_cites_recorded_values_and_a_replay_answers_it_again(completed_api, monkeypatch):
    """The answer's facts are the scalars the LLM cited, copied from the evidence (a string
    verbatim, a number as its JSON); replaying the Run's analysis tape gives the same facts
    without a Provider for the same question of the same evidence."""

    def cite(task):
        found = [
            (item["alias"], path, value)
            for item in evidence_of(task)
            for path, value in scalars(item["data"])
        ]
        text = next(item for item in found if isinstance(item[2], str))
        number = next(
            item for item in found if isinstance(item[2], (int, float)) and item[2] is not True
        )
        return json.dumps(
            {
                "claims": [
                    {"evidence": [{"alias": text[0], "field": text[1], "value": text[2]}]},
                    {
                        "evidence": [
                            {"alias": number[0], "field": number[1], "value": json.dumps(number[2])}
                        ]
                    },
                ],
                "explanations": [
                    {
                        "kind": "hypothesis",
                        "text": "待ちの長さは処理能力に関係する可能性があります。",
                        "evidence": [
                            {"alias": number[0], "field": number[1], "value": json.dumps(number[2])}
                        ],
                    }
                ],
            },
            ensure_ascii=False,
        )

    provider = FakeProvider({QUESTION_VERSION: cite})
    install(monkeypatch, provider)
    answer = ask(completed_api, "What was recorded?")
    assert provider.calls == [QUESTION_VERSION]
    assert answer["mode"] == "record"
    assert answer["availability"] == "configured"
    assert answer["usage"]["outcome"] == "recorded"
    assert answer["usage"]["accepted_fact_count"] == 2
    assert answer["usage"]["rejected_fact_batch"] is False
    assert [item["kind"] for item in answer["explanations"]] == ["hypothesis"]
    assert answer["explanations"][0]["text"].startswith("AIの解釈・仮説")
    assert all(fact["text"].startswith("記録値 · ") for fact in answer["facts"])
    assert {source["alias"] for source in answer["sources"]} == {
        row["alias"] for claim in answer["claims"] for row in claim["evidence"]
    }
    turns = completed_api.get(["p", "runs", "r", "branches", "main", "conversations"])["turns"]
    assert turns[-1]["answer"] == answer["answer"]
    assert turns[-1]["conversation_id"] == answer["context_id"]

    replaying = FakeProvider({})
    install(monkeypatch, replaying, mode="replay")
    # A new conversation: the same question of the same evidence, with no earlier turns.
    again = ask(completed_api, "What was recorded?")
    assert replaying.calls == []
    assert again["mode"] == "replay"
    assert again["facts"] == answer["facts"]
    assert again["explanations"] == answer["explanations"]
    assert again["answer"] == answer["answer"]


def test_a_question_whose_citation_does_not_match_its_evidence_cites_nothing(
    completed_api, monkeypatch
):
    """A claim whose value is not the recorded one rejects the claims, an explanation with
    digits is not shown, and the answer says the records do not support one."""

    def wrong(task):
        alias, path, value = next(
            (item["alias"], path, value)
            for item in evidence_of(task)
            for path, value in scalars(item["data"])
            if isinstance(value, (int, float)) and value is not True
        )
        proof = {"alias": alias, "field": path, "value": json.dumps(value + 1)}
        return json.dumps(
            {
                "claims": [{"evidence": [proof]}],
                "explanations": [
                    {
                        "kind": "suggestion",
                        "text": "3人増やすことを検討できます。",
                        "evidence": [dict(proof, value=json.dumps(value))],
                    }
                ],
            },
            ensure_ascii=False,
        )

    install(monkeypatch, FakeProvider({QUESTION_VERSION: wrong}))
    answer = ask(completed_api, "Which value?")
    assert answer["facts"] == [] and answer["explanations"] == []
    assert answer["usage"]["rejected_fact_batch"] is True
    assert answer["usage"]["rejected_explanation_batch"] is True
    assert answer["answer"] == "提示された記録だけでは回答を裏付けられません。"
    assert answer["availability"] == "unavailable"


def test_an_answer_that_does_not_fit_its_schema_is_the_fallback(completed_api, monkeypatch):
    """Text that is not the schema's JSON is the fallback, and the question that names one field
    is answered with that field's recorded value."""
    install(monkeypatch, FakeProvider({QUESTION_VERSION: lambda _task: '{"claims": 3}'}))
    # The question names the field by its label (the Run's id "r" is redacted from a question).
    answer = ask(completed_api, "平均ストレスは？")
    assert answer["usage"]["outcome"] in {"invalid_output", "contract_error"}
    assert answer["usage"]["fallback_used"] is True
    assert answer["usage"]["answer_fallback_kind"] == "deterministic_exact_recorded_field"
    assert answer["facts"][0]["evidence"][0]["field"] == "stress_mean"


@pytest.fixture(scope="module")
def llm_run(tmp_path_factory, generated_previews):
    """Run r of the generated Scenario computed to the end with an LLM (a fake Provider that
    answers the review with valid codes): its API and the Provider's calls."""
    from sheltersim.storage.projects import ProjectStore, digest
    from sheltersim.web.stored_runs import project_files, save_scenario, start_run

    preview = generated_previews(duration_days=END / 1440)
    scenario = ScenarioSpec.model_validate(copy.deepcopy(preview["scenario"]))

    def review(task):
        prompt = json.loads(task.input_text)
        metric = next(key for key, value in prompt["final_metrics"].items() if value == 0)
        return json.dumps(
            {
                "observed": [{"metric_id": metric, "direction": "zero"}],
                "event_observations": [],
                "assumptions": prompt["assumption_ids"][:1],
                "hypotheses": [
                    {
                        "hypothesis": "stress_pattern_needs_validation",
                        "metric_ids": [metric],
                        "event_refs": [],
                        "assumption_ids": [],
                    }
                ],
                "recommendations": [],
            }
        )

    provider = FakeProvider({REVIEW_VERSION: review})
    with pytest.MonkeyPatch.context() as monkeypatch:
        # A module's fixture comes before each test's isolation: the usage ledgers it writes
        # must not be this PC's.
        _isolate(monkeypatch, tmp_path_factory.mktemp("llm-run-state"))
        install(monkeypatch, provider)
        store = ProjectStore(tmp_path_factory.mktemp("llm-run") / "projects")
        files = project_files(store)
        with files.transaction():
            files.create_project("p")
        saved = save_scenario(files, "p", scenario, digest(json.loads(scenario.canonical_json())))
        llm = {"schema_version": 1, "mode": "record", "execution_purpose": None}
        start_run(store, files, "p", "r", saved["scenario_id"], saved["scenario_version"], llm=llm)
        api = ProjectAPI(store)
        try:
            api.submit("p", "r", "main", END)
            api.wait_idle(timeout=120)
            yield api, provider
        finally:
            api.close()


def test_the_review_of_a_branch_the_llm_completes_is_checked_against_its_evidence(llm_run):
    """The review's codes fit the evidence: the report is `validated`, its markdown made of the
    evidence's values and fixed labels."""
    api, provider = llm_run
    assert provider.calls.count(REVIEW_VERSION) == 1
    report = api.get(["p", "runs", "r", "branches", "main", "report"])
    assert report["status"] == "validated"
    assert report["review"]["status"] == "validated"
    assert report["review"]["review"]["observed"][0]["direction"] == "zero"
    assert "is zero in the final snapshot (0)." in report["markdown"]
    assert "The stress pattern needs validation against suitable data." in report["markdown"]


def test_the_llm_status_of_a_run_computed_with_an_llm_counts_its_tape(llm_run):
    """llm-status answers the Run's mode and the outcomes of the calls its stored tape holds."""
    api, provider = llm_run
    status = api.get(["p", "runs", "r", "branches", "main", "llm-status"])
    assert status["mode"] == "record"
    outcomes = status["outcomes"]
    runtime_calls = len(provider.calls) - provider.calls.count(REVIEW_VERSION)
    assert outcomes["call_count"] == runtime_calls
    assert outcomes["accepted_count"] + outcomes["fallback_count"] == runtime_calls


COMPILER_VERSION = "geometry-population-effect-compiler-v2"
REPAIR_VERSION = "h07-opus-effect-compiler-v1-repair1"


def toilets(task, **extra: Any) -> str:
    """A plan that adds two toilets to the first zone at the minute asked."""
    payload = json.loads(task.input_text)
    effect = {
        "op": "add_toilet",
        "target_kind": "zone",
        "target_id": payload["target_ids_by_kind"]["zone"][0],
        "start_minute": payload["branch"]["selected_minute"],
        "parameters": {"count": 2},
        **extra,
    }
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


def preview(api: ProjectAPI, text: str) -> dict[str, Any]:
    return api.post(
        ["p", "runs", "r", "branches", "main", "intervention-preview"],
        {"minute": 10, "text": text},
    )


def test_an_intervention_text_is_read_by_the_llm_into_a_checked_program(completed_api, monkeypatch):
    """The compiler's task offers the operators the core runs; the plan the LLM gives is read by
    the core's contract, checked on the branch's world and kept as a draft to approve."""
    provider = FakeProvider({COMPILER_VERSION: toilets})
    install(monkeypatch, provider)
    answer = preview(completed_api, "トイレが足りないので増やしたい")
    assert provider.calls == [COMPILER_VERSION]
    assert answer["status"] == "compiled"
    assert answer["fallback_used"] is False
    assert answer["can_apply"] is True
    assert answer["program"]["compiler_provenance"]["compiler_kind"] == "llm"
    [effect] = answer["program"]["effects"]
    assert (effect["op"], effect["parameters"], effect["source_id"]) == (
        "add_toilet",
        {"count": 2},
        "opus-effect-compiler-v1",
    )
    assert answer["usage"]["api_request_count"] == 1
    assert answer["preview_id"]


def test_a_plan_the_checks_refuse_is_repaired_once(completed_api, monkeypatch):
    """A one-shot operation given an end minute is refused; the errors go back in one repair
    task, whose plan is used."""
    provider = FakeProvider(
        {
            COMPILER_VERSION: lambda task: toilets(task, end_minute=20),
            REPAIR_VERSION: lambda task: toilets(task),
        }
    )
    install(monkeypatch, provider)
    answer = preview(completed_api, "仮設トイレを置いてほしい")
    assert provider.calls == [COMPILER_VERSION, REPAIR_VERSION]
    assert answer["status"] == "compiled"
    assert answer["usage"]["api_request_count"] == 2


def test_without_an_accepted_plan_the_rules_read_the_text_or_a_stop_policy_refuses(
    completed_api, monkeypatch
):
    """An answer that is not JSON falls back to the offline rules, saying why; under a stop
    policy the preview is refused instead."""
    install(monkeypatch, FakeProvider({COMPILER_VERSION: lambda _task: "not json"}))
    answer = preview(completed_api, "空に虹をかけたい")
    assert answer["fallback_used"] is True
    assert answer["status"] == "not_simulatable"
    assert answer["usage"]["error_kind"] is not None
    assert any("理由コード" in item for item in answer["limitations"]), answer["limitations"]

    from sheltersim.web.projects import answers

    monkeypatch.setenv("SHELTERSIM_LLM_ERROR_POLICY", "stop")
    with pytest.raises(answers().Refused) as refused:
        preview(completed_api, "空に虹をかけたい")
    assert refused.value.args[0] == 409
    assert refused.value.args[2] == "intervention_compiler_unavailable"
