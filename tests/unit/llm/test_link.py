"""`RouterLink`: Python's `LLMRouter` behind the core's port to an LLM (#428).

The core sends a task as JSON (`llm::Task`) and reads a reply as JSON (`llm::Reply`); the link makes
the `LLMTask` of the task, so that the router's cache key and a recorded call's identity are the
core's, and gives back the router's text with its record of the call.
"""

from __future__ import annotations

import json
from typing import Any

import pytest

from sheltersim.llm.cache import ResponseTape
from sheltersim.llm.config import LLMMode, LLMSettings
from sheltersim.llm.contracts import LLMTask, ProviderResponse
from sheltersim.llm.link import RouterLink, llm_task
from sheltersim.llm.provider import LLMRouter, OfflineProvider

SCHEMA = {
    "type": "object",
    "properties": {"action": {"type": "string", "enum": ["offer_reassurance", "no_interaction"]}},
    "required": ["action"],
    "additionalProperties": False,
}
TASK = {
    "task": "social_support_request",
    "stage": "runtime",
    "prompt_version": "event-triggered-social-v2",
    "schema_version": 1,
    "input_text": '{"minute":3}',
    "state_hash": "ab" * 32,
    "output_schema": SCHEMA,
    "fallback_text": '{"action":"no_interaction"}',
    "max_output_tokens": 24,
}


class _Answering:
    def __init__(self, text: str) -> None:
        self.text = text
        self.asked: list[LLMTask] = []

    def complete(self, *, model_id: str, task: LLMTask, **_options: Any) -> ProviderResponse:
        self.asked.append(task)
        return ProviderResponse(
            text=self.text, model_id=model_id, input_tokens=12, output_tokens=4, elapsed_ms=0.0
        )


def _settings(mode: LLMMode, **fields: Any) -> LLMSettings:
    return LLMSettings(
        mode=mode,
        api_key="synthetic-not-a-key" if mode is LLMMode.RECORD else None,
        max_api_calls=10,
        max_input_tokens=4096,
        max_output_tokens=256,
        budget_usd=1.0,
        **fields,
    )


def _ask(link: RouterLink, task: dict[str, Any] = TASK) -> dict[str, Any]:
    return json.loads(link.complete(json.dumps(task).encode()))


def test_the_task_is_the_routers_task_with_the_cores_identity() -> None:
    task = llm_task(TASK)
    assert (task.stage.value, task.input_text, task.state_hash) == (
        "runtime",
        TASK["input_text"],
        TASK["state_hash"],
    )
    assert (task.prompt_version, task.schema_version, task.max_output_tokens) == (
        "event-triggered-social-v2",
        1,
        24,
    )
    assert task.output_schema == SCHEMA and task.fallback_text == TASK["fallback_text"]


def test_a_recorded_answer_comes_back_with_the_routers_record() -> None:
    provider = _Answering('{"action":"offer_reassurance"}')
    router = LLMRouter(_settings(LLMMode.RECORD), provider=provider, tape=ResponseTape())
    link = RouterLink(router)
    assert not link.offline
    reply = _ask(link)
    record = router.records[-1]
    assert reply == {
        "text": '{"action":"offer_reassurance"}',
        "outcome": "recorded",
        "fallback_used": False,
        "cache_hit": False,
        "model_id": record.model_id,
        "estimated_cost_usd": record.estimated_cost_usd,
        "api_request_count": 1,
        "input_tokens": 12,
        "output_tokens": 4,
    }
    assert record.input_hash == provider.asked[0].input_hash
    # The same task again is the router's tape's answer: no second request.
    again = _ask(link)
    assert (again["outcome"], again["cache_hit"], again["api_request_count"]) == (
        "cache_hit",
        True,
        0,
    )
    assert len(provider.asked) == 1


def test_an_offline_or_refused_call_comes_back_as_the_fallback() -> None:
    link = RouterLink(LLMRouter(_settings(LLMMode.OFFLINE), provider=OfflineProvider()))
    assert link.offline
    reply = _ask(link)
    assert (reply["text"], reply["outcome"], reply["fallback_used"]) == (
        TASK["fallback_text"],
        "offline",
        True,
    )
    invalid = _Answering('{"action":"run this code"}')
    router = LLMRouter(_settings(LLMMode.RECORD), provider=invalid, tape=ResponseTape())
    reply = _ask(RouterLink(router))
    assert (reply["text"], reply["outcome"], reply["fallback_used"]) == (
        TASK["fallback_text"],
        "invalid_output",
        True,
    )


def test_a_router_that_records_nothing_is_an_error() -> None:
    class Silent(LLMRouter):
        def complete(self, task: LLMTask) -> str:
            return task.fallback_text

    with pytest.raises(RuntimeError, match="without recording the call"):
        _ask(RouterLink(Silent(_settings(LLMMode.OFFLINE), provider=OfflineProvider())))
