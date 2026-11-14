"""The core's Run without Python (`PlainRun`) from Python, against the native path (#368, step 2).

`sheltersim_native.rules.plain.PlainRun` is the browser's `Simulation` for Python: it reads the
JSON of `export_world` (as `scripts/build_browser_sample.py` writes it) and runs the ticks on the
core's host without Python (`sheltersim_core::plain`). Each world of `support/core_tick.py` runs
through it and through `TickEngine` (the native path) to the same world digests and metric samples.
With its decisions collected, a `PlainRun` gives the bytes of the rows the core writes for the
native path's `_IndexedMetricSink` on a Project route's Run (the controller of
`TimelineStore._advance_collecting_less`, with the offline router a Project's Run has). A saved
point restored from the stored log and samples goes on to the same world as the Run straight
through, and the errors are the browser's texts.
"""

from __future__ import annotations

import dataclasses
import json
from typing import Any

import pytest
from support.core_fixtures import run_world
from support.core_tick import (
    GENERATED_TICKS,
    TICKS,
    core_tick_run,
    generated_run,
    generated_scenario,
    native_result,
    route_closed_run,
    world_digests,
)

from sheltersim import native
from sheltersim.core.engine import TickEngine
from sheltersim.core.world_export import export_world
from sheltersim.events.effects import schedule_effect_program, validate_effect_program
from sheltersim.experiments.metrics import MetricsRecorder
from sheltersim.experiments.runner import OfflineSimulationController
from sheltersim.experiments.timeline import _IndexedMetricSink, settings_from_record
from sheltersim.inspection.frames import world_frame_bytes
from sheltersim.llm.config import LLMMode
from sheltersim.llm.natural_language import scenario_summary_from_world
from sheltersim.llm.provider import LLMRouter, OfflineProvider

PLAIN = native.rules("plain")
BUILDERS = {
    "small_replay": (core_tick_run, TICKS),
    "generated": (generated_run, GENERATED_TICKS),
    "route_closed": (route_closed_run, GENERATED_TICKS),
}


def _entry(world: Any, controller: Any) -> bytes:
    """The exported world's JSON bytes, as `build_browser_sample.py` writes them."""
    entry = export_world(world, controller)
    return json.dumps(entry, sort_keys=True, ensure_ascii=False, separators=(",", ":")).encode()


def _canonical(value: Any) -> bytes:
    return json.dumps(
        value, sort_keys=True, ensure_ascii=False, separators=(",", ":"), allow_nan=False
    ).encode()


@pytest.mark.parametrize("name", sorted(BUILDERS))
def test_a_plain_run_reaches_the_native_path_s_world_and_samples(name: str) -> None:
    build, ticks = BUILDERS[name]
    world, controller, engine = build()
    run = PLAIN.PlainRun(_entry(world, controller))
    assert (run.minute(), run.duration()) == (0, world.duration_minutes)
    assert run.advance(ticks) == ticks
    expected = native_result(world, controller, engine, ticks)
    assert run.minute() == world.minute_index == ticks
    assert json.loads(run.digests_json()) == expected["world"]
    assert run.metric_count() == expected["metric_count"]
    assert run.metrics_digest() == expected["metrics"]
    lines = [run.metric_line(index) for index in range(run.metric_count())]
    assert run.metric_line(run.metric_count()) is None
    snapshots = controller.artifacts.metrics.snapshots
    assert lines == [
        _canonical([getattr(snapshot, field.name) for field in dataclasses.fields(snapshot)])
        for snapshot in snapshots
    ]
    # What the API reads of the world now: its digest, the frame and the scenario summary.
    assert run.state_digest() == world.state_digest()
    frame = run.frame("run-1", "main", ticks)
    assert frame == world_frame_bytes(world, run_id="run-1", branch_id="main", computed_until=ticks)
    summary = scenario_summary_from_world(world).model_dump(mode="json")
    assert run.scenario_summary() == _canonical(summary)
    # The digest of each part: the scalars aside, the run's digests are some of them.
    parts = json.loads(run.part_digests_json())
    assert {part: digest for part, digest in expected["world"].items() if part != "scalars"} == {
        part: parts[part] for part in expected["world"] if part != "scalars"
    }


