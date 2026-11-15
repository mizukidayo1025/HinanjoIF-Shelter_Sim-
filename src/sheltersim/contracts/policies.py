"""The Model Catalog and the models a Scenario selects: the Rust core's (`validate/catalog.rs`,
`validate/selection.rs`), read through `sheltersim_native` (#475).

The catalog (`native/core/src/validate/model-catalog.json`) is the one copy of the installed
models: their definitions (formula, reference, source, versions, parameters with units and
bounds), the default models and the model suites. Python keeps no factory of its own; a model is
installed when the core computes it.
"""

from __future__ import annotations

import json
from copy import deepcopy
from typing import Any

from sheltersim.native import _native
from sheltersim.scenario_authoring.configs.psychological_state import (
    INTEGRATED_PSYCHOLOGY_POLICY_VERSION,
)

_CATALOG: dict[str, Any] = json.loads(_native.model_catalog())

# Historical Scenarios without a suite selector keep the default models so a saved Scenario can
# still be replayed as authored. New Scenario entry points opt into this coherent, internally
# tested candidate set explicitly.
CORE_MODEL_SUITE_V1 = "core-model-suite-v1"
CORE_MODEL_SUITE_V2 = "core-model-suite-v2"
CORE_MODEL_SUITES: dict[str, dict[str, str]] = {
    suite_id: dict(members) for suite_id, members in _CATALOG["suites"].items()
}
CORE_MODEL_SUITE_MODELS_V1 = CORE_MODEL_SUITES[CORE_MODEL_SUITE_V1]
CORE_MODEL_SUITE_MODELS = CORE_MODEL_SUITES[CORE_MODEL_SUITE_V2]

# Pre-research psychology models stay installed so saved Runs and Checkpoints
# replay exactly and remain usable as comparison controls. The mark lives here,
# not in the Model Catalog, because catalog entries are part of policy signatures.
LEGACY_PSYCHOLOGY_MODELS = frozenset({"causal-stress-v1", "life-state-feedback-v1"})


def legacy_psychology_notice(model_id: object) -> str | None:
    """Explain when a Scenario or Run uses a pre-research psychology model."""
    if model_id not in LEGACY_PSYCHOLOGY_MODELS:
        return None
    return (
        f"psychology model {model_id} is a legacy model kept for replay and comparison; "
        f"new generated Scenarios use {INTEGRATED_PSYCHOLOGY_POLICY_VERSION}"
    )


def selected_models(configs: dict[str, dict[str, Any]]) -> dict[str, str]:
    """The models the configs select, by domain, or the core's ValueError for configs it refuses."""
    try:
        text = json.dumps(configs, allow_nan=False)
    except (TypeError, ValueError) as exc:
        raise ValueError("model configs must be finite JSON values") from exc
    try:
        pairs = json.loads(_native.selected_models(text.encode()))
    except _native.Unsupported as exc:
        raise ValueError(str(exc)) from exc
    return dict(pairs)


def model_catalog() -> dict[str, dict[str, Any]]:
    """The installed models' definitions, by model ID (a copy the caller may change)."""
    return deepcopy(_CATALOG["models"])
