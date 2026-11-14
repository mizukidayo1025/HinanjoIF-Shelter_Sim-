"""Project-local compare-and-swap draft revisions (the Scenario previews and the CLI's drafts).

The plan assets and their uploads went with the older screens (#376)."""

from __future__ import annotations

from typing import Any

from sheltersim.contracts.workspace import DraftInput
from sheltersim.storage.projects import ProjectStore, StorageError, canonical, digest, safe_document


class ArtifactStore:
    DRAFT_BYTES = 8 * 1024 * 1024

    def __init__(self, store: ProjectStore) -> None:
        self.store = store

    def draft(self, project_id: str, draft_id: str) -> dict[str, Any]:
        path = self.store._path(project_id, "drafts", draft_id, "current.json")
        pointer = self.store._read(path)
        revision = self.store._read(
            self.store._path(project_id, "drafts", draft_id, f"v{pointer['revision']}.json")
        )
        if pointer["content_hash"] != digest(revision["content"]):
            raise StorageError("draft_integrity_mismatch")
        return revision

    def save_draft(self, project_id: str, draft_id: str, request: DraftInput) -> dict[str, Any]:
        content = safe_document(request.content)
        if content != request.content:
            raise StorageError("remove sensitive text before saving or approving a draft")
        if len(canonical(content).encode()) > self.DRAFT_BYTES:
            raise StorageError("draft_capacity_exceeded")
        with self.store.transaction():
            self.store._read(self.store._path(project_id, "project.json"))
            pointer = self.store._path(project_id, "drafts", draft_id, "current.json")
            prior = self.store._read(pointer) if pointer.exists() else None
            if request.expected_hash != (prior["content_hash"] if prior else None):
                raise StorageError("draft_conflict: reload the current revision")
            revision = 1 + (prior["revision"] if prior else 0)
            record = {
                "schema_version": 1,
                "draft_id": draft_id,
                "kind": request.kind,
                "revision": revision,
                "content_hash": digest(content),
                "content": content,
            }
            self.store._write(
                self.store._path(project_id, "drafts", draft_id, f"v{revision}.json"),
                record,
                immutable=True,
            )
            self.store._write(
                pointer,
                {key: value for key, value in record.items() if key not in {"content", "kind"}},
            )
            return record