def test_a_plain_run_checks_and_schedules_an_effect_program_as_python_does(
    add_toilets_program: Any,
) -> None:
    """Toilets added at minute 30 (a structure intervention, after which the routes are made
    again): the issues, the receipt and the world after the ticks are the native path's."""
    world, controller, engine = run_world(generated_scenario())
    run = PLAIN.PlainRun(_entry(world, controller))
    program = add_toilets_program(start_minute=30)
    raw = _canonical(program.model_dump(mode="json"))
    late = add_toilets_program(start_minute=world.duration_minutes + 10)
    issues = list(validate_effect_program(world, late))
    assert len(issues) == 2
    assert json.loads(run.validate_effect_program(_canonical(late.model_dump(mode="json")))) == (
        issues
    )
    # An issue that holds pydantic's text is left to Python, as in the browser.
    many = _canonical(add_toilets_program(50, start_minute=30).model_dump(mode="json"))
    with pytest.raises(native.Unsupported, match="pydantic's text"):
        run.validate_effect_program(many)
    assert (
        json.loads(run.validate_effect_program(raw))
        == list(validate_effect_program(world, program))
        == []
    )
    receipt = json.loads(run.apply_effect_program(raw))
    scheduled = schedule_effect_program(world, program)
    assert receipt == {
        "command": "effect_program",
        "program_id": scheduled.program_id,
        "program_hash": scheduled.program_hash,
        "event_ids": list(scheduled.event_ids),
        "minute": 0,
    }
    assert run.advance(GENERATED_TICKS) == GENERATED_TICKS
    for _ in range(GENERATED_TICKS):
        engine.step()
    assert json.loads(run.digests_json()) == world_digests(world)
    assert "effect_program_structure" in {
        record.event.event_type for record in world.event_log.records
    }


def _project_route(world: Any) -> tuple[Any, Any]:
    """The controller and the engine of a Project route's computation on `world`
    (`TimelineStore._advance_collecting_less`): the indexed sink, and the offline router a
    Project's Run is given."""
    sink = _IndexedMetricSink(MetricsRecorder(), world)
    router = LLMRouter(settings_from_record({}, mode=LLMMode.OFFLINE), provider=OfflineProvider())
    controller = OfflineSimulationController(world, sink, llm_router=router)
    return controller, TickEngine(world, phase_handler=controller)


def test_a_plain_run_s_decisions_are_the_bytes_of_the_project_route_s_rows() -> None:
    world, controller, _engine = generated_run()
    run = PLAIN.PlainRun(_entry(world, controller))
    run.collect_decisions(True)
    project, engine = _project_route(world)
    half = GENERATED_TICKS // 2
    for _ in range(half):
        engine.step()
    assert run.advance(half) == half
    records = run.take_decisions()
    for _ in range(GENERATED_TICKS - half):
        engine.step()
    assert run.advance(GENERATED_TICKS) == GENERATED_TICKS - half
    records += run.take_decisions()
    observations = project.artifacts.observations
    rows = [row for row in observations if isinstance(row, bytes)]
    assert len(rows) > 100
    assert records == rows
    assert run.decisions_missed() == 0
    # The world is the native path's with the offline router, and collecting changed nothing.
    assert json.loads(run.digests_json()) == world_digests(world)
    silent = PLAIN.PlainRun(_entry(*generated_run()[:2]))
    assert silent.advance(GENERATED_TICKS) == GENERATED_TICKS
    assert silent.take_decisions() == []
    assert silent.digests_json() == run.digests_json()
    assert silent.metrics_digest() == run.metrics_digest()
    # The rows only Python writes: the offline social policy's decisions (`record_social_decision`),
    # which a host without an LLM never asks for.
    others = [row for row in observations if not isinstance(row, bytes)]
    assert others
    assert {row["record_kind"] for row in others} == {"dialogue"}
    assert {json.loads(row)["record_kind"] for row in rows} == {"actions"}


