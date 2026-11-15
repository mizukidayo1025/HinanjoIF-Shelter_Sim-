"""Installed model metadata shared by scenario validation and runtime factories."""

from __future__ import annotations

import json
import math
from contextvars import ContextVar
from copy import deepcopy
from importlib.resources import files
from typing import Any, Literal

from pydantic import ConfigDict, Field, model_validator

from sheltersim.contracts.extensions import ExtensionModel, Identifier
from sheltersim.domain.psychological_state import (
    INTEGRATED_PSYCHOLOGY_POLICY_VERSION,
    LIFE_STATE_PSYCHOLOGY_POLICIES,
)

# Literature motivates operators, not calibrated shelter coefficients.
MODEL_DEFINITIONS: dict[str, dict[str, Any]] = json.loads(
    files("sheltersim.contracts").joinpath("model-catalog.json").read_text(encoding="utf-8")
)["models"]
DEFAULT_MODELS = {
    "behavior": "utility-softmax-v1",
    "psychology": "causal-stress-v1",
    "social": "sparse-family-zone-contact-v1",
    "information": "sparse-information-contact-v1",
}

# Historical Scenarios without a suite selector keep DEFAULT_MODELS so a saved
# Scenario can still be replayed as authored. New Scenario entry points opt into
# this coherent, internally tested candidate set explicitly.
CORE_MODEL_SUITE_V1 = "core-model-suite-v1"
CORE_MODEL_SUITE_V2 = "core-model-suite-v2"
CORE_MODEL_SUITE_MODELS_V1: dict[str, str] = {
    "behavior": "utility-softmax-v1",
    "psychology": "mean-reverting-life-state-v1",
    "social": "expressiveness-channel-openness-v1",
    "information": "evidence-aware-rumor-v1",
    "affect": "linear-positive-negative-v1",
    "body_dynamics": "body-feedback-complete-v2",
    "queue": "need-weighted-physical-queue-v1",
    "relationship": "distance-duration-repeat-trust-v1",
    "social_proximity": "disabled-proximity-v1",
}
CORE_MODEL_SUITE_MODELS: dict[str, str] = {
    "behavior": "sleep-aware-utility-softmax-v2",
    "psychology": "mean-reverting-life-state-v1",
    "social": "expressiveness-channel-openness-v1",
    "information": "evidence-aware-rumor-v1",
    "affect": "linear-positive-negative-v1",
    "body_dynamics": "body-feedback-complete-v2",
    "queue": "need-weighted-physical-queue-v1",
    "relationship": "distance-duration-repeat-trust-v1",
    "social_proximity": "disabled-proximity-v1",
}
CORE_MODEL_SUITES = {
    CORE_MODEL_SUITE_V1: CORE_MODEL_SUITE_MODELS_V1,
    CORE_MODEL_SUITE_V2: CORE_MODEL_SUITE_MODELS,
}

# Pre-research psychology models stay installed so saved Runs and Checkpoints
# replay exactly and remain usable as comparison controls. The mark lives here,
# not in model-catalog.json, because catalog entries are part of policy signatures.
LEGACY_PSYCHOLOGY_MODELS = frozenset({"causal-stress-v1", "life-state-feedback-v1"})


def legacy_psychology_notice(model_id: object) -> str | None:
    """Explain when a Scenario or Run uses a pre-research psychology model."""
    if model_id not in LEGACY_PSYCHOLOGY_MODELS:
        return None
    return (
        f"psychology model {model_id} is a legacy model kept for replay and comparison; "
        f"new generated Scenarios use {INTEGRATED_PSYCHOLOGY_POLICY_VERSION}"
    )


