//! The sections of a world's configs that Python parses (`SectionId::parser() == Parser::Host`),
//! as `sections(world)` of `core/world_export.py` writes them (#429): `{"value": ...}`,
//! `{"fault": [class, text]}` for what the parse raised, `{"absent": true}` for a facility geometry
//! the configs lack, or `{"host_error": text}` where the PyO3 crate raises out of `fill_section`.
//! The class of a fault is `config::FaultClass`'s name: `Value`, `Type`, `Key`, `Validation`,
//! `Refused` (a refusal kept as one), or the module and name of another class.

use std::sync::Arc;

use indexmap::IndexMap;

use super::{text, vmap, vstr};
use crate::config::Selection;
use crate::contracts::{self, Checked, Holder};
use crate::error::{Error, Result};
use crate::geometry::Geo;
use crate::host::Contract;
use crate::mathx;
use crate::validate::catalog::catalog;
use crate::validate::py::equal;
use crate::validate::{self, selection, signatures, View};
use crate::value::V;
use crate::world::World;

type Map = IndexMap<Arc<str>, V>;

/// The attributes of `InformationSpreadConfig` the PyO3 crate reads, but `present`.
const INFORMATION_FIELDS: [&str; 12] = [
    "enabled",
    "minimum_contact_minutes",
    "default_source_reliability",
    "correction_source_reliability_by_id",
    "rumor_belief_probability",
    "rumor_trust_sensitivity",
    "rumor_source_sensitivity",
    "query_pressure_per_claim",
    "query_pressure_relief_per_correction",
    "correction_acceptance_probability",
    "correction_trust_sensitivity",
    "correction_source_sensitivity",
];

/// `H03_EVENT_TYPES | {"resource_delivery"}`, sorted.
const OPERATIONAL_EVENTS: [&str; 6] = ["delivery_delayed", "resource_delivery", "staff_shift_change", "staff_unavailable", "water_outage", "water_restored"];

/// `INFRASTRUCTURE_EVENT_TYPES`, sorted.
const INFRASTRUCTURE_EVENTS: [&str; 10] = [
    "environmental_stressor_end",
    "equipment_failure",
    "equipment_repaired",
    "extreme_cold",
    "extreme_heat",
    "power_outage",
    "power_restored",
    "staff_shift_change",
    "supply_route_interruption",
    "supply_route_restored",
];

/// The default staff-task ranking rule (`create_staff_operations_formula`).
const STAFF_OPERATIONS_DEFAULT: &str = "staff-urgency-first-v1";

// ---- Faults ------------------------------------------------------------------------------------

fn fault(class: &str, text: impl AsRef<str>) -> V {
    vmap([("fault", V::List(vec![vstr(class), vstr(text.as_ref())]))])
}

fn value(found: V) -> V {
    vmap([("value", found)])
}

/// `_fault(error)`: the class by `FaultClass`'s name, with `str(error)`.
fn fault_of(error: &Error) -> V {
    match error {
        Error::Value(text) | Error::MarkedValue(text) => fault("Value", text),
        Error::Validation(text) => fault("Validation", text),
        Error::Type(text) => fault("Type", text),
        Error::Key(text) => fault("Key", text),
        Error::Attribute(text) => fault("builtins.AttributeError", text),
        Error::Runtime(text) => fault("builtins.RuntimeError", text),
        Error::Invariant(text) => fault("sheltersim.core.world.StateInvariantError", text),
        Error::Unsupported(text) => fault("sheltersim_native.Unsupported", text),
        other => fault("sheltersim_native.Unsupported", other.to_string()),
    }
}

/// `_fault(error, refusal=True)`: an `Unsupported` kept as a refusal.
fn refusal_or_fault(error: &Error) -> V {
    match error {
        Error::Unsupported(text) => fault("Refused", text),
        other => fault_of(other),
    }
}

/// A contract's `ValueError` is pydantic's `ValidationError`.
fn validation(error: Error) -> Error {
    match error {
        Error::Value(text) => Error::Validation(text),
        other => other,
    }
}

