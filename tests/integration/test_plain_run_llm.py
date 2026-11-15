"""The LLM's rules of a tick in the core (#428), from Python.

The core asks an LLM through one port (`LlmLink::complete`, `native/core/src/llm.rs`).
`PlainRun.set_llm(RouterLink(router))` puts Python's `LLMRouter` behind the port: its call limits
and budget, its response tape, its usage observer.

- With the gate's scripted provider and everyone under stress, the core asks each kind of
  question, and the router records each call; the stored tape holds no prompt and no free text of
  the model. (That the core's questions and worlds were the Python rules' was checked against them
  until #430; the gate's `llm_tape` pins the worlds.)
- Offline (a Project's Run without an API key), the world is the Run's without an LLM.
- A Run computed again answers from its tape (`set_llm_replay`), which never asks a provider.
- The router's budget and stop policy hold through the port.
"""

from __future__ import annotations

import dataclasses
import json
import sys
from collections import Counter
from pathlib import Path
from typing import Any

import pytest

from sheltersim import native
from sheltersim.llm.cache import ResponseTape
from sheltersim.llm.config import LLMErrorPolicy, LLMMode
from sheltersim.llm.link import RouterLink
from sheltersim.llm.provider import LLMExecutionError, LLMRouter, OfflineProvider

BENCHMARKS = Path(__file__).resolve().parents[2] / "benchmarks"
if str(BENCHMARKS) not in sys.path:
    sys.path.insert(0, str(BENCHMARKS))

from gate_scenarios import (  # noqa: E402
    STRESSED_LIMITS,
    ScriptedProvider,
    _llm_settings,
    generated,
    stressed_entry,
    with_rumors,
)

PLAIN = native.rules("plain")
MINUTES = 240


class _Keeping(ScriptedProvider):
    """Keeps what each call asks, and never leaves this process."""

    def __init__(self) -> None:
        self.asked: list[tuple[str, str]] = []

    def complete(self, *, model_id: str, task: Any, **options: Any) -> Any:
        self.asked.append((task.prompt_version, task.input_text))
        return super().complete(model_id=model_id, task=task, **options)


def _record_router(provider: Any, **settings: Any) -> LLMRouter:
    settings = dataclasses.replace(_llm_settings(LLMMode.RECORD), **settings)
    return LLMRouter(settings, provider=provider, tape=ResponseTape())


def _entry() -> bytes:
    return stressed_entry(with_rumors(generated(evacuees=24, minutes=MINUTES, staff_count=1)))


def _linked(entry: bytes, link: Any) -> Any:
    run = PLAIN.PlainRun(entry)
    run.set_llm(link, STRESSED_LIMITS)
    return run


@pytest.fixture(scope="module")
def recorded() -> dict[str, Any]:
    """The stressed Run with the scripted provider: what it asked and gave."""
    entry = _entry()
    provider = _Keeping()
    router = _record_router(provider)
    seen: list[Any] = []
    router.observer = seen.append
    run = _linked(entry, RouterLink(router))
    assert run.advance(MINUTES) == MINUTES
    return {
        "entry": entry,
        "world": json.loads(run.digests_json()),
        "asked": provider.asked,
        "router": router,
        "observed": seen,
        "tape": run.take_tape(),
        "calls": run.llm_calls(),
    }


def test_the_core_asks_each_kind_of_question_and_the_router_records_each_call(
    recorded: dict[str, Any],
) -> None:
    kinds = Counter(version for version, _ in recorded["asked"])
    assert set(kinds) == {
        "support-priority-v2",
        "event-triggered-social-v2",
        "event-triggered-social-v2-complaint_response",
        "event-triggered-social-v2-cooperation",
        "event-triggered-social-v2-information_reaction",
    }
    router = recorded["router"]
    assert recorded["calls"] == len(recorded["tape"]) == len(router.records) == sum(kinds.values())
    # The router's observer (the usage ledger's hook) saw each call.
    assert recorded["observed"] == router.records


