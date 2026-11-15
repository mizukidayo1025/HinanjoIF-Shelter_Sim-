//! The namespaces of `model_state` at minute 0, made in this order:
//! `WorldState.from_scenario` (the stock's ledger, the resource service, the operational events,
//! infrastructure, the zone environment, information and the psychological state), then
//! `initialize_simulator_extensions` (its factories, then `validate_simulator_extensions`), then
//! `initialize_declared_states` (#429). Each is put into the world as it is made, so a later
//! factory reads the earlier ones; an error is the first one met, with its class and text.

use std::collections::BTreeSet;
use std::sync::Arc;

use indexmap::IndexMap;
use serde_json::Value;

use super::{bad, text, vmap, vstr, Scenario};
use crate::config::agent_dynamics::{awareness, canonical_policy};
use crate::config::Selection;
use crate::contracts::{self, Checked, Holder};
use crate::entities::AgentRow;
use crate::error::{Error, Result};
use crate::geometry::{zone_center, Pt};
use crate::host::Contract;
use crate::load;
use crate::row::Row;
use crate::rules::agent_dynamics as agent_rules;
use crate::validate::catalog::catalog;
use crate::validate::extension_head::py::Held;
use crate::validate::{self, selection, signatures, View};
use crate::value::V;
use crate::world::World;

/// `RESOURCE_IDS`, in their order.
pub(crate) const RESOURCE_IDS: [&str; 2] = ["water_ml", "food_meal_units"];

/// `H03_EVENT_TYPES`.
pub(crate) const H03_EVENT_TYPES: [&str; 6] =
    ["resource_delivery", "delivery_delayed", "water_outage", "water_restored", "staff_unavailable", "staff_shift_change"];

const SHARED_OPERATIONS_POLICY: &str = "shared-operations-v1";

fn sorted_ids<'a>(ids: impl Iterator<Item = &'a Arc<str>>) -> Vec<Arc<str>> {
    let mut ids: Vec<Arc<str>> = ids.cloned().collect();
    ids.sort_by(|first, second| first.as_bytes().cmp(second.as_bytes()));
    ids.dedup();
    ids
}

fn agent_ids(scenario: &Scenario) -> Vec<Arc<str>> {
    sorted_ids(scenario.agents.iter().map(|agent| &agent.profile.id))
}

/// The agents by ID, in ID order (`sorted(world.agents.items())`).
fn agents_by_id(scenario: &Scenario) -> Vec<(Arc<str>, &AgentRow)> {
    let mut by_id: IndexMap<Arc<str>, &AgentRow> = IndexMap::new();
    for agent in &scenario.agents {
        by_id.insert(agent.profile.id.clone(), agent);
    }
    by_id.sort_by(|first, _, second, _| first.as_bytes().cmp(second.as_bytes()));
    by_id.into_iter().collect()
}

fn staff_ids(scenario: &Scenario) -> Vec<Arc<str>> {
    sorted_ids(scenario.staff.iter().map(|person| &person.id))
}

fn records(ids: &[Arc<str>], record: impl Fn(&Arc<str>) -> Result<V>) -> Result<V> {
    let mut map = IndexMap::with_capacity(ids.len());
    for id in ids {
        map.insert(id.clone(), record(id)?);
    }
    Ok(V::Map(map))
}

fn empty_map() -> V {
    V::Map(IndexMap::new())
}

fn empty_list() -> V {
    V::List(Vec::new())
}

fn member<'a>(value: &'a V, key: &str) -> Option<&'a V> {
    match value {
        V::Map(map) => map.get(key),
        _ => None,
    }
}

fn value_error(text: impl Into<String>) -> Error {
    Error::Value(text.into())
}

// ---- Made by `WorldState.from_scenario` --------------------------------------------------------------

