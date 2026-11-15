"""Versioned staff shift and fatigue memory built on existing staff tasks.

Rust owns the namespace, and its rules are native/src/rules/staff_dynamics.rs's kernels
(`kernel_only`): a value they refuse is KernelRefusal, before anything changes. The checks of a
Scenario's config, of an override and of a saved namespace stay here, in Python (they run outside
the tick).
"""

from __future__ import annotations

import json
import math
import re
from typing import Any

STAFF_DYNAMICS_STATE_KEY = "staff_dynamics_v1"
STAFF_DYNAMICS_POLICY_VERSION = "continuous-duty-fatigue-v1"
STAFF_DYNAMICS_SOURCE_ID = "ASM-STAFF-DYNAMICS-V1"
SHIFT_STAFF_DYNAMICS_POLICY_VERSION = "continuous-duty-fatigue-v2"
SHIFT_STAFF_DYNAMICS_SOURCE_ID = "ASM-STAFF-DYNAMICS-V2"
_CONFIG_FIELDS = {
    "schema_version",
    "policy_version",
    "source_id",
    "max_continuous_work_minutes",
    "max_continuous_work_minutes_by_staff",
    "rest_minutes",
    "shift_windows_by_staff",
    "off_shift_fatigue_recovery_per_minute",
}


def canonical_staff_dynamics_config(raw: Any) -> dict[str, Any]:
    """Validate the bounded, replaceable shift/rest assumptions.

    A Scenario's check, outside the tick. `create_staff_dynamics_state`'s kernel makes the same
    config in Rust.
    """
    if raw is None:
        raw = {}
    if not isinstance(raw, dict) or set(raw) - _CONFIG_FIELDS:
        raise ValueError("staff_dynamics config has unsupported fields")
    legacy = raw.get("policy_version") == STAFF_DYNAMICS_POLICY_VERSION or (
        "policy_version" not in raw
        and not {"shift_windows_by_staff", "off_shift_fatigue_recovery_per_minute"}.intersection(
            raw
        )
    )
    policy_version = (
        STAFF_DYNAMICS_POLICY_VERSION if legacy else SHIFT_STAFF_DYNAMICS_POLICY_VERSION
    )
    source_id = raw.get(
        "source_id",
        STAFF_DYNAMICS_SOURCE_ID if legacy else SHIFT_STAFF_DYNAMICS_SOURCE_ID,
    )
    config = {
        "schema_version": 1,
        "policy_version": policy_version,
        "source_id": source_id,
        "max_continuous_work_minutes": 240,
        "max_continuous_work_minutes_by_staff": {},
        "rest_minutes": 30,
        "shift_windows_by_staff": {},
        "off_shift_fatigue_recovery_per_minute": 0.0 if legacy else 0.005,
        **raw,
    }
    if (
        config["schema_version"] != 1
        or config["policy_version"]
        not in {
            STAFF_DYNAMICS_POLICY_VERSION,
            SHIFT_STAFF_DYNAMICS_POLICY_VERSION,
        }
        or not isinstance(config["source_id"], str)
        or not config["source_id"]
    ):
        raise ValueError("unsupported staff_dynamics policy or source")
    # 1.0 and true equal 1; the canonical config holds the int (model version 2).
    config["schema_version"] = 1
    if config["policy_version"] == STAFF_DYNAMICS_POLICY_VERSION and (
        config["shift_windows_by_staff"] or config["off_shift_fatigue_recovery_per_minute"] != 0.0
    ):
        raise ValueError("staff shift and off-shift recovery assumptions require policy v2")
    for name, lower, upper in (
        ("max_continuous_work_minutes", 1, 24 * 60),
        ("rest_minutes", 1, 8 * 60),
    ):
        value = config[name]
        if not isinstance(value, int) or isinstance(value, bool) or not lower <= value <= upper:
            raise ValueError(f"staff_dynamics {name} is outside its supported range")
    per_staff_maxima = config["max_continuous_work_minutes_by_staff"]
    if not isinstance(per_staff_maxima, dict):
        raise ValueError("staff_dynamics per-staff work limits must be an object")
    for staff_id, value in per_staff_maxima.items():
        if (
            not isinstance(staff_id, str)
            or not staff_id
            or type(value) is not int
            or not 1 <= value <= 24 * 60
        ):
            raise ValueError(
                "staff_dynamics per-staff work limits must be integer minutes in 1..1440"
            )
    windows = config["shift_windows_by_staff"]
    if not isinstance(windows, dict):
        raise ValueError("staff_dynamics shift_windows_by_staff must be an object")
    for staff_id, rows in windows.items():
        if (
            not isinstance(staff_id, str)
            or not staff_id
            or not isinstance(rows, list)
            or len(rows) > 21
        ):
            raise ValueError(
                "staff_dynamics shift windows require a staff ID and at most 21 intervals"
            )
        intervals: list[tuple[int, int]] = []
        for row in rows:
            if (
                not isinstance(row, dict)
                or set(row) != {"start_minute", "end_minute"}
                or type(row["start_minute"]) is not int
                or type(row["end_minute"]) is not int
                or not 0 <= row["start_minute"] < row["end_minute"] <= 10_080
            ):
                raise ValueError(
                    "staff shift intervals must be non-empty [start,end) weekly minutes"
                )
            intervals.append((row["start_minute"], row["end_minute"]))
        ordered = sorted(intervals)
        if any(first[1] > second[0] for first, second in zip(ordered, ordered[1:])):
            raise ValueError("staff shift intervals for one person cannot overlap")
    recovery = config["off_shift_fatigue_recovery_per_minute"]
    if (
        not isinstance(recovery, (int, float))
        or isinstance(recovery, bool)
        or not math.isfinite(recovery)
        or not 0.0 <= recovery <= 1.0
    ):
        raise ValueError("staff_dynamics off-shift recovery must be within 0..1 per minute")
    return config


