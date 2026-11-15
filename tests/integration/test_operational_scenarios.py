"""H05 operational scenarios: a seven-day combined Run keeps its invariants.

That the six scenario files stay frozen is checked in
tests/unit/repo/test_scenario_fixtures.py.
"""

from __future__ import annotations

import csv
import json
from pathlib import Path

import pytest
from support.paths import SCENARIOS

from sheltersim.core.engine import TraceLevel
from sheltersim.domain.resources import RESOURCE_LEDGER_KEY, validate_resource_ledger
from sheltersim.experiments.runner import run_scenario
from sheltersim.experiments.scenario import ScenarioSpec


def _load(name: str) -> ScenarioSpec:
    return ScenarioSpec.load(SCENARIOS / f"{name}.json")


@pytest.mark.long_simulation
def test_100_agent_seven_day_combined_scenario_preserves_operational_invariants(
    tmp_path: Path,
) -> None:
    scenario = _load("multi_operational")
    output = tmp_path / "multi-operational"

    world = run_scenario(scenario, output, trace_level=TraceLevel.SUMMARY)

    assert world.minute_index == 10080
    assert len(world.agents) == 100
    world.validate()
    ledger = world.model_state[RESOURCE_LEDGER_KEY]
    validate_resource_ledger(ledger, world.resources, set(world.agents), set(world.zones))
    assert all(account["remaining"] >= 0 for account in ledger["accounts"].values())
    assert all(
        account["reserved"] <= account["remaining"] for account in ledger["accounts"].values()
    )

    event_records = {record.event.event_id: record for record in world.event_log.records}
    assert event_records["delivery-delay-day1"].status.value == "applied"
    assert event_records["water-outage-day4"].status.value == "applied"
    delayed_delivery = event_records["shipment-meals-day2"]
    assert delayed_delivery.realized_minute == 5760
    assert delayed_delivery.event.causation_id == "delivery-delay-day1"
    stalls = world.model_state["resource_service_v1"]["queue_stall_history"]
    assert any(item["reason"] == "water_outage" for item in stalls)
    assert any(item["reason"] == "stock" for item in stalls)

    with (output / "operational_kpis.csv").open(encoding="utf-8", newline="") as stream:
        kpis = list(csv.DictReader(stream))
    assert len(kpis) == 10080
    assert {
        "minute_index",
        "hunger_need_mean",
        "hydration_need_mean",
        "water_queue_length",
        "meal_queue_length",
        "water_service_wait_mean_minutes",
        "food_meal_units_remaining",
        "staff_load_ratio",
        "stress_mean",
        "support_wait_staff_busy_request_minutes",
        "support_wait_staff_off_shift_request_minutes",
    } <= set(kpis[0])

    manifest = json.loads((output / "manifest.json").read_text(encoding="utf-8"))
    assert manifest["execution_mode"] == "offline"
    assert manifest["scenario_input_source_ids"]["facility_capacity"] == [
        "ASM-H05-OPERATIONAL-CAPACITY-V1"
    ]
    assert manifest["scheduled_event_source_ids"]
    assert any(
        facility["id"] == "water_station_main" and facility["capacity"] == 1
        for facility in manifest["facilities"]
    )
    initial_profiles = json.loads((output / "agents_initial.json").read_text(encoding="utf-8"))[
        "profiles"
    ]
    planned_events = json.loads((output / "events_planned.json").read_text(encoding="utf-8"))[
        "events"
    ]
    assert len(initial_profiles) == 100
    assert {event["id"] for event in planned_events} >= {
        "delivery-delay-day1",
        "water-outage-day4",
    }
