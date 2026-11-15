//! What a new Run of a Scenario records besides its world (#301, #429): its `model_versions`
//! (`model_signatures(scenario.model_configs)` of `contracts/policies.py`) and the evidence route's
//! answer (`evidence_context(scenario)` of `experiments/validation.py` with the catalogs
//! `web/project_services.py` adds, in a Project without datasets), made in the core for any
//! Scenario. What they read of the installed code (the operator and model catalog, the evidence's
//! catalogs and texts) is `records.json`, which `tests/support/core_initial.py` writes
//! (`initial_records`); `tests/integration/test_core_fixtures.py` keeps it current.
//!
//! `safe_document` redacts what looks like a secret, an e-mail address or a phone number in a
//! string and refuses a key that names private data. The core does not redact: a string where
//! Python's patterns could match, or such a key, is `Unsupported` (the Scenario screen's builder
//! writes neither).

use std::sync::Arc;

use indexmap::IndexMap;
use serde_json::Value;

use std::sync::OnceLock;

use super::{bad, canonical, map_of, text, vmap, vstr};
use crate::checkpoint::sha256_hex;
use crate::error::{Error, Result};
use crate::json::write_float;
use crate::text::casefold;

/// A superset of `\\d` (Unicode decimal digits): any numeric character.
fn re_decimal(char: char) -> bool {
    char.is_numeric()
}
use crate::value::V;

/// `json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False)`, with the default
/// `ensure_ascii=True`: every character past ASCII as `\uXXXX`.
fn ascii_json(value: &V, out: &mut Vec<u8>) -> Result<()> {
    match value {
        V::Null => out.extend_from_slice(b"null"),
        V::Bool(flag) => out.extend_from_slice(if *flag { b"true" } else { b"false" }),
        V::Int(number) => out.extend_from_slice(number.to_string().as_bytes()),
        V::Big(digits) => out.extend_from_slice(digits.as_bytes()),
        V::Float(number) => write_float(out, *number).map_err(|_| Error::unsupported("NaN and infinity are not JSON"))?,
        V::Str(text) => ascii_string(text, out),
        V::List(items) => {
            out.push(b'[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                ascii_json(item, out)?;
            }
            out.push(b']');
        }
        V::Map(map) => {
            let mut entries: Vec<(&Arc<str>, &V)> = map.iter().collect();
            entries.sort_by(|first, second| first.0.as_bytes().cmp(second.0.as_bytes()));
            out.push(b'{');
            for (index, (key, item)) in entries.into_iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                ascii_string(key, out);
                out.push(b':');
                ascii_json(item, out)?;
            }
            out.push(b'}');
        }
    }
    Ok(())
}

fn ascii_string(text: &str, out: &mut Vec<u8>) {
    out.push(b'"');
    for char in text.chars() {
        match char {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\r' => out.extend_from_slice(b"\\r"),
            '\t' => out.extend_from_slice(b"\\t"),
            '\u{8}' => out.extend_from_slice(b"\\b"),
            '\u{c}' => out.extend_from_slice(b"\\f"),
            ' '..='~' => out.push(char as u8),
            _ => {
                let mut units = [0u16; 2];
                for unit in char.encode_utf16(&mut units) {
                    out.extend_from_slice(format!("\\u{unit:04x}").as_bytes());
                }
            }
        }
    }
    out.push(b'"');
}

/// The Scenario's configs and provenance.
fn read(scenario: &[u8]) -> Result<(IndexMap<Arc<str>, V>, V)> {
    let json: Value = serde_json::from_slice(scenario).map_err(bad)?;
    if !json.is_object() {
        return Err(bad("the document is not a dict"));
    }
    let configs = map_of(&json["model_configs"], "model_configs")?;
    let provenance = V::Map(map_of(&json["provenance"], "provenance")?);
    Ok((configs, provenance))
}

// ---- The installed code's catalogs ------------------------------------------------------------------

/// `records.json`'s version that this reader takes.
const RECORDS_SCHEMA_VERSION: i64 = 1;

/// What the records read of the installed code.
struct Installed {
    /// The evidence route's answer but what depends on the Scenario: the legend, the catalogs
    /// (`operator_evidence`, `state_contracts`, `simulator_states`, `agents_social_operators`), the
    /// sources and the dataset registry.
    evidence: IndexMap<Arc<str>, V>,
}

impl Installed {
    /// `operator_evidence_catalog()[model_id]` (a model of `model_catalog()` among them).
    fn model(&self, model_id: &str) -> Result<&IndexMap<Arc<str>, V>> {
        match self.evidence.get("operator_evidence").and_then(|catalog| member(catalog, model_id)) {
            Some(V::Map(definition)) => Ok(definition),
            _ => Err(Error::Key(model_id.to_owned())),
        }
    }
}

