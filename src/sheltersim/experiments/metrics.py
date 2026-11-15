"""Pure per-minute KPI snapshots for reproducible run inspection."""

from __future__ import annotations

from dataclasses import asdict, dataclass
from typing import Any


@dataclass(frozen=True)
class MetricSnapshot:
    """Aggregated metrics captured without mutating simulation state."""

    minute_index: int
    toilet_queue_length: int
    toilet_active_users: int
    toilet_working_units: int
    toilet_wait_count: float
    toilet_wait_total_minutes: float
    toilet_wait_mean_minutes: float
    toilet_wait_max_minutes: float
    stress_mean: float
    stress_high_count: int
    staff_assigned_task_count: int
    staff_busy_count: int
    staff_load_ratio: float
    unresolved_support_request_count: int
    source_ids_json: str
    support_no_qualified_worker_count: int = 0
    support_no_qualified_worker_request_minutes: float = 0.0
    support_fairness_promoted_assignment_count: float = 0.0
    support_wait_staff_busy_request_minutes: float = 0.0
    support_wait_staff_off_shift_request_minutes: float = 0.0

    def csv_row(self) -> dict[str, Any]:
        """Return a detached row with stable CSV column names."""
        return asdict(self)
