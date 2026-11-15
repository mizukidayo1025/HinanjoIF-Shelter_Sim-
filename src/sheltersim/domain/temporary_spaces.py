"""Versioned, operator-approved temporary capacity for dynamic population overflow.

This contract affects admission and capacity validation only. It carries no floorplan
coordinates, so consumers must not draw a fabricated room or walking path for a space.
"""

from __future__ import annotations

TEMPORARY_SPACES_STATE_KEY = "temporary_spaces_v1"
TEMPORARY_SPACES_POLICY_VERSION = "typed-temporary-capacity-v1"
TEMPORARY_SPACES_SOURCE_ID = "ASM-TEMPORARY-CAPACITY-V1"