fn read_installed() -> std::result::Result<Installed, String> {
    let json: Value = serde_json::from_str(include_str!("records.json")).map_err(|error| error.to_string())?;
    if json["schema_version"].as_i64() != Some(RECORDS_SCHEMA_VERSION) {
        return Err("records.json has another schema_version".into());
    }
    let Some(V::Map(evidence)) = V::from_json(&json["evidence"]) else {
        return Err("records.json has no evidence".into());
    };
    Ok(Installed { evidence })
}

fn installed() -> Result<&'static Installed> {
    static INSTALLED: OnceLock<std::result::Result<Installed, String>> = OnceLock::new();
    INSTALLED.get_or_init(read_installed).as_ref().map_err(|why| Error::Value(format!("the core's records catalog: {why}")))
}

// ---- model_signatures ---------------------------------------------------------------------------

/// `SUITE_RUNTIME_RESOLVED_DOMAINS`.
const SUITE_RUNTIME_RESOLVED: [&str; 3] = ["psychology", "queue", "relationship"];

fn sha_of(value: &V) -> Result<String> {
    let mut out = Vec::new();
    ascii_json(value, &mut out)?;
    Ok(sha256_hex(&out))
}

fn map_mut<'a>(value: &'a mut V, key: &str) -> Option<&'a mut IndexMap<Arc<str>, V>> {
    match value {
        V::Map(map) => match map.get_mut(key) {
            Some(V::Map(found)) => Some(found),
            _ => None,
        },
        _ => None,
    }
}

fn config<'a>(configs: &'a IndexMap<Arc<str>, V>, section: &str) -> Option<&'a V> {
    configs.get(section)
}

/// `configs.get(section, {}).get(field)`, truthy.
fn configured(configs: &IndexMap<Arc<str>, V>, section: &str, field: &str) -> bool {
    config(configs, section).and_then(|found| member(found, field)).is_some_and(V::is_truthy)
}

