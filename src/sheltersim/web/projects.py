"""The Project API of the local and VM versions: the screen's routes under /api/v1/projects,
answered from the stored Runs of format v6 as the browser answers them (#427).

The records and each branch's stored Run are in the browser's folders
(native/wasm/js/project_store.mjs), written and read by the store in Rust
(`sheltersim_native.rules.run_files`, native/store, #426). The answers are the core's
(`sheltersim_native.rules.answers`, #449), the functions the browser calls through WASM
(native/wasm/js/browser_api.mjs), so both give the same bytes. What stays here is what stays in
the browser's JS: routing a request to its answer, reading the files an answer needs, the order of
the computations, and which Runs are kept at which minute. Python also makes what only it makes:
a Scenario from text (`scenario-preview`, web/workspace.py) and the world a Run starts from
(web/stored_runs.py).

Computing. A branch computes from its head in chunks, a saved point every 360 minutes
(web/worker_process.py): in a process of its own (the server, up to max_jobs at once across Runs
and their branches, #48) or on a thread of this process (tests and the CLI, one at a time). A
fork is made at once in the caller, as the browser makes it: the parent at the minute, the program
applied, the child's stored Run (which reads the parent's pieces up to its base).

The Runs at a minute (`LiveRuns`): a branch keeps one Run; a later minute steps it forward when
that is cheaper than restoring the nearest saved point, any other minute seeks, as the browser's
`BrowserApi.simulationAt` does. A Run stays on the thread that made it: the server answers on one
thread.

A stored Run of an older format (the Python server's Checkpoints before #427, the browser's v1 to
v4) is not read: 409 `stored_run_outdated`, compute it again.
"""

from __future__ import annotations

import json
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from typing import Any
from urllib.parse import parse_qs, urlencode

from sheltersim import native
from sheltersim.storage.projects import StorageError
from sheltersim.web.stored_runs import FILES, project_files, storage_error

_JSON: dict[str, Any] = {
    "separators": (",", ":"),
    "sort_keys": True,
    "ensure_ascii": False,
    "allow_nan": False,
}


def answers() -> Any:
    """The core's answers (`sheltersim_native.rules.answers`)."""
    return native.rules("answers")


def fields(document: dict[str, Any], required: set[str], optional: set[str] | None = None) -> None:
    if set(document) - required - (optional or set()) or required - set(document):
        raise StorageError("request fields do not match this operation")


def json_bytes(value: Any) -> bytes:
    """An answer Python writes, as the server writes JSON."""
    return json.dumps(value, **_JSON).encode()


def _refusals(method: Any) -> Any:
    """A method of the API whose refusals of the files or the core are raised as the server
    answers them (`_refusal`), for callers in Python (the CLI, tests)."""
    import functools

    @functools.wraps(method)
    def call(*values: Any, **options: Any) -> Any:
        try:
            return method(*values, **options)
        except ValueError as exc:
            raise _refusal(exc) from None

    return call


@dataclass
class _Job:
    """A computation running for one branch."""

    branch_id: str
    cancel: Any
    # In process mode, the thread that waits for the job's process to end.
    watcher: threading.Thread | None = None


@dataclass
class _Live:
    """A branch's Run kept at a minute (`LiveRuns`)."""

    run: Any
    head: str
    saved: list[int]
    restored_at: int | None
    used: int


