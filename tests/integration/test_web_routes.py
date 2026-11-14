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


@pytest.mark.parametrize(
    "route",
    [
        "/app.js",
        "/project.js",
        "/project-round3.js",
        "/style.css",
        "/project.css",
        "/map-lod.js",
        "/performance.js",
        "/vendor/pdfjs/pdf.min.mjs",
    ],
)
def test_the_older_screens_files_are_not_served(
    tmp_path: Path, bare_request_handler, route: str
) -> None:
    handler, statuses, _, output = bare_request_handler(tmp_path)
    handler.path = route

    handler.do_GET()

    assert statuses == [404]
    assert b"page not found" in output.getvalue()


@pytest.mark.parametrize(
    "route", ["/api/v1/runs", "/api/v1/runs/run-one", "/api/v1/demo", "/api/v1/jobs"]
)
def test_the_older_screens_apis_are_unknown(
    tmp_path: Path, bare_request_handler, route: str
) -> None:
    handler, statuses, _, output = bare_request_handler(tmp_path)
    handler.path = route

    handler.do_GET()

    assert statuses == [400]
    assert b"unknown API route" in output.getvalue()


@pytest.mark.parametrize(
    "route",
    ["/api/v1/demo/start", "/api/v1/nl/apply", "/api/v1/jobs/j/cancel", "/api/v1/results-chat"],
)
def test_the_older_screens_mutations_are_refused(
    tmp_path: Path, bare_request_handler, route: str
) -> None:
    from io import BytesIO

    handler, statuses, _, output = bare_request_handler(tmp_path)
    handler.path = route
    handler.headers = {"Content-Type": "application/json", "Content-Length": "2"}
    handler.rfile = BytesIO(b"{}")

    handler.do_POST()

    assert statuses == [405]
    assert b"read-only" in output.getvalue()