/// `create_resource_ledger(initial_stock)`.
fn resource_ledger(stock: &Value) -> Result<V> {
    let warehouse = vstr(super::str_of(&stock["warehouse_id"], "warehouse_id")?);
    let zone = match &stock["zone_id"] {
        Value::Null => V::Null,
        found => vstr(super::str_of(found, "a stock's zone_id")?),
    };
    let mut sources: Vec<Arc<str>> =
        super::array(&stock["source_ids"], "source_ids")?.iter().map(|id| super::str_of(id, "a source ID").map(text)).collect::<Result<_>>()?;
    sources.sort_by(|first, second| first.as_bytes().cmp(second.as_bytes()));
    let sources = V::List(sources.into_iter().map(V::Str).collect());
    let mut accounts = IndexMap::new();
    let mut audit = Vec::new();
    for resource in RESOURCE_IDS {
        let initial = V::Int(super::int_of(&stock[resource], resource)?);
        accounts.insert(
            text(resource),
            vmap([
                ("initial", initial.clone()),
                ("delivered", V::Int(0)),
                ("served", V::Int(0)),
                ("losses", V::Int(0)),
                ("remaining", initial.clone()),
                ("reserved", V::Int(0)),
                ("source_ids", sources.clone()),
                ("reservations", empty_map()),
            ]),
        );
        audit.push(vmap([
            ("schema_version", V::Int(1)),
            ("minute", V::Int(0)),
            ("event_id", V::Null),
            ("operation", vstr("initial")),
            ("resource_id", vstr(resource)),
            ("quantity", initial.clone()),
            ("reservation_id", V::Null),
            ("agent_id", V::Null),
            ("source_id", V::Null),
            ("source_ids", sources.clone()),
            ("warehouse_id", warehouse.clone()),
            ("initial", initial.clone()),
            ("delivered", V::Int(0)),
            ("served", V::Int(0)),
            ("losses", V::Int(0)),
            ("remaining", initial),
            ("reserved", V::Int(0)),
        ]));
    }
    Ok(vmap([
        ("schema_version", V::Int(1)),
        ("warehouse_id", warehouse),
        ("zone_id", zone),
        ("accounts", V::Map(accounts)),
        ("used_reservation_ids", empty_list()),
        ("audit", V::List(audit)),
    ]))
}

fn resource_service() -> V {
    vmap([
        ("schema_version", V::Int(1)),
        ("cancelled_task_ids", empty_list()),
        ("active_reservation_ids", empty_list()),
        ("request_history", empty_list()),
        ("queue_stall_history", empty_list()),
        ("stall_state", empty_map()),
    ])
}

/// `operational_events_enabled(configs, events)`.
pub(crate) fn operational_enabled(configs: &IndexMap<Arc<str>, V>, events: &[crate::events::EventRow]) -> Result<bool> {
    let configured = match configs.get("operational_events") {
        None | Some(V::Null) => false,
        Some(V::Map(map)) if map.len() == 1 => match map.get("enabled") {
            Some(V::Bool(enabled)) => *enabled,
            _ => return Err(value_error("operational_events config must be {'enabled': bool}")),
        },
        Some(_) => return Err(value_error("operational_events config must be {'enabled': bool}")),
    };
    Ok(configured
        || events.iter().any(|event| {
            H03_EVENT_TYPES.contains(&&*event.event_type) || (&*event.event_type == "resource_delivery" && member(&event.payload, "shipment_id").is_some())
        }))
}

fn payload(event: &crate::events::EventRow, key: &str) -> Result<V> {
    member(&event.payload, key).cloned().ok_or_else(|| Error::Key(key.to_owned()))
}

fn id_of(value: V) -> Result<Arc<str>> {
    match value {
        V::Str(found) => Ok(found),
        _ => Err(Error::unsupported("a payload's ID is not a str")),
    }
}

/// `create_operational_events_state(events, staff_ids)`.
fn operational_events(scenario: &Scenario) -> Result<V> {
    let mut shipments = IndexMap::new();
    let mut restores = IndexMap::new();
    for event in &scenario.events {
        if &*event.event_type == "resource_delivery" && member(&event.payload, "shipment_id").is_some() {
            shipments.insert(
                id_of(payload(event, "shipment_id")?)?,
                vmap([("event_id", V::Str(event.event_id.clone())), ("resource_id", payload(event, "resource_id")?), ("due_minute", V::Int(event.due_minute))]),
            );
        }
        if &*event.event_type == "water_restored" {
            restores.insert(id_of(payload(event, "outage_id")?)?, event.event_id.clone());
        }
    }
    let mut outages = IndexMap::new();
    for event in scenario.events.iter().filter(|event| &*event.event_type == "water_outage") {
        let outage = id_of(payload(event, "outage_id")?)?;
        let zone = member(&event.payload, "zone_id");
        let target = match zone {
            Some(found) => found.clone(),
            None => member(&event.payload, "facility_id").cloned().unwrap_or(V::Null),
        };
        let restore = restores.get(&outage).cloned().map(V::Str).unwrap_or(V::Null);
        outages.insert(
            outage,
            vmap([
                ("target_kind", vstr(if zone.is_some() { "zone" } else { "facility" })),
                ("target_id", target),
                ("outage_event_id", V::Str(event.event_id.clone())),
                ("restore_event_id", restore),
                ("active", V::Bool(false)),
            ]),
        );
    }
    let status = records(&staff_ids(scenario), |_| {
        Ok(vmap([("available", V::Bool(true)), ("unavailability_event_id", V::Null), ("until_minute", V::Null), ("restore_event_id", V::Null)]))
    })?;
    Ok(vmap([
        ("schema_version", V::Int(1)),
        ("shipments", V::Map(shipments)),
        ("water_outages", V::Map(outages)),
        ("staff_status", status),
        ("cancelled_staff_task_ids", empty_list()),
    ]))
}

