//! The Run's policies at minute 0, the document of an offline controller (no LLM router, traces
//! off), made from the configs as the controller makes its policies, managers and event handlers
//! (#429). The document is what `crate::load::policies` reads: the controller's object with the
//! attributes of `run::policies::CONTROLLER`, each `{"value": ...}`, `{"text": ...}`,
//! `{"object": ...}`, `{"method": ...}` or the `{"fault": [class, text]}` of an attribute the
//! object lacks; and the dispatch table of the event handlers (`run::dispatch`).
//!
//! The Scenario is canonical (`ScenarioSpec` dumped the psychology, resource service and toilet
//! configs it holds): a config the controller would turn down with an error of its own raises that
//! error; one only a contract run first could turn down is a refusal.

use std::sync::Arc;

use indexmap::IndexMap;

use super::{text, vmap, vstr};
use crate::config::Selection;
use crate::error::{Error, Result};
use crate::validate::catalog::catalog;
use crate::validate::{repr, selection, signatures, View};
use crate::value::V;
use crate::world::World;

type Map = IndexMap<Arc<str>, V>;

fn value_error(text: impl Into<String>) -> Error {
    Error::Value(text.into())
}

fn refused(text: &'static str) -> Error {
    Error::unsupported(text)
}

/// `{"value": ...}`.
fn value(found: V) -> V {
    vmap([("value", found)])
}

/// The fault of `getattr` of an attribute the object lacks.
fn missing(class: &str, attr: &str) -> V {
    vmap([("fault", V::List(vec![vstr("builtins.AttributeError"), vstr(&format!("'{class}' object has no attribute '{attr}'"))]))])
}

/// An object as read: its class (by module and name, which find it), its attributes, and the
/// names its class's own `__dict__` has.
fn object_of(module: &str, class: &str, attrs: Vec<(&str, V)>, class_own: Vec<(&str, V)>) -> V {
    let named = |items: Vec<(&str, V)>| V::Map(items.into_iter().map(|(name, item)| (text(name), item)).collect());
    vmap([
        ("class", V::List(vec![vstr(module), vstr(class), V::Bool(true)])),
        ("attrs", named(attrs)),
        ("class_own", named(class_own)),
        ("own", V::Map(Map::new())),
    ])
}

fn object(module: &str, class: &str, attrs: Vec<(&str, V)>) -> V {
    vmap([("object", object_of(module, class, attrs, Vec::new()))])
}

/// The fields of a dataclass, each `{"value": ...}`, with these attributes first.
fn fields(first: Vec<(&'static str, V)>, all: &[(&'static str, V)]) -> Vec<(&'static str, V)> {
    let mut attrs = first;
    for (name, found) in all {
        if !attrs.iter().any(|(known, _)| known == name) {
            attrs.push((name, value(found.clone())));
        }
    }
    attrs
}

fn get<'a>(configs: &'a Map, key: &str) -> Option<&'a Map> {
    match configs.get(key) {
        Some(V::Map(map)) => Some(map),
        _ => None,
    }
}

/// `configs.get(section, {}).get(key, default)`.
fn setting(configs: &Map, section: &str, key: &str, default: V) -> V {
    get(configs, section).and_then(|map| map.get(key)).cloned().unwrap_or(default)
}

fn number(found: &V) -> Option<f64> {
    match found {
        V::Int(number) => Some(*number as f64),
        V::Float(number) => Some(*number),
        _ => None,
    }
}

// ---- Behavior ----------------------------------------------------------------------------------

/// `behavior_policy_definition(variant, ...)["policy_version"]`.
fn behavior_version(variant: &str, resource_service: bool, autonomy: bool) -> Result<String> {
    let base = match variant {
        "utility-softmax-v1" | "sleep-aware-utility-softmax-v2" | "priority-rule-v1" => variant,
        _ => return Err(value_error(format!("unknown behavior policy variant {}", crate::text::py_repr(variant)))),
    };
    let mut version = if autonomy { format!("{}-autonomy-v2", base.strip_suffix("-v1").unwrap_or(base)) } else { base.to_owned() };
    if resource_service {
        version = format!("{}-resource-service-v1", version.strip_suffix("-v1").unwrap_or(&version));
    }
    Ok(version)
}

