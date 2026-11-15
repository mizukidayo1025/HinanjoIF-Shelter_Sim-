"""Fake LLM providers and the gated live API checks."""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path
from typing import Any

import pytest


class _CountingProvider:
    def __init__(self) -> None:
        self.calls = 0

    def complete(self, **_kwargs: object) -> Any:
        from sheltersim.llm.contracts import ProviderResponse

        self.calls += 1
        return ProviderResponse(
            text='{"ok":true}', model_id="fake", input_tokens=5, output_tokens=3, elapsed_ms=0
        )


@pytest.fixture
def counting_provider() -> _CountingProvider:
    """A fake LLM provider that answers {"ok":true} and counts its calls."""
    return _CountingProvider()


_LIVE_ACK_SCHEMA = {
    "type": "object",
    "properties": {"ok": {"type": "boolean"}},
    "required": ["ok"],
    "additionalProperties": False,
}


def _live_model_check(settings: Any, cache_path: Path, provider: Any = None) -> tuple:
    import uuid
    from dataclasses import replace

    from sheltersim.llm.accounting import AccountedProvider, runtime_usage_store, usage_limits
    from sheltersim.llm.contracts import LLMStage, LLMTask
    from sheltersim.llm.provider import LLMRouter
    from sheltersim.storage.usage import UsageLedger, task_campaign_ledger

    # Opus and Sonnet always think, so each non-runtime call reserves thinking headroom
    # (about $0.09 for Opus before the call); actual spend stays far below this cap.
    settings = replace(
        settings.for_live_integration_tests(),
        max_api_calls=3,
        live_test_max_calls=3,
        live_test_budget_usd=0.15,
        budget_usd=0.15,
        intervention_model_family="sonnet",
        cache_path=cache_path,
    )
    store = runtime_usage_store()
    store.create_project("runtime-accounts", _internal=True)
    # Each run gets its own 3-call account. One fixed account kept every earlier run's
    # receipts, so the caps covered the machine's lifetime and a second run could not pass
    # (#148). The machine-wide task campaign ledger still bounds all runs together.
    ledger = UsageLedger(store, "runtime-accounts", f"live-task-qa-{uuid.uuid4().hex}")
    ledger.create(usage_limits(settings))
    router = LLMRouter(settings)
    router.provider = AccountedProvider(
        provider or router.provider,
        ledger,
        settings,
        "live_api_qa",
        branch_id=None,
        campaign_ledger=task_campaign_ledger(store),
    )
    # A runtime call must leave calls for the later analysis (ANALYSIS_RESERVED_CALLS), so
    # on a 3-call account it fits only before the other two (#148).
    tasks = [
        LLMTask(
            stage=stage,
            input_text='Reply with exactly {"ok":true}.',
            state_hash=f"live-test-{stage.value}",
            prompt_version="live-integration-ack-v1",
            output_schema=_LIVE_ACK_SCHEMA,
            fallback_text='{"ok":false}',
            max_output_tokens=32,
        )
        for stage in (LLMStage.RUNTIME, LLMStage.PREPARE, LLMStage.INTERVENTION)
    ]
    return settings, router, tasks, store


def _live_source_search_check(settings: Any, cache_path: Path, provider: Any = None) -> tuple:
    import uuid
    from dataclasses import replace

    from sheltersim.llm.accounting import AccountedProvider, runtime_usage_store, usage_limits
    from sheltersim.llm.provider import LLMRouter
    from sheltersim.region.models import RegionResearchRequest
    from sheltersim.region.research import _official_source_search_task
    from sheltersim.storage.usage import UsageLedger, task_campaign_ledger

    # Two calls of allowance: a web-search call reserves output for two sampling passes.
    settings = replace(
        settings,
        mode="record",
        max_api_calls=2,
        max_input_tokens=4096,
        max_output_tokens=256,
        max_concurrency=1,
        timeout_seconds=120,
        budget_usd=10,
        cache_path=cache_path,
    )
    store = runtime_usage_store()
    store.create_project("runtime-accounts", _internal=True)
    # Each run gets its own account, as in _live_model_check. Every run sends the same
    # request with a fresh tape, and an account refuses a request key it already holds a
    # receipt for (usage_receipt_exists), so a fixed account let only the first run pass
    # (#152). Its receipts would also make the 2-call cap cover every run on the machine.
    local = UsageLedger(
        store, "runtime-accounts", f"live-source-search-stream-qa-{uuid.uuid4().hex}"
    )
    local.create(usage_limits(settings))
    campaign = task_campaign_ledger(store)
    router = LLMRouter(settings)
    router.provider = AccountedProvider(
        provider or router.provider,
        local,
        settings,
        "live_official_source_search",
        branch_id=None,
        campaign_ledger=campaign,
    )
    task = _official_source_search_task(
        RegionResearchRequest(
            municipality_query="墨田区", selected_municipality_name="墨田区", disaster="earthquake"
        )
    )
    return settings, router, task, campaign


