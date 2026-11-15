"""Versioned runtime geometry changes; immutable Scenario geometry stays unchanged.

Idle facility moves and transfers, bounded obstacle partitions, and timed route
closures are supported. Physical temporary rooms require a separate explicit
geometry operator; zone-only capacity never pretends to be a surveyed room.
"""

from __future__ import annotations

GEOMETRY_OVERLAY_KEY = "geometry_overlay_v1"


GEOMETRY_OVERLAY_POLICY = "bounded-runtime-geometry-v1"
TEMPORARY_ROOM_BED_POLICY_VERSION = "exclusive-physical-sleep-beds-v1"
TEMPORARY_ROOM_BED_SOURCE_ID = "ASM-TEMPORARY-ROOM-BEDS-V1"
TEMPORARY_ROOM_INTAKE_POLICY_VERSION = "physical-temporary-room-intake-v1"
TEMPORARY_ROOM_INTAKE_SOURCE_ID = "ASM-TEMPORARY-ROOM-INTAKE-V1"
# geometry_overlay schema_version=4 places facilities installed during a Run on
# the floor plan with the Scenario generator's own grid rule.
INSTALLED_FACILITY_PLACEMENT_POLICY_VERSION = "generator-grid-installed-facility-v1"
INSTALLED_FACILITY_PLACEMENT_SOURCE_ID = "ASM-INSTALLED-FACILITY-PLACEMENT-V1"
