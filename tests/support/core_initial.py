"""The worlds and the installed catalogs of the core's minute-0 builder (Issues #249, #429).

The core builds the entry of any Scenario's world at minute 0 (`native/core/src/initial/`) from
the Scenario's canonical JSON, the bytes `export_world` writes for the browser, and a new Run's
records (`model_versions` and the evidence route's answer, #301). What the records read of the installed code (the operator and model
catalog, the evidence's catalogs, sources and texts) is `native/core/src/initial/records.json`
(`initial_records`). `SCREEN_REQUESTS` are the requests of the Scenario screen (the structured
form and the example text's) with edge values, whose Scenarios the tests build in both.

`initial_worlds.json` (`initial_worlds_fixture`) holds generated Scenarios' canonical JSON with the
sha256 of the entry `export_world` writes of their worlds at minute 0, and the digests of the
worlds after `ticks` ticks through `TickEngine`: the core's tests (`native/core/tests/initial.rs`)
and the WASM build's (`native/wasm/tests/node_initial.mjs`) build the entry in the core and run it.
"""

from __future__ import annotations

import hashlib
import json
from typing import Any

from sheltersim.core.world_export import export_world
from support.core_fixtures import run_world

RECORDS_SCHEMA_VERSION = 1
# What the evidence route's answer holds that depends on the Scenario: the core makes it.
SCENARIO_EVIDENCE = (
    "models",
    "model_versions",
    "parameters",
    "scenario_sources",
    "scenario_assumptions",
    "region_limits",
)
# The screen's form with edge values and the example text's request (墨田区, earthquake, 100
# people, 7 days). The screen sends no policy IDs, plan, selected shelter or overrides.
BASE_REQUEST: dict[str, Any] = {
    "municipality": "墨田区",
    "disaster": "地震",
    "evacuees": 100,
    "duration_days": 7.0,
    "seed": 2026,
}
SCREEN_REQUESTS: dict[str, dict[str, Any]] = {
    "example": {},
    "three": {"evacuees": 3, "municipality": "合成市"},
    "twenty": {"evacuees": 20},
    "no_staff": {"staff_count": 0},
    "staff_12": {"staff_count": 12},
    "no_accessible_toilet": {"accessible_toilet_count": 0},
    "twenty_accessible_toilets": {"accessible_toilet_count": 20},
    "half_day": {"duration_days": 0.5},
    "two_and_a_half_days": {"duration_days": 2.5, "evacuees": 30},
    "small_area": {"gross_area_m2": 400.0, "evacuees": 20},
    "large_area": {"gross_area_m2": 2400.0, "evacuees": 250},
    "conditions": {
        "facility_conditions": ("ペット同伴あり", "授乳室あり", "個室あり", "車いす利用者あり")
    },
    "flood": {"disaster": "水害", "seed": 7},
    "stock_entered": {
        "operational_conditions": {
            "initial_resources": {"status": "entered", "water_ml": 300000, "food_meal_units": 250}
        }
    },
    "stock_incomplete": {
        "operational_conditions": {"initial_resources": {"status": "incomplete", "water_ml": 1000}}
    },
}
# The worlds of `initial_worlds.json`: the request (over `BASE_REQUEST`) and the ticks to run.
WORLDS: dict[str, tuple[dict[str, Any], int]] = {
    "twenty": ({"evacuees": 20, "duration_days": 1.0}, 1440),
    "hundred": ({"duration_days": 1.0}, 1440),
}


def scenario_of(request: dict[str, Any]) -> Any:
    """The Scenario the builder makes of `BASE_REQUEST` updated with `request`."""
    from sheltersim.experiments.initial_scenario import (
        InitialScenarioBuilder,
        InitialScenarioRequest,
    )

    built = InitialScenarioBuilder().build(InitialScenarioRequest(**{**BASE_REQUEST, **request}))
    return built.scenario


def entry_text(entry: dict[str, Any]) -> bytes:
    """The bytes of an entry as the browser's sample writes them (`export.json`)."""
    return json.dumps(entry, sort_keys=True, ensure_ascii=False, separators=(",", ":")).encode()


def minute_zero_entry(scenario: Any) -> dict[str, Any]:
    """`export_world` of the Scenario's world at minute 0, set up for a Run."""
    world, controller, _ = run_world(scenario)
    return export_world(world, controller)


def evidence_answer(scenario: Any) -> dict[str, Any]:
    """What the server's evidence route answers for a Run of the Scenario in a Project without
    datasets (`web/project_services.py`): `evidence_context` with the catalogs it adds."""
    from sheltersim.contracts.evidence import operator_evidence_catalog
    from sheltersim.contracts.runtime import state_catalog
    from sheltersim.domain.simulator_state import simulator_state_catalog
    from sheltersim.experiments.validation import evidence_context
    from sheltersim.policies.agents_social_manifest import agents_social_model_catalog

    result = evidence_context(scenario)
    result["state_contracts"] = state_catalog()
    result["simulator_states"] = simulator_state_catalog()
    result["operator_evidence"] = operator_evidence_catalog()
    result["agents_social_operators"] = agents_social_model_catalog()
    result["project_datasets"] = []
    result["project_datasets_truncated"] = False
    return json.loads(json.dumps(result))


def initial_records() -> dict[str, Any]:
    """`native/core/src/initial/records.json`: the evidence route's answer for a new Run but what
    depends on the Scenario (`SCENARIO_EVIDENCE`): the catalogs of the installed code (the
    operator and model catalog the core also signs the models with), the sources, the legend and
    the texts."""
    answer = evidence_answer(scenario_of({}))
    evidence = {key: value for key, value in answer.items() if key not in SCENARIO_EVIDENCE}
    return {"schema_version": RECORDS_SCHEMA_VERSION, "evidence": evidence}


def _sha(raw: bytes) -> str:
    return hashlib.sha256(raw).hexdigest()


def initial_worlds_fixture() -> dict[str, Any]:
    """`initial_worlds.json`: for each of `WORLDS`, the Scenario's canonical JSON (`scenario`, a
    str), the sha256 and length of the entry of its world at minute 0, and the digests of its world
    after the ticks."""
    from sheltersim.contracts.policies import model_signatures
    from support.core_tick import metrics_digest, world_digests

    worlds = {}
    for name, (request, ticks) in WORLDS.items():
        scenario = scenario_of(request)
        text = scenario.canonical_json()
        world, controller, _engine = run_world(scenario)
        entry = entry_text(export_world(world, controller))
        initial = world_digests(world)
        del world, controller, _engine
        world, controller, _engine = run_world(scenario, ticks)
        snapshots = controller.artifacts.metrics.snapshots
        evidence = json.dumps(
            evidence_answer(scenario), sort_keys=True, ensure_ascii=False, separators=(",", ":")
        ).encode()
        worlds[name] = {
            "scenario": text,
            "expected": {
                "entry_sha256": _sha(entry),
                "entry_bytes": len(entry),
                "model_versions": model_signatures(scenario.model_configs),
                "evidence_sha256": _sha(evidence),
                "ticks": ticks,
                "initial": initial,
                "world": world_digests(world),
                "metrics": metrics_digest(snapshots),
                "metric_count": len(snapshots),
            },
        }
    return {"initial_worlds": worlds}
