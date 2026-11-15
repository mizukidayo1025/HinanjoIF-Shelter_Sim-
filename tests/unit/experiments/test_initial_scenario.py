"""The generator records which model implementations a new Scenario uses, default
drafts can start without hand-entered households or geometry, and the
parameter overrides a request selects reach the runtime config, provenance and the
Checkpoint signature.

These check the generator itself, so they build drafts directly. That a generated
Scenario's people survive frames and checkpoints is checked in
tests/unit/web/test_frames.py.
"""

from __future__ import annotations

import pytest

from sheltersim.experiments.initial_scenario import (
    InitialScenarioBuilder,
    InitialScenarioRequest,
)
from sheltersim.region.population import (
    OPERATOR_POPULATION_SOURCE_ID,
    SYNTHETIC_HOUSEHOLD_TEMPLATE_ID,
)


def _request(**policies: str) -> InitialScenarioRequest:
    return InitialScenarioRequest(
        municipality="Synthetic municipality",
        disaster="earthquake",
        evacuees=12,
        duration_days=1,
        seed=42,
        **policies,
    )


def test_generated_scenarios_select_the_default_policies() -> None:
    scenario = InitialScenarioBuilder().build(_request()).scenario

    context = scenario.model_configs["population_context"]
    assert len(context["people"]) == 12
    assert all(person["medication_support"] is None for person in context["people"].values())
    registry = scenario.model_configs["policy_registry"]
    assert registry["body_dynamics"] == "body-feedback-complete-v2"
    assert registry["staff_operations"] == "staff-urgency-first-v1"
    assert registry["staff_fatigue"] == "staff-fatigue-linear-v1"
    assert registry["zone_environment"] == "zone-temperature-comfort-band-v1"
    assert registry["zone_rest"] == "zone-rest-multiplicative-v1"
    assert registry["zone_hygiene"] == "zone-hygiene-linear-v1"
    assert registry["zone_privacy"] == "zone-privacy-threshold-v1"


def _override_request(**overrides: object) -> InitialScenarioRequest:
    return InitialScenarioRequest(
        municipality="Synthetic municipality",
        disaster="earthquake",
        evacuees=8,
        duration_days=1,
        seed=7,
        **overrides,
    )


config_override_values = {
    "behavior.temperature": 0.35,
    "body_dynamics.sleep_debt_stress_rate": 0.004,
    "facility_queues.retry_delay_minutes": 7,
    "social_propagation.stress_spread_per_minute": 0.003,
    "information_spread.base_transmission_probability": 0.35,
    "staff_dynamics.max_continuous_work_minutes": 300,
    "zone_environment.temperature_stress_per_degree_minute": 0.0003,
    "body_dynamics.hygiene_support_relief": 0.3,
}


def test_parameter_overrides_reject_unselected_coefficients_and_catalog_bound_violations() -> None:
    with pytest.raises(ValueError, match="finite JSON numbers"):
        _override_request(
            model_parameter_overrides={
                "body_dynamics.sleep_debt_stress_rate": True,
            }
        )

    with pytest.raises(ValueError, match="not consumed by a selected installed formula"):
        InitialScenarioBuilder().build(
            _override_request(
                model_parameter_overrides={
                    "body_dynamics.unregistered_stress_rate": 0.02,
                },
            )
        )

    with pytest.raises(ValueError, match="outside its Model Catalog bounds"):
        InitialScenarioBuilder().build(
            _override_request(
                model_parameter_overrides={
                    "body_dynamics.sleep_debt_stress_rate": 2.0,
                },
            )
        )

    with pytest.raises(ValueError, match="requires an integer value"):
        InitialScenarioBuilder().build(
            _override_request(
                model_parameter_overrides={
                    "staff_dynamics.max_continuous_work_minutes": 300.5,
                },
            )
        )


