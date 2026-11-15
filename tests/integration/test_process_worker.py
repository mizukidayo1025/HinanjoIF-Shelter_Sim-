"""A computation run in its own process (#48) stores what a worker thread stores (format v5,
#427), can be cancelled, refuses a branch another process writes, leaves a killed computation
interrupted, and runs beside other Runs' and branches' computations up to the limit; a fork is
made at once while another branch computes."""

from __future__ import annotations

import json
import multiprocessing
import time

import pytest

from sheltersim.storage.projects import StorageError, digest
from sheltersim.web.projects import ProjectAPI

# Run on every pull request on Windows and macOS too (pytest --os-subset, #380): computations run
# in processes started by spawn.
pytestmark = pytest.mark.os_dependent


def _branch(api: ProjectAPI, run: str, branch: str) -> dict:
    return json.loads(api.files.branch_record("p", run, branch))


def _run(api: ProjectAPI, run: str = "r") -> dict:
    return api.get(["p", "runs", run])


def _duration(api: ProjectAPI) -> int:
    return json.loads(api.files.run_scenario_bytes("p", "r"))["duration_minutes"]


def _computed(tmp_path, saved_project, worker, target):
    store = saved_project(tmp_path)
    api = ProjectAPI(store, recover=False, worker=worker)
    try:
        api.submit("p", "r", "main", target)
        api.wait_idle(timeout=300)
        return _run(api), _branch(api, "r", "main")
    finally:
        api.close()


def test_a_process_stores_the_same_head_as_a_thread(tmp_path, monkeypatch, saved_project):
    monkeypatch.setenv("SHELTERSIM_LLM_MODE", "offline")
    thread_run, thread = _computed(tmp_path / "thread", saved_project, "thread", 30)
    process_run, process = _computed(tmp_path / "process", saved_project, "process", 30)
    assert process_run["status"] == thread_run["status"] == "paused"
    assert (process["computed_until"], process["checkpoint_hash"]) == (
        30,
        thread["checkpoint_hash"],
    )


def test_a_process_run_can_be_cancelled(tmp_path, monkeypatch, saved_project):
    monkeypatch.setenv("SHELTERSIM_LLM_MODE", "offline")
    api = ProjectAPI(saved_project(tmp_path), recover=False, worker="process")
    try:
        duration = _duration(api)
        api.submit("p", "r", "main", duration)
        assert _run(api)["status"] == "running"
        with pytest.raises(StorageError, match="busy"):
            api.submit("p", "r", "main", duration)
        api.post(["p", "runs", "r", "cancel"], {})
        api.wait_idle(timeout=300)
        assert _run(api)["status"] == "paused"
        assert _branch(api, "r", "main")["computed_until"] < duration
    finally:
        api.close()


def test_a_branch_another_process_writes_is_refused(tmp_path, monkeypatch, saved_project):
    monkeypatch.setenv("SHELTERSIM_LLM_MODE", "offline")
    api = ProjectAPI(saved_project(tmp_path), recover=False, worker="process")
    try:
        status = _run(api)["status"]
        lease = api.files.claim_branch("p", "r", "main")
        with pytest.raises(StorageError, match="run_worker_busy"):
            api.submit("p", "r", "main", 30)
        assert _run(api)["status"] == status
        assert not multiprocessing.active_children()
        lease.release()
        # Once the lease is free the branch computes.
        api.submit("p", "r", "main", 30)
        api.wait_idle(timeout=300)
        assert _branch(api, "r", "main")["computed_until"] == 30
    finally:
        api.close()


def test_a_killed_process_leaves_the_run_interrupted(tmp_path, monkeypatch, saved_project):
    monkeypatch.setenv("SHELTERSIM_LLM_MODE", "offline")
    api = ProjectAPI(saved_project(tmp_path), recover=False, worker="process")
    try:
        api.submit("p", "r", "main", _duration(api))
        (worker,) = multiprocessing.active_children()
        worker.kill()
        api.wait_idle(timeout=300)
        assert _run(api)["status"] == "interrupted"
        head = _branch(api, "r", "main")["computed_until"]
        # The killed process held the lease; it ended with the process.
        api.submit("p", "r", "main", head + 30)
        api.wait_idle(timeout=300)
        assert _branch(api, "r", "main")["computed_until"] == head + 30
    finally:
        api.close()