/// `dependent_facilities(config, component_id, facility_ids)`: the facilities downstream of a
/// component, sorted.
fn dependent_facilities(edges: &[(Arc<str>, Arc<str>)], component: &Arc<str>, facilities: &BTreeSet<Arc<str>>) -> V {
    let mut dependents: IndexMap<&Arc<str>, BTreeSet<&Arc<str>>> = IndexMap::new();
    for (dependent, dependency) in edges {
        dependents.entry(dependency).or_default().insert(dependent);
    }
    let mut reached: BTreeSet<Arc<str>> = BTreeSet::new();
    let mut pending = std::collections::VecDeque::from([component.clone()]);
    let mut visited: BTreeSet<Arc<str>> = BTreeSet::from([component.clone()]);
    while let Some(current) = pending.pop_front() {
        if facilities.contains(&current) {
            reached.insert(current.clone());
        }
        let Some(next) = dependents.get(&current) else {
            continue;
        };
        for dependent in next {
            if facilities.contains(*dependent) {
                reached.insert((*dependent).clone());
            }
            if visited.insert((*dependent).clone()) {
                pending.push_back((*dependent).clone());
            }
        }
    }
    let mut reached: Vec<Arc<str>> = reached.into_iter().collect();
    reached.sort_by(|first, second| first.as_bytes().cmp(second.as_bytes()));
    V::List(reached.into_iter().map(V::Str).collect())
}

/// `create_infrastructure_runtime_state(world.facilities, events, configs["infrastructure"])`.
fn infrastructure(scenario: &Scenario, raw: &V) -> Result<V> {
    let config = contracts::validate(&Contract::Infrastructure, raw)?;
    let mut edges = Vec::new();
    if let Some(V::List(items)) = member(&config, "dependency_edges") {
        for item in items {
            let (Some(V::Str(dependent)), Some(V::Str(dependency))) = (member(item, "dependent_id"), member(item, "dependency_id")) else {
                return Err(Error::unsupported("a dependency edge has no IDs"));
            };
            edges.push((dependent.clone(), dependency.clone()));
        }
    }
    let facility_ids: BTreeSet<Arc<str>> = scenario.facilities.iter().map(|facility| facility.id.clone()).collect();
    let mut outages = IndexMap::new();
    let mut stressors = IndexMap::new();
    let mut interruptions = IndexMap::new();
    for event in &scenario.events {
        match &*event.event_type {
            "power_outage" | "equipment_failure" => {
                let outage = id_of(payload(event, "outage_id")?)?;
                let power = &*event.event_type == "power_outage";
                let target = id_of(payload(event, if power { "component_id" } else { "facility_id" })?)?;
                let units_out = if power { V::Null } else { payload(event, "units_out")? };
                let record = vmap([
                    ("event_type", V::Str(event.event_type.clone())),
                    ("target_id", V::Str(target.clone())),
                    ("start_event_id", V::Str(event.event_id.clone())),
                    ("restore_event_id", V::Null),
                    ("affected_facility_ids", dependent_facilities(&edges, &target, &facility_ids)),
                    ("units_out", units_out),
                    ("until_minute", payload(event, "until_minute")?),
                    ("active", V::Bool(false)),
                ]);
                outages.insert(outage, record);
            }
            "extreme_heat" | "extreme_cold" => {
                let stressor = id_of(payload(event, "stressor_id")?)?;
                let rate = if &*event.event_type == "extreme_heat" { "heat_stress_per_minute" } else { "cold_stress_per_minute" };
                let record = vmap([
                    ("event_type", V::Str(event.event_type.clone())),
                    ("zone_id", payload(event, "zone_id")?),
                    ("start_event_id", V::Str(event.event_id.clone())),
                    ("end_event_id", V::Null),
                    ("until_minute", payload(event, "until_minute")?),
                    ("stress_per_minute", member(&config, rate).cloned().unwrap_or(V::Null)),
                    ("source_id", member(&config, "source_id").cloned().unwrap_or(V::Null)),
                    ("active", V::Bool(false)),
                ]);
                stressors.insert(stressor, record);
            }
            "supply_route_interruption" => {
                let outage = id_of(payload(event, "outage_id")?)?;
                let shipments = match payload(event, "shipment_ids")? {
                    V::List(items) => V::List(items),
                    _ => return Err(Error::unsupported("shipment_ids is not a list")),
                };
                let record = vmap([
                    ("route_id", payload(event, "route_id")?),
                    ("start_event_id", V::Str(event.event_id.clone())),
                    ("restore_event_id", V::Null),
                    ("until_minute", payload(event, "until_minute")?),
                    ("shipment_ids", shipments),
                    ("active", V::Bool(false)),
                ]);
                interruptions.insert(outage, record);
            }
            _ => {}
        }
    }
    let mut facilities: Vec<&crate::entities::FacilityRow> = scenario.facilities.iter().collect();
    facilities.sort_by(|first, second| first.id.as_bytes().cmp(second.id.as_bytes()));
    let base = facilities.into_iter().map(|facility| (facility.id.clone(), V::Int(facility.working_units))).collect();
    Ok(vmap([
        ("schema_version", V::Int(1)),
        ("base_working_units", V::Map(base)),
        ("outages", V::Map(outages)),
        ("environmental_stressors", V::Map(stressors)),
        ("route_interruptions", V::Map(interruptions)),
    ]))
}

