"""Versioned inputs shared by persistent services and independent UI workers."""

from __future__ import annotations

from typing import Any, Literal

from pydantic import Field, model_validator

from sheltersim.contracts.extensions import ExtensionModel, Finite, Identifier


class UsageLimits(ExtensionModel):
    schema_version: Literal[1] = 1
    max_calls: int = Field(strict=True, gt=0, le=100000)
    max_input_tokens: int = Field(strict=True, gt=0)
    max_output_tokens: int = Field(strict=True, gt=0)
    budget_usd: Finite = Field(strict=True, ge=0)
    max_concurrency: int = Field(default=1, strict=True, ge=1, le=16)


class DraftInput(ExtensionModel):
    schema_version: Literal[1] = 1
    kind: Literal["scenario_input", "scenario", "intervention"] = "scenario_input"
    content: dict[str, Any]
    expected_hash: str | None = Field(default=None, pattern=r"^[a-f0-9]{64}$")


class HistoryQuery(ExtensionModel):
    schema_version: Literal[1] = 1
    kind: Literal[
        "people",
        "households",
        "events",
        "causal",
        "kpis",
        "conversations",
        "social",
        "staff",
        "actions",
        "transitions",
        "dialogue",
        "care",
        "operations",
    ]
    start: int = Field(default=0, strict=True, ge=0)
    end: int = Field(strict=True, ge=0)
    step: int = Field(default=1, strict=True, ge=1)
    limit: int = Field(default=25, strict=True, ge=1, le=100)
    cursor: str | None = Field(default=None, max_length=2048)
    agent_id: Identifier | None = None
    household_id: Identifier | None = None
    event_id: Identifier | None = None
    staff_id: Identifier | None = None
    conversation_id: Identifier | None = None
    zone_id: Identifier | None = None
    event_type: Identifier | None = None
    source_id: Identifier | None = None
    status: Identifier | None = None
    text: str | None = Field(default=None, min_length=1, max_length=160)
    include_related: bool = False

    @model_validator(mode="after")
    def bounded_query(self) -> HistoryQuery:
        for selector, allowed in (
            (
                "agent_id",
                {
                    "people",
                    "events",
                    "causal",
                    "social",
                    "actions",
                    "transitions",
                    "dialogue",
                    "care",
                },
            ),
            (
                "household_id",
                {"households", "events", "actions", "transitions", "dialogue", "care"},
            ),
            ("staff_id", {"staff", "actions", "dialogue", "operations"}),
            ("conversation_id", {"conversations"}),
            (
                "event_id",
                {
                    "causal",
                    "events",
                    "social",
                    "staff",
                    "actions",
                    "transitions",
                    "dialogue",
                    "care",
                    "operations",
                },
            ),
        ):
            if getattr(self, selector) is not None and self.kind not in allowed:
                raise ValueError(f"{selector} is not supported for {self.kind} history")
        if self.include_related and self.kind != "causal":
            raise ValueError("include_related applies only to causal history")
        if self.start > self.end:
            raise ValueError("history start must not exceed end")
        if self.kind == "people" and self.agent_id is None:
            raise ValueError("people history requires an agent_id")
        if self.kind == "households" and self.household_id is None:
            raise ValueError("household history requires a household_id")
        if self.kind == "causal" and self.event_id is None:
            raise ValueError("causal history requires an event_id")
        return self


class StateDescriptor(ExtensionModel):
    """Installed code declares this; scenario JSON cannot authorize Python code."""

    namespace: Identifier
    implementation_id: Identifier
    implementation_version: Identifier
    schema_version: int = Field(strict=True, ge=1)
    fields: dict[str, dict[str, Any]] = Field(default_factory=dict)
    source_ids: tuple[Identifier, ...] = ()
    entity_kind: Literal["agent", "staff"] = "agent"
    entity_collection: Identifier | None = None
    policy_domain: Identifier | None = None