@dataclass
class LiveRuns:
    """The Runs the API keeps at a minute, at most `keep` of them (the browser's
    `BrowserApi.live`): a later minute of a kept Run is stepped to when that costs no more ticks
    than restoring the nearest saved point; any other minute is sought (`StoredRun.seek`). The
    least recently used Run goes first."""

    files: Any
    keep: int = 2
    live: dict[tuple[str, str, str], _Live] = field(default_factory=dict)
    used: int = 0

    def at(
        self,
        project_id: str,
        run_id: str,
        record: dict[str, Any],
        minute: int,
        *,
        exact: bool = False,
    ) -> Any:
        """The branch's Run at `minute`. With `exact` (a saved point of a Run of other model
        versions, which is not computed again) the kept Run is used only when it is the saved
        point as restored."""
        key = (project_id, run_id, record["branch_id"])
        live = self.live.get(key)
        if live is not None and (
            live.head != record["checkpoint_hash"]
            or (exact and not (live.restored_at == minute and live.run.minute() == minute))
        ):
            del self.live[key]
            live = None
        self.used += 1
        if live is not None:
            live.used = self.used
            now = live.run.minute()
            if now == minute:
                return live.run
            point = max([0, *(saved for saved in live.saved if saved <= minute)])
            live.restored_at = None
            if now < minute and (now >= point or minute - now <= FILES.SAVE_EVERY):
                if live.run.advance(minute - now) != minute - now:
                    raise StorageError(f"the Run does not reach minute {minute}")
                return live.run
            del self.live[key]
        while len(self.live) >= self.keep:
            del self.live[min(self.live, key=lambda each: self.live[each].used)]
        reader = self.files.reader(project_id, run_id, record["branch_id"])
        run, start, _ = reader.seek(minute)
        saved = [
            head["minute"] for head in json.loads(reader.heads_json()) if head["kind"] != "end"
        ]
        restored = minute if start == minute else None
        self.live[key] = _Live(run, record["checkpoint_hash"], saved, restored, self.used)
        return run

    def forget(self, project_id: str, run_id: str) -> None:
        for key in [key for key in self.live if key[:2] == (project_id, run_id)]:
            del self.live[key]

    def clear(self) -> None:
        self.live.clear()


