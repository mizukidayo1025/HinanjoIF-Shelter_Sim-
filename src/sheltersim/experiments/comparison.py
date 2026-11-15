"""Comparisons of computed Runs by their metric records (#450), and the paired-seed summary of a
model sensitivity case (scripts/run_model_sensitivity.py).

`compare_runs` compares branches of Runs in a Project (`sheltersim compare`): each branch's KPI
rows, as the screen's history answer gives them (`kind=kpis`, experiments/project_runs.py), over
the minutes both have computed. The baseline and the candidate branches are paired by their
Scenarios' seeds and must have the same people.
"""

from __future__ import annotations

import math
import re
import statistics
from collections.abc import Iterable
from typing import Any

from sheltersim.experiments.project_runs import (
    branch_record,
    kpi_rows,
    metrics_of,
    run_scenario_document,
)
from sheltersim.experiments.validation import paired_summary
from sheltersim.storage.projects import digest


def parse_branch(text: str) -> tuple[str, str]:
    """`RUN` (its branch `main`) or `RUN/BRANCH`."""
    run_id, _, branch_id = text.partition("/")
    if not run_id or "/" in branch_id:
        raise ValueError(f"not RUN or RUN/BRANCH: {text}")
    return run_id, branch_id or "main"


def compare_runs(
    api: Any,
    project_id: str,
    baseline: list[str],
    candidate: list[str],
    *,
    metrics: list[str] | None = None,
) -> dict[str, Any]:
    """Compares the candidate branches with the baseline branches of the same seeds: for each
    pair, the first minute a metric differs and the metrics at their last common minute; over
    the pairs, the paired summary of each metric's final difference."""

    def side(texts: list[str]) -> dict[int, dict[str, Any]]:
        by_seed: dict[int, dict[str, Any]] = {}
        for text in texts:
            run_id, branch_id = parse_branch(text)
            record = branch_record(api, project_id, run_id, branch_id)
            scenario = run_scenario_document(api, project_id, run_id)
            if scenario["seed"] in by_seed:
                raise ValueError("duplicate paired seed")
            by_seed[scenario["seed"]] = {
                "run_id": run_id,
                "branch_id": branch_id,
                "seed": scenario["seed"],
                "scenario_id": scenario["id"],
                "scenario_hash": digest(scenario),
                "head_hash": record["checkpoint_hash"],
                "fork_minute": record["fork_minute"],
                "computed_until": record["computed_until"],
                "input_hash": digest(scenario["profiles"]),
            }
        return by_seed

    before, after = side(baseline), side(candidate)
    if set(before) != set(after) or not before:
        raise ValueError("same paired seeds required")
    pairs = []
    finals: dict[str, list[dict[str, Any]]] = {"baseline": [], "candidate": []}
    names: list[str] | None = None
    for seed in sorted(before):
        left, right = before[seed], after[seed]
        if left["input_hash"] != right["input_hash"]:
            raise ValueError("paired Runs do not have the same people")
        start = max(left["fork_minute"], right["fork_minute"])
        end = min(left["computed_until"], right["computed_until"])
        if end <= start:
            raise ValueError("paired branches have no common computed minute")
        rows = [
            {
                row["minute_index"]: metrics_of(row)
                for row in kpi_rows(api, project_id, each["run_id"], each["branch_id"], start, end)
            }
            for each in (left, right)
        ]
        common = sorted(set(rows[0]) & set(rows[1]))
        if not common:
            raise ValueError("paired branches have no common KPI row")
        if names is None:
            available = set(rows[0][common[-1]]) & set(rows[1][common[-1]])
            names = sorted(available) if metrics is None else list(metrics)
            unknown = [name for name in names if name not in available]
            if unknown:
                raise ValueError(f"unknown metric: {', '.join(unknown)}")
        divergence = None
        for minute in common:
            changed = [name for name in names if rows[0][minute][name] != rows[1][minute][name]]
            if changed:
                divergence = {"minute": minute, "metrics": changed}
                break
        last = common[-1]
        final = {
            name: {
                "baseline": rows[0][last][name],
                "candidate": rows[1][last][name],
                "delta": rows[1][last][name] - rows[0][last][name],
            }
            for name in names
        }
        for key, each, values in (("baseline", left, rows[0]), ("candidate", right, rows[1])):
            finals[key].append(
                {
                    "seed": seed,
                    "input_hash": each["input_hash"],
                    "metrics": {name: values[last][name] for name in names},
                }
            )
        pairs.append(
            {
                "seed": seed,
                "baseline": left,
                "candidate": right,
                "common_range": [start, end],
                "final_minute": last,
                "first_divergence": divergence,
                "final_metrics": final,
            }
        )
    return {
        "schema_version": 1,
        "project_id": project_id,
        "pairs": pairs,
        "paired_summary": [
            paired_summary(finals["baseline"], finals["candidate"], name) for name in names or []
        ],
        "limits": (
            "Differences of the simulator's metric records for the chosen seeds; not a "
            "real-world causal effect."
        ),
    }


