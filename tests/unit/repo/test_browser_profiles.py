"""The fresh browser profiles of the browser checks (benchmarks/browser_profiles.py, #253).

Playwright's WebKit cannot open the origin private file system in an ephemeral context, so the
checks open WebKit on a persistent context of a new, empty directory per context, and remove the
directories when the browser is closed; Chromium and Firefox keep their ephemeral contexts. A fake
Playwright stands in for the real one, which is not a dependency of the project.
"""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

ROOT = Path(__file__).parents[3]


def _browser_profiles():
    spec = importlib.util.spec_from_file_location(
        "browser_profiles", ROOT / "benchmarks" / "browser_profiles.py"
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules.setdefault(spec.name, module)
    spec.loader.exec_module(module)
    return module


class _Context:
    def __init__(self, kind: str, directory: str | None = None) -> None:
        self.kind = kind
        self.directory = directory
        self.closed = False

    def close(self) -> None:
        self.closed = True


class _Browser:
    version = "1.0"

    def __init__(self) -> None:
        self.contexts: list[_Context] = []
        self.closed = False

    def new_context(self) -> _Context:
        context = _Context("ephemeral")
        self.contexts.append(context)
        return context

    def close(self) -> None:
        self.closed = True


class _BrowserType:
    def __init__(self) -> None:
        self.launched: list[_Browser] = []
        self.persistent: list[_Context] = []

    def launch(self) -> _Browser:
        browser = _Browser()
        self.launched.append(browser)
        return browser

    def launch_persistent_context(self, directory: str) -> _Context:
        # The profile must start empty, as a fresh OPFS.
        assert list(Path(directory).iterdir()) == []
        (Path(directory) / "profile-file").write_text("written by the browser")
        context = _Context("persistent", directory)
        self.persistent.append(context)
        return context


class _Playwright:
    def __init__(self) -> None:
        self.webkit = _BrowserType()
        self.chromium = _BrowserType()


def test_webkit_gets_a_new_empty_profile_per_context_removed_on_close() -> None:
    profiles = _browser_profiles()
    playwright = _Playwright()
    browser = profiles.FreshBrowser(playwright, "webkit")
    assert browser.version == "1.0"
    # The browser launched for the version is closed at once.
    assert [launched.closed for launched in playwright.webkit.launched] == [True]

    first = browser.new_context()
    second = browser.new_context()
    assert (first.kind, second.kind) == ("persistent", "persistent")
    assert first.directory != second.directory
    directories = [Path(first.directory), Path(second.directory)]
    assert all(directory.is_dir() for directory in directories)

    first.close()
    browser.close()
    assert second.closed
    assert not any(directory.exists() for directory in directories)


def test_chromium_keeps_ephemeral_contexts() -> None:
    profiles = _browser_profiles()
    playwright = _Playwright()
    browser = profiles.FreshBrowser(playwright, "chromium")
    context = browser.new_context()
    assert context.kind == "ephemeral"
    assert playwright.chromium.persistent == []
    (launched,) = playwright.chromium.launched
    assert not launched.closed
    browser.close()
    assert launched.closed