def test_the_tape_holds_each_call_without_the_prompt_or_the_models_free_text(
    recorded: dict[str, Any],
) -> None:
    for raw, record in zip(recorded["tape"], recorded["router"].records, strict=True):
        entry = json.loads(raw)
        assert set(entry) == {
            "minute",
            "task",
            "stage",
            "prompt_version",
            "schema_version",
            "input_hash",
            "state_hash",
            "text",
            "outcome",
            "fallback_used",
            "cache_hit",
            "model_id",
            "estimated_cost_usd",
            "api_request_count",
            "input_tokens",
            "output_tokens",
        }
        assert (entry["input_hash"], entry["state_hash"], entry["outcome"]) == (
            record.input_hash,
            record.state_hash,
            record.outcome,
        )
        if entry["task"] == "support_priority" and not entry["fallback_used"]:
            assert json.loads(entry["text"])["rationale"] == ""


def test_the_cores_replay_answers_from_the_stored_tape_and_never_asks(
    recorded: dict[str, Any],
) -> None:
    tape = recorded["tape"]
    run = PLAIN.PlainRun(recorded["entry"])
    run.set_llm_replay(tape, False, None, STRESSED_LIMITS)
    assert run.advance(MINUTES) == MINUTES
    assert json.loads(run.digests_json()) == recorded["world"]
    assert run.take_tape() == tape
    # Past the tape's end, a replay stops; a link given to go on with answers from there.
    short = PLAIN.PlainRun(recorded["entry"])
    short.set_llm_replay(tape[:10], False, None, STRESSED_LIMITS)
    with pytest.raises(RuntimeError, match="history cannot call a provider"):
        short.advance(MINUTES)
    provider = _Keeping()
    going_on = PLAIN.PlainRun(recorded["entry"])
    going_on.set_llm_replay(tape[:10], False, RouterLink(_record_router(provider)), STRESSED_LIMITS)
    assert going_on.advance(MINUTES) == MINUTES
    assert json.loads(going_on.digests_json()) == recorded["world"]
    assert len(provider.asked) == len(tape) - 10


def test_an_offline_run_is_the_run_without_an_llm() -> None:
    """A Project's Run without an API key: no call, and the world of the Run without a link."""
    scenario = generated(evacuees=10, minutes=360).canonical_json().encode()
    run = PLAIN.PlainRun.from_scenario(scenario)
    link = RouterLink(LLMRouter(_llm_settings(LLMMode.OFFLINE), provider=OfflineProvider()))
    assert link.offline
    run.set_llm(link)
    assert run.advance(360) == 360
    assert run.llm_calls() == 0 and run.take_tape() == []
    without = PLAIN.PlainRun.from_scenario(scenario)
    assert without.advance(360) == 360
    assert without.digests_json() == run.digests_json()
    assert without.metrics_digest() == run.metrics_digest()


def test_the_routers_budget_and_stop_policy_hold_through_the_link() -> None:
    """A router without budget falls back without asking; one that stops raises through the tick."""
    entry = _entry()
    provider = _Keeping()
    poor = _record_router(provider, budget_usd=0.0)
    run = _linked(entry, RouterLink(poor))
    assert run.advance(MINUTES) == MINUTES
    assert provider.asked == []
    tape = [json.loads(raw) for raw in run.take_tape()]
    assert tape and {row["outcome"] for row in tape} == {"budget_exceeded"}
    assert all(row["fallback_used"] for row in tape)
    stopping = _record_router(_Keeping(), budget_usd=0.0, error_policy=LLMErrorPolicy.STOP)
    run = _linked(entry, RouterLink(stopping))
    with pytest.raises(RuntimeError) as stopped:
        run.advance(MINUTES)
    assert isinstance(stopped.value.__cause__, LLMExecutionError)


def test_what_a_link_raises_stops_the_tick_as_the_cause() -> None:
    class Raising:
        offline = False

        def complete(self, task: bytes) -> bytes:
            raise ValueError("no answer here")

    entry = _entry()
    run = _linked(entry, Raising())
    with pytest.raises(RuntimeError, match="the LLM link raised an exception") as stopped:
        run.advance(MINUTES)
    assert isinstance(stopped.value.__cause__, ValueError)
    assert str(stopped.value.__cause__) == "no answer here"
    # A link without `offline` is refused when it is given.
    with pytest.raises(AttributeError):
        PLAIN.PlainRun(entry).set_llm(object())
    with pytest.raises(ValueError, match="social stress threshold must be in 0..1"):
        PLAIN.PlainRun(entry).set_llm(Raising(), b'{"social":{"stress_threshold":2}}')
