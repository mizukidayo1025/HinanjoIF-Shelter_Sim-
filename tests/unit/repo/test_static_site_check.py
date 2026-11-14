"""The browser site's check against the Python server (benchmarks/static_site_check.py, #395).

The site's Worker makes, in Project `sample`, a Run the browser computes itself
(`run-browser-compute`); the Python ProjectStore of build_project_sample.py has only the sample's
Run. The check leaves that Run out of the browser's Run lists, says so, and compares everything
else in them.
"""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

ROOT = Path(__file__).parents[3]


def _site_check():
    # The script imports its neighbour browser_profiles.py as a script would.
    if str(ROOT / "benchmarks") not in sys.path:
        sys.path.insert(0, str(ROOT / "benchmarks"))
    spec = importlib.util.spec_from_file_location(
        "static_site_check", ROOT / "benchmarks" / "static_site_check.py"
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules.setdefault(spec.name, module)
    spec.loader.exec_module(module)
    return module


def _run(run_id: str, status: str = "completed") -> dict[str, object]:
    return {"run_id": run_id, "status": status, "branches": [{"branch_id": "main"}]}


def test_the_compute_run_is_left_out_of_both_run_lists() -> None:
    check = _site_check()
    sample, compute = _run(check.RUN), _run(check.COMPUTE_RUN, "queued")
    for route, document in (
        (f"/api/v1/projects/{check.PROJECT}", {"project_id": "sample", "runs": [sample, compute]}),
        (f"/api/v1/projects/{check.PROJECT}/runs", {"runs": [compute, sample]}),
    ):
        normalized, applied = check._normalize(route, document)
        assert normalized == {**document, "runs": [sample]}
        assert applied == [check.NORMALIZED["compute_run"]]
        assert len(document["runs"]) == 2  # the browser's answer itself is not changed


def test_other_runs_and_routes_are_compared_as_they_are() -> None:
    check = _site_check()
    other = _run("run-other")
    document = {"runs": [_run(check.RUN), other]}
    # No compute Run: nothing applied, an extra Run still differs from Python's list.
    assert check._normalize(f"/api/v1/projects/{check.PROJECT}/runs", document) == (document, [])
    found: list[tuple[object, ...]] = []
    check._differences({"runs": [_run(check.RUN)]}, document, (), found)
    assert found == [(("runs", "length"), 1, 2)]
    # Only the sample Project's record and Run list are normalized.
    listed = {"runs": [_run(check.COMPUTE_RUN)]}
    for route in (check.RUN_PATH, "/api/v1/projects", "/api/v1/projects/other/runs"):
        assert check._normalize(route, listed) == (listed, [])
    assert check._normalize(f"/api/v1/projects/{check.PROJECT}", ["not", "a", "dict"]) == (
        ["not", "a", "dict"],
        [],
    )
