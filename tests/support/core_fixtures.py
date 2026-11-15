"""The fixtures of the core's tests that are still written from a route of the application (#225,
#430).

Most files under `native/core/tests/data/` hold worlds the Python side built before #430, with the
values the Python rules gave on them: they pin the core's answers now, and nothing writes them
again. The files registered here (`FIXTURES`) are written by a builder that needs no Python
simulation: the server's answers (the Project routes through the core), and the Scenario authoring
of Python whose results the core's builder must give. `benchmarks/core_fixtures.py` writes them
again, and `integration/test_core_fixtures.py` checks that each file is what its builder makes
today.
"""

from __future__ import annotations

import importlib
import json
from collections.abc import Callable
from pathlib import Path
from typing import Any

from support.paths import CORE_TEST_DATA

# Each fixture file under native/core/tests/data/ (its stem) and the function that builds its
# document, as "module:function".
FIXTURES: dict[str, str] = {
    "api_answers": "support.core_api_answers:api_answers_fixture",
    "scenario_facility": "support.core_scenario_facility:scenario_facility_fixture",
    "interventions": "support.core_interventions:interventions_fixture",
    "scenario_request": "support.core_scenario_request:scenario_request_fixture",
    "scenario_population": "support.core_scenario_population:scenario_population_fixture",
    "scenario_conditions": "support.core_scenario_conditions:scenario_conditions_fixture",
    "research_cases": "support.core_research:research_cases_fixture",
    "scenario_builder": "support.core_scenario_builder:scenario_builder_fixture",
}


# ---- The registry ------------------------------------------------------------------------------


def fixture_path(name: str) -> Path:
    """The file of a registered fixture."""
    return CORE_TEST_DATA / f"{name}.json"


def build(name: str) -> dict[str, Any]:
    """The document of a registered fixture, as its builder makes it now."""
    module, function = FIXTURES[name].split(":")
    builder: Callable[[], dict[str, Any]] = getattr(importlib.import_module(module), function)
    return builder()


def render(document: dict[str, Any], name: str | None = None) -> str:
    """The text of a fixture file, indented."""
    del name
    return json.dumps(document, sort_keys=True, ensure_ascii=False, indent=1) + "\n"


def write(name: str) -> Path:
    """Write a registered fixture again."""
    path = fixture_path(name)
    path.write_text(render(build(name), name), encoding="utf-8")
    return path