/// The signature of a selected model (`model_signatures`' loop).
fn policy_signature(configs: &IndexMap<Arc<str>, V>, domain: &str, model_id: &str, registry_version: &V) -> Result<String> {
    let definition = installed()?.model(model_id)?;
    let field = |name: &str| definition.get(name).cloned().unwrap_or(V::Null);
    let written = |found: V| match found {
        V::Str(text) => text.to_string(),
        V::Int(number) => number.to_string(),
        other => crate::validate::repr(&other),
    };
    let mut base = format!("{model_id}@{}:state{}", written(field("implementation_version")), written(field("state_schema_version")));
    let version_two = match registry_version {
        V::Int(version) => *version >= 2,
        V::Float(version) => *version >= 2.0,
        V::Bool(_) => false,
        _ => return Err(Error::unsupported("the registry's schema_version is not a number")),
    };
    if !version_two {
        return Ok(base);
    }
    let mut signature = V::Map(definition.clone());
    let mut sections: Vec<Arc<str>> = match definition.get("parameters") {
        Some(V::Map(parameters)) => parameters.keys().map(|path| text(path.split('.').next().unwrap_or(path))).collect(),
        _ => Vec::new(),
    };
    sections.sort_by(|first, second| first.as_bytes().cmp(second.as_bytes()));
    sections.dedup();
    let mut inputs: IndexMap<Arc<str>, V> = sections
        .into_iter()
        .map(|section| {
            let found = config(configs, &section).cloned().unwrap_or(V::Map(IndexMap::new()));
            (section, found)
        })
        .collect();
    let pop_parameter = |signature: &mut V, path: &str| {
        if let Some(parameters) = map_mut(signature, "parameters") {
            parameters.shift_remove(path);
        }
    };
    let pop_input = |inputs: &mut IndexMap<Arc<str>, V>, section: &str, field: &str| {
        if let Some(V::Map(found)) = inputs.get_mut(section) {
            found.shift_remove(field);
        }
    };
    match domain {
        "queue" => {
            if let Some(V::Map(limit)) =
                map_mut(&mut signature, "parameters").and_then(|parameters| parameters.get_mut("facility_queues.abandon_after_minutes_by_service.*"))
            {
                limit.shift_remove("scope");
            }
            if model_id == "strict-fifo-queue-v1" {
                let assumption = field("assumption_source_id");
                let services = vmap([("toilet", V::Int(120)), ("water", V::Int(240)), ("meal", V::Int(360))]);
                let legacy = [
                    ("facility_queues.abandon_after_minutes_by_service.*", "minute", services, V::Int(1), V::Int(10080)),
                    ("facility_queues.retry_delay_minutes", "minute", V::Int(15), V::Int(1), V::Int(1440)),
                    ("facility_queues.persistence_urgency_threshold", "normalized_0_1", V::Float(0.8), V::Float(0.0), V::Float(1.0)),
                ];
                let parameters = legacy
                    .into_iter()
                    .map(|(path, unit, default, lower, upper)| {
                        let metadata = vmap([
                            ("unit", vstr(unit)),
                            ("default", default),
                            ("lower", lower),
                            ("upper", upper),
                            ("lower_exclusive", V::Bool(false)),
                            ("source_id", assumption.clone()),
                            ("evidence_grade", vstr("C")),
                            ("applicability", vstr("Uncalibrated installed model assumption.")),
                        ]);
                        (text(path), metadata)
                    })
                    .collect();
                if let V::Map(map) = &mut signature {
                    map.insert(text("parameters"), V::Map(parameters));
                }
                inputs = [(text("facility_queues"), config(configs, "facility_queues").cloned().unwrap_or(V::Map(IndexMap::new())))].into_iter().collect();
            }
        }
        "zone_environment" => pop_parameter(&mut signature, "zone_environment.baseline_by_zone.*"),
        "body_dynamics" => {
            for name in [
                "sleep_debt_stress_rate_by_agent",
                "negative_affect_stress_rate_by_agent",
                "discontent_stress_rate_by_agent",
                "complaint_reassurance_gain_by_agent",
            ] {
                if !configured(configs, "body_dynamics", name) {
                    pop_parameter(&mut signature, &format!("body_dynamics.{name}.*"));
                }
            }
        }
        "psychology" => {
            for name in ["toilet_growth_by_agent", "hunger_growth_by_agent", "awake_fatigue_growth_by_agent"] {
                if !configured(configs, "psychology", name) {
                    pop_parameter(&mut signature, &format!("psychology.{name}.*"));
                    pop_input(&mut inputs, "psychology", name);
                }
            }
        }
        "behavior" => {
            for name in ["decision_interval_minutes_by_agent", "temperature_by_agent"] {
                if !configured(configs, "behavior", name) {
                    pop_parameter(&mut signature, &format!("behavior.{name}.*"));
                    pop_input(&mut inputs, "behavior", name);
                }
            }
        }
        "care_outcome" => {
            if !configured(configs, "body_dynamics", "hygiene_support_relief_by_agent") {
                pop_parameter(&mut signature, "body_dynamics.hygiene_support_relief_by_agent.*");
            }
        }
        "staff_fatigue" => {
            if !configured(configs, "staff_dynamics", "max_continuous_work_minutes_by_staff") {
                pop_parameter(&mut signature, "staff_dynamics.max_continuous_work_minutes_by_staff.*");
                pop_input(&mut inputs, "staff_dynamics", "max_continuous_work_minutes_by_staff");
            }
        }
        "staff_operations" => {
            if let Some(V::Map(durations)) =
                map_mut(&mut signature, "parameters").and_then(|parameters| parameters.get_mut("staff_operations.task_duration_minutes.*"))
            {
                for name in ["scope", "replaceable", "applicability"] {
                    durations.shift_remove(name);
                }
            }
            for (name, path) in [("thresholds_by_zone", "staff_operations.thresholds_by_zone.*"), ("effects_by_zone", "staff_operations.effects_by_zone.*")] {
                if !configured(configs, "staff_operations", name) {
                    pop_parameter(&mut signature, path);
                    pop_input(&mut inputs, "staff_operations", name);
                }
            }
        }
        _ => {}
    }
    base.push_str(&format!(":manifest={}:inputs={}", sha_of(&signature)?, sha_of(&V::Map(inputs))?));
    Ok(base)
}

/// A world that holds only these configs, which the extensions' signatures read.
fn configs_world(configs: &IndexMap<Arc<str>, V>) -> Result<crate::world::World> {
    let mut world = crate::world::World::default();
    world.slots.add(text(crate::world::MODEL_CONFIGS), crate::world::Place::Field)?;
    let slot = world.slots.get(crate::world::MODEL_CONFIGS).ok_or_else(|| Error::unsupported("the configs have no slot"))?;
    slot.try_borrow_mut()?.row = Some(Box::new(crate::row::ValueRow(V::Map(configs.clone()))));
    Ok(world)
}