fn section_of(parse: impl FnOnce() -> Result<V>) -> V {
    match parse() {
        Ok(found) => value(found),
        Err(error) => fault_of(&error),
    }
}

fn parsed(parse: impl FnOnce() -> Result<V>) -> V {
    match parse() {
        Ok(found) => value(found),
        Err(error) => refusal_or_fault(&error),
    }
}

fn refused(text: &'static str) -> Error {
    Error::unsupported(text)
}

/// `_exact_int(value)`.
fn exact_int(found: Option<&V>) -> Result<V> {
    match found {
        Some(V::Int(number)) => Ok(V::Int(*number)),
        Some(V::Big(_)) => Err(refused("an int is too large")),
        _ => Err(refused("a value is not an int")),
    }
}

/// `_exact_str(value)`.
fn exact_str(found: Option<&V>) -> Result<V> {
    match found {
        Some(V::Str(text)) => Ok(V::Str(text.clone())),
        _ => Err(refused("a value is not a str")),
    }
}

fn member<'a>(found: &'a V, key: &str) -> Option<&'a V> {
    match found {
        V::Map(map) => map.get(key),
        _ => None,
    }
}

fn empty() -> V {
    V::Map(Map::new())
}

// ---- Each section ------------------------------------------------------------------------------

fn selection_section(selection: &Result<Selection>) -> V {
    match selection {
        Ok(selection) => value(V::List(selection.iter().map(|(domain, model)| V::List(vec![vstr(domain), vstr(model)])).collect())),
        Err(error) => fault_of(error),
    }
}

