"""Checkpoint-safe, event-backed lifecycle receipts for autonomous activities.

Lifecycle records are synthetic state-transition evidence. They contain no generated
utterance and are separate from selected-policy proposals and completion events.

The four rules below are native/src/rules/action_lifecycle.rs's kernels (`kernel_only`). They keep
the open actions in the store `active_action_store` makes (the Rust rows); a plain dict is not one,
and a kernel raises TypeError for it. A receipt the kernels can't write as the receipts' model would
(`ActionLifecycleReceipt`: a reason past 120 characters, an ID that is not stable, a value that is
not finite, a transition before its start, an event ID or sequence the log already holds) is
refused before anything changes: KernelRefusal.
"""

from __future__ import annotations

ACTION_LIFECYCLE_POLICY_VERSION = "applied-action-lifecycle-v2"
