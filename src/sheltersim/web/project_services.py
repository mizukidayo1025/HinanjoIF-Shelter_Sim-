"""The Project routes of the local and VM versions outside the Run's: the EffectProgram operations
the core schedules (its catalog, which the browser answers too), and the LLM usage of a Project
and of a Run (the Python server's LLM Provider)."""

from __future__ import annotations

from typing import Any
from urllib.parse import parse_qs

from sheltersim.storage.projects import StorageError, digest
from sheltersim.storage.usage import UsageLedger
from sheltersim.web.project_queries import page


def one_query(query: str, allowed: set[str]) -> dict[str, str]:
    values = parse_qs(query)
    if set(values) - allowed or any(len(each) != 1 for each in values.values()):
        raise StorageError("invalid_query_fields")
    return {key: each[0] for key, each in values.items()}


class ProjectServices:
    def __init__(self, api: Any) -> None:
        self.api, self.store = api, api.store

    def get(self, parts: list[str], query: str) -> bytes | None:
        from sheltersim.web.projects import answers, json_bytes

        if len(parts) == 2 and parts[1] == "contracts" and not query:
            self.api.files.project(parts[0])
            return answers().contracts_answer()
        result = self._get(parts, query)
        return None if result is None else json_bytes(result)

    def _get(self, parts: list[str], query: str) -> dict[str, Any] | None:
        project_id = parts[0]
        if len(parts) == 2 and parts[1] == "usage":
            values = one_query(query, {"limit", "cursor"})
            limit = int(values.get("limit", "25"))
            if not 1 <= limit <= 100:
                raise StorageError("invalid_page_limit")
            self.store._read(self.store._path(project_id, "project.json"))
            rows = [
                UsageLedger(self.store, project_id, self.store._read(p)["account_id"]).summary()
                for p in sorted(self.store._path(project_id, "usage").glob("*.json"))
            ]
            result = page(
                rows,
                {"project_id": project_id, "kind": "usage"},
                digest(rows),
                limit=limit,
                cursor=values.get("cursor"),
            )
            result["project_totals"] = {
                key: sum(row["totals"][key] for row in rows)
                for key in ("calls", "input_tokens", "output_tokens", "cost_nusd")
            }
            return result
        if len(parts) == 4 and parts[1] == "runs" and parts[3] == "usage" and not query:
            self.api.files.run_record(project_id, parts[2])
            path = self.store._path(project_id, "runs", parts[2], "usage.json")
            if not path.exists():
                return {"schema_version": 1, "status": "legacy_account_not_initialized"}
            return UsageLedger(
                self.store, project_id, self.store._read(path)["account_id"]
            ).summary()
        return None
