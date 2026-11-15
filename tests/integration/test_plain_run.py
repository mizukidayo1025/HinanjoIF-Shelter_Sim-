"""The core's Run without Python (`PlainRun`) from Python (#368, step 2).

`sheltersim_native.rules.plain.PlainRun` is the browser's `Simulation` for Python: it reads the
entry the core builds of a Scenario's world at minute 0 (`initial_entry`, #429), and runs the
ticks on the core's host (`sheltersim_core::plain`). A saved point restored from the stored log
and samples goes on to the same world as the Run straight through, and the errors are the
browser's texts. That the core's Runs give the results the Python rules gave is pinned by the
gate (`benchmarks/gate_all_paths.py`) and the core's own tests.
"""

from __future__ import annotations

import json
from typing import Any

import pytest
import sheltersim_native
from support.paths import SMALL_REPLAY

from sheltersim import native
from sheltersim.experiments.scenario import ScenarioSpec

PLAIN = native.rules("plain")


def _canonical(value: Any) -> bytes:
    return json.dumps(
        value, sort_keys=True, ensure_ascii=False, separators=(",", ":"), allow_nan=False
    ).encode()


def _entry(scenario: Any) -> bytes:
    return bytes(sheltersim_native.initial_entry(scenario.canonical_json().encode()))


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


def test_a_plain_run_of_a_scenario_starts_from_the_entry_the_core_builds(
    generated_scenario: Any,
) -> None:
    text = generated_scenario.canonical_json().encode()
    built = PLAIN.PlainRun.from_scenario(text)
    read = PLAIN.PlainRun(_entry(generated_scenario))
    assert (built.minute(), built.duration()) == (0, generated_scenario.duration_minutes)
    assert built.advance(120) == read.advance(120) == 120
    assert built.digests_json() == read.digests_json()
    assert built.metrics_digest() == read.metrics_digest()


def test_a_plain_run_restored_at_a_saved_point_goes_on_as_the_run_straight_through(
    generated_scenario: Any,
) -> None:
    entry = _entry(generated_scenario)
    end = 360
    straight = PLAIN.PlainRun(entry)
    at = 120
    assert straight.advance(at) == at
    stored = _stored(straight)
    records = straight.log_records()
    samples = straight.metric_count()
    world_parts = straight.world_parts()
    assert straight.advance(end - at) == end - at
    restore = _restore(stored)
    assert (restore.minute(), restore.records(), restore.samples()) == (at, records, samples)
    again = PLAIN.PlainRun.restore(entry, restore)
    assert again.minute() == at
    assert again.world_parts() == world_parts
    assert again.advance(end - at) == end - at
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
    scenario = ScenarioSpec.load(SMALL_REPLAY)
    entry = json.loads(_entry(scenario))
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
    ticks = run.duration()
    assert run.advance(ticks + 10) == ticks
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
