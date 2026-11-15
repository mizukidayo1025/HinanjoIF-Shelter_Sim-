"""Offline timing of a Run in the core: `PlainRun` straight through (route plain), or the Project
API's worker into a stored Run of format v5 (route project, the screen's since #427). The core's
digests of the world and the metric samples it ends with are compared with --expect.

Run in a fresh process per case. Output belongs outside the repository.
"""

from __future__ import annotations

import argparse
import cProfile
import json
import os
import pstats
import socket
import sys
import time
from pathlib import Path

os.environ["SHELTERSIM_LLM_MODE"] = "offline"
os.environ.pop("ANTHROPIC_API_KEY", None)

from sheltersim import native
from sheltersim.contracts.policies import model_signatures
from sheltersim.experiments.benchmark import build_benchmark_scenario, environment_manifest
from sheltersim.experiments.scenario import ScenarioSpec
from sheltersim.storage.projects import ProjectStore, digest
from sheltersim.web.projects import ProjectAPI
from sheltersim.web.stored_runs import save_scenario


def peak_rss_mib(children: bool = False) -> float | None:
    """The high-water RSS of this process, or of the largest ended child, in MiB.

    None where the resource module is missing (Windows). A child counts only after it
    has ended and been waited for.
    """
    try:
        import resource
    except ImportError:
        return None
    usage = resource.getrusage(resource.RUSAGE_CHILDREN if children else resource.RUSAGE_SELF)
    # ru_maxrss is in KiB on Linux and in bytes on macOS.
    return round(usage.ru_maxrss / (2**20 if sys.platform == "darwin" else 2**10), 1)


def _stored_sizes(root: Path) -> dict[str, int]:
    """The bytes of the stored Runs' files by name (journal.jsonl, data.pack, the records)."""
    sizes: dict[str, int] = {}
    for path in root.rglob("*"):
        if path.is_file():
            sizes[path.name] = sizes.get(path.name, 0) + path.stat().st_size
    return sizes


