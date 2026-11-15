"""Compute one branch of a Run in its own process (#48), into its stored Run of format v5 (#427).

The server process keeps answering HTTP requests without sharing the GIL with the simulation,
and the memory a computation used returns to the OS when it ends. The computation is the core's
(`sheltersim_native.rules.run_files`: the branch cut back to its head, restored there and computed
in chunks, a saved point every 360 minutes); this module starts it, stops it when asked and sets
the branch's final status.
"""

from __future__ import annotations

import contextlib
import json
import multiprocessing
from collections.abc import Callable
from multiprocessing.connection import Connection
from multiprocessing.context import SpawnProcess
from multiprocessing.synchronize import Event
from typing import Any

from sheltersim.storage.projects import DEFAULT_MAX_BYTES, ProjectStore, StorageError

# Spawn starts a clean interpreter on every platform; forking a process that runs
# server threads could copy locks held by those threads.
_CONTEXT = multiprocessing.get_context("spawn")
# Starting the interpreter and importing the simulation takes well under a second;
# the bound only ends a start that hangs.
_START_TIMEOUT_SECONDS = 120.0
# Ticks per chunk: the computation stops between chunks when asked (about a tenth of a second
# for 100 people), and holds the store's lock while a chunk may move the branch's head.
CHUNK_TICKS = 60


def start_worker(
    store: ProjectStore, job: dict[str, Any]
) -> tuple[SpawnProcess, Event, Connection]:
    """Start a worker and return it once it owns the branch's writer lease.

    job is {"project_id", "run_id", "branch_id", "target_minute"}. The worker claims the lease
    itself, so a server that stops abnormally never leaves a job writing without one. A branch
    another process writes is refused with the same error as before. The returned connection
    reaches end of file when the worker ends.
    """
    cancel = _CONTEXT.Event()
    receiver, sender = _CONTEXT.Pipe(duplex=False)
    process = _CONTEXT.Process(
        target=run_worker,
        args=(str(store.root), store.max_bytes, job, cancel, sender),
        name=f"sheltersim-advance-{job['run_id']}",
    )
    process.start()
    sender.close()
    try:
        if not receiver.poll(_START_TIMEOUT_SECONDS):
            process.kill()
            process.join()
            raise StorageError("run_worker_start_timeout")
        try:
            state, detail = receiver.recv()
        except EOFError:
            process.join()
            raise StorageError("run_worker_start_failed") from None
    except BaseException:
        receiver.close()
        raise
    if state != "running":
        receiver.close()
        process.join()
        raise StorageError(detail)
    return process, cancel, receiver


def run_worker(
    root: str, max_bytes: int, job: dict[str, Any], cancel: Event, outcome: Connection
) -> None:
    from sheltersim.web.stored_runs import FILES

    project_id, run_id, branch_id = job["project_id"], job["run_id"], job["branch_id"]
    try:
        files = FILES.ProjectFiles(root, max_bytes)
        lease = files.claim_branch(project_id, run_id, branch_id)
    except ValueError as exc:
        outcome.send(("refused", str(exc)))
        return
    try:
        with files.transaction():
            files.set_branch_status(project_id, run_id, branch_id, "running")
    except BaseException as exc:
        lease.release()
        outcome.send(("refused", str(exc) if isinstance(exc, ValueError) else "run_worker_failed"))
        return
    outcome.send(("running", None))
    parent: Any = multiprocessing.parent_process()

    def stopped() -> bool:
        # A server that is gone can no longer cancel; stop at the next boundary.
        return cancel.is_set() or (parent is not None and not parent.is_alive())

    try:
        finish(files, lease, project_id, run_id, branch_id, job["target_minute"], stopped)
    finally:
        outcome.close()


