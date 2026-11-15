"""The offline scenarios of the all-paths gate (benchmarks/gate_all_paths.py).

Each entry builds small synthetic Scenarios from code and fixtures/, runs them in the core
(`PlainRun`, the tick of every version since #427) and returns the core's hashes of the worlds it
ended with. An intervention is an EffectProgram applied to the Run at its minute, or, for a
primitive the core does not schedule, a recorded minute-0 entry with the program scheduled ahead
(`SCHEDULED`). Nothing here calls a provider over the network: a scripted local provider writes a
response tape and the replay reads that tape.
"""

from __future__ import annotations

import copy
import hashlib
import json
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from sheltersim import native
from sheltersim.contracts.policies import model_catalog, selected_models
from sheltersim.experiments.initial_scenario import InitialScenarioBuilder, InitialScenarioRequest
from sheltersim.experiments.scenario import ScenarioSpec
from sheltersim.llm.cache import ResponseTape
from sheltersim.llm.config import LLMMode, LLMSettings
from sheltersim.llm.contracts import ProviderResponse
from sheltersim.llm.link import RouterLink
from sheltersim.llm.provider import LLMRouter
from sheltersim.storage.projects import ProjectStore, digest
from sheltersim.web.projects import ProjectAPI

REPOSITORY = Path(__file__).resolve().parents[1]
SCENARIOS = REPOSITORY / "fixtures" / "scenarios"
SOURCE_ID = "GATE-SYNTHETIC-INPUT"
PLAIN = native.rules("plain")
# The Branches whose programs the core does not schedule (it schedules the primitives a text's
# reading gives, `native/core/src/program.rs`): each the minute-0 entry of its Scenario with the
# program scheduled at minute 0 (recorded by the Python path before #430, which ended with the
# same world as the core), run to its end.
SCHEDULED = REPOSITORY / "fixtures" / "gate-scheduled-programs.json.xz"
# The policies each effect primitive the core schedules requires (the EffectProgram contract's
# `policy_dependencies`).
REQUIRED_POLICIES = {
    "add_evacuees": ["population_dynamics_v1"],
    "add_staff": ["staff_operations"],
    "add_toilet": ["facility_capacity"],
    "depart_evacuees": ["population_dynamics_v1"],
    "facility_outage": ["facility_capacity"],
    "water_outage": ["operational_events_v1", "resource_service_v1"],
}


def core_hashes(parts: dict[str, str], metrics: str) -> dict[str, str]:
    """The three hashes of a world as the core digests it: its parts' digests, its event log's,
    and its metric samples'."""
    return {
        "world_hash": hashlib.sha256(json.dumps(parts, sort_keys=True).encode()).hexdigest(),
        "events_hash": parts["event_log"],
        "metrics_hash": metrics,
    }


def hashes_of(run: Any) -> dict[str, str]:
    """The three hashes of a Run's world now."""
    return core_hashes(json.loads(run.digests_json()), run.metrics_digest())


@dataclass
class Outcome:
    """What one gate scenario ended with."""

    # One entry per world the scenario ended with, by a label stable across runs.
    runs: dict[str, dict[str, str]] = field(default_factory=dict)
    # Labels whose three hashes must be equal, each with what the equality means.
    must_match: list[tuple[str, str, str]] = field(default_factory=list)
    # Interventions the simulation refused, with the reason; part of the expected result.
    refused: dict[str, str] = field(default_factory=dict)

    def add(self, label: str, run: Any) -> None:
        self.runs[label] = hashes_of(run)


@dataclass(frozen=True)
class GateScenario:
    name: str
    what: str
    run: Callable[[Path], Outcome]


# Scenario builders -----------------------------------------------------------------------


def generated(*, evacuees: int = 30, minutes: int = 360, seed: int = 2026, **fields: Any) -> Any:
    """The Scenario the Project screen generates for a typed form (no regional research)."""
    return (
        InitialScenarioBuilder()
        .build(
            InitialScenarioRequest(
                municipality="合成市",
                disaster="地震",
                evacuees=evacuees,
                duration_days=minutes / 1440,
                seed=seed,
                **fields,
            )
        )
        .scenario
    )


def edited(scenario: Any, change: Callable[[dict[str, Any]], None]) -> ScenarioSpec:
    document = copy.deepcopy(scenario.model_dump(mode="json"))
    change(document)
    return ScenarioSpec.model_validate(document)


def fixture_short(
    name: str, *, minutes: int, agents: int, remap: dict[str, int] | None = None
) -> ScenarioSpec:
    """A file of fixtures/scenarios cut to size; its dated events are moved inside the Run."""
    document = json.loads((SCENARIOS / f"{name}.json").read_text(encoding="utf-8"))
    document["schema_version"] = 3
    document["duration_minutes"] = minutes
    document["profiles"] = document["profiles"][:agents]
    for event in document.get("scheduled_events", []):
        if remap and event["event_type"] in remap:
            event["due_minute"] = remap[event["event_type"]]
        if event["event_type"] == "delivery_delayed":
            event["payload"]["to_minute"] = minutes // 2
    document["scheduled_events"] = [
        event for event in document.get("scheduled_events", []) if event["due_minute"] < minutes
    ]
    return ScenarioSpec.model_validate(document)


