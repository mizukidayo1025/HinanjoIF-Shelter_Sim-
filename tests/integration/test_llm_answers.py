"""The answers made with an LLM (#463) through the Project API, with a fake Provider: a question
about a completed branch, the review of a branch the Run's LLM completes, and its llm-status.

The core builds the evidence and the task and checks the answer; the router behind the core's
port (`RouterLink`) only answers. No test calls a paid API: the Provider is a fake installed in
place of Anthropic's, and a replay answers from the Run's analysis tape without any Provider.
The answers without an API key are the browser's, fixed by the core's answer fixtures.

Each kind of task the core asks has its cache key pinned (`LLMTask.cache_key`: the input, the
state hash, the versions and the schema). A recorded call answers only a task of the same key, so
a change that keeps the answers keeps these keys (#489).

The browser asks the same tasks through the site's relay (#470): the relay sends Anthropic what
the router's provider sends for them.
"""

from __future__ import annotations

import copy
import json
from typing import Any

import pytest
from support.isolation import _isolate
from support.llm_relay import relay_requests, router_request

from sheltersim.experiments.scenario import ScenarioSpec
from sheltersim.web.projects import ProjectAPI

END = 30
QUESTION_VERSION = "read-only-results-chat-v2-grounded-explanations"
REVIEW_VERSION = "evidence-review-v1"
QUESTION_KEY = "35085c37876657596793b896eef2f4fd336efb364899b681707aec4f4434c348"
REVIEW_KEY = "ad5f34b6442a3def1f881135e9d87d98ea426a23c5dbdc573d0980e07e325146"
COMPILER_KEY = "ee3939e1a4e39805baf9e106414704bd2344ee3e6b6770bec19f106a7f7974f3"
REPAIR_KEY = "631ff3fd30d1dd3376d534beec770079cff1bfe1300a5d1c5efb1469741552b7"


class FakeProvider:
    """Answers each task with `answers[prompt_version](task)`, counting the calls and keeping
    their cache keys; a task it has no answer for gets `otherwise(task)`, by default text that is
    not JSON (the core then falls back)."""

    def __init__(self, answers: dict[str, Any], otherwise: Any = None) -> None:
        self.answers = answers
        self.otherwise = otherwise or (lambda _task: "not json")
        self.calls: list[str] = []
        self.keys: list[str] = []

    def complete(self, *, model_id, task, max_output_tokens, timeout_seconds):
        from sheltersim.llm.contracts import ProviderResponse

        del max_output_tokens, timeout_seconds
        self.calls.append(task.prompt_version)
        self.keys.append(task.cache_key(model_id))
        answer = self.answers.get(task.prompt_version)
        text = (answer or self.otherwise)(task)
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


def cite(task):
    """A question's answer citing a recorded string and a recorded number of its evidence."""
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


@pytest.fixture(scope="module")
def completed_api(tmp_path_factory, generated_previews, saved_project):
    """A ProjectAPI over Run r of the generated Scenario, its branch main computed to the end
    without an LLM (format v6)."""
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
    provider = FakeProvider({QUESTION_VERSION: cite})
    install(monkeypatch, provider)
    answer = ask(completed_api, "What was recorded?")
    assert provider.calls == [QUESTION_VERSION]
    assert provider.keys == [QUESTION_KEY]
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


def review(task):
    """A review whose codes fit the evidence: a metric that is zero at the end, observed so."""
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


@pytest.fixture(scope="module")
def llm_run(tmp_path_factory, generated_previews):
    """Run r of the generated Scenario computed to the end with an LLM (a fake Provider that
    answers the review with valid codes): its API and the Provider's calls."""
    from sheltersim.storage.projects import ProjectStore, digest
    from sheltersim.web.stored_runs import project_files, save_scenario, start_run

    preview = generated_previews(duration_days=END / 1440)
    scenario = ScenarioSpec.model_validate(copy.deepcopy(preview["scenario"]))
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
    assert provider.keys[provider.calls.index(REVIEW_VERSION)] == REVIEW_KEY
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
    assert provider.keys == [COMPILER_KEY, REPAIR_KEY]
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


def test_the_live_feature_check_takes_routes_that_exist(live_feature_route, tmp_path, monkeypatch):
    """The paid check of each LLM stage (tests/live/test_live_api.py) runs offline with this fake
    in Anthropic's place, and nothing leaves the process: a route it takes that is gone fails
    here, not first when the paid gate is opened (#522). A task without a scripted answer gets
    its own fallback text, which fits its schema."""
    import socket

    def refuse(*_args: Any, **_kwargs: Any) -> None:
        raise AssertionError("the offline feature check opened a network connection")

    monkeypatch.setattr(socket.socket, "connect", refuse)
    monkeypatch.setattr(socket.socket, "connect_ex", refuse)
    monkeypatch.setattr(socket, "create_connection", refuse)
    provider = FakeProvider(
        {REVIEW_VERSION: review, COMPILER_VERSION: toilets, QUESTION_VERSION: cite},
        otherwise=lambda task: task.fallback_text,
    )
    install(monkeypatch, provider)

    summary = live_feature_route(tmp_path, monkeypatch)

    assert summary["review_status"] == "validated"
    assert summary["intervention"] == {"status": "compiled", "fallback_used": False}
    assert summary["question"] == {"outcome": "recorded", "accepted_fact_count": 2}
    assert {call["outcome"] for call in summary["calls"]} == {"recorded"}
    assert provider.calls.count(REVIEW_VERSION) == 1
    assert provider.calls.count(COMPILER_VERSION) == 1
    assert provider.calls.count(QUESTION_VERSION) == 1


def test_the_browser_s_relay_sends_the_core_s_tasks_as_the_router_does(completed_api, monkeypatch):
    """#470: the tasks the core asks (a question, an intervention's text and its repair), sent
    to the relay as the browser sends them, are the requests the router's provider makes: the
    model, the answer's size, the system prompt and the input."""
    from sheltersim.llm.link import RouterLink

    tasks: list[bytes] = []
    complete = RouterLink.complete

    def keep(self: RouterLink, task: bytes) -> bytes:
        tasks.append(task)
        return complete(self, task)

    requests: list[dict[str, Any]] = []

    class Recording(FakeProvider):
        def complete(self, *, model_id, task, max_output_tokens, timeout_seconds):
            requests.append(
                router_request(model_id=model_id, task=task, max_output_tokens=max_output_tokens)
            )
            return super().complete(
                model_id=model_id,
                task=task,
                max_output_tokens=max_output_tokens,
                timeout_seconds=timeout_seconds,
            )

    monkeypatch.setattr(RouterLink, "complete", keep)
    install(
        monkeypatch,
        Recording(
            {
                QUESTION_VERSION: cite,
                COMPILER_VERSION: lambda task: toilets(task, end_minute=20),
                REPAIR_VERSION: toilets,
            }
        ),
    )
    # Questions and texts the other tests do not ask: the Run's tapes answer those.
    ask(completed_api, "Which values did the run record?")
    preview(completed_api, "仮設トイレを2基置いてほしい")
    assert [json.loads(task)["task"] for task in tasks] == [
        "result_question",
        "intervention_compiler",
        "intervention_repair",
    ]
    sent = relay_requests(tasks)
    assert len(sent) == len(requests) == 3
    for body, request in zip(sent, requests, strict=True):
        assert {key: body[key] for key in request} == request