class ModelCoefficientDefinition(ExtensionModel):
    """Versioned metadata for a consumed coefficient; source is not validation."""

    model_config = ConfigDict(extra="allow", frozen=True)
    unit: str = Field(min_length=1, max_length=80)
    default: Any = None
    lower: Any = None
    upper: Any = None
    source_id: Identifier | None = None
    evidence_grade: str = Field(default="C", min_length=1, max_length=16)
    applicability: str = Field(
        default="Uncalibrated installed model assumption.", min_length=1, max_length=500
    )
    lower_exclusive: bool = False
    scope: (
        Literal[
            "per_staff_id",
            "per_staff_shift_windows",
            "per_service",
            "per_zone_id",
            "per_agent_id",
            "per_task_kind",
        ]
        | None
    ) = None

    @model_validator(mode="after")
    def ordered_numeric_bounds(self) -> ModelCoefficientDefinition:
        for value in (self.default, self.lower, self.upper):
            if type(value) in (int, float) and not math.isfinite(value):
                raise ValueError("model coefficient metadata must be finite")
        if (
            type(self.lower) in (int, float)
            and type(self.upper) in (int, float)
            and self.lower > self.upper
        ):
            raise ValueError("model coefficient lower bound exceeds upper bound")
        if type(self.default) in (int, float):
            if type(self.lower) in (int, float) and (
                self.default < self.lower or self.lower_exclusive and self.default == self.lower
            ):
                raise ValueError("model coefficient default is below its declared lower bound")
            if type(self.upper) in (int, float) and self.default > self.upper:
                raise ValueError("model coefficient default exceeds its declared upper bound")
        return self


class PolicyModelDefinition(ExtensionModel):
    """Registry metadata only. Executability still requires a code-owned factory."""

    model_config = ConfigDict(extra="allow", frozen=True)
    domain: Identifier
    reference: str = Field(min_length=1, max_length=500)
    formula: str = Field(min_length=1, max_length=2000)
    assumption_source_id: Identifier
    implementation_version: Identifier = "1"
    state_schema_version: int = Field(default=1, strict=True, ge=1)
    external_validation: bool = False
    parameters: dict[str, ModelCoefficientDefinition] = Field(default_factory=dict)
    required_state_contracts: dict[str, dict[str, Any]] = Field(default_factory=dict)

    @model_validator(mode="after")
    def parameter_paths_are_explicit(self) -> PolicyModelDefinition:
        if any("." not in path or len(path) > 128 for path in self.parameters):
            raise ValueError("policy parameters require explicit namespaced config paths")
        if self.external_validation:
            raise ValueError("an installed registry entry cannot certify empirical validity")
        return self


# Each person's social event and each proposal asks for the selection several times per
# tick. Inside a scope (one tick phase, in which no model_configs changes), the selection
# of a configs object is computed once and shared, so callers must not change it.
_SELECTION_SCOPE: ContextVar[list[tuple[Any, dict[str, str]]] | None] = ContextVar(
    "selected_models_scope", default=None
)


def selected_models(configs: dict[str, dict[str, Any]]) -> dict[str, str]:
    """Opt-in selectors are separate from legacy coefficient schema versions."""
    scope = _SELECTION_SCOPE.get()
    if scope is None:
        return _select_models(configs)
    for known, selection in scope:
        if known is configs:
            return selection
    selection = _select_models(configs)
    scope.append((configs, selection))
    return selection


