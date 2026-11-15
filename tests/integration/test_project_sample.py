"""The precomputed sample Project (scripts/build_project_sample.py) that the screen opens.

A short variant of the sample is built through the builder: the same ProjectAPI routes
the new UI calls (scenario-preview and scenario-approve of a generated Scenario, resume,
intervention-preview and intervention-apply), with the structured form the Scenario
screen sends for 20 people and 0.1 days instead of the 7-day example text, and an
earlier fork. The 7-day sample itself is built on the judges' VM (docs/vm-deploy.md).

The sample is in the browser's folders and format v6 (#427); the browser's form of it is the same
Project written as one file (`--bundle`, #265), which native/wasm/tests import.
"""

from __future__ import annotations

import html
import json
import runpy
import shutil
import sys
from collections.abc import Callable
from pathlib import Path
from typing import Any

import pytest

from sheltersim.storage.projects import ProjectStore
from sheltersim.web.projects import ProjectAPI

# Run on every pull request on Windows and macOS too (pytest --os-subset, #380): the sample
# Project is built by a script that writes, copies and deletes files.
pytestmark = pytest.mark.os_dependent

BUILDER = Path(__file__).parents[2] / "scripts" / "build_project_sample.py"
LANDING = Path(__file__).parents[2] / "deploy" / "vm" / "render_landing.py"
END = 144
FORK = 60


@pytest.fixture(scope="module")
def builder() -> dict[str, Any]:
    return runpy.run_path(str(BUILDER))