def test_per_zone_staff_task_thresholds_reject_unknown_zones_and_fields() -> None:
    builder = InitialScenarioBuilder()
    with pytest.raises(ValueError, match="unknown zone IDs"):
        builder.build(
            _override_request(
                model_parameter_overrides={
                    "staff_operations.thresholds_by_zone.*": {
                        "missing-zone": {"patrol_stress_at_least": 0.5},
                    },
                }
            )
        )
    with pytest.raises(ValueError, match="unsupported per-zone fields"):
        builder.build(
            _override_request(
                model_parameter_overrides={
                    "staff_operations.thresholds_by_zone.*": {
                        "sleeping": {"unknown_threshold": 0.5},
                    },
                }
            )
        )


def _draft(evacuees: int, seed: int, **extra):
    return InitialScenarioBuilder().build(
        InitialScenarioRequest(
            municipality="Synthetic municipality",
            disaster="earthquake",
            evacuees=evacuees,
            duration_days=1,
            seed=seed,
            **extra,
        )
    )


@pytest.mark.parametrize("evacuees", [10, 30, 100])
@pytest.mark.parametrize("seed", [0, 1, 2])
def test_default_drafts_have_no_approval_blockers(evacuees: int, seed: int) -> None:
    draft = _draft(evacuees, seed)
    assert draft.approval_blockers == ()
    assert draft.can_start


def test_default_templates_link_every_minor_and_are_recorded_as_assumptions() -> None:
    scenario = _draft(100, 7).scenario
    people = scenario.model_configs["population"]["people"]
    context = scenario.model_configs["population_context"]["people"]
    minors = [agent_id for agent_id, person in people.items() if person["age_years"] < 18]
    assert minors
    assert all(context[agent_id]["guardian_status"] == "accompanied" for agent_id in minors)
    assert all(context[agent_id]["parent_ids"] for agent_id in minors)
    assert SYNTHETIC_HOUSEHOLD_TEMPLATE_ID in scenario.model_configs["population"]["source_ids"]
    # Assumed templates are not operator input.
    assert OPERATOR_POPULATION_SOURCE_ID not in scenario.model_configs["population"]["source_ids"]
    source = next(
        row
        for row in scenario.provenance["sources"]
        if row["id"] == SYNTHETIC_HOUSEHOLD_TEMPLATE_ID
    )
    assert source["source_kind"] == "assumption"


def test_entered_household_shapes_replace_the_default_templates() -> None:
    unlinked = {
        "household_composition": {
            "templates": [
                {"template_id": f"unlinked-{size}", "members": [{"role": "unspecified"}] * size}
                for size in range(1, 6)
            ],
            "source_ids": ["SRC-TEST-UNLINKED-HOUSEHOLDS"],
        },
        "source_ids": ["SRC-TEST-UNLINKED-HOUSEHOLDS"],
    }
    draft = _draft(20, 42, population_conditions=unlinked)
    population_sources = draft.scenario.model_configs["population"]["source_ids"]
    assert SYNTHETIC_HOUSEHOLD_TEMPLATE_ID not in population_sources
    assert any("未成年者が" in row for row in draft.approval_blockers)


def test_adult_only_selection_drops_child_templates_instead_of_failing() -> None:
    draft = _draft(
        30,
        1,
        evacuation_selection={
            "age_band_multipliers": {"child": 0.0, "adult": 1.0, "older_adult": 1.0},
            "source_ids": ["SRC-TEST-ADULT-SELECTION"],
        },
    )
    people = draft.scenario.model_configs["population"]["people"]
    assert all(person["age_years"] >= 18 for person in people.values())
    assert draft.can_start


def test_the_core_builder_s_constants_are_python_s_today() -> None:
    """The core's builder holds what no screen request changes as constants (#299); they must be
    what Python makes now. Write them again with `native/core/tools/scenario_constants.py`."""
    from support.core_scenario_builder import constants_text
    from support.paths import FIXTURES

    path = FIXTURES.parent / "native" / "core" / "src" / "scenario" / "constants.json"
    assert path.read_text(encoding="utf-8") == constants_text(), (
        "native/core/src/scenario/constants.json is stale; "
        "run native/core/tools/scenario_constants.py"
    )