/// `create_information_runtime_state(set(world.agents))`.
fn information(scenario: &Scenario) -> Result<V> {
    let ids = agent_ids(scenario);
    Ok(vmap([
        ("schema_version", V::Int(2)),
        ("messages", empty_map()),
        ("beliefs", records(&ids, |_| Ok(empty_map()))?),
        ("contacts", empty_map()),
        ("query_pressure", records(&ids, |_| Ok(V::Float(0.0)))?),
        ("support_requests", empty_list()),
        (
            "counters",
            vmap([("contact_attempts", V::Int(0)), ("contact_deliveries", V::Int(0)), ("correction_deliveries", V::Int(0)), ("support_requests", V::Int(0))]),
        ),
    ]))
}

/// `create_psychological_state` of each person's baseline in `psychology.baseline_distress_by_agent`.
fn psychological(scenario: &Scenario) -> Result<V> {
    let baselines = scenario.configs.get("psychology").and_then(|psychology| member(psychology, "baseline_distress_by_agent"));
    let agents = records(&agent_ids(scenario), |id| {
        let baseline = match baselines {
            None => V::Float(0.0),
            Some(V::Map(map)) => map.get(id).cloned().unwrap_or(V::Float(0.0)),
            Some(_) => return Err(Error::unsupported("the baselines are not a dict")),
        };
        let baseline = match baseline {
            V::Int(value) => Some(value as f64),
            V::Float(value) if value.is_finite() => Some(value),
            _ => None,
        }
        .filter(|value| (0.0..=1.0).contains(value))
        .ok_or_else(|| value_error(format!("baseline distress for {id} must be finite and between 0 and 1")))?;
        Ok(vmap([
            ("baseline_distress", V::Float(baseline)),
            ("long_term_distress", V::Float(baseline)),
            ("daily_acute_sum", V::Float(0.0)),
            ("daily_high_distress_minutes", V::Int(0)),
            ("daily_observed_minutes", V::Int(0)),
            ("completed_days", V::Int(0)),
        ]))
    })?;
    Ok(vmap([("schema_version", V::Int(1)), ("last_updated_minute", V::Int(-1)), ("agents", agents)]))
}

// ---- The simulator extensions ----------------------------------------------------------------------

/// `create_population_dynamics_state`: the people's traits as `PopulationSpec` dumps them.
fn population_dynamics(scenario: &Scenario) -> Result<V> {
    let mut traits = IndexMap::new();
    if let Some(raw) = scenario.configs.get("population") {
        let dump = contracts::validate(&Contract::PopulationSpec, raw)?;
        let Some(V::Map(people)) = member(&dump, "people") else {
            return Err(Error::unsupported("the population has no people"));
        };
        traits = people.clone();
        traits.sort_by(|first, _, second, _| first.as_bytes().cmp(second.as_bytes()));
    }
    Ok(vmap([
        ("schema_version", V::Int(1)),
        ("policy_version", vstr("capacity-aware-arrivals-departures-v1")),
        ("source_id", vstr("ASM-POPULATION-DYNAMICS-V1")),
        ("initial_agent_ids", V::List(agent_ids(scenario).into_iter().map(V::Str).collect())),
        ("profile_traits", V::Map(traits)),
        ("profile_context", empty_map()),
        ("pending_arrivals", empty_list()),
        ("arrivals", empty_list()),
        ("departures", empty_list()),
    ]))
}

