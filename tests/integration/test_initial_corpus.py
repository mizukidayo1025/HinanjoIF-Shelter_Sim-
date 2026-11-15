"""The core's minute-0 builder (`sheltersim_core::initial`, #249, #429) on the corpus of Scenarios.

For each Scenario of `fixtures/initial_corpus.json.xz` (one per configuration the tests used), the
core builds the entry, the model versions and the evidence answer whose digests were recorded when
they were Python's bytes; and where the evidence route raised, the core raises the same error. The
mutations below are configs `ScenarioSpec` accepts and the building of a Run's world refuses: the
core raises the class and text Python raised (recorded at #430).
"""

from __future__ import annotations

import hashlib
import json
from collections.abc import Callable
from typing import Any

import pytest
import sheltersim_native
from support.initial_corpus import error_text, load_corpus

from sheltersim.experiments.scenario import ScenarioSpec

CORPUS = load_corpus()


@pytest.mark.parametrize("index", range(len(CORPUS)))
def test_the_core_builds_the_recorded_entry_of_any_corpus_scenario(index: int) -> None:
    case = CORPUS[index]
    made = sheltersim_native.initial_entry(case["scenario"].encode())

    assert hashlib.sha256(made).hexdigest() == case["entry_sha256"]
    assert len(made) == case["entry_bytes"]


@pytest.mark.parametrize("index", range(len(CORPUS)))
def test_the_core_records_a_new_run_of_any_corpus_scenario_as_recorded(index: int) -> None:
    case = CORPUS[index]
    raw = case["scenario"].encode()
    versions = sheltersim_native.initial_model_versions(raw).encode()

    assert hashlib.sha256(versions).hexdigest() == case["model_versions_sha256"]
    if "evidence_error" in case:
        with pytest.raises(Exception) as core:
            sheltersim_native.initial_evidence(raw)
        assert error_text(core.value) == case["evidence_error"]
        return
    evidence = sheltersim_native.initial_evidence(raw)
    assert hashlib.sha256(evidence).hexdigest() == case["evidence_sha256"]


def _rich_case() -> dict[str, Any]:
    """The first Scenario of the corpus with every extension these mutations change."""
    keys = {"body_dynamics", "staff_operations", "staff_dynamics", "shared_operations"}
    keys |= {"agent_dynamics", "geometry_overlay", "facility_geometry", "spatial_constraints"}
    return next(
        json.loads(case["scenario"])
        for case in CORPUS
        if keys <= set(json.loads(case["scenario"])["model_configs"])
    )


# Each mutation and the class and text of the error Python raised building the world.
REFUSED_CONFIGS: dict[str, tuple[Callable[[dict[str, Any]], Any], str, str]] = {
    "body_value": (
        lambda configs: configs["body_dynamics"].update(affect_transfer_rate=2.0),
        "ValueError",
        "body_dynamics affect_transfer_rate must be finite and within 0..1",
    ),
    "body_field": (
        lambda configs: configs["body_dynamics"].update(unknown=1),
        "ValueError",
        "body_dynamics config contains unsupported fields",
    ),
    "staff_operations_field": (
        lambda configs: configs["staff_operations"].update(unknown=1),
        "ValueError",
        "staff_operations config has unsupported fields",
    ),
    "staff_operations_duration": (
        lambda configs: configs["staff_operations"]["task_duration_minutes"].update(patrol=0),
        "ValueError",
        "staff_operations duration for patrol is outside 1..1440 minutes",
    ),
    "staff_dynamics_rest": (
        lambda configs: configs["staff_dynamics"].update(rest_minutes=0),
        "ValueError",
        "staff_dynamics rest_minutes is outside its supported range",
    ),
    "staff_dynamics_policy": (
        lambda configs: configs["staff_dynamics"].update(policy_version="x"),
        "ValueError",
        "unsupported staff_dynamics policy or source",
    ),
    "shared_operations": (
        lambda configs: configs["shared_operations"].update(schema_version=2),
        "ValueError",
        "invalid shared_operations config",
    ),
    "agent_dynamics_version": (
        lambda configs: configs["agent_dynamics"].update(policy_version="unknown"),
        "ValueError",
        "agent_dynamics.create_state refused: a config the Python rule refuses",
    ),
    "agent_dynamics_source": (
        lambda configs: configs["agent_dynamics"].update(source_id="X"),
        "ValueError",
        "agent_dynamics.create_state refused: a config the Python rule refuses",
    ),
    "overlay_without_geometry": (
        lambda configs: (configs.pop("facility_geometry"), configs.pop("spatial_constraints")),
        "ValueError",
        "geometry_overlay requires facility_geometry",
    ),
    "social_unknown": (
        lambda configs: configs["social_propagation"].update(unknown=1),
        "ValueError",
        "unknown social_propagation settings: ['unknown']",
    ),
    "social_rate": (
        lambda configs: configs["social_propagation"].update(stress_spread_per_minute=0.5),
        "ValueError",
        "stress_spread_per_minute must be finite and between 0 and 0.25",
    ),
    "social_cooldown": (
        lambda configs: configs["social_propagation"].update(cooldown_minutes=0),
        "ValueError",
        "cooldown_minutes must be between 1 and 10080",
    ),
    "social_source": (
        lambda configs: configs["social_propagation"].update(source_id="ASM-DEGROOT-CONTACT-V1"),
        "ValueError",
        "social_propagation source_id must match the selected social model",
    ),
    "social_source_type": (
        lambda configs: configs["social_propagation"].update(source_type="x"),
        "ValueError",
        "social_propagation source_type must be assumption",
    ),
}


@pytest.mark.parametrize("name", sorted(REFUSED_CONFIGS))
def test_the_core_refuses_a_world_python_refused_with_its_text(name: str) -> None:
    change, kind, text = REFUSED_CONFIGS[name]
    document = _rich_case()
    change(document["model_configs"])
    scenario = ScenarioSpec.model_validate(document)

    with pytest.raises(Exception) as core:
        sheltersim_native.initial_entry(scenario.canonical_json().encode())

    assert (type(core.value).__name__, str(core.value)) == (kind, text)


def test_what_is_not_a_scenario_is_a_value_error() -> None:
    for raw in (b"", b"[]", b"{}", b'{"seed": "1"}', b"\xff"):
        with pytest.raises(ValueError):
            sheltersim_native.initial_entry(raw)
