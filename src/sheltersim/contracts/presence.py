"""Shared typed presence/relocation receipts; domain Policies own actual transitions."""

from __future__ import annotations

PRESENCE_NAMESPACE = "population_presence_v1"


def register_presence_contract() -> None:
    """Explicit installed bootstrap hook; never enabled by importing a JSON scenario."""
    from sheltersim.contracts.runtime import register_state_contract, state_catalog
    from sheltersim.contracts.workspace import StateDescriptor

    descriptor = StateDescriptor(
        namespace=PRESENCE_NAMESPACE,
        implementation_id="population-presence-ledger-v1",
        implementation_version="1",
        schema_version=1,
        entity_collection="people",
        fields={
            "status": {"unit": "presence_state", "public": True},
            "household_id": {"unit": "identifier", "public": True},
            "transition_zone_id": {
                "unit": "identifier",
                "public": True,
                "semantics": "last_transition_location_not_current_position",
            },
            "expected_return_minute": {"unit": "minute", "public": True},
        },
    )
    present = next((row for row in state_catalog() if row["namespace"] == PRESENCE_NAMESPACE), None)
    if present is not None:
        if present != descriptor.model_dump(mode="json"):
            raise ValueError("presence contract already registered differently")
        return

    register_state_contract(descriptor)
