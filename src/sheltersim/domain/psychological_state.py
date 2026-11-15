"""Checkpointable day-scale latent distress state for the research psychology model.

The update phase's `_update_long_term_psychology` (native/core/src/rules/psychology.rs) adds each
minute's final acute distress, after social influence, to a resident's day. At the last minute of
a day the long-term distress D becomes `baseline + retention (D - baseline) + acute_weight
(mean acute - baseline) + high_distress_weight (share of high minutes)`, within 0..1.
"""

from __future__ import annotations

INTEGRATED_PSYCHOLOGY_POLICY_VERSION = "mean-reverting-life-state-v1"
LIFE_STATE_PSYCHOLOGY_POLICIES = frozenset(
    {"life-state-feedback-v1", INTEGRATED_PSYCHOLOGY_POLICY_VERSION}
)


# Rust holds this namespace in native/src/state/psychological_distress.rs.
