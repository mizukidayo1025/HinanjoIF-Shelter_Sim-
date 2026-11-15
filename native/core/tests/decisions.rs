//! The decisions of lane P8 a5 (`rules::behavior`, `rules::decisions`) over a world built here.
//!
//! The world is the one of `psychology.rs` at the last minute of a day, with everyone idle and due,
//! a third person in the hall and two zone demand modifiers. `data/decisions.json` holds, for three
//! policies, each proposal's action, target, next decision, draw and probabilities (as
//! `float.hex`), its candidates and the draw's key. It also holds the candidates' trigger texts and
//! each decision's indexed record, which the core no longer writes (#556). The values were
//! recorded before #430 and pin the core's output now.

use std::sync::Arc;

use indexmap::IndexMap;
use serde_json::{json, Value};
use sheltersim_core::cell::Shard;
use sheltersim_core::config::{Fault, FaultClass, Selection};
use sheltersim_core::entities::{Activity, AgentRow, FacilityRow, Mobility, NeedRow, ProfileRow, RequestRow, RequestStatus, Table, TaskRow, ZoneRow};
use sheltersim_core::error::Error;
use sheltersim_core::events::log::EventLog;
use sheltersim_core::events::queue::Queue;
use sheltersim_core::row::{IdMap, Row, ValueRow};
use sheltersim_core::rules::action_lifecycle::ActiveActions;
use sheltersim_core::rules::behavior::{self as behavior, Got, Proposal};
use sheltersim_core::rules::decisions::{self as decisions, Recording, LIFECYCLE, MEMORY};
use sheltersim_core::rules::psychology::Bad;
use sheltersim_core::rules::world_view::{self as view, Inputs, ViewCache, ViewData};
use sheltersim_core::run::behavior::{Behavior, OTHER_POLICY, OTHER_RUNNER, OTHER_SCORER};
use sheltersim_core::run::held::{Class, Held, Obj};
use sheltersim_core::state::body_state::{BodyRecord, BodyRow, History, Relationships};
use sheltersim_core::state::information::{BeliefRow, CountersRow, InformationRow, MessageRow};
use sheltersim_core::state::metric_state::MetricRow;
use sheltersim_core::state::psychological_distress::{AgentRecord, DistressRow};
use sheltersim_core::value::V;
use sheltersim_core::world::{Fit, Place, World, WorldCell, ID_COUNTERS, METRIC_STATE, MODEL_CONFIGS};

const MINUTE: i64 = 1439;
/// The seed of the world's draws: 7.
const SEED: &str = "7";
const MODULE: &str = "sheltersim.policies.behavior";

fn expected() -> Value {
    let raw = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/decisions.json")).unwrap();
    serde_json::from_str(&raw).unwrap()
}

fn text(value: &str) -> Arc<str> {
    Arc::from(value)
}

fn v(value: &Value) -> V {
    match value {
        Value::Null => V::Null,
        Value::Bool(flag) => V::Bool(*flag),
        Value::Number(number) => match number.as_i64() {
            Some(int) if !number.to_string().contains(['.', 'e', 'E']) => V::Int(int),
            _ => V::Float(number.as_f64().expect("a float")),
        },
        Value::String(text) => V::Str(Arc::from(text.as_str())),
        Value::Array(items) => V::List(items.iter().map(v).collect()),
        Value::Object(map) => V::Map(map.iter().map(|(key, item)| (Arc::from(key.as_str()), v(item))).collect::<IndexMap<_, _>>()),
    }
}

/// `float.hex` of CPython.
fn hex(value: f64) -> String {
    let sign = if value.is_sign_negative() { "-" } else { "" };
    if value == 0.0 {
        return format!("{sign}0x0.0p+0");
    }
    let bits = value.to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i64;
    let fraction = bits & ((1 << 52) - 1);
    let (lead, exponent) = if exponent == 0 { (0, -1022) } else { (1, exponent - 1023) };
    format!("{sign}0x{lead}.{fraction:013x}p{exponent:+}")
}

// ---- The world -----------------------------------------------------------------------------------

fn put<R: Clone>(table: &mut Table<R>, id: &str, row: R) {
    table.insert(text(id), row);
}

