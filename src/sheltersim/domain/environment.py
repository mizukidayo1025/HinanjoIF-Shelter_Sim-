"""Opt-in, assumption-tagged zone environment modifiers (H19)."""

from __future__ import annotations

from typing import TYPE_CHECKING, Annotated, Literal

from pydantic import BaseModel, ConfigDict, Field, StringConstraints, model_validator

if TYPE_CHECKING:
    pass


ZONE_ENVIRONMENT_SOURCE_ID = "ASM-H19-ZONE-ENVIRONMENT-V2"
_LEGACY_ZONE_ENVIRONMENT_SOURCE_ID = "ASM-H19-ZONE-ENVIRONMENT-V1"
MAX_ZONE_ENVIRONMENT_MODIFIERS = 16

_StableId = Annotated[
    str,
    StringConstraints(
        strip_whitespace=True,
        min_length=1,
        max_length=64,
        pattern=r"^[A-Za-z0-9][A-Za-z0-9._:-]*$",
    ),
]


class _EnvironmentContract(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)


class ZoneEnvironmentBaseline(_EnvironmentContract):
    """Initial experimental relative conditions for one zone."""

    ventilation_effectiveness: float = Field(
        default=0.7, strict=True, ge=0.0, le=1.0, allow_inf_nan=False
    )
    lighting_level: float = Field(default=0.5, strict=True, ge=0.0, le=1.0, allow_inf_nan=False)
    privacy_level: float = Field(default=0.5, strict=True, ge=0.0, le=1.0, allow_inf_nan=False)
    temperature_c: float = Field(default=20.0, strict=True, ge=-40.0, le=60.0, allow_inf_nan=False)


class ScenarioZoneEnvironmentConfig(_EnvironmentContract):
    """Bounded model assumptions for zone noise and action-demand modifiers."""

    schema_version: Literal[1] = 1
    policy_version: Literal["zone-environment-v1", "zone-environment-v2"] = "zone-environment-v2"
    source_type: Literal["assumption"] = "assumption"
    source_id: _StableId = ZONE_ENVIRONMENT_SOURCE_ID
    noise_rest_efficiency_loss: float = Field(
        default=0.5, strict=True, ge=0.0, le=0.9, allow_inf_nan=False
    )
    min_probability_multiplier: float = Field(
        default=0.5, strict=True, ge=0.1, le=1.0, allow_inf_nan=False
    )
    max_probability_multiplier: float = Field(
        default=2.0, strict=True, ge=1.0, le=2.0, allow_inf_nan=False
    )
    min_rest_decision_minutes: int = Field(default=1, strict=True, ge=1, le=60)
    max_rest_decision_minutes: int = Field(default=60, strict=True, ge=1, le=240)
    max_active_modifiers_per_zone: int = Field(
        default=MAX_ZONE_ENVIRONMENT_MODIFIERS,
        strict=True,
        ge=1,
        le=MAX_ZONE_ENVIRONMENT_MODIFIERS,
    )
    lighting_sleep_efficiency_loss: float = Field(
        default=0.2, strict=True, ge=0.0, le=0.9, allow_inf_nan=False
    )
    ventilation_hygiene_growth_per_minute: float = Field(
        default=0.0001, strict=True, ge=0.0, le=0.01, allow_inf_nan=False
    )
    privacy_discontent_per_minute: float = Field(
        default=0.0001, strict=True, ge=0.0, le=0.01, allow_inf_nan=False
    )
    temperature_stress_per_degree_minute: float = Field(
        default=0.0001, strict=True, ge=0.0, le=0.01, allow_inf_nan=False
    )
    comfortable_min_temperature_c: float = Field(
        default=18.0, strict=True, ge=-10.0, le=40.0, allow_inf_nan=False
    )
    comfortable_max_temperature_c: float = Field(
        default=26.0, strict=True, ge=-10.0, le=50.0, allow_inf_nan=False
    )
    baseline_by_zone: dict[_StableId, ZoneEnvironmentBaseline] = Field(default_factory=dict)

    @model_validator(mode="before")
    @classmethod
    def keep_legacy_environment_assumptions_neutral(cls, raw: object) -> object:
        if not isinstance(raw, dict):
            return raw
        legacy = raw.get("policy_version") == "zone-environment-v1" or (
            "policy_version" not in raw
            and not {
                "lighting_sleep_efficiency_loss",
                "ventilation_hygiene_growth_per_minute",
                "privacy_discontent_per_minute",
                "temperature_stress_per_degree_minute",
                "baseline_by_zone",
            }.intersection(raw)
        )
        if not legacy:
            return raw
        result = dict(raw)
        result.setdefault("policy_version", "zone-environment-v1")
        result.setdefault("source_id", _LEGACY_ZONE_ENVIRONMENT_SOURCE_ID)
        for name in (
            "lighting_sleep_efficiency_loss",
            "ventilation_hygiene_growth_per_minute",
            "privacy_discontent_per_minute",
            "temperature_stress_per_degree_minute",
        ):
            if name not in result:
                result[name] = 0.0
        result.setdefault("baseline_by_zone", {})
        return result

    @model_validator(mode="after")
    def multipliers_are_ordered(self) -> ScenarioZoneEnvironmentConfig:
        if self.policy_version == "zone-environment-v1" and (
            self.lighting_sleep_efficiency_loss
            or self.ventilation_hygiene_growth_per_minute
            or self.privacy_discontent_per_minute
            or self.temperature_stress_per_degree_minute
            or self.baseline_by_zone
        ):
            raise ValueError("new environment feedback requires zone-environment-v2")
        if self.min_probability_multiplier > self.max_probability_multiplier:
            raise ValueError("minimum probability multiplier exceeds its maximum")
        if self.min_rest_decision_minutes > self.max_rest_decision_minutes:
            raise ValueError("minimum rest decision interval exceeds its maximum")
        if self.comfortable_min_temperature_c >= self.comfortable_max_temperature_c:
            raise ValueError("comfortable temperature interval must be non-empty")
        return self
