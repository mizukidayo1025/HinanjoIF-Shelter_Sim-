"""The LLM's rules of a tick in the core (#428), from Python, against the Python rules.

The core asks an LLM through one port (`LlmLink::complete`, `native/core/src/llm.rs`); its rules
(`rules/llm_runtime.rs`) are Python's `_prioritize_support_request` and `EventTriggeredSocialPolicy`
moved. `PlainRun.set_llm(RouterLink(router))` puts Python's `LLMRouter` behind the port: its call
limits and budget, its response tape, its usage observer, or a `RecordedRouter`'s recorded order.

- With the gate's scripted provider and everyone under stress, the core asks what the Python rules
  of the native path ask, byte for byte, and its world after the ticks is the native path's.
- Offline (a Project's Run without an API key), the world is the Run's without an LLM, and the
  decision records are the Project route's rows, the social policy's `dialogue` rows among them.
- A Run computed again answers from its tape: Python's `RecordedRouter` through the link, or the
  core's own replay (`set_llm_replay`), which never asks a provider. A call the tape does not hold
  stops the tick, with the router's error as the cause.
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
from support.core_tick import GENERATED_TICKS, generated_run, world_digests

from sheltersim import native
from sheltersim.core.engine import TickEngine
from sheltersim.core.world import WorldState
from sheltersim.core.world_export import export_world
from sheltersim.experiments.metrics import MetricsRecorder
from sheltersim.experiments.runner import OfflineSimulationController
from sheltersim.experiments.timeline import (
    RecordedRouter,
    TimelineError,
    _IndexedMetricSink,
    _MetricSink,
    public_router_settings,
    settings_from_record,
)
from sheltersim.llm.cache import ResponseTape
from sheltersim.llm.config import LLMErrorPolicy, LLMMode
from sheltersim.llm.link import RouterLink
from sheltersim.llm.provider import LLMExecutionError, LLMRouter, OfflineProvider
from sheltersim.llm.runtime import SocialRuntimeLimits

BENCHMARKS = Path(__file__).resolve().parents[2] / "benchmarks"
if str(BENCHMARKS) not in sys.path:
    sys.path.insert(0, str(BENCHMARKS))

from gate_scenarios import ScriptedProvider, _llm_settings, generated, with_rumors  # noqa: E402

PLAIN = native.rules("plain")
MINUTES = 240
# The gate's stressed Run (`_stressed_run`): the limits open, so each kind of decision is asked.
LIMITS = {"cooldown_minutes": 5, "per_agent_per_day": 40, "global_per_day": 120}
CORE_LIMITS = json.dumps({"social": {**LIMITS, "complaint_wait_minutes": 5}}).encode()
# The keys of a social decision `_IndexedMetricSink.record_social_decision` keeps.
DIALOGUE_KEYS = {"minute", "agent_id", "event_id", "source_id", "status", "accepted", "reason"}


class _Keeping(ScriptedProvider):
    """Keeps what each call asks, and never leaves this process."""

    def __init__(self) -> None:
        self.asked: list[tuple[str, str]] = []

    def complete(self, *, model_id: str, task: Any, **options: Any) -> Any:
        self.asked.append((task.prompt_version, task.input_text))
        return super().complete(model_id=model_id, task=task, **options)


def _entry(world: Any, controller: Any) -> bytes:
    entry = export_world(world, controller)
    return json.dumps(entry, sort_keys=True, ensure_ascii=False, separators=(",", ":")).encode()


def _stressed(router: LLMRouter, sink: Any = None) -> tuple[WorldState, Any, bytes]:
    """The gate's stressed world at minute 0 with its controller, and its export."""
    scenario = with_rumors(generated(evacuees=24, minutes=MINUTES, staff_count=1))
    world = WorldState.from_scenario(scenario)
    for agent in world.agents.values():
        agent.stress = 0.9
    sink = sink if sink is not None else _MetricSink(MetricsRecorder())
    controller = OfflineSimulationController(world, sink, llm_router=router)
    controller.social_policy.limits = SocialRuntimeLimits(**LIMITS, complaint_wait_minutes=5)
    controller._run_core.refresh_policies(controller)
    return world, controller, _entry(world, controller)


def _record_router(provider: Any, **settings: Any) -> LLMRouter:
    settings = dataclasses.replace(_llm_settings(LLMMode.RECORD), **settings)
    return LLMRouter(settings, provider=provider, tape=ResponseTape())