@pytest.fixture
def build(
    builder: dict[str, Any],
    generated_request: Callable[..., dict[str, Any]],
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> Callable[[str], dict[str, Any]]:
    monkeypatch.setenv("SHELTERSIM_LLM_MODE", "offline")
    monkeypatch.delenv("ANTHROPIC_API_KEY", raising=False)

    def run(name: str) -> dict[str, Any]:
        return builder["build_sample"](
            tmp_path / name,
            text="",
            request=generated_request(duration_days=END / 1440),
            fork_minute=FORK,
        )

    return run


def _results(summary: dict[str, Any]) -> list[tuple[Any, ...]]:
    return [
        (b["branch_id"], b["computed_until"], b["head"], b["world_digests"], b["metrics_digest"])
        for b in summary["branches"]
    ]


def test_two_builds_of_the_sample_give_the_same_hashes(
    build: Callable[[str], dict[str, Any]],
) -> None:
    first, second = build("first"), build("second")
    assert first["scenario"]["scenario_hash"] == second["scenario"]["scenario_hash"]
    assert _results(first) == _results(second)
    assert first["intervention"] == second["intervention"]
    assert first["branches"][0]["state_hash"] != first["branches"][1]["state_hash"]


def test_the_sample_holds_a_completed_run_and_an_approved_outage_branch(
    builder: dict[str, Any], build: Callable[[str], dict[str, Any]], tmp_path: Path
) -> None:
    summary = build("projects")
    run_id, child_id = builder["RUN_ID"], builder["CHILD_ID"]
    assert summary["project_id"] == "sample"
    assert summary["run_id"] == run_id
    assert [b["branch_id"] for b in summary["branches"]] == ["main", child_id]
    assert summary["intervention"]["text"] == "Day 1 01:00 toilet_01 outage"
    assert summary["scenario"]["has_facility_geometry"] is True
    assert "toilet_01" in summary["scenario"]["toilets"]
    assert summary["open_branch_id"] == "main"
    assert summary["open_path"] == f"/?project=sample&run={run_id}&branch=main#simulation"

    api = ProjectAPI(ProjectStore(tmp_path / "projects"), recover=False)
    try:
        assert [p["project_id"] for p in api.get([])["projects"]] == ["sample"]
        project = api.get(["sample"])
        run = api.get(["sample", "runs", run_id])
        branch = ["sample", "runs", run_id, "branches"]
        main = api.get([*branch, "main", "timeline"], f"minute={END}")
        child = api.get([*branch, child_id, "timeline"], f"minute={END}")
    finally:
        api.close()
    assert run["status"] == "completed"
    [saved] = project["scenarios"]
    provenance = saved["scenario"]["provenance"]
    assert provenance["region_research"]["warnings"] == [builder["RESEARCH_WARNING"]]
    branches = {b["branch_id"]: b for b in run["branches"]}
    assert branches["main"]["computed_until"] == END
    assert branches[child_id]["parent_branch_id"] == "main"
    assert branches[child_id]["fork_minute"] == FORK
    assert branches[child_id]["computed_until"] == END
    lines = json.loads(api.files.reader("sample", run_id, child_id).lines_json())
    [fork] = [line for line in lines if line["kind"] == "fork"]
    assert fork["minute"] == FORK
    assert fork["approved_program_hash"] == summary["intervention"]["approved_program_hash"]

    def working(frame: dict[str, Any]) -> int:
        [toilet] = [item for item in frame["facilities"] if item["id"] == "toilet_01"]
        return toilet["working_units"]

    assert (working(main), working(child)) == (1, 0)


def test_a_copy_of_the_sample_answers_the_routes_the_new_ui_opens_it_with(
    builder: dict[str, Any], build: Callable[[str], dict[str, Any]], tmp_path: Path
) -> None:
    """The VM copies the built sample into each slot; the copy opens like the original,
    with people at physical points the map draws."""
    build("built")
    shutil.copytree(tmp_path / "built" / "sample", tmp_path / "slot" / "sample")
    run_id, child_id = builder["RUN_ID"], builder["CHILD_ID"]
    api = ProjectAPI(ProjectStore(tmp_path / "slot"))
    try:
        assert [p["project_id"] for p in api.get([])["projects"]] == ["sample"]
        assert [r["run_id"] for r in api.get(["sample", "runs"])["runs"]] == [run_id]
        branch = ["sample", "runs", run_id, "branches", child_id]
        frame = api.get([*branch, "timeline"], {"minute": [str(FORK)]})
        assert (frame["branch_id"], frame["minute"]) == (child_id, FORK)
        assert frame["agents"]
        assert all(agent["point"] is not None for agent in frame["agents"])
        assert "toilet_01" in frame["geometry"]["facility_points"]
        assert api.get([*branch, "llm-status"])["mode"] == "offline"
        comparison = api.get(
            ["sample", "runs", run_id, "compare"],
            {"branch_ids": [f"main,{child_id}"], "minute": [str(END)]},
        )
        assert [f["branch_id"] for f in comparison["frames"]] == ["main", child_id]
    finally:
        api.close()


def test_the_vm_landing_page_links_to_the_built_sample(
    build: Callable[[str], dict[str, Any]],
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    summary = build("projects")
    path = tmp_path / "sample-build.json"
    path.write_text(json.dumps(summary), encoding="utf-8")
    monkeypatch.setattr(sys, "argv", ["render_landing.py", "example", "1", str(path)])
    runpy.run_path(str(LANDING), run_name="__main__")
    page = capsys.readouterr().out
    href = html.escape(f"https://s1.example{summary['open_path']}")
    assert f'<a href="{href}">計算済みの例をすぐ見る</a>' in page


def test_a_sample_is_rebuilt_only_when_a_build_now_would_give_another_result(
    builder: dict[str, Any],
    build: Callable[[str], dict[str, Any]],
    generated_request: Callable[..., dict[str, Any]],
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """The VM's setup.sh asks --check before it rebuilds (docs/vm-deploy.md)."""
    summary = build("sample")
    differ = builder["sample_difference"]

    def check(built: dict[str, Any], **plan: Any) -> str | None:
        return differ(
            tmp_path / "sample",
            built,
            **{
                "text": "",
                "request": generated_request(duration_days=END / 1440),
                "fork_minute": FORK,
                **plan,
            },
        )

    # Another app commit with the same plan, Scenario and model: nothing to rebuild.
    assert check(summary) is None
    assert check(summary, fork_minute=FORK + 1) == "the plan changed: fork_minute, intervention"
    other = json.loads(json.dumps(summary))
    other["scenario"]["scenario_hash"] = "0" * 64
    assert check(other) == "the Scenario the preview makes from the text changed"
    # A sample the Python server stored before #427 (no journal) is built again.
    journal = Path(tmp_path / "sample" / "sample" / "runs" / summary["run_id"] / "branches")
    journal = journal / "main" / "journal.jsonl"
    kept = journal.read_bytes()
    journal.unlink()
    assert check(summary) == "the model changed for branch main"
    journal.write_bytes(kept)
    assert check(summary) is None
    shutil.rmtree(tmp_path / "sample" / "sample")
    monkeypatch.setenv("SHELTERSIM_LLM_MODE", "offline")
    assert check(summary).startswith("branch main cannot be read")


def test_the_check_command_answers_setup_sh_with_its_first_line_and_status(
    builder: dict[str, Any],
    build: Callable[[str], dict[str, Any]],
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """setup.sh rebuilds unless --check prints `current`; this short sample was built from
    another text than the example the command plans with."""
    summary = build("sample")
    path = tmp_path / "sample-build.json"
    path.write_text(json.dumps(summary), encoding="utf-8")
    monkeypatch.setattr(sys, "argv", ["build", str(tmp_path / "sample"), "--check"])
    assert builder["main"]() == 3
    assert capsys.readouterr().out == "rebuild: the plan changed: fork_minute, intervention, text\n"
    path.unlink()
    assert builder["main"]() == 3
    assert capsys.readouterr().out.startswith("rebuild: no readable summary")


def test_the_sample_is_written_as_one_file_the_browser_imports(
    builder: dict[str, Any], build: Callable[[str], dict[str, Any]], tmp_path: Path
) -> None:
    """--bundle (#427): the same Project as the browser's bundle (#265), what made it in its
    manifest, without the server's own files."""
    summary = build("projects")
    path = tmp_path / "sample.shelterproj"
    source = builder["write_bundle"](tmp_path / "projects", path, summary)
    bundle = path.read_bytes()
    assert bundle[:16] == b"\x89SHELTERSIM-PRJ\n"
    length = int.from_bytes(bundle[20:24], "big")
    manifest = json.loads(bundle[56 : 56 + length])
    assert (manifest["format"], manifest["version"]) == ("sheltersim-project-bundle", 2)
    assert manifest["project_id"] == "sample"
    assert manifest["source"] == source
    assert source["tool"] == "scripts/build_project_sample.py"
    paths = [entry["path"] for entry in manifest["entries"]]
    run = f"runs/{builder['RUN_ID']}"
    for branch in ("main", builder["CHILD_ID"]):
        for name in ("branch.json", "journal.jsonl", "data.pack"):
            assert f"{run}/branches/{branch}/{name}" in paths
    assert not any("worker-lock" in path or path.startswith("drafts") for path in paths)