def compare_sensitivity_run_sets(
    baseline_runs: Iterable[dict[str, Any]],
    variant_runs: Iterable[dict[str, Any]],
    *,
    case_id: str,
    parameter_path: str,
    baseline_value: Any,
    variant_value: Any,
    assumption_id: str,
    model_version: str,
    expected_seeds: Iterable[int] | None = None,
    context_source_ids: Iterable[str] = (),
) -> dict[str, Any]:
    """Compare one changed model assumption across the same small set of seeds.

    Unlike an event intervention comparison, the frozen model configuration is expected
    to differ at exactly one declared parameter. The caller supplies a canonical input
    hash with that parameter and the simulation seed replaced by fixed markers.
    """
    if not case_id or not parameter_path.startswith("model_configs."):
        raise ValueError("sensitivity case and model_configs parameter path are required")
    if not assumption_id or not model_version:
        raise ValueError("sensitivity results require an assumption ID and model version")
    if type(baseline_value) is not type(variant_value) or baseline_value == variant_value:
        raise ValueError("sensitivity baseline and variant values must be distinct, matching types")

    def index_runs(
        documents: Iterable[dict[str, Any]], expected_value: Any, condition: str
    ) -> dict[int, dict[str, Any]]:
        indexed: dict[int, dict[str, Any]] = {}
        for document in documents:
            if not isinstance(document, dict):
                raise ValueError("sensitivity run records must be objects")
            if document.get("case_id") != case_id:
                raise ValueError("sensitivity run record belongs to a different case")
            seed = document.get("seed")
            if not isinstance(seed, int) or isinstance(seed, bool) or seed < 0:
                raise ValueError("sensitivity run seed must be a non-negative integer")
            if seed in indexed:
                raise ValueError(f"sensitivity run set contains duplicate seed {seed}")
            value = document.get("parameter_value")
            if type(value) is not type(expected_value) or value != expected_value:
                raise ValueError(f"{condition} run does not use its declared parameter value")
            for key in (
                "scenario_hash",
                "model_config_hash",
                "final_state_hash",
                "input_hash",
            ):
                if not _is_sha256(document.get(key)):
                    raise ValueError(f"sensitivity run {key} must be a SHA-256 hex digest")
            metrics = document.get("metrics")
            if not isinstance(metrics, dict) or not metrics:
                raise ValueError("sensitivity runs need final numeric metrics")
            for metric, value in metrics.items():
                if (
                    not isinstance(metric, str)
                    or not isinstance(value, (int, float))
                    or isinstance(value, bool)
                    or not math.isfinite(value)
                ):
                    raise ValueError("sensitivity metric names and values must be finite numbers")
            indexed[seed] = document
        if len(indexed) < 2:
            raise ValueError("sensitivity analysis requires at least two paired seeds")
        return indexed

    baseline_by_seed = index_runs(baseline_runs, baseline_value, "baseline")
    variant_by_seed = index_runs(variant_runs, variant_value, "variant")
    if baseline_by_seed.keys() != variant_by_seed.keys():
        raise ValueError("sensitivity variants must use the same paired seeds")
    seeds = sorted(baseline_by_seed)
    if expected_seeds is not None:
        requested = list(expected_seeds)
        if (
            any(
                not isinstance(seed, int) or isinstance(seed, bool) or seed < 0
                for seed in requested
            )
            or len(requested) != len(set(requested))
            or seeds != sorted(requested)
        ):
            raise ValueError("sensitivity runs do not match the declared seed list")

    paired = [(seed, baseline_by_seed[seed], variant_by_seed[seed]) for seed in seeds]
    input_hashes = {
        document["input_hash"]
        for _, baseline, variant in paired
        for document in (baseline, variant)
    }
    scenario_ids = {
        document.get("scenario_id")
        for _, baseline, variant in paired
        for document in (baseline, variant)
    }
    metric_names = {
        frozenset(document["metrics"])
        for _, baseline, variant in paired
        for document in (baseline, variant)
    }
    if len(input_hashes) != 1:
        raise ValueError("paired sensitivity runs must have identical inputs except seed/parameter")
    if len(scenario_ids) != 1 or None in scenario_ids:
        raise ValueError("paired sensitivity runs must use the same scenario ID")
    if len(metric_names) != 1:
        raise ValueError("paired sensitivity runs must report the same metrics")

    metrics: dict[str, dict[str, Any]] = {}
    for metric in sorted(next(iter(metric_names))):
        baseline_values = [float(base["metrics"][metric]) for _, base, _ in paired]
        variant_values = [float(candidate["metrics"][metric]) for _, _, candidate in paired]
        deltas = [
            after - before for before, after in zip(baseline_values, variant_values, strict=True)
        ]
        positive = sum(value > 0.0 for value in deltas)
        negative = sum(value < 0.0 for value in deltas)
        zero = len(deltas) - positive - negative
        metrics[metric] = {
            "baseline": _distribution(baseline_values),
            "variant": _distribution(variant_values),
            "paired_delta": {
                **_distribution(deltas),
                "positive_seed_count": positive,
                "negative_seed_count": negative,
                "zero_seed_count": zero,
                "same_nonzero_direction": zero == 0 and (positive == 0 or negative == 0),
            },
        }

    paired_runs = []
    for seed, baseline, variant in paired:
        paired_runs.append(
            {
                "seed": seed,
                "baseline": {
                    "scenario_hash": baseline["scenario_hash"],
                    "model_config_hash": baseline["model_config_hash"],
                    "final_state_hash": baseline["final_state_hash"],
                    "metrics": baseline["metrics"],
                },
                "variant": {
                    "scenario_hash": variant["scenario_hash"],
                    "model_config_hash": variant["model_config_hash"],
                    "final_state_hash": variant["final_state_hash"],
                    "metrics": variant["metrics"],
                },
            }
        )
    return {
        "schema_version": 1,
        "case_id": case_id,
        "scenario_id": next(iter(scenario_ids)),
        "input_hash": next(iter(input_hashes)),
        "parameter": {
            "path": parameter_path,
            "baseline_value": baseline_value,
            "variant_value": variant_value,
            "source_type": "assumption",
            "assumption_id": assumption_id,
            "model_version": model_version,
            "context_source_ids": sorted(set(context_source_ids)),
        },
        "seed_count": len(seeds),
        "seeds": seeds,
        "metrics": metrics,
        "paired_runs": paired_runs,
        "uncertainty_notes": [
            f"Descriptive paired results from {len(seeds)} synthetic simulation seeds.",
            "Dispersion is sample standard deviation and empirical quantiles.",
            "No confidence interval or real-world calibration is claimed.",
            "All non-seed inputs match within this case; only the declared parameter changes.",
        ],
        "interpretation": (
            "Conditional behavior of the configured synthetic simulator, not an estimate of "
            "real shelter outcomes or evidence that one policy is generally better."
        ),
    }


def _distribution(values: list[float]) -> dict[str, float | None]:
    ordered = sorted(values)

    def quantile(probability: float) -> float:
        if len(ordered) == 1:
            return ordered[0]
        position = (len(ordered) - 1) * probability
        lower = math.floor(position)
        upper = math.ceil(position)
        if lower == upper:
            return ordered[lower]
        fraction = position - lower
        return ordered[lower] * (1.0 - fraction) + ordered[upper] * fraction

    return {
        "mean": statistics.fmean(ordered),
        "stdev": statistics.stdev(ordered) if len(ordered) > 1 else 0.0,
        "p05": quantile(0.05),
        "median": quantile(0.5),
        "p95": quantile(0.95),
    }


def _is_sha256(value: Any) -> bool:
    return isinstance(value, str) and re.fullmatch(r"[a-f0-9]{64}", value) is not None
