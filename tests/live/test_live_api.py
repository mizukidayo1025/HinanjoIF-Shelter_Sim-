"""Opt-in, durably accounted live checks for actual Claude model families."""

from __future__ import annotations

import os

import pytest
from support.paths import SCENARIO_100_AGENTS

from sheltersim.llm.accounting import AccountedProvider, runtime_usage_store, usage_limits
from sheltersim.llm.cache import ResponseTape
from sheltersim.llm.config import LLMSettings
from sheltersim.llm.contracts import LLMStage
from sheltersim.llm.provider import THINKING_HEADROOM_TOKENS, LLMRouter
from sheltersim.storage.usage import UsageLedger, task_campaign_ledger

pytestmark = [
    pytest.mark.live_api,
    pytest.mark.skipif(
        os.getenv("SHELTERSIM_ALLOW_LIVE_API_TESTS") != "1",
        reason="set SHELTERSIM_ALLOW_LIVE_API_TESTS=1 for the capped live API command",
    ),
    pytest.mark.skipif(
        not os.getenv("ANTHROPIC_API_KEY"),
        reason="ANTHROPIC_API_KEY is not configured",
    ),
]


def test_live_opus_sonnet_and_haiku_calls_are_durably_capped_and_replayable(
    tmp_path, live_model_check
) -> None:
    settings, router, tasks, store = live_model_check(
        LLMSettings.from_env(), tmp_path / "live-api-tape.json"
    )

    outputs = [router.complete(task) for task in tasks]

    assert [output for output in outputs if '"ok":true' in output] == outputs
    assert router.api_request_count == 3
    assert [record.model_id for record in router.records] == [
        settings.haiku_model,
        settings.opus_model,
        settings.sonnet_model,
    ]
    assert all(record.outcome == "recorded" for record in router.records)
    # Billed output includes thinking outside runtime; runtime Haiku runs without it.
    answer_cap = 32
    for record in router.records:
        limit = (
            answer_cap
            if record.stage == LLMStage.RUNTIME.value
            else (answer_cap + THINKING_HEADROOM_TOKENS)
        )
        assert record.output_tokens <= limit
    assert router.spent_usd <= settings.budget_usd
    campaign_summary = task_campaign_ledger(store).summary()
    assert campaign_summary["estimated_cost_usd"] <= campaign_summary["limits"]["budget_usd"]

    replay = LLMRouter(
        settings,
        tape=ResponseTape(path=settings.cache_path),
    )
    assert [replay.complete(task) for task in tasks] == outputs
    assert replay.api_request_count == 0
    assert all(record.cache_hit for record in replay.records)


def test_live_official_domain_search_returns_only_verifiable_source_candidates(
    tmp_path, live_source_search_check
) -> None:
    settings, router, task, campaign = live_source_search_check(
        LLMSettings.from_env(), tmp_path / "source-search-tape.json"
    )

    output = router.complete(task)
    assert router.records[-1].outcome == "recorded"
    assert router.records[-1].web_search_requests <= 1
    response = router.tape.get(task.cache_key(settings.model_for(task.stage.value)))
    assert response is not None
    assert response.citations
    candidates = [
        {
            "title": item.title,
            "url": item.url,
            "cited_text": item.cited_text,
            "accessed_at": "2026-10-23T00:00:00+00:00",
        }
        for item in response.citations
    ]
    from sheltersim.region.models import LLMSearchSourceCandidate

    verified_candidates = [LLMSearchSourceCandidate.model_validate(item) for item in candidates]
    assert all(
        item.status == "candidate_not_independently_verified" for item in verified_candidates
    )
    assert output
    assert campaign.summary()["totals"]["calls"] >= 1
    assert campaign.summary()["estimated_cost_usd"] < 50


