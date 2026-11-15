"""A Branch's history route on a generated Run with a Branch that adds toilets: the KPIs answer
(#368 decision A), from the stored Run of format v6 (#427). The conversation of the questions has
its own route (`…/conversations`), which answers the turns in the order the questions were asked.

The comparison of several Runs, the first divergence, the approved-condition differences and
the people and household samples went with the older screens (#376). The history routes only read
the Run; the questions keep their turns in the Branch's stored Run.
"""

from __future__ import annotations

import copy
import json

import pytest

from sheltersim.experiments.scenario import ScenarioSpec
from sheltersim.storage.projects import digest
from sheltersim.web.projects import ProjectAPI, answers

END = 60
FORK_MINUTE = 10
ADDED_TOILETS = 2
ANSWERED_KINDS = ("kpis",)


@pytest.fixture(scope="module")
def evidence_api(tmp_path_factory, generated_previews, add_toilets_program, saved_project):
    """A ProjectAPI over Run r of the generated Scenario, with a capacity Branch (format v6)."""
    preview = generated_previews(duration_days=END / 1440)
    scenario = ScenarioSpec.model_validate(copy.deepcopy(preview["scenario"]))
    with pytest.MonkeyPatch.context() as monkeypatch:
        monkeypatch.setenv("SHELTERSIM_LLM_MODE", "offline")
        store = saved_project(tmp_path_factory.mktemp("evidence") / "projects", scenario)
        api = ProjectAPI(store)
        try:
            api.submit("p", "r", "main", END)
            api.wait_idle(timeout=120)
            program = add_toilets_program(ADDED_TOILETS, start_minute=FORK_MINUTE)
            text = json.dumps(program, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
            api.fork("p", "r", "main", "capacity", FORK_MINUTE, text, digest(program))
            api.submit("p", "r", "capacity", END)
            api.wait_idle(timeout=120)
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
        assert result["time_semantics"] == "end_of_tick"
        assert all(item["branch_id"] == branch_id for item in result["items"])


def test_kpi_history_has_one_end_of_tick_row_per_computed_minute(evidence_api):
    api, _ = evidence_api
    rows = _history(api, "capacity", kind="kpis", start=FORK_MINUTE, end=END, limit=100)["items"]

    # The tick at END has not run, so its end-of-tick row does not exist yet.
    assert [row["minute_index"] for row in rows] == list(range(FORK_MINUTE, END))


@pytest.mark.parametrize("start,end", [(0, END), (FORK_MINUTE, END + 1)])
def test_history_outside_the_branch_range_is_refused(evidence_api, start, end):
    api, _ = evidence_api
    with pytest.raises(answers().Refused, match="history_outside_computed_range"):
        _history(api, "capacity", kind="kpis", start=start, end=end)


def test_the_conversation_keeps_its_turns_in_the_order_asked_even_if_the_clock_goes_back(
    evidence_api, monkeypatch
):
    """A turn's id holds the wall clock (`t-` and nanoseconds), which can go back (WSL's clock
    under load): the conversation is in the order the questions were asked, not in that of the
    ids. Its order once followed the ids, and the interventions fixture then differed at its
    first stored question when the tests ran in parallel (#437)."""
    api, _ = evidence_api
    branch = ["p", "runs", "r", "branches", "capacity"]
    before = len(api.get([*branch, "conversations"])["turns"])
    clock = iter(range(3_000_000_000_000_000_000, 0, -1_000_000_000))
    monkeypatch.setattr("time.time_ns", lambda: next(clock))
    asked = ["一つ目", "二つ目", "三つ目"]
    for question in asked:
        api.post([*branch, "questions"], {"question": question})
    monkeypatch.undo()

    turns = api.get([*branch, "conversations"])["turns"][before:]
    assert [turn["question"] for turn in turns] == asked
    stamps = [int(turn["turn_id"].split("-")[1]) for turn in turns]
    assert stamps == sorted(stamps, reverse=True)
