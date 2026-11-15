"""The corpus of Scenarios the core's minute-0 builder is checked on (#429).

`fixtures/initial_corpus.json.xz` holds Scenarios recorded from the offline test suite before
#430: every Scenario that Python's `WorldState.from_scenario` built while `tests/unit` and
`tests/integration` ran, one per fingerprint of their configs, so that each installed model,
extension and config the tests used is there once. Each case holds the Scenario's canonical JSON,
the sha256 and length of the entry Python exported of its world at minute 0 with the Run's
controller, and the sha256 of a new Run's records (the model versions and the evidence route's
answer), or the class and text of the error the evidence route raised. The core built the same
bytes when they were recorded; `integration/test_initial_corpus.py` checks that it still does. The
file is xz-compressed: the Scenarios share most of their text. Nothing records it again since the
Python builder was removed (#430).
"""

from __future__ import annotations

import json
import lzma
from typing import Any

from support.paths import INITIAL_CORPUS

SCHEMA_VERSION = 3


def error_text(error: BaseException) -> str:
    return f"{type(error).__name__}: {error}"


def load_corpus() -> list[dict[str, Any]]:
    """The committed cases, in their order."""
    document = json.loads(lzma.decompress(INITIAL_CORPUS.read_bytes()))
    if document["schema_version"] != SCHEMA_VERSION:
        raise ValueError("the corpus has another schema_version")
    return document["cases"]
