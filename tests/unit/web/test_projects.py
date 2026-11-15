"""The Project API's checks before a computation starts, a computation that fails on a corrupt
stored Run, and the end of a job a client may see (format v5, #427)."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from sheltersim.storage.projects import StorageError
from sheltersim.web.projects import ProjectAPI


def _record(api: ProjectAPI) -> dict:
    return json.loads(api.files.branch_record("p", "r", "main"))


def test_resume_is_refused_outside_the_uncomputed_range(tmp_path, monkeypatch, saved_project):
    monkeypatch.setenv("SHELTERSIM_LLM_MODE", "offline")
    api = ProjectAPI(saved_project(tmp_path))
    try:
        status = api.get(["p", "runs", "r"])["status"]
        duration = json.loads(api.files.run_scenario_bytes("p", "r"))["duration_minutes"]
        for target in (0, duration + 1, "5"):
            with pytest.raises(StorageError, match="uncomputed remaining range"):
                api.submit("p", "r", "main", target)
        with pytest.raises(StorageError, match="saved artifact is missing"):
            api.submit("p", "r", "missing", 5)
        assert api.get(["p", "runs", "r"])["status"] == status
    finally:
        api.close()


def test_a_corrupt_stored_run_fails_the_computation_at_its_head(
    tmp_path, monkeypatch, saved_project
):
    monkeypatch.setenv("SHELTERSIM_LLM_MODE", "offline")
    api = ProjectAPI(saved_project(tmp_path))
    try:
        head = _record(api)["checkpoint_hash"]
        data = Path(api.files.branch_folder("p", "r", "main")) / "data.pack"
        data.write_bytes(b"corrupt" + data.read_bytes()[7:])
        api.submit("p", "r", "main", 5)
        api.wait_idle(timeout=60)
        assert api.get(["p", "runs", "r"])["status"] == "failed"
        assert _record(api)["checkpoint_hash"] == head
    finally:
        api.close()


def test_a_client_that_sees_a_job_end_may_start_the_next_one(tmp_path, monkeypatch, saved_project):
    """The final status, the lease and the job end together, so a submit right after the status
    leaves "running" is not refused as busy."""
    import time as clock

    monkeypatch.setenv("SHELTERSIM_LLM_MODE", "offline")
    api = ProjectAPI(saved_project(tmp_path))
    try:
        api.submit("p", "r", "main", 5)
        deadline = clock.monotonic() + 60
        while api.get(["p", "runs", "r"])["status"] == "running":
            assert clock.monotonic() < deadline
            clock.sleep(0.001)
        assert json.loads(api.submit("p", "r", "main", 10))["status"] == "running"
        api.wait_idle(timeout=60)
        assert _record(api)["computed_until"] == 10
    finally:
        api.close()
