"""A Project's intervention preview reads the text with the offline rules on the Branch at a
minute, and an approved preview forks the Branch without changing it (format v6, #427).

The preview-and-apply tests took over from the older demo's natural-language tests (#137;
the demo went with the older screens in #376), as in
tests/e2e/test_project_scenario_to_results.py. The answers are the core's (#449), the browser's.
"""

from __future__ import annotations

import json

import pytest

from sheltersim.web import workspace
from sheltersim.web.projects import ProjectAPI, answers

# The main Branch is computed to COMPUTED_UNTIL; previews pick an earlier minute, as the screen's
# time slider does.
COMPUTED_UNTIL = 90
PREVIEW_MINUTE = 40
ARRIVALS = "避難スペースに避難者を10人追加"
MAIN = ["p", "runs", "r", "branches", "main"]


@pytest.fixture
def project_api(tmp_path, generated_scenario, monkeypatch, saved_project):
    """The Project API over a generated Scenario Run computed to COMPUTED_UNTIL, offline."""
    monkeypatch.setenv("SHELTERSIM_LLM_MODE", "offline")
    api = ProjectAPI(saved_project(tmp_path, generated_scenario), recover=False)
    try:
        api.submit("p", "r", "main", COMPUTED_UNTIL)
        api.wait_idle(timeout=120)
        yield api
    finally:
        api.close()


def _saved_state(api: ProjectAPI) -> tuple[bytes, bytes, str]:
    """The main Branch's record and stored Run, and its state at PREVIEW_MINUTE."""
    run, _, _ = api.files.reader("p", "r", "main").seek(PREVIEW_MINUTE)
    return (
        api.files.branch_file("p", "r", "main", "branch.json"),
        api.files.branch_file("p", "r", "main", "journal.jsonl"),
        run.state_digest(),
    )


def _preview(api: ProjectAPI, body: dict) -> dict:
    return api.post([*MAIN, "intervention-preview"], {"minute": PREVIEW_MINUTE, **body})


def _apply(api: ProjectAPI, preview: dict, child_id: str, **overrides) -> dict:
    return api.post(
        [*MAIN, "intervention-apply"],
        {
            "preview_id": preview["preview_id"],
            "approved_program_hash": preview["approved_program_hash"],
            "approved": True,
            "child_id": child_id,
            **overrides,
        },
    )


def _branches(api: ProjectAPI) -> list[str]:
    return [branch["branch_id"] for branch in api.get(["p", "runs", "r"])["branches"]]


def test_project_preview_changes_neither_the_branch_nor_its_stored_run(project_api) -> None:
    before = _saved_state(project_api)

    preview = _preview(project_api, {"text": ARRIVALS})

    assert preview["status"] == "compiled"
    assert preview["can_apply"] is True
    assert _saved_state(project_api) == before


@pytest.mark.parametrize("text", ["2026-10-14 14:00 toilet outage", "10/13 9am toilet outage"])
def test_project_preview_of_a_date_asks_for_the_day_number(project_api, text) -> None:
    """The route's summary has no start date; a date was a 400 instead of a question (#325)."""
    preview = _preview(project_api, {"text": text})

    assert preview["status"] == "clarification_required"
    assert "program" not in preview
    assert any("Day番号を指定してください" in item for item in preview["questions"])


def test_project_preview_of_a_vague_time_asks_without_choices(project_api) -> None:
    """The choice "HH:MM" became a button that added "対象ID: HH:MM" to the request and
    previewed the same question again (#340)."""
    preview = _preview(project_api, {"text": "午後にトイレが故障"})

    assert preview["status"] == "clarification_required"
    assert preview["choices"] == []
    assert any("具体的な時刻を指定してください" in item for item in preview["questions"])


def test_project_apply_forks_at_a_past_minute_and_leaves_the_source_unchanged(
    project_api,
) -> None:
    before = _saved_state(project_api)
    preview = _preview(project_api, {"text": ARRIVALS})

    answer = _apply(project_api, preview, "arrivals")

    assert answer == {"branch_id": "arrivals", "minute": PREVIEW_MINUTE, "status": "running"}
    run = project_api.get(["p", "runs", "r"])
    assert run["last_fork"]["status"] == "created"
    child = json.loads(project_api.files.branch_record("p", "r", "arrivals"))
    assert (child["parent_branch_id"], child["fork_minute"]) == ("main", PREVIEW_MINUTE)
    assert child["computed_until"] == PREVIEW_MINUTE
    assert _saved_state(project_api) == before
    lines = json.loads(project_api.files.reader("p", "r", "arrivals").lines_json())
    fork = next(line for line in lines if line["kind"] == "fork")
    assert fork["fork_origin"]["state_hash"] == before[2]
    assert '"add_evacuees"' in fork["program"]

    # A second approval of the same plan cannot reuse the Branch name.
    again = _preview(project_api, {"text": ARRIVALS})
    with pytest.raises(answers().Refused, match="child branch already exists"):
        _apply(project_api, again, "arrivals")
    assert _saved_state(project_api) == before


def test_project_apply_needs_an_applicable_preview_its_hash_and_approval(project_api) -> None:
    before = _saved_state(project_api)
    preview = _preview(project_api, {"text": ARRIVALS})
    with pytest.raises(answers().Refused, match="does not match validated preview"):
        _apply(project_api, preview, "wrong-hash", approved_program_hash="0" * 64)
    with pytest.raises(answers().Refused, match="explicit human approval"):
        _apply(project_api, preview, "not-approved", approved=False)

    text_only = _preview(project_api, {"text": "空を飛ぶ"})
    assert text_only["status"] == "not_simulatable"
    assert "preview_id" not in text_only
    assert _saved_state(project_api) == before
    assert _branches(project_api) == ["main"]


def test_project_apply_is_refused_when_the_source_changed_after_the_preview(
    project_api, monkeypatch
) -> None:
    """The fork compares the source state it restores with the preview's (#46)."""
    before = _saved_state(project_api)
    preview = _preview(project_api, {"text": ARRIVALS})
    # Stand in for a source branch that changed after the preview.
    monkeypatch.setattr(workspace, "_draft_state_hash", lambda _draft: "0" * 64)

    _apply(project_api, preview, "arrivals")

    outcome = project_api.get(["p", "runs", "r"])["last_fork"]
    assert outcome["status"] == "failed"
    assert outcome["error"] == "source state changed; preview the intervention again"
    assert _branches(project_api) == ["main"]
    assert _saved_state(project_api) == before
