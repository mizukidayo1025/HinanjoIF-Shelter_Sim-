"""Typed event and realized-event records."""

from __future__ import annotations

from enum import StrEnum


class EventKind(StrEnum):
    """Origin category for a scheduled event."""

    EXOGENOUS = "exogenous"
    COMPLETION = "completion"
    ENDOGENOUS = "endogenous"
    INTERVENTION = "intervention"