fn add_row<T: Row>(world: &mut World, name: &str, place: Place, row: T) {
    let index = world.slots.add(text(name), place).unwrap();
    world.slots.at(index).unwrap().try_borrow_mut().unwrap().row = Some(Box::new(row));
}

fn agent(id: &str, zone: &str, group: Option<&str>, stress: f64, needs: [f64; 4], mobility: Mobility) -> AgentRow {
    let [toilet_urgency, hunger, fatigue, hydration_need] = needs;
    AgentRow {
        profile: ProfileRow {
            id: text(id),
            group_id: group.map(text),
            initial_zone_id: text(zone),
            mobility,
            support_need: 0.25,
            stress_sensitivity: 1.5,
            rng_key: text(&format!("key-{id}")),
            initial_needs: NeedRow { toilet_urgency: 0.0, hunger: 0.0, fatigue: 0.0, hydration_need: 0.0 },
            provenance: V::Map(IndexMap::new()),
        },
        zone_id: text(zone),
        activity: Activity::Idle,
        target_id: None,
        needs: NeedRow { toilet_urgency, hunger, fatigue, hydration_need },
        stress,
        trust: 0.5,
        next_decision_at: 0,
        support_request_id: None,
        support_cooldown_until: 0,
    }
}

fn facility(id: &str, kind: &str, zone: &str, active: &[&str], queue: &[&str]) -> FacilityRow {
    FacilityRow {
        id: text(id),
        kind: text(kind),
        zone_id: text(zone),
        capacity: 2,
        working_units: 2,
        active_agent_ids: active.iter().map(|id| text(id)).collect(),
        queue_agent_ids: queue.iter().map(|id| text(id)).collect(),
        resource_active_agent_ids: Vec::new(),
        resource_queue_agent_ids: Vec::new(),
    }
}

fn zone(id: &str, name: &str, capacity: Option<i64>) -> ZoneRow {
    ZoneRow { id: text(id), name: text(name), capacity, display_x: None, display_y: None }
}

fn add_value(world: &mut World, name: &str, place: Place, value: Value) {
    add_row(world, name, place, ValueRow(v(&value)));
}

fn information() -> InformationRow {
    let mut messages: IdMap<MessageRow> = IdMap::default();
    messages.insert(
        text("m1"),
        MessageRow {
            id: text("m1"),
            truth_status: text("false"),
            source_id: text("rumor"),
            zone_id: text("hall"),
            time: 3,
            origin_event_id: text("e-m1"),
            origin_agent_id: text("A1"),
        },
    );
    let mut believed: IdMap<BeliefRow> = IdMap::default();
    believed.insert(
        text("m1"),
        BeliefRow {
            message_id: text("m1"),
            belief: text("heard"),
            source: text("A1"),
            last_contact: 4,
            last_event_id: text("e-b1"),
            exposed: true,
            belief_strength: 0.75,
            share_propensity: 0.5,
            correction_exposed: false,
            correction_accepted: false,
            ever_misbelieved: true,
            ever_forgotten: false,
            last_correction_accepted: false,
        },
    );
    let mut beliefs: IdMap<IdMap<BeliefRow>> = IdMap::default();
    beliefs.insert(text("A1"), IdMap::default());
    beliefs.insert(text("A2"), believed);
    beliefs.insert(text("A3"), IdMap::default());
    let mut query_pressure: IdMap<f64> = IdMap::default();
    query_pressure.insert(text("A1"), 0.125);
    query_pressure.insert(text("A2"), 0.5);
    query_pressure.insert(text("A3"), 0.75);
    InformationRow {
        schema_version: 2,
        messages,
        beliefs,
        contacts: IdMap::default(),
        query_pressure,
        support_requests: Vec::new(),
        counters: CountersRow { contact_attempts: 0, contact_deliveries: 0, correction_deliveries: 0, support_requests: 0 },
    }
}

fn body_record(values: [f64; 7]) -> BodyRecord {
    let [sleep_debt_minutes, uncertainty, reassurance, discontent, positive_affect, negative_affect, hygiene_need] = values;
    BodyRecord { sleep_debt_minutes, uncertainty, reassurance, discontent, positive_affect, negative_affect, hygiene_need }
}