def rumor_events(*, rumor_minute: int = 5, correction_minute: int = 60) -> list[dict[str, Any]]:
    """Three synthetic claims of each truth status, with staff corrections for two."""
    events: list[dict[str, Any]] = []
    for index, (truth, origin) in enumerate(
        (("false", "A001"), ("true", "A002"), ("unknown", "A003"))
    ):
        message_id = f"claim-{truth}"
        events.append(
            {
                "id": f"rumor-{truth}",
                "kind": "exogenous",
                "due_minute": rumor_minute + index,
                "priority": 0,
                "sequence": 10 + index,
                "event_type": "information_rumor",
                "payload": {
                    "message_id": message_id,
                    "truth_status": truth,
                    "origin_agent_id": origin,
                },
                "source_id": SOURCE_ID,
            }
        )
    for index, (truth, belief) in enumerate((("false", "rejected"), ("true", "verified"))):
        events.append(
            {
                "id": f"correction-{truth}",
                "kind": "exogenous",
                "due_minute": correction_minute + index,
                "priority": 0,
                "sequence": 20 + index,
                "event_type": "information_correction",
                "payload": {
                    "message_id": f"claim-{truth}",
                    "zone_id": "sleeping",
                    "belief": belief,
                },
                "source_id": SOURCE_ID,
                "causation_id": f"rumor-{truth}",
            }
        )
    return events


def with_rumors(scenario: Any, **config: Any) -> ScenarioSpec:
    def change(document: dict[str, Any]) -> None:
        document["scheduled_events"] = [*document.get("scheduled_events", []), *rumor_events()]
        document["model_configs"]["information_spread"].update(
            base_transmission_probability=0.9,
            minimum_contact_minutes=1,
            max_contacts_per_agent=6,
            rumor_belief_probability=0.9,
            forgetting_rate_per_minute=0.002,
            **config,
        )

    return edited(scenario, change)


# Runs ------------------------------------------------------------------------------------


def scenario_bytes(scenario: Any) -> bytes:
    return scenario.canonical_json().encode()


def entry_of(scenario: Any) -> bytes:
    """The minute-0 entry the core builds of a Scenario (`initial_entry`, #429)."""
    import sheltersim_native

    return bytes(sheltersim_native.initial_entry(scenario_bytes(scenario)))


def advance_to(run: Any, minute: int) -> None:
    """Run the ticks up to `minute`; a stop raises RuntimeError, caused by the tick's error."""
    if minute > run.minute():
        run.advance(minute - run.minute())


def run_core(scenario: Any) -> Any:
    """The Scenario's Run in the core, from minute 0 to its end."""
    run = PLAIN.PlainRun.from_scenario(scenario_bytes(scenario))
    advance_to(run, run.duration())
    return run


def _fork_with(
    outcome: Outcome, scenario: Any, name: str, program: dict[str, Any], end: int
) -> None:
    """A Branch of the Scenario's Run: the program applied at its first minute, then the Run to
    `end`. A refusal is recorded, as it is a result too."""
    run = PLAIN.PlainRun.from_scenario(scenario_bytes(scenario))
    advance_to(run, program["simulation_start_minute"])
    try:
        run.apply_effect_program(json.dumps(program, ensure_ascii=False).encode())
    except ValueError as exc:
        outcome.refused[name] = str(exc)
        return
    try:
        advance_to(run, end)
    except RuntimeError as exc:
        # A Branch the simulation cannot finish: pinned, so that a fix shows up in the gate.
        outcome.refused[name] = f"stopped: {exc}: {exc.__cause__}"
        return
    outcome.add(name, run)


def _scheduled(outcome: Outcome, scenario_name: str, names: dict[str, str]) -> None:
    """The recorded Branches of a scenario (`SCHEDULED`), each run from minute 0 to its end."""
    import lzma

    branches = json.loads(lzma.decompress(SCHEDULED.read_bytes()))["branches"]
    for name in names:
        row = branches[f"{scenario_name}/{name}"]
        run = PLAIN.PlainRun(
            json.dumps(
                row["entry"], sort_keys=True, ensure_ascii=False, separators=(",", ":")
            ).encode()
        )
        try:
            advance_to(run, row["end"])
        except RuntimeError as exc:
            outcome.refused[name] = f"stopped: {exc}: {exc.__cause__}"
            continue
        outcome.add(name, run)


def _action(
    op: str,
    kind: str,
    target: str | None,
    units: str,
    parameters: dict[str, Any],
    *,
    start: int,
    end: int | None = None,
    effect_id: str | None = None,
) -> dict[str, Any]:
    """A timed effect's JSON; its policy dependencies are the ones the primitive requires."""
    return {
        "effect_id": effect_id or f"{op}-1",
        "op": op,
        "target_selector": {"kind": kind, "target_id": target, "candidate_ids": []},
        "start_minute": start,
        "end_minute": end,
        "guard": None,
        "parameters": parameters,
        "units": units,
        "source_id": SOURCE_ID,
        "assumption_ids": [],
        "policy_dependencies": REQUIRED_POLICIES[op],
        "causation_id": None,
        "reversible": end is not None,
    }


def _program(name: str, *effects: dict[str, Any]) -> dict[str, Any]:
    """An EffectProgram's JSON, as the contract dumps it."""
    return {
        "schema_version": 1,
        "program_id": f"program-{name}",
        "narrative": name,
        "source_text": name,
        "simulation_start_minute": min(effect["start_minute"] for effect in effects),
        "effects": list(effects),
        "assumptions": [],
        "support_mode": "native",
        "compiler_provenance": {
            "compiler_kind": "offline_rule",
            "compiler_version": "gate-v1",
            "model_id": None,
            "prompt_hash": None,
        },
    }


