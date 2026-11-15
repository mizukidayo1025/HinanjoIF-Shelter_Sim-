"""Generated Scenarios select the research psychology model; legacy models stay replayable.

Installed formula factories change their operator and sign the Checkpoint, and the core
model suite is pinned for new configs without rewriting historical ones. A suite runs
the models it declares even where it leaves a per-domain selector implicit (#169).

Running a generated Scenario is checked in tests/integration/test_research_psychology.py,
and the CLI warning for a legacy model in tests/e2e/test_legacy_psychology_cli.py.
"""

from __future__ import annotations

import json

import pytest
from support.paths import SOURCE_MANIFEST

from sheltersim.contracts.policies import (
    CORE_MODEL_SUITE_MODELS,
    CORE_MODEL_SUITE_MODELS_V1,
    CORE_MODEL_SUITE_V1,
    CORE_MODEL_SUITE_V2,
    LEGACY_PSYCHOLOGY_MODELS,
    legacy_psychology_notice,
    selected_models,
)
from sheltersim.domain.psychological_state import INTEGRATED_PSYCHOLOGY_POLICY_VERSION
from sheltersim.experiments.model_suite import install_default_model_suite
from sheltersim.policies.psychology import (
    IntegratedDistressLifeStatePolicy,
)


def test_generated_psychology_selection_is_recorded_in_provenance_and_manifest(
    generated_scenario,
) -> None:
    scenario = generated_scenario
    source_id = IntegratedDistressLifeStatePolicy.source_id
    assert any(row["id"] == source_id for row in scenario.provenance["sources"])
    assumption = next(
        row for row in scenario.provenance["assumptions"] if row["subject"] == "psychology_model"
    )
    assert assumption["source_id"] == source_id
    assert assumption["value"] == {"policy_version": INTEGRATED_PSYCHOLOGY_POLICY_VERSION}
    manifest = json.loads(SOURCE_MANIFEST.read_text("utf-8"))
    assert any(row["id"] == source_id for row in manifest["assumptions"])


@pytest.mark.parametrize("model_id", sorted(LEGACY_PSYCHOLOGY_MODELS))
def test_legacy_psychology_models_have_a_notice(model_id: str) -> None:
    notice = legacy_psychology_notice(model_id)
    assert notice is not None
    assert model_id in notice
    assert INTEGRATED_PSYCHOLOGY_POLICY_VERSION in notice


@pytest.mark.parametrize(
    "model_id", [INTEGRATED_PSYCHOLOGY_POLICY_VERSION, "mean-reverting-distress-v1", None]
)
def test_research_or_unknown_models_have_no_notice(model_id) -> None:
    assert legacy_psychology_notice(model_id) is None


def test_unknown_operator_cannot_be_imported():
    with pytest.raises(ValueError, match="unsupported installed"):
        selected_models({"policy_registry": {"schema_version": 1, "psychology": "os.system"}})




def test_new_unselected_configs_receive_one_pinned_core_suite():
    configs = install_default_model_suite({"behavior": {"policy": "utility-softmax-v1"}})

    assert configs["policy_registry"] == {
        "schema_version": 3,
        "suite_id": CORE_MODEL_SUITE_V2,
    }
    assert selected_models(configs) == CORE_MODEL_SUITE_MODELS
    assert configs["body_dynamics"]["awake_sleep_debt_minutes_per_minute"] == 0.5
    assert configs["body_dynamics"]["rest_sleep_debt_recovery_per_minute"] == 0.0
    assert configs["social_propagation"]["enabled"] is True
    assert configs["information_spread"]["enabled"] is True
    assert "social_proximity" not in configs
    assert selected_models(configs)["social_proximity"] == "disabled-proximity-v1"


def test_explicit_historical_registry_is_not_rewritten_by_new_defaults():
    configs = {
        "behavior": {"policy": "utility-softmax-v1"},
        "policy_registry": {
            "schema_version": 1,
            "psychology": "causal-stress-v1",
        },
    }

    assert install_default_model_suite(configs) == configs


def test_suite_v1_selector_keeps_its_original_model_map():
    configs = install_default_model_suite({"behavior": {"policy": "utility-softmax-v1"}})
    configs["behavior"]["policy"] = "utility-softmax-v1"
    configs["behavior"]["source_id"] = "ASM-UTILITY-BEHAVIOR-V1"
    configs["behavior"].pop("sleep_debt_utility_weight")
    configs["policy_registry"]["suite_id"] = CORE_MODEL_SUITE_V1

    assert selected_models(configs) == CORE_MODEL_SUITE_MODELS_V1