/// The world of `psychology.rs` (lane a3), with A1 idle, A3 in the hall and two demand modifiers.
fn world() -> World {
    let mut world = World::default();
    {
        let scalars = world.scalars.get_mut();
        scalars.minute_index = Fit::Ok(MINUTE);
        scalars.schema_version = Fit::Ok(6);
    }
    {
        let rows = world.entities.get_mut();
        let mut a2 = agent("A2", "hall", Some("g1"), 0.5, [0.1, 0.2, 0.3, 0.4], Mobility::Independent);
        a2.support_request_id = Some(text("r1"));
        a2.support_cooldown_until = 40;
        put(&mut rows.agents, "A2", a2);
        let mut a1 = agent("A1", "room", None, 0.0, [0.0, 0.5, 1.0, 0.0], Mobility::Wheelchair);
        a1.next_decision_at = 31;
        put(&mut rows.agents, "A1", a1);
        put(&mut rows.agents, "A3", agent("A3", "hall", Some("g1"), 0.875, [0.75, 0.9, 0.1, 0.85], Mobility::Independent));
        put(&mut rows.zones, "room", zone("room", "Room", Some(5)));
        put(&mut rows.zones, "hall", zone("hall", "Hall", None));
        put(&mut rows.facilities, "f-water", facility("f-water", "Water_Station", "hall", &["A2"], &[]));
        put(&mut rows.facilities, "f-toilet", facility("f-toilet", "toilet", "room", &[], &["A1"]));
        put(
            &mut rows.support_requests,
            "r1",
            RequestRow {
                id: text("r1"),
                agent_id: text("A2"),
                requested_minute: 20,
                timeout_minute: 90,
                status: RequestStatus::Pending,
                assigned_staff_id: None,
                task_id: None,
                resolved_minute: None,
                cooldown_until: 0,
                last_assignment_attempt_minute: None,
                last_assignment_wait_reason_code: None,
            },
        );
        put(
            &mut rows.active_tasks,
            "t1",
            TaskRow {
                id: text("t1"),
                kind: text("toilet_queue"),
                agent_id: Some(text("A1")),
                resource_id: Some(text("f-toilet")),
                start_minute: 25,
                due_minute: 60,
                staff_id: None,
            },
        );
    }
    add_value(
        &mut world,
        MODEL_CONFIGS,
        Place::Field,
        json!({
            "resource_service": {"a": 1.5, "b": [1, 2.5, "x"]},
            "information_spread": {"query_pressure_per_claim": 0.25},
            "policy_registry": {"schema_version": 2, "psychology": "life-state-feedback-v1", "zone_rest": "zone-rest-limiting-v1"},
            "zone_environment": {
                "policy_version": "zone-environment-v2",
                "temperature_stress_per_degree_minute": 0.001,
                "baseline_by_zone": {"hall": {"temperature_c": 30.0}}
            },
            "population_context": {"people": {"A1": {"roles": ["parent"], "usual_wake_minute": 420, "care_for_ids": ["A2"]}}}
        }),
    );
    add_row(&mut world, "information_spread_v1", Place::Namespace, information());
    let mut distress: IdMap<AgentRecord> = IdMap::default();
    distress.insert(
        text("A2"),
        AgentRecord {
            baseline_distress: 0.125,
            long_term_distress: 0.25,
            daily_acute_sum: 2.5,
            daily_high_distress_minutes: 2,
            daily_observed_minutes: 5,
            completed_days: 3,
        },
    );
    add_row(&mut world, "psychological_distress_v1", Place::Namespace, DistressRow { schema_version: 1, last_updated_minute: MINUTE - 1, agents: distress });
    let mut bodies: IdMap<BodyRecord> = IdMap::default();
    bodies.insert(text("A1"), body_record([120.0, 0.25, 0.125, 0.5, 0.375, 0.25, 0.5]));
    bodies.insert(text("A2"), body_record([30.5, 0.75, 0.0, 0.125, 0.625, 0.5, 0.0625]));
    add_row(
        &mut world,
        "body_state_v1",
        Place::Namespace,
        BodyRow {
            schema_version: 1,
            policy_version: text("life-state-feedback-v1"),
            source_id: text("ASM-LIFE-STATE-FEEDBACK-V1"),
            config: V::Map(IndexMap::new()),
            agents: bodies,
            relationships: Relationships::from_map(IdMap::default()),
            history: History::default(),
        },
    );
    let mut metrics: MetricRow = IdMap::default();
    metrics.insert(text("acute_distress_monitor:A2:current_run"), 3.0);
    metrics.insert(text("acute_distress_monitor:A2:longest_run"), 4.0);
    metrics.insert(text("acute_distress_monitor:A2:episodes"), 1.0);
    add_row(&mut world, METRIC_STATE, Place::Field, metrics);
    add_value(&mut world, "population_presence_v1", Place::Namespace, json!({"people": {"A1": {"household_id": "h1"}}}));
    add_value(
        &mut world,
        "infrastructure_v1",
        Place::Value,
        json!({
            "environmental_stressors": {
                "s2": {"active": true, "zone_id": "hall", "stress_per_minute": 0.1, "start_event_id": "e-s2", "source_id": "src-2"},
                "s1": {"active": true, "zone_id": "hall", "stress_per_minute": 0.2, "start_event_id": "e-s1", "source_id": "src-1"},
                "s3": {"active": false, "zone_id": "room", "stress_per_minute": 9.0, "start_event_id": "e-s3", "source_id": null}
            },
            "outages": {"o1": {"active": true, "affected_facility_ids": ["f-toilet"], "start_event_id": "e-o1"}}
        }),
    );
    add_value(
        &mut world,
        "operational_events_v1",
        Place::Value,
        json!({"water_outages": {"w1": {"outage_event_id": "e-w1", "active": true, "target_kind": "zone", "target_id": "hall"}}}),
    );
    add_value(
        &mut world,
        "zone_environment_v1",
        Place::Value,
        json!({"schema_version": 1, "modifiers": {"z1": {
            "zone_id": "room", "op": "set_lighting", "parameters": {"level": 0.25}, "start_minute": 0,
            "start_event_id": "e-z1", "source_id": "src-z", "assumption_ids": ["asm-z"]},
          "z2": {"zone_id": "room", "op": "modify_demand_rate",
            "parameters": {"action": "rest", "probability_multiplier": 1.5, "duration_multiplier": 1.5},
            "start_minute": 0, "start_event_id": "e-z2", "source_id": "src-z2", "assumption_ids": ["asm-z2"]},
          "z3": {"zone_id": "hall", "op": "modify_demand_rate",
            "parameters": {"action": "request_toilet", "probability_multiplier": 0.5, "duration_multiplier": 1.0},
            "start_minute": 0, "start_event_id": "e-z3", "source_id": "src-z3", "assumption_ids": []}}}),
    );
    world
}

