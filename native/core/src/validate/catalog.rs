//! The Model Catalog: the installed models a Scenario selects (`model-catalog.json`), with the
//! models' definitions (domain, formula, reference, versions, assumption source, parameters with
//! their units and bounds, required state contracts), the default models (`default_models`), the
//! model suites (`suites`) and the psychology models of each kind. It is the one copy: the
//! validation (`selection`), the Run's records (`initial::records`: the model signatures and the
//! evidence's `operator_evidence`) and Python's Scenario authoring (`sheltersim_native.model_catalog`,
//! `contracts/policies.py`) read it. A definition is part of its model's signature, so a change to
//! one changes the `model_versions` of a new Run.
//!
//! The installed state contracts are the evidence's (`initial::records`, `records.json`).

use std::sync::{Arc, OnceLock};

use indexmap::IndexMap;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::value::V;

/// The bounds of a `behavior.` parameter of a model.
#[derive(Clone, Debug)]
pub struct Bounds {
    pub lower: Option<V>,
    pub upper: Option<V>,
    /// `metadata.get("lower_exclusive")`, truthy.
    pub lower_exclusive: bool,
}

/// A model of the Model Catalog.
#[derive(Clone, Debug)]
pub struct ModelDefinition {
    pub domain: Arc<str>,
    /// `required_state_contracts`: a dict of namespace to signature, or another value (which
    /// `selected_models` refuses).
    pub required_state_contracts: V,
    pub state_schema_version: i64,
    /// The parameters whose name starts with `behavior.`, by name.
    pub behavior_parameters: IndexMap<Arc<str>, Bounds>,
    /// `assumption_source_id`: the source an indexed decision sink writes for a decision whose
    /// policy version is this model's ID (`Host::decision_source`, #368).
    pub assumption_source_id: Arc<str>,
}

/// An installed state contract (`StateDescriptor`).
#[derive(Clone, Debug)]
pub struct StateContract {
    pub namespace: Arc<str>,
    pub implementation_id: Arc<str>,
    pub implementation_version: Arc<str>,
    pub schema_version: i64,
    pub policy_domain: Option<Arc<str>>,
}

/// The catalog.
#[derive(Debug)]
pub struct Catalog {
    /// The models, by model ID.
    pub models: IndexMap<Arc<str>, ModelDefinition>,
    pub default_models: Vec<(Arc<str>, Arc<str>)>,
    pub suites: IndexMap<Arc<str>, Vec<(Arc<str>, Arc<str>)>>,
    /// By namespace, sorted.
    pub state_contracts: IndexMap<Arc<str>, StateContract>,
    pub life_state_psychology: Vec<Arc<str>>,
    pub mean_reverting_psychology: Vec<Arc<str>>,
}

impl Catalog {
    /// Whether a model is a life-state psychology model (`LIFE_STATE_PSYCHOLOGY_POLICIES`).
    pub fn is_life_state(&self, model: &str) -> bool {
        self.life_state_psychology.iter().any(|known| &**known == model)
    }

    /// Whether a model is a mean-reverting psychology model.
    pub fn is_mean_reverting(&self, model: &str) -> bool {
        self.mean_reverting_psychology.iter().any(|known| &**known == model)
    }

    /// Whether some model of the catalog has this domain.
    pub fn has_domain(&self, domain: &str) -> bool {
        self.models.values().any(|model| &*model.domain == domain)
    }
}

fn bad() -> Error {
    Error::unsupported("the validation's catalog does not read")
}

fn text(value: &Value) -> Result<Arc<str>> {
    value.as_str().map(Arc::from).ok_or_else(bad)
}

fn pairs(value: &Value) -> Result<Vec<(Arc<str>, Arc<str>)>> {
    value.as_array().ok_or_else(bad)?.iter().map(|pair| Ok((text(&pair[0])?, text(&pair[1])?))).collect()
}

fn texts(value: &Value) -> Result<Vec<Arc<str>>> {
    value.as_array().ok_or_else(bad)?.iter().map(text).collect()
}

fn bound(value: &Value) -> Result<Option<V>> {
    match value {
        Value::Null => Ok(None),
        other => V::from_json(other).map(Some).ok_or_else(bad),
    }
}

/// The Model Catalog's text, as `sheltersim_native.model_catalog()` gives it to Python.
pub const SOURCE: &str = include_str!("model-catalog.json");

/// The Model Catalog's JSON, read once.
pub(crate) fn source() -> Result<&'static Value> {
    static SOURCE_JSON: OnceLock<Option<Value>> = OnceLock::new();
    SOURCE_JSON.get_or_init(|| serde_json::from_str(SOURCE).ok()).as_ref().ok_or_else(bad)
}

/// The definitions of the models, by model ID (the evidence's `operator_evidence` holds them).
pub(crate) fn definitions() -> Result<&'static serde_json::Map<String, Value>> {
    source()?["models"].as_object().ok_or_else(bad)
}

