"""Paths of the files under fixtures/ that tests read.

Test files may import these constants directly, as they may the stateless synthetic
data of other support modules (docs/testing.md); anything with state or cleanup in
tests/support/ reaches them as a fixture.
"""

from __future__ import annotations

from pathlib import Path

FIXTURES = Path(__file__).parents[2] / "fixtures"
SCENARIOS = FIXTURES / "scenarios"
SCENARIO_100_AGENTS = FIXTURES / "scenario_100_agents.json"
SMALL_REPLAY = SCENARIOS / "h00_small_replay.json"
BASELINE_100_7D = SCENARIOS / "baseline_100_7d.json"
FAILURE_DAY3 = SCENARIOS / "failure_day3.json"
RESEARCH_OPERATIONAL_100_7D = SCENARIOS / "research_operational_100_7d.json"
REGRESSION_CONTRACT = FIXTURES / "regression-contract.json"
GATE_ALL_PATHS = FIXTURES / "gate-all-paths.json"
SOURCE_MANIFEST = FIXTURES / "source-manifest.json"
AGENTS_INITIAL = FIXTURES / "agents_initial.json"
PROFILES_SMALL = FIXTURES / "profiles_small.json"
BENCHMARKS = FIXTURES.parent / "benchmarks"
# The fixtures of the core's tests (native/core/tests/, #152, #225); support/core_fixtures.py
# lists those that are written again.
CORE_TEST_DATA = FIXTURES.parent / "native" / "core" / "tests" / "data"
# The Scenarios the core's minute-0 builder is checked on (#429), recorded before #430
# (support/initial_corpus.py).
INITIAL_CORPUS = FIXTURES / "initial_corpus.json.xz"