fn social_network() -> V {
    vmap([
        ("schema_version", V::Int(1)),
        ("policy_version", vstr("directed-contact-relationships-v1")),
        ("relationships", empty_map()),
        ("interaction_count", V::Int(0)),
        ("information_transfer_count", V::Int(0)),
    ])
}

/// `create_staff_dynamics_state`.
fn staff_dynamics(scenario: &Scenario) -> Result<V> {
    let raw = scenario.configs.get("staff_dynamics").cloned().unwrap_or(V::Null);
    let mut config = validate::staff_dynamics::canonical_staff_dynamics_config(&raw, Held::Tracked)?;
    if matches!(config.get("max_continuous_work_minutes_by_staff"), Some(V::Map(limits)) if limits.is_empty()) {
        config.shift_remove("max_continuous_work_minutes_by_staff");
    }
    let staff = records(&staff_ids(scenario), |_| {
        Ok(vmap([
            ("consecutive_work_minutes", V::Int(0)),
            ("total_work_minutes", V::Int(0)),
            ("total_rest_minutes", V::Int(0)),
            ("fatigue", V::Float(0.0)),
            ("rest_until_minute", V::Int(0)),
            ("last_activity", vstr("off_duty")),
        ]))
    })?;
    Ok(vmap([
        ("schema_version", V::Int(1)),
        ("policy_version", config.get("policy_version").cloned().unwrap_or(V::Null)),
        ("source_id", config.get("source_id").cloned().unwrap_or(V::Null)),
        ("config", V::Map(config)),
        ("staff", staff),
    ]))
}

fn shared_operations() -> V {
    vmap([
        ("schema_version", V::Int(1)),
        ("policy_version", vstr(SHARED_OPERATIONS_POLICY)),
        ("support_requests", empty_map()),
        ("history", empty_list()),
        ("history_truncated", V::Bool(false)),
    ])
}

fn population_lifecycle() -> V {
    vmap([
        ("schema_version", V::Int(1)),
        ("policy_version", vstr("same-id-population-lifecycle-v1")),
        ("source_id", vstr("ASM-POPULATION-LIFECYCLE-V1")),
        ("people", empty_map()),
        ("relationship_archive", empty_list()),
        ("messages", empty_map()),
        ("deferred_beliefs", empty_map()),
        ("pending_returns", empty_list()),
        ("pending_relocations", empty_map()),
        ("history", empty_list()),
    ])
}

fn temporary_spaces() -> V {
    vmap([("schema_version", V::Int(1)), ("policy_version", vstr("typed-temporary-capacity-v1")), ("spaces", empty_map()), ("history", empty_list())])
}

/// `create_staff_operations_state`: each zone's sanitation and each staff member idle.
fn staff_operations(scenario: &Scenario) -> Result<V> {
    let raw = scenario.configs.get("staff_operations").cloned().unwrap_or(V::Null);
    let config = validate::staff_operations::canonical_staff_operations_config(&raw)?;
    let zones = records(&sorted_ids(scenario.zones.iter().map(|zone| &zone.id)), |_| Ok(vmap([("sanitation", V::Float(1.0)), ("updated_minute", V::Int(0))])))?;
    let staff = records(&staff_ids(scenario), |_| {
        Ok(vmap([
            ("phase", vstr("idle")),
            ("task_id", V::Null),
            ("task_kind", V::Null),
            ("target_id", V::Null),
            ("target_zone_id", V::Null),
            ("route_zone_ids", empty_list()),
            ("route_edge_ids", empty_list()),
            ("travel_minutes", V::Int(0)),
            ("started_minute", V::Null),
            ("arrival_minute", V::Null),
            ("travel_due_minute", V::Null),
            ("due_minute", V::Null),
            ("reason", V::Null),
            ("causation_event_ids", empty_list()),
        ]))
    })?;
    Ok(vmap([
        ("schema_version", V::Int(1)),
        ("policy_version", vstr("autonomous-routed-staff-v1")),
        ("source_id", vstr("ASM-STAFF-OPERATIONS-V1")),
        ("config", V::Map(config)),
        ("zones", zones),
        ("staff", staff),
        ("history", empty_list()),
    ]))
}

