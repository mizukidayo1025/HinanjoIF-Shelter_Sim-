"""Lifecycle hooks for the versioned simulator extension namespaces."""

from __future__ import annotations

from typing import Any

from sheltersim.contracts.operations import (
    SHARED_OPERATIONS_KEY,
    SHARED_OPERATIONS_POLICY,
    validate_shared_operations_config,
)
from sheltersim.contracts.policies import selected_models
from sheltersim.domain.action_lifecycle import ACTION_LIFECYCLE_POLICY_VERSION
from sheltersim.domain.agent_dynamics import (
    AGENT_DYNAMICS_STATE_KEY,
    canonical_agent_dynamics_config,
)
from sheltersim.domain.body_state import (
    BODY_POLICY_VERSION,
    BODY_STATE_KEY,
)
from sheltersim.domain.facility_awareness import (
    FACILITY_AWARENESS_POLICY_VERSION,
    canonical_facility_awareness_config,
)
from sheltersim.domain.facility_queues import (
    FACILITY_QUEUES_POLICY_VERSION,
    FACILITY_QUEUES_STATE_KEY,
    independent_queues_required,
)
from sheltersim.domain.geometry_overlay import (
    GEOMETRY_OVERLAY_KEY,
    GEOMETRY_OVERLAY_POLICY,
    INSTALLED_FACILITY_PLACEMENT_POLICY_VERSION,
    TEMPORARY_ROOM_BED_POLICY_VERSION,
    TEMPORARY_ROOM_INTAKE_POLICY_VERSION,
)
from sheltersim.domain.movement import (
    MOVEMENT_POLICY_VERSION,
    MOVEMENT_STATE_KEY,
)
from sheltersim.domain.population import (
    POPULATION_DYNAMICS_POLICY_VERSION,
    POPULATION_DYNAMICS_STATE_KEY,
)
from sheltersim.domain.population_lifecycle import (
    POPULATION_LIFECYCLE_POLICY_VERSION,
    POPULATION_LIFECYCLE_STATE_KEY,
)
from sheltersim.domain.psychological_state import LIFE_STATE_PSYCHOLOGY_POLICIES
from sheltersim.domain.social_network import (
    PROXIMITY_CONTACT_POLICY_VERSION,
    SOCIAL_NETWORK_POLICY_VERSION,
    SOCIAL_NETWORK_STATE_KEY,
    canonical_proximity_contact_config,
)
from sheltersim.domain.staff_dynamics import (
    STAFF_DYNAMICS_POLICY_VERSION,
    STAFF_DYNAMICS_SOURCE_ID,
    STAFF_DYNAMICS_STATE_KEY,
    canonical_staff_dynamics_config,
)
from sheltersim.domain.staff_operations import (
    STAFF_OPERATIONS_POLICY_VERSION,
    STAFF_OPERATIONS_STATE_KEY,
)
from sheltersim.domain.temporary_spaces import (
    TEMPORARY_SPACES_POLICY_VERSION,
    TEMPORARY_SPACES_SOURCE_ID,
    TEMPORARY_SPACES_STATE_KEY,
)


