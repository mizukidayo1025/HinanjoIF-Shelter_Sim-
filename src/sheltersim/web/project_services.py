"""The Project routes of the local and VM versions that the browser has not: the catalog of the
contracts and the LLM usage of a Project and of a Run (the Python server's LLM Provider)."""

from __future__ import annotations

from typing import Any
from urllib.parse import parse_qs

from sheltersim.storage.projects import StorageError, digest
from sheltersim.storage.usage import UsageLedger
from sheltersim.web.project_queries import page


def one_query(query: str, allowed: set[str]) -> dict[str, str]:
    values = parse_qs(query)
    if set(values) - allowed or any(len(each) != 1 for each in values.values()):
        raise StorageError("invalid_query_fields")
    return {key: each[0] for key, each in values.items()}


class ProjectServices:
    def __init__(self, api: Any) -> None:
        self.api, self.store = api, api.store

    def get(self, parts: list[str], query: str) -> bytes | None:
        from sheltersim.web.projects import json_bytes

        result = self._get(parts, query)
        return None if result is None else json_bytes(result)

    def _get(self, parts: list[str], query: str) -> dict[str, Any] | None:
        project_id = parts[0]
        if len(parts) == 2 and parts[1] == "contracts" and not query:
            self.api.files.project(project_id)
            from sheltersim.contracts.evidence import operator_evidence_catalog
            from sheltersim.contracts.extensions import FacilityServicePoint as GeometryServicePoint
            from sheltersim.contracts.operations import (
                ActionLifecycleReceipt,
                CareTransitionReceipt,
                EquipmentTransferParameters,
                FacilityRecoveryReceipt,
                FacilityRepairParameters,
                OperationalDemandSnapshot,
                SharedOperationsConfig,
                SharedOperationsState,
                StaffHandoffParameters,
                StaffHandoffReceipt,
                StaffSupportContinuityReceipt,
                TemporaryRoomGeometry,
            )
            from sheltersim.contracts.policies import model_catalog
            from sheltersim.contracts.population import PopulationContext
            from sheltersim.contracts.presence import PresenceState, PresenceTransition
            from sheltersim.contracts.runtime import ModelStateProposal, state_catalog
            from sheltersim.contracts.spatial import PopulationTransition, SpatialConstraints
            from sheltersim.contracts.workspace import HistoryQuery, StateDescriptor
            from sheltersim.domain.simulator_state import simulator_state_catalog
            from sheltersim.events.effects import available_primitives

            return {
                "schema_version": 1,
                "history_query": HistoryQuery.model_json_schema(),
                "spatial_constraints": SpatialConstraints.model_json_schema(),
                "population_transition": PopulationTransition.model_json_schema(),
                "population_context": PopulationContext.model_json_schema(),
                "presence_state": PresenceState.model_json_schema(),
                "presence_transition": PresenceTransition.model_json_schema(),
                "state_descriptor": StateDescriptor.model_json_schema(),
                "model_state_proposal": ModelStateProposal.model_json_schema(),
                "states": state_catalog(),
                "simulator_states": simulator_state_catalog(),
                "operator_evidence": operator_evidence_catalog(),
                "models": model_catalog(),
                "policy_registry_schema_versions": [1, 2, 3],
                "policy_models": model_catalog(),
                "facility_service_point": GeometryServicePoint.model_json_schema(),
                "shared_operations_config": SharedOperationsConfig.model_json_schema(),
                "shared_operations_state": SharedOperationsState.model_json_schema(),
                "action_lifecycle_receipt": ActionLifecycleReceipt.model_json_schema(),
                "care_transition_receipt": CareTransitionReceipt.model_json_schema(),
                (
                    "staff_support_continuity_receipt"
                ): StaffSupportContinuityReceipt.model_json_schema(),
                "staff_handoff_receipt": StaffHandoffReceipt.model_json_schema(),
                "facility_recovery_receipt": FacilityRecoveryReceipt.model_json_schema(),
                "operational_demand_snapshot": OperationalDemandSnapshot.model_json_schema(),
                "executable_effect_primitives": available_primitives(),
                "typed_effect_contracts_only": {
                    "execution_status": "schemas_with_registered_effect_program_handlers",
                    "schemas": {
                        "temporary_room_geometry": TemporaryRoomGeometry.model_json_schema(),
                        "equipment_transfer": EquipmentTransferParameters.model_json_schema(),
                        "staff_handoff": StaffHandoffParameters.model_json_schema(),
                        "facility_repair": FacilityRepairParameters.model_json_schema(),
                    },
                    "temporary_room_beds": {
                        "policy_version": "exclusive-physical-sleep-beds-v1",
                        "enabled_by": "geometry_overlay.schema_version=2, 3 or 4",
                        "route_scope": "same_zone_in_room_route",
                    },
                    "temporary_room_intake": {
                        "policy_version": "physical-temporary-room-intake-v1",
                        "enabled_by": "geometry_overlay.schema_version=3 or 4",
                        "arrival_parameters": {
                            "admission_zone_ids": "explicit ordered fallback zones",
                            "approach_route_id": (
                                "optional FacilityGeometry.arrival_approach_routes ID ending at "
                                "the preferred zone"
                            ),
                        },
                        "capacity": (
                            "active room people_capacity; assignments persist through expiry"
                        ),
                        "arrival_movement": (
                            "optional operator/source-supplied route with explicit metric "
                            "coordinates and walking speed, checkpointed progress, entrance "
                            "waiting, and admission history; route-free arrivals retain direct "
                            "admission"
                        ),
                    },
                },
                "time_semantics": {
                    "operation_receipts": (
                        "event realized_minute; transition occurs at this boundary"
                    ),
                    "timeline_frames": "before_tick",
                    "operational_demand": "before_tick observed state only",
                },
                "errors": {
                    "operation_receipts": [
                        "contract_not_enabled",
                        "unapplied_or_unmatched_event",
                        "event_payload_mismatch",
                        "identity_or_transition_mismatch",
                        "unobserved_state_result",
                    ],
                    "service_point": [
                        "geometry_schema_invalid",
                        "service_point_unknown_facility",
                        "service_point_outside_zone",
                        "service_point_in_obstacle",
                        "service_point_on_wall",
                        "service_point_outside_equipment",
                        "service_point_in_other_equipment",
                        "service_point_access_route_missing",
                        "service_point_capacity_exceeds_facility",
                    ],
                },
            }
        if len(parts) == 2 and parts[1] == "usage":
            values = one_query(query, {"limit", "cursor"})
            limit = int(values.get("limit", "25"))
            if not 1 <= limit <= 100:
                raise StorageError("invalid_page_limit")
            self.store._read(self.store._path(project_id, "project.json"))
            rows = [
                UsageLedger(self.store, project_id, self.store._read(p)["account_id"]).summary()
                for p in sorted(self.store._path(project_id, "usage").glob("*.json"))
            ]
            result = page(
                rows,
                {"project_id": project_id, "kind": "usage"},
                digest(rows),
                limit=limit,
                cursor=values.get("cursor"),
            )
            result["project_totals"] = {
                key: sum(row["totals"][key] for row in rows)
                for key in ("calls", "input_tokens", "output_tokens", "cost_nusd")
            }
            return result
        if len(parts) == 4 and parts[1] == "runs" and parts[3] == "usage" and not query:
            self.api.files.run_record(project_id, parts[2])
            path = self.store._path(project_id, "runs", parts[2], "usage.json")
            if not path.exists():
                return {"schema_version": 1, "status": "legacy_account_not_initialized"}
            return UsageLedger(
                self.store, project_id, self.store._read(path)["account_id"]
            ).summary()
        return None
