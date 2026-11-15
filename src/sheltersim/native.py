"""The Rust kernels (docs/rust-port.md).

The sheltersim_native extension is required; docs/development.md ("Rustのkernel") builds and
installs it. A kernel returns exactly the bytes of the Python code it
replaced. Where a function here returns None, or a kernel raises Unsupported, the kernel leaves
that value to the Python code that the caller still has.
"""

from __future__ import annotations

from typing import Any

try:
    import sheltersim_native as _native
except ImportError as exc:  # pragma: no cover - the package cannot run without it
    raise ImportError(
        "sheltersim needs its Rust extension sheltersim_native; build and install it as "
        "docs/development.md (Rustのkernel) says"
    ) from exc


# A kernel's refusal: the input is outside the rule's contract.
Unsupported: type[Exception] = _native.Unsupported


def rules(name: str) -> Any:
    """The module of native/src/rules/<name>.rs.

    Porting lanes call their kernels through it and need no function in this file.
    """
    return getattr(_native.rules, name)


def canonical_json(value: Any) -> bytes | None:
    """json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False,
    allow_nan=False).encode(), or None when the kernel leaves value to json."""
    try:
        return _native.canonical_json(value)
    except _native.Unsupported:
        return None


def model_versions(scenario: Any) -> dict[str, str]:
    """The model versions the core records for a new Run of the Scenario (`initial::records`,
    #429): each selected model's signature, the extensions', the declared states' and the
    engine's."""
    import json

    return json.loads(_native.initial_model_versions(scenario.canonical_json().encode()))