def finish(
    files: Any,
    lease: Any,
    project_id: str,
    run_id: str,
    branch_id: str,
    target_minute: int,
    cancelled: Callable[[], bool],
    *,
    lock: Any = None,
    ended: Callable[[], None] | None = None,
) -> str:
    """Computes the branch (`compute`), writes its final status and releases its lease: a
    computation that fails leaves the branch `failed` at its last head. The status, `ended` (the
    thread worker forgets its job) and the release happen together under `lock`, the lock a
    submit takes, so a client that sees the status may start the next computation."""
    status = "failed"
    try:
        status = compute(files, lease, project_id, run_id, branch_id, target_minute, cancelled)
        if status == "completed":
            # The review is a part of the report, not of the result: a failure keeps the
            # branch completed, and its report is then the rule fallback.
            with contextlib.suppress(Exception):
                review(files, project_id, run_id, branch_id)
    except Exception:
        # A Provider's error may hold secrets; the branch is only marked failed.
        status = "failed"
    finally:
        with lock if lock is not None else contextlib.nullcontext():
            try:
                with files.transaction():
                    files.set_branch_status(project_id, run_id, branch_id, status)
            finally:
                if ended is not None:
                    ended()
                lease.release()
    return status


def compute(
    files: Any,
    lease: Any,
    project_id: str,
    run_id: str,
    branch_id: str,
    target_minute: int,
    cancelled: Callable[[], bool],
) -> str:
    """Computes a branch from its head toward target_minute with its writer lease: the status it
    ends with ("completed" at the end of the clock, "paused" at the target or when cancelled)."""
    with files.transaction():
        computation = files.go_on(lease, project_id, run_id, branch_id, target_minute)
    link = runtime_link(files, project_id, run_id, branch_id)
    if link is not None:
        computation.set_llm(link)
    while True:
        if cancelled():
            with files.transaction():
                return computation.stop(files)
        with files.transaction():
            ended = computation.chunk(files, CHUNK_TICKS)
        if ended is not None:
            return ended


def review(files: Any, project_id: str, run_id: str, branch_id: str) -> None:
    """The review of a branch the Run's LLM has just completed (#463, Python's `finish_review`):
    the core builds the task from the Run at its end, the Run's router answers it, the core
    checks the codes against the evidence, and the report is kept. A Run without a Provider (the
    screen's offline Run, an offline experiment) gets its report when it is first read, without
    an LLM; so does this one where the review was not kept."""
    from sheltersim.llm.config import LLMSettings
    from sheltersim.llm.link import analysis_link, ask
    from sheltersim.web.projects import answers
    from sheltersim.web.stored_runs import llm_settings

    store = ProjectStore(files.root(), max_bytes=files.limit() or DEFAULT_MAX_BYTES)
    saved = llm_settings(store, project_id, run_id)
    if saved is None or saved["mode"] == "offline":
        return
    if files.branch_file(project_id, run_id, branch_id, "report.json") is not None:
        return
    settings = LLMSettings.from_env()
    if settings.effective_mode.value != saved["mode"]:
        return
    link = analysis_link(
        store, project_id, run_id, operation="report", branch_id=branch_id, settings=settings
    )
    if link is None:
        return
    record = json.loads(files.branch_record(project_id, run_id, branch_id))
    run, _, _ = files.reader(project_id, run_id, branch_id).seek(record["computed_until"])
    reviewing = answers().review(run, files.run_scenario_bytes(project_id, run_id), branch_id)
    report = reviewing.report(ask(link, reviewing.task()))
    with files.transaction():
        files.keep_report(project_id, run_id, branch_id, report + b"\n")


def runtime_link(files: Any, project_id: str, run_id: str, branch_id: str) -> Any:
    """The LLM link a Run computes with (#428): None for the screen's offline Run and an
    offline experiment; else the configured router behind the core's port (`RouterLink`), its
    calls counted in the Run's usage account. A Run computed with a Provider goes on only with
    the same LLM mode configured."""
    from sheltersim.llm.accounting import accounted_router
    from sheltersim.llm.config import LLMSettings
    from sheltersim.llm.link import RouterLink
    from sheltersim.web.stored_runs import llm_settings

    store = ProjectStore(files.root(), max_bytes=files.limit() or DEFAULT_MAX_BYTES)
    saved = llm_settings(store, project_id, run_id)
    if saved is None or saved["mode"] == "offline":
        return None
    settings = LLMSettings.from_env()
    if settings.effective_mode.value != saved["mode"]:
        raise StorageError("saved LLM mode unavailable; restore original configuration")
    router = accounted_router(
        store,
        project_id,
        run_id=run_id,
        operation="runtime",
        branch_id=branch_id,
        settings=settings,
    )
    return RouterLink(router)
