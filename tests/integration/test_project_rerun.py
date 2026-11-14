"""A rerun of a saved Run (`POST …/runs/{run}/rerun`, as the CLI's `project rerun` sends it)
reproduces its world."""

from __future__ import annotations

import copy
import json

import pytest

from sheltersim.experiments.scenario import ScenarioSpec
from sheltersim.experiments.timeline import TimelineStore
from sheltersim.web.projects import ProjectAPI

END = 30


@pytest.fixture
def project_api(tmp_path, monkeypatch, generated_previews, saved_timeline):
    """A ProjectAPI over Run r of the generated Scenario, not yet computed."""
    monkeypatch.setenv("SHELTERSIM_LLM_MODE", "offline")
    preview = generated_previews(duration_days=END / 1440)
    scenario = ScenarioSpec.model_validate(copy.deepcopy(preview["scenario"]))
    store, _ = saved_timeline(tmp_path / "projects", scenario)
    api = ProjectAPI(store)
    try:
        yield api
    finally:
        api.close()


def _compute(api: ProjectAPI, run_id: str) -> None:
    api.post(["p", "runs", run_id, "resume"], {"branch_id": "main", "target_minute": END})
    # The worker is single; this waits for the resume queued above (#143).
    api.wait_idle(timeout=120)
    assert api.get(["p", "runs", run_id], {})["status"] == "completed"


def test_rerun_of_a_saved_run_reproduces_its_world(project_api):
    api = project_api
    _compute(api, "r")

    api.post(["p", "runs", "r", "rerun"], {"new_run_id": "r2"})
    _compute(api, "r2")

    def world(run_id: str) -> str:
        document = TimelineStore(api.store, "p", run_id).seek("main", END).world.to_dict()
        return json.dumps(document, sort_keys=True)

    assert world("r2") == world("r")
