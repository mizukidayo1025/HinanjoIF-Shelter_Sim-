"""Runs of the CLI in a Project (#450): computed and read through the screen's routes.

`sheltersim run` and `sheltersim evaluate-models` save a Scenario as a revision of a Project,
create a Run of it and compute its branch `main` to the end, as the screen's `POST runs` and
`POST runs/{run}/resume` do (web/projects.py: the core's `PlainRun` into a stored Run of format
v5, #427). The metrics of a computed branch are its KPI rows: the core's history answer
(`kind=kpis`) of the branch's stored metric samples (the core's `KpiTable`), as the screen's
`GET …/branches/{branch}/history` answers them.
"""

from __future__ import annotations

import json
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path
from typing import Any
from urllib.parse import urlencode

from sheltersim.experiments.scenario import ScenarioSpec
from sheltersim.storage.projects import ProjectStore, StorageError, digest

# The fields of a KPI row that are not metrics.
KPI_LABELS = frozenset({"minute_index", "time_semantics", "record_kind", "run_id", "branch_id"})
# Rows per page of the history answer (its largest page).
_PAGE = 100


@contextmanager
def project_api(root: Path) -> Iterator[Any]:
    """The screen's Project routes on the Projects in `root`, computing on a thread of this
    process. A refusal of the core's answers is raised as the server's StorageError (409) or a
    ValueError (400), with its text."""
    from sheltersim.web.projects import ProjectAPI, answers

    api = ProjectAPI(ProjectStore(root), recover=False)
    try:
        yield api
    except answers().Refused as refusal:
        status, error, code, _ = refusal.args
        if status == 400:
            raise ValueError(error) from None
        raise StorageError(error, code=code) from None
    finally:
        api.close()


def ensure_project(api: Any, project_id: str) -> None:
    """Creates the Project unless it is there."""
    try:
        api.files.project(project_id)
    except ValueError:
        api.post([], {"project_id": project_id})


def save_revision(api: Any, project_id: str, scenario: ScenarioSpec) -> dict[str, Any]:
    """The Project's revision of the Scenario: the one with the same content, else a new one
    (the screen's checks: no sensitive text)."""
    from sheltersim.web.stored_runs import save_scenario

    document = json.loads(scenario.canonical_json())
    scenario_hash = digest(document)
    for record, _ in api.files.scenarios(project_id):
        saved = json.loads(record)
        if saved["scenario_id"] == scenario.id and saved["scenario_hash"] == scenario_hash:
            return saved
    return save_scenario(api.files, project_id, scenario, scenario_hash)


def compute_run(
    api: Any, project_id: str, run_id: str, scenario: ScenarioSpec, *, offline: bool
) -> dict[str, Any]:
    """Creates a Run of the Scenario in the Project and computes its branch `main` to the end:
    the branch's record (from the Run's answer). `offline` makes it an offline experiment; else
    the configured LLM mode is the Run's, as on the server."""
    ensure_project(api, project_id)
    revision = save_revision(api, project_id, scenario)
    request: dict[str, Any] = {
        "run_id": run_id,
        "scenario_id": revision["scenario_id"],
        "scenario_version": revision["scenario_version"],
    }
    if offline:
        request["execution_mode"] = "offline"
    api.post([project_id, "runs"], request)
    api.post(
        [project_id, "runs", run_id, "resume"],
        {"branch_id": "main", "target_minute": scenario.duration_minutes},
    )
    api.wait_idle()
    branch = branch_record(api, project_id, run_id, "main")
    if branch["status"] != "completed":
        raise RuntimeError(f"the computation ended {branch['status']}")
    return branch


def branch_record(api: Any, project_id: str, run_id: str, branch_id: str) -> dict[str, Any]:
    """A branch's record in the Run's answer (`GET {project}/runs/{run}`)."""
    for branch in api.get([project_id, "runs", run_id])["branches"]:
        if branch["branch_id"] == branch_id:
            return branch
    raise StorageError("branch not found")


def run_scenario_document(api: Any, project_id: str, run_id: str) -> dict[str, Any]:
    """The Scenario of a Run, as saved."""
    return json.loads(api.files.run_scenario_bytes(project_id, run_id))


def kpi_rows(
    api: Any, project_id: str, run_id: str, branch_id: str, start: int, end: int
) -> list[dict[str, Any]]:
    """Every KPI row of a branch from minute `start` to `end`: the pages of the history answer
    (`kind=kpis`), its samples read once."""
    from sheltersim.web.projects import answers

    core = answers()
    branch = api.files.branch_file(project_id, run_id, branch_id, "branch.json")
    table = None
    rows: list[dict[str, Any]] = []
    cursor = None
    while True:
        query = {"kind": "kpis", "start": start, "end": end, "limit": _PAGE}
        if cursor is not None:
            query["cursor"] = cursor
        plan = core.history_plan(urlencode(query), project_id, run_id, branch_id, branch)
        if table is None:
            table = api.kpi_table(project_id, run_id, branch_id, plan)
        page = json.loads(plan.answer(table))
        rows.extend(page["items"])
        cursor = page["next_cursor"]
        if cursor is None:
            return rows


def final_kpis(api: Any, project_id: str, run_id: str, branch: dict[str, Any]) -> dict[str, Any]:
    """A computed branch's last KPI row (the end of its last tick)."""
    end = branch["computed_until"]
    rows = kpi_rows(api, project_id, run_id, branch["branch_id"], max(end - 1, 0), end)
    if not rows:
        raise ValueError("the branch has no KPI rows")
    return rows[-1]


def metrics_of(row: dict[str, Any]) -> dict[str, float]:
    """The numeric metrics of a KPI row."""
    return {
        name: float(value)
        for name, value in row.items()
        if name not in KPI_LABELS
        and isinstance(value, (int, float))
        and not isinstance(value, bool)
    }