@pytest.fixture
def live_source_search_check() -> Callable[..., tuple]:
    """Build the live official-domain search check: settings, accounted router, task.

    Call it like live_model_check. It returns (settings, router, task, campaign ledger).
    tests/live/test_live_api.py runs it against the real API; tests/unit/llm/
    test_accounting.py checks offline that two runs each fit their own allowance.
    """
    return _live_source_search_check


@pytest.fixture
def live_model_check() -> Callable[..., tuple]:
    """Build the live model check: its capped settings, an accounted router and its tasks.

    Call it with LLMSettings, the tape path and, offline, a fake provider. It returns
    (settings, router, tasks, usage store). tests/live/test_live_api.py runs it against the
    real API; tests/unit/llm/test_accounting.py checks offline that it fits the ledgers.
    """
    return _live_model_check


def _recording_router(provider: Any, *, max_api_calls: int = 1_000) -> Any:
    from sheltersim.llm.cache import ResponseTape
    from sheltersim.llm.config import LLMMode, LLMSettings
    from sheltersim.llm.provider import LLMRouter

    settings = LLMSettings(
        mode=LLMMode.RECORD, api_key="mock-key", max_api_calls=max_api_calls, budget_usd=1.0
    )
    return LLMRouter(settings, provider=provider, tape=ResponseTape())


@pytest.fixture(scope="session")
def recording_router() -> Callable[..., Any]:
    """Make an LLMRouter that records a fake provider's answers: a mock key, a $1 budget
    and at most max_api_calls (1,000 by default) calls.
    """
    return _recording_router


# The stressed Run's runtime limits in the live feature check: one social decision and one
# support priority a day at most, so its runtime calls stay a handful.
_FEATURE_RUNTIME_LIMITS = (
    b'{"social":{"global_per_day":1,"per_agent_per_day":1},"priority":{"global_per_day":1}}'
)
# What a call that reached the Provider ends in when it fails (`LLMRouter.complete`); a call
# that succeeds is "recorded".
_PROVIDER_FAILURES = {"api_error", "rate_limited", "invalid_output"}


