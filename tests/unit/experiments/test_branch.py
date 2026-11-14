"""Typed layout Branch commands: route edits and facility relocation are atomic,
reproducible and leave the source Branch unchanged.

Typed interventions, their validation and the Branch lineage saved in a Checkpoint
are checked on the 100-person baseline.
"""

from __future__ import annotations

from typing import Any

import pytest
from support.paths import BASELINE_100_7D

from sheltersim.core.world import WorldState
from sheltersim.experiments.branch import (
    AddStaffCommand,
    AddToiletCommand,
    BranchError,
    BranchRuntime,
    RelocateFacilityCommand,
    SetFacilityWorkingUnitsCommand,
    _apply_to_world,
    command_from_dict,
)
from sheltersim.experiments.scenario import ScenarioSpec


def _layout_fork(scenario: ScenarioSpec, documents: list[dict[str, Any]]) -> Any:
    """A Branch of scenario at minute 0, and its child with the layout commands applied."""
    source = BranchRuntime.from_world(WorldState.from_scenario(scenario), branch_id="baseline")
    child = source.fork("layout")
    for document in documents:
        child.apply(command_from_dict(document))
    return source, child


def test_route_edit_branches_are_reproducible_and_leave_source_unchanged(
    small_replay_scenario, route_edit_command
) -> None:
    scenario = small_replay_scenario()
    before_hash = WorldState.from_scenario(scenario).canonical_json()

    source, child = _layout_fork(scenario, [route_edit_command()])
    _, replay = _layout_fork(scenario, [route_edit_command()])

    assert child.parent_branch_id == "baseline"
    assert replay.world.canonical_json() == child.world.canonical_json()
    assert source.world.canonical_json() == before_hash
    assert child.world.zone_edges["corridor_toilet"].travel_minutes == 8
    assert child.world.model_configs["layout_routing"]["max_agents_in_transit_by_edge"] == {
        "corridor_toilet": 1
    }


def test_invalid_route_edit_changes_neither_the_branch_nor_its_source(
    small_replay_scenario, route_edit_command
) -> None:
    source, child = _layout_fork(small_replay_scenario(), [route_edit_command()])
    source_hash, child_hash = source.world.canonical_json(), child.world.canonical_json()

    with pytest.raises(BranchError, match="unknown zone edge"):
        child.apply(command_from_dict(route_edit_command(edge_id="missing-edge")))

    assert child.world.canonical_json() == child_hash
    assert source.world.canonical_json() == source_hash


def test_route_edit_that_disconnects_a_facility_is_rejected_atomically(
    small_replay_scenario,
) -> None:
    source, child = _layout_fork(small_replay_scenario(), [])
    before_hash = child.world.canonical_json()

    with pytest.raises(BranchError, match="would be unreachable"):
        child.apply(
            command_from_dict(
                {
                    "kind": "set_zone_edge",
                    "edge_id": "corridor_toilet",
                    "travel_minutes": 2,
                    "closed": True,
                    "max_agents_in_transit": None,
                }
            )
        )

    assert child.world.canonical_json() == before_hash
    assert child.command_log == []


def test_facility_relocation_is_a_typed_branch_intervention(small_replay_scenario) -> None:
    relocation = {"kind": "relocate_facility", "facility_id": "toilet_1", "zone_id": "corridor"}
    source, child = _layout_fork(small_replay_scenario(), [relocation])

    assert source.world.facilities["toilet_1"].zone_id == "toilet"
    assert child.world.facilities["toilet_1"].zone_id == "corridor"
    assert child.command_log[-1]["command"]["kind"] == "relocate_facility"
    assert child.world.canonical_json() != source.world.canonical_json()


def test_facility_relocation_rejects_unknown_zone_without_partial_change(
    small_replay_scenario,
) -> None:
    source, child = _layout_fork(small_replay_scenario(), [])
    before_hash = child.world.canonical_json()

    with pytest.raises(BranchError, match="unknown facility zone"):
        child.apply(
            command_from_dict(
                {"kind": "relocate_facility", "facility_id": "toilet_1", "zone_id": "missing"}
            )
        )

    assert child.world.canonical_json() == before_hash
    assert source.world.facilities["toilet_1"].zone_id == "toilet"


