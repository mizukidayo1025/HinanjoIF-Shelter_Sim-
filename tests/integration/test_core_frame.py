"""The frame of a minute is the core's, the frame Python made before (#264).

`inspection.frames.world_frame` reads the frame the core makes (`sheltersim_core::frame`, through
the native rule `frame.world_frame`). Until PR #270 Python made it; these frames have the sha256
of their canonical JSON that the Python frame had then, on the same worlds (generated Scenarios of
20 and 100 people over a day, at 0, 1, 720 and 1439 minutes). The worlds of
`support/core_frames.py` (no floor plan, facilities moved by an EffectProgram, people walking in
on an approach route) are checked the same way by `native/core/tests/data/frames.json`, which
`integration/test_core_fixtures.py` rebuilds.
"""

from __future__ import annotations

import hashlib

import pytest
from support.core_fixtures import run_world
from support.core_tick import generated_run

from sheltersim.core.checkpoint_cache import canonical_bytes
from sheltersim.inspection.frames import world_frame, world_frame_bytes

# The sha256 of the Python frame at each minute, by the number of people (PR #270; since #376 the
# shared operations' read_route names the event log instead of removed history kinds).
PYTHON_FRAMES: dict[int, tuple[tuple[int, str], ...]] = {
    20: (
        (0, "ac6edce2eea92b76b429da6664a47798506e1e72ab660fb4f0cd775a24274279"),
        (1, "556ad59554aac29e3452a55f9b0615d7cfbc594ccef427cf242fc72c89236195"),
        (720, "cc15c7ebac89e7e677f8d77f7285c92955b891fb6635dbe3c138e7e610a538fa"),
        (1439, "d61f0d7630b0be80c22c0138823adf3fe4857eac18512471f639e469026c3a08"),
    ),
    100: (
        (0, "33276ed58d71cfc85fde1938657e086d9d0cf8f797a092c095a53b7347fc7f95"),
        (1, "86134312fb745fdffbd353b082505da99c664ee9298e40d6215395279c21c228"),
        (720, "59e48a1c6967c119d053686e1922628c82bbe9d2348bcfee40f199fc702523e2"),
        (1439, "6d72366ac6bad7bc6ba39ea742ff527102bab83562f3014811c28ec1cc4bf9c3"),
    ),
}


def _generated(people: int):
    from sheltersim.experiments.initial_scenario import (
        InitialScenarioBuilder,
        InitialScenarioRequest,
    )

    request = InitialScenarioRequest(
        municipality="合成市", disaster="地震", evacuees=people, duration_days=1, seed=2026
    )
    return run_world(InitialScenarioBuilder().build(request).scenario)


@pytest.mark.parametrize("people", sorted(PYTHON_FRAMES))
def test_frame_of_a_generated_day_is_the_one_python_made(people: int) -> None:
    world, _, engine = _generated(people)
    for minute, digest in PYTHON_FRAMES[people]:
        while world.minute_index < minute:
            engine.step()
        raw = world_frame_bytes(world, run_id="run", branch_id="main", computed_until=minute)
        assert hashlib.sha256(raw).hexdigest() == digest, minute
    frame = world_frame(world, run_id="run", branch_id="main", computed_until=1439)
    assert canonical_bytes(frame) == raw
    assert any(person["care_history"] for person in frame["people"])
    if people == 100:
        assert frame["recorded_history"]["social"]["event_window_truncated"]


def test_frame_names_the_digest_given_and_changes_nothing() -> None:
    world, _, engine = generated_run()
    for _ in range(40):
        engine.step()
    before = world.canonical_json()
    framed = world_frame(
        world, run_id="run", branch_id="main", computed_until=60, state_hash="0" * 64
    )
    assert framed["state_hash"] == "0" * 64
    assert (framed["minute"], framed["computed_until"]) == (40, 60)
    framed = world_frame(world, run_id="run", branch_id="main", computed_until=40)
    assert framed["state_hash"] == world.state_digest()
    assert world.canonical_json() == before
    engine.step()
