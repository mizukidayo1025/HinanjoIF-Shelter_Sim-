"""The Project API's checks before a computation starts, a computation that fails on a corrupt
stored Run, the end of a job a client may see (format v6, #427), and a new Run's creation time
(#447)."""

from __future__ import annotations

import json
import re
from datetime import UTC, datetime, timedelta, timezone
from pathlib import Path

import pytest

from sheltersim.storage.projects import StorageError
from sheltersim.web.projects import ProjectAPI
from sheltersim.web.stored_runs import created_at_of


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


def test_a_new_run_keeps_its_creation_time_from_the_api_s_clock(
    tmp_path, monkeypatch, saved_project
):
    """`POST runs` records when the Run was made (#447), UTC to the second, from the clock the API
    is given; the Run made without one has the time it was made."""
    monkeypatch.setenv("SHELTERSIM_LLM_MODE", "offline")
    tokyo = timezone(timedelta(hours=9))
    api = ProjectAPI(
        saved_project(tmp_path), clock=lambda: datetime(2026, 11, 15, 10, 30, 59, 999, tokyo)
    )
    try:
        first = api.get(["p", "runs", "r"])
        assert re.fullmatch(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ", first["created_at"])
        api.post(
            ["p", "runs"],
            {"run_id": "r2", "scenario_id": first["scenario_id"], "scenario_version": 1},
        )
        second = api.get(["p", "runs", "r2"])
        assert (second["schema_version"], second["created_at"]) == (2, "2026-11-15T01:30:59Z")
    finally:
        api.close()
    assert created_at_of(datetime(2026, 1, 2, 3, 4, 5)) == "2026-01-02T03:04:05Z"
    assert created_at_of(datetime(2026, 1, 2, 3, 4, 5, tzinfo=UTC)) == "2026-01-02T03:04:05Z"


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