fn view_of(world: &World) -> ViewData {
    let selection = Ok(Arc::new(Selection::new(vec![(text("psychology"), text("life-state-feedback-v1"))])));
    let inputs = Inputs { selection: &selection, configs: None };
    view::build(world, &inputs, &mut ViewCache::default()).unwrap().finish(false)
}

// ---- The policies, as the Run reads them ---------------------------------------------------------

fn object(module: &str, name: &str, attrs: Vec<(&str, Result<Held, Fault>)>) -> Arc<Obj> {
    let mut found = Obj::new(Class::named(module, name));
    for (key, held) in attrs {
        found.attrs.insert(text(key), held);
    }
    Arc::new(found)
}

fn value(value: V) -> Result<Held, Fault> {
    Ok(Held::Value(value))
}

/// The fields of the utility scorer, with `more` changed, in the record's order.
fn scorer_fields(more: &[(&str, V)]) -> Vec<(&'static str, Result<Held, Fault>)> {
    let mut fields: Vec<(&'static str, V)> = vec![
        ("temperature", V::Float(0.35)),
        ("decision_interval_minutes", V::Int(15)),
        ("decision_interval_minutes_by_agent", v(&json!({"A3": 20}))),
        ("temperature_by_agent", v(&json!({"A2": 0.5}))),
        ("sleep_debt_utility_weight", V::Float(1.0)),
        ("policy_version", V::Str(text("utility-softmax-v1"))),
        ("resource_service_enabled", V::Bool(true)),
        ("hydration_action_threshold", V::Float(0.25)),
        ("hunger_action_threshold", V::Float(0.7)),
        ("water_utility_base", V::Float(0.18)),
        ("water_utility_need_weight", V::Float(1.0)),
        ("meal_utility_base", V::Float(0.18)),
        ("meal_utility_need_weight", V::Float(0.85)),
        ("information_query_threshold", V::Float(0.4)),
        ("information_query_utility_weight", V::Float(0.3)),
        ("autonomy_enabled", V::Bool(true)),
    ];
    for (name, changed) in more {
        fields.iter_mut().find(|(field, _)| field == name).unwrap().1 = changed.clone();
    }
    fields.into_iter().map(|(name, item)| (name, value(item))).collect()
}

