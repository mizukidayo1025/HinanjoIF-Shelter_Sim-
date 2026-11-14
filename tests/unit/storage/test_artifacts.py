"""Drafts: a stale edit never overwrites a newer revision."""

from __future__ import annotations

from pathlib import Path

import pytest

from sheltersim.contracts.workspace import DraftInput
from sheltersim.storage.artifacts import ArtifactStore
from sheltersim.storage.projects import StorageError


def test_draft_cas_preserves_prior_revision_and_rejects_stale_editor(
    project_store_at, tmp_path: Path
) -> None:
    store = project_store_at(tmp_path)
    drafts = ArtifactStore(store)
    initial = drafts.save_draft("project", "draft", DraftInput(content={"people": 100}))
    edited = drafts.save_draft(
        "project",
        "draft",
        DraftInput(content={"people": 500}, expected_hash=initial["content_hash"]),
    )
    with pytest.raises(StorageError, match="draft_conflict"):
        drafts.save_draft(
            "project",
            "draft",
            DraftInput(content={"people": 30}, expected_hash=initial["content_hash"]),
        )
    assert ArtifactStore(store).draft("project", "draft")["content_hash"] == edited["content_hash"]
    assert store._read(store._path("project", "drafts", "draft", "v1.json"))["content"] == {
        "people": 100
    }
