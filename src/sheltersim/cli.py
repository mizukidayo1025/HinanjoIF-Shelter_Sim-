"""The CLI: Scenario preparation, Runs of a Project through the screen's routes (#450), model
analysis, LLM accounting and the local server."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import uuid
from collections.abc import Sequence
from pathlib import Path
from typing import Any
from urllib.parse import urlencode

from sheltersim import __version__
from sheltersim.contracts.policies import legacy_psychology_notice, selected_models
from sheltersim.experiments.profiles import GeneratedProfileSet
from sheltersim.experiments.scenario import ScenarioSpec
from sheltersim.llm.config import LLMSettings
from sheltersim.llm.generation import generate_initial_needs
from sheltersim.llm.outcomes import fallback_warning, summarize_call_outcomes
from sheltersim.llm.prepare import generate_scenario_draft
from sheltersim.llm.provider import LLMRouter
from sheltersim.storage.projects import DEFAULT_MAX_BYTES


def _new_cli_account_id(scope: str) -> str:
    """Open a fresh ledger account so one exhausted allowance never silences later runs."""
    return f"cli-{scope}-{uuid.uuid4().hex}"


def _unfreeze_usage_account(account_id: str, *, confirm: str, reason: str) -> dict[str, Any]:
    from sheltersim.llm.accounting import RUNTIME_ACCOUNTS_PROJECT_ID, runtime_usage_store
    from sheltersim.storage.usage import (
        TASK_CAMPAIGN_ACCOUNT_ID,
        UsageLedger,
        task_campaign_store,
    )

    if account_id == TASK_CAMPAIGN_ACCOUNT_ID:
        store = task_campaign_store()
    elif account_id.startswith("cli-") or account_id == "legacy_demo":
        store = runtime_usage_store()
    else:
        raise ValueError("only task-global-v1, cli-* and legacy_demo accounts can be unfrozen here")
    ledger = UsageLedger(store, RUNTIME_ACCOUNTS_PROJECT_ID, account_id)
    if not ledger.path.exists():
        raise ValueError("usage account does not exist")
    return ledger.unfreeze(confirm_account_id=confirm, reason=reason)


def _increase_usage_budget(
    account_id: str, *, confirm: str, new_budget_usd: float, reason: str
) -> dict[str, Any]:
    from sheltersim.storage.usage import TASK_CAMPAIGN_ACCOUNT_ID, UsageLedger, task_campaign_store

    if account_id != TASK_CAMPAIGN_ACCOUNT_ID:
        raise ValueError("only task-global-v1 supports an operator-approved budget increase")
    store = task_campaign_store()
    ledger = UsageLedger(store, "runtime-accounts", account_id)
    if not ledger.path.exists():
        raise ValueError("usage account does not exist")
    return ledger.increase_budget(
        confirm_account_id=confirm, new_budget_usd=new_budget_usd, reason=reason
    )


def _cli_accounted_router(settings: LLMSettings, *, operation: str, account_id: str) -> LLMRouter:
    """Use one opaque ledger account throughout a CLI preparation."""
    from sheltersim.llm.accounting import runtime_accounted_router, runtime_usage_store

    return runtime_accounted_router(
        runtime_usage_store(os.environ.get("SHELTERSIM_PROJECTS_DIR", "runs/.projects")),
        account_id=account_id,
        operation=operation,
        settings=settings,
    )


def _positive_int(value: str) -> int:
    try:
        number = int(value)
    except ValueError as exc:
        raise argparse.ArgumentTypeError("must be a positive integer") from exc
    if number < 1:
        raise argparse.ArgumentTypeError("must be a positive integer")
    return number


def _project_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--root", type=Path, default=Path("runs/.projects"), help="the Projects' directory"
    )
    parser.add_argument("--project-id", required=True)


def build_parser() -> argparse.ArgumentParser:
    """Build the command parser."""
    parser = argparse.ArgumentParser(
        prog="sheltersim",
        description="Shelter operations simulation tools.",
    )
    parser.add_argument("--version", action="version", version=f"%(prog)s {__version__}")
    subparsers = parser.add_subparsers(dest="command")
    from sheltersim.storage.commands import configure_parser

    configure_parser(subparsers)

    usage = subparsers.add_parser(
        "usage", help="show or explicitly maintain bounded LLM accounting receipts"
    )
    usage.add_argument(
        "--unfreeze",
        metavar="ACCOUNT_ID",
        help=(
            "re-open a frozen ledger (task-global-v1, cli-*, or legacy_demo) after reviewing "
            "why actual usage exceeded its reservation; recorded spend is kept"
        ),
    )
    usage.add_argument(
        "--confirm", metavar="ACCOUNT_ID", help="repeat the account ID for confirmation"
    )
    usage.add_argument(
        "--reason",
        help="why the account operation is approved (recorded, at most 200 chars)",
    )
    usage.add_argument(
        "--increase-budget",
        metavar="ACCOUNT_ID",
        help="explicitly raise task-global-v1's shared total budget (audit logged)",
    )
    usage.add_argument(
        "--new-budget-usd",
        type=float,
        help="new total dollar limit, greater than the existing limit",
    )

    catalog = subparsers.add_parser(
        "model-catalog", help="show installed model equations, sources and limits"
    )
    catalog.add_argument("--scenario", required=True, type=Path)
    toilet_diagnostic = subparsers.add_parser(
        "toilet-diagnostic", help="compare Scenario toilet capacity with planning benchmarks"
    )
    toilet_diagnostic.add_argument("--scenario", required=True, type=Path)
    toilet_diagnostic.add_argument("--phase", choices=("initial", "long-term"), default="initial")
    capacity_diagnostic = subparsers.add_parser(
        "service-capacity-diagnostic",
        help="compare toilet, water and meal units with the work their daily demand needs",
    )
    capacity_diagnostic.add_argument("--scenario", required=True, type=Path)
    capacity_diagnostic.add_argument("--phase", choices=("initial", "long-term"), default="initial")
    evaluation = subparsers.add_parser(
        "evaluate-models",
        help="compute an explicitly requested offline paired model/parameter matrix as Runs",
    )
    evaluation.add_argument("--scenario", required=True, type=Path)
    evaluation.add_argument("--plan", required=True, type=Path)
    _project_arguments(evaluation)
    evaluation.add_argument(
        "--output", required=True, type=Path, help="new file for the evaluation JSON"
    )
    evaluation.add_argument("--sources", type=Path)

    preview = subparsers.add_parser(
        "profiles-preview",
        help="display every profile and the generation manifest before a run",
    )
    preview.add_argument("path", type=Path, help="generated agents_initial.json artifact")

    prepare = subparsers.add_parser(
        "prepare",
        help="validate a scenario and preview every profile and planned event",
    )
    prepare.add_argument("--scenario", required=True, type=Path, help="ScenarioSpec JSON file")
    prepare.add_argument("--output", type=Path, help="write the preview JSON to this path")
    prepare.add_argument(
        "--freeze",
        action="store_true",
        help="mark this exact preview as explicitly frozen after review; requires --output",
    )
    generation = prepare.add_mutually_exclusive_group()
    generation.add_argument(
        "--generate-initial-needs",
        action="store_true",
        help="draft synthetic initial needs with Opus; output remains reviewable before freezing",
    )
    generation.add_argument(
        "--generate-scenario",
        action="store_true",
        help="draft all synthetic profiles and facility events with Opus for review",
    )

    run = subparsers.add_parser(
        "run", help="compute one frozen ScenarioSpec as a Run of a Project (the screen's route)"
    )
    run.add_argument("--scenario", required=True, type=Path, help="frozen preview of prepare")
    _project_arguments(run)
    run.add_argument("--run-id", required=True)
    run.add_argument(
        "--offline", action="store_true", help="an offline experiment: no Provider is called"
    )

    inspect = subparsers.add_parser(
        "inspect", help="read a Run of a Project as the screen reads it"
    )
    _project_arguments(inspect)
    inspect.add_argument("--run-id", required=True)
    inspect.add_argument("--branch-id", default="main", help="branch of --kpis/--conversations")
    query = inspect.add_mutually_exclusive_group(required=True)
    query.add_argument("--record", action="store_true", help="the Run's record and branches")
    query.add_argument(
        "--evidence", action="store_true", help="the Run's models, sources and assumptions"
    )
    query.add_argument("--kpis", action="store_true", help="a page of the branch's KPI rows")
    query.add_argument(
        "--conversations", action="store_true", help="the questions asked about the branch"
    )
    inspect.add_argument("--start", type=int, help="first minute of --kpis (default: fork)")
    inspect.add_argument("--end", type=int, help="last minute of --kpis (default: computed)")
    inspect.add_argument("--step", type=_positive_int, default=1, help="minutes between rows")
    inspect.add_argument("--limit", type=_positive_int, default=100, help="rows per page")
    inspect.add_argument("--cursor", help="next_cursor of the previous page")

    compare = subparsers.add_parser(
        "compare", help="compare branches of Runs of a Project by their KPI records"
    )
    _project_arguments(compare)
    compare.add_argument(
        "--baseline", required=True, nargs="+", help="RUN or RUN/BRANCH, one per seed"
    )
    compare.add_argument(
        "--candidate", required=True, nargs="+", help="RUN or RUN/BRANCH, paired by seed"
    )
    compare.add_argument("--metric", action="append", help="a KPI to compare (default: all)")
    compare.add_argument("--output", type=Path, help="write the comparison JSON here")

    review = subparsers.add_parser(
        "review", help="the review of a computed branch, with the comparisons that include it"
    )
    _project_arguments(review)
    review.add_argument("--run-id", required=True)
    review.add_argument("--branch-id", default="main")
    review.add_argument(
        "--comparison", type=Path, help="a comparison JSON of compare that includes the branch"
    )

    web = subparsers.add_parser("web", help="serve the local browser screen (/)")
    web.add_argument(
        "--runs-dir",
        type=Path,
        default=Path("runs"),
        help="directory whose .projects subdirectory holds the saved Projects",
    )
    web.add_argument("--host", default="127.0.0.1", help="bind address (default: loopback)")
    web.add_argument("--port", type=int, default=8765, help="HTTP port (default: 8765)")
    web.add_argument(
        "--max-parallel-runs",
        type=_positive_int,
        default=2,
        help="Runs computed at once, each in its own process (default: 2)",
    )
    web.add_argument(
        "--project-storage-gb",
        type=_positive_int,
        default=DEFAULT_MAX_BYTES // 1_000_000_000,
        help="saved data the projects directory may hold, in GB of 10^9 bytes (default: 10)",
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    """Run one offline CLI action and return its process status."""
    # The CLI prints JSON with Japanese text. Piped or redirected on Windows, stdout
    # would use the locale's code page, which cannot hold it.
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8")
    parser = build_parser()
    arguments = parser.parse_args(argv)
    if arguments.command == "usage":
        from sheltersim.llm.accounting import runtime_usage_store, runtime_usage_summary

        if arguments.unfreeze is not None and arguments.increase_budget is not None:
            parser.error("usage --unfreeze and --increase-budget are mutually exclusive")
        if arguments.increase_budget is not None:
            if (
                arguments.new_budget_usd is None
                or arguments.confirm is None
                or arguments.reason is None
            ):
                parser.error(
                    "usage --increase-budget requires --new-budget-usd, --confirm, and --reason"
                )
            try:
                event = _increase_usage_budget(
                    arguments.increase_budget,
                    confirm=arguments.confirm,
                    new_budget_usd=arguments.new_budget_usd,
                    reason=arguments.reason,
                )
            except (OSError, ValueError, RuntimeError) as exc:
                parser.error(f"could not increase usage budget: {exc}")
            print(
                _json_text(
                    {"account_id": arguments.increase_budget, "budget_increase_event": event}
                )
            )
            return 0
        if arguments.unfreeze is None:
            if (
                arguments.confirm is not None
                or arguments.reason is not None
                or arguments.new_budget_usd is not None
            ):
                parser.error("usage --confirm/--reason require an explicit account operation")
            print(_json_text(runtime_usage_summary(runtime_usage_store())))
            return 0
        if arguments.new_budget_usd is not None:
            parser.error("usage --new-budget-usd requires --increase-budget")
        try:
            event = _unfreeze_usage_account(
                arguments.unfreeze, confirm=arguments.confirm or "", reason=arguments.reason or ""
            )
        except (OSError, ValueError, RuntimeError) as exc:
            parser.error(f"could not unfreeze usage account: {exc}")
        print(_json_text({"account_id": arguments.unfreeze, "unfreeze_event": event}))
        return 0
    if arguments.command == "project":
        from sheltersim.storage.commands import execute

        try:
            print(_json_text(execute(arguments)))
        except (OSError, ValueError, RuntimeError) as exc:
            parser.error(f"project operation failed: {exc}")
        return 0
    if arguments.command == "web":
        from sheltersim.web.server import serve_inspector

        serve_inspector(
            arguments.runs_dir,
            host=arguments.host,
            port=arguments.port,
            max_parallel_runs=arguments.max_parallel_runs,
            project_storage_bytes=arguments.project_storage_gb * 1_000_000_000,
        )
        return 0
    if arguments.command == "profiles-preview":
        try:
            profiles = GeneratedProfileSet.load(arguments.path)
        except (OSError, ValueError) as exc:
            parser.error(f"could not load profile artifact: {exc}")
        print(profiles.preview())
        return 0
    if arguments.command == "prepare":
        if arguments.freeze and arguments.output is None:
            parser.error("prepare --freeze requires --output so the frozen preview can be reviewed")
        if arguments.generate_scenario and arguments.output is None:
            parser.error("prepare --generate-scenario requires --output for review before freezing")
        if arguments.freeze and (arguments.generate_scenario or arguments.generate_initial_needs):
            parser.error("generated inputs must be previewed before a later --freeze run")
        try:
            source_document = json.loads(arguments.scenario.read_text(encoding="utf-8"))
            scenario = _load_scenario_input(arguments.scenario)
            from sheltersim.experiments.model_suite import apply_default_model_suite

            scenario = apply_default_model_suite(scenario)
            router = None
            generation_manifest = None
            generated_scenario = None
            generation_calls: list[dict[str, Any]] = []
            if isinstance(source_document, dict):
                existing_manifest = source_document.get("generation_manifest")
                if isinstance(existing_manifest, dict):
                    generation_manifest = dict(existing_manifest)
                existing_generated = source_document.get("generated_scenario")
                if isinstance(existing_generated, dict):
                    generated_scenario = existing_generated
                existing_calls = source_document.get("llm_calls")
                if isinstance(existing_calls, list):
                    generation_calls = existing_calls
            if arguments.generate_scenario:
                account_id = _new_cli_account_id("prepare")
                router = _cli_accounted_router(
                    LLMSettings.from_env(), operation="prepare", account_id=account_id
                )
                scenario, generation_manifest = generate_scenario_draft(scenario, router)
                generated_scenario = json.loads(scenario.canonical_json())
            if arguments.generate_initial_needs:
                account_id = _new_cli_account_id("prepare")
                router = _cli_accounted_router(
                    LLMSettings.from_env(), operation="prepare", account_id=account_id
                )
                scenario = generate_initial_needs(scenario, router)
                generation_manifest = scenario.provenance.get("llm_initial_needs")
                generated_scenario = json.loads(scenario.canonical_json())
            preview_document = _scenario_preview(
                scenario,
                frozen=arguments.freeze,
                generation_manifest=generation_manifest,
                generated_scenario=generated_scenario,
            )
            if router is not None:
                generation_calls.extend(record.to_dict() for record in router.records)
            if router is not None:
                preview_document["llm_usage_account_id"] = account_id
            if generation_calls:
                preview_document["llm_calls"] = generation_calls
            output = _json_text(preview_document)
            if arguments.output is not None:
                _write_atomic(arguments.output, output + "\n")
            print(output)
            _warn_legacy_psychology(scenario)
            if router is not None:
                warning = fallback_warning(summarize_call_outcomes(router.records))
                if warning is not None:
                    print(f"warning: {warning}", file=sys.stderr)
        except (OSError, ValueError, RuntimeError) as exc:
            parser.error(f"could not prepare scenario: {exc}")
        return 0
    if arguments.command in {
        "model-catalog",
        "evaluate-models",
        "toilet-diagnostic",
        "service-capacity-diagnostic",
    }:
        try:
            scenario = ScenarioSpec.load(arguments.scenario)
            if arguments.command == "model-catalog":
                from sheltersim.experiments.validation import evidence_context

                result = evidence_context(scenario)
            elif arguments.command == "toilet-diagnostic":
                from sheltersim.experiments.sanitation_diagnostics import toilet_planning_diagnostic

                result = toilet_planning_diagnostic(scenario, phase=arguments.phase)
            elif arguments.command == "service-capacity-diagnostic":
                from sheltersim.experiments.service_capacity import service_capacity_diagnostic

                result = service_capacity_diagnostic(scenario, phase=arguments.phase)
            else:
                from sheltersim.experiments.project_runs import project_api
                from sheltersim.experiments.validation import EvaluationPlan, evaluate_models

                if arguments.output.exists():
                    raise ValueError("evaluation output exists; prior results are immutable")
                plan = EvaluationPlan.model_validate(_read_json_object(arguments.plan))
                sources = _read_json_object(arguments.sources) if arguments.sources else None
                with project_api(arguments.root) as api:
                    result = evaluate_models(
                        scenario, plan, api, arguments.project_id, source_manifest=sources
                    )
                _write_atomic(arguments.output, _json_text(result) + "\n")
            print(_json_text(result))
        except (OSError, ValueError, RuntimeError) as exc:
            parser.error(f"model analysis failed: {exc}")
        return 0
    if arguments.command in {"run", "inspect", "compare", "review"}:
        try:
            print(_json_text(_project_command(arguments)))
        except (OSError, ValueError, RuntimeError) as exc:
            parser.error(f"could not {arguments.command}: {exc}")
        return 0
    parser.print_help()
    return 0


def _project_command(arguments: argparse.Namespace) -> Any:
    """`run`, `inspect`, `compare` and `review`: through the screen's routes on a Project (#450,
    experiments/project_runs.py)."""
    from sheltersim.experiments.project_runs import compute_run, project_api

    project_id = arguments.project_id
    with project_api(arguments.root) as api:
        if arguments.command == "run":
            scenario = _load_frozen_scenario(arguments.scenario)
            branch = compute_run(
                api, project_id, arguments.run_id, scenario, offline=arguments.offline
            )
            run = api.get([project_id, "runs", arguments.run_id])
            usage = api.get([project_id, "runs", arguments.run_id, "usage"])
            llm_status = usage.get("outcomes", {"status": "not_requested"})
            _warn_legacy_psychology(scenario)
            warning = fallback_warning(llm_status) if "outcomes" in usage else None
            if warning is not None:
                print(f"warning: {warning}", file=sys.stderr)
            return {
                "project_id": project_id,
                "run_id": arguments.run_id,
                "scenario_id": run["scenario_id"],
                "scenario_version": run["scenario_version"],
                "scenario_hash": run["scenario_hash"],
                "branch": branch,
                "llm_status": llm_status,
                "psychology_model": selected_models(scenario.model_configs)["psychology"],
            }
        if arguments.command == "compare":
            from sheltersim.experiments.comparison import compare_runs

            report = compare_runs(
                api,
                project_id,
                arguments.baseline,
                arguments.candidate,
                metrics=arguments.metric,
            )
            if arguments.output is None:
                return report
            _write_atomic(arguments.output, _json_text(report) + "\n")
            return {"report": str(arguments.output)}
        run = [project_id, "runs", arguments.run_id]
        branch = [*run, "branches", arguments.branch_id]
        if arguments.command == "review":
            comparisons = []
            if arguments.comparison is not None:
                comparisons = _comparisons_of(
                    _read_json_object(arguments.comparison),
                    project_id,
                    arguments.run_id,
                    arguments.branch_id,
                )
            return {
                "project_id": project_id,
                "run_id": arguments.run_id,
                "branch_id": arguments.branch_id,
                "report": api.get([*branch, "report"]),
                "comparisons": comparisons,
            }
        if arguments.command == "inspect" and not arguments.kpis:
            if any(
                value is not None for value in (arguments.start, arguments.end, arguments.cursor)
            ):
                raise ValueError("--start, --end and --cursor read --kpis")
        if arguments.record:
            return api.get(run)
        if arguments.evidence:
            return api.get([*run, "evidence"])
        if arguments.conversations:
            return api.get([*branch, "conversations"])
        from sheltersim.experiments.project_runs import branch_record

        record = branch_record(api, project_id, arguments.run_id, arguments.branch_id)
        query = {
            "kind": "kpis",
            "start": record["fork_minute"] if arguments.start is None else arguments.start,
            "end": record["computed_until"] if arguments.end is None else arguments.end,
            "step": arguments.step,
            "limit": arguments.limit,
        }
        if arguments.cursor is not None:
            query["cursor"] = arguments.cursor
        return api.get([*branch, "history"], urlencode(query))


def _comparisons_of(
    report: dict[str, Any], project_id: str, run_id: str, branch_id: str
) -> list[dict[str, Any]]:
    """The pairs of a comparison of `compare` that include the branch."""
    if report.get("schema_version") != 1 or not isinstance(report.get("pairs"), list):
        raise ValueError("the comparison is not one of compare")
    if report.get("project_id") != project_id:
        raise ValueError("the comparison is of another Project")
    pairs = [
        pair
        for pair in report["pairs"]
        if any(
            (pair[side]["run_id"], pair[side]["branch_id"]) == (run_id, branch_id)
            for side in ("baseline", "candidate")
        )
    ]
    if not pairs:
        raise ValueError("the comparison does not include the branch")
    return pairs


def _scenario_preview(
    scenario: ScenarioSpec,
    *,
    frozen: bool,
    generation_manifest: dict[str, Any] | None = None,
    generated_scenario: dict[str, Any] | None = None,
) -> dict[str, Any]:
    if generation_manifest is not None:
        generation_manifest = dict(generation_manifest)
        if generated_scenario is None:
            generated_scenario = json.loads(scenario.canonical_json())
        generation_manifest["manual_edits"] = _manual_edit_paths(
            generated_scenario, json.loads(scenario.canonical_json())
        )
        if frozen:
            generation_manifest["approved"] = True
            provenance = dict(scenario.provenance)
            provenance_key = (
                "llm_initial_needs"
                if "llm_initial_needs" in provenance
                else "llm_scenario_generation"
            )
            stable_keys = {
                "schema_version",
                "generator",
                "model_id",
                "prompt_version",
                "output_schema_version",
                "input_hash",
                "output_hash",
                "status",
                "scenario_source_hash",
                "generated_scenario_hash",
                "assumption",
                "manual_edits",
            }
            provenance[provenance_key] = {
                key: value for key, value in generation_manifest.items() if key in stable_keys
            }
            scenario = scenario.model_copy(update={"provenance": provenance})
    canonical_scenario = json.loads(scenario.canonical_json())
    document = {
        "preview_schema_version": 1,
        "frozen": frozen,
        "approval": {"approved": frozen, "method": "prepare --freeze" if frozen else None},
        "scenario_id": scenario.id,
        "scenario_hash": hashlib.sha256(scenario.canonical_json().encode("utf-8")).hexdigest(),
        "profile_count": len(scenario.profiles),
        "planned_event_count": len(scenario.scheduled_events),
        "scenario": canonical_scenario,
    }
    if generation_manifest is not None:
        document["generation_manifest"] = generation_manifest
    if generated_scenario is not None:
        document["generated_scenario"] = generated_scenario
    return document


def _manual_edit_paths(generated: dict[str, Any], frozen: dict[str, Any]) -> list[str]:
    """Describe changed generated fields without copying user-entered values into audit logs."""
    changed: list[str] = []
    ignored = {"provenance"}
    for key in sorted((set(generated) | set(frozen)) - ignored):
        if key == "profiles":
            before = {item.get("id"): item for item in generated.get("profiles", [])}
            after = {item.get("id"): item for item in frozen.get("profiles", [])}
            for index, profile_id in enumerate(sorted(before.keys() | after.keys())):
                left, right = before.get(profile_id, {}), after.get(profile_id, {})
                for field in sorted((set(left) | set(right)) - ignored):
                    if left.get(field) != right.get(field):
                        changed.append(f"profiles[{index}].{field}")
        elif generated.get(key) != frozen.get(key):
            changed.append(key)
    return changed


def _load_frozen_scenario(path: Path) -> ScenarioSpec:
    document = json.loads(path.read_text(encoding="utf-8"))
    if isinstance(document, dict) and document.get("preview_schema_version") == 1:
        if document.get("frozen") is not True:
            raise ValueError("scenario preview is not frozen; review it and run prepare --freeze")
        approval = document.get("approval")
        if not isinstance(approval, dict) or approval.get("approved") is not True:
            raise ValueError("scenario preview has no explicit prepare --freeze approval")
        frozen_scenario = document.get("scenario")
        if not isinstance(frozen_scenario, dict):
            raise ValueError("frozen preview does not contain a scenario object")
        expected_hash = document.get("scenario_hash")
        actual_hash = hashlib.sha256(
            ScenarioSpec.model_validate(frozen_scenario).canonical_json().encode("utf-8")
        ).hexdigest()
        if expected_hash != actual_hash:
            raise ValueError("frozen scenario hash does not match the approved scenario contents")
        return ScenarioSpec.model_validate(frozen_scenario)
    raise ValueError("run requires a reviewed frozen preview; use prepare --freeze first")


def _load_scenario_input(path: Path) -> ScenarioSpec:
    """Load a ScenarioSpec directly or its previously generated review preview."""
    document = json.loads(path.read_text(encoding="utf-8"))
    if isinstance(document, dict) and document.get("preview_schema_version") == 1:
        scenario_document = document.get("scenario")
        if not isinstance(scenario_document, dict):
            raise ValueError("scenario preview does not contain a scenario object")
        return ScenarioSpec.model_validate(scenario_document)
    return ScenarioSpec.from_json(json.dumps(document, ensure_ascii=False))


def _warn_legacy_psychology(scenario: ScenarioSpec) -> None:
    notice = legacy_psychology_notice(selected_models(scenario.model_configs)["psychology"])
    if notice is not None:
        print(f"warning: {notice}", file=sys.stderr)


def _json_text(value: Any) -> str:
    return json.dumps(
        value,
        ensure_ascii=False,
        sort_keys=True,
        indent=2,
        allow_nan=False,
    )


def _write_atomic(path: Path, contents: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_text(contents, encoding="utf-8", newline="")
    temporary.replace(path)


def _read_json_object(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"{path.name} must contain a JSON object")
    return value