/// `tick_plan_fields(world)`.
fn tick_plan(view: &View<'_>, configs: &Map, selection: &Result<Selection>) -> Result<V> {
    let configs_value = V::Map(configs.clone());
    let psychology = selection.clone()?.get("psychology").cloned().ok_or_else(|| Error::Key("'psychology'".into()))?;
    let catalog = catalog()?;
    let declared = validate::declared::declared_states(&configs_value)?;
    let mut required: Vec<Arc<str>> =
        signatures::simulator_signatures(view)?.into_iter().filter_map(|(key, _)| key.strip_prefix("simulator:").map(Arc::from)).collect();
    required.sort_by(|first, second| first.as_bytes().cmp(second.as_bytes()));
    let mut resource_config = V::Null;
    if let Some(raw) = configs.get("resource_service") {
        let dumped = validate::models::resource_service_config(raw).map_err(validation)?;
        if !equal(raw, &dumped) {
            return Err(Error::Value("resource_service config is not canonical".into()));
        }
        resource_config = vmap([
            ("water_ml_per_service", exact_int(member(&dumped, "water_ml_per_service"))?),
            ("food_meal_units_per_service", exact_int(member(&dumped, "food_meal_units_per_service"))?),
            ("water_staff_skill", exact_str(member(&dumped, "water_staff_skill"))?),
            ("meal_staff_skill", exact_str(member(&dumped, "meal_staff_skill"))?),
        ]);
    }
    let information = information_config(configs.get("information_spread"))?;
    if let Some(raw) = configs.get("information_spread") {
        if !equal(raw, &V::Map(information.clone())) {
            return Err(Error::Value("information_spread config is not canonical".into()));
        }
    }
    let operational_enabled = match super::namespaces::operational_enabled(configs, &[]) {
        Ok(enabled) => V::Bool(enabled),
        Err(Error::Value(_)) => V::Null,
        Err(other) => return Err(other),
    };
    let mut environment_zones = V::Null;
    if let Some(raw) = configs.get("zone_environment").filter(|raw| !matches!(raw, V::Null)) {
        let canonical = contracts::checked(&Checked::ZoneEnvironment, raw, Holder::Tracked).map_err(validation)?;
        let V::Map(canonical_map) = &canonical else {
            return Err(Error::unsupported("a config's dump is no dict"));
        };
        let legacy: Map = canonical_map
            .iter()
            .filter(|(key, _)| !validate::zone_environment::LEGACY_FIELDS.contains(&&***key))
            .map(|(key, found)| (key.clone(), found.clone()))
            .collect();
        if !equal(raw, &canonical) && !equal(raw, &V::Map(legacy)) {
            return Err(Error::Value("zone_environment config is not canonical".into()));
        }
        let mut zones: Vec<Arc<str>> = match canonical_map.get("baseline_by_zone") {
            Some(V::Map(baseline)) => baseline.keys().cloned().collect(),
            _ => Vec::new(),
        };
        zones.sort_by(|first, second| first.as_bytes().cmp(second.as_bytes()));
        environment_zones = V::List(zones.into_iter().map(V::Str).collect());
    }
    if let Some(raw) = configs.get("infrastructure").filter(|raw| !matches!(raw, V::Null)) {
        if !matches!(raw, V::Map(_)) {
            return Err(Error::Value("infrastructure config is not an object".into()));
        }
        let dumped = contracts::validate(&Contract::Infrastructure, raw).map_err(validation)?;
        if !equal(raw, &dumped) {
            return Err(Error::Value("infrastructure config is not canonical".into()));
        }
    }
    let texts = |items: &[&str]| V::List(items.iter().map(|item| vstr(item)).collect());
    Ok(vmap([
        ("declared", V::List(declared.iter().map(|contract| V::List(vec![V::Str(contract.namespace.clone()), V::Int(contract.schema_version)])).collect())),
        ("required", V::List(required.into_iter().map(V::Str).collect())),
        ("independent_queues", V::Bool(signatures::independent_queues_required(configs)?)),
        ("information_enabled", V::Bool(matches!(information.get("enabled"), Some(V::Bool(true))))),
        ("mean_reverting", V::Bool(catalog.is_mean_reverting(&psychology))),
        ("shared_configured", V::Bool(configs.contains_key("shared_operations"))),
        ("operational_enabled", operational_enabled),
        ("environment_zones", environment_zones),
        ("infrastructure_configured", V::Bool(configs.get("infrastructure").is_some_and(|raw| !matches!(raw, V::Null)))),
        ("operational_events", texts(&OPERATIONAL_EVENTS)),
        ("infrastructure_events", texts(&INFRASTRUCTURE_EVENTS)),
        ("resource_config", resource_config),
    ]))
}

/// `InformationSpreadConfig.from_mapping(document).to_dict()`.
pub(crate) fn information_config(document: Option<&V>) -> Result<Map> {
    validate::information::from_mapping(document)?;
    let mut config: Map = [
        ("enabled", V::Bool(false)),
        ("max_contacts_per_agent", V::Int(2)),
        ("minimum_contact_minutes", V::Int(2)),
        ("contact_duration_scale_minutes", V::Int(5)),
        ("base_transmission_probability", V::Float(0.2)),
        ("correction_transmission_multiplier", V::Float(1.25)),
        ("trust_sensitivity", V::Float(0.5)),
        ("family_contact_multiplier", V::Float(1.5)),
        ("adjacent_zone_contact_multiplier", V::Float(0.5)),
        ("query_pressure_per_claim", V::Float(0.5)),
        ("query_pressure_decay_per_minute", V::Float(0.002)),
        ("query_pressure_relief_per_correction", V::Float(0.5)),
        ("query_threshold", V::Float(0.45)),
        ("query_utility_weight", V::Float(0.8)),
        ("correction_acceptance_probability", V::Float(0.7)),
        ("correction_trust_sensitivity", V::Float(0.2)),
        ("correction_source_sensitivity", V::Float(0.2)),
        ("rumor_belief_probability", V::Float(0.5)),
        ("rumor_trust_sensitivity", V::Float(0.2)),
        ("rumor_source_sensitivity", V::Float(0.2)),
        ("default_source_reliability", V::Float(0.7)),
        ("correction_source_reliability_by_id", empty()),
        ("distress_sharing_sensitivity", V::Float(0.5)),
        ("share_propensity_decay_per_minute", V::Float(0.002)),
        ("forgetting_rate_per_minute", V::Float(0.0)),
        ("schema_version", V::Int(1)),
        ("policy_version", vstr("sparse-information-contact-v1")),
        ("source_type", vstr("assumption")),
        ("source_id", vstr("ASM-INFORMATION-SPREAD-V1")),
    ]
    .into_iter()
    .map(|(key, found)| (text(key), found))
    .collect();
    if let Some(V::Map(document)) = document {
        for (key, found) in document {
            config.insert(key.clone(), found.clone());
        }
    }
    if let Some(V::Map(reliability)) = config.get_mut("correction_source_reliability_by_id") {
        reliability.sort_by(|first, _, second, _| first.as_bytes().cmp(second.as_bytes()));
    }
    Ok(config)
}

