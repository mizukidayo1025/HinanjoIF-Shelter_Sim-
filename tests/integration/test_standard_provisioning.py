"""Standard provisioning keeps a generated Scenario served while it runs.

How the generator chooses facilities, staff, stock and resupply is checked in
tests/unit/region/test_facility.py and tests/unit/region/test_conditions.py.
"""

from __future__ import annotations

import json

import sheltersim_native

from sheltersim import native


def test_a_pending_region_delivery_does_not_stop_the_run(generated_scenario_with) -> None:
    # Region-condition deliveries lacked a shipment ID, so no operational-event state
    # was created and the first validation with a pending delivery stopped the Run.
    scenario = generated_scenario_with(
        operational_conditions={
            "initial_resources": {
                "status": "entered",
                "water_ml": 90_000,
                "food_meal_units": 90,
                "source_kind": "operator",
                "source_ids": ["SRC-OPERATOR-INITIAL-CONDITIONS"],
            },
            "scheduled": [
                {
                    "kind": "resource_delivery",
                    "resource_id": "water_ml",
                    "quantity": 1_000,
                    "due_minute": 60,
                },
            ],
        },
    )
    text = scenario.canonical_json().encode()
    entry = json.loads(sheltersim_native.initial_entry(text))
    shipments = entry["world"]["model_state"]["operational_events_v1"]["shipments"]
    assert any(key.startswith("region-shipment-") for key in shipments)
    run = native.rules("plain").PlainRun.from_scenario(text)
    assert run.advance(120) == 120