/// `create_body_state`'s record of each person, from their fatigue and stress (0.0 at minute 0),
/// with the config `_body_config_for_world` gives.
fn body_state(scenario: &Scenario, selection: &Selection) -> Result<V> {
    let raw = scenario.configs.get("body_dynamics").cloned().unwrap_or(V::Null);
    let mut config = validate::body_state::canonical_body_config(&raw)?;
    if let Some(affect) = selection.get("affect") {
        config.insert(text("affect_policy"), V::Str(affect.clone()));
    }
    let mut agents = IndexMap::new();
    for (id, agent) in agents_by_id(scenario) {
        let sleep_debt = agent.needs.fatigue * 360.0;
        let uncertainty = 0.25 + agent.stress * 0.5;
        agents.insert(
            id,
            vmap([
                ("sleep_debt_minutes", V::Float(if sleep_debt < 1440.0 { sleep_debt } else { 1440.0 })),
                ("uncertainty", V::Float(if uncertainty < 1.0 { uncertainty } else { 1.0 })),
                ("reassurance", V::Float(0.0)),
                ("discontent", V::Float(agent.stress)),
                ("positive_affect", V::Float(0.5)),
                ("negative_affect", V::Float(agent.stress)),
                ("hygiene_need", V::Float(0.0)),
            ]),
        );
    }
    Ok(vmap([
        ("schema_version", V::Int(1)),
        ("policy_version", vstr("life-state-feedback-v1")),
        ("source_id", vstr("ASM-LIFE-STATE-FEEDBACK-V1")),
        ("config", V::Map(config)),
        ("agents", V::Map(agents)),
        ("relationships", empty_map()),
        ("history", empty_list()),
    ]))
}

fn number(value: Option<&V>) -> Result<f64> {
    match value {
        Some(V::Int(found)) => Ok(*found as f64),
        Some(V::Float(found)) if found.is_finite() => Ok(*found),
        _ => Err(bad("a point's coordinate is not a number")),
    }
}

/// The center of each zone of the floor plan (`_zone_center`), by zone ID; the first of a
/// repeated ID.
fn zone_centers(geometry: Option<&V>) -> Result<IndexMap<Arc<str>, Option<Pt>>> {
    let mut centers = IndexMap::new();
    let Some(geometry) = geometry else {
        return Ok(centers);
    };
    let Some(V::List(zones)) = member(geometry, "zones") else {
        return Err(bad("facility_geometry has no zones"));
    };
    for zone in zones {
        let Some(V::Str(id)) = member(zone, "zone_id") else {
            return Err(bad("a floor plan's zone has no zone_id"));
        };
        if centers.contains_key(id) {
            continue;
        }
        let Some(V::List(corners)) = member(zone, "polygon") else {
            return Err(bad("a floor plan's zone has no polygon"));
        };
        let polygon = corners.iter().map(|corner| Ok(Pt { x: number(member(corner, "x"))?, y: number(member(corner, "y"))? })).collect::<Result<Vec<Pt>>>()?;
        if polygon.is_empty() {
            return Err(bad("a floor plan's zone has no corners"));
        }
        let center = zone_center(&polygon).map_err(|()| Error::unsupported("a zone's center is not finite"))?;
        centers.insert(id.clone(), center);
    }
    Ok(centers)
}

/// `create_movement_runtime_state`: each person and staff member at their zone's center.
fn movement(scenario: &Scenario, geometry: Option<&V>) -> Result<V> {
    let centers = zone_centers(geometry)?;
    let position = |zone: &Arc<str>| {
        let point = centers.get(zone).copied().flatten();
        vmap([
            ("zone_id", V::Str(zone.clone())),
            ("point", point.map_or(V::Null, |point| vmap([("x", V::Float(point.x)), ("y", V::Float(point.y))]))),
            ("edge_id", V::Null),
            ("progress", V::Float(0.0)),
            ("position_kind", vstr(if point.is_some() { "physical" } else { "unavailable" })),
        ])
    };
    let positions = V::Map(agents_by_id(scenario).into_iter().map(|(id, agent)| (id, position(&agent.zone_id))).collect());
    let mut staff: IndexMap<Arc<str>, V> = scenario.staff.iter().map(|person| (person.id.clone(), position(&person.zone_id))).collect();
    staff.sort_by(|first, _, second, _| first.as_bytes().cmp(second.as_bytes()));
    Ok(vmap([
        ("schema_version", V::Int(2)),
        ("policy_version", vstr("spatial-route-and-room-waypoint-v2")),
        ("positions", positions),
        ("staff_positions", V::Map(staff)),
        ("journeys", empty_map()),
        ("edge_usage_minutes", empty_map()),
        ("last_updated_minute", V::Int(-1)),
    ]))
}

