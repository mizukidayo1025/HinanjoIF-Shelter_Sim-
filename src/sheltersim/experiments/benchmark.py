"""The synthetic Scenario of the offline timing (benchmarks/investigate_performance.py) and the
manifest of the environment it ran in."""

from __future__ import annotations

import hashlib
import os
import platform
import subprocess
from pathlib import Path
from typing import Any

try:
    import resource
except ImportError:  # pragma: no cover - exercised on Windows
    resource = None

from sheltersim.experiments.profiles import ProfileTemplate, SyntheticProfileGenerator
from sheltersim.experiments.scenario import ScenarioSpec


def build_benchmark_scenario(
    base: ScenarioSpec,
    *,
    agent_count: int,
    seed: int,
    duration_minutes: int,
) -> ScenarioSpec:
    """Scale a baseline scenario with deterministic synthetic profiles."""
    if not isinstance(agent_count, int) or isinstance(agent_count, bool) or agent_count <= 0:
        raise ValueError("agent_count must be a positive integer")
    if not isinstance(seed, int) or isinstance(seed, bool) or seed < 0:
        raise ValueError("seed must be a non-negative integer")
    if (
        not isinstance(duration_minutes, int)
        or isinstance(duration_minutes, bool)
        or duration_minutes <= 0
    ):
        raise ValueError("duration_minutes must be a positive integer")
    if any(event.due_minute >= duration_minutes for event in base.scheduled_events):
        raise ValueError("base scenario has planned events outside the benchmark duration")

    generation = base.provenance.get("profile_generation")
    if not isinstance(generation, dict):
        raise ValueError("base scenario must include a profile_generation manifest")
    try:
        template = ProfileTemplate.model_validate(generation["template"])
        zone_ids = tuple(generation["zone_ids"])
    except (KeyError, TypeError, ValueError) as exc:
        raise ValueError("base profile-generation manifest is invalid") from exc

    generated = SyntheticProfileGenerator().generate(
        count=agent_count,
        seed=seed,
        zone_ids=zone_ids,
        template=template,
    )
    document = base.model_dump(mode="json")
    document["id"] = f"benchmark-{agent_count}-{duration_minutes}-{seed}"
    document["duration_minutes"] = duration_minutes
    document["seed"] = seed
    document["profiles"] = [profile.model_dump(mode="json") for profile in generated.profiles]
    document["provenance"] = {
        **document["provenance"],
        "profile_generation": generated.manifest.model_dump(mode="json"),
        "benchmark": {"source_scenario_id": base.id, "synthetic_load": True},
    }
    # All generated profiles start in this zone; expand its declared capacity to fit them.
    for zone in document["zones"]:
        if zone["id"] in {profile.initial_zone_id for profile in generated.profiles}:
            current_capacity = zone["capacity"]
            zone["capacity"] = max(agent_count, current_capacity or 0)
    return ScenarioSpec.model_validate(document)


def environment_manifest() -> dict[str, Any]:
    """Describe the environment needed to compare benchmark results."""
    project_root = Path(__file__).resolve().parents[3]
    try:
        git_sha = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=project_root,
            check=True,
            capture_output=True,
            text=True,
            timeout=3,
        ).stdout.strip()
    except (OSError, subprocess.SubprocessError):
        git_sha = None
    try:
        git_status = subprocess.run(
            ["git", "status", "--porcelain", "--untracked-files=all"],
            cwd=project_root,
            check=True,
            capture_output=True,
            text=True,
            timeout=3,
        ).stdout
        git_dirty = bool(git_status)
    except (OSError, subprocess.SubprocessError):
        git_dirty = None
    return {
        "python_version": platform.python_version(),
        "implementation": platform.python_implementation(),
        "platform": platform.platform(),
        "machine": platform.machine(),
        "cpu_count": os.cpu_count(),
        "git_sha": git_sha,
        "git_worktree_dirty": git_dirty,
        "source_tree_hash": _source_tree_hash(project_root),
        "execution_mode": "offline",
        "rng_algorithm": "sha256-keyed-v1",
        "model_versions": {
            "behavior": "utility-softmax-v1",
            "psychology": "causal-stress-v1",
            "space": "shortest-travel-time-v1",
            "toilet_service": "toilet-service-uniform-v1",
        },
    }


def _source_tree_hash(project_root: Path) -> str | None:
    sources = sorted((project_root / "src" / "sheltersim").rglob("*.py"))
    sources.extend(sorted((project_root / "benchmarks").rglob("*.py")))
    project_config = project_root / "pyproject.toml"
    if project_config.is_file():
        sources.append(project_config)
    if not sources:
        return None
    digest = hashlib.sha256()
    for source in sources:
        digest.update(source.relative_to(project_root).as_posix().encode("utf-8"))
        digest.update(b"\0")
        digest.update(source.read_bytes())
        digest.update(b"\0")
    return digest.hexdigest()