/// `facility_geometry_section(configs)`: the model's dump with each approach route's `distance_m`
/// and `travel_minutes`.
pub(crate) fn facility_geometry(configs: &Map) -> V {
    let Some(raw) = configs.get("facility_geometry").filter(|raw| !matches!(raw, V::Null)) else {
        return vmap([("absent", V::Bool(true))]);
    };
    let mut dumped = match contracts::validate(&Contract::FacilityGeometry, raw) {
        Ok(dumped) => dumped,
        Err(error) => return fault_of(&validation(error)),
    };
    if let V::Map(map) = &mut dumped {
        if let Some(V::List(routes)) = map.get_mut("arrival_approach_routes") {
            for route in routes {
                let V::Map(route) = route else { continue };
                let mut steps = Vec::new();
                if let Some(V::List(points)) = route.get("points_m") {
                    let coordinate = |point: &V, axis: &str| match member(point, axis) {
                        Some(V::Float(number)) => *number,
                        Some(V::Int(number)) => *number as f64,
                        _ => f64::NAN,
                    };
                    for pair in points.windows(2) {
                        steps.push(mathx::hypot(&[
                            coordinate(&pair[1], "x") - coordinate(&pair[0], "x"),
                            coordinate(&pair[1], "y") - coordinate(&pair[0], "y"),
                        ]));
                    }
                }
                let distance = if steps.is_empty() { V::Int(0) } else { V::Float(mathx::py_sum(&steps)) };
                let speed = match route.get("walking_speed_m_per_min") {
                    Some(V::Float(number)) => *number,
                    Some(V::Int(number)) => *number as f64,
                    _ => f64::NAN,
                };
                let quotient = match &distance {
                    V::Float(number) => number / speed,
                    _ => 0.0 / speed,
                };
                let minutes = if quotient.is_nan() {
                    fault("Value", "cannot convert float NaN to integer")
                } else if quotient.is_infinite() {
                    fault("builtins.OverflowError", "cannot convert float infinity to integer")
                } else {
                    let ceiled = quotient.ceil();
                    if ceiled >= 9.223_372_036_854_775_807e18 {
                        fault("sheltersim_native.Unsupported", "an int is too large")
                    } else {
                        value(V::Int((ceiled as i64).max(1)))
                    }
                };
                route.insert(text("distance_m"), distance);
                route.insert(text("travel_minutes"), minutes);
            }
        }
    }
    value(dumped)
}

/// `spatial_constraints_section(configs)`.
pub(crate) fn spatial_constraints(configs: &Map) -> V {
    let raw = configs.get("spatial_constraints").cloned().unwrap_or_else(empty);
    section_of(|| contracts::validate(&Contract::SpatialConstraints, &raw).map_err(validation))
}