def _second_run(api: ProjectAPI) -> None:
    """A second Run of the same Scenario beside Run r, at minute 0."""
    run = _run(api)
    api.post(
        ["p", "runs"],
        {"run_id": "r2", "scenario_id": run["scenario_id"], "scenario_version": 1},
    )


def _fork(api: ProjectAPI, run: str, parent: str, child: str, minute: int, program) -> None:
    document = program(start_minute=minute).model_dump(mode="json")
    text = json.dumps(document, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
    api.fork("p", run, parent, child, minute, text, digest(document))


def test_two_runs_compute_at_once_up_to_the_limit(tmp_path, monkeypatch, saved_project):
    """Different Runs compute in parallel processes, up to max_jobs (#48).

    The check that both compute at once asks each for far more than the time it takes to start
    the other process, then cancels them, so it does not depend on how fast either computes.
    """
    monkeypatch.setenv("SHELTERSIM_LLM_MODE", "offline")
    store = saved_project(tmp_path)
    api = ProjectAPI(store, recover=False, worker="process", max_jobs=2)
    try:
        _second_run(api)
        duration = _duration(api)
        api.submit("p", "r", "main", 30)
        api.submit("p", "r2", "main", 30)
        api.wait_idle(timeout=300)
        heads = {_branch(api, run, "main")["checkpoint_hash"] for run in ("r", "r2")}
        assert len(heads) == 1
        assert {_branch(api, run, "main")["computed_until"] for run in ("r", "r2")} == {30}
        api.submit("p", "r", "main", duration)
        api.submit("p", "r2", "main", duration)
        assert {_run(api, run)["status"] for run in ("r", "r2")} == {"running"}
        assert len(multiprocessing.active_children()) == 2
        api.post(["p", "runs", "r", "cancel"], {})
        api.post(["p", "runs", "r2", "cancel"], {})
        api.wait_idle(timeout=300)
    finally:
        api.close()
    api = ProjectAPI(store, recover=False, worker="process", max_jobs=1)
    try:
        api.submit("p", "r", "main", duration)
        with pytest.raises(StorageError, match="workers are busy"):
            api.submit("p", "r2", "main", 60)
        assert _run(api, "r2")["status"] == "paused"
    finally:
        api.close()


def test_two_branches_of_a_run_compute_at_once_and_a_fork_waits_for_its_source(
    tmp_path, monkeypatch, saved_project, add_toilets_program
):
    """Branches of one Run compute in parallel processes; the Run is running while either is and
    each branch shows its own status (#48). A fork needs only its source branch to be still, and
    cancelling one branch leaves the other computing."""
    monkeypatch.setenv("SHELTERSIM_LLM_MODE", "offline")
    api = ProjectAPI(saved_project(tmp_path), recover=False, worker="process", max_jobs=3)
    try:
        _fork(api, "r", "main", "child", 0, add_toilets_program)
        # Each branch is asked for the whole week, far more than this test takes before it
        # cancels them, so the check does not depend on how fast either computes.
        duration = _duration(api)
        api.submit("p", "r", "main", duration)
        api.submit("p", "r", "child", duration)
        assert len(multiprocessing.active_children()) == 2
        run = _run(api)
        assert run["status"] == "running"
        statuses = {branch["branch_id"]: branch["status"] for branch in run["branches"]}
        assert statuses == {"main": "running", "child": "running"}
        with pytest.raises(StorageError, match="pause background calculation"):
            _fork(api, "r", "main", "c2", 0, add_toilets_program)
        api.post(["p", "runs", "r", "cancel"], {"branch_id": "main"})
        # The branch's job ends once its process has ended, after it saved its status.
        deadline = time.monotonic() + 300
        while api.busy("p", "r", "main") and time.monotonic() < deadline:
            time.sleep(0.1)
        assert _branch(api, "r", "main")["status"] == "paused"
        assert _branch(api, "r", "child")["status"] == "running"
        assert _run(api)["status"] == "running"
        _fork(api, "r", "main", "c2", 0, add_toilets_program)
        api.post(["p", "runs", "r", "cancel"], {})
        api.wait_idle(timeout=300)
        run = _run(api)
    finally:
        api.close()
    assert run["status"] == "paused"
    statuses = {branch["branch_id"]: branch["status"] for branch in run["branches"]}
    assert statuses == {"main": "paused", "child": "paused", "c2": "paused"}
    # Each cancelled branch stopped before its target.
    assert all(branch["computed_until"] < duration for branch in run["branches"])