fn geometry_overlay() -> V {
    vmap([
        ("schema_version", V::Int(1)),
        ("policy_version", vstr("bounded-runtime-geometry-v1")),
        ("facility_points", empty_map()),
        ("partitions", empty_map()),
        ("route_modifiers", empty_map()),
        ("history", empty_list()),
        ("temporary_rooms", empty_map()),
        ("bed_reservations", empty_map()),
        ("room_admissions", empty_map()),
    ])
}

/// The declared presence state's initializer: each person admitted in their zone.
fn presence(scenario: &Scenario) -> V {
    let people = agents_by_id(scenario)
        .into_iter()
        .map(|(id, agent)| {
            let record = vmap([
                ("agent_id", V::Str(id.clone())),
                ("household_id", agent.profile.group_id.clone().map_or(V::Null, V::Str)),
                ("status", vstr("admitted")),
                ("since_minute", V::Int(0)),
                ("transition_zone_id", V::Str(agent.zone_id.clone())),
                ("return_zone_id", V::Null),
                ("expected_return_minute", V::Null),
                ("archive_path", V::Null),
                ("source_event_ids", empty_list()),
            ]);
            (id, record)
        })
        .collect();
    vmap([("schema_version", V::Int(1)), ("people", V::Map(people)), ("history", empty_list())])
}

// ---- In order ------------------------------------------------------------------------------------

fn put(world: &mut World, name: &str, state: V) -> Result<()> {
    load::put_namespace(world, name, state)
}

fn put_row<T: Row>(world: &mut World, name: &str, row: T) -> Result<()> {
    let slot = world.slots.get(name).ok_or_else(|| Error::unsupported("a namespace has no slot"))?;
    slot.try_borrow_mut()?.row = Some(Box::new(row));
    Ok(())
}

fn has(scenario: &Scenario, key: &str) -> bool {
    scenario.configs.contains_key(key)
}

fn configs_value(scenario: &Scenario) -> V {
    V::Map(scenario.configs.clone())
}

/// `WorldState.from_scenario`'s namespaces, to the psychological state.
pub(crate) fn from_scenario(world: &mut World, scenario: &Scenario) -> Result<()> {
    if let Some(stock) = &scenario.initial_stock {
        put(world, "resource_ledger_v1", resource_ledger(stock)?)?;
    }
    if has(scenario, "resource_service") {
        put(world, "resource_service_v1", resource_service())?;
    }
    if operational_enabled(&scenario.configs, &scenario.events)? {
        put(world, "operational_events_v1", operational_events(scenario)?)?;
    }
    if let Some(raw) = scenario.configs.get("infrastructure") {
        put(world, "infrastructure_v1", infrastructure(scenario, raw)?)?;
    }
    if has(scenario, "zone_environment") {
        put(world, "zone_environment_v1", vmap([("schema_version", V::Int(1)), ("modifiers", empty_map())]))?;
    }
    if validate::information::from_mapping(scenario.configs.get("information_spread"))? {
        put(world, "information_spread_v1", information(scenario)?)?;
    }
    let selection = selection::select(&configs_value(scenario))?;
    let psychology = selection.get("psychology").ok_or_else(|| Error::unsupported("the selection has no psychology"))?;
    if catalog()?.is_mean_reverting(psychology) {
        put(world, "psychological_distress_v1", psychological(scenario)?)?;
    }
    Ok(())
}

/// `validate_shared_operations_config(raw)`.
fn shared_operations_config(raw: &V) -> Result<()> {
    let accepted = match raw {
        V::Map(raw) => raw.iter().all(|(key, value)| match &**key {
            "schema_version" => matches!(value, V::Int(1) | V::Bool(true)) || matches!(value, V::Float(number) if *number == 1.0),
            "policy_version" => matches!(value, V::Str(policy) if &**policy == SHARED_OPERATIONS_POLICY),
            _ => false,
        }),
        _ => false,
    };
    if accepted {
        Ok(())
    } else {
        Err(value_error("invalid shared_operations config"))
    }
}

