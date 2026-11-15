"""Independent checkpointed FIFO state for explicitly distinct service queues."""

from __future__ import annotations

from typing import Any

FACILITY_QUEUES_STATE_KEY = "facility_queues_v1"
FACILITY_QUEUES_POLICY_VERSION = "independent-facility-queues-v1"


def independent_queues_required(configs: dict[str, Any]) -> bool:
    """Opt in only when queue IDs change legacy one-FIFO-per-facility semantics.

    True when the signed `facility_geometry` config has a service point whose queue ID is not its
    facility's ID, or a facility with two points; False without a config dict. A config the
    contract turns down raises its error.
    """
    from sheltersim.contracts.extensions import FacilityGeometry

    raw = configs.get("facility_geometry")
    if not isinstance(raw, dict):
        return False
    points = FacilityGeometry.model_validate(raw).service_points
    by_facility: dict[str, int] = {}
    for point in points:
        by_facility[point.facility_id] = by_facility.get(point.facility_id, 0) + 1
    return any(point.queue_id != point.facility_id for point in points) or any(
        count > 1 for count in by_facility.values()
    )
