"""Checkpoint-safe metre-scale positions and route progress for Agents."""

from __future__ import annotations

import math
from typing import Any

MOVEMENT_STATE_KEY = "movement_v1"
MOVEMENT_POLICY_VERSION = "spatial-route-and-room-waypoint-v2"
MOVEMENT_SOURCE_ID = "ASM-SPATIAL-MOVEMENT-V2"
_MOVEMENT_DEFAULTS = {
    "schema_version": 1,
    "policy_version": MOVEMENT_POLICY_VERSION,
    "source_id": MOVEMENT_SOURCE_ID,
    "speed_m_per_minute_by_mobility": {
        "independent": 72.0,
        "assisted": 36.0,
        "wheelchair": 48.0,
    },
    "density_slowdown_per_person_m2": 0.12,
    "maximum_density_slowdown": 2.5,
}


def canonical_movement_config(raw: Any) -> dict[str, Any]:
    """Validate versioned speed/crowding assumptions used by physical routes."""
    if raw is None:
        raw = {}
    if not isinstance(raw, dict) or set(raw) - set(_MOVEMENT_DEFAULTS):
        raise ValueError("movement_model config has unsupported fields")
    config = {**_MOVEMENT_DEFAULTS, **raw}
    if isinstance(config["speed_m_per_minute_by_mobility"], dict):
        # The speed table is nested mutable data; isolate each Scenario so a
        # coefficient override cannot mutate the module defaults or its siblings.
        config["speed_m_per_minute_by_mobility"] = dict(config["speed_m_per_minute_by_mobility"])
    if (
        config["schema_version"] != 1
        or config["policy_version"] != MOVEMENT_POLICY_VERSION
        or config["source_id"] != MOVEMENT_SOURCE_ID
    ):
        raise ValueError("unsupported movement_model version or source")
    speeds = config["speed_m_per_minute_by_mobility"]
    if not isinstance(speeds, dict) or set(speeds) != {"independent", "assisted", "wheelchair"}:
        raise ValueError("movement_model speed table must define every mobility class")
    for value in speeds.values():
        if (
            not isinstance(value, (int, float))
            or isinstance(value, bool)
            or not math.isfinite(value)
            or value <= 0
        ):
            raise ValueError("movement_model speeds must be finite positive metres per minute")
    for name in ("density_slowdown_per_person_m2", "maximum_density_slowdown"):
        value = config[name]
        if (
            not isinstance(value, (int, float))
            or isinstance(value, bool)
            or not math.isfinite(value)
            or value < 0
        ):
            raise ValueError(f"movement_model {name} must be finite and non-negative")
    if config["maximum_density_slowdown"] < 1.0:
        raise ValueError("movement_model maximum_density_slowdown must be at least one")
    return config

    # Geometry may have changed since this route was checkpointed. The
    # movement phase checks the untraversed segment before advancing and
    # replans queue trips against the current wall/obstacle set.
