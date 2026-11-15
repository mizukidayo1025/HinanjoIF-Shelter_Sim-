"""Opt-in, durably accounted live checks for actual Claude model families."""

from __future__ import annotations

import os

import pytest

from sheltersim.llm.cache import ResponseTape
from sheltersim.llm.config import LLMSettings
from sheltersim.llm.contracts import LLMStage
from sheltersim.llm.provider import THINKING_HEADROOM_TOKENS, LLMRouter
from sheltersim.storage.usage import task_campaign_ledger

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


def test_live_scenario_intervention_runtime_and_review(
    tmp_path, monkeypatch, live_feature_route
) -> None:
    """Each LLM stage on its current route under the shared paid cap: the Scenario draft, the
    core's runtime questions and their response tape, a Project Run's review, an intervention's
    text and a question with its replay (`live_feature_route`, tests/support/llm.py).

    tests/integration/test_llm_answers.py runs the same route offline with a fake Provider.
    """
    import json

    summary = live_feature_route(tmp_path, monkeypatch)
    print("LIVE_FEATURE_QA=" + json.dumps(summary, sort_keys=True))
