"""Versioned shared contracts for action, support and shelter operations.

These schemas define record and state boundaries. They do not implement care outcomes,
staff scheduling, facility repair physics, or inferred resource demand.
"""

from __future__ import annotations

from typing import Any, Literal

from sheltersim.contracts.extensions import ExtensionModel

SHARED_OPERATIONS_KEY = "shared_operations_v1"
SHARED_OPERATIONS_POLICY = "shared-operations-v1"


class SharedOperationsConfig(ExtensionModel):
    schema_version: Literal[1] = 1
    policy_version: Literal[SHARED_OPERATIONS_POLICY] = SHARED_OPERATIONS_POLICY


SupportKind = Literal[
    "medical_assessment",
    "medication_support",
    "caregiver_assistance",
    "privacy_accommodation",
    "mobility_assistance",
    "hygiene_support",
    "emotional_support",
    "language_access",
    "dependent_assistance",
]


def validate_shared_operations_config(raw: Any) -> SharedOperationsConfig:
    try:
        return SharedOperationsConfig.model_validate(raw)
    except (TypeError, ValueError) as exc:
        raise ValueError("invalid shared_operations config") from exc