/// `_staff_operations(configs)`.
fn staff_operations(configs: &Map) -> V {
    parsed(|| {
        let registry = configs.get("policy_registry");
        let selected = registry.and_then(|registry| member(registry, "staff_operations"));
        let model = match selected {
            None => Arc::from(STAFF_OPERATIONS_DEFAULT),
            Some(V::Str(model)) => model.clone(),
            Some(other) => return Err(Error::Key(format!("('staff_operations', {})", crate::validate::repr(other)))),
        };
        let known = ["staff-urgency-first-v1", "staff-travel-first-v1"];
        if !known.contains(&&*model) {
            return Err(Error::Key(format!("('staff_operations', {})", crate::text::py_repr(&model))));
        }
        let source =
            catalog()?.models.get(&model).map(|definition| definition.assumption_source_id.clone()).ok_or_else(|| Error::Key(crate::text::py_repr(&model)))?;
        Ok(vmap([("model_id", V::Str(model)), ("selected_source", if selected.is_some() { V::Str(source) } else { V::Null })]))
    })
}

/// `_care_outcome(configs)`.
fn care_outcome(configs: &Map, selection: &Result<Selection>) -> V {
    parsed(|| {
        let Ok(selection) = selection else {
            return Err(refused("the formula refuses"));
        };
        let Some(model) = selection.get("care_outcome") else {
            return Err(refused("an input has no such attribute"));
        };
        let catalog = catalog()?;
        let Some(definition) = catalog.models.get(model).filter(|definition| &*definition.domain == "care_outcome") else {
            return Err(refused("the formula refuses"));
        };
        let config = configs.get("body_dynamics").cloned().unwrap_or_else(empty);
        Ok(vmap([("model_id", V::Str(model.clone())), ("source_id", V::Str(definition.assumption_source_id.clone())), ("config", config)]))
    })
}

/// `_body_models(world, selection)`.
fn body_models(configs: &Map, selection: &Result<Selection>) -> V {
    let selection = match selection {
        Ok(selection) => selection,
        Err(Error::Unsupported(_)) => return vmap([("host_error", vstr("not a selection"))]),
        Err(_) => return vmap([("host_error", vstr("the model selection raises"))]),
    };
    let affect = match selection.get("affect") {
        Some(model) => V::Str(model.clone()),
        None => {
            let raw = configs.get("body_dynamics").cloned().unwrap_or(V::Null);
            match validate::body_state::canonical_body_config(&raw) {
                Ok(config) => match config.get("affect_policy") {
                    Some(V::Str(affect)) => V::Str(affect.clone()),
                    _ => return fault("Refused", "a value is not a str"),
                },
                Err(_) => return fault("Refused", "the body config raises"),
            }
        }
    };
    let relationship = selection.get("relationship").map_or(V::Null, |model| V::Str(model.clone()));
    value(vmap([("affect", affect), ("relationship", relationship)]))
}

/// `_validated(configs, key, contract)`.
fn validated(configs: &Map, key: &str, contract: Contract) -> V {
    parsed(|| {
        let raw = configs.get(key).ok_or_else(|| Error::Key(crate::text::py_repr(key)))?;
        contracts::validate(&contract, raw).map_err(validation)
    })
}

/// `_proximity(configs)`: `canonical_proximity_contact_config(configs.get("social_proximity"))`.
fn proximity(configs: &Map) -> V {
    parsed(|| {
        Ok(match signatures::canonical_proximity(configs.get("social_proximity"))? {
            None => V::Null,
            Some((maximum, interval, pairs)) => vmap([
                ("schema_version", V::Int(1)),
                ("enabled", V::Bool(true)),
                ("policy_version", vstr("distance-sampled-proximity-v1")),
                ("sample_interval_minutes", V::Int(interval)),
                ("max_distance_m", V::Float(maximum)),
                ("max_pairs_per_sample", V::Int(pairs)),
            ]),
        })
    })
}