/// The extensions' namespaces, as `initialize_simulator_extensions` makes them, then its
/// `validate_simulator_extensions`.
pub(crate) fn extensions(world: &mut World, scenario: &Scenario) -> Result<()> {
    let configs = &scenario.configs;
    let selection = selection::select(&configs_value(scenario))?;
    let catalog = catalog()?;
    let body = selection.get("psychology").is_some_and(|model| catalog.is_life_state(model));
    let enabled = match configs.get("simulator_extensions") {
        None => false,
        Some(V::Map(extensions)) => extensions.get("enabled").is_some_and(V::is_truthy),
        Some(_) => return Err(Error::unsupported("simulator_extensions is not a dict")),
    };
    let any = ["facility_geometry", "population", "staff_operations", "staff_dynamics", "shared_operations", "facility_awareness"]
        .iter()
        .any(|key| has(scenario, key));
    if !enabled && !any && !body {
        return Ok(());
    }
    if has(scenario, "body_dynamics") && !body {
        return Err(value_error("body_dynamics config requires a life-state psychology policy"));
    }
    // The factories in the dict's order; its conditions are read in the order the dict is built.
    let mut factories: Vec<&str> = vec!["agent_dynamics_v1", "population_dynamics_v1", "social_network_v1", "staff_dynamics_v1"];
    if let Some(raw) = configs.get("shared_operations") {
        shared_operations_config(raw)?;
        factories.push("shared_operations_v1");
    }
    if enabled || has(scenario, "population") {
        factories.push("population_lifecycle_v1");
        if enabled {
            factories.push("temporary_spaces_v1");
        }
    }
    if enabled || has(scenario, "staff_operations") || body {
        factories.push("staff_operations_v1");
    }
    if body {
        factories.push("body_state_v1");
    }
    if has(scenario, "facility_geometry") {
        factories.push("movement_v1");
    }
    if signatures::independent_queues_required(configs)? {
        factories.push("facility_queues_v1");
    }
    if has(scenario, "geometry_overlay") {
        factories.push("geometry_overlay_v1");
    }
    for name in factories {
        match name {
            "agent_dynamics_v1" => {
                // The kernel `create_agent_dynamics_state` reads the policy as a rule does
                // (`config::agent_dynamics::policy`): a config the check refuses is its refusal.
                let refused = |reason: String| Error::Refused { label: "agent_dynamics.create_state".to_owned(), reason: reason.into() };
                let policy = match canonical_policy(configs.get("agent_dynamics")) {
                    Ok(Ok(policy)) => policy,
                    Ok(Err(_)) | Err(_) => return Err(refused("a config the Python rule refuses".to_owned())),
                };
                let known = awareness(&configs_value(scenario)).map_err(|fault| refused(fault.text.to_string()))?;
                let row = agent_rules::create_state(world, policy, known.as_ref())?;
                put_row(world, name, row)?;
            }
            "population_dynamics_v1" => put(world, name, population_dynamics(scenario)?)?,
            "social_network_v1" => put(world, name, social_network())?,
            "staff_dynamics_v1" => put(world, name, staff_dynamics(scenario)?)?,
            "shared_operations_v1" => put(world, name, shared_operations())?,
            "population_lifecycle_v1" => put(world, name, population_lifecycle())?,
            "temporary_spaces_v1" => put(world, name, temporary_spaces())?,
            "staff_operations_v1" => put(world, name, staff_operations(scenario)?)?,
            "body_state_v1" => put(world, name, body_state(scenario, &selection)?)?,
            "movement_v1" => {
                // `_geometry(world)`: the parsed floor plan (`effective_geometry`, no overlay yet).
                let geometry = match &configs["facility_geometry"] {
                    V::Null => None,
                    raw => Some(contracts::checked(&Checked::FacilityGeometry, raw, Holder::Tracked)?),
                };
                put(world, name, movement(scenario, geometry.as_ref())?)?;
            }
            "facility_queues_v1" => {
                let geo = super::sections::floor_plan(configs)?;
                put_row(world, name, crate::rules::facility_queues::create_state(&geo)?)?;
            }
            "geometry_overlay_v1" => {
                if !has(scenario, "facility_geometry") {
                    return Err(value_error("geometry_overlay requires facility_geometry"));
                }
                put(world, name, geometry_overlay())?;
            }
            _ => return Err(Error::unsupported("an extension the core does not make")),
        }
    }
    validate::extensions(world, true)
}

/// `initialize_declared_states(world)`: each declared namespace the world lacks, made.
pub(crate) fn declared(world: &mut World, scenario: &Scenario) -> Result<()> {
    for contract in validate::declared::declared_states(&configs_value(scenario))? {
        if View::new(world, true, None).contains(&contract.namespace)? {
            continue;
        }
        match &*contract.namespace {
            "population_presence_v1" => put(world, "population_presence_v1", presence(scenario))?,
            _ => return Err(Error::unsupported("a declared state the core does not make")),
        }
    }
    Ok(())
}