def test_live_scenario_intervention_runtime_and_review(tmp_path, monkeypatch) -> None:
    """Exercise real feature schemas and saved evidence under the shared paid cap.

    The question about the saved Run's results went with the older screens' saved-run route
    (#376); questions are asked of a Project's Branch (`POST …/questions`).
    """
    import json
    import uuid
    from dataclasses import replace

    from sheltersim.core.engine import TraceLevel
    from sheltersim.core.world import WorldState
    from sheltersim.events.handlers import EventDispatcher, SocialInteractionHandler
    from sheltersim.experiments.runner import run_scenario
    from sheltersim.experiments.scenario import ScenarioSpec
    from sheltersim.llm.natural_language import (
        compile_natural_language,
        scenario_summary_from_world,
    )
    from sheltersim.llm.prepare import generate_scenario_draft
    from sheltersim.llm.runtime import EventTriggeredSocialPolicy
    from sheltersim.llm.schemas import ParseRequest
    from sheltersim.scenario_authoring.configs.staff import StaffService

    run_tag = uuid.uuid4().hex[:12]
    cache_path = tmp_path / f"feature-{run_tag}-qna-tape.json"
    monkeypatch.setenv("SHELTERSIM_LLM_CACHE_PATH", str(cache_path))
    settings = replace(
        LLMSettings.from_env(),
        mode="record",
        max_api_calls=12,
        max_input_tokens=16_384,
        max_output_tokens=4096,
        max_concurrency=1,
        timeout_seconds=120,
        budget_usd=2.0,
        prepare_model_family="opus",
        intervention_model_family="sonnet",
        review_model_family="opus",
        cache_path=tmp_path / f"feature-{run_tag}-provider-tape.json",
    )
    store = runtime_usage_store()
    store.create_project("runtime-accounts", _internal=True)
    ledger = UsageLedger(store, "runtime-accounts", f"live-feature-{run_tag}")
    ledger.create(usage_limits(settings))
    campaign = task_campaign_ledger(store)
    router = LLMRouter(settings)
    router.provider = AccountedProvider(
        router.provider,
        ledger,
        settings,
        "live_feature_contracts",
        branch_id=None,
        campaign_ledger=campaign,
    )

    fixture = SCENARIO_100_AGENTS
    document = json.loads(fixture.read_text(encoding="utf-8"))
    document["id"] = f"live-feature-{run_tag}"
    document["duration_minutes"] = 30
    document["profiles"] = document["profiles"][:2]
    document["scheduled_events"] = []
    scenario = ScenarioSpec.model_validate(document)

    generated, manifest = generate_scenario_draft(scenario, router)
    assert manifest["status"] == "generated"
    assert router.records[-1].stage == LLMStage.PREPARE.value
    assert router.records[-1].model_id == settings.opus_model

    compiler_world = WorldState.from_scenario(generated)
    request = ParseRequest(
        text="開始から5分後に toilet_1 の1ユニットを停止し、10分後に復旧する。",
        branch_id="main",
        selected_minute=0,
        scenario_summary=scenario_summary_from_world(compiler_world),
    )
    compiled = compile_natural_language(request, router)
    assert compiled.status == "compiled"
    assert compiled.draft_plan is not None
    assert compiled.draft_plan.requires_confirmation is True
    assert compiled.usage.api_request_count >= 1
    assert compiled.usage.model_id == settings.sonnet_model

    runtime_document = json.loads(generated.canonical_json())
    runtime_document["profiles"][0]["support_need"] = 1.0
    runtime_scenario = ScenarioSpec.model_validate(runtime_document)
    runtime_world = WorldState.from_scenario(runtime_scenario)
    runtime_world.minute_index = 10
    resident = runtime_world.agents["A001"]
    resident.stress = 0.9
    resident.trust = 0.1
    support = StaffService(runtime_world).request("A001", minute=10)
    decisions = []
    policy = EventTriggeredSocialPolicy(router, record_decision=decisions.append)
    decision = policy.consider_support_request(
        runtime_world, agent_id="A001", request_id=support.request_id
    )
    assert decision.api_request_count == 1
    assert decision.model_id == settings.haiku_model
    runtime_record = router.records[-1]
    assert runtime_record.api_request_count == 1
    if decision.fallback_used:
        # A provider failure must stay visible and must not turn into a fabricated
        # APPLIED social event. Continue the end-to-end path to verify safe fallback,
        # persisted run evidence, paid review and saved-results Q&A independently.
        assert runtime_record.outcome in {"api_error", "rate_limited", "invalid_output"}
        assert runtime_record.error_kind
        assert decision.event_id is None
    else:
        assert runtime_record.outcome == "recorded"
    if decision.event_id is not None:
        runtime_world.minute_index = 11
        EventDispatcher({"social_interaction": SocialInteractionHandler()}).dispatch_due(
            runtime_world, 11
        )
        assert runtime_world.event_log.records[-1].status.value == "applied"
    assert decisions[-1]["fallback_used"] is decision.fallback_used

    output = tmp_path / "runs" / f"live-{run_tag}"
    completed = run_scenario(generated, output, trace_level=TraceLevel.OFF, llm_router=router)
    assert completed.minute_index == generated.duration_minutes
    review = json.loads((output / "review.json").read_text(encoding="utf-8"))
    assert router.records[-1].stage == LLMStage.REVIEW.value
    assert router.records[-1].model_id == settings.opus_model
    assert review["status"] in {"validated", "rule_fallback"}, router.records[-1].error_kind
    if review["status"] == "validated":
        assert router.records[-1].outcome == "recorded"
    else:
        # Preserve the failure category; a fallback review is a QA finding, never a
        # paid-review pass.
        assert router.records[-1].fallback_used is True, router.records[-1].error_kind

    print(
        "LIVE_FEATURE_QA="
        + json.dumps(
            {
                "runtime": {
                    "outcome": runtime_record.outcome,
                    "error_kind": runtime_record.error_kind,
                    "fallback_used": decision.fallback_used,
                    "event_applied": bool(decision.event_id),
                },
                "review_status": review["status"],
                "calls": [
                    {
                        "stage": record.stage,
                        "model": record.model_id,
                        "outcome": record.outcome,
                        "error_kind": record.error_kind,
                        "calls": record.api_request_count,
                        "input_tokens": record.input_tokens,
                        "output_tokens": record.output_tokens,
                        "estimated_cost_usd": record.estimated_cost_usd,
                    }
                    for record in router.records
                ],
                "campaign_estimated_cost_usd": campaign.summary()["estimated_cost_usd"],
            },
            sort_keys=True,
        )
    )
