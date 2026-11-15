"""Generated Scenarios with entered food and water stock run a whole day in the core."""

from __future__ import annotations

import copy

import pytest

from sheltersim import native
from sheltersim.experiments.scenario import ScenarioSpec


@pytest.mark.long_simulation
def test_generated_scenario_with_entered_stock_completes_a_day(
    generated_previews, tmp_path
) -> None:
    # Before the fixes this Run stopped at minute 495 (stale stall) and, once that
    # was fixed, at minute 512 (in-room request reason rejected by validation);
    # both were found with seed 7.
    preview = generated_previews(
        evacuees=30,
        duration_days=1,
        operational_conditions={
            "initial_resources": {
                "status": "entered",
                "water_ml": 9_000 * 30,
                "food_meal_units": 9 * 30,
                "source_kind": "assumption",
                "source_ids": ["ASM-TEST-ENTERED-STOCK"],
            }
        },
    )
    assert preview["approval_blockers"] == []
    del tmp_path
    scenario = ScenarioSpec.model_validate(copy.deepcopy(preview["scenario"]))
    run = native.rules("plain").PlainRun.from_scenario(scenario.canonical_json().encode())
    assert run.advance(1440) == 1440
