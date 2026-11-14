"""`pytest --os-subset` keeps what the Windows and macOS jobs of 「PRの確認」 (ci.yml) run (#380).

Those jobs keep tests/unit, tests/e2e and the integration tests marked os_dependent; the rest
runs on Linux on every pull request and on each OS by hand. The subset is taken before the
shard, so --shard splits what it left.
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).parents[3]
# Quick to collect: a directory of tests/unit, one integration file marked os_dependent and
# one that is not.
UNIT = "tests/unit/repo"
MARKED = "tests/integration/test_cli_entrypoint.py"
UNMARKED = "tests/integration/test_service_capacity_cli.py"


def _ids(*args: str) -> list[str]:
    result = subprocess.run(
        [sys.executable, "-m", "pytest", "--collect-only", "-q", "-p", "no:cacheprovider"]
        + [UNIT, MARKED, UNMARKED, *args],
        cwd=ROOT,
        env={**os.environ, "SHELTERSIM_LLM_MODE": "offline"},
        capture_output=True,
        text=True,
        timeout=300,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    return [line for line in result.stdout.splitlines() if "::" in line]


def test_the_subset_keeps_the_unit_tests_and_the_marked_integration_tests() -> None:
    every = _ids()
    subset = _ids("--os-subset")
    assert any(test.startswith(UNMARKED) for test in every)
    assert any(test.startswith(MARKED) for test in every)
    assert subset == [test for test in every if not test.startswith(UNMARKED)]


def test_the_shard_splits_what_the_subset_left() -> None:
    subset = _ids("--os-subset")
    assert _ids("--os-subset", "--shard", "1/2") == subset[0::2]
    assert _ids("--os-subset", "--shard", "2/2") == subset[1::2]
