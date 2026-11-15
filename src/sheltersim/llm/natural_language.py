"""Pure offline parsing of a small, explicit subset of intervention requests.

The parser creates a proposal only. It does not receive a WorldState, RNG, event queue,
branch registry, or provider, and its operation names are data rather than executable code.
"""

from __future__ import annotations

import re
import unicodedata
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    pass


def _sanitize_narrative(text: str) -> str:
    normalized = unicodedata.normalize("NFKC", text)
    normalized = "".join(char if char in "\n\t" or ord(char) >= 32 else " " for char in normalized)
    normalized = re.sub(
        r"\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b",
        "[redacted-email]",
        normalized,
        flags=re.IGNORECASE,
    )
    normalized = re.sub(
        r"(?<!\w)(?:\+?81[- ]?)?0\d{1,4}[- ]?\d{1,4}[- ]?\d{3,4}(?!\w)",
        "[redacted-phone]",
        normalized,
    )
    normalized = re.sub(r"(?<!\w)\d{10,13}(?!\w)", "[redacted-phone]", normalized)
    return normalized[:4000]
