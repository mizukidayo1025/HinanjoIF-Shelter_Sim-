"""Opt-in infrastructure dependency graph and runtime event state (A03)."""

from __future__ import annotations

from collections import defaultdict, deque
from collections.abc import Mapping
from typing import TYPE_CHECKING, Annotated, Any, Literal

from pydantic import BaseModel, ConfigDict, Field, StringConstraints, model_validator

if TYPE_CHECKING:
    pass


INFRASTRUCTURE_START_EVENT_TYPES = frozenset(
    {
        "power_outage",
        "equipment_failure",
        "extreme_heat",
        "extreme_cold",
        "staff_shift_change",
        "supply_route_interruption",
    }
)
INFRASTRUCTURE_COMPLETION_EVENT_TYPES = frozenset(
    {
        "power_restored",
        "equipment_repaired",
        "environmental_stressor_end",
        "supply_route_restored",
    }
)
INFRASTRUCTURE_MAX_START_EVENTS = 256
INFRASTRUCTURE_MAX_ROUTE_SHIPMENTS = 100
_StableId = Annotated[
    str,
    StringConstraints(
        strip_whitespace=True,
        min_length=1,
        max_length=64,
        pattern=r"^[A-Za-z0-9][A-Za-z0-9._:-]*$",
    ),
]


class InfrastructureDependencyEdge(BaseModel):
    """One dependent component and the component it requires."""

    model_config = ConfigDict(extra="forbid", frozen=True)

    dependent_id: _StableId
    dependency_id: _StableId


class ScenarioInfrastructureSpec(BaseModel):
    """Opt-in, source-attributed dependency and experimental environment contract."""

    model_config = ConfigDict(extra="forbid", frozen=True)

    schema_version: Literal[1] = 1
    policy_version: Literal["infrastructure-events-v1"] = "infrastructure-events-v1"
    source_type: Literal["assumption"] = "assumption"
    source_id: _StableId = "ASM-A03-INFRASTRUCTURE-V1"
    dependency_edges: tuple[InfrastructureDependencyEdge, ...] = ()
    heat_stress_per_minute: float = Field(default=0.001, ge=0.0, le=0.01, allow_inf_nan=False)
    cold_stress_per_minute: float = Field(default=0.001, ge=0.0, le=0.01, allow_inf_nan=False)

    @model_validator(mode="after")
    def validate_dependency_graph(self) -> ScenarioInfrastructureSpec:
        edges = {(edge.dependent_id, edge.dependency_id) for edge in self.dependency_edges}
        if len(edges) != len(self.dependency_edges):
            raise ValueError("infrastructure dependency edges must be unique")
        if any(dependent == dependency for dependent, dependency in edges):
            raise ValueError("infrastructure dependency edges cannot point to themselves")

        reverse: dict[str, set[str]] = defaultdict(set)
        nodes: set[str] = set()
        for dependent, dependency in edges:
            reverse[dependency].add(dependent)
            nodes.update((dependent, dependency))
        visiting: set[str] = set()
        visited: set[str] = set()

        def visit(node: str) -> None:
            if node in visiting:
                raise ValueError("infrastructure dependency graph must be acyclic")
            if node in visited:
                return
            visiting.add(node)
            for dependent in sorted(reverse.get(node, ())):
                visit(dependent)
            visiting.remove(node)
            visited.add(node)

        for node in sorted(nodes):
            visit(node)
        object.__setattr__(
            self,
            "dependency_edges",
            tuple(
                sorted(
                    self.dependency_edges, key=lambda edge: (edge.dependent_id, edge.dependency_id)
                )
            ),
        )
        return self


def dependent_facilities(
    infrastructure_config: Mapping[str, Any],
    component_id: str,
    facility_ids: set[str],
) -> tuple[str, ...]:
    """Return facilities downstream of a failed component, in stable order."""
    config = ScenarioInfrastructureSpec.model_validate(dict(infrastructure_config))
    dependents: dict[str, set[str]] = defaultdict(set)
    for edge in config.dependency_edges:
        dependents[edge.dependency_id].add(edge.dependent_id)
    reached: set[str] = set()
    pending = deque([component_id])
    visited = {component_id}
    while pending:
        current = pending.popleft()
        if current in facility_ids:
            reached.add(current)
        for dependent in sorted(dependents.get(current, ())):
            if dependent in facility_ids:
                reached.add(dependent)
            if dependent not in visited:
                visited.add(dependent)
                pending.append(dependent)
    return tuple(sorted(reached))
