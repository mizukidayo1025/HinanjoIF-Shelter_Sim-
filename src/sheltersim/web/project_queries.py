"""A page of a long answer, with a cursor bound to its query and anchor."""

from __future__ import annotations

import base64
import json
from typing import Any

from sheltersim.storage.projects import StorageError, canonical, digest


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
