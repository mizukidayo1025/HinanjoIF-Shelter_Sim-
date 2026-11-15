"""Same-identity population lifecycle commands and retained absent-person state.

The common presence ledger records where a person is in the population lifecycle;
this module owns the simulation state needed to resume that same identity. Commands
are typed and deterministic. Natural-language compilers must map into these commands
only after validating an approved EffectProgram.

The rules are native/src/rules/population_lifecycle.rs (`kernel_only`): each docstring states its
contract. The command's contract, PopulationPresenceCommand, stays here; the kernel parses with it.
"""

from __future__ import annotations

POPULATION_LIFECYCLE_STATE_KEY = "population_lifecycle_v1"
POPULATION_LIFECYCLE_POLICY_VERSION = "same-id-population-lifecycle-v1"


# native/src/rules/population_lifecycle.rs: validate_population_lifecycle_state below, as a
# validator that says yes or Unsupported.
