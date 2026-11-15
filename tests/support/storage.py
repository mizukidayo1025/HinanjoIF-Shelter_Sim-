"""Project stores, a stored Run, and storage on another platform."""

from __future__ import annotations

import json
import os
from collections.abc import Callable
from pathlib import Path
from typing import Any

import pytest

from support.paths import SCENARIO_100_AGENTS


@pytest.fixture
def project_store_at() -> Callable[[Path], Any]:
    """Open a ProjectStore at a folder, with an empty project named "project"."""

    def open_store(root: Path) -> Any:
        from sheltersim.storage.projects import ProjectStore

        store = ProjectStore(root)
        store.create_project("project")
        return store

    return open_store


def _saved_project(tmp_path: Path, scenario: Any = None) -> Any:
    from sheltersim.experiments.scenario import ScenarioSpec
    from sheltersim.storage.projects import ProjectStore, digest
    from sheltersim.web.stored_runs import project_files, save_scenario, start_run

    store = ProjectStore(tmp_path)
    files = project_files(store)
    with files.transaction():
        files.create_project("p")
    if scenario is None:
        scenario = ScenarioSpec.load(SCENARIO_100_AGENTS)
    saved = save_scenario(files, "p", scenario, digest(json.loads(scenario.canonical_json())))
    start_run(store, files, "p", "r", saved["scenario_id"], saved["scenario_version"])
    return store


@pytest.fixture(scope="session")
def saved_project() -> Callable[..., Any]:
    """Open a store at a folder with project p and Run r, its branch `main` stored at minute 0
    in format v5 (the server's Run since #427); return the ProjectStore.

    The Run is of scenario_100_agents unless a ScenarioSpec is passed."""
    return _saved_project


def _fake_msvcrt() -> Any:
    import fcntl
    import struct
    import types

    def locking(fd: int, mode: int, size: int) -> None:
        kind = fcntl.F_UNLCK if mode == 0 else fcntl.F_WRLCK
        try:
            lock = struct.pack("hh4xqqi4x", kind, os.SEEK_CUR, 0, size, 0)
            fcntl.fcntl(fd, fcntl.F_OFD_SETLK, lock)
        except OSError as exc:
            raise PermissionError(13, "locked") from exc

    return types.SimpleNamespace(LK_LOCK=1, LK_NBLCK=2, LK_UNLCK=0, locking=locking)


@pytest.fixture
def windows_like(monkeypatch: pytest.MonkeyPatch) -> None:
    """Make project storage see Windows: no fcntl, msvcrt locks, and no os.O_NOFOLLOW.

    The msvcrt locks are Linux open-file-description locks, so a test using this needs
    them (fcntl.F_OFD_SETLK).
    """
    import sheltersim.storage.projects as projects_module

    monkeypatch.setattr(projects_module, "fcntl", None)
    monkeypatch.setattr(projects_module, "msvcrt", _fake_msvcrt(), raising=False)
    monkeypatch.delattr(os, "O_NOFOLLOW")
