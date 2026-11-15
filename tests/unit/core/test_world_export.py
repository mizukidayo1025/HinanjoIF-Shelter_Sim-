"""`sheltersim.core.world_export`: one world as the JSON the core's loader reads (#244).

The core's tests check what the loader builds of an export (`native/core/tests/`, with the files
`integration/test_core_fixtures.py` keeps current); here, the parts of an export, that exporting
changes nothing of the world, and that the core's own builder of the world at minute 0
(`sheltersim_core::initial`, #249) writes the bytes Python exports for the screen's Scenarios.
"""

from __future__ import annotations

import hashlib
import json
from collections.abc import Callable
from typing import Any

import pytest
from support.core_initial import (
    TABLE_REQUESTS,
    entry_text,
    evidence_answer,
    minute_zero_entry,
    scenario_of,
)
from support.initial_corpus import load_corpus, world_entry_bytes

from sheltersim.core.world import WorldState
from sheltersim.core.world_export import configs_fingerprint, export_world

# The sections the host parses (`SectionId::parser() == Parser::Host`), by their `Configs` field.
HOST_SECTIONS = {
    "selection",
    "tick_plan",
    "facility_geometry",
    "spatial_constraints",
    "staff_operations",
    "support_cooldown_minutes",
    "information_spread",
    "body_models",
    "care_outcome",
    "zone_environment",
    "infrastructure",
    "proximity",
    "transit_capacities",
    "overlay_rooms",
    "extensions_enabled",
}


def test_export_has_the_world_its_slots_and_the_host_sections(
    small_replay_scenario: Callable[..., Any],
) -> None:
    world = WorldState.from_scenario(small_replay_scenario())
    before = world.canonical_json()

    entry = export_world(world)

    assert entry["world"] == json.loads(before)
    assert world.canonical_json() == before
    assert "run" not in entry
    names = [name for name, _ in entry["slots"]]
    assert len(names) == len(set(names))
    assert {"resources", "metric_state", "model_configs", "id_counters"} <= set(names)
    assert set(entry["sections"]) == HOST_SECTIONS
    # The small replay has no facility geometry.
    assert entry["sections"]["facility_geometry"] == {"absent": True}
    json.dumps(entry, allow_nan=False)


# ---- The core's minute-0 builder (#249) ----------------------------------------------------------

CORPUS = load_corpus()


@pytest.mark.parametrize("index", range(len(CORPUS)))
def test_the_core_builds_the_world_python_builds_of_any_corpus_scenario(index: int) -> None:
    """`sheltersim_core::initial::world_entry` of a Scenario of the corpus (one per configuration
    the tests use, #429) is the bytes of `export_world(world)` of the world
    `WorldState.from_scenario` builds, and the digest recorded with the case."""
    import sheltersim_native

    from sheltersim.experiments.scenario import ScenarioSpec

    case = CORPUS[index]
    made = sheltersim_native.initial_world_entry(case["scenario"].encode())

    assert hashlib.sha256(made).hexdigest() == case["world_entry_sha256"]
    assert len(made) == case["world_entry_bytes"]
    assert made == world_entry_bytes(ScenarioSpec.model_validate_json(case["scenario"]))


def _rich_case() -> dict[str, Any]:
    """The first Scenario of the corpus with every extension these mutations change."""
    keys = {"body_dynamics", "staff_operations", "staff_dynamics", "shared_operations"}
    keys |= {"agent_dynamics", "geometry_overlay", "facility_geometry", "spatial_constraints"}
    return next(
        json.loads(case["scenario"])
        for case in CORPUS
        if keys <= set(json.loads(case["scenario"])["model_configs"])
    )


# Configs `ScenarioSpec` accepts and the world's construction refuses (#429).
REFUSED_CONFIGS: dict[str, Callable[[dict[str, Any]], Any]] = {
    "body_value": lambda configs: configs["body_dynamics"].update(affect_transfer_rate=2.0),
    "body_field": lambda configs: configs["body_dynamics"].update(unknown=1),
    "staff_operations_field": lambda configs: configs["staff_operations"].update(unknown=1),
    "staff_operations_duration": lambda configs: configs["staff_operations"][
        "task_duration_minutes"
    ].update(patrol=0),
    "staff_dynamics_rest": lambda configs: configs["staff_dynamics"].update(rest_minutes=0),
    "staff_dynamics_policy": lambda configs: configs["staff_dynamics"].update(policy_version="x"),
    "shared_operations": lambda configs: configs["shared_operations"].update(schema_version=2),
    "agent_dynamics_version": lambda configs: configs["agent_dynamics"].update(
        policy_version="unknown"
    ),
    "agent_dynamics_source": lambda configs: configs["agent_dynamics"].update(source_id="X"),
    "overlay_without_geometry": lambda configs: (
        configs.pop("facility_geometry"),
        configs.pop("spatial_constraints"),
    ),
}


