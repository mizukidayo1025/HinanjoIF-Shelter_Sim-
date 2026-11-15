"""The corpus of Scenarios the core's minute-0 builder is checked on (#429).

`fixtures/initial_corpus.json.xz` holds Scenarios recorded from the offline test suite: every
Scenario that `WorldState.from_scenario` built while `tests/unit` and `tests/integration` ran,
one per fingerprint of their configs (`world_export.configs_fingerprint`, the smallest Scenario of
each), so that each installed model, extension and config the tests use is there once. Each case
holds the Scenario's canonical JSON and the sha256 and length of what
`export_world(world, controller)` writes of its world at minute 0 with the Run's controller
(sorted keys, no spaces). `tests/unit/core/test_world_export.py` checks that the core builds the
same bytes (`sheltersim_native.initial_entry`), against Python and against the recorded digest.

`python benchmarks/initial_corpus.py` records it again: it runs the suite with this module as a
pytest plugin (`-p support.initial_corpus`), which writes each Scenario to the directory named by
`SHELTERSIM_RECORD_SCENARIOS`, then keeps those Python builds. The file is xz-compressed: the
Scenarios share most of their text.
"""

from __future__ import annotations

import gc
import hashlib
import json
import lzma
import os
from pathlib import Path
from typing import Any

from support.paths import INITIAL_CORPUS

SCHEMA_VERSION = 2
RECORD_DIRECTORY = "SHELTERSIM_RECORD_SCENARIOS"


def entry_bytes(scenario: Any) -> bytes:
    """`export_world(world, controller)` of the Scenario's world at minute 0, set up for a Run as
    `run_scenario` sets it up (offline, traces off), as the browser's sample writes an entry
    (sorted keys, no spaces, not ASCII-escaped)."""
    import tempfile

    from sheltersim.core.engine import TickEngine, TraceLevel
    from sheltersim.core.world import WorldState
    from sheltersim.core.world_export import export_world
    from sheltersim.experiments.runner import OfflineSimulationController
    from sheltersim.inspection.traces import RunArtifactWriter

    world = WorldState.from_scenario(scenario)
    # The artifacts' trace spool (SQLite) stays open while the controller lives, which Windows does
    # not let the cleanup remove.
    with tempfile.TemporaryDirectory(
        prefix="initial-corpus-", ignore_cleanup_errors=True
    ) as directory:
        artifacts = RunArtifactWriter(scenario, directory, trace_level=TraceLevel.OFF)
        artifacts.write_initial(world, execution_mode="offline")
        controller = OfflineSimulationController(world, artifacts)
        TickEngine(world, phase_handler=controller)
        entry = export_world(world, controller)
        del controller, artifacts
        gc.collect()
    return json.dumps(entry, sort_keys=True, ensure_ascii=False, separators=(",", ":")).encode()


def load_corpus() -> list[dict[str, Any]]:
    """The committed cases, in their order."""
    document = json.loads(lzma.decompress(INITIAL_CORPUS.read_bytes()))
    if document["schema_version"] != SCHEMA_VERSION:
        raise ValueError("the corpus has another schema_version")
    return document["cases"]


def corpus_of(directory: Path) -> dict[str, Any]:
    """The corpus of the Scenarios recorded in `directory`: one per fingerprint of the configs,
    the smallest, among those Python builds a world of."""
    from sheltersim.core.world_export import configs_fingerprint
    from sheltersim.experiments.scenario import ScenarioSpec

    smallest: dict[str, str] = {}
    for path in sorted(directory.glob("*.json")):
        text = path.read_text(encoding="utf-8")
        fingerprint = configs_fingerprint(json.loads(text)["model_configs"])
        known = smallest.get(fingerprint)
        if known is None or (len(text), text) < (len(known), known):
            smallest[fingerprint] = text
    cases = []
    for text in sorted(smallest.values(), key=lambda text: hashlib.sha256(text.encode()).digest()):
        try:
            scenario = ScenarioSpec.model_validate_json(text)
            entry = entry_bytes(scenario)
        except Exception:  # noqa: BLE001 - a Scenario only a test's own registrations accept
            continue
        if scenario.canonical_json() != text:
            continue
        cases.append(
            {
                "scenario": text,
                "entry_sha256": hashlib.sha256(entry).hexdigest(),
                "entry_bytes": len(entry),
            }
        )
    return {"schema_version": SCHEMA_VERSION, "cases": cases}


def write_corpus(directory: Path) -> Path:
    document = json.dumps(corpus_of(directory), sort_keys=True, separators=(",", ":"))
    INITIAL_CORPUS.write_bytes(lzma.compress(document.encode(), preset=9 | lzma.PRESET_EXTREME))
    return INITIAL_CORPUS


# ---- The recording plugin ----------------------------------------------------------------------


def pytest_configure(config: Any) -> None:
    """With `SHELTERSIM_RECORD_SCENARIOS` set, write each Scenario `WorldState.from_scenario` is
    given to that directory, named by the sha256 of its canonical JSON."""
    directory = os.environ.get(RECORD_DIRECTORY)
    if not directory:
        return
    from sheltersim.core.world import WorldState

    original = WorldState.from_scenario.__func__  # type: ignore[attr-defined]

    def from_scenario(cls: Any, scenario: Any) -> Any:
        try:
            text = scenario.canonical_json()
        except Exception:  # noqa: BLE001 - a test's broken Scenario is not recorded
            return original(cls, scenario)
        path = Path(directory) / f"{hashlib.sha256(text.encode()).hexdigest()}.json"
        if not path.exists():
            partial = path.with_suffix(f".{os.getpid()}.part")
            partial.write_text(text, encoding="utf-8")
            partial.replace(path)
        return original(cls, scenario)

    WorldState.from_scenario = classmethod(from_scenario)  # type: ignore[method-assign]