def _linked(entry: bytes, link: Any) -> Any:
    run = PLAIN.PlainRun(entry)
    run.collect_decisions(True)
    run.set_llm(link, CORE_LIMITS)
    return run


def _dialogue(records: list[bytes]) -> list[dict[str, Any]]:
    return [row for row in (json.loads(raw) for raw in records) if row["record_kind"] == "dialogue"]


def _kept(row: dict[str, Any]) -> dict[str, Any]:
    """A core dialogue record as the Python sink keeps it."""
    kept = {key: value for key, value in row.items() if key in DIALOGUE_KEYS}
    kept.update(record_kind=row["record_kind"], speech_status=row["speech_status"])
    return kept


@pytest.fixture(scope="module")
def recorded() -> dict[str, Any]:
    """The stressed Run on both paths with the scripted provider: what each asked and gave."""
    python_provider = _Keeping()
    python_router = _record_router(python_provider)
    sink = _IndexedMetricSink(MetricsRecorder(), None)
    world, controller, entry = _stressed(python_router, sink)
    sink.world = world
    TickEngine(world, phase_handler=controller).run(MINUTES)
    core_provider = _Keeping()
    core_router = _record_router(core_provider)
    seen: list[Any] = []
    core_router.observer = seen.append
    run = _linked(entry, RouterLink(core_router))
    assert run.advance(MINUTES) == MINUTES
    return {
        "entry": entry,
        "python": {
            "world": world_digests(world),
            "asked": python_provider.asked,
            "router": python_router,
            "rows": [row for row in sink.observations if not isinstance(row, bytes)],
        },
        "core": {
            "run": run,
            "world": json.loads(run.digests_json()),
            "asked": core_provider.asked,
            "router": core_router,
            "observed": seen,
            "records": run.take_decisions(),
            "tape": run.take_tape(),
            "calls": run.llm_calls(),
        },
    }


def test_the_core_asks_what_the_python_rules_ask_and_reaches_the_same_world(
    recorded: dict[str, Any],
) -> None:
    python, core = recorded["python"], recorded["core"]
    kinds = Counter(version for version, _ in core["asked"])
    assert set(kinds) == {
        "support-priority-v2",
        "event-triggered-social-v2",
        "event-triggered-social-v2-complaint_response",
        "event-triggered-social-v2-cooperation",
        "event-triggered-social-v2-information_reaction",
    }
    # The prompts, byte for byte, in the same order, and the world after the ticks.
    assert core["asked"] == python["asked"]
    assert core["world"] == python["world"]

    # Each call's record but the state's hash, which is the core's digest of the world now, and the
    # time the call took.
    def identity(record: Any) -> tuple[Any, ...]:
        fields = record.to_dict()
        return tuple(v for k, v in fields.items() if k not in ("state_hash", "elapsed_ms"))

    assert [identity(r) for r in core["router"].records] == [
        identity(r) for r in python["router"].records
    ]
    assert core["calls"] == len(core["tape"]) == len(core["router"].records) == sum(kinds.values())
    # The router's observer (the usage ledger's hook) saw each call.
    assert core["observed"] == core["router"].records
    # The social policy's records, as the Python sink keeps them.
    assert [_kept(row) for row in _dialogue(core["records"])] == python["rows"]


def test_the_tape_holds_each_call_without_the_prompt_or_the_models_free_text(
    recorded: dict[str, Any],
) -> None:
    core = recorded["core"]
    for raw, record in zip(core["tape"], core["router"].records, strict=True):
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


def test_a_recorded_router_answers_the_run_again_through_the_link(
    recorded: dict[str, Any],
) -> None:
    """Python's `RecordedRouter` replays the recorded calls in order, without a provider."""
    router = recorded["core"]["router"]
    start = {"mode": "record", "api_request_count": 0, "spent_usd": 0.0, "records": []}
    replay = RecordedRouter(start, router.snapshot_state(), public_router_settings(router))
    run = _linked(recorded["entry"], RouterLink(replay))
    assert run.advance(MINUTES) == MINUTES
    assert json.loads(run.digests_json()) == recorded["core"]["world"]
    assert run.take_tape() == recorded["core"]["tape"]
    assert [r.to_dict() for r in replay.records] == [r.to_dict() for r in router.records]
    # Another world asks other questions: the recorded order refuses them.
    other = RecordedRouter(start, router.snapshot_state(), public_router_settings(router))
    world, controller, _entry_bytes = _stressed(_record_router(_Keeping()))
    for agent in world.agents.values():
        agent.stress = 0.8
    run = _linked(_entry(world, controller), RouterLink(other))
    with pytest.raises(RuntimeError) as stopped:
        run.advance(MINUTES)
    assert isinstance(stopped.value.__cause__, TimelineError)
    assert "does not match reconstructed history" in str(stopped.value.__cause__)


