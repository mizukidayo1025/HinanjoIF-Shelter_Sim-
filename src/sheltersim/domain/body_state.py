"""Versioned long-term body and affect state for the registered life-state policy."""

from __future__ import annotations

import math
import re
from typing import Any

from sheltersim.policies.affect import create_affect_policy

BODY_STATE_KEY = "body_state_v1"
BODY_POLICY_VERSION = "life-state-feedback-v1"
BODY_SOURCE_ID = "ASM-LIFE-STATE-FEEDBACK-V1"

_DEFAULTS: dict[str, float] = {
    "awake_sleep_debt_minutes_per_minute": 1.0,
    "rest_sleep_debt_recovery_per_minute": 0.25,
    "sleep_sleep_debt_recovery_per_minute": 1.0,
    "sleep_debt_stress_rate": 0.001,
    "uncertainty_drift_per_minute": 0.00002,
    "uncertainty_decay_per_minute": 0.00005,
    "discontent_stress_rate": 0.001,
    "discontent_queue_rate": 0.0005,
    "discontent_relief_per_minute": 0.0002,
    "reassurance_decay_per_minute": 0.0003,
    "positive_affect_decay_per_minute": 0.0001,
    "negative_affect_stress_rate": 0.001,
    "hygiene_growth_per_minute": 0.00005,
    "sanitation_hygiene_growth_per_minute": 0.0001,
    "sanitation_stress_rate": 0.002,
    "affect_transfer_rate": 0.05,
    "relationship_trust_gain": 0.0005,
    "relationship_trust_decay_per_day": 0.002,
    "relationship_repeat_contact_bonus": 0.05,
    "cooperation_positive_gain": 0.03,
    "cooperation_negative_reduction": 0.02,
    "cooperation_reassurance_gain": 0.03,
    "cooperation_discontent_reduction": 0.01,
    "social_action_affect_transfer": 0.02,
    "complaint_reassurance_gain": 0.02,
    "complaint_discontent_reduction": 0.01,
    "hygiene_support_relief": 0.15,
    "caregiver_assistance_stress_relief": 0.03,
    "caregiver_assistance_reassurance_gain": 0.02,
    "privacy_accommodation_discontent_relief": 0.02,
    "mobility_assistance_stress_relief": 0.03,
    "language_access_uncertainty_relief": 0.05,
    "medical_assessment_uncertainty_relief": 0.02,
    "medication_support_uncertainty_relief": 0.01,
    "dependent_assistance_stress_relief": 0.04,
    "dependent_assistance_reassurance_gain": 0.03,
    "correction_uncertainty_reduction": 0.05,
    "correction_reassurance_gain": 0.01,
}


