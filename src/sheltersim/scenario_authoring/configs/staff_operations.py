"""Autonomous, routed staff assignments with bounded causal task histories.

The rules of the tick are native/src/rules/staff.rs's and staff_operation_events.rs's kernels
(`kernel_only`): a value they refuse is KernelRefusal, before anything changes. The checks of a
config, of a saved namespace and of an operator's order stay here.
"""

from __future__ import annotations

import math
from typing import Any

STAFF_OPERATIONS_STATE_KEY = "staff_operations_v1"
STAFF_OPERATIONS_POLICY_VERSION = "autonomous-routed-staff-v1"
STAFF_OPERATIONS_SOURCE_ID = "ASM-STAFF-OPERATIONS-V1"

_DURATION_DEFAULTS = {
    "cleaning": 15,
    "maintenance": 20,
    "information_outreach": 10,
    "intake": 5,
    "patrol": 15,
}


def canonical_staff_operations_config(raw: Any) -> dict[str, Any]:
    if raw is None:
        raw = {}
    if not isinstance(raw, dict) or set(raw) - {
        "schema_version",
        "policy_version",
        "source_id",
        "task_duration_minutes",
        "sanitation_use_per_person_minute",
        "cleaning_sanitation_recovery",
        "outreach_uncertainty_relief",
        "patrol_reassurance",
        "thresholds_by_zone",
        "effects_by_zone",
    }:
        raise ValueError("staff_operations config has unsupported fields")
    config = {
        "schema_version": 1,
        "policy_version": STAFF_OPERATIONS_POLICY_VERSION,
        "source_id": STAFF_OPERATIONS_SOURCE_ID,
        "task_duration_minutes": dict(_DURATION_DEFAULTS),
        "sanitation_use_per_person_minute": 0.000005,
        "cleaning_sanitation_recovery": 0.15,
        "outreach_uncertainty_relief": 0.04,
        "patrol_reassurance": 0.01,
        "thresholds_by_zone": {},
        "effects_by_zone": {},
        **raw,
    }
    if (
        config["schema_version"] != 1
        or config["policy_version"] != STAFF_OPERATIONS_POLICY_VERSION
        or config["source_id"] != STAFF_OPERATIONS_SOURCE_ID
    ):
        raise ValueError("unsupported staff_operations policy or source")
    durations = config["task_duration_minutes"]
    if not isinstance(durations, dict) or set(durations) != set(_DURATION_DEFAULTS):
        raise ValueError("staff_operations task durations must define every registered task")
    for kind, value in durations.items():
        if type(value) is not int or not 1 <= value <= 24 * 60:
            raise ValueError(f"staff_operations duration for {kind} is outside 1..1440 minutes")
    for name in (
        "sanitation_use_per_person_minute",
        "cleaning_sanitation_recovery",
        "outreach_uncertainty_relief",
        "patrol_reassurance",
    ):
        value = config[name]
        if (
            not isinstance(value, (int, float))
            or isinstance(value, bool)
            or not math.isfinite(value)
            or not 0.0 <= value <= 1.0
        ):
            raise ValueError(f"staff_operations {name} must be finite and within 0..1")
    thresholds_by_zone = config["thresholds_by_zone"]
    if not isinstance(thresholds_by_zone, dict):
        raise ValueError("staff_operations thresholds_by_zone must be an object")
    allowed_thresholds = {
        "cleaning_sanitation_below",
        "outreach_uncertainty_at_least",
        "patrol_stress_at_least",
    }
    for zone_id, thresholds in thresholds_by_zone.items():
        if (
            not isinstance(zone_id, str)
            or not zone_id
            or not isinstance(thresholds, dict)
            or not thresholds
            or set(thresholds) - allowed_thresholds
        ):
            raise ValueError("staff_operations per-zone threshold entry is invalid")
        for name, value in thresholds.items():
            if (
                not isinstance(value, (int, float))
                or isinstance(value, bool)
                or not math.isfinite(value)
                or not 0.0 <= value <= 1.0
            ):
                raise ValueError(f"staff_operations per-zone {name} must be finite and within 0..1")
    effects_by_zone = config["effects_by_zone"]
    allowed_effects = {
        "sanitation_use_per_person_minute": (0.0, 0.01),
        "cleaning_sanitation_recovery": (0.0, 1.0),
        "outreach_uncertainty_relief": (0.0, 1.0),
        "patrol_reassurance": (0.0, 1.0),
    }
    if not isinstance(effects_by_zone, dict):
        raise ValueError("staff_operations effects_by_zone must be an object")
    for zone_id, effects in effects_by_zone.items():
        if (
            not isinstance(zone_id, str)
            or not zone_id
            or not isinstance(effects, dict)
            or not effects
            or set(effects) - set(allowed_effects)
        ):
            raise ValueError("staff_operations per-zone effect entry is invalid")
        for name, value in effects.items():
            lower, upper = allowed_effects[name]
            if (
                not isinstance(value, (int, float))
                or isinstance(value, bool)
                or not math.isfinite(value)
                or not lower <= value <= upper
            ):
                raise ValueError(
                    f"staff_operations per-zone {name} must be finite and within "
                    f"{lower:g}..{upper:g}"
                )
    return config
