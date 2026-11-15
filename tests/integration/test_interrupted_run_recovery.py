"""A Run that was computing when the server stopped is marked interrupted at the next start, and
computing it again reaches the same world as an uninterrupted Run (format v6, #427).
"""

from __future__ import annotations

import copy
import json

import pytest

from sheltersim.experiments.scenario import ScenarioSpec
from sheltersim.web.projects import ProjectAPI

# Run on every pull request on Windows and macOS too (pytest --os-subset, #380): the leases and
# locks of a Run are files.
pytestmark = pytest.mark.os_dependent

END = 60
STOPPED_AT = 20


def _end_digests(api: ProjectAPI) -> str:
    run, _, _ = api.files.reader("p", "r", "main").seek(END)
    return run.digests_json()


def test_an_interrupted_run_resumes_to_the_uninterrupted_world(
    tmp_path, monkeypatch, generated_previews, saved_project
):
    monkeypatch.setenv("SHELTERSIM_LLM_MODE", "offline")
    preview = generated_previews(duration_days=END / 1440)
    scenario = ScenarioSpec.model_validate(copy.deepcopy(preview["scenario"]))
    store = saved_project(tmp_path / "stopped", scenario)
    api = ProjectAPI(store, recover=False)
    try:
        api.submit("p", "r", "main", STOPPED_AT)
        api.wait_idle(timeout=120)
        # The server stopped while this Run was computing.
        with api.files.transaction():
            api.files.set_branch_status("p", "r", "main", "running")
    finally:
        api.close()

    api = ProjectAPI(store)
    try:
        restarted = api.get(["p", "runs", "r"])["status"]
        api.post(["p", "runs", "r", "resume"], {"branch_id": "main", "target_minute": END})
        api.wait_idle(timeout=120)
        finished = api.get(["p", "runs", "r"])["status"]
        stopped = _end_digests(api)
    finally:
        api.close()
    straight = ProjectAPI(saved_project(tmp_path / "uninterrupted", scenario), recover=False)
    try:
        straight.submit("p", "r", "main", END)
        straight.wait_idle(timeout=120)
        uninterrupted = _end_digests(straight)
    finally:
        straight.close()

    assert restarted == "interrupted"
    assert finished == "completed"
    assert json.loads(stopped) == json.loads(uninterrupted)
