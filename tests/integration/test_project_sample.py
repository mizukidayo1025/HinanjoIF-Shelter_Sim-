"""The precomputed example Projects (scripts/build_project_sample.py) that the screen opens.

Short variants of the examples are built through the builder: the same ProjectAPI routes the new
UI calls (scenario-preview and scenario-approve of a generated Scenario, resume,
intervention-preview of the typed program the comparison screen builds and intervention-apply),
with the structured form the Scenario screen sends for 20 people and 0.1 days instead of the
examples' texts, and an earlier fork. The examples themselves are built on the judges' VM
(docs/vm-deploy.md) and for the browser site (deploy-browser.yml).

The examples are in the browser's folders and format v6 (#427); the browser's form of each is
the same Project written as one file (`--bundles`, #265), which native/wasm/tests import.
"""

from __future__ import annotations

import dataclasses
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

# Run on every pull request on Windows and macOS too (pytest --os-subset, #380): the example
# Projects are built by a script that writes, copies and deletes files.
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
) -> Callable[..., dict[str, Any]]:
    monkeypatch.setenv("SHELTERSIM_LLM_MODE", "offline")
    monkeypatch.delenv("ANTHROPIC_API_KEY", raising=False)

    def run(name: str, index: int = 0) -> dict[str, Any]:
        example = builder["EXAMPLES"][index]
        # A timed intervention ends within the short Scenario.
        if example.minutes is not None:
            example = dataclasses.replace(example, minutes=(END - FORK) // 2)
        return builder["build_sample"](
            tmp_path / name,
            example,
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


def test_the_examples_stay_within_100_people_and_7_days_and_can_start(
    builder: dict[str, Any], tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The examples' plans (decided for the hackathon of 2026-11-22): at most 100 people at any
    time, arrivals included, and at most 7 days; the first is the one in 東京都港区; the Projects
    are `sample` and `sample-…`, which the screen takes for examples (demo-v2-state.js)."""
    monkeypatch.setenv("SHELTERSIM_LLM_MODE", "offline")
    monkeypatch.delenv("ANTHROPIC_API_KEY", raising=False)
    examples = builder["EXAMPLES"]
    assert [example.project_id for example in examples] == ["sample", "sample-2", "sample-3"]
    assert len({example.run_id for example in examples}) == len(examples)
    assert len({example.op for example in examples}) == len(examples)
    api = ProjectAPI(ProjectStore(tmp_path), recover=False)
    api.workspace.research.research = builder["_no_research"]
    try:
        for example in examples:
            draft, scenario = builder["_preview"](api, example.text, None, example.project_id)
            arriving = example.parameters["count"] if example.op == "add_evacuees" else 0
            assert len(scenario.profiles) + arriving <= 100, example.project_id
            places = {zone.id: zone.capacity for zone in scenario.zones}["sleeping"]
            assert len(scenario.profiles) + arriving <= places, example.project_id
            assert scenario.duration_minutes <= 7 * 1440, example.project_id
            assert 0 < example.fork_minute < scenario.duration_minutes
            if example.minutes is not None:
                assert example.fork_minute + example.minutes <= scenario.duration_minutes
            assert draft["can_start"] is True
    finally:
        api.close()
    assert examples[0].text.startswith("東京都港区")


def test_two_builds_of_an_example_give_the_same_hashes(
    build: Callable[..., dict[str, Any]],
) -> None:
    first, second = build("first"), build("second")
    assert first["scenario"]["scenario_hash"] == second["scenario"]["scenario_hash"]
    assert _results(first) == _results(second)
    assert first["intervention"] == second["intervention"]
    assert first["branches"][0]["state_hash"] != first["branches"][1]["state_hash"]


@pytest.mark.parametrize("index", [0, 1, 2])
def test_each_example_holds_a_completed_run_and_its_approved_branch(
    builder: dict[str, Any], build: Callable[..., dict[str, Any]], tmp_path: Path, index: int
) -> None:
    example = builder["EXAMPLES"][index]
    summary = build("projects", index)
    project, run_id, child_id = example.project_id, example.run_id, example.child_id
    assert summary["project_id"] == project
    assert summary["run_id"] == run_id
    assert [b["branch_id"] for b in summary["branches"]] == ["main", child_id]
    assert summary["intervention"]["text"] == example.label
    assert summary["intervention"]["op"] == example.op
    assert summary["intervention"]["support_mode"] == "native"
    assert summary["scenario"]["has_facility_geometry"] is True
    assert summary["open_branch_id"] == "main"
    assert summary["open_path"] == f"/?project={project}&run={run_id}&branch=main#simulation"

    api = ProjectAPI(ProjectStore(tmp_path / "projects"), recover=False)
    try:
        assert [p["project_id"] for p in api.get([])["projects"]] == [project]
        saved = api.get([project])
        run = api.get([project, "runs", run_id])
        branch = [project, "runs", run_id, "branches"]
        main = api.get([*branch, "main", "timeline"], f"minute={END}")
        child = api.get([*branch, child_id, "timeline"], f"minute={END}")
    finally:
        api.close()
    assert run["status"] == "completed"
    [scenario] = saved["scenarios"]
    provenance = scenario["scenario"]["provenance"]
    assert provenance["region_research"]["warnings"] == [builder["RESEARCH_WARNING"]]
    branches = {b["branch_id"]: b for b in run["branches"]}
    assert branches["main"]["computed_until"] == END
    assert branches[child_id]["parent_branch_id"] == "main"
    assert branches[child_id]["fork_minute"] == FORK
    assert branches[child_id]["computed_until"] == END
    lines = json.loads(api.files.reader(project, run_id, child_id).lines_json())
    [fork] = [line for line in lines if line["kind"] == "fork"]
    assert fork["minute"] == FORK
    assert fork["approved_program_hash"] == summary["intervention"]["approved_program_hash"]
    # The intervention changed the world as its operator does.
    if example.op == "add_evacuees":
        count = example.parameters["count"]
        assert main["occupancy"]["admitted_count"] + count == (
            child["occupancy"]["admitted_count"] + child["occupancy"]["waiting_outside_count"]
        )
    elif example.op == "add_staff":
        assert len(child["staff"]) == len(main["staff"]) + example.parameters["count"]
    else:
        assert main["state_hash"] != child["state_hash"]


def test_a_copy_of_an_example_answers_the_routes_the_new_ui_opens_it_with(
    builder: dict[str, Any], build: Callable[..., dict[str, Any]], tmp_path: Path
) -> None:
    """The VM copies the built examples into each slot; a copy opens like the original,
    with people at physical points the map draws."""
    build("built")
    example = builder["EXAMPLES"][0]
    project, run_id, child_id = example.project_id, example.run_id, example.child_id
    shutil.copytree(tmp_path / "built" / project, tmp_path / "slot" / project)
    api = ProjectAPI(ProjectStore(tmp_path / "slot"))
    try:
        assert [p["project_id"] for p in api.get([])["projects"]] == [project]
        assert [r["run_id"] for r in api.get([project, "runs"])["runs"]] == [run_id]
        branch = [project, "runs", run_id, "branches", child_id]
        frame = api.get([*branch, "timeline"], {"minute": [str(FORK)]})
        assert (frame["branch_id"], frame["minute"]) == (child_id, FORK)
        assert frame["agents"]
        assert all(agent["point"] is not None for agent in frame["agents"])
        assert api.get([*branch, "llm-status"])["mode"] == "offline"
        comparison = api.get(
            [project, "runs", run_id, "compare"],
            {"branch_ids": [f"main,{child_id}"], "minute": [str(END)]},
        )
        assert [f["branch_id"] for f in comparison["frames"]] == ["main", child_id]
    finally:
        api.close()


def test_the_vm_landing_page_links_to_the_first_built_example(
    build: Callable[..., dict[str, Any]],
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    summary = build("projects")
    path = tmp_path / "sample-build.json"
    path.write_text(json.dumps({"examples": [summary]}), encoding="utf-8")
    monkeypatch.setattr(sys, "argv", ["render_landing.py", "example", "1", str(path)])
    runpy.run_path(str(LANDING), run_name="__main__")
    page = capsys.readouterr().out
    href = html.escape(f"https://s1.example{summary['open_path']}")
    assert f'<a href="{href}">計算済みの例をすぐ見る</a>' in page


def test_an_example_is_rebuilt_only_when_a_build_now_would_give_another_result(
    builder: dict[str, Any],
    build: Callable[..., dict[str, Any]],
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
            builder["EXAMPLES"][0],
            **{
                "text": "",
                "request": generated_request(duration_days=END / 1440),
                "fork_minute": FORK,
                **plan,
            },
        )

    # Another app commit with the same plan, Scenario and model: nothing to rebuild.
    assert check(summary) is None
    assert check(summary, fork_minute=FORK + 1) == "the plan of sample changed: fork_minute"
    other = json.loads(json.dumps(summary))
    other["intervention"]["parameters"] = {"count": 1}
    assert check(other) == "the plan of sample changed: intervention"
    other = json.loads(json.dumps(summary))
    other["scenario"]["scenario_hash"] = "0" * 64
    assert check(other) == "the Scenario the preview makes from the text of sample changed"
    # An example the Python server stored before #427 (no journal) is built again.
    journal = Path(tmp_path / "sample" / "sample" / "runs" / summary["run_id"] / "branches")
    journal = journal / "main" / "journal.jsonl"
    kept = journal.read_bytes()
    journal.unlink()
    assert check(summary) == "the model changed for branch main of sample"
    journal.write_bytes(kept)
    assert check(summary) is None
    shutil.rmtree(tmp_path / "sample" / "sample")
    monkeypatch.setenv("SHELTERSIM_LLM_MODE", "offline")
    assert check(summary).startswith("branch main of sample cannot be read")


def test_the_check_command_answers_setup_sh_with_its_first_line_and_status(
    builder: dict[str, Any],
    build: Callable[..., dict[str, Any]],
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """setup.sh rebuilds unless --check prints `current`; this short build holds one example
    where the command plans with all of EXAMPLES, and then another text than the first's."""
    summary = build("sample")
    path = tmp_path / "sample-build.json"
    path.write_text(json.dumps({"examples": [summary]}), encoding="utf-8")
    monkeypatch.setattr(sys, "argv", ["build", str(tmp_path / "sample"), "--check"])
    assert builder["main"]() == 3
    assert capsys.readouterr().out == "rebuild: the examples changed\n"
    others = [dict(summary, project_id=e.project_id) for e in builder["EXAMPLES"]]
    path.write_text(json.dumps({"examples": others}), encoding="utf-8")
    assert builder["main"]() == 3
    assert capsys.readouterr().out == "rebuild: the plan of sample changed: fork_minute, text\n"
    path.unlink()
    assert builder["main"]() == 3
    assert capsys.readouterr().out.startswith("rebuild: no readable summary")


def test_an_example_is_written_as_one_file_the_browser_imports(
    builder: dict[str, Any], build: Callable[..., dict[str, Any]], tmp_path: Path
) -> None:
    """--bundles (#427): the same Project as the browser's bundle (#265), what made it in its
    manifest, without the server's own files."""
    example = builder["EXAMPLES"][1]
    summary = build("projects", 1)
    path = tmp_path / f"{example.project_id}.shelterproj"
    source = builder["write_bundle"](tmp_path / "projects", path, summary)
    bundle = path.read_bytes()
    assert bundle[:16] == b"\x89SHELTERSIM-PRJ\n"
    length = int.from_bytes(bundle[20:24], "big")
    manifest = json.loads(bundle[56 : 56 + length])
    assert (manifest["format"], manifest["version"]) == ("sheltersim-project-bundle", 2)
    assert manifest["project_id"] == example.project_id
    assert manifest["source"] == source
    assert source["tool"] == "scripts/build_project_sample.py"
    assert source["intervention"] == example.label
    paths = [entry["path"] for entry in manifest["entries"]]
    run = f"runs/{example.run_id}"
    for branch in ("main", example.child_id):
        for name in ("branch.json", "journal.jsonl", "data.pack"):
            assert f"{run}/branches/{branch}/{name}" in paths
    assert not any("worker-lock" in path or path.startswith("drafts") for path in paths)
