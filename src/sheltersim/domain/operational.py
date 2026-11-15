"""Versioned runtime state for opt-in H03 operational events."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    pass


H03_EVENT_TYPES = frozenset(
    {
        "delivery_delayed",
        "water_outage",
        "water_restored",
        "staff_unavailable",
        "staff_shift_change",
    }
)


def uses_operational_events(events: Any) -> bool:
    """Return whether planned events opt into the H03 runtime contract."""
    return any(
        event.event_type in H03_EVENT_TYPES
        or (event.event_type == "resource_delivery" and "shipment_id" in event.payload)
        for event in events
    )


def operational_events_enabled(configs: dict[str, Any], events: Any) -> bool:
    """Return whether a Scenario keeps operational_events_v1 state.

    Planning an H03 event opts in, and so does model_configs.operational_events,
    which generated Scenarios set so that a water outage or staff absence can be
    approved before any delivery is planned (#65).
    """
    config = configs.get("operational_events")
    if config is not None and (
        not isinstance(config, dict)
        or set(config) != {"enabled"}
        or type(config["enabled"]) is not bool
    ):
        raise ValueError("operational_events config must be {'enabled': bool}")
    return (config is not None and config["enabled"]) or uses_operational_events(events)