class ProjectAPI:
    """The Project routes of the server, the CLI and the tests."""

    def __init__(
        self,
        store: Any,
        *,
        recover: bool = True,
        worker: str = "thread",
        max_jobs: int = 1,
        keep: int = 2,
    ) -> None:
        """worker="process" computes each branch in its own process (the server, #48), up to
        max_jobs at once, across Runs and their branches; "thread" computes one branch at a time
        on a thread of this process (tests and CLI). `keep` Runs are kept at a minute."""
        if worker not in {"thread", "process"}:
            raise ValueError("worker must be 'thread' or 'process'")
        if type(max_jobs) is not int or max_jobs < 1:
            raise ValueError("max_jobs must be a positive integer")
        self.store = store
        self.files = project_files(store)
        if recover:
            with self.files.transaction():
                self.files.recover_interrupted()
        self._worker = worker
        # A thread shares the GIL, so only processes compute Runs in parallel.
        self._max_jobs = max_jobs if worker == "process" else 1
        self._pool = ThreadPoolExecutor(max_workers=1, thread_name_prefix="sheltersim-project")
        self._lock = threading.RLock()
        # The job of each branch being computed, by (project, run, branch).
        self._jobs: dict[tuple[str, str, str], _Job] = {}
        # The outcome of the last fork of each Run in this process: its JSON bytes.
        self._fork_outcomes: dict[tuple[str, str], bytes] = {}
        self._closed = False
        self.live = LiveRuns(self.files, keep)
        from sheltersim.web.workspace import ProjectWorkspace

        self.workspace = ProjectWorkspace(self)
        from sheltersim.web.pdf_report import PdfReports

        self.pdf_reports = PdfReports(self)
        from sheltersim.web.project_services import ProjectServices

        self.services = ProjectServices(self)

    def close(self) -> None:
        with self._lock:
            self._closed = True
            jobs = list(self._jobs.values())
            for job in jobs:
                job.cancel.set()
        for job in jobs:
            if job.watcher is not None:
                job.watcher.join()
        self._pool.shutdown(wait=True)
        self.live.clear()

    def wait_idle(self, timeout: float | None = None) -> None:
        """Wait until the computations submitted so far have ended."""
        if self._worker == "thread":
            self._pool.submit(lambda: None).result(timeout=timeout)
            return
        with self._lock:
            watchers = [job.watcher for job in self._jobs.values() if job.watcher is not None]
        deadline = None if timeout is None else time.monotonic() + timeout
        for watcher in watchers:
            watcher.join(None if deadline is None else max(0.0, deadline - time.monotonic()))
            if watcher.is_alive():
                raise TimeoutError("the background job is still running")

    # ---- Requests ------------------------------------------------------------------------------

    def get(self, parts: list[str], query: dict[str, list[str]] | str | None = None) -> Any:
        """A GET's answer as Python values (`query` as `parse_qs` gives it, or its text)."""
        text = urlencode(query, doseq=True) if isinstance(query, dict) else (query or "")
        return json.loads(self.answer_get(parts, text))

    def post(self, parts: list[str], document: dict[str, Any]) -> Any:
        """A POST's answer as Python values."""
        return json.loads(self.answer_post(parts, json_bytes(document)))

    def answer_get(self, parts: list[str], query: str) -> bytes:
        """The JSON bytes of the answer to `GET /api/v1/projects/<parts>?<query>`. A refusal
        raises the core's `Refused`, a StorageError (409) or a ValueError (400)."""
        try:
            return self._get(parts, query)
        except ValueError as exc:
            raise _refusal(exc) from None

    def answer_post(self, parts: list[str], body: bytes) -> bytes:
        """The JSON bytes of the answer to `POST /api/v1/projects/<parts>` with `body`."""
        try:
            return self._post(parts, body)
        except ValueError as exc:
            raise _refusal(exc) from None

    # ---- GET -----------------------------------------------------------------------------------

    def _get(self, parts: list[str], query: str) -> bytes:
        core = answers()
        files = self.files
        no_query = query == ""
        if not parts:
            if not no_query:
                raise StorageError("projects list takes no query parameters")
            used, count = files.usage()
            return core.projects_answer(
                files.projects_listing(), used, count, float(self.store.max_bytes)
            )
        shared = self.services.get(parts, query)
        if shared is not None:
            return shared
        project_id = parts[0]
        if len(parts) == 1 and no_query:
            project, scenarios, runs = files.project_listing(project_id, True)
            return core.project_answer(project_id, project, scenarios, runs)
        if len(parts) == 2 and parts[1] == "runs" and no_query:
            project, _, runs = files.project_listing(project_id, False)
            return core.runs_answer(project_id, project, runs)
        if len(parts) >= 3 and parts[1] == "runs":
            run_id = parts[2]
            if len(parts) == 3 and no_query:
                with self._lock:
                    outcome = self._fork_outcomes.get((project_id, run_id))
                return core.run_answer(
                    project_id, run_id, *files.run_listing(project_id, run_id), outcome
                )
            if len(parts) == 4 and parts[3] == "evidence" and no_query:
                return core.evidence_answer(
                    project_id,
                    run_id,
                    files.run_file(project_id, run_id, "run.json"),
                    files.run_file(project_id, run_id, "evidence.json"),
                )
            if len(parts) == 4 and parts[3] == "replay-frames":
                return self._replay(project_id, run_id, query)
            if (
                len(parts) == 4
                and parts[3] == "compare"
                and set(parse_qs(query))
                == {
                    "branch_ids",
                    "minute",
                }
            ):
                return self._compare(project_id, run_id, query)
            if len(parts) == 6 and parts[3] == "branches":
                return self._branch_get(project_id, run_id, parts[4], parts[5], query)
        raise StorageError("unknown project route or query")

    def _branch_get(
        self, project_id: str, run_id: str, branch_id: str, action: str, query: str
    ) -> bytes:
        core = answers()
        files = self.files
        branch = files.branch_file(project_id, run_id, branch_id, "branch.json")
        if action == "timeline":
            values = parse_qs(query)
            if set(values) != {"minute"} or len(values["minute"]) != 1:
                raise StorageError("unknown project route or query")
            record, minute = core.timeline_plan(
                values["minute"][0],
                project_id,
                run_id,
                *files.run_listing(project_id, run_id),
                branch_id,
            )
            return self.frame(project_id, run_id, json.loads(record), minute)
        if action == "conversations" and query == "":
            core.branch_of(project_id, run_id, *files.run_listing(project_id, run_id), branch_id)
            return core.conversations_answer(
                files.conversation_pieces(project_id, run_id, branch_id)
            )
        if action == "llm-status":
            from sheltersim.web.stored_runs import llm_settings

            offline = core.llm_status_answer(query, project_id, run_id, branch_id, branch)
            saved = llm_settings(self.store, project_id, run_id)
            if saved is None or saved["mode"] == "offline":
                return offline
            # A Run computed with an LLM: the outcomes of the calls its stored tape holds.
            stored = json.loads(offline)["checkpoint_hash"] is not None
            tape = files.reader(project_id, run_id, branch_id).records("llm_tape") if stored else []
            return core.llm_run_status_answer(
                query, project_id, run_id, branch_id, branch, saved["mode"], tape
            )
        if action == "history":
            plan = core.history_plan(query, project_id, run_id, branch_id, branch)
            return plan.answer(self.kpi_table(project_id, run_id, branch_id, plan))
        if action == "report":
            kept = files.branch_file(project_id, run_id, branch_id, "report.json")
            answer = core.report_answer(query, project_id, run_id, branch_id, branch, kept)
            if answer is not None:
                return answer
            record = json.loads(branch)
            # A report not made yet is made from the Run at its end, in a Run of its own.
            self.live.forget(project_id, run_id)
            reader = files.reader(project_id, run_id, branch_id)
            run, _, _ = reader.seek(record["computed_until"])
            scenario = files.run_scenario_bytes(project_id, run_id)
            text = core.build_report(run, scenario, branch_id)
            with files.transaction():
                files.keep_report(project_id, run_id, branch_id, text + b"\n")
            return text
        raise StorageError("unknown project route or query")

    def kpi_table(self, project_id: str, run_id: str, branch_id: str, plan: Any) -> Any:
        """The branch's metric samples a history plan reads (`KpiTable`): those of its head,
        none for a branch without one."""
        table = answers().KpiTable()
        if plan.anchor() is not None:
            reader = self.files.reader(project_id, run_id, branch_id)
            for kind, raw in reader.sample_pieces():
                if kind == "sample_tables":
                    table.read_sample_tables(raw)
                else:
                    table.read_sample_block(raw)
        return table

    def frame(self, project_id: str, run_id: str, record: dict[str, Any], minute: int) -> bytes:
        """The frame of a branch at a minute (the browser's `frameText`). A Run computed with
        other model versions is not computed again (Python's D2): the frame is the saved point's
        at or before the minute, with the fields that say so."""
        core = answers()
        files = self.files
        branch_id = record["branch_id"]
        journal = files.branch_file(project_id, run_id, branch_id, "journal.jsonl") or b""
        scenario = files.run_scenario_bytes(project_id, run_id)
        if journal and core.view_only(journal, scenario):
            heads = json.loads(files.reader(project_id, run_id, branch_id).heads_json())
            saved = max(
                head["minute"]
                for head in heads
                if head["kind"] != "end" and head["minute"] <= minute
            )
            run = self.live.at(project_id, run_id, record, saved, exact=True)
            frame = run.frame(run_id, branch_id, record["computed_until"])
            return core.view_frame(frame, journal, scenario, saved, minute)
        run = self.live.at(project_id, run_id, record, minute)
        return run.frame(run_id, branch_id, record["computed_until"])

    def journals(self, project_id: str, run_id: str, branch_ids: list[str]) -> list[bytes]:
        """The branches' journals (empty where there is none), for their fork receipts."""
        return [
            self.files.branch_file(project_id, run_id, branch_id, "journal.jsonl") or b""
            for branch_id in branch_ids
        ]

    def _compare(self, project_id: str, run_id: str, query: str) -> bytes:
        plan = answers().compare_plan(
            query, project_id, run_id, *self.files.run_listing(project_id, run_id)
        )
        plan.receipts(self.journals(project_id, run_id, plan.branch_ids))
        records = json.loads(plan.records_json())
        frames = [self.frame(project_id, run_id, record, plan.minute) for record in records]
        return plan.answer(run_id, frames)

    def _replay(self, project_id: str, run_id: str, query: str) -> bytes:
        listing = self.files.run_listing(project_id, run_id)
        plan = answers().replay_plan(query, project_id, run_id, *listing)
        records = json.loads(plan.records_json())
        minutes = range(plan.start, plan.end + 1)
        if len(records) == 2:
            plan.receipts(
                project_id, run_id, *listing, self.journals(project_id, run_id, plan.branch_ids)
            )
            samples = [
                plan.compare(
                    run_id,
                    minute,
                    [self.frame(project_id, run_id, record, minute) for record in records],
                )
                for minute in minutes
            ]
        else:
            samples = [self.frame(project_id, run_id, records[0], minute) for minute in minutes]
        return plan.answer(samples)

    # ---- POST ----------------------------------------------------------------------------------

    def _post(self, parts: list[str], body: bytes) -> bytes:
        files = self.files
        if not parts:
            document = _document(body)
            fields(document, {"project_id"})
            if not isinstance(document["project_id"], str):
                raise ValueError("invalid project request")
            with files.transaction():
                return files.create_project(document["project_id"]).encode()
        project_id = parts[0]
        if len(parts) == 2 and parts[1] in {"scenario-preview", "scenario-approve", "runs"}:
            document = _document(body)
            if parts[1] == "scenario-preview":
                return json_bytes(self.workspace.preview(project_id, document))
            if parts[1] == "scenario-approve":
                return self.workspace.approve(project_id, document)
            return self.create_run(project_id, document)
        if len(parts) == 4 and parts[1] == "runs":
            run_id, action = parts[2], parts[3]
            if action == "resume":
                document = _document(body)
                fields(document, {"branch_id", "target_minute"})
                return self.submit(
                    project_id, run_id, document["branch_id"], document["target_minute"]
                )
            if action == "cancel":
                document = _document(body)
                # Without branch_id, every computation of the Run stops (#48).
                fields(document, set(), {"branch_id"})
                return self.cancel(project_id, run_id, document.get("branch_id"))
            if action == "pdf-report":
                # The PDF's bytes; without an API key, the core's refusal (#463).
                return self.pdf_reports.export(project_id, run_id, body)
            if action == "delete":
                return self.delete_run(project_id, run_id, body)
        if len(parts) == 6 and parts[1] == "runs" and parts[3] == "branches":
            run_id, branch_id, action = parts[2], parts[4], parts[5]
            if action == "intervention-preview":
                return self.workspace.intervention_preview(project_id, run_id, branch_id, body)
            if action == "intervention-apply":
                return self.workspace.intervention_apply(project_id, run_id, branch_id, body)
            if action == "questions":
                return self.workspace.question(project_id, run_id, branch_id, body)
        raise StorageError("unknown project operation")

    @_refusals
    def create_run(self, project_id: str, document: dict[str, Any]) -> bytes:
        """`POST {project}/runs`: a Run of a saved Scenario revision, its branch `main` stored at
        minute 0 (the browser's `ScenarioRoutes.runs`); a Run of the same Scenario revision that
        is there already is answered as it is."""
        from sheltersim.llm.config import LLMMode, LLMSettings
        from sheltersim.web.stored_runs import start_run

        fields(
            document,
            {"run_id", "scenario_id", "scenario_version"},
            {"usage_account_id", "execution_mode"},
        )
        if "execution_mode" in document and document["execution_mode"] != "offline":
            raise StorageError("only_explicit_offline_experiment_mode_is_supported")
        run_id = document["run_id"]
        if not isinstance(run_id, str) or not answers().is_artifact_id(run_id):
            raise StorageError("invalid artifact identifier")
        run = self.files.run_file(project_id, run_id, "run.json")
        if run is not None:
            record = json.loads(run)
            if (record["scenario_id"], record["scenario_version"]) != (
                document["scenario_id"],
                document["scenario_version"],
            ):
                raise StorageError("run_id_conflict")
        if run is None or not json.loads(self.files.branches(project_id, run_id)):
            settings = LLMSettings.from_env()
            offline = document.get("execution_mode") == "offline"
            llm = None
            if offline or settings.effective_mode is not LLMMode.OFFLINE:
                llm = {
                    "schema_version": 1,
                    "mode": "offline" if offline else settings.effective_mode.value,
                    "execution_purpose": "offline_experiment" if offline else None,
                }
            start_run(
                self.store,
                self.files,
                project_id,
                run_id,
                document["scenario_id"],
                document["scenario_version"],
                llm=llm,
            )
            if llm is not None and llm["mode"] != "offline" and document.get("usage_account_id"):
                from sheltersim.llm.accounting import usage_limits
                from sheltersim.storage.usage import UsageLedger

                # The Scenario's preparation and the Run share one LLM allowance.
                UsageLedger.for_run(
                    self.store,
                    project_id,
                    run_id,
                    usage_limits(settings),
                    document["usage_account_id"],
                )
        return answers().run_answer(
            project_id, run_id, *self.files.run_listing(project_id, run_id), None
        )

    @_refusals
    def submit(self, project_id: str, run_id: str, branch_id: str, target_minute: Any) -> bytes:
        """`POST runs/{run}/resume`: computing a branch from its head toward target_minute starts
        (the browser's `Computing.resume`: Python's checks in their order)."""
        files = self.files
        with self._lock:
            key = (project_id, run_id, branch_id)
            if self._closed or key in self._jobs:
                raise StorageError("background worker is busy; wait or pause the active run")
            if len(self._jobs) >= self._max_jobs:
                raise StorageError(
                    "background workers are busy; wait until a running computation ends"
                )
            record = json.loads(files.branch_record(project_id, run_id, branch_id))
            scenario = files.run_scenario_bytes(project_id, run_id)
            # A Run of another model version is view-only; say so before any job starts.
            answers().require_current_model(
                files.branch_file(project_id, run_id, branch_id, "journal.jsonl") or b"",
                scenario,
            )
            duration = json.loads(scenario)["duration_minutes"]
            if (
                type(target_minute) is not int
                or not record["computed_until"] < target_minute <= duration
            ):
                raise StorageError("target minute must be in the uncomputed remaining range")
            if record["checkpoint_hash"] is None:
                raise StorageError("branch has no computed minutes")
            self._require_llm_mode(project_id, run_id)
            if self._worker == "process":
                self._start_process(key, target_minute)
            else:
                self._start_thread(key, target_minute)
        return json_bytes(
            {
                "branch_id": branch_id,
                "run_id": run_id,
                "status": "running",
                "target_minute": target_minute,
            }
        )

    def _require_llm_mode(self, project_id: str, run_id: str) -> None:
        """A Run computed with a Provider goes on only with the same LLM mode configured."""
        from sheltersim.llm.config import LLMSettings
        from sheltersim.web.stored_runs import llm_settings

        saved = llm_settings(self.store, project_id, run_id)
        if saved is not None and saved["mode"] != "offline":
            if LLMSettings.from_env().effective_mode.value != saved["mode"]:
                raise StorageError("saved LLM mode unavailable; restore original configuration")

    def _start_process(self, key: tuple[str, str, str], target_minute: int) -> None:
        """Starts the branch's computation in its own process; called with self._lock held."""
        from sheltersim.web.worker_process import start_worker

        project_id, run_id, branch_id = key
        process, cancel, outcome = start_worker(
            self.store,
            {
                "project_id": project_id,
                "run_id": run_id,
                "branch_id": branch_id,
                "target_minute": target_minute,
            },
        )
        job = _Job(branch_id, cancel)
        job.watcher = threading.Thread(
            target=self._watch,
            args=(process, outcome, key, job),
            name=f"sheltersim-watch-{run_id}",
            daemon=True,
        )
        self._jobs[key] = job
        job.watcher.start()

    def _start_thread(self, key: tuple[str, str, str], target_minute: int) -> None:
        """Starts the branch's computation on the API's thread; called with self._lock held."""
        from sheltersim.web.worker_process import finish

        files = self.files
        project_id, run_id, branch_id = key
        lease = files.claim_branch(project_id, run_id, branch_id)
        job = _Job(branch_id, threading.Event())
        try:
            with files.transaction():
                files.set_branch_status(project_id, run_id, branch_id, "running")
            self._jobs[key] = job
            self._pool.submit(
                finish,
                files,
                lease,
                project_id,
                run_id,
                branch_id,
                target_minute,
                job.cancel.is_set,
                lock=self._lock,
                ended=lambda: self._forget(key, job),
            )
        except BaseException:
            self._jobs.pop(key, None)
            try:
                with files.transaction():
                    files.set_branch_status(project_id, run_id, branch_id, "interrupted")
            finally:
                lease.release()
            raise

    def _forget(self, key: tuple[str, str, str], job: _Job) -> None:
        with self._lock:
            if self._jobs.get(key) is job:
                del self._jobs[key]

    def _watch(self, process: Any, outcome: Any, key: tuple[str, str, str], job: _Job) -> None:
        """Wait for a computation's process. A process that ended without setting its branch's
        final status (killed, crashed) leaves the branch interrupted, as a restart would."""
        try:
            while True:
                outcome.recv()
        except (EOFError, OSError):
            pass
        finally:
            outcome.close()
        process.join()
        project_id, run_id, branch_id = key
        files = self.files
        try:
            try:
                lease = files.claim_branch(project_id, run_id, branch_id)
            except ValueError:
                # Another process owns the branch now, or the Run was deleted.
                return
            try:
                with files.transaction():
                    record = json.loads(files.branch_record(project_id, run_id, branch_id))
                    if record.get("status") in {"queued", "running"}:
                        files.set_branch_status(project_id, run_id, branch_id, "interrupted")
            except ValueError:
                pass
            finally:
                lease.release()
        finally:
            self._forget(key, job)

    @_refusals
    def cancel(self, project_id: str, run_id: str, branch_id: str | None) -> bytes:
        """`POST runs/{run}/cancel`: asks the Run's computations (of `branch_id` only, when
        given) to stop at their next chunk; the Run as it is now."""
        with self._lock:
            jobs = [
                job
                for key, job in self._jobs.items()
                if key[:2] == (project_id, run_id) and branch_id in {None, key[2]}
            ]
            for job in jobs:
                job.cancel.set()
            text = answers().run_answer(
                project_id, run_id, *self.files.run_listing(project_id, run_id), None
            )
            if not jobs and json.loads(text)["status"] == "running":
                raise StorageError("run_worker_busy: cancellation must reach the owning worker")
        return text

    @_refusals
    def delete_run(self, project_id: str, run_id: str, body: bytes) -> bytes:
        """`POST runs/{run}/delete`: the Run's id as the confirmation, no computation of it
        running and a status that is not running; then its folder is removed."""
        files = self.files
        with self._lock:
            active = any(key[:2] == (project_id, run_id) for key in self._jobs)
            answer = answers().delete_answer(
                project_id, run_id, body, active, *files.run_listing(project_id, run_id)
            )
            self.live.forget(project_id, run_id)
            self._fork_outcomes.pop((project_id, run_id), None)
            with files.claim_run(project_id, run_id), files.transaction():
                files.remove_run(project_id, run_id)
        return answer

    @_refusals
    def fork(
        self,
        project_id: str,
        run_id: str,
        parent_id: str,
        child_id: str,
        minute: int,
        program: str,
        approved_program_hash: str,
        state_hash: str | None = None,
    ) -> tuple[dict[str, Any], str, str]:
        """Makes the branch `child_id` of `parent_id` at `minute` with the EffectProgram
        `program` (its JSON text), at once: `(the child's record, the receipt's JSON text, the
        child's state hash)`."""
        files = self.files
        with self._lock:
            if (project_id, run_id, parent_id) in self._jobs:
                raise StorageError("pause background calculation before forking")
            with files.claim_branch(project_id, run_id, child_id) as lease, files.transaction():
                record, receipt, state = files.fork(
                    lease,
                    project_id,
                    run_id,
                    parent_id,
                    child_id,
                    minute,
                    program,
                    approved_program_hash,
                    state_hash,
                )
        return json.loads(record), receipt, state

    def keep_fork_outcome(self, project_id: str, run_id: str, outcome: bytes | None) -> None:
        """Keeps the outcome of the Run's last fork (its JSON bytes), the Run's `last_fork`."""
        with self._lock:
            if outcome is None:
                self._fork_outcomes.pop((project_id, run_id), None)
            else:
                self._fork_outcomes[(project_id, run_id)] = outcome

    def busy(self, project_id: str, run_id: str, branch_id: str) -> bool:
        """Whether a computation of the branch is running."""
        with self._lock:
            return (project_id, run_id, branch_id) in self._jobs


def _document(body: bytes) -> dict[str, Any]:
    """A POST's body as the server reads it: not JSON 400, not an object 409."""
    try:
        document = json.loads(body.decode("utf-8"))
    except (ValueError, UnicodeDecodeError):
        raise ValueError("invalid project request") from None
    if not isinstance(document, dict):
        raise StorageError("project request must be an object")
    return document


def _refusal(error: ValueError) -> Exception:
    """A ValueError of the files or the core as the server answers it: the core's refusal and a
    StorageError as they are; a stored Run of an older format 409 `stored_run_outdated`; a
    refusal of the files (`run_files.Refused`, with the text of Python's StorageError) a
    StorageError; any other ValueError as it is (400)."""
    if isinstance(error, (StorageError, answers().Refused)):
        return error
    text = str(error)
    if text.startswith("stored_run_outdated:"):
        return StorageError(text, code="stored_run_outdated")
    if isinstance(error, FILES.Refused):
        return storage_error(error)
    return error