/// `model_signatures(configs)`.
pub(crate) fn model_signatures(configs: &IndexMap<Arc<str>, V>) -> Result<IndexMap<Arc<str>, V>> {
    let configs_value = V::Map(configs.clone());
    let selection = crate::validate::selection::select(&configs_value)?;
    let registry = config(configs, "policy_registry");
    let registry_version = registry.and_then(|registry| member(registry, "schema_version")).cloned().unwrap_or(V::Int(1));
    let mut signatures = IndexMap::new();
    for (domain, model_id) in selection.iter() {
        signatures.insert(text(&format!("policy:{domain}")), V::Str(Arc::from(policy_signature(configs, domain, model_id, &registry_version)?.as_str())));
    }
    let world = configs_world(configs)?;
    for (key, signature) in crate::validate::signatures::simulator_signatures(&crate::validate::View::new(&world, true, None))? {
        signatures.insert(key, V::Str(signature));
    }
    for contract in crate::validate::declared::declared_states(&configs_value)? {
        let signature = format!("{}@{}:state{}", contract.implementation_id, contract.implementation_version, contract.schema_version);
        signatures.insert(text(&format!("state:{}", contract.namespace)), V::Str(Arc::from(signature.as_str())));
    }
    signatures.insert(text(crate::model_version::ENGINE_VERSION_KEY), vstr(crate::model_version::ENGINE_VERSION));
    signatures.insert(text(crate::model_version::RNG_ALGORITHM_KEY), vstr("sha256-keyed-v1"));
    if let Some(suite) = registry.and_then(|registry| member(registry, "suite_id")).filter(|suite| !matches!(suite, V::Null)) {
        let V::Str(suite) = suite else {
            return Err(Error::unsupported("the suite is not a str"));
        };
        let implicit = SUITE_RUNTIME_RESOLVED.iter().any(|domain| !registry.is_some_and(|registry| member(registry, domain).is_some()));
        signatures.insert(text("model_suite"), V::Str(Arc::from(format!("{suite}@{}", if implicit { 2 } else { 1 }).as_str())));
    }
    Ok(signatures)
}

/// `model_signatures(scenario.model_configs)` of a Scenario's canonical JSON.
pub fn model_versions(scenario: &[u8]) -> Result<V> {
    let (configs, _) = read(scenario)?;
    Ok(V::Map(model_signatures(&configs)?))
}

// ---- safe_document --------------------------------------------------------------------------------

/// Whether `_PRIVATE` (`api.?key|secret|password|credential|authorization|raw.?prompt|
/// full.?name|email|phone|address`, ignoring case) could match a key: one of its words, or both
/// words of a pair, in the folded key (`ı` read as `i`, as `re.IGNORECASE` matches it).
fn private_key(key: &str) -> bool {
    let folded = casefold(key).replace('ı', "i");
    let both = |first: &str, second: &str| folded.contains(first) && folded.contains(second);
    ["secret", "password", "credential", "authorization", "email", "phone", "address"].iter().any(|word| folded.contains(word))
        || both("api", "key")
        || both("raw", "prompt")
        || both("full", "name")
}

/// `re.IGNORECASE` of an ASCII letter (lower case) against a character, with Python's extras.
fn ci(char: char, letter: char) -> bool {
    (char.is_ascii() && char.to_ascii_lowercase() == letter) || matches!((letter, char), ('i', '\u{130}' | '\u{131}') | ('k', '\u{212a}') | ('s', '\u{17f}'))
}

/// Whether `_redact` could change the text: an `@`; `sk-` with 12 or more of `[A-Za-z0-9_-]`
/// after it, or `bearer`, white space and something more (both ignoring case); or three groups of
/// 2 to 4 digits joined by `-` or a space.
fn may_redact(value: &str) -> bool {
    if value.contains('@') {
        return true;
    }
    let chars: Vec<char> = value.chars().collect();
    let word = |start: usize, letters: &str| {
        letters.chars().enumerate().all(|(offset, letter)| {
            chars.get(start + offset).is_some_and(|char| if letter.is_ascii_alphabetic() { ci(*char, letter) } else { *char == letter })
        })
    };
    let key_char = |char: char| char.is_ascii_alphanumeric() || char == '_' || char == '-' || matches!(char, '\u{130}' | '\u{131}' | '\u{17f}' | '\u{212a}');
    for start in 0..chars.len() {
        if word(start, "sk-") && chars[start + 3..].iter().take_while(|char| key_char(**char)).count() >= 12 {
            return true;
        }
        if word(start, "bearer") {
            let spaces = chars[start + 6..].iter().take_while(|char| char.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(*char)).count();
            if spaces > 0 && chars.get(start + 6 + spaces).is_some() {
                return true;
            }
        }
    }
    let digits = |start: usize| chars[start..].iter().take_while(|char| re_decimal(**char)).count();
    let joined = |char: char| char == '-' || char == ' ';
    for start in 0..chars.len() {
        let first = digits(start);
        if first < 2 || (start > 0 && re_decimal(chars[start - 1])) {
            continue;
        }
        // Any split of a run into groups: a run of 2 to 4 digits, a joint, then again, then 3+.
        let mut at = start + first;
        let second_start = at + 1;
        if at >= chars.len() || !joined(chars[at]) {
            continue;
        }
        let second = digits(second_start);
        if second < 2 {
            continue;
        }
        at = second_start + second;
        if at < chars.len() && joined(chars[at]) && digits(at + 1) >= 3 {
            return true;
        }
    }
    false
}