def canonical_body_config(raw: Any) -> dict[str, Any]:
    """Validate explicit units/ranges; arbitrary policy expressions are never accepted."""
    if raw is None:
        raw = {}
    if not isinstance(raw, dict):
        raise ValueError("body_dynamics config must be an object")
    allowed = {
        "schema_version",
        "policy_version",
        "source_id",
        *_DEFAULTS,
        "social_interaction_minutes",
        "affect_policy",
        "relationship_distance_scale_m",
        "sleep_debt_stress_rate_by_agent",
        "negative_affect_stress_rate_by_agent",
        "discontent_stress_rate_by_agent",
        "complaint_reassurance_gain_by_agent",
        "hygiene_support_relief_by_agent",
        "caregiver_assistance_stress_relief_by_agent",
        "caregiver_assistance_reassurance_gain_by_agent",
        "privacy_accommodation_discontent_relief_by_agent",
        "mobility_assistance_stress_relief_by_agent",
        "language_access_uncertainty_relief_by_agent",
        "medical_assessment_uncertainty_relief_by_agent",
        "medication_support_uncertainty_relief_by_agent",
        "dependent_assistance_stress_relief_by_agent",
        "dependent_assistance_reassurance_gain_by_agent",
    }
    if set(raw) - allowed:
        raise ValueError("body_dynamics config contains unsupported fields")
    config = {
        "schema_version": 1,
        "policy_version": BODY_POLICY_VERSION,
        "source_id": BODY_SOURCE_ID,
        **_DEFAULTS,
        "social_interaction_minutes": 5,
        "affect_policy": "linear-positive-negative-v1",
        "relationship_distance_scale_m": 2.0,
        **raw,
    }
    if (
        config["schema_version"] != 1
        or config["policy_version"] != BODY_POLICY_VERSION
        or config["source_id"] != BODY_SOURCE_ID
    ):
        raise ValueError("unsupported body_dynamics policy or source")
    for name in _DEFAULTS:
        value = config[name]
        if (
            not isinstance(value, (int, float))
            or isinstance(value, bool)
            or not math.isfinite(value)
            or not 0.0 <= value <= 1.0
        ):
            raise ValueError(f"body_dynamics {name} must be finite and within 0..1")
    if (
        type(config["social_interaction_minutes"]) is not int
        or not 1 <= config["social_interaction_minutes"] <= 1440
    ):
        raise ValueError("body_dynamics social_interaction_minutes must be within 1..1440")
    if not isinstance(config["affect_policy"], str):
        raise ValueError("body_dynamics affect_policy must be an installed model ID")
    create_affect_policy(config["affect_policy"])
    distance_scale = config["relationship_distance_scale_m"]
    if (
        not isinstance(distance_scale, (int, float))
        or isinstance(distance_scale, bool)
        or not math.isfinite(distance_scale)
        or not 0.1 <= distance_scale <= 100.0
    ):
        raise ValueError(
            "body_dynamics relationship_distance_scale_m must be within 0.1..100 metres"
        )
    if config["awake_sleep_debt_minutes_per_minute"] > 10.0:
        raise ValueError("awake sleep debt rate cannot exceed 10 minutes per minute")
    if (
        config["rest_sleep_debt_recovery_per_minute"] > 10.0
        or config["sleep_sleep_debt_recovery_per_minute"] > 10.0
    ):
        raise ValueError("sleep debt recovery cannot exceed 10 minutes per minute")
    for field, label in (
        ("sleep_debt_stress_rate_by_agent", "sleep debt rates"),
        ("negative_affect_stress_rate_by_agent", "negative-affect stress rates"),
        ("discontent_stress_rate_by_agent", "discontent stress rates"),
        ("complaint_reassurance_gain_by_agent", "reassurance gains"),
        ("hygiene_support_relief_by_agent", "hygiene support relief"),
        ("caregiver_assistance_stress_relief_by_agent", "caregiver stress relief"),
        ("caregiver_assistance_reassurance_gain_by_agent", "caregiver reassurance gains"),
        ("privacy_accommodation_discontent_relief_by_agent", "privacy discontent relief"),
        ("mobility_assistance_stress_relief_by_agent", "mobility stress relief"),
        ("language_access_uncertainty_relief_by_agent", "language-access uncertainty relief"),
        ("medical_assessment_uncertainty_relief_by_agent", "medical-support uncertainty relief"),
        ("medication_support_uncertainty_relief_by_agent", "medication-support uncertainty relief"),
        ("dependent_assistance_stress_relief_by_agent", "dependent-assistance stress relief"),
        (
            "dependent_assistance_reassurance_gain_by_agent",
            "dependent-assistance reassurance gains",
        ),
    ):
        per_agent = config.get(field)
        if per_agent is None:
            continue
        if not isinstance(per_agent, dict) or not per_agent:
            raise ValueError(f"body_dynamics per-Agent {label} must be a non-empty ID map")
        for agent_id, rate in per_agent.items():
            if (
                not isinstance(agent_id, str)
                or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._:-]{0,63}", agent_id)
                or type(rate) not in (int, float)
                or not math.isfinite(rate)
                or not 0.0 <= rate <= 1.0
            ):
                raise ValueError(f"body_dynamics per-Agent {label} are invalid")
    return config
