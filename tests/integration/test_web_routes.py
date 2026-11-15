"""Page routing of the local server: the screen is at / (#376).

/index.html goes to / with the query kept; the earlier paths are not redirected (#434). The
screen's files are checked in tests/integration/test_demo_v2_routes.py.
"""

from __future__ import annotations

from pathlib import Path


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
