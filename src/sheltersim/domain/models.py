"""Typed in-memory domain state used by the simulation controller."""

from __future__ import annotations

import math
import re
from dataclasses import dataclass, field
from enum import StrEnum
from typing import Any


class Mobility(StrEnum):
    """Functional mobility categories, not demographic labels."""

    INDEPENDENT = "independent"
    ASSISTED = "assisted"
    WHEELCHAIR = "wheelchair"


_INFORMATION_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._:-]{0,63}$")


@dataclass(frozen=True)
class InformationSpreadConfig:
    """Versioned, opt-in information diffusion and inquiry assumptions."""

    enabled: bool = False
    max_contacts_per_agent: int = 2
    minimum_contact_minutes: int = 2
    contact_duration_scale_minutes: int = 5
    base_transmission_probability: float = 0.2
    correction_transmission_multiplier: float = 1.25
    trust_sensitivity: float = 0.5
    family_contact_multiplier: float = 1.5
    adjacent_zone_contact_multiplier: float = 0.5
    query_pressure_per_claim: float = 0.5
    query_pressure_decay_per_minute: float = 0.002
    query_pressure_relief_per_correction: float = 0.5
    query_threshold: float = 0.45
    query_utility_weight: float = 0.8
    correction_acceptance_probability: float = 0.7
    correction_trust_sensitivity: float = 0.2
    correction_source_sensitivity: float = 0.2
    rumor_belief_probability: float = 0.5
    rumor_trust_sensitivity: float = 0.2
    rumor_source_sensitivity: float = 0.2
    default_source_reliability: float = 0.7
    correction_source_reliability_by_id: dict[str, float] = field(default_factory=dict)
    distress_sharing_sensitivity: float = 0.5
    share_propensity_decay_per_minute: float = 0.002
    forgetting_rate_per_minute: float = 0.0
    schema_version: int = 1
    policy_version: str = "sparse-information-contact-v1"
    source_type: str = "assumption"
    source_id: str = "ASM-INFORMATION-SPREAD-V1"

    def __post_init__(self) -> None:
        if not isinstance(self.enabled, bool):
            raise ValueError("information_spread enabled must be boolean")
        for name, value, minimum, maximum in (
            ("max_contacts_per_agent", self.max_contacts_per_agent, 0, 16),
            ("minimum_contact_minutes", self.minimum_contact_minutes, 1, 10_080),
            ("contact_duration_scale_minutes", self.contact_duration_scale_minutes, 1, 10_080),
        ):
            if (
                not isinstance(value, int)
                or isinstance(value, bool)
                or not minimum <= value <= maximum
            ):
                raise ValueError(
                    f"information_spread {name} must be between {minimum} and {maximum}"
                )
        for name in (
            "base_transmission_probability",
            "correction_transmission_multiplier",
            "trust_sensitivity",
            "family_contact_multiplier",
            "adjacent_zone_contact_multiplier",
            "query_pressure_per_claim",
            "query_pressure_decay_per_minute",
            "query_pressure_relief_per_correction",
            "query_threshold",
            "query_utility_weight",
            "correction_acceptance_probability",
            "correction_trust_sensitivity",
            "correction_source_sensitivity",
            "rumor_belief_probability",
            "rumor_trust_sensitivity",
            "rumor_source_sensitivity",
            "default_source_reliability",
            "distress_sharing_sensitivity",
            "share_propensity_decay_per_minute",
            "forgetting_rate_per_minute",
        ):
            value = getattr(self, name)
            maximum = (
                2.0
                if name
                in {
                    "correction_transmission_multiplier",
                    "family_contact_multiplier",
                    "adjacent_zone_contact_multiplier",
                }
                else 4.0
                if name == "query_utility_weight"
                else 1.0
                if name
                in {
                    "correction_acceptance_probability",
                    "correction_trust_sensitivity",
                    "correction_source_sensitivity",
                    "rumor_belief_probability",
                    "rumor_trust_sensitivity",
                    "rumor_source_sensitivity",
                    "default_source_reliability",
                    "distress_sharing_sensitivity",
                    "share_propensity_decay_per_minute",
                    "forgetting_rate_per_minute",
                }
                else 1.0
            )
            if (
                not isinstance(value, (int, float))
                or isinstance(value, bool)
                or not math.isfinite(value)
                or not 0.0 <= value <= maximum
            ):
                raise ValueError(
                    f"information_spread {name} must be finite and between 0 and {maximum}"
                )
        if len(self.correction_source_reliability_by_id) > 100 or any(
            not isinstance(source_id, str)
            or not _INFORMATION_ID.fullmatch(source_id)
            or type(value) not in (int, float)
            or isinstance(value, bool)
            or not math.isfinite(value)
            or not 0.0 <= value <= 1.0
            for source_id, value in self.correction_source_reliability_by_id.items()
        ):
            raise ValueError(
                "correction source reliability must map up to 100 source IDs to [0, 1]"
            )
        if (
            not isinstance(self.schema_version, int)
            or isinstance(self.schema_version, bool)
            or self.schema_version != 1
            or self.policy_version != "sparse-information-contact-v1"
        ):
            raise ValueError("unsupported information_spread schema or policy version")
        if self.source_type != "assumption" or self.source_id != "ASM-INFORMATION-SPREAD-V1":
            raise ValueError("information_spread provenance must use ASM-INFORMATION-SPREAD-V1")

    @classmethod
    def from_mapping(cls, document: Any = None) -> InformationSpreadConfig:
        if document is None:
            return cls()
        if not isinstance(document, dict):
            raise ValueError("information_spread model config must be an object")
        allowed = set(cls.__dataclass_fields__)
        unknown = set(document) - allowed
        if unknown:
            raise ValueError(f"unknown information_spread settings: {sorted(unknown)}")
        try:
            return cls(**document)
        except TypeError as exc:
            raise ValueError(f"invalid information_spread settings: {exc}") from exc

    def to_dict(self) -> dict[str, Any]:
        return {
            "enabled": self.enabled,
            "max_contacts_per_agent": self.max_contacts_per_agent,
            "minimum_contact_minutes": self.minimum_contact_minutes,
            "contact_duration_scale_minutes": self.contact_duration_scale_minutes,
            "base_transmission_probability": self.base_transmission_probability,
            "correction_transmission_multiplier": self.correction_transmission_multiplier,
            "trust_sensitivity": self.trust_sensitivity,
            "family_contact_multiplier": self.family_contact_multiplier,
            "adjacent_zone_contact_multiplier": self.adjacent_zone_contact_multiplier,
            "query_pressure_per_claim": self.query_pressure_per_claim,
            "query_pressure_decay_per_minute": self.query_pressure_decay_per_minute,
            "query_pressure_relief_per_correction": self.query_pressure_relief_per_correction,
            "query_threshold": self.query_threshold,
            "query_utility_weight": self.query_utility_weight,
            "correction_acceptance_probability": self.correction_acceptance_probability,
            "correction_trust_sensitivity": self.correction_trust_sensitivity,
            "correction_source_sensitivity": self.correction_source_sensitivity,
            "rumor_belief_probability": self.rumor_belief_probability,
            "rumor_trust_sensitivity": self.rumor_trust_sensitivity,
            "rumor_source_sensitivity": self.rumor_source_sensitivity,
            "default_source_reliability": self.default_source_reliability,
            "correction_source_reliability_by_id": dict(
                sorted(self.correction_source_reliability_by_id.items())
            ),
            "distress_sharing_sensitivity": self.distress_sharing_sensitivity,
            "share_propensity_decay_per_minute": self.share_propensity_decay_per_minute,
            "forgetting_rate_per_minute": self.forgetting_rate_per_minute,
            "schema_version": self.schema_version,
            "policy_version": self.policy_version,
            "source_type": self.source_type,
            "source_id": self.source_id,
        }

    def provenance(self) -> dict[str, Any]:
        return {
            "source_type": self.source_type,
            "source_id": self.source_id,
            "policy_version": self.policy_version,
            "values": self.to_dict(),
            "calibration_status": "uncalibrated experimental assumptions",
        }


def validate_unit_interval(name: str, value: float) -> None:
    """Validate a finite normalized state value, including mutable psychology fields."""
    if (
        not isinstance(value, (int, float))
        or isinstance(value, bool)
        or not math.isfinite(value)
        or not 0.0 <= value <= 1.0
    ):
        raise ValueError(f"{name} must be finite and between 0 and 1")


# ---- The Rust rows of the entities (native/src/state/entities.rs) -----------------------------