def _live_feature_route(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> dict[str, Any]:
    """Each LLM stage of the product on its current route, under one capped configuration.

    - prepare: `generate_scenario_draft` (the CLI's `prepare --generate-scenario`) with Opus;
    - runtime: the core's rules of a tick asking Haiku through `RouterLink` (`PlainRun.set_llm`),
      everyone under stress; the same Run again from its LLM tape (`set_llm_replay`) asks nothing
      and ends in the same world;
    - a Project Run computed with the LLM (`POST …/resume`): its review (`report`) with Opus,
      counted in the Run's usage account;
    - an intervention's text read by Sonnet (`POST …/intervention-preview`): a draft only, the
      branch unchanged until it is approved;
    - a question (`POST …/questions`) with Opus, then answered again from the Run's analysis tape
      in replay mode without a Provider.

    The Provider is whatever `LLMRouter` builds for record mode: Anthropic's in the live test,
    a fake installed in its place offline. It returns what the live test prints.
    """
    import json
    import uuid

    import sheltersim_native

    from sheltersim import native
    from sheltersim.experiments.scenario import ScenarioSpec
    from sheltersim.llm.accounting import AccountedProvider, runtime_usage_store, usage_limits
    from sheltersim.llm.config import LLMMode, LLMSettings
    from sheltersim.llm.contracts import LLMStage
    from sheltersim.llm.link import RouterLink
    from sheltersim.llm.prepare import generate_scenario_draft
    from sheltersim.llm.provider import LLMRouter
    from sheltersim.storage.projects import ProjectStore, digest
    from sheltersim.storage.usage import UsageLedger, task_campaign_ledger
    from sheltersim.web.projects import ProjectAPI
    from sheltersim.web.stored_runs import save_scenario
    from support.paths import SCENARIO_100_AGENTS

    run_tag = uuid.uuid4().hex[:12]
    # The Project API reads its settings from the environment, so the caps are set there: each
    # usage account (the preparation's here, the Project Run's) gets 12 calls and $1.
    for name, value in {
        "SHELTERSIM_LLM_MODE": "record",
        "SHELTERSIM_LLM_ERROR_POLICY": "fallback",
        "SHELTERSIM_LLM_MAX_API_CALLS": "12",
        "SHELTERSIM_LLM_MAX_INPUT_TOKENS": "16384",
        "SHELTERSIM_LLM_MAX_OUTPUT_TOKENS": "4096",
        "SHELTERSIM_LLM_MAX_CONCURRENCY": "1",
        "SHELTERSIM_LLM_TIMEOUT_SECONDS": "120",
        "SHELTERSIM_LLM_BUDGET_USD": "1.0",
        "SHELTERSIM_PREPARE_MODEL_FAMILY": "opus",
        "SHELTERSIM_INTERVENTION_MODEL_FAMILY": "sonnet",
        "SHELTERSIM_REVIEW_MODEL_FAMILY": "opus",
        "SHELTERSIM_LLM_CACHE_PATH": str(tmp_path / f"feature-{run_tag}-tape.json"),
    }.items():
        monkeypatch.setenv(name, value)
    settings = LLMSettings.from_env()
    assert settings.effective_mode is LLMMode.RECORD

    # The machine-wide task campaign ledger bounds every run of the live checks together.
    accounts = runtime_usage_store()
    accounts.create_project("runtime-accounts", _internal=True)
    ledger = UsageLedger(accounts, "runtime-accounts", f"live-feature-{run_tag}")
    ledger.create(usage_limits(settings))
    campaign = task_campaign_ledger(accounts)
    router = LLMRouter(settings)
    router.provider = AccountedProvider(
        router.provider,
        ledger,
        settings,
        "live_feature_contracts",
        branch_id=None,
        campaign_ledger=campaign,
    )

    document = json.loads(SCENARIO_100_AGENTS.read_text(encoding="utf-8"))
    document["id"] = f"live-feature-{run_tag}"
    document["duration_minutes"] = 30
    document["profiles"] = document["profiles"][:6]
    document["scheduled_events"] = []
    scenario = ScenarioSpec.model_validate(document)

    # Prepare: the draft of the profiles and events.
    generated, manifest = generate_scenario_draft(scenario, router)
    assert manifest["status"] == "generated"
    assert router.records[-1].stage == LLMStage.PREPARE.value
    assert router.records[-1].model_id == settings.opus_model
    prepare_record = router.records[-1]

    # Runtime: the core asks through the router while everyone is under stress, one of them
    # needing support whatever profiles the draft gave.
    plain = native.rules("plain")
    runtime_document = json.loads(generated.canonical_json())
    runtime_document["profiles"][0]["support_need"] = 1.0
    runtime_scenario = ScenarioSpec.model_validate(runtime_document)
    entry = json.loads(
        bytes(sheltersim_native.initial_entry(runtime_scenario.canonical_json().encode()))
    )
    for agent in entry["world"]["agents"]:
        agent["stress"] = 0.9
    stressed = json.dumps(entry, sort_keys=True, ensure_ascii=False, separators=(",", ":"))
    run = plain.PlainRun(stressed.encode())
    run.set_llm(RouterLink(router), _FEATURE_RUNTIME_LIMITS)
    before = len(router.records)
    assert run.advance(run.duration()) == runtime_scenario.duration_minutes
    runtime_records = router.records[before:]
    assert runtime_records, "the stressed Run asked no runtime question"
    tape = run.take_tape()
    assert len(tape) == run.llm_calls() == len(runtime_records)
    for record in runtime_records:
        assert (record.stage, record.model_id) == (LLMStage.RUNTIME.value, settings.haiku_model)
        assert record.api_request_count == 1
        if record.outcome != "recorded":
            # A failure stays visible as its kind and the rules' fallback; never an
            # accepted decision.
            assert record.outcome in _PROVIDER_FAILURES, record.outcome
            assert record.error_kind and record.fallback_used
    calls_before_replay = router.api_request_count
    replayed = plain.PlainRun(stressed.encode())
    replayed.set_llm_replay(tape, False, None, _FEATURE_RUNTIME_LIMITS)
    replayed.advance(replayed.duration())
    assert replayed.digests_json() == run.digests_json()
    assert router.api_request_count == calls_before_replay

    # A Project Run computed with the LLM, then its review, an intervention and a question.
    projects = ProjectStore(tmp_path / "projects")
    api = ProjectAPI(projects)
    branch = ["p", "runs", "r", "branches", "main"]
    try:
        api.post([], {"project_id": "p"})
        saved = save_scenario(
            api.files, "p", generated, digest(json.loads(generated.canonical_json()))
        )
        api.post(
            ["p", "runs"],
            {
                "run_id": "r",
                "scenario_id": saved["scenario_id"],
                "scenario_version": saved["scenario_version"],
            },
        )
        api.post(
            ["p", "runs", "r", "resume"],
            {"branch_id": "main", "target_minute": generated.duration_minutes},
        )
        api.wait_idle(timeout=600)
        assert api.get([*branch, "llm-status"])["mode"] == "record"
        run_ledger = UsageLedger.for_run(projects, "p", "r", usage_limits(settings))
        [review_receipt] = run_ledger.receipt_records({"operation": "report"})[0]
        assert (review_receipt["stage"], review_receipt["model_id"]) == (
            LLMStage.REVIEW.value,
            settings.opus_model,
        )
        report = api.get([*branch, "report"])
        # A fallback review is a finding of the check, never a pass of the paid review.
        assert report["status"] in {"validated", "rule_fallback"}, review_receipt

        run_before = api.get(["p", "runs", "r"])
        preview = api.post(
            [*branch, "intervention-preview"],
            {"minute": 10, "text": "15分後に toilet_1 の1ユニットを停止し、20分後に復旧する。"},
        )
        assert preview["usage"]["api_request_count"] >= 1
        assert preview["usage"]["model_id"] == settings.sonnet_model
        if preview["status"] == "compiled":
            assert preview["preview_id"]
        # A preview is a draft: the branch changes only through an approved apply.
        assert api.get(["p", "runs", "r"]) == run_before

        question = {"question": "トイレの待ち時間はどうでしたか？"}
        answer = api.post([*branch, "questions"], question)
        assert answer["mode"] == "record"
        assert answer["usage"]["api_request_count"] == 1
        if answer["usage"]["outcome"] != "recorded":
            assert answer["usage"]["fallback_used"] is True, answer["usage"]
        monkeypatch.setenv("SHELTERSIM_LLM_MODE", "replay")
        again = api.post([*branch, "questions"], question)
        assert again["mode"] == "replay"
        assert again["usage"]["api_request_count"] == 0
        assert (again["answer"], again["facts"]) == (answer["answer"], answer["facts"])
        monkeypatch.setenv("SHELTERSIM_LLM_MODE", "record")
        [question_receipt] = run_ledger.receipt_records({"operation": "questions"})[0]
        assert (question_receipt["stage"], question_receipt["model_id"]) == (
            LLMStage.REVIEW.value,
            settings.opus_model,
        )
        run_usage = run_ledger.summary()
    finally:
        api.close()

    return {
        "prepare": prepare_record.outcome,
        "runtime": [(record.outcome, record.error_kind) for record in runtime_records],
        "review_status": report["status"],
        "intervention": {
            "status": preview["status"],
            "fallback_used": preview["fallback_used"],
        },
        "question": {
            "outcome": answer["usage"]["outcome"],
            "accepted_fact_count": answer["usage"].get("accepted_fact_count"),
        },
        "calls": [
            {
                "stage": record.stage,
                "model": record.model_id,
                "outcome": record.outcome,
                "input_tokens": record.input_tokens,
                "output_tokens": record.output_tokens,
                "estimated_cost_usd": record.estimated_cost_usd,
            }
            for record in router.records
        ],
        "run_usage": {
            "by_operation": run_usage["by_operation"],
            "by_outcome": run_usage["by_outcome"],
            "estimated_cost_usd": run_usage["estimated_cost_usd"],
        },
        "campaign_estimated_cost_usd": campaign.summary()["estimated_cost_usd"],
    }


@pytest.fixture
def live_feature_route() -> Callable[..., dict[str, Any]]:
    """Run each LLM stage on its current route (`_live_feature_route`) with the Provider that
    record mode builds; call it with tmp_path and monkeypatch.

    tests/live/test_live_api.py runs it against the real API; tests/integration/
    test_llm_answers.py runs it offline with a fake Provider in Anthropic's place, so a route
    the live check takes that no longer exists fails in every test run (#522).
    """
    return _live_feature_route
