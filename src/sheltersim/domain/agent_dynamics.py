"""Checkpoint-safe autonomy memory and facility awareness for synthetic Agents.

The rules are native/src/rules/agent_dynamics.rs's kernels (`kernel_only`); the validation of a
saved namespace stays here. A kernel refuses (KernelRefusal) a namespace its row can't hold or a
`facility_awareness` config the Scenario's model would refuse or coerce, before it changes anything.
"""

from __future__ import annotations

from typing import Any

AGENT_DYNAMICS_STATE_KEY = "agent_dynamics_v1"
# v3 keeps the v2 intake orientation and also announces shared facilities that
# are installed during a Run to the Agents already present.
AGENT_DYNAMICS_POLICY_VERSION = "need-led-autonomy-memory-v1"
ORIENTATION_AGENT_DYNAMICS_POLICY_VERSION = "need-led-autonomy-memory-v2"
# v3 keeps the v2 intake orientation and also announces shared facilities that
# are installed during a Run to the Agents already present.
ANNOUNCED_FACILITY_AGENT_DYNAMICS_POLICY_VERSION = "need-led-autonomy-memory-v3"
_ORIENTED_POLICY_VERSIONS = frozenset(
    {ORIENTATION_AGENT_DYNAMICS_POLICY_VERSION, ANNOUNCED_FACILITY_AGENT_DYNAMICS_POLICY_VERSION}
)
ORIENTATION_FACILITY_SOURCE_ID = "ASM-INTAKE-FACILITY-ORIENTATION-V1"
INSTALLED_FACILITY_ANNOUNCEMENT_SOURCE_ID = "ASM-INSTALLED-FACILITY-ANNOUNCEMENT-V1"
# Shared services every evacuee is assumed to be shown at intake. Group-specific
# spaces (pet, family, privacy) are still learned only on arrival.
ORIENTATION_FACILITY_KINDS = frozenset(
    {
        "intake",
        "meal_distribution",
        "medical_support",
        "quiet_space",
        "toilet",
        "water_station",
    }
)


def canonical_agent_dynamics_config(raw: Any) -> dict[str, Any]:
    """Select the facility-awareness version; a missing config keeps v1 histories exact.

    None is `{}`. A new dict: `{"schema_version": 1, "policy_version": v1}`, or for v2 and v3 also
    `"source_id": ORIENTATION_FACILITY_SOURCE_ID`. ValueError, in this order: "agent_dynamics
    config has unsupported fields" (not a dict, or a key beside schema_version, policy_version and
    source_id); "agent_dynamics config has an unsupported version" (a schema that is not `== 1`, or
    a policy that is not one of the three); "agent_dynamics v1 has no orientation source" (v1 with
    a source_id); "agent_dynamics v2/v3 must cite the intake orientation assumption".
    """
    if raw is None:
        raw = {}
    if not isinstance(raw, dict) or set(raw) - {"schema_version", "policy_version", "source_id"}:
        raise ValueError("agent_dynamics config has unsupported fields")
    policy_version = raw.get("policy_version", AGENT_DYNAMICS_POLICY_VERSION)
    if raw.get("schema_version", 1) != 1 or policy_version not in {
        AGENT_DYNAMICS_POLICY_VERSION,
        *_ORIENTED_POLICY_VERSIONS,
    }:
        raise ValueError("agent_dynamics config has an unsupported version")
    if policy_version == AGENT_DYNAMICS_POLICY_VERSION:
        if "source_id" in raw:
            raise ValueError("agent_dynamics v1 has no orientation source")
        return {"schema_version": 1, "policy_version": policy_version}
    if raw.get("source_id", ORIENTATION_FACILITY_SOURCE_ID) != ORIENTATION_FACILITY_SOURCE_ID:
        raise ValueError("agent_dynamics v2/v3 must cite the intake orientation assumption")
    return {
        "schema_version": 1,
        "policy_version": policy_version,
        "source_id": ORIENTATION_FACILITY_SOURCE_ID,
    }