fn controller_of(runner: Result<Held, Fault>) -> Obj {
    let mut controller = Obj::new(Class::named("sheltersim.experiments.runner", "OfflineSimulationController"));
    controller.attrs.insert(text("decisions"), runner);
    controller
}

fn runner_of(policy: Result<Held, Fault>) -> Result<Held, Fault> {
    Ok(Held::Object(object(MODULE, "BehaviorDecisionRunner", vec![("policy", policy)])))
}

/// `controller.decisions` with a utility policy of these fields.
fn utility(fields: Vec<(&'static str, Result<Held, Fault>)>) -> Behavior {
    Behavior::of(&controller_of(runner_of(Ok(Held::Object(object(MODULE, "UtilityBehaviorPolicy", fields))))))
}

/// The policy `name`: `utility`, `sleep` or `priority`.
fn behavior_of(name: &str) -> Behavior {
    match name {
        "utility" => utility(scorer_fields(&[])),
        "sleep" => utility(scorer_fields(&[("policy_version", V::Str(text("sleep-aware-utility-softmax-v2"))), ("sleep_debt_utility_weight", V::Float(1.25))])),
        _ => {
            let scorer = object(MODULE, "UtilityBehaviorPolicy", scorer_fields(&[("hydration_action_threshold", V::Float(0.5))]));
            let policy = object(
                MODULE,
                "PriorityRuleBehaviorPolicy",
                vec![("policy_version", value(V::Str(text("priority-rule-v1")))), ("scorer", Ok(Held::Object(scorer)))],
            );
            Behavior::of(&controller_of(runner_of(Ok(Held::Object(policy)))))
        }
    }
}

fn due(data: &ViewData, behavior: &Behavior) -> Got<Option<Vec<Proposal>>> {
    let scoring = behavior::scoring_of(behavior)?;
    behavior::due(data, &scoring, SEED, text(&"0".repeat(64)))
}

/// The text of a refusal: the rule's, or one the Run's reading of the policy kept.
fn refusal<T: std::fmt::Debug>(got: Got<T>) -> String {
    match got {
        Err(Bad::Refused(Error::Unsupported(reason))) => reason.into_owned(),
        Err(Bad::Policy(fault)) if fault.refusal().is_some() => fault.refusal().unwrap().to_owned(),
        other => panic!("not a refusal: {other:?}"),
    }
}

// ---- The tests -----------------------------------------------------------------------------------

#[test]
fn the_proposals_are_the_recorded_ones() {
    let expected = expected();
    for name in ["utility", "sleep", "priority"] {
        let entry = &expected[name];
        let world = world();
        let data = view_of(&world);
        let state_hash = text(entry["state_hash"].as_str().unwrap());
        let behavior = behavior_of(name);
        let scoring = behavior::scoring_of(&behavior).map_err(|_| "the policy").unwrap();
        let proposals = behavior::due(&data, &scoring, SEED, state_hash).map_err(|_| "a refusal").unwrap().unwrap();
        let wanted = entry["proposals"].as_array().unwrap();
        assert_eq!(proposals.len(), wanted.len(), "{name}");
        for (proposal, wanted) in proposals.iter().zip(wanted) {
            let outcome = &proposal.outcome;
            let agent = &data.agents[proposal.index];
            let candidates: Vec<Value> = outcome
                .candidates
                .iter()
                .map(|item| json!([item.action.value(), item.eligible, hex(item.utility), item.priority, hex(item.probability)]))
                .collect();
            let key: Vec<Value> = proposal
                .rng_key(&data)
                .into_iter()
                .map(|part| match part {
                    Ok(text) => json!(text),
                    Err(number) => json!(number),
                })
                .collect();
            let found = json!({
                "agent_id": &*agent.id,
                "action": outcome.action.value(),
                "choice": hex(outcome.choice_value),
                "selected": hex(outcome.selection_probability),
                "next": outcome.next_decision_at,
                "target": outcome.target.map(|target| data.agents[target].id.to_string()),
                "rng_key": key,
                "policy_version": &*proposal.common.policy_version,
                "candidates": candidates,
            });
            for field in ["agent_id", "action", "choice", "selected", "next", "target", "rng_key", "policy_version"] {
                assert_eq!(found[field], wanted[field], "{name}: {} {field}", agent.id);
            }
            // The recorded candidates without their trigger texts (the third item).
            let recorded: Vec<Value> = wanted["candidates"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| {
                    Value::Array(item.as_array().unwrap().iter().enumerate().filter(|(place, _)| *place != 2).map(|(_, value)| value.clone()).collect())
                })
                .collect();
            assert_eq!(found["candidates"], Value::Array(recorded), "{name}: {} candidates", agent.id);
        }
    }
}

#[test]
fn the_policy_is_read_with_the_refusals_and_order_of_the_kernel() {
    let data = view_of(&world());
    // The classes, by `is`.
    let other = Behavior::of(&controller_of(Ok(Held::Object(object("tests", "OtherRunner", vec![])))));
    assert_eq!(refusal(due(&data, &other)), OTHER_RUNNER);
    assert_eq!(refusal(due(&data, &Behavior::of(&controller_of(runner_of(value(V::Null)))))), OTHER_POLICY);
    let foreign = object("tests", "Counting", vec![]);
    assert_eq!(refusal(due(&data, &Behavior::of(&controller_of(runner_of(Ok(Held::Object(foreign.clone()))))))), OTHER_POLICY);
    let priority = object(MODULE, "PriorityRuleBehaviorPolicy", vec![("scorer", Ok(Held::Object(foreign)))]);
    assert_eq!(refusal(due(&data, &Behavior::of(&controller_of(runner_of(Ok(Held::Object(priority))))))), OTHER_SCORER);

    // A read that raised is raised again by its path from the controller.
    let raised = Fault::new(FaultClass::Other(text("builtins.AttributeError")), "no attribute 'temperature'");
    let mut fields = scorer_fields(&[]);
    fields.iter_mut().find(|(name, _)| *name == "temperature").unwrap().1 = Err(raised.clone());
    match due(&data, &utility(fields)) {
        Err(Bad::Policy(fault)) => {
            assert_eq!(fault.path, ["decisions", "policy", "temperature"]);
            assert_eq!(fault.fault, raised);
        }
        other => panic!("{other:?}"),
    }
    let missing = Behavior::of(&controller_of(Err(raised.clone())));
    let Err(Bad::Policy(fault)) = due(&data, &missing) else { panic!() };
    assert_eq!(fault.path, ["decisions"]);

    // The values, as the kernel read them.
    let refused_with = |name: &str, changed: V| refusal(due(&data, &utility(scorer_fields(&[(name, changed)]))));
    assert_eq!(refused_with("temperature", V::Big("1".repeat(30).into())), "an int is too large");
    assert_eq!(refused_with("temperature", V::Int(1 << 54)), "a policy value is too large");
    assert_eq!(refused_with("temperature", V::Bool(true)), "a policy value is not an int");
    assert_eq!(refused_with("decision_interval_minutes", V::Float(15.0)), "a policy value is not an int");
    assert_eq!(refused_with("temperature_by_agent", V::List(Vec::new())), "the per-person coefficients are not a plain dict");
    assert_eq!(refused_with("policy_version", V::Int(1)), "the policy version is not a str");
    assert_eq!(refused_with("policy_version", V::Str(text(""))), "the policy version is empty or not a str");
    assert_eq!(refused_with("temperature", V::Float(-1e-300)), "an exponent is out of range");
    assert_eq!(refused_with("decision_interval_minutes", V::Int(0)), "a proposal has invalid decision timing");
    let mut subclass = scorer_fields(&[]);
    subclass.iter_mut().find(|(name, _)| *name == "policy_version").unwrap().1 = Ok(Held::Text(text("utility-softmax-v1")));
    assert_eq!(refusal(due(&data, &utility(subclass))), "the policy version is not a str");
    let mut opaque = scorer_fields(&[]);
    opaque.iter_mut().find(|(name, _)| *name == "autonomy_enabled").unwrap().1 = Ok(Held::Opaque { callable: false });
    assert_eq!(refusal(due(&data, &utility(opaque))), "a flag is not a plain value");
    // An int counts as its float, and a person's own coefficients win.
    let as_ints = utility(scorer_fields(&[("water_utility_base", V::Int(0)), ("temperature_by_agent", v(&json!({"A2": 1})))]));
    assert!(due(&data, &as_ints).map_err(|_| "refused").unwrap().is_some());

    // Nobody due: nothing is read past the runner's classes.
    let mut world = world();
    for id in ["A1", "A2", "A3"] {
        world.entities.get_mut().agents.get_mut(id).unwrap().next_decision_at = MINUTE + 1;
    }
    let data = view_of(&world);
    assert!(due(&data, &utility(scorer_fields(&[("temperature", V::Bool(true))]))).map_err(|_| "refused").unwrap().is_none());
}

/// The first proposal of the utility policy, and the world.
fn first_proposal() -> (Shard<World>, ViewData, Proposal) {
    let world = world();
    let data = view_of(&world);
    let proposal = due(&data, &behavior_of("utility")).map_err(|_| "refused").unwrap().unwrap().remove(0);
    (Shard::new(world), data, proposal)
}

fn recording<'a>(data: &'a ViewData, proposal: &'a Proposal) -> Recording<'a> {
    Recording { data, proposal, target_id: Some("X"), next_decision_at: MINUTE + 15, committed: false, remembers: false }
}

/// The outcome's two steps (`decisions::commit` and `decisions::remember`, in that order in the
/// propose phase).
fn outcome(cell: &Shard<World>, active: &mut ActiveActions, recording: &Recording<'_>) -> Result<(), Error> {
    cell.with(|world| decisions::commit(world, active, recording)).unwrap().map_err(|error| Error::refused(LIFECYCLE, error))?;
    cell.with(|world| decisions::remember(world, recording)).unwrap().map_err(|error| Error::refused(MEMORY, error))
}

#[test]
fn the_lifecycle_and_the_memory_change_nothing_unless_asked() {
    let (cell, data, proposal) = first_proposal();
    let mut active = ActiveActions::default();
    // Nothing committed and no memory: nothing is done.
    assert_eq!(outcome(&cell, &mut active, &recording(&data, &proposal)), Ok(()));
    assert!(active.is_empty());
    // What the rules refuse is an `Error::Refused`: the world keeps no event queue for the
    // lifecycle, and no `agent_dynamics_v1` for the memory.
    let committing = Recording { committed: true, ..recording(&data, &proposal) };
    assert_eq!(outcome(&cell, &mut active, &committing), Err(Error::Refused { label: LIFECYCLE.to_owned(), reason: "the event queue is not in Rust".into() }));
    assert!(active.is_empty());
    let remembering = Recording { remembers: true, ..recording(&data, &proposal) };
    match outcome(&cell, &mut active, &remembering) {
        Err(Error::Refused { label, .. }) => assert_eq!(label, MEMORY),
        other => panic!("{other:?}"),
    }

    // With the queue, the log and the event counter, the action starts with its receipt.
    let mut world = world();
    world.scalars.get_mut().duration_minutes = Fit::Ok(2000);
    let index = world.slots.add(text(ID_COUNTERS), Place::Field).unwrap();
    world.slots.at(index).unwrap().try_borrow_mut().unwrap().row = Some(Box::new(ValueRow(v(&json!({"event": 5})))));
    world.hold_queue(Arc::new(Queue::new()));
    world.hold_log(Arc::new(EventLog::new()));
    let cell = Shard::new(world);
    assert_eq!(outcome(&cell, &mut active, &committing), Ok(()));
    assert!(active.contains("A1"));
    let receipt = cell
        .with(|world| {
            let log = world.log().unwrap();
            assert_eq!(log.len(), 1);
            serde_json::from_slice::<Value>(log.written(0).unwrap()).unwrap()
        })
        .unwrap();
    assert_eq!(receipt["event"]["event_id"], json!("action-lifecycle-5"));
    let payload = &receipt["event"]["payload"];
    assert_eq!(
        (&payload["action_id"], &payload["action"], &payload["transition"]),
        (&json!("action-5"), &json!(proposal.outcome.action.value()), &json!("started"))
    );
    assert_eq!(payload["target_id"], json!("X"));
}
