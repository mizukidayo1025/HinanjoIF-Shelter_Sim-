"""Measuring a Run's computations: a resume with `measure_performance` records each execution
beside the Run, never in its Checkpoints.

The telemetry routes and the screen's render reports (FPS) went with the older screens (#376).
"""

from __future__ import annotations

import copy

import pytest

from sheltersim.experiments.scenario import ScenarioSpec
from sheltersim.storage.projects import StorageError
from sheltersim.storage.telemetry import RunTelemetryStore
from sheltersim.web.projects import ProjectAPI

END = 60


@pytest.fixture
def run_api(tmp_path, monkeypatch, generated_previews, saved_timeline):
    """A ProjectAPI over Run r of a one-hour generated Scenario, not yet computed."""
    monkeypatch.setenv("SHELTERSIM_LLM_MODE", "offline")
    preview = generated_previews(duration_days=END / 1440)
    scenario = ScenarioSpec.model_validate(copy.deepcopy(preview["scenario"]))
    store, _ = saved_timeline(tmp_path / "projects", scenario)
    api = ProjectAPI(store)
    try:
        yield api
    finally:
        api.close()


def _compute(api: ProjectAPI, minute: int, *, measure: bool) -> None:
    api.post(
        ["p", "runs", "r", "resume"],
        {"branch_id": "main", "target_minute": minute, "measure_performance": measure},
    )
    # The worker is single; this waits for the resume queued above (#143).
    api.wait_idle(timeout=120)


def test_measured_computations_are_recorded_beside_the_run(run_api):
    empty = RunTelemetryStore(run_api.store, "p", "r").read()
    _compute(run_api, 20, measure=True)
    _compute(run_api, 40, measure=False)
    _compute(run_api, END, measure=True)

    telemetry = RunTelemetryStore(run_api.store, "p", "r").read()

    assert (empty["execution_count"], empty["executions"]) == (0, [])
    assert telemetry["execution_count"] == 2, "only the measured computations"
    executions = telemetry["executions"]
    assert [(row["from_minute"], row["computed_until"]) for row in executions] == [
        (0, 20),
        (40, 60),
    ]
    assert [row["status"] for row in executions] == ["paused", "completed"]
    assert all(row["wall_seconds"] > 0 and row["checkpoint_write_bytes"] > 0 for row in executions)
    # Runs share the server process, so their memory cannot be told apart.
    assert {row["memory_attribution_status"] for row in executions} == {"unavailable"}
    assert telemetry["totals"]["checkpoint_write_bytes"] == sum(
        row["checkpoint_write_bytes"] for row in executions
    )


def test_the_telemetry_routes_are_gone(run_api):
    with pytest.raises(StorageError, match="unknown project route"):
        run_api.get(["p", "runs", "r", "telemetry"], {})
    with pytest.raises(StorageError, match="unknown project operation"):
        run_api.post(
            ["p", "runs", "r", "telemetry"],
            {
                "kind": "render",
                "branch_id": "main",
                "samples": 1,
                "fps": 60,
                "p95_frame_ms": 20,
                "visibility": "visible",
            },
        )