def test_relocate_facility_command_rejects_active_facility_use(small_replay_scenario) -> None:
    world = WorldState.from_scenario(small_replay_scenario())
    world.facilities["toilet_1"].queue_agent_ids.append("A001")

    with pytest.raises(BranchError, match="active users, queues, or tasks"):
        _apply_to_world(world, RelocateFacilityCommand("toilet_1", "corridor"))


def test_typed_interventions_are_logged_and_do_not_leak_between_branches() -> None:
    parent = BranchRuntime.from_world(WorldState.from_scenario(ScenarioSpec.load(BASELINE_100_7D)))
    with_staff = parent.fork("with-staff")
    with_toilet = parent.fork("with-toilet")

    staff_receipt = with_staff.apply(AddStaffCommand("staff-extra", "sleeping"))
    toilet_receipt = with_toilet.apply(
        AddToiletCommand("toilet-extra", "toilet", capacity=2, working_units=1)
    )
    disable_receipt = with_toilet.apply(
        SetFacilityWorkingUnitsCommand("toilet-extra", working_units=0)
    )

    assert "staff-extra" in with_staff.world.staff
    assert "staff-extra" not in with_toilet.world.staff
    assert "staff-extra" not in parent.world.staff
    assert with_toilet.world.facilities["toilet-extra"].working_units == 0
    assert len(with_staff.world.event_log.records) == 1
    assert len(with_toilet.world.event_log.records) == 2
    assert staff_receipt.state_hash != toilet_receipt.state_hash
    assert toilet_receipt.event_id != disable_receipt.event_id
    assert all(
        record.event.kind.value == "intervention" and record.status.value == "applied"
        for record in (*with_staff.world.event_log.records, *with_toilet.world.event_log.records)
    )


def test_rejected_intervention_is_atomic_and_command_parser_is_strict() -> None:
    runtime = BranchRuntime.from_world(WorldState.from_scenario(ScenarioSpec.load(BASELINE_100_7D)))
    original = runtime.world.canonical_json()

    with pytest.raises(BranchError, match="already exists"):
        runtime.apply(AddStaffCommand("S01", "sleeping"))
    with pytest.raises(BranchError, match="unknown facility"):
        runtime.apply(SetFacilityWorkingUnitsCommand("missing", 0))
    with pytest.raises(BranchError, match="unsupported"):
        command_from_dict({"kind": "add_staff", "staff_id": "x", "zone_id": "sleeping", "extra": 1})

    assert runtime.world.canonical_json() == original
    assert runtime.command_log == []


def test_branch_checkpoint_restores_lineage_and_interventions() -> None:
    root = BranchRuntime.from_world(WorldState.from_scenario(ScenarioSpec.load(BASELINE_100_7D)))
    child = root.fork("staff-branch")
    child.apply(AddStaffCommand("staff-extra", "sleeping"))

    restored = BranchRuntime.from_checkpoint(child.checkpoint(), branch_id="restored")

    assert restored.parent_branch_id == "staff-branch"
    assert restored.pairing_key == child.pairing_key
    assert restored.world.canonical_json() == child.world.canonical_json()
    assert restored.command_log == child.command_log


def test_fork_is_the_child_of_the_branch_checkpoint_and_shares_nothing_with_it() -> None:
    root = BranchRuntime.from_world(WorldState.from_scenario(ScenarioSpec.load(BASELINE_100_7D)))
    root.apply(AddStaffCommand("staff-extra", "sleeping"))

    child, checkpoint = root.fork_with_checkpoint("child")

    # One Checkpoint gives both the fork's receipt and the child (#46).
    assert checkpoint.to_json() == root.checkpoint().to_json()
    expected = BranchRuntime.from_checkpoint(checkpoint, branch_id="child")
    assert child.checkpoint().to_json() == expected.checkpoint().to_json()
    child.apply(AddStaffCommand("staff-child", "sleeping"))
    child.metrics.snapshots.clear()
    assert checkpoint.to_json() == root.checkpoint().to_json()
