"""Page routing of the local server: the screen is at / (#376).

/index.html goes to / with the query kept; the earlier paths are not redirected (#434). The
screen's files are checked in tests/integration/test_demo_v2_routes.py. /api/v1/llm answers
whether the AI's features are ON (#581).
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest


def test_index_html_redirects_to_the_screen_with_the_query(
    tmp_path: Path, bare_request_handler
) -> None:
    handler, statuses, headers, _ = bare_request_handler(tmp_path)
    handler.path = "/index.html?project=sample&run=run-one&branch=main"

    handler.do_GET()

    assert statuses == [302]
    assert headers["Location"] == "/?project=sample&run=run-one&branch=main"
    assert headers["Cache-Control"] == "no-store"


def test_index_html_without_a_query_redirects_to_the_bare_screen(
    tmp_path: Path, bare_request_handler
) -> None:
    handler, statuses, headers, _ = bare_request_handler(tmp_path)
    handler.path = "/index.html"

    handler.do_GET()

    assert statuses == [302]
    assert headers["Location"] == "/"


@pytest.mark.parametrize(("mode", "available"), [("offline", False), ("replay", True)])
def test_the_llm_status_route_answers_the_server_s_llm_mode(
    tmp_path: Path,
    bare_request_handler,
    monkeypatch: pytest.MonkeyPatch,
    mode: str,
    available: bool,
) -> None:
    monkeypatch.setenv("SHELTERSIM_LLM_MODE", mode)
    handler, statuses, _, output = bare_request_handler(tmp_path)
    handler.path = "/api/v1/llm"

    handler.do_GET()

    assert statuses == [200]
    answer = json.loads(output.getvalue())
    assert (answer["available"], answer["mode"]) == (available, mode)


def test_the_llm_status_route_refuses_a_query(tmp_path: Path, bare_request_handler) -> None:
    handler, statuses, _, _ = bare_request_handler(tmp_path)
    handler.path = "/api/v1/llm?x=1"

    handler.do_GET()

    assert statuses == [409]
