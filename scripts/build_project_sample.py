"""Build the precomputed example Projects that the screen (/) opens at once.

    SHELTERSIM_LLM_MODE=offline python scripts/build_project_sample.py OUTPUT_PROJECTS_DIR

OUTPUT_PROJECTS_DIR is a projects folder, such as `<runs-dir>/.projects` of
`sheltersim web`. The script creates the examples' Projects in it (EXAMPLES, the first is the one
the home lists first and the VM's landing page links to): each one Run of a Scenario made from
its text (synthetic people in a standard synthetic shelter, at most 100 people and 7 days, seed
2026), the branch `main` computed to the end, and one branch forked with an intervention and
computed to the end. The three show different interventions whose effect the comparison screen
shows clearly: people arriving in the evening (東京都港区), more staff (墨田区) and a
12-hour water outage (江戸川区). The municipality and the disaster are the Scenario's words only:
the regional research is not run, so they do not change what is computed.

It goes through the ProjectAPI routes the new UI calls, in this process: the Project,
`scenario-preview` of the text with the offline parser, `scenario-approve`, `resume` to the end,
`intervention-preview` of the typed program the comparison screen builds from the core's catalog
(#584), `intervention-apply` with explicit approval, and `resume` of the child. No Provider is
called: the LLM mode is forced offline, the API key is dropped, and the usage ledger
(XDG_STATE_HOME) and response tape point to a temporary directory. The regional research of the
preview, which reads public web services, is not run: the Scenario is built from the standard
synthetic shelter and the recorded defaults alone, as a preview without research results is, and
the research record says so with a fixed date.

    SHELTERSIM_LLM_MODE=offline python scripts/build_project_sample.py OUTPUT_PROJECTS_DIR \\
        --bundles BUNDLE_DIR

also writes each example as one file the browser imports (`BUNDLE_DIR/<project>.shelterproj`,
#265): the site's examples (scripts/build_static_site.py --bundle, in EXAMPLES' order).

Use --example PROJECT_ID to build or check only the named example.

The same texts, seeds and model give the same hashes on every build. A JSON summary (each
example's ids, Scenario, minutes, heads and the core's digests, size) is printed and written to
`<OUTPUT>/../sample-build.json` unless --summary names another file. Open an example with
`/?project=<project>&run=<run_id>&branch=main` (docs/demo-guide.md).

    python scripts/build_project_sample.py OUTPUT_PROJECTS_DIR --check

answers, without computing, whether the examples built earlier in OUTPUT_PROJECTS_DIR are what a
build now would give: exit 0 if so, 3 with the reason if not. A build gives the same result when
its plans (texts, ids, fork minutes, interventions) are the same, the Scenarios the preview makes
from the texts have the same hashes, and the core records the same model versions for them as the
branches' stored Runs did (they are not view-only), as a Run is continued only then.

The Projects are in the browser's folders and format v6 (#427): the server reads them, and the
browser can import each as one file (`ProjectFiles.export_project`).
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import tempfile
import time
from dataclasses import dataclass, field
from datetime import UTC, datetime
from pathlib import Path
from typing import Any
from urllib.parse import urlencode

from sheltersim.experiments.scenario import ScenarioSpec
from sheltersim.llm.config import LLMMode, LLMSettings
from sheltersim.storage.projects import ProjectStore


@dataclass(frozen=True)
class Example:
    """One example: a Project with one Run of the Scenario of `text`, and a branch forked at
    `fork_minute` with the operator `op` of the core's catalog on `target` ("zone:<id>" or
    "facility:<id>") with `parameters`, lasting `minutes` (None: to the Scenario's end)."""

    project_id: str
    run_id: str
    child_id: str
    text: str
    fork_minute: int
    op: str
    target: str
    parameters: dict[str, Any] = field(default_factory=dict)
    minutes: int | None = None
    # What the branch's program says of itself (the screen's label of its built program).
    label: str = ""


# The examples, the one the home lists first (and the VM's landing page links to) first. Example
# Projects are `sample` and `sample-…` (the screen tells them from one's own Projects by that,
# demo-v2-state.js). The screen names a branch by its id's last 6 characters (demo-v2-text.js's
# shortBranch): arrive, 5staff, outage. Chosen among 19 trial builds for effects the comparison
# shows: the toilet outage of the earlier single example changed the 24-hour toilet wait by about
# 0.5 min.
EXAMPLES = (
    Example(
        project_id="sample",
        run_id="run-minato-arrivals",
        child_id="branch-arrive",
        # The floor area gives room for the 30 who arrive (70 people alone get about 73 places).
        text=(
            "東京都港区で地震が発生。避難者70人、7日間、面積700㎡。"
            "高齢者や子ども連れの支援ニーズも見たい。"
        ),
        fork_minute=18 * 60,
        op="add_evacuees",
        target="zone:sleeping",
        parameters={"count": 30},
        label="避難者が到着する · 区域：避難スペース · 人数30人 · 1日目 18:00（帰宅困難者を想定）",
    ),
    Example(
        project_id="sample-2",
        run_id="run-sumida-staff",
        child_id="branch-5staff",
        text="墨田区で地震が発生。避難者100人、7日間。高齢者や子ども連れの支援ニーズも見たい。",
        fork_minute=12 * 60,
        op="add_staff",
        target="zone:staff",
        parameters={"count": 5},
        label="スタッフを増やす · 区域：運営本部 · 人数5人 · 1日目 12:00",
    ),
    Example(
        project_id="sample-3",
        run_id="run-edogawa-water-outage",
        child_id="branch-water-outage",
        text="江戸川区で水害が発生。避難者100人、5日間。高齢者や子ども連れの支援ニーズも見たい。",
        fork_minute=1440 + 8 * 60,
        op="water_outage",
        target="zone:food",
        minutes=12 * 60,
        label="断水する · 区域：食事・給水スペース · 2日目 08:00から2日目 20:00まで",
    ),
)
# The Project the fixtures' builders (tests/support) make their Runs in.
PROJECT_ID = EXAMPLES[0].project_id
# The branch the landing page opens: the original Scenario from Day 1 00:00, which plays
# from the start; the comparison screen pairs it with the intervention's branch.
OPEN_BRANCH_ID = "main"
# The research record's date; fixed, since no research is run (see the module docstring).
RESEARCHED_AT = datetime(2026, 1, 1, tzinfo=UTC)
RESEARCH_WARNING = (
    "計算済みの例のため、地域の公開データは調べていません。"
    "標準の合成施設と既定値だけで作ったシナリオです。"
)
# As the old demo's replay said of itself (removed in #376).
NOTE = (
    "Precomputed offline sample of a synthetic scenario, not a live simulation and not "
    "real shelter records. The single-seed comparison is descriptive and is not a "
    "prediction or evidence of real-world effects."
)


def open_path(project_id: str, run_id: str, branch_id: str) -> str:
    """The new UI's address of a saved branch; the VM landing page links to it."""
    query = urlencode({"project": project_id, "run": run_id, "branch": branch_id})
    return f"/?{query}#simulation"


def _no_research(query: Any, **_kwargs: Any) -> Any:
    """The research of a preview, without the public web services it would read."""
    from sheltersim.region.models import RegionResearchResult

    return RegionResearchResult(
        request=query, researched_at=RESEARCHED_AT, warnings=(RESEARCH_WARNING,)
    )


def program(example: Example, catalog: list[dict[str, Any]], minute: int) -> dict[str, Any]:
    """The program the comparison screen builds for the example's intervention at `minute` from
    the core's catalog (`GET {project}/contracts`, demo-v2-comparison.js's builtProgram, #584)."""
    entry = next((item for item in catalog if item["op"] == example.op), None)
    if entry is None:
        raise ValueError(f"the core's catalog has no operator {example.op}")
    kind, target = example.target.split(":", 1)
    end = minute + example.minutes if example.minutes is not None else None
    return {
        "schema_version": 1,
        "program_id": f"manual-{example.run_id}",
        "narrative": example.label,
        "source_text": example.label,
        "simulation_start_minute": minute,
        "effects": [
            {
                "effect_id": "effect-1",
                "op": example.op,
                "target_selector": {"kind": kind, "target_id": target, "candidate_ids": []},
                "start_minute": minute,
                "end_minute": end,
                "guard": None,
                "parameters": example.parameters,
                "units": entry["units"][0],
                "source_id": "screen-builder-v1",
                "assumption_ids": [],
                "policy_dependencies": entry["required_policies"],
                "causation_id": None,
                "reversible": True,
            }
        ],
        "assumptions": [],
        "support_mode": "native",
        "compiler_provenance": {
            "compiler_kind": "manual",
            "compiler_version": "screen-builder-v1",
            "model_id": None,
            "prompt_hash": None,
        },
    }


def build_sample(
    projects_dir: Path,
    example: Example = EXAMPLES[0],
    *,
    text: str | None = None,
    request: dict[str, Any] | None = None,
    fork_minute: int | None = None,
) -> dict[str, Any]:
    """Create the example's Project in projects_dir and compute both branches to the end.

    text is what the operator types on the Scenario screen (the example's by default); request,
    when given, is the structured form the screen sends with it (tests use a short one).
    """
    from sheltersim.web.projects import ProjectAPI

    if LLMSettings.from_env().effective_mode is not LLMMode.OFFLINE:
        raise RuntimeError("the sample is built offline only; set SHELTERSIM_LLM_MODE=offline")
    text = example.text if text is None else text
    fork_minute = example.fork_minute if fork_minute is None else fork_minute
    project_id, run_id, child_id = example.project_id, example.run_id, example.child_id
    store = ProjectStore(projects_dir)
    if (store.root / project_id).exists():
        raise FileExistsError(f"{store.root / project_id} exists; build into an empty directory")
    started = time.perf_counter()
    api = ProjectAPI(store, recover=False)
    api.workspace.research.research = _no_research
    try:
        draft, scenario = _preview(api, text, request, project_id)
        end = scenario.duration_minutes
        if not 0 < fork_minute < end:
            raise ValueError("the fork minute must fall inside the Scenario")
        api.post(
            [project_id, "scenario-approve"],
            {
                "preview_id": draft["preview_id"],
                "approved_hash": draft["approved_hash"],
                "approved": True,
                "run_id": run_id,
            },
        )
        _compute(api, example, "main", end)

        catalog = api.get([project_id, "contracts"])["executable_effect_primitives"]
        branch = [project_id, "runs", run_id, "branches", "main"]
        preview = api.post(
            [*branch, "intervention-preview"],
            {"minute": fork_minute, "program": program(example, catalog, fork_minute)},
        )
        if preview.get("status") != "compiled" or not preview.get("can_apply"):
            raise RuntimeError(f"the intervention preview cannot be applied: {preview}")
        if preview.get("usage", {}).get("api_request_count", 0) != 0:
            raise RuntimeError("the intervention preview called a Provider")
        api.post(
            [*branch, "intervention-apply"],
            {
                "preview_id": preview["preview_id"],
                "approved_program_hash": preview["approved_program_hash"],
                "approved": True,
                "child_id": child_id,
            },
        )
        _compute(api, example, child_id, end)
    finally:
        api.close()

    geometry = scenario.model_configs.get("facility_geometry") or {}
    return {
        "schema_version": 2,
        "kind": "precomputed_project_sample",
        "note": NOTE,
        "project_id": project_id,
        "run_id": run_id,
        "scenario": {
            "text": text,
            "municipality": draft["request"]["municipality"],
            "disaster": draft["request"]["disaster"],
            "scenario_id": scenario.id,
            "scenario_hash": draft["approved_hash"],
            "seed": scenario.seed,
            "facilities": sorted(item.id for item in scenario.facilities),
            "toilets": sorted(item.id for item in scenario.facilities if item.kind == "toilet"),
            "has_facility_geometry": bool(geometry),
            "regional_research": "not_run",
        },
        "agent_count": len(scenario.profiles),
        "duration_minutes": end,
        "fork_minute": fork_minute,
        "intervention": {
            "text": example.label,
            "op": example.op,
            "target": example.target,
            "parameters": example.parameters,
            "minutes": example.minutes,
            "support_mode": preview.get("support_mode"),
            "fallback_used": preview.get("fallback_used"),
            "approved_program_hash": preview["approved_program_hash"],
        },
        "branches": [
            _branch_summary(api.files, example, "main"),
            _branch_summary(api.files, example, child_id),
        ],
        "open_branch_id": OPEN_BRANCH_ID,
        "open_path": open_path(project_id, run_id, OPEN_BRANCH_ID),
        "size_bytes": sum(
            path.stat().st_size for path in (store.root / project_id).rglob("*") if path.is_file()
        ),
        "build_seconds": round(time.perf_counter() - started, 1),
    }


def _preview(
    api: Any, text: str, request: dict[str, Any] | None, project_id: str = PROJECT_ID
) -> tuple[dict[str, Any], ScenarioSpec]:
    """Create the Project and preview its Scenario from the text, offline."""
    api.post([], {"project_id": project_id})
    draft = api.post(
        [project_id, "scenario-preview"],
        {"text": text, **({"request": request} if request is not None else {})},
    )
    if not draft.get("can_start"):
        raise RuntimeError(
            "the Scenario preview cannot start: "
            f"{draft.get('missing_fields') or draft.get('approval_blockers')}"
        )
    if draft["llm_usage"]["api_request_count"] != 0:
        raise RuntimeError("the Scenario preview called a Provider")
    return draft, ScenarioSpec.model_validate(draft["scenario"])


def _plan(example: Example, text: str, fork_minute: int) -> dict[str, Any]:
    """What a build of the example is made from, as its summary records it."""
    return {
        "kind": "precomputed_project_sample",
        "project_id": example.project_id,
        "run_id": example.run_id,
        "branch_ids": ["main", example.child_id],
        "text": text,
        "fork_minute": fork_minute,
        "intervention": {
            "text": example.label,
            "op": example.op,
            "target": example.target,
            "parameters": example.parameters,
            "minutes": example.minutes,
        },
        "open_branch_id": OPEN_BRANCH_ID,
    }


def _built(summary: dict[str, Any]) -> dict[str, Any]:
    intervention = summary.get("intervention", {})
    return {
        "kind": summary.get("kind"),
        "project_id": summary.get("project_id"),
        "run_id": summary.get("run_id"),
        "branch_ids": [branch.get("branch_id") for branch in summary.get("branches", [])],
        "text": summary.get("scenario", {}).get("text"),
        "fork_minute": summary.get("fork_minute"),
        "intervention": {
            key: intervention.get(key) for key in ("text", "op", "target", "parameters", "minutes")
        },
        "open_branch_id": summary.get("open_branch_id"),
    }


def sample_difference(
    projects_dir: Path,
    summary: dict[str, Any],
    example: Example = EXAMPLES[0],
    *,
    text: str | None = None,
    request: dict[str, Any] | None = None,
    fork_minute: int | None = None,
) -> str | None:
    """None if the example in projects_dir is what a build now would give, else why not.

    Nothing is computed: the plan is compared, the Scenario is previewed into a
    temporary store, and the installed model signatures are compared with those the
    branches' head Checkpoints recorded.
    """
    from sheltersim.web.projects import ProjectAPI

    if LLMSettings.from_env().effective_mode is not LLMMode.OFFLINE:
        raise RuntimeError("the sample is checked offline only; set SHELTERSIM_LLM_MODE=offline")
    text = example.text if text is None else text
    fork_minute = example.fork_minute if fork_minute is None else fork_minute
    plan, built = _plan(example, text, fork_minute), _built(summary)
    changed = sorted(key for key in plan if plan[key] != built[key])
    if changed:
        return f"the plan of {example.project_id} changed: " + ", ".join(changed)
    with tempfile.TemporaryDirectory(prefix="sheltersim-sample-check-") as scratch:
        api = ProjectAPI(ProjectStore(Path(scratch)), recover=False)
        api.workspace.research.research = _no_research
        try:
            draft, _ = _preview(api, text, request, example.project_id)
        finally:
            api.close()
    if draft["approved_hash"] != summary["scenario"].get("scenario_hash"):
        return f"the Scenario the preview makes from the text of {example.project_id} changed"
    from sheltersim.web.projects import answers
    from sheltersim.web.stored_runs import project_files

    files = project_files(ProjectStore(projects_dir))
    for branch_id in plan["branch_ids"]:
        try:
            journal = files.branch_file(
                example.project_id, example.run_id, branch_id, "journal.jsonl"
            )
            scenario = files.run_scenario_bytes(example.project_id, example.run_id)
            view_only = journal is None or answers().view_only(journal, scenario)
        except Exception as exc:  # A missing or unreadable sample is rebuilt.
            return f"branch {branch_id} of {example.project_id} cannot be read: {exc}"
        if view_only:
            return f"the model changed for branch {branch_id} of {example.project_id}"
    return None


def samples_difference(
    projects_dir: Path, summary: dict[str, Any], examples: tuple[Example, ...] = EXAMPLES
) -> str | None:
    """None if every example in projects_dir is what a build now would give (the summary of
    main(), its `examples` in EXAMPLES' order), else why not."""
    built = summary.get("examples")
    if not isinstance(built, list) or [item.get("project_id") for item in built] != [
        example.project_id for example in examples
    ]:
        return "the examples changed"
    for example, item in zip(examples, built, strict=True):
        reason = sample_difference(projects_dir, item, example)
        if reason is not None:
            return reason
    return None


def _compute(api: Any, example: Example, branch_id: str, end: int) -> None:
    """Resume a branch to the end, as the new UI's compute button does, and wait for it."""
    project_id, run_id = example.project_id, example.run_id
    api.post([project_id, "runs", run_id, "resume"], {"branch_id": branch_id, "target_minute": end})
    api.wait_idle()
    record = json.loads(api.files.branch_record(project_id, run_id, branch_id))
    status, reached = record.get("status"), record["computed_until"]
    if status != "completed" or reached != end:
        raise RuntimeError(f"branch {branch_id} ended {status} at minute {reached}, not {end}")


def _branch_summary(files: Any, example: Example, branch_id: str) -> dict[str, Any]:
    """The branch's head, the core's digests of the world it ended with and of its metric
    samples, the model versions it was computed with and the state its fork started from."""
    project_id, run_id = example.project_id, example.run_id
    record = json.loads(files.branch_record(project_id, run_id, branch_id))
    reader = files.reader(project_id, run_id, branch_id)
    run, _, _ = reader.seek(record["computed_until"])
    fork = next(
        (line for line in json.loads(reader.lines_json()) if line.get("kind") == "fork"), None
    )
    return {
        "branch_id": branch_id,
        "parent_branch_id": record["parent_branch_id"],
        "fork_minute": record["fork_minute"],
        "computed_until": record["computed_until"],
        "head": record["checkpoint_hash"],
        "state_hash": run.state_digest(),
        "world_digests": json.loads(run.digests_json()),
        "metrics_digest": run.metrics_digest(),
        "model_versions": json.loads(reader.model_versions_json()),
        "fork_origin_state_hash": fork["fork_origin"]["state_hash"] if fork else None,
    }


def write_bundle(projects_dir: Path, path: Path, summary: dict[str, Any]) -> dict[str, Any]:
    """Write the example's Project of projects_dir (the summary's) as one file the browser
    imports (the browser's bundle, `.shelterproj`, #265): a site's example
    (scripts/build_static_site.py). What made it is kept in its manifest's `source`. Returns that
    `source`."""
    from sheltersim.web.stored_runs import project_files

    source = {
        "tool": "scripts/build_project_sample.py",
        "text": summary["scenario"]["text"],
        "agent_count": summary["agent_count"],
        "duration_minutes": summary["duration_minutes"],
        "seed": summary["scenario"]["seed"],
        "engine_version": summary["branches"][0]["model_versions"]["engine:model"],
        "regional_research": summary["scenario"]["regional_research"],
        "intervention": summary["intervention"]["text"],
        "note": NOTE,
    }
    files = project_files(ProjectStore(projects_dir))
    with files.transaction():
        bundle = files.export_project(summary["project_id"], json.dumps(source, ensure_ascii=False))
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(bundle)
    return source


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("output", type=Path, help="ProjectStore root to create the examples in")
    parser.add_argument(
        "--summary",
        type=Path,
        help="where to write the JSON summary (default: sample-build.json next to OUTPUT)",
    )
    parser.add_argument(
        "--bundles",
        type=Path,
        help="also write each example as one file the browser imports, <project>.shelterproj",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="compute nothing; exit 0 if the examples in OUTPUT are what a build now would give",
    )
    parser.add_argument(
        "--example",
        choices=[example.project_id for example in EXAMPLES],
        help="build or check just this example (default: all examples)",
    )
    arguments = parser.parse_args()
    selected = tuple(
        example
        for example in EXAMPLES
        if arguments.example is None or arguments.example == example.project_id
    )
    output = arguments.output.resolve()
    summary_path = (arguments.summary or output.parent / "sample-build.json").resolve()
    with tempfile.TemporaryDirectory(prefix="sheltersim-sample-") as scratch:
        # Never a Provider, and never the shared usage ledger or response tape.
        os.environ["SHELTERSIM_LLM_MODE"] = "offline"
        os.environ.pop("ANTHROPIC_API_KEY", None)
        os.environ["XDG_STATE_HOME"] = str(Path(scratch) / "state")
        os.environ["SHELTERSIM_LLM_CACHE_PATH"] = str(Path(scratch) / "llm-tape.json")
        os.environ["SHELTERSIM_PROJECTS_DIR"] = str(output)
        if arguments.check:
            try:
                built = json.loads(summary_path.read_text(encoding="utf-8"))
            except (OSError, ValueError) as exc:
                print(f"rebuild: no readable summary ({exc})")
                return 3
            reason = samples_difference(output, built, selected)
            print("current" if reason is None else f"rebuild: {reason}")
            return 0 if reason is None else 3
        examples = [build_sample(output, example) for example in selected]
        if arguments.bundles is not None:
            for item in examples:
                path = arguments.bundles.resolve() / f"{item['project_id']}.shelterproj"
                write_bundle(output, path, item)
    summary = {
        "schema_version": 2,
        "kind": "precomputed_project_samples",
        "note": NOTE,
        "examples": examples,
    }
    summary_path.parent.mkdir(parents=True, exist_ok=True)
    summary_path.write_text(
        json.dumps(summary, ensure_ascii=False, sort_keys=True, indent=2) + "\n", encoding="utf-8"
    )
    json.dump(summary, sys.stdout, ensure_ascii=False, sort_keys=True, indent=2)
    print()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
