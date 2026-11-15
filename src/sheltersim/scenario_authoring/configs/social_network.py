"""Directed, checkpoint-safe contact and relationship records for Agents."""

from __future__ import annotations

import math
from typing import Any, Mapping

SOCIAL_NETWORK_STATE_KEY = "social_network_v1"
SOCIAL_NETWORK_POLICY_VERSION = "directed-contact-relationships-v1"
PROXIMITY_CONTACT_POLICY_VERSION = "distance-sampled-proximity-v1"
PROXIMITY_CONTACT_SOURCE_ID = "ASM-PROXIMITY-CONTACT-V1"
PROXIMITY_DISABLED_POLICY_VERSION = "disabled-proximity-v1"


def canonical_proximity_contact_config(raw: Any) -> dict[str, Any] | None:
    """Validate the opt-in physical-contact sampler's bounded assumptions."""
    if raw is None:
        return None
    if not isinstance(raw, Mapping) or set(raw) != {
        "schema_version",
        "enabled",
        "policy_version",
        "sample_interval_minutes",
        "max_distance_m",
        "max_pairs_per_sample",
    }:
        raise ValueError("social_proximity config fields do not match")
    if (
        type(raw["schema_version"]) is not int
        or raw["schema_version"] != 1
        or type(raw["enabled"]) is not bool
        or raw["policy_version"] != PROXIMITY_CONTACT_POLICY_VERSION
    ):
        raise ValueError("social_proximity config version is unsupported")
    interval = raw["sample_interval_minutes"]
    maximum = raw["max_distance_m"]
    pair_limit = raw["max_pairs_per_sample"]
    if (
        type(interval) is not int
        or not 1 <= interval <= 60
        or isinstance(maximum, bool)
        or not isinstance(maximum, (int, float))
        or not math.isfinite(float(maximum))
        or not 0.25 <= float(maximum) <= 10.0
        or type(pair_limit) is not int
        or not 1 <= pair_limit <= 10_000
    ):
        raise ValueError("social_proximity bounds are invalid")
    if not raw["enabled"]:
        return None
    return {
        "schema_version": 1,
        "enabled": True,
        "policy_version": PROXIMITY_CONTACT_POLICY_VERSION,
        "sample_interval_minutes": interval,
        "max_distance_m": float(maximum),
        "max_pairs_per_sample": pair_limit,
    }