/// `UtilityBehaviorPolicy.__post_init__`.
fn check_behavior(parameters: &[(&'static str, V)]) -> Result<()> {
    let field = |name: &str| parameters.iter().find(|(known, _)| *known == name).map(|(_, found)| found.clone()).unwrap_or(V::Null);
    let float = |name: &str| number(&field(name)).ok_or_else(|| refused("a behavior coefficient is not a number"));
    let temperature = float("temperature")?;
    if !temperature.is_finite() || temperature <= 0.0 {
        return Err(value_error("temperature must be positive and finite"));
    }
    if float("decision_interval_minutes")? <= 0.0 {
        return Err(value_error("decision interval must be positive"));
    }
    let weight = float("sleep_debt_utility_weight")?;
    if !weight.is_finite() || weight < 0.0 {
        return Err(value_error("sleep debt utility weight must be finite and non-negative"));
    }
    let (V::Map(intervals), V::Map(temperatures)) = (field("decision_interval_minutes_by_agent"), field("temperature_by_agent")) else {
        return Err(refused("the per-Agent behavior coefficients are not dicts"));
    };
    if intervals.len() > 100 || temperatures.len() > 100 {
        return Err(value_error("per-Agent behavior coefficients exceed the 100-resident limit"));
    }
    let id_ok = |id: &str| !id.is_empty() && id.chars().count() <= 64;
    if intervals.iter().any(|(id, minutes)| !id_ok(id) || !matches!(minutes, V::Int(minutes) if (1..=1440).contains(minutes))) {
        return Err(value_error("per-Agent decision intervals must be integer minutes in [1, 1440]"));
    }
    if temperatures.iter().any(|(id, found)| !id_ok(id) || !number(found).is_some_and(|value| value.is_finite() && 0.0 < value && value <= 10.0)) {
        return Err(value_error("per-Agent choice temperatures must be finite values in (0, 10]"));
    }
    for name in [
        "hydration_action_threshold",
        "hunger_action_threshold",
        "water_utility_base",
        "water_utility_need_weight",
        "meal_utility_base",
        "meal_utility_need_weight",
        "information_query_threshold",
        "information_query_utility_weight",
    ] {
        let found = float(name)?;
        if !found.is_finite() || found < 0.0 {
            return Err(value_error(format!("{name} must be finite and non-negative")));
        }
        let (maximum, written) = if name == "information_query_threshold" { (1.1, "1.1") } else { (1.0, "1") };
        if name.ends_with("threshold") && found > maximum {
            return Err(value_error(format!("{name} must be between zero and {written}")));
        }
    }
    Ok(())
}

/// The controller's `decisions`: `BehaviorDecisionRunner(create_behavior(configs, **parameters))`.
fn decisions(configs: &Map, selection: &Selection, world: &World, information: &Map) -> Result<V> {
    let interval = setting(configs, "behavior", "decision_interval_minutes", V::Int(15));
    if !matches!(interval, V::Int(minutes) if minutes > 0) {
        return Err(value_error("behavior decision_interval_minutes must be a positive integer"));
    }
    let temperature = setting(configs, "behavior", "temperature", V::Float(0.2));
    if !matches!(temperature, V::Int(_) | V::Float(_)) {
        return Err(value_error("behavior temperature must be numeric"));
    }
    let resource_service = world.schema_version()? == 6;
    let variant = match setting(configs, "behavior", "policy", vstr("utility-softmax-v1")) {
        V::Str(variant) => variant,
        _ => return Err(value_error("behavior policy must be a registered string variant")),
    };
    let extensions = get(configs, "simulator_extensions").and_then(|map| map.get("enabled")).is_some_and(V::is_truthy);
    let autonomy = extensions || configs.contains_key("population_context") || View::new(world, true, None).contains("body_state_v1")?;
    let enabled = matches!(information.get("enabled"), Some(V::Bool(true)));
    let service = |key: &str, default: f64| setting(configs, "resource_service", key, V::Float(default));
    let version = behavior_version(&variant, resource_service, autonomy)?;
    let selected = selection.get("behavior").ok_or_else(|| refused("the selection has no behavior"))?;
    if **selected != *variant {
        return Err(refused("the behavior selection is not the configured policy"));
    }
    let parameters: Vec<(&'static str, V)> = vec![
        ("temperature", temperature),
        ("decision_interval_minutes", interval),
        ("decision_interval_minutes_by_agent", setting(configs, "behavior", "decision_interval_minutes_by_agent", V::Map(Map::new()))),
        ("temperature_by_agent", setting(configs, "behavior", "temperature_by_agent", V::Map(Map::new()))),
        ("sleep_debt_utility_weight", setting(configs, "behavior", "sleep_debt_utility_weight", V::Float(1.0))),
        ("policy_version", vstr(&version)),
        ("resource_service_enabled", V::Bool(resource_service)),
        ("hydration_action_threshold", service("hydration_action_threshold", 0.25)),
        ("hunger_action_threshold", service("hunger_action_threshold", 0.7)),
        ("water_utility_base", service("water_utility_base", 0.18)),
        ("water_utility_need_weight", service("water_utility_need_weight", 1.0)),
        ("meal_utility_base", service("meal_utility_base", 0.18)),
        ("meal_utility_need_weight", service("meal_utility_need_weight", 0.85)),
        ("information_query_threshold", if enabled { information.get("query_threshold").cloned().unwrap_or(V::Null) } else { V::Float(1.1) }),
        ("information_query_utility_weight", if enabled { information.get("query_utility_weight").cloned().unwrap_or(V::Null) } else { V::Float(0.0) }),
        ("autonomy_enabled", V::Bool(autonomy)),
    ];
    check_behavior(&parameters)?;
    const MODULE: &str = "sheltersim.policies.behavior";
    const UTILITY: &str = "UtilityBehaviorPolicy";
    let policy = if &*variant == "priority-rule-v1" {
        let scorer = object(MODULE, UTILITY, fields(Vec::new(), &parameters));
        object(MODULE, "PriorityRuleBehaviorPolicy", vec![("policy_version", value(vstr(&version))), ("scorer", scorer)])
    } else {
        object(MODULE, UTILITY, fields(vec![("scorer", missing(UTILITY, "scorer"))], &parameters))
    };
    Ok(object(MODULE, "BehaviorDecisionRunner", vec![("policy", policy)]))
}

// ---- Psychology --------------------------------------------------------------------------------

/// The fields of `PsychologyParameters`, with their defaults, in its order.
fn psychology_defaults() -> Vec<(&'static str, V)> {
    let empty = || V::Map(Map::new());
    vec![
        ("toilet_growth", V::Float(0.001)),
        ("hunger_growth", V::Float(0.0005)),
        ("awake_fatigue_growth", V::Float(0.0008)),
        ("resting_fatigue_recovery", V::Float(0.004)),
        ("sleeping_fatigue_recovery", V::Float(0.010)),
        ("stress_pressure", V::Float(0.012)),
        ("queued_stress", V::Float(0.002)),
        ("resting_stress_recovery", V::Float(0.003)),
        ("sleeping_stress_recovery", V::Float(0.006)),
        ("need_pressure_threshold", V::Float(0.35)),
        ("hydration_growth", V::Float(0.0)),
        ("support_wait_stress", V::Float(0.001)),
        ("toilet_growth_by_agent", empty()),
        ("hunger_growth_by_agent", empty()),
        ("awake_fatigue_growth_by_agent", empty()),
        ("natural_recovery_rate_per_minute", V::Float(0.01)),
        ("natural_recovery_rate_by_agent", empty()),
        ("baseline_distress_by_agent", empty()),
        ("chronic_retention", V::Float(0.90)),
        ("chronic_acute_weight", V::Float(0.08)),
        ("chronic_high_distress_weight", V::Float(0.02)),
        ("high_distress_threshold", V::Float(0.70)),
    ]
}

/// `PsychologyParameters(**PsychologyModelConfig.model_validate(raw).coefficient_values())`, with
/// the resource service's hydration growth: the canonical config's values over the defaults.
fn psychology_parameters(configs: &Map, resource_service: bool) -> Result<Vec<(&'static str, V)>> {
    let mut parameters = psychology_defaults();
    let raw = get(configs, "psychology").cloned().unwrap_or_default();
    for (key, found) in &raw {
        if matches!(&**key, "schema_version" | "policy_version" | "source_type" | "source_id") {
            continue;
        }
        let Some(slot) = parameters.iter_mut().find(|(name, _)| *name == &**key) else {
            return Err(refused("a psychology config the model does not have"));
        };
        let coerced = match (&slot.1, found) {
            (_, V::Null) => continue,
            (V::Map(_), V::Map(map)) if map.is_empty() => continue,
            (V::Map(_), V::Map(_)) => found.clone(),
            (V::Float(_), V::Float(_)) => found.clone(),
            (V::Float(_), V::Int(number)) => V::Float(*number as f64),
            _ => return Err(refused("a psychology coefficient the model does not take")),
        };
        slot.1 = coerced;
    }
    if resource_service {
        let growth = setting(configs, "resource_service", "hydration_growth_per_minute", V::Float(0.0));
        if let Some(slot) = parameters.iter_mut().find(|(name, _)| *name == "hydration_growth") {
            slot.1 = growth;
        }
    }
    Ok(parameters)
}

fn psychology(configs: &Map, selection: &Selection, resource_service: bool) -> Result<V> {
    const MODULE: &str = "sheltersim.policies.psychology";
    let selected = selection.get("psychology").ok_or_else(|| refused("the selection has no psychology"))?;
    let parameters = object(MODULE, "PsychologyParameters", fields(Vec::new(), &psychology_parameters(configs, resource_service)?));
    let simple = |module: &str, class: &str, version: &str, source: &str| {
        object(
            module,
            class,
            vec![
                ("policy_version", value(vstr(version))),
                ("source_id", value(vstr(source))),
                ("parameters", parameters.clone()),
                ("include_direct_rest_stress_recovery", value(V::Bool(true))),
                ("use_tick_start_stress_for_feedback", missing(class, "use_tick_start_stress_for_feedback")),
                ("base", missing(class, "base")),
                ("body_config", missing(class, "body_config")),
                ("body_formula", missing(class, "body_formula")),
                ("update_long_term_state", missing(class, "update_long_term_state")),
            ],
        )
    };
    Ok(match &**selected {
        "causal-stress-v1" => simple(MODULE, "SimplePsychologyPolicy", "causal-stress-v1", "ASM-H04-CAUSAL-STRESS-V1"),
        "bounded-stress-v1" => simple("sheltersim.policies.alternatives", "BoundedStressPolicy", "bounded-stress-v1", "ASM-BOUNDED-STRESS-V1"),
        "mean-reverting-distress-v1" => simple(MODULE, "MeanRevertingDistressPolicy", "mean-reverting-distress-v1", "ASM-MEAN-REVERTING-ACUTE-DISTRESS-V1"),
        life @ ("life-state-feedback-v1" | "mean-reverting-life-state-v1") => {
            let integrated = life == "mean-reverting-life-state-v1";
            let (class, source) = if integrated {
                ("IntegratedDistressLifeStatePolicy", "ASM-MEAN-REVERTING-LIFE-STATE-V1")
            } else {
                ("LifeStatePsychologyPolicy", "ASM-LIFE-STATE-FEEDBACK-V1")
            };
            let raw = configs.get("body_dynamics").cloned().unwrap_or(V::Null);
            let body_config = crate::validate::body_state::canonical_body_config(&raw)?;
            let formula = selection.get("body_dynamics").map_or_else(|| text("body-feedback-complete-v1"), Arc::clone);
            object(
                MODULE,
                class,
                vec![
                    ("policy_version", value(vstr(life))),
                    ("source_id", value(vstr(source))),
                    ("parameters", parameters.clone()),
                    ("include_direct_rest_stress_recovery", missing(class, "include_direct_rest_stress_recovery")),
                    ("use_tick_start_stress_for_feedback", value(V::Bool(integrated))),
                    ("base", object(MODULE, "SimplePsychologyPolicy", vec![("include_direct_rest_stress_recovery", value(V::Bool(!integrated)))])),
                    ("body_config", value(V::Map(body_config))),
                    ("body_formula", object("sheltersim.policies.registry", "BodyDynamicsFormula", vec![("model_id", value(V::Str(formula)))])),
                    ("update_long_term_state", missing(class, "update_long_term_state")),
                ],
            )
        }
        _ => return Err(refused("a psychology model the core does not make")),
    })
}

// ---- Social propagation ------------------------------------------------------------------------

const SOCIAL_SOURCE: &str = "ASM-SOCIAL-PROPAGATION-V1";
const BOSSE_SOURCE: &str = "ASM-BOSSE-AFFECT-CONTAGION-V1";
const DEGROOT_SOURCE: &str = "ASM-DEGROOT-CONTACT-V1";

/// The fields of `SocialPropagationParameters`, with their defaults, in its order.
fn social_defaults() -> Vec<(&'static str, V)> {
    vec![
        ("enabled", V::Bool(false)),
        ("max_contacts_per_agent", V::Int(3)),
        ("minimum_contact_minutes", V::Int(5)),
        ("stress_spread_per_minute", V::Float(0.001)),
        ("natural_recovery_per_minute", V::Float(0.0005)),
        ("cooldown_minutes", V::Int(30)),
        ("max_pair_stress_increase", V::Float(0.05)),
        ("max_agent_stress_increase_per_minute", V::Float(0.05)),
        ("source_expressiveness_by_agent", V::Map(Map::new())),
        ("family_channel_strength", V::Float(1.0)),
        ("same_zone_channel_strength", V::Float(0.75)),
        ("nearby_zone_channel_strength", V::Float(0.5)),
        ("source_id", vstr(SOCIAL_SOURCE)),
    ]
}

/// `SocialPropagationParameters.from_mapping(document, source_id=source)`.
pub(crate) fn social_parameters(document: Option<&V>, source: &str) -> Result<Vec<(&'static str, V)>> {
    let mut parameters = social_defaults();
    let document = match document {
        None | Some(V::Null) => return Ok(parameters),
        Some(V::Map(document)) => document,
        Some(_) => return Err(value_error("social_propagation model config must be an object")),
    };
    let allowed = |key: &str| parameters.iter().any(|(name, _)| *name == key);
    let mut unknown: Vec<&str> = document.keys().map(|key| &**key).filter(|key| !allowed(key) && !matches!(*key, "source_type" | "source_id")).collect();
    if !unknown.is_empty() {
        unknown.sort();
        return Err(value_error(format!("unknown social_propagation settings: {}", crate::validate::repr_texts(unknown))));
    }
    if document.get("source_type").is_some_and(|found| !matches!(found, V::Null) && !matches!(found, V::Str(kind) if &**kind == "assumption")) {
        return Err(value_error("social_propagation source_type must be assumption"));
    }
    let configured = document.get("source_id").filter(|found| !matches!(found, V::Null));
    let known_source = |found: &V| matches!(found, V::Str(id) if [SOCIAL_SOURCE, BOSSE_SOURCE, DEGROOT_SOURCE].contains(&&**id));
    if configured.is_some_and(|found| !known_source(found)) {
        return Err(value_error("social_propagation source_id does not match its model assumption"));
    }
    if configured.is_some_and(|found| !matches!(found, V::Str(id) if &**id == source || &**id == SOCIAL_SOURCE)) {
        return Err(value_error("social_propagation source_id must match the selected social model"));
    }
    for (key, found) in document {
        if let Some(slot) = parameters.iter_mut().find(|(name, _)| *name == &**key) {
            slot.1 = found.clone();
        }
    }
    if let Some(slot) = parameters.iter_mut().find(|(name, _)| *name == "source_id") {
        slot.1 = vstr(source);
    }
    // `__post_init__`.
    let field = |name: &str| parameters.iter().find(|(known, _)| *known == name).map(|(_, found)| found.clone()).unwrap_or(V::Null);
    if !matches!(field("enabled"), V::Bool(_)) {
        return Err(value_error("social propagation enabled must be boolean"));
    }
    for (name, minimum, maximum) in [("max_contacts_per_agent", 0, 16), ("minimum_contact_minutes", 1, 10_080), ("cooldown_minutes", 1, 10_080)] {
        let V::Int(found) = field(name) else {
            return Err(value_error(format!("{name} must be an integer")));
        };
        if !(minimum..=maximum).contains(&found) {
            return Err(value_error(format!("{name} must be between {minimum} and {maximum}")));
        }
    }
    let unit = |found: &V, upper: f64| number(found).is_some_and(|value| value.is_finite() && (0.0..=upper).contains(&value));
    for name in ["stress_spread_per_minute", "natural_recovery_per_minute", "max_pair_stress_increase", "max_agent_stress_increase_per_minute"] {
        if !unit(&field(name), 0.25) {
            return Err(value_error(format!("{name} must be finite and between 0 and 0.25")));
        }
    }
    for name in ["family_channel_strength", "same_zone_channel_strength", "nearby_zone_channel_strength"] {
        if !unit(&field(name), 1.0) {
            return Err(value_error(format!("{name} must be finite and between 0 and 1")));
        }
    }
    let expressiveness_ok = match field("source_expressiveness_by_agent") {
        V::Map(map) => map.len() <= 10_000 && map.iter().all(|(id, found)| !id.is_empty() && unit(found, 1.0)),
        _ => false,
    };
    if !expressiveness_ok {
        return Err(value_error("source_expressiveness_by_agent values must be within 0..1"));
    }
    Ok(parameters)
}

fn stress(configs: &Map, selection: &Selection) -> Result<V> {
    let selected = selection.get("social").ok_or_else(|| refused("the selection has no social model"))?;
    let (module, class) = match &**selected {
        "sparse-family-zone-contact-v1" => ("sheltersim.policies.social", "StressPropagationPolicy"),
        "degroot-contact-v1" => ("sheltersim.policies.alternatives", "DeGrootContactPolicy"),
        "expressiveness-channel-openness-v1" => ("sheltersim.policies.social", "ExpressivenessChannelOpennessPolicy"),
        _ => return Err(refused("a social model the core does not make")),
    };
    let source =
        catalog()?.models.get(selected).map(|definition| definition.assumption_source_id.clone()).ok_or_else(|| Error::Key(repr(&V::Str(selected.clone()))))?;
    let mut parameters = social_parameters(configs.get("social_propagation"), &source)?;
    if selection.get("psychology").is_some_and(|model| matches!(&**model, "mean-reverting-distress-v1" | "mean-reverting-life-state-v1")) {
        if let Some(slot) = parameters.iter_mut().find(|(name, _)| *name == "natural_recovery_per_minute") {
            slot.1 = V::Float(0.0);
        }
    }
    Ok(object(
        module,
        class,
        vec![
            ("policy_version", value(V::Str(selected.clone()))),
            ("parameters", object("sheltersim.policies.social", "SocialPropagationParameters", fields(Vec::new(), &parameters))),
        ],
    ))
}

// ---- Information, proximity and the services ---------------------------------------------------

fn information_policy(selection: &Selection, information: &Map) -> Result<V> {
    if !matches!(information.get("enabled"), Some(V::Bool(true))) {
        return Ok(value(V::Null));
    }
    let selected = selection.get("information").ok_or_else(|| refused("the selection has no information model"))?;
    let (module, class) = match &**selected {
        "sparse-information-contact-v1" => ("sheltersim.policies.information", "InformationSpreadPolicy"),
        "hazard-information-v1" => ("sheltersim.policies.alternatives", "HazardInformationPolicy"),
        "evidence-aware-rumor-v1" => ("sheltersim.policies.information", "EvidenceAwareInformationPolicy"),
        _ => return Err(refused("an information model the core does not make")),
    };
    let config: Vec<(&str, V)> = information.iter().map(|(name, found)| (&**name, value(found.clone()))).collect();
    Ok(object(
        module,
        class,
        vec![("policy_version", value(V::Str(selected.clone()))), ("config", object("sheltersim.domain.models", "InformationSpreadConfig", config))],
    ))
}

/// The controller's `proximity_policy` and `proximity_contact_config`.
fn proximity(configs: &Map, selection: &Selection, world: &World) -> Result<(V, V)> {
    const MODULE: &str = "sheltersim.policies.proximity";
    let canonical = |raw: Option<&V>| -> Result<V> {
        Ok(match signatures::canonical_proximity(raw)? {
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
    };
    let config = match selection.get("social_proximity").map(|model| &**model) {
        Some("disabled-proximity-v1") => V::Null,
        Some("distance-sampled-proximity-v1") => {
            let raw = configs.get("social_proximity").filter(|raw| raw.is_truthy()).cloned().unwrap_or_else(|| {
                vmap([
                    ("schema_version", V::Int(1)),
                    ("enabled", V::Bool(true)),
                    ("policy_version", vstr("distance-sampled-proximity-v1")),
                    ("sample_interval_minutes", V::Int(15)),
                    ("max_distance_m", V::Float(1.5)),
                    ("max_pairs_per_sample", V::Int(500)),
                ])
            });
            let config = canonical(Some(&raw))?;
            if matches!(config, V::Null) {
                return Err(value_error("selected distance proximity policy requires an enabled config"));
            }
            config
        }
        Some(_) => return Err(value_error("requested model has no installed factory")),
        None => canonical(configs.get("social_proximity"))?,
    };
    let policy = if matches!(config, V::Null) {
        object(MODULE, "DisabledProximityPolicy", vec![("config", missing("DisabledProximityPolicy", "config"))])
    } else {
        let view = View::new(world, true, None);
        if !view.contains("movement_v1")? || !view.contains("body_state_v1")? || !view.contains("social_network_v1")? {
            return Err(value_error("social_proximity requires installed movement and social state"));
        }
        object(MODULE, "DistanceSampledProximityPolicy", vec![("config", value(config.clone()))])
    };
    Ok((policy, value(config)))
}

/// `ToiletServiceManager(world, ...)` of `ToiletServiceConfig.model_validate(toilet_service)`.
fn toilet_service(configs: &Map) -> Result<V> {
    let minimum = setting(configs, "toilet_service", "minimum_minutes", V::Int(4));
    let maximum = setting(configs, "toilet_service", "maximum_minutes", V::Int(8));
    let (V::Int(low), V::Int(high)) = (&minimum, &maximum) else {
        return Err(refused("a toilet service config ToiletServiceConfig refuses"));
    };
    if !(1..=1440).contains(low) || !(1..=1440).contains(high) || high < low {
        return Err(refused("a toilet service config ToiletServiceConfig refuses"));
    }
    Ok(object(
        "sheltersim.domain.facilities",
        "ToiletServiceManager",
        vec![("policy_version", value(vstr("toilet-service-uniform-v1"))), ("min_service_minutes", value(minimum)), ("max_service_minutes", value(maximum))],
    ))
}

/// `StaffService(world, ...)` of `model_configs["staff"]`: its values and their checks.
fn staff_service(configs: &Map) -> Result<(V, i64)> {
    let task = setting(configs, "staff", "support_task_minutes", V::Int(10));
    let unresolved = setting(configs, "staff", "unresolved_after_minutes", V::Int(90));
    let cooldown = setting(configs, "staff", "support_cooldown_minutes", V::Int(30));
    let fairness = match setting(configs, "staff", "support_fairness_minutes", V::Null) {
        V::Null => V::Int(match unresolved {
            V::Int(minutes) if minutes > 0 => (minutes - 1).clamp(1, 30),
            _ => 30,
        }),
        other => other,
    };
    for (name, found) in
        [("task_duration_minutes", &task), ("unresolved_after_minutes", &unresolved), ("cooldown_minutes", &cooldown), ("fairness_after_minutes", &fairness)]
    {
        if !matches!(found, V::Int(minutes) if *minutes >= 0) {
            return Err(value_error(format!("{name} must be a non-negative integer")));
        }
    }
    let minutes = |found: &V| if let V::Int(minutes) = found { *minutes } else { 0 };
    if minutes(&task) == 0 || minutes(&unresolved) == 0 {
        return Err(value_error("task duration and unresolved timeout must be positive"));
    }
    if minutes(&fairness) == 0 || minutes(&fairness) > minutes(&unresolved) {
        return Err(value_error("fairness promotion must occur before or at the request timeout"));
    }
    let cooldown_minutes = minutes(&cooldown);
    let service = object(
        "sheltersim.domain.staff",
        "StaffService",
        vec![
            ("policy_version", value(vstr("staff-support-fifo-aging-v2"))),
            ("task_duration_minutes", value(task)),
            ("unresolved_after_minutes", value(unresolved)),
            ("cooldown_minutes", value(cooldown)),
            ("fairness_after_minutes", value(fairness)),
            ("route_resolver", vmap([("method", vstr("_route"))])),
            ("travel_scheduler", vmap([("method", vstr("_schedule_staff_support_travel"))])),
        ],
    );
    Ok((service, cooldown_minutes))
}

fn resource_service(configs: &Map, world: &World) -> Result<V> {
    if world.schema_version()? != 6 {
        return Ok(value(V::Null));
    }
    let view = View::new(world, true, None);
    if !view.contains("resource_service_v1")? || !view.contains("resource_ledger_v1")? {
        return Err(value_error("WorldState v6 requires resource service and ledger state"));
    }
    let config = configs.get("resource_service").cloned().ok_or_else(|| value_error("ResourceServiceManager requires an opted-in WorldState v6"))?;
    Ok(object(
        "sheltersim.domain.resource_service",
        "ResourceServiceManager",
        vec![("policy_version", value(vstr("resource-service-v1"))), ("config", value(config))],
    ))
}

// ---- The dispatch table --------------------------------------------------------------------------

/// The handlers `OfflineSimulationController.__init__` installs, with their modules (under
/// `sheltersim.events.`), classes and attributes (`manager` stands for a service manager the
/// handler holds, `cooldown` for the staff service's cooldown).
const HANDLERS: [(&str, &str, &str); 47] = [
    ("agent_travel_complete", "handlers", "AgentTravelCompletionHandler"),
    ("delivery_delayed", "handlers", "DeliveryDelayHandler"),
    ("effect_program_apply", "effects", "EffectProgramFacilityHandler"),
    ("effect_program_environment", "effects", "EffectProgramEnvironmentHandler"),
    ("effect_program_environment_expire", "effects", "EffectProgramEnvironmentHandler"),
    ("effect_program_expire", "effects", "EffectProgramFacilityHandler"),
    ("effect_program_geometry", "effects", "EffectProgramGeometryHandler"),
    ("effect_program_geometry_expire", "effects", "EffectProgramGeometryHandler"),
    ("effect_program_population", "handlers", "EffectProgramPopulationHandler"),
    ("effect_program_structure", "effects", "EffectProgramStructureHandler"),
    ("effect_program_temporary_space", "effects", "EffectProgramTemporarySpaceHandler"),
    ("effect_program_temporary_space_expire", "effects", "EffectProgramTemporarySpaceHandler"),
    ("environmental_stressor_end", "handlers", "InfrastructureEventHandler"),
    ("equipment_failure", "handlers", "InfrastructureEventHandler"),
    ("equipment_repaired", "handlers", "InfrastructureEventHandler"),
    ("extreme_cold", "handlers", "InfrastructureEventHandler"),
    ("extreme_heat", "handlers", "InfrastructureEventHandler"),
    ("facility_outage", "handlers", "FacilityOutageHandler"),
    ("facility_recovery", "handlers", "FacilityRecoveryHandler"),
    ("information_contact_update", "handlers", "InformationContactHandler"),
    ("information_correction", "handlers", "InformationCorrectionHandler"),
    ("information_rumor", "handlers", "InformationRumorHandler"),
    ("population_approach_complete", "handlers", "EffectProgramPopulationHandler"),
    ("power_outage", "handlers", "InfrastructureEventHandler"),
    ("power_restored", "handlers", "InfrastructureEventHandler"),
    ("proximity_contact_update", "handlers", "ProximityContactHandler"),
    ("resource_consume", "handlers", "ResourceLedgerHandler"),
    ("resource_delivery", "handlers", "ResourceLedgerHandler"),
    ("resource_loss", "handlers", "ResourceLedgerHandler"),
    ("resource_release", "handlers", "ResourceLedgerHandler"),
    ("resource_reserve", "handlers", "ResourceLedgerHandler"),
    ("resource_service_cancel", "handlers", "ResourceServiceCancellationHandler"),
    ("resource_service_complete", "handlers", "ResourceServiceCompletionHandler"),
    ("social_interaction", "handlers", "SocialInteractionHandler"),
    ("staff_operation_arrive", "handlers", "StaffOperationArrivalHandler"),
    ("staff_operation_complete", "handlers", "StaffOperationCompletionHandler"),
    ("staff_operation_relief", "handlers", "StaffOperationReliefHandler"),
    ("staff_restored", "handlers", "StaffRestorationHandler"),
    ("staff_shift_change", "handlers", "StaffUnavailabilityHandler"),
    ("staff_support_complete", "handlers", "StaffSupportCompletionHandler"),
    ("staff_unavailable", "handlers", "StaffUnavailabilityHandler"),
    ("supply_route_interruption", "handlers", "InfrastructureEventHandler"),
    ("supply_route_restored", "handlers", "InfrastructureEventHandler"),
    ("support_request_timeout", "handlers", "SupportRequestTimeoutHandler"),
    ("toilet_service_complete", "handlers", "ToiletServiceCompletionHandler"),
    ("water_outage", "handlers", "WaterOutageHandler"),
    ("water_restored", "handlers", "WaterRestorationHandler"),
];

/// `controller.event_dispatcher`, as `load::run`'s `dispatch` reads it.
fn dispatch(resource_service: bool, cooldown: i64) -> V {
    let manager = |module: &str, class: &str| vmap([("object", object_of(module, class, Vec::new(), Vec::new()))]);
    let service_manager = || manager("sheltersim.domain.resource_service", "ResourceServiceManager");
    let mut handlers = Map::new();
    for (event_type, module, class) in HANDLERS {
        let attrs: Vec<(&str, V)> = match event_type {
            "resource_service_cancel" | "resource_service_complete" if !resource_service => continue,
            "resource_service_cancel" | "resource_service_complete" => vec![("service", service_manager())],
            "agent_travel_complete" => vec![
                ("resource_service", if resource_service { service_manager() } else { value(V::Null) }),
                ("toilet_service", manager("sheltersim.domain.facilities", "ToiletServiceManager")),
                ("validate_request_state", value(V::Bool(false))),
            ],
            "staff_support_complete" => vec![("cooldown_minutes", value(V::Int(cooldown))), ("require_colocated", value(V::Bool(true)))],
            "support_request_timeout" => vec![("cooldown_minutes", value(V::Int(cooldown)))],
            "toilet_service_complete" => vec![("after_complete", vmap([("method", vstr("_schedule_return_to_initial_zone"))]))],
            resource if resource.starts_with("resource_") => vec![("operation", value(vstr(resource.trim_start_matches("resource_"))))],
            expiring if module == "effects" && class != "EffectProgramStructureHandler" => vec![("expire", value(V::Bool(expiring.ends_with("_expire"))))],
            _ => Vec::new(),
        };
        let module = format!("sheltersim.events.{module}");
        handlers.insert(
            text(event_type),
            vmap([
                ("class", V::List(vec![vstr(&module), vstr(class)])),
                ("handler", object_of(&module, class, attrs, Vec::new())),
                ("own_managers", V::Bool(true)),
                ("after_arrival_as_defined", V::Bool(true)),
            ]),
        );
    }
    vmap([("known", value(V::Bool(true))), ("handlers", value(V::Map(handlers)))])
}

// ---- The document ------------------------------------------------------------------------------

/// The policies document of the Run of a world at minute 0, made as the controller's `__init__`
/// makes its parts, in its order (an error is the first one it raises).
pub(crate) fn policies(world: &World) -> Result<V> {
    let configs = match &*View::new(world, true, None).configs()? {
        V::Map(configs) => configs.clone(),
        _ => return Err(refused("model_configs is not a dict")),
    };
    let selection = selection::select(&V::Map(configs.clone()))?;
    let (proximity_policy, proximity_config) = proximity(&configs, &selection, world)?;
    let information = super::sections::information_config(configs.get("information_spread"))?;
    let decisions = decisions(&configs, &selection, world, &information)?;
    let information_policy = information_policy(&selection, &information)?;
    let resource = world.schema_version()? == 6;
    let resource_service = resource_service(&configs, world)?;
    let psychology = psychology(&configs, &selection, resource)?;
    let stress = stress(&configs, &selection)?;
    let toilet = toilet_service(&configs)?;
    let (staff, cooldown) = staff_service(&configs)?;
    let artifacts = vmap([(
        "object",
        object_of(
            "sheltersim.inspection.traces",
            "RunArtifactWriter",
            vec![("trace_level", vmap([("text", vstr("off"))]))],
            vec![("decision_sink", value(vstr("artifacts")))],
        ),
    )]);
    let controller = object_of(
        "sheltersim.experiments.runner",
        "OfflineSimulationController",
        vec![
            ("psychology", psychology),
            ("decisions", decisions),
            ("stress_policy", stress),
            ("information_policy", information_policy),
            ("proximity_contact_config", proximity_config),
            ("proximity_policy", proximity_policy),
            ("social_policy", value(V::Null)),
            ("toilet_service", toilet),
            ("resource_service", resource_service),
            ("staff_service", staff),
            ("artifacts", artifacts),
            ("llm_router", value(V::Null)),
        ],
        Vec::new(),
    );
    Ok(vmap([("controller", controller), ("dispatch", dispatch(resource, cooldown))]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_behavior_version_names_autonomy_and_the_resource_service() {
        assert_eq!(behavior_version("utility-softmax-v1", false, false).unwrap(), "utility-softmax-v1");
        assert_eq!(behavior_version("utility-softmax-v1", true, false).unwrap(), "utility-softmax-resource-service-v1");
        assert_eq!(behavior_version("utility-softmax-v1", false, true).unwrap(), "utility-softmax-autonomy-v2");
        assert_eq!(behavior_version("sleep-aware-utility-softmax-v2", true, true).unwrap(), "sleep-aware-utility-softmax-v2-autonomy-v2-resource-service-v1");
        assert_eq!(behavior_version("priority-rule-v1", true, true).unwrap(), "priority-rule-autonomy-v2-resource-service-v1");
    }
}