@pytest.mark.parametrize("name", sorted(REFUSED_CONFIGS))
def test_the_core_raises_python_s_error_for_a_world_python_refuses(name: str) -> None:
    """Where `WorldState.from_scenario` raises for a Scenario `ScenarioSpec` accepts, the core
    raises the same class with the same text (#429)."""
    import sheltersim_native

    from sheltersim.core.world import WorldState
    from sheltersim.experiments.scenario import ScenarioSpec

    document = _rich_case()
    REFUSED_CONFIGS[name](document["model_configs"])
    scenario = ScenarioSpec.model_validate(document)

    with pytest.raises(Exception) as python:
        WorldState.from_scenario(scenario)
    with pytest.raises(Exception) as core:
        sheltersim_native.initial_world_entry(scenario.canonical_json().encode())

    assert type(core.value) is type(python.value)
    assert str(core.value) == str(python.value)


@pytest.mark.parametrize("name", sorted(TABLE_REQUESTS))
def test_the_core_builds_the_minute_zero_entry_python_exports(name: str) -> None:
    """`sheltersim_core::initial::entry` of a generated Scenario's canonical JSON is the bytes of
    `export_world` of its world at minute 0, set up for a Run (the screen's requests, #249)."""
    import sheltersim_native

    scenario = scenario_of(TABLE_REQUESTS[name])
    made = sheltersim_native.initial_entry(scenario.canonical_json().encode())

    assert made == entry_text(minute_zero_entry(scenario))


@pytest.mark.parametrize("name", sorted(TABLE_REQUESTS))
def test_the_core_records_a_new_run_s_model_versions_and_evidence_as_python(name: str) -> None:
    """`initial::model_versions` and `initial::evidence` of a generated Scenario are Python's
    `model_signatures` and the evidence route's answer for a new Run of it (#301)."""
    import sheltersim_native

    from sheltersim.contracts.policies import model_signatures

    scenario = scenario_of(TABLE_REQUESTS[name])
    raw = scenario.canonical_json().encode()

    versions = json.loads(sheltersim_native.initial_model_versions(raw))
    assert versions == model_signatures(scenario.model_configs)
    evidence = json.dumps(
        evidence_answer(scenario), sort_keys=True, ensure_ascii=False, separators=(",", ":")
    ).encode()
    assert sheltersim_native.initial_evidence(raw) == evidence


def test_the_evidence_template_is_the_server_s_answer(tmp_path: Any) -> None:
    """`core_initial.evidence_answer` is what the server's evidence route answers for a Run of
    the Scenario (#301)."""
    from sheltersim.storage.projects import ProjectStore
    from sheltersim.web.projects import ProjectAPI

    scenario = scenario_of({"evacuees": 3})
    store = ProjectStore(tmp_path / "projects")
    store.create_project("evidence")
    saved = store.save_scenario(
        "evidence",
        scenario,
        approved_hash=hashlib.sha256(scenario.canonical_json().encode()).hexdigest(),
    )
    store.create_run("evidence", "run-evidence", saved["scenario_id"], saved["scenario_version"])
    api = ProjectAPI(store, recover=False)
    try:
        answer = api.get(["evidence", "runs", "run-evidence", "evidence"], {})
    finally:
        api.close()
    assert json.loads(json.dumps(answer)) == evidence_answer(scenario)


def test_the_core_refuses_configs_its_table_lacks_and_what_is_not_a_scenario() -> None:
    """A fingerprint the table lacks is Unsupported; bytes that are no Scenario are a ValueError."""
    import sheltersim_native

    from sheltersim.native import Unsupported

    document = json.loads(scenario_of({"evacuees": 3}).canonical_json())
    document["model_configs"]["behavior"]["temperature"] = 0.3
    with pytest.raises(Unsupported, match="fingerprint"):
        sheltersim_native.initial_entry(json.dumps(document).encode())
    for raw in (b"", b"[]", b"{}", b'{"seed": "1"}', b"\xff"):
        with pytest.raises(ValueError):
            sheltersim_native.initial_entry(raw)


def test_the_fingerprint_keeps_only_that_the_data_configs_are_there() -> None:
    first = json.loads(scenario_of({"evacuees": 3}).canonical_json())["model_configs"]
    second = json.loads(scenario_of({"evacuees": 20, "gross_area_m2": 900.0}).canonical_json())
    second_configs = second["model_configs"]

    assert first["facility_geometry"] != second_configs["facility_geometry"]
    assert configs_fingerprint(first) == configs_fingerprint(second_configs)
    del second_configs["population_context"]
    assert configs_fingerprint(first) != configs_fingerprint(second_configs)
