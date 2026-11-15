"""Optional synthetic context, separate from immutable PopulationSpec v1 encodings."""

from __future__ import annotations

from typing import Any, Literal

from pydantic import Field, model_validator

from sheltersim.contracts.extensions import ExtensionModel, Identifier
from sheltersim.contracts.operations import SupportKind


class PersonContext(ExtensionModel):
    household_id: Identifier | None = None
    parent_ids: tuple[Identifier, ...] = Field(default=(), max_length=4)
    caregiver_ids: tuple[Identifier, ...] = Field(default=(), max_length=16)
    care_for_ids: tuple[Identifier, ...] = Field(default=(), max_length=16)
    partner_ids: tuple[Identifier, ...] = Field(default=(), max_length=16)
    relative_ids: tuple[Identifier, ...] = Field(default=(), max_length=32)
    requires_review: bool = False
    review_reasons: tuple[str, ...] = Field(default=(), max_length=32)
    roles: tuple[Literal["parent", "child", "caregiver", "dependent", "partner", "alone"], ...] = ()
    medical_support: bool | None = None
    guardian_status: Literal["accompanied", "unaccompanied", "not_applicable"] | None = None
    daily_rhythm_id: Identifier | None = None
    medication_support: bool | None = None
    care_support_needs: tuple[SupportKind, ...] = Field(default=(), max_length=8)
    pet_count: int | None = Field(default=None, strict=True, ge=0, le=20)
    information_access: tuple[Identifier, ...] | None = None
    usual_wake_minute: int | None = Field(default=None, strict=True, ge=0, lt=1440)
    usual_sleep_minute: int | None = Field(default=None, strict=True, ge=0, lt=1440)
    required_sleep_minutes: int | None = Field(default=None, strict=True, ge=0, le=1440)
    source_ids: tuple[Identifier, ...] = ()

    @model_validator(mode="after")
    def unique_care_support_needs(self) -> PersonContext:
        if len(self.care_support_needs) != len(set(self.care_support_needs)):
            raise ValueError("care_support_needs cannot contain duplicates")
        return self


class HouseholdContext(ExtensionModel):
    member_ids: tuple[Identifier, ...] = Field(min_length=1, max_length=100)
    source_ids: tuple[Identifier, ...] = ()


class PopulationContext(ExtensionModel):
    schema_version: Literal[1] = 1
    people: dict[Identifier, PersonContext]
    households: dict[Identifier, HouseholdContext] = Field(default_factory=dict)
    resident_distribution_source_ids: tuple[Identifier, ...] = ()
    evacuation_selection_source_ids: tuple[Identifier, ...] = ()


def validate_population_context(scenario: Any) -> None:
    raw = scenario.model_configs.get("population_context")
    if raw is None:
        return
    context = PopulationContext.model_validate(raw)
    profiles = {profile.id: profile for profile in scenario.profiles}
    if not set(context.people) <= set(profiles):
        raise ValueError("person context references an unknown initial profile")
    claimed_sources = set(context.resident_distribution_source_ids) | set(
        context.evacuation_selection_source_ids
    )
    for identity, person in context.people.items():
        relatives = [
            *person.parent_ids,
            *person.caregiver_ids,
            *person.care_for_ids,
            *person.partner_ids,
            *person.relative_ids,
        ]
        if identity in relatives or not set(relatives) <= set(profiles):
            raise ValueError("synthetic relationship contains a self/unknown reference")
        if person.household_id is not None and person.household_id != profiles[identity].group_id:
            raise ValueError("initial household context must agree with Profile.group_id")
        facts = person.model_dump(
            exclude_defaults=True, exclude_none=True, exclude={"household_id", "source_ids"}
        )
        if facts and not person.source_ids:
            raise ValueError("synthetic personal-context assumptions require source IDs")
        claimed_sources.update(person.source_ids)
    children: dict[str, list[str]] = {key: [] for key in profiles}
    degrees = {key: 0 for key in profiles}
    for identity, person in context.people.items():
        for parent in set(person.parent_ids):
            children[parent].append(identity)
            degrees[identity] += 1
    ready = [key for key, degree in degrees.items() if degree == 0]
    visited = 0
    while ready:
        parent = ready.pop()
        visited += 1
        for child in children[parent]:
            degrees[child] -= 1
            if degrees[child] == 0:
                ready.append(child)
    if visited != len(profiles):
        raise ValueError("synthetic parent relationships cannot contain cycles")
    seen_members: set[str] = set()
    for identity, household in context.households.items():
        members = set(household.member_ids)
        if (
            len(members) != len(household.member_ids)
            or seen_members.intersection(members)
            or not members <= set(profiles)
            or any(profiles[key].group_id != identity for key in members)
        ):
            raise ValueError("household membership conflicts with immutable initial profiles")
        seen_members.update(members)
        claimed_sources.update(household.source_ids)
    sources = {source["id"] for source in scenario.provenance.get("sources", [])}
    if not claimed_sources <= sources:
        raise ValueError("population context provenance references an unknown source")