/// `_overlay_rooms(configs)`.
fn overlay_rooms(configs: &Map) -> V {
    let config = configs.get("geometry_overlay").cloned().unwrap_or_else(empty);
    let is_dict = matches!(config, V::Map(_));
    let schema = if is_dict {
        match member(&config, "schema_version") {
            Some(V::Int(3 | 4)) => value(V::Bool(true)),
            Some(V::Float(number)) if *number == 3.0 || *number == 4.0 => value(V::Bool(true)),
            Some(V::List(_)) => fault("Type", "unhashable type: 'list'"),
            Some(V::Map(_)) => fault("Type", "unhashable type: 'dict'"),
            _ => value(V::Bool(false)),
        }
    } else {
        value(V::Bool(false))
    };
    value(vmap([("is_dict", V::Bool(is_dict)), ("room_schema", schema)]))
}

/// `_extensions_enabled(configs)`.
fn extensions_enabled(configs: &Map) -> V {
    parsed(|| {
        Ok(V::Bool(match configs.get("simulator_extensions") {
            None => false,
            Some(V::Map(extensions)) => extensions.get("enabled").is_some_and(|enabled| !matches!(enabled, V::Null) && enabled.is_truthy()),
            Some(_) => return Err(Error::Attribute("the extensions config has no get".into())),
        }))
    })
}

/// `sections(world)`: every section Python parses, by its field of `Configs`.
pub(crate) fn sections(world: &World) -> Result<V> {
    let view = View::new(world, true, None);
    let configs = match &*view.configs()? {
        V::Map(configs) => configs.clone(),
        _ => return Err(Error::unsupported("model_configs is not a dict")),
    };
    let selection = selection::select(&V::Map(configs.clone()));
    let support_cooldown = match configs.get("staff") {
        None => V::Int(30),
        Some(V::Map(staff)) => staff.get("support_cooldown_minutes").cloned().unwrap_or(V::Int(30)),
        Some(_) => V::Null,
    };
    let transit = match configs.get("layout_routing") {
        None => false,
        Some(V::Map(routing)) => routing.get("max_agents_in_transit_by_edge").is_some_and(V::is_truthy),
        Some(_) => false,
    };
    let information = parsed(|| {
        let config = information_config(configs.get("information_spread"))?;
        let mut fields: Map = INFORMATION_FIELDS.iter().map(|name| (text(name), config.get(*name).cloned().unwrap_or(V::Null))).collect();
        fields.insert(text("present"), V::Bool(configs.contains_key("information_spread")));
        Ok(V::Map(fields))
    });
    Ok(vmap([
        ("selection", selection_section(&selection)),
        ("tick_plan", section_of(|| tick_plan(&view, &configs, &selection))),
        ("facility_geometry", facility_geometry(&configs)),
        ("spatial_constraints", spatial_constraints(&configs)),
        ("staff_operations", staff_operations(&configs)),
        ("support_cooldown_minutes", parsed(|| exact_int(Some(&support_cooldown)))),
        ("information_spread", information),
        ("body_models", body_models(&configs, &selection)),
        ("care_outcome", care_outcome(&configs, &selection)),
        ("zone_environment", validated(&configs, "zone_environment", Contract::ZoneEnvironment)),
        ("infrastructure", validated(&configs, "infrastructure", Contract::Infrastructure)),
        ("proximity", proximity(&configs)),
        ("transit_capacities", value(V::Bool(transit))),
        ("overlay_rooms", overlay_rooms(&configs)),
        ("extensions_enabled", extensions_enabled(&configs)),
    ]))
}

/// The floor plan of the configs with no overlay (`Geo::of` of a world at minute 0, before its
/// `geometry_overlay_v1`): the facility geometry, with the spatial constraints where the configs
/// have them.
pub(crate) fn floor_plan(configs: &Map) -> Result<Arc<Geo>> {
    let json = |found: &V| -> Result<serde_json::Value> { serde_json::from_slice(&found.to_json()?).map_err(|error| Error::unsupported(error.to_string())) };
    let facility = json(&facility_geometry(configs))?;
    let spatial = if configs.contains_key("spatial_constraints") { Some(json(&spatial_constraints(configs))?) } else { None };
    crate::load::floor_plan_of(&facility, spatial.as_ref())
}