def single(scenario_of: Callable[[], Any]) -> Callable[[Path], Outcome]:
    """One Run of a Scenario."""

    def run(directory: Path) -> Outcome:
        del directory
        outcome = Outcome()
        outcome.add("run", run_core(scenario_of()))
        return outcome

    return run


# The scenarios ---------------------------------------------------------------------------


def project_run(directory: Path, scenario: Any, minutes: int) -> Any:
    """The route of the screen: the Project API's worker computes `scenario` up to `minutes` as
    Project "p", Run "r", Branch "main" of a store in `directory`, into its stored Run of format
    v6 (#427, #478). The Run read back from the store at `minutes`."""
    from sheltersim.web.stored_runs import save_scenario

    store = ProjectStore(directory)
    api = ProjectAPI(store, recover=False)
    api.post([], {"project_id": "p"})
    save_scenario(api.files, "p", scenario, digest(json.loads(scenario.canonical_json())))
    api.post(
        ["p", "runs"],
        {
            "run_id": "r",
            "scenario_id": scenario.id,
            "scenario_version": 1,
            "execution_mode": "offline",
        },
    )
    for target in (minutes // 3, minutes):
        # A resume from what is stored: the second computation goes on from the first's end.
        api.submit("p", "r", "main", target)
        # Only ends a stuck run; the variants on a busy machine take long.
        api.wait_idle(timeout=7200)
    run, _, _ = api.files.reader("p", "r", "main").seek(minutes)
    api.close()
    return run


def _generated_project(directory: Path) -> Outcome:
    """The route of the screen (the Project API's worker into a stored Run, computed in two
    parts) against a Run straight through."""
    outcome = Outcome()
    scenario = generated(evacuees=100, minutes=720)
    outcome.add("main", project_run(directory / "projects", scenario, 720))
    outcome.add("plain", run_core(scenario))
    outcome.must_match.append(("main", "plain", "saving a Run does not change it"))
    return outcome


def _rumors(directory: Path) -> Outcome:
    """Rumors and corrections, on the generated Scenario and on the older Scenario form."""
    del directory
    outcome = Outcome()
    outcome.add("generated", run_core(with_rumors(generated(evacuees=20, minutes=240))))
    outcome.add("old-form", run_core(fixture_short("baseline_100_7d", minutes=180, agents=12)))
    return outcome


class ScriptedProvider:
    """A local stand-in for the paid provider: every answer is a pure function of the task.

    It picks among the actions a task allows by the hash of its input, so a Run reaches
    each kind of accepted decision.
    """

    def complete(
        self, *, model_id: str, task: Any, max_output_tokens: int, timeout_seconds: float
    ) -> ProviderResponse:
        del max_output_tokens, timeout_seconds
        return ProviderResponse(
            text=self._answer(task),
            model_id=model_id,
            input_tokens=10,
            output_tokens=5,
            elapsed_ms=0.0,
        )

    @staticmethod
    def _answer(task: Any) -> str:
        properties = task.output_schema.get("properties", {})
        pick = int(hashlib.sha256(task.input_hash.encode()).hexdigest()[:8], 16)
        if "event_id" in properties:
            actions = [item for item in properties["action"]["enum"] if item != "no_interaction"]
            reasons = [
                item for item in properties["reason_short"]["enum"] if item != "rule_fallback"
            ]
            if pick % 5 == 4:
                return task.fallback_text
            return json.dumps(
                {
                    "event_id": json.loads(task.fallback_text)["event_id"],
                    "action": actions[pick % len(actions)],
                    "reason_short": reasons[pick % len(reasons)],
                }
            )
        if "priority" in properties:
            return json.dumps(
                {"priority": pick % 3 + 1, "rationale": "synthetic scripted answer"},
                ensure_ascii=False,
            )
        if set(properties) == {"action"}:
            choices = properties["action"].get("enum") or []
            real = [choice for choice in choices if choice != "no_interaction"] or choices
            if pick % 5 == 4 or not real:
                return task.fallback_text
            return json.dumps({"action": real[pick % len(real)]})
        return task.fallback_text


def _llm_settings(mode: LLMMode) -> LLMSettings:
    return LLMSettings(
        mode=mode,
        api_key="synthetic-not-a-key" if mode is LLMMode.RECORD else None,
        max_api_calls=400,
        max_input_tokens=16_384,
        max_output_tokens=256,
        budget_usd=10.0,
    )


# The stressed Run's social limits: the daily caps opened, so each kind of decision is asked.
STRESSED_LIMITS = json.dumps(
    {
        "social": {
            "cooldown_minutes": 5,
            "per_agent_per_day": 40,
            "global_per_day": 120,
            "complaint_wait_minutes": 5,
        }
    }
).encode()


def stressed_entry(scenario: Any) -> bytes:
    """The Scenario's minute-0 entry with everyone under stress."""
    entry = json.loads(entry_of(scenario))
    for agent in entry["world"]["agents"]:
        agent["stress"] = 0.9
    return json.dumps(entry, sort_keys=True, ensure_ascii=False, separators=(",", ":")).encode()


def _linked(run: Any, router: LLMRouter, limits: bytes | None = None) -> Any:
    run.set_llm(RouterLink(router), limits)
    advance_to(run, run.duration())
    return run


def _llm_tape(directory: Path) -> Outcome:
    """Decisions a scripted provider records through the core's LLM port (#428), then the same
    Run from its tape alone: through the router's replay, and through the core's own."""
    del directory
    outcome = Outcome()
    scenario = with_rumors(generated(evacuees=30, minutes=360, staff_count=1))
    router = LLMRouter(
        _llm_settings(LLMMode.RECORD), provider=ScriptedProvider(), tape=ResponseTape()
    )
    recorded = _linked(PLAIN.PlainRun.from_scenario(scenario_bytes(scenario)), router)
    outcome.add("record", recorded)
    replay = LLMRouter(
        _llm_settings(LLMMode.REPLAY), tape=ResponseTape.from_dict(router.tape.to_dict())
    )
    outcome.add("replay", _linked(PLAIN.PlainRun.from_scenario(scenario_bytes(scenario)), replay))
    outcome.must_match.append(("record", "replay", "a Run replays from its response tape"))
    tape = list(recorded.take_tape())
    again = PLAIN.PlainRun.from_scenario(scenario_bytes(scenario))
    again.set_llm_replay(tape, False)
    advance_to(again, again.duration())
    outcome.add("core-replay", again)
    outcome.must_match.append(("record", "core-replay", "a Run replays from its stored tape"))
    # Everyone under stress and the daily caps opened, so each kind of decision applies.
    stressed = stressed_entry(with_rumors(generated(evacuees=24, minutes=240, staff_count=1)))
    router = LLMRouter(
        _llm_settings(LLMMode.RECORD), provider=ScriptedProvider(), tape=ResponseTape()
    )
    outcome.add("stressed-record", _linked(PLAIN.PlainRun(stressed), router, STRESSED_LIMITS))
    replay = LLMRouter(
        _llm_settings(LLMMode.REPLAY), tape=ResponseTape.from_dict(router.tape.to_dict())
    )
    outcome.add("stressed-replay", _linked(PLAIN.PlainRun(stressed), replay, STRESSED_LIMITS))
    outcome.must_match.append(("stressed-record", "stressed-replay", "a Run replays from its tape"))
    return outcome


def _stored(run: Any) -> dict[str, Any]:
    """What a stored Run holds up to now (`run_store`): its saved point, and its log's and
    samples' tables and blocks, sealed."""
    run.seal_blocks()
    log_blocks, _ = run.log_block_counts()
    sample_blocks, _ = run.sample_block_counts()
    return {
        "point": run.saved_point(),
        "log_tables": run.log_tables(0, 0),
        "log_blocks": [run.log_block(index) for index in range(log_blocks)],
        "sample_tables": run.sample_tables(0, 0),
        "sample_blocks": [run.sample_block(index) for index in range(sample_blocks)],
    }


def _restored(entry: bytes, stored: dict[str, Any]) -> Any:
    """The Run at a saved point, built again from what was stored."""
    restore = PLAIN.Restore(stored["point"])
    restore.read_log_tables(stored["log_tables"])
    for block in stored["log_blocks"]:
        if restore.log_needed():
            restore.read_log_block(block)
    restore.read_sample_tables(stored["sample_tables"])
    for block in stored["sample_blocks"]:
        if restore.samples_needed():
            restore.read_sample_block(block)
    return PLAIN.PlainRun.restore(entry, restore)


def _fork_resume(directory: Path) -> Outcome:
    """A Run restored at a saved point, a restore of a restored Run and a resume from the stored
    Run end as the straight Run does.

    The defined behaviour (AGENTS.md): a saved point holds everything a Run needs.
    """
    outcome = Outcome()
    end, middle = 300, 130
    scenario = with_rumors(generated(evacuees=24, minutes=end))
    entry = entry_of(scenario)
    straight = PLAIN.PlainRun(entry)
    advance_to(straight, middle)
    seen = straight.world_parts()
    advance_to(straight, end)
    outcome.add("straight", straight)
    # Restored between two saves, at a saved point and at the first minute.
    for minute in (middle, 120, 1):
        name = f"fork-{minute}"
        source = PLAIN.PlainRun(entry)
        advance_to(source, minute)
        restored = _restored(entry, _stored(source))
        if minute == middle and restored.world_parts() != seen:
            raise AssertionError("a restored minute reads differently from the straight Run")
        if minute == middle:
            advance_to(restored, 200)
            again = _restored(entry, _stored(restored))
            advance_to(again, end)
            outcome.add("fork-of-fork", again)
            outcome.must_match.append(("straight", "fork-of-fork", "a restore of a restore too"))
        advance_to(restored, end)
        outcome.add(name, restored)
        outcome.must_match.append(("straight", name, "a restored Run ends as the straight Run"))
    # A resume: the stored Run's second computation goes on from the first's end.
    outcome.add("resumed", project_run(directory / "resumed", scenario, end))
    outcome.must_match.append(("straight", "resumed", "a resume ends as the straight Run"))
    return outcome


def _operations() -> dict[str, Any]:
    """Outages, an absence and a shipment the operator entered on the form."""
    return {
        "equipment": [{"facility_id": "intake_desk", "capacity": 2, "working_units": 1}],
        "initial_resources": {
            "status": "entered",
            "food_meal_units": 120,
            "water_ml": 60_000,
            "source_kind": "operator",
        },
        "scheduled": [
            {
                "kind": "facility_outage",
                "facility_id": "toilet_01",
                "start_minute": 0,
                "end_minute": 120,
                "units_out": 1,
            },
            {
                "kind": "water_outage",
                "facility_id": "water_station_main",
                "start_minute": 30,
                "end_minute": 90,
            },
            {
                "kind": "staff_unavailable",
                "staff_id": "staff_002",
                "start_minute": 5,
                "end_minute": 30,
                "repeat_every_minutes": 60,
            },
            {
                "kind": "resource_delivery",
                "resource_id": "water_ml",
                "quantity": 100_000,
                "due_minute": 150,
                "warehouse_id": "main_warehouse",
            },
        ],
    }


def _interventions(directory: Path) -> Outcome:
    """The EffectPrograms other than the floor plan's: those the core schedules applied at a
    minute of the Run, and the others scheduled ahead at minute 0 (`SCHEDULED`)."""
    del directory
    outcome = Outcome()
    start, end = 60, 240
    scenario = generated(evacuees=30, minutes=end, operational_conditions=_operations())
    outcome.add("main", run_core(scenario))
    actions = {
        "add-staff": _action(
            "add_staff", "zone", "staff", "staff_member", {"count": 2}, start=start
        ),
        "add-toilets": _action(
            "add_toilet", "zone", "toilet", "toilet_stall", {"count": 2}, start=start
        ),
        "outage": _action(
            "facility_outage",
            "facility",
            "water_station_02",
            "facility_unit",
            {"units": 1},
            start=start,
            end=120,
        ),
        "water-outage": _action(
            "water_outage",
            "facility",
            "water_station_02",
            "water_supply",
            {"outage": True},
            start=start,
            end=120,
        ),
    }
    for name, action in actions.items():
        _fork_with(outcome, scenario, name, _program(name, action), end)
    _scheduled(
        outcome,
        "interventions",
        {
            "availability": "medical_desk's working units 0 from 60 to 120",
            "delivery": "100 l of water delivered at 60",
            "delay-shipment": "the water shipment moved to 200 at 60",
            "staff-away": "staff_003 away from 60 to 120",
            "staff-role": "staff_002's skills changed at 62",
            "noise": "noise 1.0 in sleeping from 60 to 180",
            "lighting": "lighting 1.0 in sleeping from 60 to 180",
            "ventilation": "ventilation 0.0 in sleeping from 60 to 180",
            "privacy": "privacy 0.0 in sleeping from 60 to 180",
            "temperature": "35 °C in sleeping from 60 to 180",
            "demand": "rest twice as likely and long in sleeping from 60 to 180",
            "trained": "staff_001 given every skill and a worker added at 135",
            "guard-and": "the availability under an and of four true guards",
            "guard-or-false": "the availability under an or of two false guards",
            "guard-zone": "noise 0.5 under a guard on the zone's occupancy",
            "guard-resource": "a delivery under a guard on the stock",
        },
    )
    return outcome


def _geometry(directory: Path) -> Outcome:
    """The floor-plan primitives, scheduled ahead at minute 0 (`SCHEDULED`): the overlay, and
    walks that replan around it."""
    del directory
    outcome = Outcome()
    outcome.add("main", run_core(generated(evacuees=20, minutes=300)))
    _scheduled(
        outcome,
        "geometry",
        {
            "move-facility": "toilet_01 moved at 52",
            "transfer": "toilet_01 carried to sleeping at 52",
            "partition": "a partition in the toilet zone from 60 to 150",
            "temporary-room": "a room of four beds in sleeping from 30 to 150",
            "close-route": "route_corridor_toilet closed from 30 to 95",
            "support-trip-closed": "route_corridor_staff closed from 39 to 59",
            "walk-closed": "route_corridor_sleeping closed from 172 to 187",
        },
    )
    return outcome


def _run_stops(directory: Path) -> Outcome:
    """Valid interventions that stopped the Run, listed in docs/rust-port.md.

    A stop found here is pinned as the expected result, so that a fix or another stop shows up
    in the gate. A fixed one stays as a Run, so that it doesn't come back.
    """
    del directory
    outcome = Outcome()
    _scheduled(
        outcome,
        "run_stops",
        {
            # The closure's expiry is due in the minute support requests time out, after them in
            # the batch of due events. Their care interruptions are recorded after the batch, and
            # the expiry's copy of the world used to fail its validation in between (#88, 元 #309).
            "expiry-at-a-timeout": "route_corridor_toilet closed from 30 to 90",
            # A guard on an absence with an end. The restoration event used to inherit the guard
            # and stop the Run (#89, 元 #310); now the guard is only the start's.
            "guarded-absence": "staff_003 away from 60 to 120 under a guard",
        },
    )
    return outcome


def _population(directory: Path) -> Outcome:
    """Arrivals, departures and each presence change of one person."""
    del directory
    outcome = Outcome()
    start, end = 30, 240
    scenario = generated(evacuees=30, minutes=end)
    outcome.add("main", run_core(scenario))
    programs = {
        "arrive": _action(
            "add_evacuees",
            "zone",
            "sleeping",
            "evacuee",
            {"count": 6, "household_size": 2, "mobility": "assisted"},
            start=start,
        ),
        "depart": _action(
            "depart_evacuees",
            "zone",
            "sleeping",
            "evacuee",
            {"count": 5, "include_initial": True},
            start=start,
        ),
    }
    for name, action in programs.items():
        _fork_with(outcome, scenario, name, _program(name, action), end)
    _scheduled(
        outcome,
        "population",
        {
            "arrive-space": "20 wheelchair users and a temporary space at 30",
            "exit-expected-return": "A005 out at 30, expected back at 90",
            "exit-then-return": "A005 out at 30, back at 70",
            "transfer": "A005 transferred at 30",
            "relocate": "A005 moved to quiet at 30",
            "reunite": "A005 and A012 reunited at 30",
            "exit-waiting": "A001 out at 30 while waiting for support",
        },
    )
    return outcome


def _conditions(directory: Path) -> Outcome:
    """Every kind of condition the operator can enter on the form, including infrastructure."""
    del directory
    outcome = Outcome()
    infrastructure = {
        "dependency_edges": [
            {"dependent_id": "toilet_01", "dependency_id": "panel-main"},
            {"dependent_id": "accessible_toilet_01", "dependency_id": "panel-main"},
            {"dependent_id": "panel-main", "dependency_id": "grid-main"},
        ],
        "heat_stress_per_minute": 0.001,
        "cold_stress_per_minute": 0.001,
        "source_id": SOURCE_ID,
    }
    scheduled = [
        {
            "kind": "power_outage",
            "component_id": "grid-main",
            "start_minute": 20,
            "end_minute": 70,
            "repeat_every_minutes": 150,
        },
        {
            "kind": "equipment_failure",
            "facility_id": "water_station_main",
            "units_out": 1,
            "start_minute": 40,
            "end_minute": 90,
        },
        {"kind": "extreme_heat", "zone_id": "sleeping", "start_minute": 10, "end_minute": 100},
        {"kind": "extreme_cold", "zone_id": "food", "start_minute": 110, "end_minute": 200},
        {
            "kind": "staff_unavailable",
            "staff_id": "staff_001",
            "start_minute": 15,
            "end_minute": 45,
            "repeat_every_minutes": 70,
        },
        {
            "kind": "resource_loss",
            "resource_id": "water_ml",
            "quantity": 2_000,
            "due_minute": 50,
        },
        {
            "kind": "resource_delivery",
            "resource_id": "food_meal_units",
            "quantity": 40,
            "due_minute": 120,
            "repeat_every_minutes": 120,
        },
    ]
    conditions = {
        "initial_resources": {
            "status": "entered",
            "food_meal_units": 20,
            "water_ml": 8_000,
            "source_kind": "operator",
        },
        "scheduled": scheduled,
        "infrastructure": infrastructure,
    }
    scenario = generated(evacuees=40, minutes=300, operational_conditions=conditions)
    outcome.add("run", run_core(scenario))

    # With the staff all away the queues stall: requests time out and walks are interrupted.
    def understaffed(document: dict[str, Any]) -> None:
        document["staff"] = [row for row in document["staff"] if row["id"] in ("staff_001",)]

    thin = edited(
        generated(
            evacuees=30,
            minutes=240,
            operational_conditions={
                "scheduled": [
                    {
                        "kind": "staff_unavailable",
                        "staff_id": "staff_001",
                        "start_minute": 10,
                        "end_minute": 40,
                        "repeat_every_minutes": 50,
                    }
                ]
            },
        ),
        understaffed,
    )
    outcome.add("one-worker", run_core(thin))
    return outcome


def _split_queues(directory: Path) -> Outcome:
    """Two service points with a queue each, which the generator never makes."""
    del directory
    outcome = Outcome()

    def split(document: dict[str, Any]) -> None:
        geometry = document["model_configs"]["facility_geometry"]
        facilities = {row["id"]: row for row in document["facilities"]}
        zones = {row["id"]: row["zone_id"] for row in document["facilities"]}
        for facility_id, skill in (
            ("toilet_01", "toilet_service"),
            ("water_station_main", "water_service"),
        ):
            facilities[facility_id]["capacity"] = facilities[facility_id]["working_units"] = 2
            point = geometry["facility_points"][facility_id]
            geometry["service_points"] = [
                row for row in geometry["service_points"] if row["facility_id"] != facility_id
            ]
            for side, offset in (("left", -1.0), ("right", 1.0)):
                geometry["service_points"].append(
                    {
                        "schema_version": 1,
                        "id": f"{facility_id}-{side}",
                        "facility_id": facility_id,
                        "point_m": {"x": point["x"], "y": point["y"] + offset},
                        "service_capacity": 1,
                        "queue_id": f"{facility_id}-{side}-fifo",
                        "access": {"accessibility": "accessible", "required_staff_skills": [skill]},
                    }
                )
        document["staff"].append(
            {"id": "toilet_staff_01", "zone_id": zones["toilet_01"], "skills": ["toilet_service"]}
        )

    outcome.add("run", run_core(edited(generated(evacuees=30, minutes=300), split)))
    fifo = edited(
        generated(evacuees=30, minutes=300, queue_policy_id="strict-fifo-queue-v1"), split
    )
    outcome.add("strict-fifo", run_core(fifo))
    return outcome


def _arrivals(directory: Path) -> Outcome:
    """Arrivals that walk in from outside, wait for intake and fill the alternative zones."""
    del directory
    outcome = Outcome()
    start, end = 20, 300

    def approach(document: dict[str, Any]) -> None:
        document["model_configs"]["facility_geometry"]["arrival_approach_routes"] = [
            {
                "route_id": "arrival-to-corridor",
                "destination_zone_id": "corridor",
                "points_m": _corridor_route(document),
                # Slow, so that the arrivals are still at the door when an intake is ordered.
                "walking_speed_m_per_min": 0.5,
                "source_kind": "operator",
                "source_ids": ["SRC-GATE-APPROACH-ROUTE"],
            }
        ]

    scenario = edited(generated(evacuees=30, minutes=end), approach)
    outcome.add("main", run_core(scenario))

    def arrival(effect_id: str, zone: str, at: int, **parameters: Any) -> dict[str, Any]:
        return _action(
            "add_evacuees", "zone", zone, "evacuee", parameters, start=at, effect_id=effect_id
        )

    programs = {
        "walk-in": [
            _action("add_staff", "zone", "intake", "staff_member", {"count": 5}, start=start),
            arrival(
                "walk-in",
                "corridor",
                start,
                count=4,
                household_size=2,
                approach_route_id="arrival-to-corridor",
            ),
        ],
        "later-departure": [
            arrival("in", "sleeping", start, count=6),
            _action(
                "depart_evacuees",
                "zone",
                "sleeping",
                "evacuee",
                {"count": 3},
                start=start + 40,
                effect_id="out",
            ),
        ],
    }
    for name, actions in programs.items():
        _fork_with(outcome, scenario, name, _program(name, *actions), end)
    _scheduled(
        outcome,
        "arrivals",
        {
            "overflow": "200 arrivals at the toilet zone at 20, one needing hygiene support",
            "room-arrivals": "a room of four beds at 20 and 8 arrivals at 21",
        },
    )
    return outcome


def _corridor_route(document: dict[str, Any]) -> list[dict[str, float]]:
    """From outside the plan, which grows to hold it, to the middle of the corridor."""
    geometry = document["model_configs"]["facility_geometry"]
    corridor = next(row for row in geometry["zones"] if row["zone_id"] == "corridor")
    points = corridor["polygon"]
    middle = {
        "x": sum(point["x"] for point in points) / len(points),
        "y": sum(point["y"] for point in points) / len(points),
    }
    outside = float(geometry["height_m"]) + 5.0
    geometry["height_m"] = outside + 5.0
    return [{"x": middle["x"], "y": outside}, middle]


def _scarcity(directory: Path) -> Outcome:
    """Stock that runs out, so services stall, and care needs that the default form lacks."""
    del directory
    outcome = Outcome()
    empty = generated(
        evacuees=24,
        minutes=480,
        operational_conditions={
            "initial_resources": {
                "status": "entered",
                "food_meal_units": 4,
                "water_ml": 1_500,
                "source_kind": "operator",
            },
        },
    )
    outcome.add("empty-stock", run_core(empty))
    care = generated(
        evacuees=30,
        minutes=480,
        staff_count=2,
        population_conditions={
            "medical_support_rate": 0.4,
            "medication_support_rate": 0.4,
            "care_support_need_rates": {
                "caregiver_assistance": 0.4,
                "privacy_accommodation": 0.4,
                "mobility_assistance": 0.4,
                "hygiene_support": 0.4,
                "emotional_support": 0.4,
                "language_access": 0.4,
                "dependent_assistance": 0.4,
            },
            "pet_household_rate": 0.3,
            "daily_rhythms": [
                {
                    "rhythm_id": "night-shift",
                    "sleep_start_minute": 480,
                    "wake_minute": 900,
                    "weight": 1.0,
                },
                {
                    "rhythm_id": "day",
                    "sleep_start_minute": 1320,
                    "wake_minute": 360,
                    "weight": 2.0,
                },
            ],
            "source_ids": [SOURCE_ID],
        },
    )
    outcome.add("care-needs", run_core(care))
    return outcome


def _operational(directory: Path) -> Outcome:
    """Stock, deliveries and outages from the fixture files, with their dated events moved."""
    del directory
    outcome = Outcome()
    moved = {
        "resource_delivery": 20,
        "delivery_delayed": 10,
        "water_outage": 60,
        "water_restored": 80,
        "facility_outage": 40,
        "facility_restored": 100,
    }
    for name in (
        "multi_operational",
        "water_intervention",
        "failure_day3",
        "delivery_intervention",
        "research_shortage_100_7d",
    ):
        outcome.add(name, run_core(fixture_short(name, minutes=200, agents=20, remap=moved)))
    return outcome


def _long_generated(directory: Path) -> Outcome:
    """Two days of a few people: rhythms, shifts and supplies that only a day boundary reaches."""
    del directory
    outcome = Outcome()
    scenario = generated(
        evacuees=16,
        minutes=2880,
        staff_count=3,
        default_supply_start_day=2,
        operational_conditions={
            "initial_resources": {
                "status": "entered",
                "food_meal_units": 30,
                "water_ml": 30_000,
                "source_kind": "operator",
            },
        },
    )
    outcome.add("run", run_core(scenario))
    return outcome


_LEGACY_PSYCHOLOGY = {
    "natural_recovery_rate_per_minute": 0.01,
    "chronic_retention": 0.9,
    "chronic_acute_weight": 0.08,
    "chronic_high_distress_weight": 0.02,
    "high_distress_threshold": 0.7,
}


def _small_legacy(registry: dict[str, str], *, psychology: dict[str, Any] | None = None) -> Any:
    def change(document: dict[str, Any]) -> None:
        document["scheduled_events"] = rumor_events(rumor_minute=2, correction_minute=30)
        for event in document["scheduled_events"]:
            event["payload"].pop("zone_id", None)
            if event["event_type"] == "information_correction":
                event["payload"]["zone_id"] = document["profiles"][0]["initial_zone_id"]
        configs = document["model_configs"]
        configs["policy_registry"] = {"schema_version": 1, **registry}
        if psychology is not None:
            configs["psychology"] = psychology
        configs["information_spread"] = {
            "enabled": True,
            "max_contacts_per_agent": 6,
            "minimum_contact_minutes": 1,
            "contact_duration_scale_minutes": 1,
            "base_transmission_probability": 0.9,
            "correction_transmission_multiplier": 1.0,
            "trust_sensitivity": 0.2,
            "family_contact_multiplier": 1.0,
            "adjacent_zone_contact_multiplier": 1.0,
            "query_pressure_per_claim": 0.5,
            "query_pressure_decay_per_minute": 0.0,
            "query_pressure_relief_per_correction": 0.5,
            "query_threshold": 0.5,
            "query_utility_weight": 4.0,
        }

    return edited(fixture_short("baseline_100_7d", minutes=240, agents=20), change)


# The older Scenario form is the only one that can select these models; the generated form
# requires a life-state psychology.
_MODELS_ON_THE_OLD_FORM: dict[tuple[str, str], Callable[[], Any]] = {
    ("psychology", "causal-stress-v1"): lambda: _small_legacy({"psychology": "causal-stress-v1"}),
    ("psychology", "bounded-stress-v1"): lambda: _small_legacy({"psychology": "bounded-stress-v1"}),
    ("psychology", "mean-reverting-distress-v1"): lambda: _small_legacy(
        {"psychology": "mean-reverting-distress-v1"}, psychology=_LEGACY_PSYCHOLOGY
    ),
    ("psychology", "life-state-feedback-v1"): lambda: _small_legacy(
        {"psychology": "life-state-feedback-v1"}
    ),
}

# The form field that selects a model of the domain on the generated Scenario.
_REQUEST_FIELDS = {
    "behavior": "behavior_policy_id",
    "social": "social_policy_id",
    "information": "information_policy_id",
    "queue": "queue_policy_id",
    "relationship": "relationship_policy_id",
    "social_proximity": "social_proximity_policy_id",
    "body_dynamics": "body_dynamics_policy_id",
    "staff_operations": "staff_operations_policy_id",
    "staff_fatigue": "staff_fatigue_policy_id",
    "zone_environment": "zone_temperature_policy_id",
    "zone_rest": "zone_rest_policy_id",
    "zone_hygiene": "zone_hygiene_policy_id",
    "zone_privacy": "zone_privacy_policy_id",
    "care_outcome": "care_outcome_policy_id",
}


def _model_scenario(domain: str, model_id: str) -> Any:
    """A Scenario that selects the model; a silent fallback would verify nothing."""
    scenario = _scenario_selecting(domain, model_id)
    selected = selected_models(scenario.model_configs).get(domain)
    if selected != model_id:
        raise AssertionError(f"{domain} selected {selected!r}, not {model_id!r}")
    return scenario


def _scenario_selecting(domain: str, model_id: str) -> Any:
    legacy = _MODELS_ON_THE_OLD_FORM.get((domain, model_id))
    if legacy is not None:
        return legacy()
    base: dict[str, Any] = {"evacuees": 20, "minutes": 180}
    if domain in _REQUEST_FIELDS:
        return with_rumors(generated(**base, **{_REQUEST_FIELDS[domain]: model_id}))

    def change(document: dict[str, Any]) -> None:
        document["model_configs"]["policy_registry"][domain] = model_id
        # A suite fixes its members, and these selections replace a member.
        document["model_configs"]["policy_registry"].pop("suite_id", None)

    return edited(with_rumors(generated(**base)), change)


def selectable_models() -> list[tuple[str, str]]:
    """Every (domain, model) of the Model Catalog, in a stable order."""
    return sorted((row["domain"], model_id) for model_id, row in model_catalog().items())


def all_scenarios() -> dict[str, GateScenario]:
    scenarios = [
        GateScenario(
            "generated_project",
            "100人の生成Scenarioを画面の経路で12時間（2回に分けた計算と保存）",
            _generated_project,
        ),
        GateScenario(
            "long_generated", "少人数の2日間（日付をまたぐ処理、配給、シフト）", _long_generated
        ),
        GateScenario("rumors", "噂と訂正（新旧のScenarioの形）", _rumors),
        GateScenario(
            "fork_resume", "保存した点からの再開・再開の再開・保存したRunの続き", _fork_resume
        ),
        GateScenario("llm_tape", "記録した応答（台本のProvider）とその再生", _llm_tape),
        GateScenario(
            "interventions",
            "床図以外のEffectProgramの操作（途中で適用するものと、0分に予定したもの）",
            _interventions,
        ),
        GateScenario("geometry", "床図を変える操作と迂回（0分に予定したもの）", _geometry),
        GateScenario("population", "人の出入りとpopulation_presence", _population),
        GateScenario(
            "operational", "fixtureの運営Scenario（在庫・配送・断水・故障）", _operational
        ),
        GateScenario("conditions", "入力できるすべての運営条件とインフラの依存", _conditions),
        GateScenario("split_queues", "1つの設備に複数の列", _split_queues),
        GateScenario("arrivals", "外から歩いて来る到着と受け入れ", _arrivals),
        GateScenario("scarcity", "在庫切れとケアの必要", _scarcity),
        GateScenario(
            "run_stops",
            "Runを止めた有効な介入（直したものは再発の確認）",
            _run_stops,
        ),
    ]
    for domain, model_id in selectable_models():
        scenarios.append(
            GateScenario(
                f"model__{domain}__{model_id}",
                f"{domain}のモデル{model_id}",
                # Bound by default arguments: the loop variables change.
                single(lambda domain=domain, model_id=model_id: _model_scenario(domain, model_id)),
            )
        )
    return {scenario.name: scenario for scenario in scenarios}
