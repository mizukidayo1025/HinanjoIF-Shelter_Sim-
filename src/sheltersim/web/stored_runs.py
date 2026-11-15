"""What the local and VM versions write when a Run starts (#427): its records and its branch
`main` at minute 0, in the browser's folders and format v5 (`sheltersim_native.rules.run_files`,
native/store), as the browser's `POST runs` does (native/wasm/js/scenario_routes.mjs).

The world at minute 0, the Run's model versions and its evidence are the core's, as the
browser's (`sheltersim_core::initial`, #429): Python reads the Scenario's bytes and stores what
the core makes of them. The Run's ticks, its saves and every answer about it are the core's.

A Run that calls an LLM Provider keeps its LLM settings in `runs/<run>/llm.json` beside its
records (the browser has none, and a bundle does not carry it): the mode it was computed in and
whether it is an offline experiment. A Run without the file is the screen's offline Run.
"""

from __future__ import annotations

import json
from typing import Any

from sheltersim import native
from sheltersim.experiments.scenario import ScenarioSpec
from sheltersim.storage.projects import ProjectStore, StorageError, canonical, digest

FILES = native.rules("run_files")
# The settings of a Run that calls an LLM Provider, beside its records.
LLM_FILE = "llm.json"


def project_files(store: ProjectStore) -> Any:
    """The stored Runs' files in the folder of `store`, with its limit."""
    return FILES.ProjectFiles(store.root, store.max_bytes)


def storage_error(error: ValueError) -> StorageError:
    """A refusal of the files (a ValueError with the text of Python's StorageError) as one."""
    return error if isinstance(error, StorageError) else StorageError(str(error))


def canonical_bytes(value: Any) -> bytes:
    return canonical(value).encode()


def minute_zero_entry(scenario: bytes) -> bytes:
    """The world at minute 0 of a Run of the Scenario of the canonical JSON `scenario`, as the
    core makes it for any Scenario (`sheltersim_core::initial::entry`, #429): the bytes
    `export_world` writes of the world and the controller a Run sets up."""
    import sheltersim_native

    return bytes(sheltersim_native.initial_entry(scenario))


def save_scenario(files: Any, project_id: str, scenario: ScenarioSpec, approved_hash: str) -> Any:
    """Saves an approved Scenario as a new revision: its record (Python's checks first: no
    sensitive text, the approval of this content)."""
    from sheltersim.storage.projects import safe_document

    document = json.loads(scenario.canonical_json())
    if safe_document(document) != document:
        raise StorageError("scenario contains sensitive text; remove it before approval")
    if approved_hash != digest(document):
        raise StorageError("scenario approval hash does not match content")
    with files.transaction():
        return json.loads(files.save_scenario(project_id, canonical_bytes(document), approved_hash))


def run_scenario(files: Any, project_id: str, run_id: str) -> ScenarioSpec:
    """The Scenario of a Run."""
    return ScenarioSpec.from_json(files.run_scenario_bytes(project_id, run_id).decode())


def llm_settings(store: ProjectStore, project_id: str, run_id: str) -> dict[str, Any] | None:
    """The LLM settings a Run keeps (`llm.json`), or None for the screen's offline Run."""
    path = store._path(project_id, "runs", run_id, LLM_FILE)
    return store._read(path) if path.exists() else None


def start_run(
    store: ProjectStore,
    files: Any,
    project_id: str,
    run_id: str,
    scenario_id: str,
    scenario_version: int,
    *,
    llm: dict[str, Any] | None = None,
) -> None:
    """Creates a Run of a saved Scenario revision and stores its branch `main` at minute 0 (the
    browser's `POST runs`). `llm` is what `llm.json` keeps, for a Run that calls a Provider.
    A Run whose branch `main` could not be stored is `failed`."""
    import sheltersim_native

    with files.transaction():
        files.project(project_id)
        record_path = store._path(project_id, "scenarios", scenario_id, f"v{scenario_version}.json")
        if type(scenario_version) is not int or scenario_version < 1:
            raise StorageError("invalid scenario revision")
        if not record_path.exists():
            raise StorageError("saved artifact is missing or corrupt")
        text = record_path.with_name(f"v{scenario_version}.scenario.json").read_bytes()
    seed = json.loads(text)["seed"]
    evidence = bytes(sheltersim_native.initial_evidence(text))
    versions = sheltersim_native.initial_model_versions(text)
    entry = minute_zero_entry(text)
    with files.transaction():
        files.create_run(
            project_id,
            run_id,
            scenario_id,
            scenario_version,
            seed,
            versions,
            evidence,
        )
        if llm is not None:
            store._write(store._path(project_id, "runs", run_id, LLM_FILE), llm, immutable=True)
    try:
        with files.claim_branch(project_id, run_id, "main") as lease, files.transaction():
            files.start_branch(lease, project_id, run_id, "main", entry)
    except Exception:
        with files.transaction():
            files.set_run_status(project_id, run_id, "failed")
        raise
