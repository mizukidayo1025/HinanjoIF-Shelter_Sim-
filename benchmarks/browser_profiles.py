"""A Playwright browser whose every context is a fresh profile with an origin private file system.

Playwright's `browser.new_context()` is an ephemeral context, like a private window. In Chromium and
Firefox it still has OPFS, but in WebKit `navigator.storage.getDirectory()` fails there with
`UnknownError` (Playwright 1.64, WebKit 27.2, #253). For WebKit each context is therefore a
persistent context on its own new, empty directory, removed when the browser is closed: the page
starts from an empty OPFS as in the other browsers, and the browser keeps it as a normal profile
does. Chromium and Firefox keep their ephemeral contexts, so their measurements stay comparable
with the earlier ones.

    browser = FreshBrowser(playwright, "webkit")
    context = browser.new_context()
    ...
    browser.close()
"""

from __future__ import annotations

import shutil
import tempfile
from pathlib import Path
from typing import Any

# The browsers whose ephemeral contexts have no OPFS.
PERSISTENT = frozenset({"webkit"})


class FreshBrowser:
    """`browser.version`, `new_context()` and `close()` of a Playwright browser by name."""

    def __init__(self, playwright: Any, name: str) -> None:
        self.name = name
        self._type = getattr(playwright, name)
        self._browser = self._type.launch()
        self.version: str = self._browser.version
        self._profiles: list[tuple[Any, Path]] = []
        if name in PERSISTENT:
            # Only the version was needed; each context launches its own browser.
            self._browser.close()
            self._browser = None

    def new_context(self) -> Any:
        if self._browser is not None:
            return self._browser.new_context()
        directory = Path(tempfile.mkdtemp(prefix=f"sheltersim-{self.name}-"))
        context = self._type.launch_persistent_context(str(directory))
        self._profiles.append((context, directory))
        return context

    def close(self) -> None:
        if self._browser is not None:
            self._browser.close()
        for context, directory in self._profiles:
            context.close()
            shutil.rmtree(directory, ignore_errors=True)
        self._profiles.clear()