/// `safe_document` of a value the Scenario holds: the value itself, or `Unsupported` where Python
/// would redact or refuse something.
fn safe(value: &V) -> Result<V> {
    match value {
        V::Str(text) if may_redact(text) => Err(Error::unsupported("a text that safe_document may redact")),
        V::Float(number) if !number.is_finite() => Err(Error::unsupported("a float that is not finite")),
        V::Map(map) => {
            if map.keys().any(|key| private_key(key)) {
                return Err(Error::unsupported("a key that names private data"));
            }
            map.values().try_for_each(|item| safe(item).map(drop))?;
            Ok(value.clone())
        }
        V::List(items) => {
            items.iter().try_for_each(|item| safe(item).map(drop))?;
            Ok(value.clone())
        }
        _ => Ok(value.clone()),
    }
}

fn member<'a>(value: &'a V, key: &str) -> Option<&'a V> {
    match value {
        V::Map(map) => map.get(key),
        _ => None,
    }
}

// ---- evidence_context --------------------------------------------------------------------------

/// The fields of `PsychologyModelConfig` `coefficient_values` reads, with their defaults (None
/// for the research fields, which it leaves out when they are None), in its order.
const PSYCHOLOGY: [(&str, Option<f64>); 21] = [
    ("toilet_growth", Some(0.001)),
    ("hunger_growth", Some(0.0005)),
    ("awake_fatigue_growth", Some(0.0008)),
    ("resting_fatigue_recovery", Some(0.004)),
    ("sleeping_fatigue_recovery", Some(0.010)),
    ("stress_pressure", Some(0.012)),
    ("queued_stress", Some(0.002)),
    ("support_wait_stress", Some(0.001)),
    ("resting_stress_recovery", Some(0.003)),
    ("sleeping_stress_recovery", Some(0.006)),
    ("need_pressure_threshold", Some(0.35)),
    ("toilet_growth_by_agent", None),
    ("hunger_growth_by_agent", None),
    ("awake_fatigue_growth_by_agent", None),
    ("natural_recovery_rate_per_minute", None),
    ("natural_recovery_rate_by_agent", None),
    ("baseline_distress_by_agent", None),
    ("chronic_retention", None),
    ("chronic_acute_weight", None),
    ("chronic_high_distress_weight", None),
    ("high_distress_threshold", None),
];

/// The operators whose parameters' source is their section's `source_id`, by section.
const SECTION_SOURCED: [(&str, &str); 5] = [
    ("resource-service-v1", "resource_service"),
    ("toilet-service-uniform-v1", "toilet_service"),
    ("staff-support-fifo-aging-v2", "staff"),
    ("continuous-duty-fatigue-v1", "staff_dynamics"),
    ("continuous-duty-fatigue-v2", "staff_dynamics"),
];

fn is_scalar(value: &V) -> bool {
    matches!(value, V::Int(_) | V::Big(_) | V::Float(_) | V::Bool(_))
}

fn section_map(configs: &IndexMap<Arc<str>, V>, section: &str) -> IndexMap<Arc<str>, V> {
    match configs.get(section) {
        Some(V::Map(map)) => map.clone(),
        _ => IndexMap::new(),
    }
}

