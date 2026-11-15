"""The psychology models of each person's minute: their settings and the values they give.

The rules are native/core/src/rules/psychology.rs, which knows each model here and
BoundedStressPolicy of policies/alternatives.py by its exact class (experiments/phases/update.py
runs it). The trace documents are native/core/src/rules/trace.rs. A model holds its settings and
its `policy_version`.
"""

from __future__ import annotations

import math
from dataclasses import dataclass, field
from typing import Any

from sheltersim.domain.body_state import BODY_SOURCE_ID, canonical_body_config
from sheltersim.domain.models import validate_unit_interval


@dataclass(frozen=True)
class PsychologyParameters:
    """Explicit uncalibrated per-minute rates used by the baseline policy."""

    toilet_growth: float = 0.001
    hunger_growth: float = 0.0005
    awake_fatigue_growth: float = 0.0008
    resting_fatigue_recovery: float = 0.004
    sleeping_fatigue_recovery: float = 0.010
    stress_pressure: float = 0.012
    queued_stress: float = 0.002
    resting_stress_recovery: float = 0.003
    sleeping_stress_recovery: float = 0.006
    need_pressure_threshold: float = 0.35
    hydration_growth: float = 0.0
    support_wait_stress: float = 0.001
    toilet_growth_by_agent: dict[str, float] = field(default_factory=dict)
    hunger_growth_by_agent: dict[str, float] = field(default_factory=dict)
    awake_fatigue_growth_by_agent: dict[str, float] = field(default_factory=dict)
    natural_recovery_rate_per_minute: float = 0.01
    natural_recovery_rate_by_agent: dict[str, float] = field(default_factory=dict)
    baseline_distress_by_agent: dict[str, float] = field(default_factory=dict)
    chronic_retention: float = 0.90
    chronic_acute_weight: float = 0.08
    chronic_high_distress_weight: float = 0.02
    high_distress_threshold: float = 0.70

    def __post_init__(self) -> None:
        for name in (
            "toilet_growth",
            "hunger_growth",
            "awake_fatigue_growth",
            "resting_fatigue_recovery",
            "sleeping_fatigue_recovery",
            "stress_pressure",
            "queued_stress",
            "resting_stress_recovery",
            "sleeping_stress_recovery",
            "need_pressure_threshold",
            "hydration_growth",
            "support_wait_stress",
            "natural_recovery_rate_per_minute",
        ):
            value = getattr(self, name)
            if not math.isfinite(value) or value < 0.0:
                raise ValueError(f"{name} must be finite and non-negative")
            if name != "need_pressure_threshold" and value > 0.25:
                raise ValueError(f"{name} cannot exceed 0.25 per minute")
        if self.need_pressure_threshold > 1.0:
            raise ValueError("need_pressure_threshold cannot exceed one")
        for field_name in (
            "toilet_growth_by_agent",
            "hunger_growth_by_agent",
            "awake_fatigue_growth_by_agent",
        ):
            values = getattr(self, field_name)
            if len(values) > 10_000 or any(
                not isinstance(agent_id, str)
                or not agent_id
                or type(value) not in (int, float)
                or not math.isfinite(value)
                or not 0.0 <= value <= 0.25
                for agent_id, value in values.items()
            ):
                raise ValueError(f"{field_name} must contain finite Agent rates in [0, 0.25]")
        for name in (
            "chronic_retention",
            "chronic_acute_weight",
            "chronic_high_distress_weight",
            "high_distress_threshold",
        ):
            validate_unit_interval(name, getattr(self, name))
        if len(self.baseline_distress_by_agent) > 10_000 or any(
            not isinstance(agent_id, str)
            or not agent_id
            or type(value) not in (int, float)
            or isinstance(value, bool)
            or not math.isfinite(value)
            or not 0.0 <= value <= 1.0
            for agent_id, value in self.baseline_distress_by_agent.items()
        ):
            raise ValueError("baseline_distress_by_agent must contain values in [0, 1]")
        if len(self.natural_recovery_rate_by_agent) > 10_000 or any(
            not isinstance(agent_id, str)
            or not agent_id
            or type(value) not in (int, float)
            or isinstance(value, bool)
            or not math.isfinite(value)
            or not 0.0 <= value <= 0.25
            for agent_id, value in self.natural_recovery_rate_by_agent.items()
        ):
            raise ValueError("natural_recovery_rate_by_agent must contain rates in [0, 0.25]")