def simulator_signatures(configs: dict[str, Any]) -> dict[str, str]:
    """Freeze enabled extension implementations alongside installed policy models."""
    if "geometry_overlay" in configs and "facility_geometry" not in configs:
        raise ValueError("geometry_overlay requires facility_geometry")
    if "shared_operations" in configs:
        validate_shared_operations_config(configs["shared_operations"])
    body_policy_enabled = selected_models(configs)["psychology"] in LIFE_STATE_PSYCHOLOGY_POLICIES
    proximity = canonical_proximity_contact_config(configs.get("social_proximity"))
    if proximity is not None and ("facility_geometry" not in configs or not body_policy_enabled):
        raise ValueError(
            "social_proximity requires facility_geometry and a life-state psychology policy"
        )
    if (
        not configs.get("simulator_extensions", {}).get("enabled", False)
        and "facility_geometry" not in configs
        and "population" not in configs
        and "staff_operations" not in configs
        and "staff_dynamics" not in configs
        and "shared_operations" not in configs
        and "facility_awareness" not in configs
        and not body_policy_enabled
    ):
        standalone = {"operator:action_lifecycle": f"{ACTION_LIFECYCLE_POLICY_VERSION}@1:state1"}
        if "population_context" in configs:
            standalone["operator:autonomy_behavior"] = "contextual-autonomy@2:stateless"
        if "zone_environment" in configs:
            from sheltersim.domain.environment import ScenarioZoneEnvironmentConfig

            environment = ScenarioZoneEnvironmentConfig.model_validate(configs["zone_environment"])
            standalone["operator:zone_environment"] = f"{environment.policy_version}@1:state1"
        return standalone
    versions = {
        AGENT_DYNAMICS_STATE_KEY: canonical_agent_dynamics_config(configs.get("agent_dynamics"))[
            "policy_version"
        ],
        POPULATION_DYNAMICS_STATE_KEY: POPULATION_DYNAMICS_POLICY_VERSION,
        SOCIAL_NETWORK_STATE_KEY: SOCIAL_NETWORK_POLICY_VERSION,
        STAFF_DYNAMICS_STATE_KEY: STAFF_DYNAMICS_POLICY_VERSION,
    }
    if independent_queues_required(configs):
        versions[FACILITY_QUEUES_STATE_KEY] = FACILITY_QUEUES_POLICY_VERSION
    if "shared_operations" in configs:
        versions[SHARED_OPERATIONS_KEY] = SHARED_OPERATIONS_POLICY
    versions[STAFF_DYNAMICS_STATE_KEY] = canonical_staff_dynamics_config(
        configs.get("staff_dynamics")
    )["policy_version"]
    if configs.get("simulator_extensions", {}).get("enabled", False) or "population" in configs:
        versions[POPULATION_LIFECYCLE_STATE_KEY] = POPULATION_LIFECYCLE_POLICY_VERSION
        if configs.get("simulator_extensions", {}).get("enabled", False):
            versions[TEMPORARY_SPACES_STATE_KEY] = TEMPORARY_SPACES_POLICY_VERSION
    if (
        configs.get("simulator_extensions", {}).get("enabled", False)
        or "staff_operations" in configs
        or body_policy_enabled
    ):
        versions[STAFF_OPERATIONS_STATE_KEY] = STAFF_OPERATIONS_POLICY_VERSION
    if "facility_geometry" in configs:
        versions[MOVEMENT_STATE_KEY] = MOVEMENT_POLICY_VERSION
    if "geometry_overlay" in configs:
        overlay_config = configs["geometry_overlay"]
        if (
            not isinstance(overlay_config, dict)
            or set(overlay_config) != {"schema_version"}
            or type(overlay_config["schema_version"]) is not int
            or overlay_config["schema_version"] not in {1, 2, 3, 4}
        ):
            raise ValueError("unsupported geometry_overlay config")
        versions[GEOMETRY_OVERLAY_KEY] = GEOMETRY_OVERLAY_POLICY
    if body_policy_enabled:
        versions[BODY_STATE_KEY] = BODY_POLICY_VERSION
    signatures = {
        f"simulator:{key}": _simulator_signature(key, version) for key, version in versions.items()
    }
    geometry = configs.get("facility_geometry", {})
    service_points = geometry.get("service_points", []) if isinstance(geometry, dict) else []
    if service_points:
        import hashlib
        import json

        service_hash = hashlib.sha256(
            json.dumps(
                service_points, sort_keys=True, separators=(",", ":"), allow_nan=False
            ).encode()
        ).hexdigest()
        signatures["operator:facility_service_points"] = f"facility-service-points@1:{service_hash}"
    if (
        configs.get("simulator_extensions", {}).get("enabled", False)
        or "population_context" in configs
        or body_policy_enabled
    ):
        signatures["operator:autonomy_behavior"] = "contextual-autonomy@2:stateless"
    if "zone_environment" in configs:
        from sheltersim.domain.environment import ScenarioZoneEnvironmentConfig

        environment = ScenarioZoneEnvironmentConfig.model_validate(configs["zone_environment"])
        signatures["operator:zone_environment"] = f"{environment.policy_version}@1:state1"
    if "facility_awareness" in configs:
        import hashlib
        import json

        awareness = canonical_facility_awareness_config(configs["facility_awareness"])
        awareness_hash = hashlib.sha256(
            json.dumps(awareness, sort_keys=True, separators=(",", ":")).encode("utf-8")
        ).hexdigest()
        signatures["operator:facility_awareness"] = (
            f"{FACILITY_AWARENESS_POLICY_VERSION}@1:{awareness_hash}"
        )
    if proximity is not None:
        signatures["operator:social_proximity"] = (
            f"{PROXIMITY_CONTACT_POLICY_VERSION}@1:state1:"
            f"distance={proximity['max_distance_m']}:"
            f"interval={proximity['sample_interval_minutes']}:"
            f"pairs={proximity['max_pairs_per_sample']}"
        )
    overlay_config = configs.get("geometry_overlay", {})
    if isinstance(overlay_config, dict) and overlay_config.get("schema_version") in {2, 3, 4}:
        signatures["operator:temporary_room_beds"] = f"{TEMPORARY_ROOM_BED_POLICY_VERSION}@1:state1"
    if isinstance(overlay_config, dict) and overlay_config.get("schema_version") in {3, 4}:
        signatures["operator:temporary_room_intake"] = (
            f"{TEMPORARY_ROOM_INTAKE_POLICY_VERSION}@1:state1"
        )
    if isinstance(overlay_config, dict) and overlay_config.get("schema_version") == 4:
        signatures["operator:installed_facility_placement"] = (
            f"{INSTALLED_FACILITY_PLACEMENT_POLICY_VERSION}@1:state1"
        )
    signatures["operator:action_lifecycle"] = f"{ACTION_LIFECYCLE_POLICY_VERSION}@1:state1"
    return signatures


