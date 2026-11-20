"""The simulation map of demo-v2 keeps its size while playing (#377).

During playback the page must not change the map's width: the page reserves the vertical
scrollbar's gutter, even when the timeline’s record list grows.
"""

from __future__ import annotations

import re
from pathlib import Path

CSS = Path("src/sheltersim/web/static/demo-v2.css")


def _declarations(css: str, selector: str) -> dict[str, str]:
    """Declarations of the top-level rules with exactly `selector`, later ones winning."""
    found: dict[str, str] = {}
    css = re.sub(r"/\*.*?\*/", "", css, flags=re.S)
    for match in re.finditer(r"(?m)^([^\s@}][^{]*)\{([^}]*)\}", css):
        if match.group(1).strip() != selector:
            continue
        for declaration in match.group(2).split(";"):
            name, _, value = declaration.partition(":")
            if value:
                found[name.strip()] = value.strip()
    return found


def test_page_reserves_the_scrollbar_gutter() -> None:
    css = CSS.read_text(encoding="utf-8")
    assert _declarations(css, "html").get("scrollbar-gutter") == "stable"


def test_timeline_events_expand_without_an_inner_scroll() -> None:
    css = CSS.read_text(encoding="utf-8")
    events = _declarations(css, ".timeline-panel #events")
    assert events["height"] == "auto"
    assert events["overflow"] == "visible"
    hidden_cards = ".timeline-panel #events:not(.expanded) .event-item:nth-child(n + 4)"
    assert _declarations(css, hidden_cards)["display"] == "none"


def test_simulation_columns_do_not_grow_with_their_content() -> None:
    """A bare `fr` track's minimum is its content's, so long side content could widen
    that column and narrow the map's."""
    css = CSS.read_text(encoding="utf-8")
    columns = re.findall(r"\.simulation-grid\s*\{\s*grid-template-columns:\s*([^;]+);", css)
    assert columns
    for value in columns:
        tracks = re.findall(r"minmax\([^)]*\)|[\w.]+", value)
        if len(tracks) > 1:
            assert not any(t.endswith("fr") for t in tracks), value
