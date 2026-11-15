"""The trial worlds of the browser's core (stage 4 of #132, PR 4-5 of #244) and what the native
build of the core gives on them.

For each size (people) of the trial Scenario, the generated one of the Project screen
(`gate_scenarios.generated`, one day unless `--minutes`, seed 2026), this writes the entry the
core builds of its world at minute 0 (`sheltersim_native.initial_entry`) to `DIR/<name>.json`.
Then it runs that entry through the native `PlainRun` for the whole day and writes, to
`DIR/expected.json`, the digests the browser's run must give (each part of the world's canonical
document and the metric samples), with the time the ticks took. Unless `--skip-project`, it also
computes the same Scenario through the Project screen's route (`ProjectAPI`,
`gate_scenarios.project_run`) in a temporary store and records its time, the size of the stored
Run on disk, and whether its last world has the native Run's digests.

    python benchmarks/wasm_trial_worlds.py --output DIR [--sizes 20 100] [--minutes 1440]
        [--skip-project]

`--minutes` sets the Scenario's length (one day by default; 10080 for the seven days of #252). A
world of one day is `trial-<people>`; one of whole days is `trial-<people>-<days>d`, and any other
`trial-<people>-<minutes>m`.

Run from the repository root with src on PYTHONPATH, the native extension built and
SHELTERSIM_LLM_MODE=offline. The files are generated; they belong outside the repository.
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
# The trial Scenario: its sizes (people), its length in minutes and its seed.
SIZES = (20, 100)
MINUTES = 1440
SEED = 2026


def _bytes(directory: Path) -> int:
    return sum(path.stat().st_size for path in directory.rglob("*") if path.is_file())


def world_name(people: int, minutes: int) -> str:
    """The name of the trial world of `people` and `minutes` (see the module's docstring)."""
    if minutes == MINUTES:
        return f"trial-{people}"
    if minutes % MINUTES == 0:
        return f"trial-{people}-{minutes // MINUTES}d"
    return f"trial-{people}-{minutes}m"


def native(scenario: Any, output: Path, name: str) -> dict[str, Any]:
    """Write the entry of the world at minute 0 as `output/<name>.json`, run it for the whole
    Scenario through the native `PlainRun`, and return what to expect of it."""
    import sheltersim_native

    from sheltersim import native as kernels

    started = time.perf_counter()
    entry = bytes(sheltersim_native.initial_entry(scenario.canonical_json().encode()))
    build_seconds = time.perf_counter() - started
    path = output / f"{name}.json"
    path.write_bytes(entry)
    run = kernels.rules("plain").PlainRun(entry)
    initial = json.loads(run.digests_json())
    ticks = scenario.duration_minutes
    started = time.perf_counter()
    run.advance(ticks)
    tick_seconds = time.perf_counter() - started
    return {
        "file": path.name,
        "entry_bytes": path.stat().st_size,
        "build_seconds": build_seconds,
        "initial": initial,
        "expected": {
            "ticks": ticks,
            "world": json.loads(run.digests_json()),
            "metrics": run.metrics_digest(),
            "metric_count": run.metric_count(),
            "tick_seconds": tick_seconds,
        },
    }


def project(scenario: Any, directory: Path, world_expected: dict[str, str]) -> dict[str, Any]:
    """The Project screen's route for the same Scenario, in a store under `directory`."""
    from gate_scenarios import project_run

    root = directory / "projects"
    started = time.perf_counter()
    digests = json.loads(project_run(root, scenario, scenario.duration_minutes).digests_json())
    seconds = time.perf_counter() - started
    run = root / "p" / "runs" / "r"
    return {
        "seconds": seconds,
        "minute": scenario.duration_minutes,
        "run_bytes": _bytes(run),
        "run_parts_bytes": {
            child.name: _bytes(child) if child.is_dir() else child.stat().st_size
            for child in sorted(run.iterdir())
        },
        "differing_parts": [
            part for part, value in digests.items() if world_expected[part] != value
        ],
    }


def main(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--output", type=Path, required=True, help="directory to write into")
    parser.add_argument("--sizes", type=int, nargs="+", default=list(SIZES), help="people")
    parser.add_argument("--minutes", type=int, default=MINUTES, help="the Scenario's length")
    parser.add_argument("--skip-project", action="store_true", help="skip the Project route")
    args = parser.parse_args(argv)
    sys.path.insert(0, str(ROOT / "benchmarks"))
    from gate_scenarios import generated

    args.output.mkdir(parents=True, exist_ok=True)
    worlds: dict[str, Any] = {}
    with tempfile.TemporaryDirectory(prefix="wasm-trial-") as work:
        # The usage ledgers stay in the temporary directory.
        os.environ["SHELTERSIM_PROJECTS_DIR"] = str(Path(work) / "ledgers")
        os.environ["XDG_STATE_HOME"] = str(Path(work) / "state")
        tempfile.tempdir = work
        try:
            for people in args.sizes:
                name = world_name(people, args.minutes)
                scenario = generated(evacuees=people, minutes=args.minutes, seed=SEED)
                result = {"evacuees": people, **native(scenario, args.output, name)}
                if not args.skip_project:
                    result["project"] = project(
                        scenario, Path(work) / name, result["expected"]["world"]
                    )
                worlds[name] = result
                print(json.dumps({name: result}, indent=1), flush=True)
        finally:
            tempfile.tempdir = None
    document = {
        "scenario": {
            "generator": "gate_scenarios.generated",
            "minutes": args.minutes,
            "seed": SEED,
        },
        "platform": {"python": platform.python_version(), "machine": platform.machine()},
        "worlds": worlds,
    }
    (args.output / "expected.json").write_text(
        json.dumps(document, sort_keys=True, indent=1) + "\n", encoding="utf-8"
    )


if __name__ == "__main__":
    main()