/// `parameter_catalog(scenario)`: the effective values with their declared units and ranges.
fn parameter_catalog(configs: &IndexMap<Arc<str>, V>, selection: &crate::config::Selection, signatures: &IndexMap<Arc<str>, V>) -> Result<V> {
    let installed = installed()?;
    let assumption = |model_id: &str| -> Result<V> { Ok(installed.model(model_id)?.get("assumption_source_id").cloned().unwrap_or(V::Null)) };
    let selected = |domain: &str| selection.get(domain).cloned().ok_or_else(|| Error::Key(domain.to_owned()));
    let staff = staff_section(configs);
    let psychology = psychology_section(configs);
    let social_source = assumption(&selected("social")?)?;
    let V::Str(social_source) = social_source else {
        return Err(Error::unsupported("a model has no assumption source"));
    };
    let social: IndexMap<Arc<str>, V> =
        super::policies::social_parameters(configs.get("social_propagation"), &social_source)?.into_iter().map(|(key, found)| (text(key), found)).collect();
    let information = super::sections::information_config(configs.get("information_spread"))?;
    let mut behavior: IndexMap<Arc<str>, V> =
        [("temperature", V::Float(0.2)), ("decision_interval_minutes", V::Int(15))].into_iter().map(|(key, found)| (text(key), found)).collect();
    for (key, found) in section_map(configs, "behavior") {
        behavior.insert(key, found);
    }
    let toilet_raw = section_map(configs, "toilet_service");
    let toilet: IndexMap<Arc<str>, V> = [("minimum_minutes", V::Int(4)), ("maximum_minutes", V::Int(8))]
        .into_iter()
        .map(|(key, default)| (text(key), toilet_raw.get(key).cloned().unwrap_or(default)))
        .collect();
    let mut sections: Vec<(&str, IndexMap<Arc<str>, V>)> = vec![
        ("psychology", psychology),
        ("social_propagation", social),
        ("information_spread", information),
        ("behavior", behavior),
        ("toilet_service", toilet),
        ("staff", staff),
    ];
    if let Some(raw) = configs.get("resource_service") {
        let V::Map(dumped) = crate::validate::models::resource_service_config(raw)? else {
            return Err(Error::unsupported("the resource service's dump is not a dict"));
        };
        sections.push(("resource_service", dumped));
    }
    let mut sources: IndexMap<&str, V> = [
        ("psychology", "ASM-H04-CAUSAL-STRESS-V1"),
        ("social_propagation", "ASM-SOCIAL-PROPAGATION-V1"),
        ("information_spread", "ASM-INFORMATION-SPREAD-V1"),
        ("behavior", "ASM-UTILITY-BEHAVIOR-V1"),
        ("toilet_service", "ASM-TOILET-SERVICE-DURATION-V1"),
        ("staff", "ASM-STAFF-SUPPORT-LIFECYCLE-V1"),
        ("resource_service", "ASM-H02-RESOURCE-SERVICE-V1"),
    ]
    .into_iter()
    .map(|(domain, source)| (domain, vstr(source)))
    .collect();
    for (domain, name) in [("psychology", "psychology"), ("behavior", "behavior"), ("social", "social_propagation"), ("information", "information_spread")] {
        sources.insert(name, assumption(&selected(domain)?)?);
    }
    let mut by_path: IndexMap<Arc<str>, V> = IndexMap::new();
    add_section_rows(&mut by_path, &sections, &sources);
    let active = active_models(configs, selection, signatures);
    let configs_value = V::Map(configs.clone());
    let parameters_of = |model_id: &str| -> Result<IndexMap<Arc<str>, V>> {
        Ok(match installed.model(model_id)?.get("parameters") {
            Some(V::Map(parameters)) => parameters.clone(),
            _ => IndexMap::new(),
        })
    };
    let get = |metadata: &V, key: &str| member(metadata, key).cloned();
    for model_id in &active {
        for (path, metadata) in parameters_of(model_id)? {
            let mut found = Some(&configs_value);
            for part in path.split('.') {
                found = found.and_then(|value| member(value, part));
                if found.is_none() {
                    break;
                }
            }
            let found = match found {
                Some(found) => found.clone(),
                None => get(&metadata, "default").unwrap_or(V::Null),
            };
            if !is_scalar(&found) {
                continue;
            }
            let mut source = match get(&metadata, "source_id") {
                Some(source) => source,
                None => assumption(model_id)?,
            };
            if let Some((_, section)) = SECTION_SOURCED.iter().find(|(model, _)| *model == &**model_id) {
                source = configs.get(*section).and_then(|config| member(config, "source_id")).cloned().ok_or_else(|| Error::Key("source_id".into()))?;
            }
            let upper = if &*path == "staff.support_fairness_minutes" {
                section_map(configs, "staff").get("unresolved_after_minutes").cloned().unwrap_or(V::Int(90))
            } else {
                get(&metadata, "upper").unwrap_or(V::Null)
            };
            let row = vmap([
                ("path", V::Str(path.clone())),
                ("value", found),
                ("unit", get(&metadata, "unit").ok_or_else(|| Error::Key("unit".into()))?),
                ("lower", get(&metadata, "lower").unwrap_or(V::Null)),
                ("upper", upper),
                ("lower_exclusive", get(&metadata, "lower_exclusive").unwrap_or(V::Bool(false))),
                ("source_id", source),
                ("evidence_grade", get(&metadata, "evidence_grade").unwrap_or_else(|| vstr("C"))),
                ("applicability", get(&metadata, "applicability").unwrap_or_else(|| vstr("installed model assumption"))),
                ("external_validation", V::Bool(false)),
            ]);
            by_path.insert(path, row);
        }
    }
    for model_id in &active {
        for (path, metadata) in parameters_of(model_id)? {
            let Some(scope) = get(&metadata, "scope").filter(|scope| !matches!(scope, V::Null)) else {
                continue;
            };
            let parts: Vec<&str> = path.split('.').collect();
            if parts.len() != 3 || parts[2] != "*" {
                continue;
            }
            let default = get(&metadata, "default").unwrap_or_else(|| V::Map(IndexMap::new()));
            let found = match configs.get(parts[0]) {
                None => default,
                Some(V::Map(config)) => config.get(parts[1]).cloned().unwrap_or(default),
                Some(_) => default,
            };
            let source = match get(&metadata, "source_id") {
                Some(source) => source,
                None => assumption(model_id)?,
            };
            let row = vmap([
                ("path", V::Str(path.clone())),
                ("value", found),
                ("unit", get(&metadata, "unit").ok_or_else(|| Error::Key("unit".into()))?),
                ("lower", get(&metadata, "lower").unwrap_or(V::Null)),
                ("upper", get(&metadata, "upper").unwrap_or(V::Null)),
                ("scope", scope),
                ("field_units", get(&metadata, "field_units").unwrap_or(V::Null)),
                ("field_bounds", get(&metadata, "field_bounds").unwrap_or(V::Null)),
                ("source_id", source),
                ("evidence_grade", get(&metadata, "evidence_grade").unwrap_or_else(|| vstr("C"))),
                ("applicability", get(&metadata, "applicability").unwrap_or_else(|| vstr("installed model assumption"))),
                ("external_validation", V::Bool(false)),
            ]);
            by_path.insert(path, row);
        }
    }
    by_path.sort_by(|first, _, second, _| first.as_bytes().cmp(second.as_bytes()));
    Ok(V::List(by_path.into_values().collect()))
}