def main() -> None:
    parser = argparse.ArgumentParser(__doc__)
    parser.add_argument("--duration", type=int, required=True)
    parser.add_argument("--route", choices=("plain", "project"), required=True)
    parser.add_argument("--profile", action="store_true")
    parser.add_argument("--scenario", choices=("baseline", "generated"), default="baseline")
    parser.add_argument("--scenario-file", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument(
        "--expect",
        type=Path,
        help="an earlier result.json whose core digests of the world and samples must match",
    )
    args = parser.parse_args()
    if args.duration <= 0:
        parser.error("duration must be positive")
    environment = environment_manifest()
    args.output.mkdir(parents=True, exist_ok=False)
    # Fail closed even if a future regression constructs a network provider.
    network_attempts = []

    def denied(*_args, **_kwargs):
        network_attempts.append(1)
        raise AssertionError("network forbidden by offline benchmark")

    socket.socket.connect = denied
    scenario = build_benchmark_scenario(
        ScenarioSpec.load("fixtures/scenarios/baseline_100_7d.json"),
        agent_count=100,
        seed=2026,
        duration_minutes=args.duration,
    )
    scenario = scenario.model_copy(update={"id": f"perf-agents100-minutes{args.duration}-seed2026"})
    if args.scenario == "generated":
        from sheltersim.experiments.initial_scenario import (
            InitialScenarioBuilder,
            InitialScenarioRequest,
        )

        scenario = (
            InitialScenarioBuilder()
            .build(
                InitialScenarioRequest(
                    municipality="Synthetic municipality",
                    disaster="earthquake",
                    evacuees=100,
                    duration_days=args.duration / 1440,
                    seed=2026,
                )
            )
            .scenario
        )
    if args.scenario_file:
        document = json.loads(ScenarioSpec.load(args.scenario_file).canonical_json())
        document["duration_minutes"] = args.duration
        document["scheduled_events"] = [
            event for event in document["scheduled_events"] if event["due_minute"] < args.duration
        ]
        scenario = ScenarioSpec.model_validate(document)
    environment["model_versions"] = model_signatures(scenario.model_configs)
    (args.output / "scenario.json").write_text(scenario.canonical_json() + "\n")
    totals: dict[str, dict[str, float]] = {}
    progress = []
    started = time.perf_counter()
    store = None
    profiler = cProfile.Profile() if args.profile else None

    if args.route == "plain":
        # The core's Run straight through, without saving: the ticks alone.
        run = native.rules("plain").PlainRun.from_scenario(scenario.canonical_json().encode())
        setup_seconds = time.perf_counter() - started
        if profiler:
            profiler.enable()
        started = time.perf_counter()
        cpu_before = time.process_time()
        last = started
        while run.minute() < args.duration:
            run.advance(min(360, args.duration - run.minute()))
            now = time.perf_counter()
            item = {
                "minute": run.minute(),
                "elapsed_seconds": now - started,
                "interval_seconds": now - last,
            }
            progress.append(item)
            print(json.dumps(item), flush=True)
            last = now
        wall_seconds = time.perf_counter() - started
        cpu_seconds = time.process_time() - cpu_before
        if profiler:
            profiler.disable()
        computation_peak = peak_rss_mib()
        totals["PlainRun.advance"] = {
            "calls": len(progress),
            "wall_seconds": wall_seconds,
            "thread_seconds": cpu_seconds,
        }
    else:
        # The screen's route (#427): the Run's branch computed in the core (`PlainRun`) into its
        # stored Run of format v5, a saved point every 360 minutes, on the API's thread.
        store = ProjectStore(args.output / "projects")
        api = ProjectAPI(store, recover=False)
        branch, target = "main", args.duration
        api.post([], {"project_id": "p"})
        save_scenario(api.files, "p", scenario, digest(json.loads(scenario.canonical_json())))
        api.post(
            ["p", "runs"],
            {
                "run_id": "r",
                "scenario_id": scenario.id,
                "scenario_version": 1,
            },
        )
        setup_seconds = time.perf_counter() - started
        if profiler:
            profiler.enable()
        started = time.perf_counter()
        cpu_before = time.process_time()
        api.post(["p", "runs", "r", "resume"], {"branch_id": branch, "target_minute": target})
        api.wait_idle()
        wall_seconds = time.perf_counter() - started
        cpu_seconds = time.process_time() - cpu_before
        if profiler:
            profiler.disable()
        computation_peak = peak_rss_mib()
        record = json.loads(api.files.branch_record("p", "r", branch))
        assert record["computed_until"] == target, "the computation stopped early"
        reader = api.files.reader("p", "r", branch)
        run, _, _ = reader.seek(target)
        api.close()
    core_digests = {
        "world": json.loads(run.digests_json()),
        "metrics": run.metrics_digest(),
    }
    peak_rss = {"computation_mib": computation_peak, "with_hashing_mib": None}
    result = {
        "environment": environment,
        "route": args.route,
        "duration": args.duration,
        "scenario_hash": digest(json.loads(scenario.canonical_json())),
        "seed": scenario.seed,
        "agent_count": 100,
        "model_configs": scenario.model_configs,
        "trace_level": "off",
        "profiled": args.profile,
        "wall_seconds": wall_seconds,
        "thread_seconds": cpu_seconds,
        "setup_seconds": setup_seconds,
        "scenario_kind": args.scenario,
        # The core's digests of the world and the samples it ended with (both routes).
        "core_digests": core_digests,
        "network_attempts": len(network_attempts),
        "peak_rss": peak_rss,
        "timings": totals,
        "progress": progress,
    }
    if store:
        result["storage"] = store.usage()
        result["storage"]["by_kind"] = _stored_sizes(store.root)
    if profiler:
        profiler.dump_stats(str(args.output / "profile.pstats"))
        with (args.output / "profile.txt").open("w") as stream:
            pstats.Stats(profiler, stream=stream).sort_stats("cumulative").print_stats(100)
    # The process's high-water mark including the read-back and hashes above.
    peak_rss["with_hashing_mib"] = peak_rss_mib()
    mismatched = []
    if args.expect:
        expected = json.loads(args.expect.read_text(encoding="utf-8"))
        if expected.get("core_digests") is None:
            mismatched = ["nothing to compare"]
        elif expected["core_digests"] != result["core_digests"]:
            mismatched = ["core_digests"]
        result["expected_hashes"] = {"source": str(args.expect), "mismatched": mismatched}
    (args.output / "result.json").write_text(
        json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    print(
        json.dumps(
            {
                key: result[key]
                for key in (
                    "route",
                    "duration",
                    "wall_seconds",
                    "thread_seconds",
                    "peak_rss",
                )
            }
        )
    )
    if mismatched:
        raise SystemExit(f"hashes differ from {args.expect}: {', '.join(mismatched)}")


if __name__ == "__main__":
    main()
