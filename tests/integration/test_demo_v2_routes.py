"""The screen (demo-v2.html at /, #376) and its files are served locally by the HTTP routes.

Its zone target resolution is checked in tests/unit/llm/test_natural_language.py.
"""

import re
from io import BytesIO
from pathlib import Path

import pytest

from sheltersim.web.server import _handler_for


@pytest.mark.parametrize(
    ("route", "content_type", "marker"),
    [
        ("/", "text/html", "避難所IF！".encode()),
        ("/demo-v2.css", "text/css", b"--navy"),
        ("/demo-v2-header-logo.png", "image/png", b"\x89PNG\r\n\x1a\n"),
        ("/demo-v2.js", "text/javascript", b"loading(init)"),
        ("/demo-v2-map.js", "text/javascript", b"Hinanj"),
        ("/demo-v2-scene.js", "text/javascript", b"HinanjoScene"),
        ("/demo-v2-clock.js", "text/javascript", b"HinanjoReplayClock"),
        ("/demo-v2-replay.js", "text/javascript", b"HinanjoReplayPlayer"),
        ("/demo-v2-sprites.js", "text/javascript", b"HinanjoSprites"),
        ("/demo-v2-state.js", "text/javascript", b"window.HinanjoApp = {"),
        ("/demo-v2-text.js", "text/javascript", b"metricDefs"),
        ("/demo-v2-api.js", "text/javascript", b"/api/v1/projects/"),
        ("/demo-v2-pages.js", "text/javascript", b"function show("),
        ("/demo-v2-draw.js", "text/javascript", b"function drawMap("),
        ("/demo-v2-home.js", "text/javascript", b"function loadHome("),
        ("/demo-v2-scenario.js", "text/javascript", b"scenario-approve"),
        ("/demo-v2-simulation.js", "text/javascript", b"function renderFrame("),
        ("/demo-v2-people.js", "text/javascript", b"function openPersonDialog("),
        ("/demo-v2-comparison.js", "text/javascript", b"intervention-apply"),
        ("/demo-v2-report.js", "text/javascript", b"function loadReport("),
        ("/demo-v2-print.js", "text/javascript", b"function printReport("),
        ("/demo-v2-detail.js", "text/javascript", b"function loadDetail("),
    ],
)
def test_backup_assets_are_independent_and_served_locally(tmp_path, route, content_type, marker):
    handler_type = _handler_for()
    handler = handler_type.__new__(handler_type)
    statuses, headers = [], {}
    handler.send_response = statuses.append
    handler.send_header = lambda key, value: headers.__setitem__(key, value)
    handler.end_headers = lambda: None
    handler.wfile = BytesIO()
    handler.path = route
    handler.do_GET()
    body = handler.wfile.getvalue()
    assert statuses == [200]
    assert headers["Content-Type"].startswith(content_type)
    assert marker in body
    if route == "/":
        assert b"/demo-v2.js" in body and b"/demo-v2.css" in body
        assert b"https://cdn" not in body


def test_every_file_the_page_loads_is_served():
    """Each script and stylesheet demo-v2.html loads is a route (the screen is in several
    demo-v2-*.js files since #491; a file left out of the server's list stops the page)."""
    html = (
        Path(__file__).resolve().parents[2]
        / "src"
        / "sheltersim"
        / "web"
        / "static"
        / "demo-v2.html"
    ).read_text(encoding="utf-8")
    paths = re.findall(r'<(?:script src|link rel="stylesheet" href)="(/demo-v2[^"]*)"', html)
    assert "/demo-v2.js" in paths and len(paths) > 10
    handler_type = _handler_for()
    for path in paths:
        handler = handler_type.__new__(handler_type)
        statuses = []
        handler.send_response = statuses.append
        handler.send_header = lambda key, value: None
        handler.end_headers = lambda: None
        handler.wfile = BytesIO()
        handler.path = path
        handler.do_GET()
        assert statuses == [200], path
