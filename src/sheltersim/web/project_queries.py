"""Bounded, read-only Project evidence queries. No live provider capability."""

from __future__ import annotations

import base64
import json
from typing import Any

from sheltersim.contracts.workspace import HistoryQuery
from sheltersim.storage.projects import ProjectStore, StorageError, canonical, digest

# The kinds of history a Branch answers (#368 decision A): the KPIs and the conversation of
# the questions. The history index still writes the other kinds until it is removed (#368
# step 10); the routes that read them went with the older screens (#376).
HISTORY_KINDS = frozenset({"kpis", "conversations"})


def page(
    items: list[Any], query: dict[str, Any], anchor: str, *, limit: int, cursor: str | None = None
) -> dict[str, Any]:
    scope = digest(query)
    offset = 0
    if cursor is not None:
        try:
            token = json.loads(base64.urlsafe_b64decode(cursor.encode()))
        except (ValueError, UnicodeDecodeError) as exc:
            raise StorageError("invalid_cursor") from exc
        if (
            not isinstance(token, dict)
            or set(token) != {"scope", "anchor", "offset"}
            or token["scope"] != scope
            or token["anchor"] != anchor
            or type(token["offset"]) is not int
            or token["offset"] < 0
        ):
            raise StorageError("stale_or_foreign_cursor: restart this query")
        offset = token["offset"]
    next_offset = offset + limit
    following = None
    if next_offset < len(items):
        following = base64.urlsafe_b64encode(
            canonical(
                {
                    "scope": scope,
                    "anchor": anchor,
                    "offset": next_offset,
                }
            ).encode()
        ).decode()
    return {
        "schema_version": 1,
        "items": items[offset:next_offset],
        "next_cursor": following,
        "anchor_hash": anchor,
        "total_items": len(items),
    }


class ProjectQueries:
    def __init__(self, store: ProjectStore) -> None:
        self.store = store

    def history(
        self, project_id: str, run_id: str, branch_id: str, query: HistoryQuery
    ) -> dict[str, Any]:
        reference = self.store.branch_reference(project_id, run_id, branch_id)
        if not reference.fork_minute <= query.start <= query.end <= reference.computed_until:
            raise StorageError("history_outside_computed_range")
        # After the range, as the browser checks it (native/wasm/js/api_answers.mjs).
        if query.kind not in HISTORY_KINDS:
            raise StorageError(
                f"history_kind_unavailable: only {', '.join(sorted(HISTORY_KINDS))} are answered"
            )
        from sheltersim.storage.history_index import HistoryIndex

        # The search reads one index transaction and names the indexed head it
        # answers as of, so a save that moves the pointer meanwhile changes
        # nothing it returns (#262).
        result = HistoryIndex(self.store, project_id, run_id).search(
            branch_id, query, reference.checkpoint_hash, reference.computed_until
        )
        time_semantics = "end_of_tick" if query.kind == "kpis" else "recorded_event_minute"
        for item in result.get("items", []):
            if isinstance(item, dict):
                item.setdefault("run_id", run_id)
                item.setdefault("branch_id", branch_id)
                item.setdefault("time_semantics", time_semantics)
        result.update(
            {
                "run_id": run_id,
                "branch_id": branch_id,
                "kind": query.kind,
                "computed_until": reference.computed_until,
                "complete_lifetime_history": False,
                "time_semantics": time_semantics,
            }
        )
        return result
