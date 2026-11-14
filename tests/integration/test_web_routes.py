"""Page routing of the local server: /demo-v2 is the only screen (#376).

The older screens' paths (/demo, /admin, /index.html) and / go to /demo-v2 with the query kept,
as the VM's Caddy does (deploy/vm/setup.sh). The screen's files are checked in
tests/integration/test_demo_v2_routes.py.
"""

from __future__ import annotations

from pathlib import Path

import pytest


@pytest.mark.parametrize("route", ["/", "/demo", "/admin", "/index.html"])
def test_old_paths_redirect_to_the_screen_with_the_query(
    tmp_path: Path, bare_request_handler, route: str
) -> None:
    handler, statuses, headers, _ = bare_request_handler(tmp_path)
    handler.path = f"{route}?project=sample&run=run-one&branch=main"

    handler.do_GET()

    assert statuses == [302]
    assert headers["Location"] == "/demo-v2?project=sample&run=run-one&branch=main"
    assert headers["Cache-Control"] == "no-store"


def test_old_paths_without_a_query_redirect_to_the_bare_screen(
    tmp_path: Path, bare_request_handler
) -> None:
    handler, statuses, headers, _ = bare_request_handler(tmp_path)
    handler.path = "/demo"

    handler.do_GET()

    assert statuses == [302]
    assert headers["Location"] == "/demo-v2"