/// The staff section with its defaults, and the fairness minutes the rule derives where unset.
fn staff_section(configs: &IndexMap<Arc<str>, V>) -> IndexMap<Arc<str>, V> {
    let raw_staff = section_map(configs, "staff");
    let mut staff: IndexMap<Arc<str>, V> =
        [("support_task_minutes", V::Int(10)), ("unresolved_after_minutes", V::Int(90)), ("support_cooldown_minutes", V::Int(30))]
            .into_iter()
            .map(|(key, found)| (text(key), found))
            .collect();
    for (key, found) in &raw_staff {
        staff.insert(key.clone(), found.clone());
    }
    if !raw_staff.contains_key("support_fairness_minutes") {
        let fairness = match staff.get("unresolved_after_minutes") {
            Some(V::Int(minutes)) if *minutes > 0 => (*minutes - 1).clamp(1, 30),
            _ => 30,
        };
        staff.insert(text("support_fairness_minutes"), V::Int(fairness));
    }
    staff
}

/// The psychology section's numbers as floats, with their defaults where unset.
fn psychology_section(configs: &IndexMap<Arc<str>, V>) -> IndexMap<Arc<str>, V> {
    let psychology_raw = section_map(configs, "psychology");
    let mut psychology = IndexMap::new();
    for (name, default) in PSYCHOLOGY {
        match (psychology_raw.get(name), default) {
            (Some(V::Int(number)), _) => {
                psychology.insert(text(name), V::Float(*number as f64));
            }
            (Some(found @ V::Float(_)), _) => {
                psychology.insert(text(name), found.clone());
            }
            (None | Some(V::Null), Some(default)) => {
                psychology.insert(text(name), V::Float(default));
            }
            _ => {}
        }
    }
    psychology
}

/// The rows of the sections' scalar values, with the units and ranges the names say and each
/// section's source.
fn add_section_rows(by_path: &mut IndexMap<Arc<str>, V>, sections: &[(&str, IndexMap<Arc<str>, V>)], sources: &IndexMap<&str, V>) {
    for (domain, config) in sections {
        for (name, found) in config {
            if !is_scalar(found) || &**name == "schema_version" {
                continue;
            }
            let mut unit = if matches!(found, V::Bool(_)) {
                "boolean"
            } else if name.contains("minutes") {
                "minute"
            } else if name.contains("_ml") {
                "mL"
            } else if name.contains("meal_units") {
                "meal_unit"
            } else if name.contains("contacts") {
                "count"
            } else {
                "normalized_or_multiplier"
            };
            let (mut lower, mut upper) = (V::Float(0.0), V::Null);
            if *domain == "staff" && &**name == "support_fairness_minutes" {
                lower = V::Int(1);
                upper = config.get("unresolved_after_minutes").cloned().unwrap_or(V::Null);
            }
            if *domain == "psychology" {
                let threshold = &**name == "need_pressure_threshold";
                upper = V::Float(if threshold { 1.0 } else { 0.25 });
                unit = if threshold { "normalized_0_1" } else { "normalized_per_minute" };
            }
            let path = text(&format!("{domain}.{name}"));
            let row = vmap([
                ("path", V::Str(path.clone())),
                ("value", found.clone()),
                ("unit", vstr(unit)),
                ("lower", lower),
                ("upper", upper),
                ("evidence_grade", vstr("C")),
                ("source_id", sources.get(domain).cloned().unwrap_or(V::Null)),
                ("applicability", vstr("synthetic single-shelter hypothesis; no empirical calibration")),
                ("external_validation", V::Bool(false)),
            ]);
            by_path.insert(path, row);
        }
    }
}

