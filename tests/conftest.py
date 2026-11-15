"""Settings for every test; the shared setup lives in tests/support/, one module per topic.

The modules are loaded as plugins below, so their fixtures reach every test file
as before. docs/testing.md lists what each module holds.
"""

from __future__ import annotations

from pathlib import Path

import pytest
from support.isolation import _isolate

pytest_plugins = [
    # Writes each run's results under .test-results/ (tests/results_log.py, #136).
    "results_log",
    "support.generated",
    "support.fixture_scenarios",
    "support.storage",
    "support.web",
    "support.llm",
    "support.reports",
    "support.public_data",
]


@pytest.fixture(autouse=True)
def _isolated_local_state(
    request: pytest.FixtureRequest,
    tmp_path_factory: pytest.TempPathFactory,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """Keep every test away from the developer's Projects, usage ledgers and LLM cache.

    Without this, a CLI test wrote a usage account under the repository's runs/.projects,
    and parallel workers (pytest -n) would share those default locations. Tests that set
    these variables themselves still override them.

    Paid live_api tests keep the real locations on purpose: their spend must count
    against this PC's campaign ledger.
    """
    if request.node.get_closest_marker("live_api") is not None:
        return
    _isolate(monkeypatch, tmp_path_factory.mktemp("local-state"))


def pytest_addoption(parser: pytest.Parser) -> None:
    parser.addoption(
        "--os-subset",
        action="store_true",
        help="keep only tests/unit, tests/e2e and the tests marked os_dependent (#380)",
    )


# What --os-subset keeps whole: quick tests, and the flows through HTTP or the CLI.
OS_SUBSET_DIRECTORIES = ("unit", "e2e")


def _take_os_subset(config: pytest.Config, items: list[pytest.Item]) -> None:
    """Keep what the Windows and macOS jobs of 「PRの確認」 (ci.yml) run (#380).

    The bytes of a Run are the same on every OS (the hashes of every path, checked on each),
    so those jobs leave out the tests that only compute: they keep the quick tests of
    tests/unit and tests/e2e, and the tests marked os_dependent, whose files, locks,
    processes or OS measurements behave differently by OS. The whole suite runs on each OS by
    hand (the long regression's `full`).
    """
    if not config.getoption("--os-subset"):
        return
    tests = Path(__file__).parent

    def kept(item: pytest.Item) -> bool:
        if item.get_closest_marker("os_dependent") is not None:
            return True
        if not item.path.is_relative_to(tests):
            return False
        return item.path.relative_to(tests).parts[0] in OS_SUBSET_DIRECTORIES

    dropped = [item for item in items if not kept(item)]
    if dropped:
        config.hook.pytest_deselected(items=dropped)
        items[:] = [item for item in items if kept(item)]


# Last, so that -m and -k have deselected first.
@pytest.hookimpl(trylast=True)
def pytest_collection_modifyitems(config: pytest.Config, items: list[pytest.Item]) -> None:
    """Keep this run's subset, then give each worker one long simulation to start with.

    pytest-xdist's worksteal scheduler hands each worker a contiguous block of the
    collected items and lets idle workers steal from the end of other blocks. When
    the long simulations sit together, one worker runs them back to back while the
    others finish early; putting one at the head of each block starts them all at once.
    Every worker collects in this same order, so the blocks still agree.
    """
    _take_os_subset(config, items)
    workers = getattr(config, "workerinput", {}).get("workercount", 1)
    long_runs = [item for item in items if item.get_closest_marker("long_simulation")]
    if workers < 2 or not long_runs:
        return
    rest = [item for item in items if item.get_closest_marker("long_simulation") is None]
    size = len(items) // workers
    ordered: list[pytest.Item] = []
    for index in range(workers):
        block = rest[: size - (1 if index < len(long_runs) else 0)] if index < workers - 1 else rest
        rest = rest[len(block) :]
        ordered.extend(long_runs[index : index + 1] + block)
    ordered.extend(long_runs[workers:])
    items[:] = ordered
