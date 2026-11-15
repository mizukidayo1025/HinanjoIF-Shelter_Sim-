"""The all-paths gate: small offline scenarios that together reach most of the tick's code.

Each scenario runs in the core (`PlainRun`, benchmarks/gate_scenarios.py) in its own process and
writes the world, events and KPI hashes of every world it ended with, plus timing. Hashes are
compared with --expect; --determinism also runs every scenario under other process settings and
compares them with the first run.

    python benchmarks/gate_all_paths.py --output DIR --expect fixtures/gate-all-paths.json
    python benchmarks/gate_all_paths.py --output DIR --determinism
    python benchmarks/gate_all_paths.py --output DIR --write-expected fixtures/gate-all-paths.json

Run from the repository root with src on PYTHONPATH. Output belongs outside the repository.
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from pathlib import Path
from typing import Any

HASHES = ("world_hash", "events_hash", "metrics_hash")


@dataclass(frozen=True)
class Variant:
    """One way to run a scenario's process."""

    name: str
    env: dict[str, str]


# Each person's compute of a tick goes to helper threads from this many people on
# (native/core/src/parallel.rs); a gate Scenario has fewer, so a variant sets 1 to send everyone.
PEOPLE_PER_HELPER_VARIABLE = "SHELTERSIM_GATE_PEOPLE_PER_HELPER"
# PYTHONHASHSEED varies set and str-hash order; RAYON_NUM_THREADS varies the helpers of the
# tick: one thread has none, four have three.
BASE = Variant("base", {"PYTHONHASHSEED": "1"})
VARIANTS = (
    Variant("second_process", {"PYTHONHASHSEED": "1"}),
    Variant("hash_seed", {"PYTHONHASHSEED": "271828"}),
    Variant("one_worker", {"PYTHONHASHSEED": "2", "RAYON_NUM_THREADS": "1"}),
    Variant(
        "four_workers",
        {"PYTHONHASHSEED": "3", "RAYON_NUM_THREADS": "4", PEOPLE_PER_HELPER_VARIABLE: "1"},
    ),
)


