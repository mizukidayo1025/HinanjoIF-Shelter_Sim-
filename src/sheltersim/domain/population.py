"""Dynamic arrivals, capacity-aware admission, and departure history.

The rules are native/src/rules/population.rs (`kernel_only`): each docstring states its contract.
"""

from __future__ import annotations

POPULATION_DYNAMICS_STATE_KEY = "population_dynamics_v1"
POPULATION_DYNAMICS_POLICY_VERSION = "capacity-aware-arrivals-departures-v1"


# The state with no one pending: native/core/src/rules/tick_validation.rs.
