"""Versioned structured output schemas used at the LLM boundary."""

from __future__ import annotations

from typing import Any

from sheltersim.experiments.scenario import ScenarioSpec
from sheltersim.scenario_authoring.configs.models import Mobility


def initial_needs_schema(profile_count: int) -> dict[str, Any]:
    """Return the exact-count output contract for Opus synthetic need generation."""
    if profile_count <= 0:
        raise ValueError("profile_count must be positive")
    needs = {
        "type": "object",
        "properties": {
            "toilet_urgency": {"type": "number", "minimum": 0.0, "maximum": 1.0},
            "hunger": {"type": "number", "minimum": 0.0, "maximum": 1.0},
            "fatigue": {"type": "number", "minimum": 0.0, "maximum": 1.0},
        },
        "required": ["toilet_urgency", "hunger", "fatigue"],
        "additionalProperties": False,
    }
    return {
        "type": "object",
        "properties": {
            "profiles": {
                "type": "array",
                "minItems": profile_count,
                "maxItems": profile_count,
                "items": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "string", "maxLength": 64},
                        "initial_needs": needs,
                    },
                    "required": ["id", "initial_needs"],
                    "additionalProperties": False,
                },
            }
        },
        "required": ["profiles"],
        "additionalProperties": False,
    }


def scenario_draft_schema(scenario: ScenarioSpec, *, max_events: int = 20) -> dict[str, Any]:
    """Return an ID-bounded schema for synthetic profile and facility-event drafts."""
    profile_count = len(scenario.profiles)
    if profile_count <= 0:
        raise ValueError("scenario generation requires at least one profile")
    if max_events < 0:
        raise ValueError("max_events cannot be negative")
    zone_indexes = list(range(len(scenario.zones)))
    facility_indexes = list(range(len(scenario.facilities)))
    maximum_capacity = max((facility.capacity for facility in scenario.facilities), default=1)
    profile_item = {
        "type": "object",
        "properties": {
            "profile_index": {"type": "integer", "minimum": 0, "maximum": profile_count - 1},
            "family_index": {
                "type": "integer",
                "minimum": 0,
                "maximum": profile_count - 1,
            },
            "zone_index": {"type": "integer", "enum": zone_indexes},
            "mobility": {"type": "string", "enum": [item.value for item in Mobility]},
            "support_need": {"type": "number", "minimum": 0.0, "maximum": 1.0},
            "stress_sensitivity": {"type": "number", "minimum": 0.0, "maximum": 1.0},
            "initial_needs": {
                "type": "object",
                "properties": {
                    "toilet_urgency": {"type": "number", "minimum": 0.0, "maximum": 1.0},
                    "hunger": {"type": "number", "minimum": 0.0, "maximum": 1.0},
                    "fatigue": {"type": "number", "minimum": 0.0, "maximum": 1.0},
                },
                "required": ["toilet_urgency", "hunger", "fatigue"],
                "additionalProperties": False,
            },
        },
        "required": [
            "profile_index",
            "family_index",
            "zone_index",
            "mobility",
            "support_need",
            "stress_sensitivity",
            "initial_needs",
        ],
        "additionalProperties": False,
    }
    event_item = {
        "type": "object",
        "properties": {
            "event_type": {
                "type": "string",
                "enum": ["facility_outage", "facility_recovery"],
            },
            "due_minute": {
                "type": "integer",
                "minimum": 0,
                "maximum": scenario.duration_minutes - 1,
            },
            "facility_index": {"type": "integer", "enum": facility_indexes},
            "units": {"type": "integer", "minimum": 1, "maximum": maximum_capacity},
        },
        "required": ["event_type", "due_minute", "facility_index", "units"],
        "additionalProperties": False,
    }
    return {
        "type": "object",
        "properties": {
            "profiles": {
                "type": "array",
                "minItems": profile_count,
                "maxItems": profile_count,
                "items": profile_item,
            },
            "events": {
                "type": "array",
                "minItems": 0,
                "maxItems": max_events,
                "items": event_item,
            },
        },
        "required": ["profiles", "events"],
        "additionalProperties": False,
    }
