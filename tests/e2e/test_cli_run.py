"""Computing frozen Scenarios as Runs of a Project from the CLI, as the screen computes them
(#450): `run`, then `inspect`, `compare` and `review` of those Runs, and `evaluate-models`.

Preparing and freezing a Scenario is checked in tests/integration/test_prepare_cli.py.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from sheltersim import cli
from sheltersim.experiments.scenario import ScenarioSpec


class Cli:
    """Run CLI commands on one Project and return their JSON output."""

    def __init__(self, tmp_path: Path, capsys: pytest.CaptureFixture[str]) -> None:
        self.tmp_path, self.capsys = tmp_path, capsys
        self.project = ["--root", str(tmp_path / "projects"), "--project-id", "study"]

    def __call__(self, command: str, *arguments: str) -> dict:
        assert cli.main([command, *self.project, *arguments]) == 0
        return json.loads(self.capsys.readouterr().out)

    def frozen(self, scenario: ScenarioSpec, name: str) -> Path:
        source = self.tmp_path / f"{name}.json"
        source.write_text(scenario.canonical_json(), encoding="utf-8")
        frozen = self.tmp_path / f"{name}-approved.json"
        arguments = ["prepare", "--scenario", str(source), "--freeze", "--output", str(frozen)]
        assert cli.main(arguments) == 0
        self.capsys.readouterr()
        return frozen


def test_runs_are_computed_inspected_compared_and_reviewed_from_the_cli(
    tmp_path: Path, capsys: pytest.CaptureFixture[str], small_benchmark_scenario
) -> None:
    run = Cli(tmp_path, capsys)
    baseline = small_benchmark_scenario(60)
    document = json.loads(baseline.canonical_json())
    document["model_configs"]["behavior"]["temperature"] = 2.0
    candidate = ScenarioSpec.model_validate(document)

    computed = run("run", "--scenario", str(run.frozen(baseline, "baseline")), "--run-id", "a")
    run("run", "--scenario", str(run.frozen(candidate, "candidate")), "--run-id", "b", "--offline")

    assert computed["branch"]["status"] == "completed"
    assert computed["branch"]["computed_until"] == 60
    assert computed["llm_status"] == {"status": "not_requested"}
    assert capsys.readouterr().err == ""
    record = run("inspect", "--run-id", "a", "--record")
    assert record["scenario_hash"] == computed["scenario_hash"]
    assert [branch["branch_id"] for branch in record["branches"]] == ["main"]
    evidence = run("inspect", "--run-id", "a", "--evidence")
    assert evidence["model_versions"] == record["model_versions"]
    assert run("inspect", "--run-id", "a", "--conversations")["turns"] == []
    page = run("inspect", "--run-id", "a", "--kpis", "--start", "10", "--limit", "5")
    assert [row["minute_index"] for row in page["items"]] == [10, 11, 12, 13, 14]
    assert page["anchor_hash"] == computed["branch"]["checkpoint_hash"]
    rest = run(
        "inspect", "--run-id", "a", "--kpis", "--start", "10", "--cursor", page["next_cursor"]
    )
    assert rest["items"][0]["minute_index"] == 15

    report = tmp_path / "comparison.json"
    assert run("compare", "--baseline", "a", "--candidate", "b/main", "--output", str(report)) == {
        "report": str(report)
    }
    comparison = json.loads(report.read_text(encoding="utf-8"))
    (pair,) = comparison["pairs"]
    assert (pair["seed"], pair["common_range"], pair["final_minute"]) == (7, [0, 60], 59)
    assert pair["first_divergence"]["minute"] < 59
    stress = pair["final_metrics"]["stress_mean"]
    assert stress["delta"] == stress["candidate"] - stress["baseline"]
    summary = {item["metric"]: item for item in comparison["paired_summary"]}
    assert set(summary) == set(pair["final_metrics"])
    assert summary["stress_mean"]["paired_deltas"] == [stress["delta"]]
    same = run("compare", "--baseline", "a", "--candidate", "a", "--metric", "stress_mean")
    assert same["pairs"][0]["first_divergence"] is None

    review = run("review", "--run-id", "a", "--comparison", str(report))
    assert review["report"]["review"]["status"] == "rule_fallback"
    assert review["comparisons"] == comparison["pairs"]


def test_a_model_matrix_is_computed_as_runs_and_compared_by_seed(
    tmp_path: Path, capsys: pytest.CaptureFixture[str], small_benchmark_scenario
) -> None:
    run = Cli(tmp_path, capsys)
    scenario = tmp_path / "scenario.json"
    scenario.write_text(small_benchmark_scenario(30).canonical_json(), encoding="utf-8")
    plan = tmp_path / "plan.json"
    plan.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "seeds": [1, 2],
                "variants": [
                    {"id": "base"},
                    {"id": "hungry", "parameters": {"psychology.hunger_growth": 0.01}},
                ],
                "metrics": ["stress_mean"],
            }
        ),
        encoding="utf-8",
    )
    output = tmp_path / "evaluation.json"
    arguments = ["--scenario", str(scenario), "--plan", str(plan), "--output", str(output)]

    evaluation = run("evaluate-models", *arguments)

    assert evaluation == json.loads(output.read_text(encoding="utf-8"))
    assert [row["run_id"] for row in evaluation["runs"]] == [
        "base-seed-1",
        "base-seed-2",
        "hungry-seed-1",
        "hungry-seed-2",
    ]
    (compared,) = evaluation["comparisons"]
    (stress,) = compared["metrics"]
    assert stress["paired_seed_count"] == 2
    by_run = {row["run_id"]: row["metrics"]["stress_mean"] for row in evaluation["runs"]}
    assert stress["paired_deltas"] == [
        by_run["hungry-seed-1"] - by_run["base-seed-1"],
        by_run["hungry-seed-2"] - by_run["base-seed-2"],
    ]
    # The same matrix computed again is refused before any Run starts.
    output.unlink()
    with pytest.raises(SystemExit):
        cli.main(["evaluate-models", *run.project, *arguments])
    assert "base-seed-1 exists" in capsys.readouterr().err
