"""Deterministic staff assignment and support-request lifecycle."""

from __future__ import annotations

STAFF_SUPPORT_POLICY_VERSION = "staff-support-fifo-aging-v2"
STAFF_SUPPORT_SOURCE_ID = "ASM-STAFF-SUPPORT-LIFECYCLE-V1"


def default_support_fairness_minutes(unresolved_after_minutes: int) -> int:
    """Pick a bounded default aging threshold that precedes shorter timeouts."""
    if type(unresolved_after_minutes) is not int or unresolved_after_minutes <= 0:
        return 30
    return max(1, min(30, unresolved_after_minutes - 1))