def _stored(run: Any) -> dict[str, Any]:
    """What a Run stores up to now (`js/run_store.mjs`): its log's and samples' tables and every
    block, after they are sealed."""
    run.seal_blocks()
    log_blocks, log_sealed = run.log_block_counts()
    sample_blocks, sample_sealed = run.sample_block_counts()
    assert (log_blocks, sample_blocks) == (log_sealed, sample_sealed)
    assert run.log_all_packed()
    return {
        "point": run.saved_point(),
        "log_tables": run.log_tables(0, 0),
        "log_blocks": [run.log_block(index) for index in range(log_blocks)],
        "sample_tables": run.sample_tables(0, 0),
        "sample_blocks": [run.sample_block(index) for index in range(sample_blocks)],
    }


def _restore(stored: dict[str, Any]) -> Any:
    restore = PLAIN.Restore(stored["point"])
    restore.read_log_tables(stored["log_tables"])
    for block in stored["log_blocks"]:
        if restore.log_needed():
            restore.read_log_block(block)
    restore.read_sample_tables(stored["sample_tables"])
    for block in stored["sample_blocks"]:
        if restore.samples_needed():
            restore.read_sample_block(block)
    assert not restore.log_needed() and not restore.samples_needed()
    return restore


def test_a_plain_run_restored_at_a_saved_point_goes_on_as_the_run_straight_through() -> None:
    world, controller, _engine = generated_run()
    entry = _entry(world, controller)
    straight = PLAIN.PlainRun(entry)
    at = 120
    assert straight.advance(at) == at
    stored = _stored(straight)
    records = straight.log_records()
    samples = straight.metric_count()
    world_parts = straight.world_parts()
    assert straight.advance(GENERATED_TICKS) == GENERATED_TICKS - at
    restore = _restore(stored)
    assert (restore.minute(), restore.records(), restore.samples()) == (at, records, samples)
    again = PLAIN.PlainRun.restore(entry, restore)
    assert again.minute() == at
    assert again.world_parts() == world_parts
    assert again.advance(GENERATED_TICKS) == GENERATED_TICKS - at
    assert again.digests_json() == straight.digests_json()
    assert again.metrics_digest() == straight.metrics_digest()
    assert again.state_digest() == straight.state_digest()
    # A restore is used once.
    with pytest.raises(ValueError, match="^the saved point was restored already$"):
        PLAIN.PlainRun.restore(entry, restore)
    with pytest.raises(ValueError, match="^the saved point was restored already$"):
        restore.minute()
    assert PLAIN.saved_point_version() == 2


def test_a_plain_run_s_errors_are_the_browser_s_texts() -> None:
    world, controller, _engine = core_tick_run()
    entry = json.loads(_entry(world, controller))
    with pytest.raises(ValueError, match="^not an exported world: "):
        PLAIN.PlainRun(b"not json")
    with pytest.raises(ValueError, match="^not an exported world: scenario_id is not a str$"):
        PLAIN.PlainRun(b"{}")
    worlds = _canonical({"worlds": {"small_replay": entry}})
    with pytest.raises(ValueError, match="^the file has no world no_such_world$"):
        PLAIN.PlainRun.from_worlds(worlds, "no_such_world")
    with pytest.raises(ValueError, match="^not a saved point: "):
        PLAIN.Restore(b"not a point")
    # Past the end of the clock, no tick runs and nothing changes.
    run = PLAIN.PlainRun.from_worlds(worlds, "small_replay")
    assert run.advance(TICKS + 10) == TICKS
    digests = run.digests_json()
    assert run.advance(1) == 0
    assert run.digests_json() == digests
    # A tick that fails stops `advance` with the minute, the step and the error, which is its cause.
    entry["world"]["seed"] = -1
    run = PLAIN.PlainRun(_canonical(entry))
    with pytest.raises(RuntimeError) as failed:
        run.advance(3)
    assert str(failed.value) == "minute 0: decisions.propose_rows: the seed is negative"
    assert isinstance(failed.value.__cause__, native.Unsupported)
    assert str(failed.value.__cause__) == "the seed is negative"