def test_the_cores_replay_answers_from_the_stored_tape_and_never_asks(
    recorded: dict[str, Any],
) -> None:
    tape = recorded["core"]["tape"]
    run = PLAIN.PlainRun(recorded["entry"])
    run.set_llm_replay(tape, False, None, CORE_LIMITS)
    assert run.advance(MINUTES) == MINUTES
    assert json.loads(run.digests_json()) == recorded["core"]["world"]
    assert run.take_tape() == tape
    # Past the tape's end, a replay stops; a link given to go on with answers from there.
    short = PLAIN.PlainRun(recorded["entry"])
    short.set_llm_replay(tape[:10], False, None, CORE_LIMITS)
    with pytest.raises(RuntimeError, match="history cannot call a provider"):
        short.advance(MINUTES)
    provider = _Keeping()
    going_on = PLAIN.PlainRun(recorded["entry"])
    going_on.set_llm_replay(tape[:10], False, RouterLink(_record_router(provider)), CORE_LIMITS)
    assert going_on.advance(MINUTES) == MINUTES
    assert json.loads(going_on.digests_json()) == recorded["core"]["world"]
    assert len(provider.asked) == len(tape) - 10


def test_an_offline_run_is_the_run_without_an_llm_and_writes_the_project_routes_rows() -> None:
    """A Project's Run without an API key (the offline router of `TimelineStore`): the core's
    records are the indexed sink's rows, in order, its `dialogue` rows (#385) too."""
    world, controller, _engine = generated_run()
    entry = _entry(world, controller)
    sink = _IndexedMetricSink(MetricsRecorder(), world)
    offline = LLMRouter(settings_from_record({}, mode=LLMMode.OFFLINE), provider=OfflineProvider())
    project = OfflineSimulationController(world, sink, llm_router=offline)
    TickEngine(world, phase_handler=project).run(GENERATED_TICKS)
    run = PLAIN.PlainRun(entry)
    run.collect_decisions(True)
    link = RouterLink(
        LLMRouter(settings_from_record({}, mode=LLMMode.OFFLINE), provider=OfflineProvider())
    )
    assert link.offline
    run.set_llm(link)
    assert run.advance(GENERATED_TICKS) == GENERATED_TICKS
    assert run.llm_calls() == 0 and run.take_tape() == []
    assert json.loads(run.digests_json()) == world_digests(world)
    without = PLAIN.PlainRun(entry)
    assert without.advance(GENERATED_TICKS) == GENERATED_TICKS
    assert without.digests_json() == run.digests_json()
    assert without.metrics_digest() == run.metrics_digest()
    rows = [row if isinstance(row, bytes) else ("dialogue", row) for row in sink.observations]
    records = [
        raw if json.loads(raw)["record_kind"] == "actions" else ("dialogue", _kept(json.loads(raw)))
        for raw in run.take_decisions()
    ]
    assert any(isinstance(row, tuple) for row in rows)
    assert records == rows


def test_the_routers_budget_and_stop_policy_hold_through_the_link() -> None:
    """A router without budget falls back without asking; one that stops raises through the tick."""
    provider = _Keeping()
    poor = _record_router(provider, budget_usd=0.0)
    _world, _controller, entry = _stressed(poor)
    run = _linked(entry, RouterLink(poor))
    assert run.advance(MINUTES) == MINUTES
    assert provider.asked == []
    tape = [json.loads(raw) for raw in run.take_tape()]
    assert tape and {entry["outcome"] for entry in tape} == {"budget_exceeded"}
    assert all(entry["fallback_used"] for entry in tape)
    accepted = [row for row in _dialogue(run.take_decisions()) if row["accepted"]]
    assert accepted == []
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

    _world, _controller, entry = _stressed(_record_router(_Keeping()))
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
