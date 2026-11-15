"""Versioned inputs shared by persistent services and independent UI workers."""

from __future__ import annotations

from typing import Any, Literal

from pydantic import Field

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
