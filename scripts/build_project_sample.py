"""Build the precomputed sample Project that the screen (/) opens at once.

    SHELTERSIM_LLM_MODE=offline python scripts/build_project_sample.py OUTPUT_PROJECTS_DIR

OUTPUT_PROJECTS_DIR is a projects folder, such as `<runs-dir>/.projects` of
`sheltersim web`. The script creates the Project `sample` in it with one Run of the
Scenario the new UI generates from its example text (the 「例文を使う」 button:
100 synthetic people in a standard synthetic shelter in 墨田区, earthquake, 7 days,
seed 2026): the branch `main` computed to the end, and a branch forked at Day 4 14:00
(minute 5160) with "Day 4 14:00 toilet_01 outage", computed to the end. It is the
comparison of the old UI's fixed replay (removed in #376), on a Scenario whose map the new
UI draws.

It goes through the ProjectAPI routes the new UI calls, in this process: the Project,
`scenario-preview` of the text with the offline parser, `scenario-approve`, `resume` to
the end, `intervention-preview` with the offline parser, `intervention-apply` with
explicit approval, and `resume` of the child. No Provider is called: the LLM mode is
forced offline, the API key is dropped, and the usage ledger (XDG_STATE_HOME) and
response tape point to a temporary directory. The regional research of the preview,
which reads public web services, is not run: the Scenario is built from the standard
synthetic shelter and the recorded defaults alone, as a preview without research
results is, and the research record says so with a fixed date.

    SHELTERSIM_LLM_MODE=offline python scripts/build_project_sample.py OUTPUT_PROJECTS_DIR \\
        --bundle sample.shelterproj

also writes the sample as one file the browser imports (`.shelterproj`, #265): the site's
example (scripts/build_static_site.py). The VM's sample and the browser's are one Project in one
format since #427; scripts/build_browser_sample.py and its Node half are gone.

The same text, seed and model give the same hashes on every build. A JSON summary
(ids, Scenario, minutes, heads and the core's digests, size) is printed and written to
`<OUTPUT>/../sample-build.json` unless --summary names another file. Open the result
with `/?project=sample&run=<run_id>&branch=<branch_id>` (docs/demo-guide.md).

    python scripts/build_project_sample.py OUTPUT_PROJECTS_DIR --check

answers, without computing, whether a sample built earlier in OUTPUT_PROJECTS_DIR is what a
build now would give: exit 0 if so, 3 with the reason if not. A build gives the same
result when its plan (text, ids, fork minute, intervention) is the same, the Scenario the
preview makes from the text has the same hash, and the core records the same model versions
for it as the branches' stored Runs did (they are not view-only), as a Run is continued only
then. How the offline parser reads the intervention is not compared; rebuild after changing it.

The Project is in the browser's folders and format v5 (#427): the server reads it, and the
browser can import it as one file (`ProjectFiles.export_project`).
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import tempfile
import time
from datetime import UTC, datetime
from pathlib import Path
from typing import Any
from urllib.parse import urlencode

from sheltersim.experiments.scenario import ScenarioSpec
from sheltersim.llm.config import LLMMode, LLMSettings
from sheltersim.storage.projects import ProjectStore

PROJECT_ID = "sample"
RUN_ID = "run-sample-toilet-outage"
CHILD_ID = "branch-toilet01-outage"
# The branch the landing page opens: the original Scenario from Day 1 00:00, which plays
# from the start; the comparison screen pairs it with the outage branch. (Playing the
# outage branch from its fork minute stops at once in the screen's path drawing.)
OPEN_BRANCH_ID = "main"
# What the new UI's 「例文を使う」 button puts in the Scenario text (demo-v2.js).
TEXT = "墨田区で地震が発生。避難者100人、7日間。高齢者や子ども連れの支援ニーズも見たい。"
FORK_MINUTE = 3 * 24 * 60 + 14 * 60
# One of the generated standard shelter's three ordinary toilets; the accessible one stays.
FACILITY_ID = "toilet_01"
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


def intervention_text(minute: int) -> str:
    """The operator's request at minute, as the old demo's 'Day 4 14:00 toilet_1 outage'."""
    day, clock = minute // 1440 + 1, f"{minute % 1440 // 60:02d}:{minute % 60:02d}"
    return f"Day {day} {clock} {FACILITY_ID} outage"


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


def build_sample(
    projects_dir: Path,
    *,
    text: str = TEXT,
    request: dict[str, Any] | None = None,
    fork_minute: int = FORK_MINUTE,
) -> dict[str, Any]:
    """Create Project `sample` in projects_dir and compute both branches to the end.

    text is what the operator types on the Scenario screen; request, when given, is the
    structured form the screen sends with it (tests use a short one).
    """
    from sheltersim.web.projects import ProjectAPI

    if LLMSettings.from_env().effective_mode is not LLMMode.OFFLINE:
        raise RuntimeError("the sample is built offline only; set SHELTERSIM_LLM_MODE=offline")
    store = ProjectStore(projects_dir)
    if (store.root / PROJECT_ID).exists():
        raise FileExistsError(f"{store.root / PROJECT_ID} exists; build into an empty directory")
    started = time.perf_counter()
    api = ProjectAPI(store, recover=False)
    api.workspace.research.research = _no_research
    try:
        draft, scenario = _preview(api, text, request)
        end = scenario.duration_minutes
        if not 0 < fork_minute < end:
            raise ValueError("the fork minute must fall inside the Scenario")
        if not any(item.id == FACILITY_ID for item in scenario.facilities):
            raise ValueError(f"the Scenario has no {FACILITY_ID} facility")
        api.post(
            [PROJECT_ID, "scenario-approve"],
            {
                "preview_id": draft["preview_id"],
                "approved_hash": draft["approved_hash"],
                "approved": True,
                "run_id": RUN_ID,
            },
        )
        _compute(api, "main", end)

        order = intervention_text(fork_minute)
        branch = [PROJECT_ID, "runs", RUN_ID, "branches", "main"]
        preview = api.post(
            [*branch, "intervention-preview"], {"text": order, "minute": fork_minute}
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
                "child_id": CHILD_ID,
            },
        )
        _compute(api, CHILD_ID, end)
    finally:
        api.close()

    geometry = scenario.model_configs.get("facility_geometry") or {}
    return {
        "schema_version": 1,
        "kind": "precomputed_project_sample",
        "note": NOTE,
        "project_id": PROJECT_ID,
        "run_id": RUN_ID,
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
            "text": order,
            "support_mode": preview.get("support_mode"),
            "fallback_used": preview.get("fallback_used"),
            "approved_program_hash": preview["approved_program_hash"],
        },
        "branches": [_branch_summary(api.files, "main"), _branch_summary(api.files, CHILD_ID)],
        "open_branch_id": OPEN_BRANCH_ID,
        "open_path": open_path(PROJECT_ID, RUN_ID, OPEN_BRANCH_ID),
        "size_bytes": sum(
            path.stat().st_size for path in (store.root / PROJECT_ID).rglob("*") if path.is_file()
        ),
        "build_seconds": round(time.perf_counter() - started, 1),
    }


def _preview(
    api: Any, text: str, request: dict[str, Any] | None
) -> tuple[dict[str, Any], ScenarioSpec]:
    """Create Project `sample` and preview its Scenario from the text, offline."""
    api.post([], {"project_id": PROJECT_ID})
    draft = api.post(
        [PROJECT_ID, "scenario-preview"],
        {"text": text, **({"request": request} if request is not None else {})},
    )
    if not draft.get("can_start"):
        raise RuntimeError(f"the Scenario preview cannot start: {draft.get('missing_fields')}")
    if draft["llm_usage"]["api_request_count"] != 0:
        raise RuntimeError("the Scenario preview called a Provider")
    return draft, ScenarioSpec.model_validate(draft["scenario"])


def sample_difference(
    projects_dir: Path,
    summary: dict[str, Any],
    *,
    text: str = TEXT,
    request: dict[str, Any] | None = None,
    fork_minute: int = FORK_MINUTE,
) -> str | None:
    """None if the sample in projects_dir is what a build now would give, else why not.

    Nothing is computed: the plan is compared, the Scenario is previewed into a
    temporary store, and the installed model signatures are compared with those the
    branches' head Checkpoints recorded.
    """
    from sheltersim.web.projects import ProjectAPI

    if LLMSettings.from_env().effective_mode is not LLMMode.OFFLINE:
        raise RuntimeError("the sample is checked offline only; set SHELTERSIM_LLM_MODE=offline")
    plan = {
        "kind": "precomputed_project_sample",
        "run_id": RUN_ID,
        "branch_ids": ["main", CHILD_ID],
        "text": text,
        "fork_minute": fork_minute,
        "intervention": intervention_text(fork_minute),
        "open_branch_id": OPEN_BRANCH_ID,
    }
    built = {
        "kind": summary.get("kind"),
        "run_id": summary.get("run_id"),
        "branch_ids": [branch.get("branch_id") for branch in summary.get("branches", [])],
        "text": summary.get("scenario", {}).get("text"),
        "fork_minute": summary.get("fork_minute"),
        "intervention": summary.get("intervention", {}).get("text"),
        "open_branch_id": summary.get("open_branch_id"),
    }
    changed = sorted(key for key in plan if plan[key] != built[key])
    if changed:
        return "the plan changed: " + ", ".join(changed)
    with tempfile.TemporaryDirectory(prefix="sheltersim-sample-check-") as scratch:
        api = ProjectAPI(ProjectStore(Path(scratch)), recover=False)
        api.workspace.research.research = _no_research
        try:
            draft, _ = _preview(api, text, request)
        finally:
            api.close()
    if draft["approved_hash"] != summary["scenario"].get("scenario_hash"):
        return "the Scenario the preview makes from the text changed"
    from sheltersim.web.projects import answers
    from sheltersim.web.stored_runs import project_files

    files = project_files(ProjectStore(projects_dir))
    for branch_id in plan["branch_ids"]:
        try:
            journal = files.branch_file(PROJECT_ID, RUN_ID, branch_id, "journal.jsonl")
            scenario = files.run_scenario_bytes(PROJECT_ID, RUN_ID)
            view_only = journal is None or answers().view_only(journal, scenario)
        except Exception as exc:  # A missing or unreadable sample is rebuilt.
            return f"branch {branch_id} cannot be read: {exc}"
        if view_only:
            return f"the model changed for branch {branch_id}"
    return None


def _compute(api: Any, branch_id: str, end: int) -> None:
    """Resume a branch to the end, as the new UI's compute button does, and wait for it."""
    api.post([PROJECT_ID, "runs", RUN_ID, "resume"], {"branch_id": branch_id, "target_minute": end})
    api.wait_idle()
    record = json.loads(api.files.branch_record(PROJECT_ID, RUN_ID, branch_id))
    status, reached = record.get("status"), record["computed_until"]
    if status != "completed" or reached != end:
        raise RuntimeError(f"branch {branch_id} ended {status} at minute {reached}, not {end}")


