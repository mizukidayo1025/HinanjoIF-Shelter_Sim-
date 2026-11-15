"""Offline checks that rule fallbacks are counted and surfaced in Run manifests, Project
status and reports, never reported as clean runs.

The outcome summary itself is checked in tests/unit/llm/test_outcomes.py, and the CLI
warnings in tests/e2e/test_cli_run.py.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from sheltersim import cli
from sheltersim.llm.contracts import ProviderResponse


class TruncatedProvider:
    """Reply with cut-off JSON, as a response stopped at max_tokens would."""

    def complete(self, *, model_id, task, max_output_tokens, timeout_seconds):
        del max_output_tokens, timeout_seconds
        return ProviderResponse(
            text='{"profiles":[{"profile_index":0,',
            model_id=model_id,
            input_tokens=10,
            output_tokens=5,
            elapsed_ms=0.0,
        )








def test_prepare_cli_warns_when_scenario_draft_falls_back(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    small_benchmark_scenario,
    recording_router,
) -> None:
    source = tmp_path / "scenario.json"
    source.write_text(small_benchmark_scenario(30).canonical_json(), encoding="utf-8")
    monkeypatch.setattr(
        cli,
        "_cli_accounted_router",
        lambda *_args, **_kwargs: recording_router(TruncatedProvider()),
    )
    arguments = [
        "prepare",
        "--scenario",
        str(source),
        "--generate-scenario",
        "--output",
        str(tmp_path / "draft.json"),
    ]

    assert cli.main(arguments) == 0

    captured = capsys.readouterr()
    preview = json.loads(captured.out)
    assert preview["generation_manifest"]["status"] == "rule_fallback"
    assert "warning: LLM fallback:" in captured.err and "invalid_output" in captured.err


