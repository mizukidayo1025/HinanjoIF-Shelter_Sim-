"""The check of the facility queue config a Scenario author writes, before the core reads it.

The queue models themselves are the core's (`native/core/src/config/queues.rs` and the queue
rules); their definitions are in the Model Catalog (`contracts/policies.py`). The texts are the
core's.
"""

from __future__ import annotations

from typing import Any

NEED_WEIGHTED_QUEUE_MODEL = "need-weighted-physical-queue-v1"


def check_facility_queue_config(configs: dict[str, dict[str, Any]]) -> None:
    """Raise the core's ValueError for a `facility_queues` config the selected queue model refuses.

    The need-weighted model is the selected one, or, with no queue selected, the one a Scenario with
    `facility_geometry` runs; the other (strict FIFO) takes any object.
    """
    from sheltersim.contracts.policies import selected_models

    selection = selected_models(configs).get("queue")
    config = configs.get("facility_queues", {})
    if config is not None and not isinstance(config, dict):
        raise ValueError("facility_queues config must be an object")
    need_weighted = (
        selection == NEED_WEIGHTED_QUEUE_MODEL
        if selection is not None
        else "facility_geometry" in configs
    )
    if not need_weighted:
        return
    config = {} if config is None else config
    if set(config) - {
        "schema_version",
        "queue_capacity_by_facility",
        "abandon_after_minutes_by_service",
        "retry_delay_minutes",
        "persistence_urgency_threshold",
    }:
        raise ValueError("facility_queues config must be an object")
    if config.get("schema_version", 1) != 1:
        raise ValueError("facility_queues requires schema_version 1")
    limits = config.get("abandon_after_minutes_by_service", {})
    if not isinstance(limits, dict) or set(limits) - {"toilet", "water", "meal"}:
        raise ValueError("queue abandonment limits contain an unsupported service")
    if any(type(value) is not int or not 1 <= value <= 10080 for value in limits.values()):
        raise ValueError("queue abandonment limits must be within 1..10080 minutes")
    retry = config.get("retry_delay_minutes", 15)
    if type(retry) is not int or not 1 <= retry <= 1440:
        raise ValueError("queue retry delay must be between 1 and 1440 minutes")
    threshold = config.get("persistence_urgency_threshold", 0.8)
    if (
        not isinstance(threshold, (int, float))
        or isinstance(threshold, bool)
        or not 0.0 <= threshold <= 1.0
    ):
        raise ValueError("queue persistence threshold must be normalized to 0..1")
