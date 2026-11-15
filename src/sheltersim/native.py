"""The Rust core's Python entries (native/README.md): every computation of a Run is the core's.

The sheltersim_native extension is required; docs/development.md ("Rustのkernel") builds and
installs it.
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


# A value the core does not compute: an input outside what the browser and the server send.
Unsupported: type[Exception] = _native.Unsupported


def rules(name: str) -> Any:
    """The module of native/src/rules/<name>.rs: `plain`, `run_files` or `answers`."""
    return getattr(_native.rules, name)


def model_versions(scenario: Any) -> dict[str, str]:
    """The model versions the core records for a new Run of the Scenario (`initial::records`,
    #429): each selected model's signature, the extensions', the declared states' and the
    engine's."""
    import json

    return json.loads(_native.initial_model_versions(scenario.canonical_json().encode()))
