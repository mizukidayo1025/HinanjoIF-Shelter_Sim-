"""Additive spatial/intake contracts; legacy geometry and hard capacity stay unchanged."""

from __future__ import annotations

import hashlib
import math
from typing import Any, Literal

from pydantic import Field

from sheltersim.contracts.extensions import (
    ExtensionModel,
    FacilityGeometry,
    FacilityServicePoint,
    Finite,
    Identifier,
    Point2D,
    ServiceAccessConditions,
)


class Wall(ExtensionModel):
    id: Identifier
    start: Point2D
    end: Point2D
    source_ids: tuple[Identifier, ...] = ()


class Opening(ExtensionModel):
    id: Identifier
    wall_id: Identifier
    center: Point2D
    width_m: Finite = Field(gt=0)
    edge_ids: tuple[Identifier, ...]
    accessible: bool = True
    source_ids: tuple[Identifier, ...] = ()


class SpatialObstacle(ExtensionModel):
    id: Identifier
    polygon: tuple[Point2D, ...] = Field(min_length=3, max_length=128)
    source_ids: tuple[Identifier, ...] = ()


class EquipmentFootprint(ExtensionModel):
    facility_id: Identifier
    polygon: tuple[Point2D, ...] = Field(min_length=4, max_length=4)
    area_m2: Finite = Field(gt=0)
    source_ids: tuple[Identifier, ...] = ()


class SpatialConstraints(ExtensionModel):
    schema_version: Literal[1] = 1
    walls: tuple[Wall, ...] = ()
    openings: tuple[Opening, ...] = ()
    obstacles: tuple[SpatialObstacle, ...] = ()
    equipment_footprints: tuple[EquipmentFootprint, ...] = ()
    planning_capacity_by_zone: dict[Identifier, int] = Field(default_factory=dict)
    accessible_facility_ids: tuple[Identifier, ...] = ()


def legacy_service_points(
    geometry: FacilityGeometry, facilities: Any
) -> tuple[FacilityServicePoint, ...]:
    """Build point records that preserve existing facility-target/FIFO semantics."""
    points = []
    for facility in sorted(facilities, key=lambda row: row.id):
        point = geometry.facility_points.get(facility.id)
        if point is None:
            continue
        capacity = (
            facility.working_units if facility.working_units is not None else facility.capacity
        )
        points.append(
            FacilityServicePoint(
                id=f"legacy-service:{hashlib.sha256(facility.id.encode()).hexdigest()[:24]}",
                facility_id=facility.id,
                point_m=point,
                access=ServiceAccessConditions(accessibility="unknown"),
                service_capacity=capacity,
                queue_id=facility.id,
            )
        )
    return tuple(points)


def validate_spatial_constraints(scenario: Any) -> None:
    raw = scenario.model_configs.get("spatial_constraints")
    if raw is None:
        return
    from sheltersim.contracts.extensions import FacilityGeometry

    spec = SpatialConstraints.model_validate(raw)
    geometry = FacilityGeometry.model_validate(scenario.model_configs["facility_geometry"])
    walls = {wall.id: wall for wall in spec.walls}
    if len(walls) != len(spec.walls) or len({o.id for o in spec.openings}) != len(spec.openings):
        raise ValueError("wall/opening IDs must be unique")
    if len({item.id for item in spec.obstacles}) != len(spec.obstacles):
        raise ValueError("obstacle IDs must be unique")
    if len({item.facility_id for item in spec.equipment_footprints}) != len(
        spec.equipment_footprints
    ):
        raise ValueError("equipment footprint facility IDs must be unique")
    from sheltersim.contracts.extensions import polygon_area

    for item in (*spec.obstacles, *spec.equipment_footprints):
        if polygon_area(item.polygon) <= 0:
            raise ValueError("obstacle/equipment polygon must have positive area")
        if any(
            not 0 <= point.x <= geometry.width_m or not 0 <= point.y <= geometry.height_m
            for point in item.polygon
        ):
            raise ValueError("obstacle/equipment polygon lies outside facility bounds")
    if not {item.facility_id for item in spec.equipment_footprints} <= {
        f.id for f in scenario.facilities
    }:
        raise ValueError("equipment footprint references an unknown facility")
    for wall in spec.walls:
        if wall.start == wall.end:
            raise ValueError("wall must have positive length")
        for point in (wall.start, wall.end):
            if not 0 <= point.x <= geometry.width_m or not 0 <= point.y <= geometry.height_m:
                raise ValueError("wall lies outside facility bounds")
    for opening in spec.openings:
        if opening.wall_id not in walls or not set(opening.edge_ids) <= {
            r.edge_id for r in geometry.routes
        }:
            raise ValueError("opening references missing wall or route")
        wall = walls[opening.wall_id]
        dx, dy = wall.end.x - wall.start.x, wall.end.y - wall.start.y
        length = math.sqrt(dx * dx + dy * dy)
        px, py = opening.center.x - wall.start.x, opening.center.y - wall.start.y
        distance = (px * dx + py * dy) / length
        if (
            abs(px * dy - py * dx) > 1e-7
            or distance - opening.width_m / 2 < -1e-9
            or distance + opening.width_m / 2 > length + 1e-9
        ):
            raise ValueError("opening must lie on and fit inside its wall")
    if not set(spec.planning_capacity_by_zone) <= {z.id for z in scenario.zones}:
        raise ValueError("planning capacity references unknown zone")
    if any(type(v) is not int or v < 0 for v in spec.planning_capacity_by_zone.values()):
        raise ValueError("planning capacities must be nonnegative integers")
    if not set(spec.accessible_facility_ids) <= {f.id for f in scenario.facilities}:
        raise ValueError("accessibility references unknown facility")