fn read() -> Result<Catalog> {
    let json = source()?;
    let mut models = IndexMap::new();
    for (id, model) in definitions()? {
        let mut behavior_parameters = IndexMap::new();
        for (name, metadata) in model["parameters"].as_object().ok_or_else(bad)? {
            if !name.starts_with("behavior.") {
                continue;
            }
            let bounds = Bounds {
                lower: bound(&metadata["lower"])?,
                upper: bound(&metadata["upper"])?,
                lower_exclusive: metadata["lower_exclusive"].as_bool().unwrap_or(false),
            };
            behavior_parameters.insert(Arc::from(name.as_str()), bounds);
        }
        let definition = ModelDefinition {
            domain: text(&model["domain"])?,
            required_state_contracts: V::from_json(&model["required_state_contracts"]).ok_or_else(bad)?,
            state_schema_version: model["state_schema_version"].as_i64().ok_or_else(bad)?,
            behavior_parameters,
            assumption_source_id: text(&model["assumption_source_id"])?,
        };
        models.insert(Arc::from(id.as_str()), definition);
    }
    let mut suites = IndexMap::new();
    for (suite, members) in json["suites"].as_object().ok_or_else(bad)? {
        suites.insert(Arc::from(suite.as_str()), pairs(members)?);
    }
    let mut state_contracts = IndexMap::new();
    let V::List(installed) = crate::initial::state_contracts()? else {
        return Err(bad());
    };
    for contract in installed {
        let field = |name: &str| match contract {
            V::Map(contract) => contract.get(name).cloned().unwrap_or(V::Null),
            _ => V::Null,
        };
        let text_of = |name: &str| match field(name) {
            V::Str(found) => Ok(found),
            _ => Err(bad()),
        };
        let contract = StateContract {
            namespace: text_of("namespace")?,
            implementation_id: text_of("implementation_id")?,
            implementation_version: text_of("implementation_version")?,
            schema_version: match field("schema_version") {
                V::Int(version) => version,
                _ => return Err(bad()),
            },
            policy_domain: match field("policy_domain") {
                V::Str(domain) => Some(domain),
                _ => None,
            },
        };
        state_contracts.insert(contract.namespace.clone(), contract);
    }
    state_contracts.sort_keys();
    Ok(Catalog {
        models,
        default_models: pairs(&json["default_models"])?,
        suites,
        state_contracts,
        life_state_psychology: texts(&json["life_state_psychology"])?,
        mean_reverting_psychology: texts(&json["mean_reverting_psychology"])?,
    })
}

/// The catalog, read once.
pub fn catalog() -> Result<&'static Catalog> {
    static CATALOG: OnceLock<std::result::Result<Catalog, Error>> = OnceLock::new();
    CATALOG.get_or_init(read).as_ref().map_err(Clone::clone)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a model of the catalog must declare (as Python's registry did before #475): its
    /// domain, reference, formula, source and versions, no claim of external validity, and each
    /// parameter's config path, unit and ordered, finite bounds around its default.
    #[test]
    fn each_model_declares_its_formula_source_and_bounded_parameters() {
        let number = |value: &Value| value.as_f64().filter(|_| value.is_number());
        for (id, model) in definitions().unwrap() {
            for field in ["domain", "reference", "formula", "assumption_source_id", "implementation_version"] {
                assert!(model[field].as_str().is_some_and(|text| !text.is_empty()), "{id}: {field}");
            }
            assert!(model["state_schema_version"].as_i64().is_some_and(|version| version >= 1), "{id}");
            assert_eq!(model["external_validation"], Value::Bool(false), "{id}");
            assert!(model["required_state_contracts"].is_object(), "{id}");
            for (path, parameter) in model["parameters"].as_object().unwrap() {
                assert!(path.contains('.') && path.len() <= 128, "{id}: {path}");
                assert!(parameter["unit"].as_str().is_some_and(|unit| !unit.is_empty()), "{id}: {path}");
                assert!(parameter["source_id"].as_str().is_some(), "{id}: {path}");
                let (lower, upper, default) = (number(&parameter["lower"]), number(&parameter["upper"]), number(&parameter["default"]));
                for bound in [lower, upper, default].into_iter().flatten() {
                    assert!(bound.is_finite(), "{id}: {path}");
                }
                if let (Some(lower), Some(upper)) = (lower, upper) {
                    assert!(lower <= upper, "{id}: {path}");
                }
                if let (Some(lower), Some(default)) = (lower, default) {
                    let exclusive = parameter["lower_exclusive"].as_bool().unwrap_or(false);
                    assert!(default > lower || (!exclusive && default == lower), "{id}: {path}");
                }
                if let (Some(upper), Some(default)) = (upper, default) {
                    assert!(default <= upper, "{id}: {path}");
                }
            }
        }
    }

    #[test]
    fn the_defaults_and_suites_select_models_of_their_domains() {
        let catalog = catalog().unwrap();
        for (domain, model) in catalog.default_models.iter().chain(catalog.suites.values().flatten()) {
            assert_eq!(&*catalog.models[&**model].domain, &**domain, "{model}");
        }
        for model in catalog.life_state_psychology.iter().chain(&catalog.mean_reverting_psychology) {
            assert_eq!(&*catalog.models[&**model].domain, "psychology", "{model}");
        }
    }
}
