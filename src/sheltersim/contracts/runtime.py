"""Installed versioned model-state contracts a Scenario may declare, with no executable
scenario expressions."""

from __future__ import annotations

from typing import Any

from sheltersim.contracts.workspace import StateDescriptor

# The installed model-state contracts a Scenario may declare, by namespace: code metadata, never
# mutable simulation state. The core holds and checks the states themselves.
_CONTRACTS: dict[str, StateDescriptor] = {}


def register_state_contract(descriptor: StateDescriptor) -> None:
    if descriptor.namespace in _CONTRACTS:
        raise ValueError("state contract already installed")
    _CONTRACTS[descriptor.namespace] = descriptor


def declared_states(configs: dict[str, Any]) -> dict[str, StateDescriptor]:
    from sheltersim.contracts.bootstrap import ensure_builtin_contracts

    ensure_builtin_contracts()
    declaration = configs.get("state_contracts")
    if declaration is None:
        return {}
    if (
        set(declaration) != {"schema_version", "namespaces"}
        or type(declaration["schema_version"]) is not int
        or declaration["schema_version"] != 1
    ):
        raise ValueError("unsupported state_contracts declaration")
    if not isinstance(declaration["namespaces"], dict):
        raise ValueError("state_contracts namespaces must be an object")
    result = {}
    for namespace, requested in declaration["namespaces"].items():
        if namespace not in _CONTRACTS:
            raise ValueError("requested model-state contract is not installed")
        descriptor = _CONTRACTS[namespace]
        signature = {
            "implementation_id": descriptor.implementation_id,
            "implementation_version": descriptor.implementation_version,
            "schema_version": descriptor.schema_version,
        }
        if (
            not isinstance(requested, dict)
            or type(requested.get("schema_version")) is not int
            or requested != signature
        ):
            raise ValueError("model-state implementation/schema mismatch; migration required")
        if descriptor.policy_domain:
            from sheltersim.contracts.policies import model_catalog, selected_models

            selected = selected_models(configs).get(descriptor.policy_domain)
            if (
                selected is None
                or model_catalog()[selected]["state_schema_version"] != descriptor.schema_version
            ):
                raise ValueError("selected policy cannot use this installed state schema")
        result[namespace] = descriptor
    return result


def state_catalog() -> list[dict[str, Any]]:
    return [value.model_dump(mode="json") for _, value in sorted(_CONTRACTS.items())]