def _branch_summary(files: Any, branch_id: str) -> dict[str, Any]:
    """The branch's head, the core's digests of the world it ended with and of its metric
    samples, the model versions it was computed with and the state its fork started from."""
    record = json.loads(files.branch_record(PROJECT_ID, RUN_ID, branch_id))
    reader = files.reader(PROJECT_ID, RUN_ID, branch_id)
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
    """Write Project `sample` of projects_dir as one file the browser imports (the browser's
    bundle, `.shelterproj`, #265): the site's example (scripts/build_static_site.py). What
    made it is kept in its manifest's `source`. Returns that `source`."""
    from sheltersim.core.model_version import ENGINE_VERSION
    from sheltersim.web.stored_runs import project_files

    source = {
        "tool": "scripts/build_project_sample.py",
        "text": summary["scenario"]["text"],
        "agent_count": summary["agent_count"],
        "duration_minutes": summary["duration_minutes"],
        "seed": summary["scenario"]["seed"],
        "engine_version": ENGINE_VERSION,
        "regional_research": summary["scenario"]["regional_research"],
        "note": NOTE,
    }
    files = project_files(ProjectStore(projects_dir))
    with files.transaction():
        bundle = files.export_project(PROJECT_ID, json.dumps(source, ensure_ascii=False))
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(bundle)
    return source


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("output", type=Path, help="ProjectStore root to create Project sample in")
    parser.add_argument(
        "--summary",
        type=Path,
        help="where to write the JSON summary (default: sample-build.json next to OUTPUT)",
    )
    parser.add_argument(
        "--bundle",
        type=Path,
        help="also write the sample as one file the browser imports (.shelterproj), the site's",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="compute nothing; exit 0 if the sample in OUTPUT is what a build now would give",
    )
    arguments = parser.parse_args()
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
            reason = sample_difference(output, built)
            print("current" if reason is None else f"rebuild: {reason}")
            return 0 if reason is None else 3
        summary = build_sample(output)
        if arguments.bundle is not None:
            write_bundle(output, arguments.bundle.resolve(), summary)
    summary_path.parent.mkdir(parents=True, exist_ok=True)
    summary_path.write_text(
        json.dumps(summary, ensure_ascii=False, sort_keys=True, indent=2) + "\n", encoding="utf-8"
    )
    json.dump(summary, sys.stdout, ensure_ascii=False, sort_keys=True, indent=2)
    print()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