def _select_models(configs: dict[str, dict[str, Any]]) -> dict[str, str]:
    from sheltersim.contracts.bootstrap import ensure_builtin_contracts

    ensure_builtin_contracts()
    if not isinstance(configs, dict) or any(
        not isinstance(value, dict) for value in configs.values()
    ):
        raise ValueError("model configs must map names to configuration objects")
    behavior = configs.get("behavior", {})
    if set(behavior) - {
        "mode",
        "source_type",
        "source_id",
        "policy",
        "temperature",
        "decision_interval_minutes",
        "decision_interval_minutes_by_agent",
        "temperature_by_agent",
        "sleep_debt_utility_weight",
    }:
        raise ValueError("unknown behavior coefficient; explicit registry support required")
    interval = behavior.get("decision_interval_minutes", 15)
    if type(interval) is not int or interval <= 0:
        raise ValueError("behavior decision interval must be a positive integer")
    temperature = behavior.get("temperature", 0.2)
    if type(temperature) not in (int, float) or not math.isfinite(temperature) or temperature <= 0:
        raise ValueError("behavior temperature must be finite and positive")
    selection = configs.get("policy_registry", {})
    domains = {value["domain"] for value in MODEL_DEFINITIONS.values()}
    if set(selection) - {"schema_version", "suite_id", *domains}:
        raise ValueError("unknown policy_registry setting")
    if selection and (
        type(selection.get("schema_version")) is not int
        or selection["schema_version"] not in (1, 2, 3)
    ):
        raise ValueError("unsupported policy_registry schema")
    suite_id = selection.get("suite_id")
    suite_models = None
    if suite_id is not None:
        suite_models = CORE_MODEL_SUITES.get(suite_id)
        if selection.get("schema_version") != 3 or suite_models is None:
            raise ValueError("unsupported or incorrectly versioned model suite")
        conflicting = {
            domain: selection[domain]
            for domain, model_id in suite_models.items()
            if domain in selection and selection[domain] != model_id
        }
        if conflicting:
            raise ValueError("model suite selectors cannot be mixed with per-domain overrides")
        if behavior.get("policy", "utility-softmax-v1") != suite_models["behavior"]:
            raise ValueError("behavior policy does not match the selected model suite")
    models = {
        **DEFAULT_MODELS,
        **(suite_models or {}),
        "behavior": configs.get("behavior", {}).get("policy", "utility-softmax-v1"),
    }
    for domain in sorted({*models, *(set(selection) - {"schema_version", "suite_id"})}):
        if domain in selection:
            models[domain] = selection[domain]
        model_id = models[domain]
        if (
            not isinstance(model_id, str)
            or model_id not in MODEL_DEFINITIONS
            or MODEL_DEFINITIONS[model_id]["domain"] != domain
        ):
            raise ValueError(f"unsupported installed {domain} model: {model_id!r}")
    if models["behavior"] != configs.get("behavior", {}).get("policy", "utility-softmax-v1"):
        raise ValueError("behavior selector must agree with model_configs.behavior.policy")
    behavior_parameters = MODEL_DEFINITIONS[models["behavior"]]["parameters"]
    if "sleep_debt_utility_weight" in behavior:
        metadata = behavior_parameters.get("behavior.sleep_debt_utility_weight")
        value = behavior["sleep_debt_utility_weight"]
        if metadata is None:
            raise ValueError(
                "behavior.sleep_debt_utility_weight is not supported by the selected behavior model"
            )
        if type(value) not in (int, float) or not math.isfinite(value):
            raise ValueError("behavior.sleep_debt_utility_weight must be finite")
        if (
            metadata.get("lower") is not None
            and value < metadata["lower"]
            or metadata.get("upper") is not None
            and value > metadata["upper"]
        ):
            raise ValueError(
                "behavior.sleep_debt_utility_weight is outside its Model Catalog bounds"
            )
    for field, expected_unit in (
        ("decision_interval_minutes_by_agent", "minute"),
        ("temperature_by_agent", "utility_temperature"),
    ):
        values = behavior.get(field, {})
        if not isinstance(values, dict) or len(values) > 100:
            raise ValueError(f"behavior.{field} must be a bounded Agent ID map")
        if values and f"behavior.{field}.*" not in behavior_parameters:
            raise ValueError(f"behavior.{field} is not supported by selected {models['behavior']}")
        metadata = behavior_parameters.get(f"behavior.{field}.*")
        for agent_id, value in values.items():
            if not isinstance(agent_id, str) or not agent_id or len(agent_id) > 64:
                raise ValueError(f"behavior.{field} contains an invalid Agent ID")
            if type(value) not in (int, float) or not math.isfinite(value):
                raise ValueError(f"behavior.{field} values must be finite numbers")
            if expected_unit == "minute" and (type(value) is not int or value < 1):
                raise ValueError("behavior decision intervals must be positive integer minutes")
            if metadata is None:
                raise ValueError(
                    f"behavior.{field} is not supported by selected {models['behavior']}"
                )
            lower, upper = metadata.get("lower"), metadata.get("upper")
            if (
                lower is not None
                and (value < lower or metadata.get("lower_exclusive") and value == lower)
                or upper is not None
                and value > upper
            ):
                raise ValueError(f"behavior.{field} value is outside its Model Catalog bounds")
    if "affect" in models and models["psychology"] not in LIFE_STATE_PSYCHOLOGY_POLICIES:
        raise ValueError("affect model requires the installed life-state psychology operator")
    if "body_dynamics" in models:
        if (
            models["psychology"] not in LIFE_STATE_PSYCHOLOGY_POLICIES
            or "body_dynamics" not in configs
        ):
            raise ValueError(
                "body_dynamics formula selection requires body_dynamics config and "
                "life-state psychology"
            )
    if "staff_operations" in models and "staff_operations" not in configs:
        raise ValueError("staff_operations formula selection requires staff_operations config")
    if "staff_fatigue" in models and "staff_dynamics" not in configs:
        raise ValueError("staff_fatigue formula selection requires staff_dynamics config")
    if "care_outcome" in models and "body_dynamics" not in configs:
        raise ValueError("care outcome selection requires body_dynamics config")
    environment_formula_domains = {
        "zone_environment",
        "zone_rest",
        "zone_hygiene",
        "zone_privacy",
    }
    if environment_formula_domains.intersection(models):
        if "zone_environment" not in configs:
            raise ValueError("environment formula selection requires zone_environment config")
        from sheltersim.domain.environment import ScenarioZoneEnvironmentConfig

        zone_config = ScenarioZoneEnvironmentConfig.model_validate(configs["zone_environment"])
        if zone_config.policy_version != "zone-environment-v2":
            raise ValueError("environment formula selection requires zone-environment-v2")
    formula_factory_domains = {
        "body_dynamics",
        "staff_operations",
        "staff_fatigue",
        "zone_environment",
        "zone_rest",
        "zone_hygiene",
        "zone_privacy",
        "care_outcome",
    }
    if formula_factory_domains.intersection(models) and selection.get("schema_version") not in (
        2,
        3,
    ):
        raise ValueError(
            "replaceable formula factories require policy_registry schema_version 2 or 3"
        )
    from sheltersim.policies.registry import has_installed_factory

    if any(not has_installed_factory(domain, model_id) for domain, model_id in models.items()):
        raise ValueError("selected model metadata has no installed Policy factory")
    proximity_model = models.get("social_proximity")
    if proximity_model is not None:
        from sheltersim.domain.social_network import canonical_proximity_contact_config

        proximity_config = canonical_proximity_contact_config(configs.get("social_proximity"))
        if proximity_model == "distance-sampled-proximity-v1" and proximity_config is None:
            raise ValueError(
                "distance-sampled-proximity-v1 requires an enabled social_proximity config"
            )
        if proximity_model == "disabled-proximity-v1" and proximity_config is not None:
            raise ValueError(
                "disabled-proximity-v1 conflicts with an enabled social_proximity config"
            )
    installed = None
    for model_id in models.values():
        required = MODEL_DEFINITIONS[model_id].get("required_state_contracts", {})
        if not isinstance(required, dict):
            raise ValueError("invalid installed model state requirements")
        if required:
            from sheltersim.contracts.runtime import state_catalog

            installed = installed or {row["namespace"]: row for row in state_catalog()}
            declared = configs.get("state_contracts", {}).get("namespaces", {})
            for namespace, signature in required.items():
                descriptor = installed.get(namespace)
                actual = (
                    {
                        key: descriptor[key]
                        for key in ("implementation_id", "implementation_version", "schema_version")
                    }
                    if descriptor
                    else None
                )
                if declared.get(namespace) != signature or actual != signature:
                    raise ValueError(
                        "selected model requires an installed, explicitly versioned state "
                        "contract; migration required"
                    )
    return models


def model_catalog() -> dict[str, dict[str, Any]]:
    from sheltersim.contracts.bootstrap import ensure_builtin_contracts

    ensure_builtin_contracts()
    catalog: dict[str, dict[str, Any]] = {}
    for key, value in sorted(MODEL_DEFINITIONS.items()):
        raw = {
            "implementation_version": "1",
            "state_schema_version": 1,
            "external_validation": False,
            **deepcopy(value),
        }
        checked = PolicyModelDefinition.model_validate(raw).model_dump(
            mode="json", exclude_none=True
        )
        for coefficient in checked["parameters"].values():
            if coefficient.get("source_id") is None:
                coefficient["source_id"] = checked["assumption_source_id"]
            coefficient["evidence_grade"] = coefficient.get("evidence_grade") or "C"
            coefficient["applicability"] = (
                coefficient.get("applicability") or "Uncalibrated installed model assumption."
            )
        catalog[key] = {
            "evidence_grade": "C",
            "coefficient_status": "synthetic assumptions",
            **checked,
        }
    return catalog