class SimplePsychologyPolicy:
    """Transparent, uncalibrated need and stress contributions for each Agent.

    Each minute the needs grow by their rates (the per-person rates first) and fatigue recovers
    while resting or sleeping, at the zone's rest efficiency. Stress moves by the need pressure
    (`stress_pressure` times the highest need above `need_pressure_threshold`, hydration counted
    only with a resource service, times the person's stress sensitivity), the queue and support
    waits, and the zone's environmental stress, less the direct recovery of rest and sleep
    unless `include_direct_rest_stress_recovery` is off. Each value is clipped to 0..1; trust
    stays as it is.
    """

    policy_version = "causal-stress-v1"

    def __init__(
        self,
        parameters: PsychologyParameters | None = None,
        *,
        source_id: str = "ASM-H04-CAUSAL-STRESS-V1",
        include_direct_rest_stress_recovery: bool = True,
    ) -> None:
        self.parameters = parameters or PsychologyParameters()
        if not source_id:
            raise ValueError("source_id must be non-empty")
        self.source_id = source_id
        self.include_direct_rest_stress_recovery = include_direct_rest_stress_recovery


class LifeStatePsychologyPolicy:
    """Registered, experimental extension for sleep debt and affect feedback.

    After SimplePsychologyPolicy's values, the body values of `body_state_v1` move by the rates of
    `body_config`: sleep debt (awake at `required_sleep / (1440 - required_sleep)`, capped at 10,
    or the configured rate without a requirement; less while resting or sleeping, at the rest
    efficiency; within 0..1440), reassurance, uncertainty (with the person's query and
    misinformation pressure), discontent, positive and negative affect, and hygiene need. Their
    feedback, from `body_formula` (body-feedback-sleep-debt-v1 keeps only the sleep debt term),
    is added to stress, which is clipped to 0..1. A household member in another zone adds
    `discontent_stress_rate`. With `use_tick_start_stress_for_feedback` the affect follows the
    stress at the start of the minute instead of the base model's.
    """

    policy_version = "life-state-feedback-v1"
    source_id = BODY_SOURCE_ID

    def __init__(
        self,
        parameters: PsychologyParameters,
        *,
        body_config: Any = None,
        body_formula: Any = None,
        include_direct_rest_stress_recovery: bool = True,
        use_tick_start_stress_for_feedback: bool = False,
    ) -> None:
        self.base = SimplePsychologyPolicy(
            parameters,
            source_id=BODY_SOURCE_ID,
            include_direct_rest_stress_recovery=include_direct_rest_stress_recovery,
        )
        self.body_config = canonical_body_config(body_config)
        self.body_formula = body_formula
        self.parameters = parameters
        self.use_tick_start_stress_for_feedback = use_tick_start_stress_for_feedback


class IntegratedDistressLifeStatePolicy(LifeStatePsychologyPolicy):
    """Exact acute mean reversion composed with existing body and affect dynamics.

    LifeStatePsychologyPolicy without the direct rest recovery and with the affect read from the
    stress at the start of the minute; then stress reverts to the baseline as in
    MeanRevertingDistressPolicy, with the life-state feedback as one more forcing. It keeps the
    day-scale distress as that model does.
    """

    policy_version = "mean-reverting-life-state-v1"
    source_id = "ASM-MEAN-REVERTING-LIFE-STATE-V1"

    def __init__(
        self,
        parameters: PsychologyParameters,
        *,
        body_config: Any = None,
        body_formula: Any = None,
    ) -> None:
        super().__init__(
            parameters,
            body_config=body_config,
            body_formula=body_formula,
            include_direct_rest_stress_recovery=False,
            use_tick_start_stress_for_feedback=True,
        )
