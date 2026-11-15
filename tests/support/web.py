"""A socketless request handler of the local web server."""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path
from typing import Any

import pytest


def _bare_request_handler(tmp_path: Path) -> tuple[Any, list[int], dict[str, str], Any]:
    from io import BytesIO

    from sheltersim.storage.projects import ProjectStore
    from sheltersim.web.projects import ProjectAPI
    from sheltersim.web.server import _handler_for

    handler_type = _handler_for(ProjectAPI(ProjectStore(tmp_path / ".projects")))
    handler = handler_type.__new__(handler_type)
    statuses: list[int] = []
    headers: dict[str, str] = {}
    output = BytesIO()
    handler.send_response = statuses.append
    handler.send_header = lambda name, value: headers.__setitem__(name, value)
    handler.end_headers = lambda: None
    handler.wfile = output
    return handler, statuses, headers, output


@pytest.fixture
def bare_request_handler() -> Callable[[Path], tuple[Any, list[int], dict[str, str], Any]]:
    """Make a web request handler over a Project store in a directory, without a socket;
    return it with the statuses, headers and body it writes.
    """
    return _bare_request_handler


def server_answer(
    api: Any, method: str, parts: list[str], query: str = "", body: str | None = None
) -> tuple[int, str]:
    """The server's status and body of a Project request (web/server.py's do_GET and do_POST):
    `parts` after /api/v1/projects, `query` the text after "?", `body` a POST's text."""
    import json

    from sheltersim.storage.projects import StorageError
    from sheltersim.web.projects import answers

    def text(value: Any) -> str:
        return json.dumps(
            value, ensure_ascii=False, sort_keys=True, separators=(",", ":"), allow_nan=False
        )

    try:
        if method == "GET":
            return 200, api.answer_get(parts, query).decode()
        raw = (body or "").encode()
        if not 0 < len(raw) <= 2_097_152:
            raise StorageError("project request exceeds the body limit")
        document = json.loads(raw.decode("utf-8"))
        if not isinstance(document, dict):
            raise StorageError("project request must be an object")
        return 200, api.answer_post(parts, raw).decode()
    except answers().Refused as refusal:
        return refusal.args[0], refusal.args[3].decode()
    except StorageError as exc:
        return 409, text({"error": str(exc), "code": exc.code})
    except (ValueError, UnicodeDecodeError):
        if method == "GET":
            return 400, text({"error": "invalid query or saved contract", "code": "invalid_input"})
        return 400, text({"error": "invalid project request"})