/// The models the Scenario runs: the selected ones, then those its simulators and sections bring.
fn active_models(configs: &IndexMap<Arc<str>, V>, selection: &crate::config::Selection, signatures: &IndexMap<Arc<str>, V>) -> Vec<Arc<str>> {
    let mut active: Vec<Arc<str>> = selection.iter().map(|(_, model)| text(model)).collect();
    if signatures.contains_key("simulator:staff_operations_v1") {
        active.push(text("autonomous-routed-staff-v1"));
    }
    if signatures.contains_key("simulator:movement_v1") {
        active.push(text("spatial-route-and-room-waypoint-v2"));
    }
    for (key, model) in [("resource_service", "resource-service-v1"), ("toilet_service", "toilet-service-uniform-v1"), ("staff", "staff-support-fifo-aging-v2")]
    {
        if configs.contains_key(key) {
            active.push(text(model));
        }
    }
    if let Some(V::Str(policy)) = section_map(configs, "staff_dynamics").get("policy_version") {
        if matches!(&**policy, "continuous-duty-fatigue-v1" | "continuous-duty-fatigue-v2") {
            active.push(policy.clone());
        }
    }
    active
}

/// The evidence route's answer for a new Run of the Scenario (canonical JSON):
/// `evidence_context(scenario)` with the installed code's catalogs.
pub fn evidence(scenario: &[u8]) -> Result<Vec<u8>> {
    let (configs, provenance) = read(scenario)?;
    let installed = installed()?;
    let selection = crate::validate::selection::select(&V::Map(configs.clone()))?;
    let signatures = model_signatures(&configs)?;
    let mut models = IndexMap::new();
    for (domain, model_id) in selection.iter() {
        let mut model: IndexMap<Arc<str>, V> = [(text("id"), vstr(model_id))].into_iter().collect();
        for (name, found) in installed.model(model_id)? {
            if &**name != "parameters" {
                model.insert(name.clone(), found.clone());
            }
        }
        models.insert(text(domain), V::Map(model));
    }
    let parameters = parameter_catalog(&configs, &selection, &signatures)?;
    let mut evidence = installed.evidence.clone();
    let list = |key: &str| member(&provenance, key).cloned().unwrap_or(V::List(Vec::new()));
    let research = member(&provenance, "region_research").cloned().unwrap_or(V::Map(IndexMap::new()));
    let limits = vmap([
        ("unknowns", member(&research, "unknowns").cloned().unwrap_or(V::List(Vec::new()))),
        ("warnings", member(&research, "warnings").cloned().unwrap_or(V::List(Vec::new()))),
    ]);
    for (key, value) in [
        ("models", V::Map(models)),
        ("model_versions", V::Map(signatures)),
        ("parameters", parameters),
        ("scenario_sources", safe(&list("sources"))?),
        ("scenario_assumptions", safe(&list("assumptions"))?),
        ("region_limits", safe(&limits)?),
    ] {
        evidence.insert(text(key), value);
    }
    canonical(&V::Map(evidence))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_text_python_may_redact_is_found() {
        for text in ["a@b.co", "SK-abcdefghijkl", "x bearer  y", "03-1234-5678", "+81 90 1234 5678", "１２-３４-５６７"] {
            assert!(may_redact(text), "{text}");
        }
        for text in ["2026-11-01", "12-34-56", "task-1", "ASM-STANDARD-PROVISIONING-V1", "100 200", "SK-abc", "bearer"] {
            assert!(!may_redact(text), "{text}");
        }
    }

    #[test]
    fn a_key_naming_private_data_is_found() {
        for key in ["address", "API_KEY", "full name", "raw-prompt", "ſecret", "Phone"] {
            assert!(private_key(key), "{key}");
        }
        for key in ["id", "title", "source_ids", "name", "key"] {
            assert!(!private_key(key), "{key}");
        }
    }

    #[test]
    fn ensure_ascii_escapes_past_ascii() {
        let mut out = Vec::new();
        ascii_json(&vmap([("b", V::Str(Arc::from("墨\u{1f600}\"\n"))), ("a", V::Float(1.0))]), &mut out).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "{\"a\":1.0,\"b\":\"\\u58a8\\ud83d\\ude00\\\"\\n\"}");
    }
}
