"""Deterministic toilet requests, FIFO queues, and capacity arbitration."""

from __future__ import annotations

from typing import TYPE_CHECKING

TOILET_SERVICE_POLICY_VERSION = "toilet-service-uniform-v1"

if TYPE_CHECKING:
    pass