def run_one(name: str, output: Path, chosen: Any = None) -> None:
    """In a scenario's own process: run it and write result.json. Fails closed on network.

    ``chosen`` is a GateScenario to run instead of the one by that name.
    """
    import socket

    os.environ["SHELTERSIM_LLM_MODE"] = "offline"
    os.environ.pop("ANTHROPIC_API_KEY", None)
    state = output / "state"
    os.environ["SHELTERSIM_PROJECTS_DIR"] = str(state / "projects")
    os.environ["XDG_STATE_HOME"] = str(state / "xdg")
    os.environ["SHELTERSIM_LLM_CACHE_PATH"] = str(state / "llm-tape.json")

    def denied(*_args: Any, **_kwargs: Any) -> None:
        raise AssertionError("network forbidden by the offline gate")

    socket.socket.connect = denied  # type: ignore[method-assign]
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    from gate_scenarios import all_scenarios

    scenario = chosen or all_scenarios()[name]
    import sheltersim_native

    people_per_helper = os.environ.get(PEOPLE_PER_HELPER_VARIABLE)
    if people_per_helper:
        sheltersim_native.set_people_per_helper(int(people_per_helper))
    started = time.perf_counter()
    outcome = scenario.run(output / "work")
    mismatched = [
        {"first": first, "second": second, "means": means}
        for first, second, means in outcome.must_match
        if any(outcome.runs[first][key] != outcome.runs[second][key] for key in HASHES)
    ]
    (output / "result.json").write_text(
        json.dumps(
            {
                "scenario": name,
                "what": scenario.what,
                "wall_seconds": time.perf_counter() - started,
                "runs": outcome.runs,
                "refused": outcome.refused,
                "must_match": [list(row) for row in outcome.must_match],
                "must_match_failed": mismatched,
            },
            ensure_ascii=False,
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )


def _spawn(name: str, directory: Path, variant: Variant) -> dict[str, Any]:
    """Run a scenario in a new process; a crash is a result, with its log kept."""
    directory.mkdir(parents=True, exist_ok=False)
    env = {**os.environ, **variant.env, "SHELTERSIM_LLM_MODE": "offline"}
    command = [sys.executable, __file__, "--run-one", name, "--output", str(directory)]
    started = time.perf_counter()
    with (directory / "log.txt").open("w", encoding="utf-8") as log:
        code = subprocess.run(
            command, env=env, stdout=log, stderr=subprocess.STDOUT, check=False
        ).returncode
    result_file = directory / "result.json"
    if code != 0 or not result_file.exists():
        tail = (directory / "log.txt").read_text(encoding="utf-8")[-2000:]
        return {"scenario": name, "crashed": True, "exit_code": code, "log_tail": tail}
    result = json.loads(result_file.read_text(encoding="utf-8"))
    result["process_seconds"] = time.perf_counter() - started
    return result


def _scenario_names(only: list[str] | None) -> list[str]:
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    from gate_scenarios import all_scenarios

    names = list(all_scenarios())
    if not only:
        return names
    chosen = [name for name in names if any(name.startswith(prefix) for prefix in only)]
    if not chosen:
        raise SystemExit(f"no scenario matches {only}")
    return chosen


def _run_all(
    names: list[str], output: Path, variant: Variant, jobs: int
) -> dict[str, dict[str, Any]]:
    def one(name: str) -> dict[str, Any]:
        result = _spawn(name, output / variant.name / name, variant)
        state = "CRASH" if result.get("crashed") else f"{result['wall_seconds']:.1f}s"
        print(f"[{variant.name}] {name}: {state}", flush=True)
        return result

    with ThreadPoolExecutor(max_workers=jobs) as pool:
        return dict(zip(names, pool.map(one, names), strict=True))


def _problems_within(results: dict[str, dict[str, Any]]) -> list[str]:
    problems = []
    for name, result in results.items():
        if result.get("crashed"):
            problems.append(f"{name}: crashed (exit {result['exit_code']}): {result['log_tail']}")
            continue
        for row in result["must_match_failed"]:
            problems.append(
                f"{name}: {row['first']} and {row['second']} differ, but {row['means']}"
            )
    return problems


def _differences(label: str, first: dict[str, Any], second: dict[str, Any]) -> list[str]:
    """What differs between two results of one scenario, by run and hash."""
    if first.get("crashed") or second.get("crashed"):
        return [f"{label}: crashed"]
    found = []
    for run in sorted(set(first["runs"]) | set(second["runs"])):
        a, b = first["runs"].get(run), second["runs"].get(run)
        if a is None or b is None:
            found.append(f"{label}/{run}: present on one side only")
            continue
        different = [key for key in HASHES if a[key] != b[key]]
        if different:
            found.append(f"{label}/{run}: {', '.join(different)}")
    if first.get("refused") != second.get("refused"):
        found.append(f"{label}: the refused interventions differ")
    return found


def _expected_document(results: dict[str, dict[str, Any]]) -> dict[str, Any]:
    return {
        "schema_version": 1,
        "scenarios": {
            name: {"runs": result["runs"], "refused": result["refused"]}
            for name, result in sorted(results.items())
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--expect", type=Path, help="fixture of expected hashes to compare with")
    parser.add_argument("--write-expected", type=Path, help="write the results as that fixture")
    parser.add_argument("--determinism", action="store_true", help="also run the other variants")
    parser.add_argument("--only", action="append", help="scenarios whose name starts with this")
    parser.add_argument("--jobs", type=int, default=min(os.cpu_count() or 1, 12))
    parser.add_argument("--run-one", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.run_one:
        run_one(args.run_one, args.output)
        return 0

    args.output.mkdir(parents=True, exist_ok=False)
    started = time.perf_counter()
    names = _scenario_names(args.only)
    results = _run_all(names, args.output, BASE, args.jobs)
    problems = _problems_within(results)
    summary: dict[str, Any] = {
        "platform": platform.platform(),
        "python": platform.python_version(),
        "scenarios": {
            name: {
                "wall_seconds": result.get("wall_seconds"),
                "what": result.get("what"),
                "runs": result.get("runs"),
                "refused": result.get("refused"),
                "crashed": bool(result.get("crashed")),
            }
            for name, result in results.items()
        },
    }

    if args.expect:
        expected = json.loads(args.expect.read_text(encoding="utf-8"))["scenarios"]
        for name in names:
            if name not in expected:
                problems.append(f"{name}: not in {args.expect}")
            else:
                problems.extend(_differences(f"expected {name}", expected[name], results[name]))

    if args.determinism:
        table: dict[str, dict[str, Any]] = {name: {} for name in names}
        for variant in VARIANTS:
            other = _run_all(names, args.output, variant, args.jobs)
            problems.extend(_problems_within(other))
            for name in names:
                differences = _differences(f"{variant.name} {name}", results[name], other[name])
                problems.extend(differences)
                table[name][variant.name] = "same" if not differences else "DIFFERENT"
        summary["determinism"] = table

    if args.write_expected:
        if problems:
            print("not writing expected hashes: there are problems", file=sys.stderr)
        else:
            args.write_expected.write_text(
                json.dumps(_expected_document(results), ensure_ascii=False, indent=1) + "\n",
                encoding="utf-8",
            )

    summary["problems"] = problems
    summary["total_seconds"] = time.perf_counter() - started
    (args.output / "summary.json").write_text(
        json.dumps(summary, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    print(f"{len(names)} scenarios in {summary['total_seconds']:.1f}s; {len(problems)} problems")
    for problem in problems:
        print(f"PROBLEM {problem}")
    return 1 if problems else 0


if __name__ == "__main__":
    raise SystemExit(main())
