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
