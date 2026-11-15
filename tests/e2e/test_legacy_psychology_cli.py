"""The CLI keeps running a Scenario saved with a legacy psychology model and warns.

The legacy model is never generated, so this uses the small fixture Scenario with
its registry set by hand.
"""

from __future__ import annotations

import json

from support.paths import SMALL_REPLAY

from sheltersim.__main__ import main
from sheltersim.experiments.scenario import ScenarioSpec


def test_cli_reports_and_warns_about_a_legacy_psychology_model(tmp_path, capsys) -> None:
    legacy_document = ScenarioSpec.load(SMALL_REPLAY).model_dump(mode="json")
    legacy_document["model_configs"]["policy_registry"] = {
        "schema_version": 2,
        "psychology": "causal-stress-v1",
    }
    legacy_scenario = tmp_path / "legacy.json"
    legacy_scenario.write_text(json.dumps(legacy_document), encoding="utf-8")
    frozen = tmp_path / "frozen.json"
    assert (
        main(["prepare", "--scenario", str(legacy_scenario), "--freeze", "--output", str(frozen)])
        == 0
    )
    assert "warning: psychology model causal-stress-v1 is a legacy model" in (
        capsys.readouterr().err
    )
    project = ["--root", str(tmp_path / "projects"), "--project-id", "p", "--run-id", "r"]
    assert main(["run", "--offline", "--scenario", str(frozen), *project]) == 0
    captured = capsys.readouterr()
    assert json.loads(captured.out)["psychology_model"] == "causal-stress-v1"
    assert "warning: psychology model causal-stress-v1 is a legacy model" in captured.err