def validate_staff_shift_windows_override(
    value: Any,
    *,
    known_staff_ids: set[str] | None = None,
) -> dict[str, list[dict[str, int]]]:
    """Validate a bounded per-staff map of non-overlapping weekly shift intervals."""
    if not isinstance(value, dict) or not 1 <= len(value) <= 100:
        raise ValueError("shift-window overrides require 1..100 staff IDs")
    if len(json.dumps(value, separators=(",", ":")).encode("utf-8")) > 16_384:
        raise ValueError("shift-window overrides exceed the 16 KiB limit")
    if any(
        not isinstance(staff_id, str)
        or re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._:-]{0,63}", staff_id) is None
        for staff_id in value
    ):
        raise ValueError("shift-window overrides contain an invalid staff ID")
    if known_staff_ids is not None and set(value) - known_staff_ids:
        raise ValueError("shift-window overrides reference unknown staff IDs")
    normalized: dict[str, list[dict[str, int]]] = {}
    for staff_id, rows in sorted(value.items()):
        if not isinstance(rows, list) or len(rows) > 21:
            raise ValueError("each staff member may have at most 21 weekly shift intervals")
        if any(
            not isinstance(row, dict)
            or set(row) != {"start_minute", "end_minute"}
            or type(row["start_minute"]) is not int
            or type(row["end_minute"]) is not int
            or not 0 <= row["start_minute"] < row["end_minute"] <= 10_080
            for row in rows
        ):
            raise ValueError("shift intervals must be non-empty [start,end) weekly minutes")
        normalized[staff_id] = sorted(
            (dict(row) for row in rows),
            key=lambda row: (row["start_minute"], row["end_minute"]),
        )
    canonical_staff_dynamics_config(
        {
            "policy_version": SHIFT_STAFF_DYNAMICS_POLICY_VERSION,
            "shift_windows_by_staff": normalized,
        }
    )
    return normalized
