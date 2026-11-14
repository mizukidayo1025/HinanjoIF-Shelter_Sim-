"""A Branch's history route on a generated Run with a Branch that adds toilets: the KPIs and
the conversation of the questions answer (#368 decision A), the other kinds are refused.

The comparison of several Runs, the first divergence, the approved-condition differences and
the people and household samples went with the older screens (#376). The routes only read the
Run.
"""

from __future__ import annotations

import copy

import pytest

from sheltersim.experiments.scenario import ScenarioSpec
from sheltersim.storage.projects import StorageError, digest
from sheltersim.web.projects import ProjectAPI

END = 60
FORK_MINUTE = 10
ADDED_TOILETS = 2
ANSWERED_KINDS = ("kpis", "conversations")
REMOVED_KINDS = (
    "people",
    "households",
    "events",
    "causal",
    "social",
    "staff",
    "actions",
    "transitions",
    "dialogue",
    "care",
    "operations",
)


@pytest.fixture(scope="module")
def evidence_api(tmp_path_factory, generated_previews, add_toilets_program, saved_timeline):
    """A ProjectAPI over Run r of the generated Scenario, with a capacity Branch."""
    preview = generated_previews(duration_days=END / 1440)
    scenario = ScenarioSpec.model_validate(copy.deepcopy(preview["scenario"]))
    with pytest.MonkeyPatch.context() as monkeypatch:
        monkeypatch.setenv("SHELTERSIM_LLM_MODE", "offline")
        store, timeline = saved_timeline(tmp_path_factory.mktemp("evidence") / "projects", scenario)
        timeline.advance("main", END, checkpoint_interval=10)
        program = add_toilets_program(ADDED_TOILETS, start_minute=FORK_MINUTE)
        timeline.fork(
            "main",
            FORK_MINUTE,
            "capacity",
            program=program,
            approved_program_hash=digest(program.model_dump(mode="json")),
        )
        timeline.advance("capacity", END)
        api = ProjectAPI(store)
        try:
            yield api, scenario
        finally:
            api.close()


def _history(api: ProjectAPI, branch_id: str, **query: object) -> dict:
    return api.get(
        ["p", "runs", "r", "branches", branch_id, "history"],
        {key: [str(value)] for key, value in query.items()},
    )


@pytest.mark.parametrize("kind", ANSWERED_KINDS)
def test_each_answered_history_kind_answers_for_both_branches(evidence_api, kind):
    api, _ = evidence_api
    for branch_id in ("main", "capacity"):
        result = _history(api, branch_id, kind=kind, start=FORK_MINUTE, end=END, limit=100)

        assert (result["run_id"], result["branch_id"], result["kind"]) == ("r", branch_id, kind)
        assert result["computed_until"] == END
        expected = "end_of_tick" if kind == "kpis" else "recorded_event_minute"
        assert result["time_semantics"] == expected
        assert all(item["branch_id"] == branch_id for item in result["items"])


def test_kpi_history_has_one_end_of_tick_row_per_computed_minute(evidence_api):
    api, _ = evidence_api
    rows = _history(api, "capacity", kind="kpis", start=FORK_MINUTE, end=END, limit=100)["items"]

    # The tick at END has not run, so its end-of-tick row does not exist yet.
    assert [row["minute_index"] for row in rows] == list(range(FORK_MINUTE, END))


@pytest.mark.parametrize("start,end", [(0, END), (FORK_MINUTE, END + 1)])
def test_history_outside_the_branch_range_is_refused(evidence_api, start, end):
    api, _ = evidence_api
    with pytest.raises(StorageError, match="history_outside_computed_range"):
        _history(api, "capacity", kind="kpis", start=start, end=end)


@pytest.mark.parametrize("kind", REMOVED_KINDS)
def test_the_other_history_kinds_are_refused(evidence_api, kind):
    """The index still writes them until #368 step 10, but no route reads them (#376)."""
    api, _ = evidence_api
    query = {"kind": kind, "start": FORK_MINUTE, "end": END}
    if kind == "people":
        query["agent_id"] = "A001"
    if kind == "households":
        query["household_id"] = "H000001"
    if kind == "causal":
        query["event_id"] = "event-0"
    with pytest.raises(StorageError, match="history_kind_unavailable"):
        _history(api, "capacity", **query)