def _simulator_signature(key: str, version: str) -> str:
    if key == STAFF_DYNAMICS_STATE_KEY:
        implementation_version = "1" if version == "continuous-duty-fatigue-v1" else "2"
    else:
        implementation_version = (
            "3"
            if key == BODY_STATE_KEY
            else "2"
            if key in {BODY_STATE_KEY, STAFF_OPERATIONS_STATE_KEY, SOCIAL_NETWORK_STATE_KEY}
            else "1"
        )
    state_version = "2" if key == MOVEMENT_STATE_KEY else "1"
    return f"{version}@{implementation_version}:state{state_version}"


# Where a namespace's past is kept. The history route answers only the KPIs and the
# conversations (#368 decision A, #376); these records stay in the saved Run's event log.
RECORDED_IN_EVENT_LOG = (
    "recorded in the saved Run's event log; GET branch/history answers kpis and conversations only"
)


def simulator_state_catalog() -> dict[str, Any]:
    """Installed built-in signatures; no world or mutable namespace is created."""
    return {
        MOVEMENT_STATE_KEY: {
            "schema_version": 2,
            "implementation_id": MOVEMENT_POLICY_VERSION,
            "implementation_version": "1",
            "entity_kind": "agent",
            "enabled_by": "facility_geometry config",
            "source_id": "ASM-SPATIAL-MOVEMENT-V2",
            "fields": {
                "positions": {"unit": "metre_2d"},
                "journeys": {"unit": "route_progress_with_room_waypoints"},
            },
            "external_validation": False,
        },
        FACILITY_QUEUES_STATE_KEY: {
            "schema_version": 1,
            "implementation_id": FACILITY_QUEUES_POLICY_VERSION,
            "implementation_version": "1",
            "entity_kind": "facility",
            "enabled_by": "multiple service points or non-facility queue_id",
            "source_id": "ASM-FACILITY-QUEUES-V1",
            "fields": {
                "queued_request_ids": {"unit": "independent_fifo_request_id"},
                "active_request_ids": {"unit": "concurrent_service_reservation"},
            },
            "history": RECORDED_IN_EVENT_LOG,
            "external_validation": False,
        },
        BODY_STATE_KEY: {
            "schema_version": 1,
            "implementation_id": BODY_POLICY_VERSION,
            "implementation_version": "3",
            "entity_kind": "agent",
            "enabled_by": (
                "policy_registry.psychology=life-state-feedback-v1 or mean-reverting-life-state-v1"
            ),
            "source_id": "ASM-LIFE-STATE-FEEDBACK-V1",
            "fields": {
                "sleep_debt_minutes": {"unit": "minute", "lower": 0, "upper": 1440},
                **{
                    key: {"unit": "normalized_0_1", "lower": 0, "upper": 1}
                    for key in (
                        "uncertainty",
                        "reassurance",
                        "discontent",
                        "positive_affect",
                        "negative_affect",
                        "hygiene_need",
                    )
                },
            },
            "history": RECORDED_IN_EVENT_LOG,
            "retention_limit": 20000,
            "external_validation": False,
        },
        STAFF_OPERATIONS_STATE_KEY: {
            "schema_version": 1,
            "implementation_id": STAFF_OPERATIONS_POLICY_VERSION,
            "implementation_version": "2",
            "entity_kind": "staff",
            "enabled_by": (
                "staff_operations config, simulator_extensions.enabled or life-state psychology"
            ),
            "source_id": "ASM-STAFF-OPERATIONS-V1",
            "fields": {
                "operation": {"unit": "typed_task_record"},
                "sanitation": {"unit": "normalized_0_1", "lower": 0, "upper": 1},
            },
            "history": RECORDED_IN_EVENT_LOG,
            "retention_limit": 20000,
            "external_validation": False,
        },
        POPULATION_LIFECYCLE_STATE_KEY: {
            "schema_version": 1,
            "implementation_id": POPULATION_LIFECYCLE_POLICY_VERSION,
            "implementation_version": "1",
            "entity_kind": "agent",
            "enabled_by": "population config or simulator_extensions.enabled",
            "source_id": "ASM-POPULATION-LIFECYCLE-V1",
            "fields": {
                "people": {"unit": "retained_same_identity_state"},
                "relationship_archive": {"unit": "sparse_pair_state"},
                "pending_returns": {"unit": "capacity_waiting_person"},
                "pending_relocations": {"unit": "route_bound_task"},
            },
            "external_validation": False,
        },
        TEMPORARY_SPACES_STATE_KEY: {
            "schema_version": 1,
            "implementation_id": TEMPORARY_SPACES_POLICY_VERSION,
            "implementation_version": "1",
            "entity_kind": "zone",
            "enabled_by": "simulator_extensions.enabled",
            "source_id": TEMPORARY_SPACES_SOURCE_ID,
            "fields": {
                "people_capacity": {"unit": "person", "input": "approved EffectProgram"},
                "sleeping_capacity": {"unit": "sleeping_place", "input": "approved EffectProgram"},
                "geometry_status": {"value": "zone_only_capacity"},
            },
            "history": RECORDED_IN_EVENT_LOG,
            "retention_limit": 20000,
            "external_validation": False,
        },
        GEOMETRY_OVERLAY_KEY: {
            "schema_version": 1,
            "implementation_id": GEOMETRY_OVERLAY_POLICY,
            "implementation_version": "1",
            "entity_kind": "system",
            "enabled_by": "geometry_overlay.schema_version=1, 2, 3 or 4",
            "source_id": "ASM-BOUNDED-RUNTIME-GEOMETRY-V1",
            "fields": {
                "facility_points": {
                    "unit": "metre_2d",
                    "limit": 256,
                    "installed_facility_placement": "geometry_overlay.schema_version=4",
                },
                "partitions": {"unit": "bounded_polygon_metre_2d", "limit": 256},
                "route_modifiers": {"unit": "boolean_with_expiry", "limit": 256},
                "bed_reservations": {
                    "unit": "person_to_sleep_bed_assignment",
                    "limit": 10000,
                    "enabled_by": "geometry_overlay.schema_version=2, 3 or 4",
                },
                "room_admissions": {
                    "unit": "resident_to_physical_room_capacity_assignment",
                    "limit": 10000,
                    "enabled_by": "geometry_overlay.schema_version=3 or 4",
                },
            },
            "history": RECORDED_IN_EVENT_LOG,
            "retention_limit": 20000,
            "external_validation": False,
        },
        SHARED_OPERATIONS_KEY: {
            "schema_version": 1,
            "implementation_id": SHARED_OPERATIONS_POLICY,
            "implementation_version": "1",
            "entity_kind": "system",
            "enabled_by": "shared_operations.schema_version=1",
            "fields": {
                "support_requests": {
                    "unit": "typed_request_lifecycle",
                    "identity": "synthetic_agent_id",
                },
                "history": {"unit": "applied_transition_receipt", "time_unit": "simulation_minute"},
            },
            "history": RECORDED_IN_EVENT_LOG,
            "retention_limit": 20000,
            "external_validation": False,
        },
        STAFF_DYNAMICS_STATE_KEY: {
            "schema_version": 1,
            "implementation_id": STAFF_DYNAMICS_POLICY_VERSION,
            "implementation_version": "1",
            "entity_kind": "staff",
            "enabled_by": "simulator_extensions.enabled or staff_dynamics config",
            "source_id": STAFF_DYNAMICS_SOURCE_ID,
            "fields": {
                "fatigue": {"unit": "normalized_0_1", "lower": 0, "upper": 1},
                "consecutive_work_minutes": {"unit": "minute"},
                "total_work_minutes": {"unit": "minute"},
                "total_rest_minutes": {"unit": "minute"},
            },
            "external_validation": False,
        },
    }
