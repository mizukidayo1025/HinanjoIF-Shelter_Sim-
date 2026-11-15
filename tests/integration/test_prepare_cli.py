"""Pre-run scenario preview and freezing through the CLI, and `run` taking only a frozen
preview whose Scenario is the one approved.

Computing a frozen Scenario from the CLI is checked in tests/e2e/test_cli_run.py.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest
from support.paths import SCENARIO_100_AGENTS

from sheltersim.__main__ import main
from sheltersim.experiments.scenario import ScenarioSpec

# Run on every pull request on Windows and macOS too (pytest --os-subset, #380): the CLI writes
# files and reads them back.
pytestmark = pytest.mark.os_dependent


def make_scenario_file(path: Path) -> Path:
    scenario_data = json.loads(SCENARIO_100_AGENTS.read_text(encoding="utf-8"))
    scenario_data["scheduled_events"] = [
        {
            "id": "planned-facility-failure",
            "kind": "exogenous",
            "due_minute": 5,
            "priority": 0,
            "sequence": 0,
            "event_type": "facility_outage",
            "payload": {"facility_id": "toilet_1", "units_out": 1},
            "source_id": "fixture-plan",
            "causation_id": None,
            "provenance": {"source_type": "assumption"},
        }
    ]
    scenario = ScenarioSpec.model_validate(scenario_data)
    scenario.save(path)
    return path


def test_prepare_previews_all_profiles_and_planned_events_offline(tmp_path: Path, capsys) -> None:
    scenario_path = make_scenario_file(tmp_path / "scenario.json")
    assert main(["prepare", "--scenario", str(scenario_path)]) == 0

    preview = json.loads(capsys.readouterr().out)
    assert preview["frozen"] is False
    assert preview["profile_count"] == 100
    assert len(preview["scenario"]["profiles"]) == 100
    assert preview["planned_event_count"] == len(preview["scenario"]["scheduled_events"]) == 1
    assert preview["scenario_hash"]


def test_prepare_freeze_writes_reviewable_canonical_snapshot(tmp_path: Path, capsys) -> None:
    scenario_path = make_scenario_file(tmp_path / "scenario.json")
    output = tmp_path / "prepared" / "scenario_preview.json"

    assert (
        main(["prepare", "--scenario", str(scenario_path), "--freeze", "--output", str(output)])
        == 0
    )

    preview = json.loads(output.read_text(encoding="utf-8"))
    assert preview["frozen"] is True
    assert preview["approval"] == {"approved": True, "method": "prepare --freeze"}
    assert preview == json.loads(capsys.readouterr().out)


def test_freeze_records_manual_generated_field_edits_and_checks_content_hash(
    tmp_path: Path, capsys
) -> None:
    scenario_path = make_scenario_file(tmp_path / "scenario.json")
    draft_path = tmp_path / "draft.json"
    assert main(["prepare", "--scenario", str(scenario_path), "--output", str(draft_path)]) == 0
    capsys.readouterr()
    draft = json.loads(draft_path.read_text(encoding="utf-8"))
    generated = json.loads(json.dumps(draft["scenario"]))
    draft["generated_scenario"] = generated
    draft["generation_manifest"] = {
        "schema_version": 1,
        "generator": "test-generator",
        "model_id": "test-model",
        "prompt_version": "test-v1",
        "output_hash": "abc",
        "status": "generated",
        "estimated_cost_usd": 0.01,
        "cache_hit": False,
    }
    draft["scenario"]["profiles"][0]["stress_sensitivity"] = 0.77
    draft_path.write_text(json.dumps(draft), encoding="utf-8")
    frozen_path = tmp_path / "frozen.json"

    assert (
        main(
            [
                "prepare",
                "--scenario",
                str(draft_path),
                "--freeze",
                "--output",
                str(frozen_path),
            ]
        )
        == 0
    )
    capsys.readouterr()
    frozen = json.loads(frozen_path.read_text(encoding="utf-8"))
    assert frozen["generation_manifest"]["manual_edits"] == ["profiles[0].stress_sensitivity"]
    assert frozen["generation_manifest"]["approved"] is True
    assert frozen["scenario"]["provenance"]["llm_scenario_generation"]["manual_edits"] == [
        "profiles[0].stress_sensitivity"
    ]

    frozen["scenario"]["seed"] += 1
    frozen_path.write_text(json.dumps(frozen), encoding="utf-8")
    with pytest.raises(SystemExit) as raised:
        main(
            [
                "run",
                "--offline",
                "--scenario",
                str(frozen_path),
                "--root",
                str(tmp_path / "projects"),
                "--project-id",
                "p",
                "--run-id",
                "tampered",
            ]
        )
    assert raised.value.code == 2
    assert "hash does not match" in capsys.readouterr().err


def test_prepare_requires_output_for_explicit_freeze(tmp_path: Path, capsys) -> None:
    scenario_path = make_scenario_file(tmp_path / "scenario.json")
    with pytest.raises(SystemExit) as raised:
        main(["prepare", "--scenario", str(scenario_path), "--freeze"])

    assert raised.value.code == 2
    assert "requires --output" in capsys.readouterr().err


def test_run_rejects_a_preview_that_was_not_frozen(tmp_path: Path, capsys) -> None:
    scenario_path = make_scenario_file(tmp_path / "scenario.json")
    preview_path = tmp_path / "preview.json"
    assert (
        main(
            [
                "prepare",
                "--scenario",
                str(scenario_path),
                "--output",
                str(preview_path),
            ]
        )
        == 0
    )
    capsys.readouterr()

    with pytest.raises(SystemExit) as raised:
        main(
            [
                "run",
                "--offline",
                "--scenario",
                str(preview_path),
                "--root",
                str(tmp_path / "projects"),
                "--project-id",
                "p",
                "--run-id",
                "r",
            ]
        )

    assert raised.value.code == 2
    assert "not frozen" in capsys.readouterr().err


def test_run_requires_explicitly_frozen_scenario_input(tmp_path: Path, capsys) -> None:
    scenario_path = make_scenario_file(tmp_path / "scenario.json")
    with pytest.raises(SystemExit) as raised:
        main(
            [
                "run",
                "--offline",
                "--scenario",
                str(scenario_path),
                "--root",
                str(tmp_path / "projects"),
                "--project-id",
                "p",
                "--run-id",
                "r",
            ]
        )

    assert raised.value.code == 2
    assert "reviewed frozen preview" in capsys.readouterr().err
