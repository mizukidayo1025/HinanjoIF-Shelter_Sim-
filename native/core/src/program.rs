//! The EffectProgram of an intervention, before it is scheduled (#304, under #292): its contract
//! (`EffectProgram`), read strictly and dumped with its defaults as
//! `program.model_dump(mode="json")` writes it, and its checks on a world (`validate`) for the
//! six operators the offline reading of a text makes (`facility_outage`, `add_staff`, `add_toilet`,
//! `water_outage`, `add_evacuees`, `depart_evacuees`), with their texts.
//!
//! The approval's hash is the sha256 of the dump's canonical JSON (`digest(program.model_dump())`,
//! `storage/projects.py`): `dump_json` gives those bytes.
//!
//! What the browser does not repeat, and says so:
//! - A program the contract refuses is a `Value` error whose text is the core's, not pydantic's.
//!   The reader takes the JSON types a dump has (an int where an int is, a bool where a bool is),
//!   and refuses a value pydantic would coerce. The screen's builder (#584) sends a typed program
//!   built from `catalog`, the reading of a text makes its dump.
//! - The world's own check (`validate::world`, E6 of #250) is not run: a valid world gives no
//!   issue there.
//! - An effect of another operator of the registry, or parameters its model refuses (whose issue
//!   holds pydantic's text), is `Unsupported`.
//! - A guard's fields are read in sorted order (the contract this was ported from read them as a
//!   set, whose order changed between processes).

use std::collections::BTreeSet;
use std::sync::Arc;

use indexmap::IndexMap;

use crate::entities::{Entities, FacilityRow};
use crate::error::{Error, Result};
use crate::events::EventRow;
use crate::geometry::Geo;
use crate::row::ValueRow;
use crate::rules::kit::Held;
use crate::rules::overlay;
use crate::state::effect_program::{EffectProgramRow, NAME as EFFECT_PROGRAM};
use crate::text::py_repr;
use crate::value::V;
use crate::world::{Place, World, MODEL_CONFIGS};

type Map = IndexMap<Arc<str>, V>;

const MAX_EFFECTS: usize = 50;
const MAX_ASSUMPTIONS: usize = 100;
const MAX_IDS: usize = 50;
const MAX_CANDIDATES: usize = 100;
const MAX_SCHEDULED_EVENTS: usize = 100;
const MAX_QUEUE: usize = 10_000;
const MAX_EFFECT_DURATION: i64 = 60 * 24 * 366;
const SUPPORT_MODES: [&str; 5] = ["native", "composed", "approximate", "clarification_required", "not_simulatable"];
const GUARD_OPERATORS: [&str; 8] = ["eq", "ne", "gt", "gte", "lt", "lte", "and", "or"];
const GUARD_FIELDS: [&str; 7] =
    ["simulation.minute", "target.capacity", "target.working_units", "target.queue_length", "zone.occupancy", "resource.available_quantity", "staff.available"];

// ---- The contract -----------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Selector {
    pub kind: Arc<str>,
    pub target_id: Option<Arc<str>>,
    pub candidate_ids: Vec<Arc<str>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Guard {
    pub operator: Arc<str>,
    pub field: Option<Arc<str>>,
    /// The scalar of a comparison, `V::Null` for none.
    pub value: V,
    pub operands: Vec<Guard>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Effect {
    pub effect_id: Arc<str>,
    pub op: Arc<str>,
    pub target_selector: Selector,
    pub start_minute: i64,
    pub end_minute: Option<i64>,
    pub guard: Option<Guard>,
    /// The parameters as given (`dict[str, Any]`), a `V::Map`.
    pub parameters: V,
    pub units: Arc<str>,
    pub source_id: Arc<str>,
    pub assumption_ids: Vec<Arc<str>>,
    pub policy_dependencies: Vec<Arc<str>>,
    pub causation_id: Option<Arc<str>>,
    pub reversible: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Assumption {
    pub assumption_id: Arc<str>,
    pub description: Arc<str>,
    pub source_id: Arc<str>,
    pub units: Option<Arc<str>>,
    pub value: V,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Provenance {
    pub compiler_kind: Arc<str>,
    pub compiler_version: Arc<str>,
    pub model_id: Option<Arc<str>>,
    pub prompt_hash: Option<Arc<str>>,
}

/// `EffectProgram`, as its contract holds it.
#[derive(Clone, Debug, PartialEq)]
pub struct EffectProgram {
    pub program_id: Arc<str>,
    pub narrative: Arc<str>,
    pub source_text: Arc<str>,
    pub simulation_start_minute: i64,
    pub effects: Vec<Effect>,
    pub assumptions: Vec<Assumption>,
    pub support_mode: Arc<str>,
    pub compiler_provenance: Provenance,
}

fn refuse<T>(what: impl std::fmt::Display) -> Result<T> {
    Err(Error::Value(format!("invalid EffectProgram: {what}")))
}

/// The fields of a dict with no others (`extra="forbid"`).
struct Fields<'a> {
    map: &'a Map,
    what: &'static str,
}

impl<'a> Fields<'a> {
    fn of(value: &'a V, what: &'static str, allowed: &[&str]) -> Result<Self> {
        let V::Map(map) = value else { return refuse(format!("{what} is not an object")) };
        if let Some(extra) = map.keys().find(|key| !allowed.contains(&&***key)) {
            return refuse(format!("{what} has the extra field {}", py_repr(extra)));
        }
        Ok(Self { map, what })
    }

    fn get(&self, name: &str) -> Option<&'a V> {
        self.map.get(name)
    }

    fn required(&self, name: &str) -> Result<&'a V> {
        self.map.get(name).map_or_else(|| refuse(format!("{} has no {name}", self.what)), Ok)
    }

    fn stable_id(&self, name: &str) -> Result<Arc<str>> {
        stable_id(self.required(name)?, self.what, name)
    }

    fn optional_id(&self, name: &str) -> Result<Option<Arc<str>>> {
        match self.get(name) {
            None | Some(V::Null) => Ok(None),
            Some(value) => stable_id(value, self.what, name).map(Some),
        }
    }

    fn text(&self, name: &str, max: usize) -> Result<Arc<str>> {
        match self.required(name)? {
            V::Str(text) if (1..=max).contains(&text.chars().count()) => Ok(text.clone()),
            _ => refuse(format!("{}.{name} is not a string of 1 to {max} characters", self.what)),
        }
    }

    fn minute(&self, name: &str) -> Result<i64> {
        minute(self.required(name)?, self.what, name)
    }

    fn ids(&self, name: &str, max: usize) -> Result<Vec<Arc<str>>> {
        match self.get(name) {
            None => Ok(Vec::new()),
            Some(V::List(items)) if items.len() <= max => items.iter().map(|item| stable_id(item, self.what, name)).collect(),
            Some(_) => refuse(format!("{}.{name} is not a list of at most {max} IDs", self.what)),
        }
    }
}

/// `StableId`: 1 to 64 characters, a letter or digit of ASCII first, then those or `._:-`.
pub fn is_stable_id(text: &str) -> bool {
    let bytes = text.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && bytes[0].is_ascii_alphanumeric()
        && bytes.iter().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

fn stable_id(value: &V, what: &str, name: &str) -> Result<Arc<str>> {
    match value {
        V::Str(text) if is_stable_id(text) => Ok(text.clone()),
        _ => refuse(format!("{what}.{name} is not a stable ID")),
    }
}

fn minute(value: &V, what: &str, name: &str) -> Result<i64> {
    match value {
        V::Int(minute) if *minute >= 0 => Ok(*minute),
        _ => refuse(format!("{what}.{name} is not a whole minute, not negative")),
    }
}

fn finite(value: &V) -> bool {
    !matches!(value, V::Float(number) if !number.is_finite())
}

fn selector(value: &V) -> Result<Selector> {
    let fields = Fields::of(value, "target_selector", &["kind", "target_id", "candidate_ids"])?;
    let found =
        Selector { kind: fields.stable_id("kind")?, target_id: fields.optional_id("target_id")?, candidate_ids: fields.ids("candidate_ids", MAX_CANDIDATES)? };
    if found.target_id.is_none() && found.candidate_ids.is_empty() {
        return refuse("a target ID or candidate IDs are required");
    }
    if found.target_id.is_some() && !found.candidate_ids.is_empty() {
        return refuse("a selected target cannot also contain candidate IDs");
    }
    if found.candidate_ids.iter().collect::<BTreeSet<_>>().len() != found.candidate_ids.len() {
        return refuse("candidate target IDs must be unique");
    }
    Ok(found)
}

fn guard(value: &V) -> Result<Guard> {
    let fields = Fields::of(value, "guard", &["operator", "field", "value", "operands"])?;
    let operator = match fields.required("operator")? {
        V::Str(operator) if GUARD_OPERATORS.contains(&&**operator) => operator.clone(),
        _ => return refuse("a guard's operator is not one of the guard operators"),
    };
    let field = match fields.get("field") {
        None | Some(V::Null) => None,
        Some(V::Str(field)) if GUARD_FIELDS.contains(&&**field) => Some(field.clone()),
        Some(_) => return refuse("a guard's field is not one of the guard fields"),
    };
    let value = match fields.get("value") {
        None => V::Null,
        Some(value @ (V::Null | V::Bool(_) | V::Int(_) | V::Float(_))) => value.clone(),
        Some(V::Str(text)) if text.chars().count() <= 256 => V::Str(text.clone()),
        Some(V::Str(_)) => return refuse("guard string values are limited to 256 characters"),
        Some(_) => return refuse("a guard's value is not a scalar"),
    };
    if !finite(&value) {
        return refuse("guard numeric values must be finite");
    }
    let operands = match fields.get("operands") {
        None => Vec::new(),
        Some(V::List(items)) if items.len() <= 4 => items.iter().map(guard).collect::<Result<_>>()?,
        Some(_) => return refuse("a guard's operands are not a list of at most 4"),
    };
    let found = Guard { operator, field, value, operands };
    if ["eq", "ne", "gt", "gte", "lt", "lte"].contains(&&*found.operator) {
        if found.field.is_none() || matches!(found.value, V::Null) || !found.operands.is_empty() {
            return refuse("comparison guards require one allowed field and scalar value");
        }
        if ["gt", "gte", "lt", "lte"].contains(&&*found.operator) && !matches!(found.value, V::Int(_) | V::Float(_)) {
            return refuse("ordered comparisons require a numeric value");
        }
    } else if found.field.is_some() || !matches!(found.value, V::Null) || found.operands.len() < 2 {
        return refuse("and/or guards require two or more operands and no leaf fields");
    }
    fn measure(node: &Guard, depth: usize) -> Result<usize> {
        if depth > 4 {
            return refuse("guard nesting depth cannot exceed four");
        }
        let mut count = 1;
        for child in &node.operands {
            count += measure(child, depth + 1)?;
        }
        Ok(count)
    }
    if measure(&found, 1)? > 20 {
        return refuse("guard AST cannot exceed 20 nodes");
    }
    Ok(found)
}

/// The checks of an effect's JSON parameters: at most 8 deep, numbers that fit 64 bits, and the
/// other bounds of a program's JSON.
fn bounded(value: &V, depth: usize) -> Result<()> {
    if depth > 8 {
        return refuse("effect JSON parameters exceed maximum nesting depth");
    }
    match value {
        V::Null | V::Bool(_) | V::Int(_) => Ok(()),
        V::Big(_) => refuse("effect JSON numbers must fit 64 bits"),
        V::Str(text) if text.chars().count() > 1000 => refuse("effect JSON strings are limited to 1000 characters"),
        V::Str(_) => Ok(()),
        V::Float(number) if !number.is_finite() => refuse("effect JSON numbers must be finite"),
        V::Float(_) => Ok(()),
        V::List(items) if items.len() > 100 => refuse("effect JSON arrays are limited to 100 items"),
        V::List(items) => items.iter().try_for_each(|item| bounded(item, depth + 1)),
        V::Map(map) if map.len() > 100 || map.keys().any(|key| key.chars().count() > 256) => {
            refuse("effect JSON objects need short string keys and at most 100 fields")
        }
        V::Map(map) => map.values().try_for_each(|item| bounded(item, depth + 1)),
    }
}

fn effect(value: &V) -> Result<Effect> {
    let fields = Fields::of(
        value,
        "effect",
        &[
            "effect_id",
            "op",
            "target_selector",
            "start_minute",
            "end_minute",
            "guard",
            "parameters",
            "units",
            "source_id",
            "assumption_ids",
            "policy_dependencies",
            "causation_id",
            "reversible",
        ],
    )?;
    let parameters = match fields.get("parameters") {
        None => V::Map(Map::new()),
        Some(parameters @ V::Map(_)) => parameters.clone(),
        Some(_) => return refuse("an effect's parameters are not an object"),
    };
    bounded(&parameters, 0)?;
    let end_minute = match fields.get("end_minute") {
        None | Some(V::Null) => None,
        Some(value) => Some(minute(value, "effect", "end_minute")?),
    };
    let found = Effect {
        effect_id: fields.stable_id("effect_id")?,
        op: fields.stable_id("op")?,
        target_selector: selector(fields.required("target_selector")?)?,
        start_minute: fields.minute("start_minute")?,
        end_minute,
        guard: match fields.get("guard") {
            None | Some(V::Null) => None,
            Some(value) => Some(guard(value)?),
        },
        parameters,
        units: fields.stable_id("units")?,
        source_id: fields.stable_id("source_id")?,
        assumption_ids: fields.ids("assumption_ids", MAX_IDS)?,
        policy_dependencies: fields.ids("policy_dependencies", MAX_IDS)?,
        causation_id: fields.optional_id("causation_id")?,
        reversible: match fields.get("reversible") {
            None => true,
            Some(V::Bool(flag)) => *flag,
            Some(_) => return refuse("an effect's reversible is not a bool"),
        },
    };
    if found.end_minute.is_some_and(|end| end <= found.start_minute) {
        return refuse("end_minute must be later than start_minute");
    }
    Ok(found)
}

fn assumption(value: &V) -> Result<Assumption> {
    let fields = Fields::of(value, "assumption", &["assumption_id", "description", "source_id", "units", "value"])?;
    let value = match fields.get("value") {
        None => V::Null,
        Some(value @ (V::Null | V::Bool(_) | V::Int(_) | V::Float(_) | V::Str(_))) => value.clone(),
        Some(_) => return refuse("an assumption's value is not a scalar"),
    };
    if !finite(&value) {
        return refuse("assumption values must be finite");
    }
    Ok(Assumption {
        assumption_id: fields.stable_id("assumption_id")?,
        description: fields.text("description", 500)?,
        source_id: fields.stable_id("source_id")?,
        units: fields.optional_id("units")?,
        value,
    })
}

fn provenance(value: &V) -> Result<Provenance> {
    let fields = Fields::of(value, "compiler_provenance", &["compiler_kind", "compiler_version", "model_id", "prompt_hash"])?;
    let compiler_kind = match fields.required("compiler_kind")? {
        V::Str(kind) if matches!(&**kind, "offline_rule" | "llm" | "manual") => kind.clone(),
        _ => return refuse("compiler_kind is not offline_rule, llm or manual"),
    };
    let prompt_hash = match fields.get("prompt_hash") {
        None | Some(V::Null) => None,
        Some(V::Str(hash)) if hash.chars().count() <= 128 => Some(hash.clone()),
        Some(_) => return refuse("prompt_hash is not a string of at most 128 characters"),
    };
    Ok(Provenance { compiler_kind, compiler_version: fields.stable_id("compiler_version")?, model_id: fields.optional_id("model_id")?, prompt_hash })
}

impl EffectProgram {
    /// `EffectProgram.model_validate(value)`, strictly (above).
    pub fn read(value: &V) -> Result<Self> {
        let fields = Fields::of(
            value,
            "program",
            &[
                "schema_version",
                "program_id",
                "narrative",
                "source_text",
                "simulation_start_minute",
                "effects",
                "assumptions",
                "support_mode",
                "compiler_provenance",
            ],
        )?;
        if !matches!(fields.get("schema_version"), None | Some(V::Int(1))) {
            return refuse("schema_version is not 1");
        }
        let effects = match fields.required("effects")? {
            V::List(items) if (1..=MAX_EFFECTS).contains(&items.len()) => items.iter().map(effect).collect::<Result<Vec<_>>>()?,
            _ => return refuse(format!("effects is not a list of 1 to {MAX_EFFECTS}")),
        };
        let assumptions = match fields.get("assumptions") {
            None => Vec::new(),
            Some(V::List(items)) if items.len() <= MAX_ASSUMPTIONS => items.iter().map(assumption).collect::<Result<Vec<_>>>()?,
            Some(_) => return refuse(format!("assumptions is not a list of at most {MAX_ASSUMPTIONS}")),
        };
        let support_mode = match fields.required("support_mode")? {
            V::Str(mode) if SUPPORT_MODES.contains(&&**mode) => mode.clone(),
            _ => return refuse("support_mode is not a support mode"),
        };
        let program = EffectProgram {
            program_id: fields.stable_id("program_id")?,
            narrative: fields.text("narrative", 1000)?,
            source_text: fields.text("source_text", 4000)?,
            simulation_start_minute: fields.minute("simulation_start_minute")?,
            effects,
            assumptions,
            support_mode,
            compiler_provenance: provenance(fields.required("compiler_provenance")?)?,
        };
        if program.effects.iter().any(|effect| effect.start_minute < program.simulation_start_minute) {
            return refuse("effects cannot start before simulation_start_minute");
        }
        if program.effects.iter().map(|effect| &effect.effect_id).collect::<BTreeSet<_>>().len() != program.effects.len() {
            return refuse("effect IDs must be unique");
        }
        let declared: BTreeSet<&Arc<str>> = program.assumptions.iter().map(|item| &item.assumption_id).collect();
        if declared.len() != program.assumptions.len() {
            return refuse("assumption IDs must be unique");
        }
        if program.effects.iter().any(|effect| effect.assumption_ids.iter().any(|id| !declared.contains(id))) {
            return refuse("effects may reference only declared assumptions");
        }
        Ok(program)
    }

    /// `read` of a program's JSON bytes.
    pub fn of_json(raw: &[u8]) -> Result<Self> {
        let value: serde_json::Value = serde_json::from_slice(raw).map_err(|error| Error::Value(format!("invalid EffectProgram: not JSON: {error}")))?;
        let value = V::from_json(&value).ok_or_else(|| Error::Value("invalid EffectProgram: a number the core cannot read".to_owned()))?;
        Self::read(&value)
    }

    /// `program.model_dump(mode="json")`: every field, the defaults too.
    pub fn dump(&self) -> V {
        object([
            ("schema_version", V::Int(1)),
            ("program_id", string(&self.program_id)),
            ("narrative", string(&self.narrative)),
            ("source_text", string(&self.source_text)),
            ("simulation_start_minute", V::Int(self.simulation_start_minute)),
            ("effects", V::List(self.effects.iter().map(Effect::dump).collect())),
            ("assumptions", V::List(self.assumptions.iter().map(Assumption::dump).collect())),
            ("support_mode", string(&self.support_mode)),
            ("compiler_provenance", self.compiler_provenance.dump()),
        ])
    }

    /// The canonical JSON of `dump`: the bytes whose sha256 is the approval's hash.
    pub fn dump_json(&self) -> Result<Vec<u8>> {
        self.dump().to_json()
    }
}

impl Effect {
    fn dump(&self) -> V {
        object([
            ("effect_id", string(&self.effect_id)),
            ("op", string(&self.op)),
            ("target_selector", self.target_selector.dump()),
            ("start_minute", V::Int(self.start_minute)),
            ("end_minute", self.end_minute.map_or(V::Null, V::Int)),
            ("guard", self.guard.as_ref().map_or(V::Null, Guard::dump)),
            ("parameters", self.parameters.clone()),
            ("units", string(&self.units)),
            ("source_id", string(&self.source_id)),
            ("assumption_ids", ids(&self.assumption_ids)),
            ("policy_dependencies", ids(&self.policy_dependencies)),
            ("causation_id", optional(&self.causation_id)),
            ("reversible", V::Bool(self.reversible)),
        ])
    }
}

impl Selector {
    fn dump(&self) -> V {
        object([("kind", string(&self.kind)), ("target_id", optional(&self.target_id)), ("candidate_ids", ids(&self.candidate_ids))])
    }
}

impl Guard {
    fn dump(&self) -> V {
        object([
            ("operator", string(&self.operator)),
            ("field", optional(&self.field)),
            ("value", self.value.clone()),
            ("operands", V::List(self.operands.iter().map(Guard::dump).collect())),
        ])
    }

    /// The fields of the guard's leaves.
    fn fields(&self, into: &mut BTreeSet<Arc<str>>) {
        match &self.field {
            Some(field) => {
                into.insert(field.clone());
            }
            None => self.operands.iter().for_each(|operand| operand.fields(into)),
        }
    }
}

impl Assumption {
    fn dump(&self) -> V {
        object([
            ("assumption_id", string(&self.assumption_id)),
            ("description", string(&self.description)),
            ("source_id", string(&self.source_id)),
            ("units", optional(&self.units)),
            ("value", self.value.clone()),
        ])
    }
}

impl Provenance {
    fn dump(&self) -> V {
        object([
            ("compiler_kind", string(&self.compiler_kind)),
            ("compiler_version", string(&self.compiler_version)),
            ("model_id", optional(&self.model_id)),
            ("prompt_hash", optional(&self.prompt_hash)),
        ])
    }
}

fn object<const N: usize>(fields: [(&str, V); N]) -> V {
    V::Map(fields.into_iter().map(|(name, value)| (Arc::from(name), value)).collect())
}

fn string(text: &Arc<str>) -> V {
    V::Str(text.clone())
}

fn optional(text: &Option<Arc<str>>) -> V {
    text.clone().map_or(V::Null, V::Str)
}

fn ids(items: &[Arc<str>]) -> V {
    V::List(items.iter().cloned().map(V::Str).collect())
}

// ---- The operators ----------------------------------------------------------------------------------

/// What the primitives' registry says of an operator the browser checks.
struct Primitive {
    target_kinds: &'static [&'static str],
    units: &'static [&'static str],
    policies: &'static [&'static str],
    timed: bool,
}

fn primitive(op: &str) -> Option<Primitive> {
    let (target_kinds, units, policies, timed): (&[&str], &[&str], &[&str], bool) = match op {
        "facility_outage" => (&["facility"], &["facility_unit"], &["facility_capacity"], true),
        "add_staff" => (&["zone"], &["staff_member"], &["staff_operations"], false),
        "add_toilet" => (&["zone"], &["toilet_stall"], &["facility_capacity"], false),
        "add_evacuees" | "depart_evacuees" => (&["zone"], &["evacuee"], &["population_dynamics_v1"], false),
        "water_outage" => (&["facility", "zone"], &["water_supply"], &["operational_events_v1", "resource_service_v1"], true),
        _ => return None,
    };
    Some(Primitive { target_kinds, units, policies, timed })
}

/// Whether the core checks and schedules effects of the operator: the six the offline reading of a
/// text makes. The intervention compiler offers an LLM only these (#463).
pub fn supports(op: &str) -> bool {
    primitive(op).is_some()
}

// ---- The catalog ------------------------------------------------------------------------------------

/// What the screen's builder of an intervention (#584) shows of an operator the core schedules:
/// its name, the facility kinds a facility target must be (none: any), whether its effect needs
/// an end, and the parameters it asks for (name, label, unit); the others keep their defaults.
/// The parameters' types and ranges are the registry's (`interpret/primitives.json`, the schema an
/// LLM is given too).
struct Form {
    op: &'static str,
    label: &'static str,
    facility_kinds: &'static [&'static str],
    end_required: bool,
    parameters: &'static [(&'static str, &'static str, &'static str)],
}

/// The operators the core schedules, in the order the builder offers them.
const FORMS: [Form; 6] = [
    Form { op: "add_toilet", label: "トイレを増やす", facility_kinds: &[], end_required: false, parameters: &[("count", "増やす数", "基")] },
    Form { op: "add_staff", label: "スタッフを増やす", facility_kinds: &[], end_required: false, parameters: &[("count", "増やす人数", "人")] },
    Form {
        op: "facility_outage",
        label: "設備を止める（故障・使用停止）",
        facility_kinds: &[],
        end_required: false,
        parameters: &[("units", "止める数", "基")],
    },
    Form { op: "water_outage", label: "断水する", facility_kinds: &["water_station"], end_required: true, parameters: &[] },
    Form {
        op: "add_evacuees", label: "避難者が到着する", facility_kinds: &[], end_required: false, parameters: &[("count", "到着する人数", "人")]
    },
    Form {
        op: "depart_evacuees",
        label: "避難者が退所する",
        facility_kinds: &[],
        end_required: false,
        parameters: &[("count", "退所する人数", "人"), ("include_initial", "最初からいた避難者も含める", "")],
    },
];

/// The catalog of the operators the core schedules (`GET …/contracts`'s
/// `executable_effect_primitives`), from which the screen builds an EffectProgram: each one's
/// `op`, `label`, the registry's `description`, `target_kinds`, `target_facility_kinds`, `units`,
/// `required_policies`, `timed`, `end_required` and the `parameters` the builder asks for, each
/// with its `name`, `label`, `unit`, the registry's `type` and, where it has them, `minimum`,
/// `maximum`, `default` and `enum`, and `required`.
pub fn catalog() -> Result<V> {
    let texts = |items: &[&str]| V::List(items.iter().map(|item| V::Str(Arc::from(*item))).collect());
    let mut entries = Vec::new();
    for form in &FORMS {
        let primitive = primitive(form.op).ok_or_else(|| Error::Runtime(format!("the catalog names {}, which the core does not schedule", form.op)))?;
        let entry = crate::interpret::registry_entry(form.op)?.ok_or_else(|| Error::Runtime(format!("the registry lacks {}", form.op)))?;
        let V::Map(entry) = entry else { return Err(Error::Runtime("a registry entry is not a dict".to_owned())) };
        let schema = match entry.get("parameters_schema") {
            Some(V::Map(schema)) => schema,
            _ => return Err(Error::Runtime(format!("{} has no parameters' schema", form.op))),
        };
        let required: Vec<&str> = match schema.get("required") {
            Some(V::List(names)) => names.iter().filter_map(|name| if let V::Str(name) = name { Some(&**name) } else { None }).collect(),
            _ => Vec::new(),
        };
        let mut parameters = Vec::new();
        for (name, label, unit) in form.parameters {
            let property = match schema.get("properties") {
                Some(V::Map(properties)) => properties.get(*name),
                _ => None,
            };
            let Some(V::Map(property)) = property else { return Err(Error::Runtime(format!("{} has no parameter {name}", form.op))) };
            let kind = match property.get("type") {
                Some(V::Str(kind)) => kind.clone(),
                _ => return Err(Error::Runtime(format!("{}.{name} has no type", form.op))),
            };
            let mut item: Map = IndexMap::new();
            let mut put = |key: &str, value: V| {
                item.insert(Arc::from(key), value);
            };
            put("name", V::Str(Arc::from(*name)));
            put("label", V::Str(Arc::from(*label)));
            put("unit", V::Str(Arc::from(*unit)));
            put("type", V::Str(kind.clone()));
            // An integer above an exclusive minimum is at least the next one.
            match (property.get("minimum"), property.get("exclusiveMinimum")) {
                (Some(minimum), _) => put("minimum", minimum.clone()),
                (None, Some(V::Int(bound))) if &*kind == "integer" => put("minimum", V::Int(bound + 1)),
                (None, Some(_)) => return Err(Error::Runtime(format!("{}.{name} has an exclusive minimum the builder cannot show", form.op))),
                (None, None) => {}
            }
            for key in ["maximum", "default", "enum"] {
                if let Some(value) = property.get(key) {
                    put(key, value.clone());
                }
            }
            put("required", V::Bool(required.contains(name)));
            parameters.push(V::Map(item));
        }
        entries.push(object([
            ("op", V::Str(Arc::from(form.op))),
            ("label", V::Str(Arc::from(form.label))),
            ("description", entry.get("description").cloned().unwrap_or(V::Null)),
            ("target_kinds", texts(primitive.target_kinds)),
            ("target_facility_kinds", texts(form.facility_kinds)),
            ("units", texts(primitive.units)),
            ("required_policies", texts(primitive.policies)),
            ("timed", V::Bool(primitive.timed)),
            ("end_required", V::Bool(form.end_required)),
            ("parameters", V::List(parameters)),
        ]));
    }
    Ok(V::List(entries))
}

/// The parameters of an effect, as its model reads them; `Unsupported` where the model refuses
/// them (the issue would hold pydantic's text).
#[derive(Default)]
struct Parameters {
    count: i64,
    units: i64,
    admission_zone_ids: Vec<Arc<str>>,
    approach_route_id: Option<Arc<str>>,
}

fn parameters(effect: &Effect) -> Result<Parameters> {
    let unsupported = || Error::unsupported("parameters their model refuses: the issue holds pydantic's text");
    let V::Map(map) = &effect.parameters else { return Err(unsupported()) };
    let allowed: &[&str] = match &*effect.op {
        "facility_outage" => &["units"],
        "add_staff" | "add_toilet" => &["count"],
        "water_outage" => &["outage"],
        "add_evacuees" => &["count", "household_size", "mobility", "language", "admission_zone_ids", "approach_route_id", "person_context_overrides"],
        "depart_evacuees" => &["count", "include_initial"],
        _ => return Err(unsupported()),
    };
    if map.keys().any(|key| !allowed.contains(&&**key)) {
        return Err(unsupported());
    }
    let int = |name: &str, low: i64, high: i64, default: Option<i64>| -> Result<i64> {
        match (map.get(name), default) {
            (Some(V::Int(value)), _) if (low..=high).contains(value) => Ok(*value),
            (None, Some(default)) => Ok(default),
            _ => Err(unsupported()),
        }
    };
    let mut found = Parameters::default();
    match &*effect.op {
        "facility_outage" => found.units = int("units", 1, 1000, None)?,
        "add_staff" => found.count = int("count", 1, 100, None)?,
        "add_toilet" => found.count = int("count", 1, 20, None)?,
        "water_outage" => {
            if !matches!(map.get("outage"), None | Some(V::Bool(true))) {
                return Err(unsupported());
            }
        }
        "depart_evacuees" => {
            found.count = int("count", 1, 1000, None)?;
            if !matches!(map.get("include_initial"), None | Some(V::Bool(_))) {
                return Err(unsupported());
            }
        }
        _ => {
            found.count = int("count", 1, 1000, None)?;
            int("household_size", 1, 12, Some(1))?;
            match map.get("mobility") {
                None => {}
                Some(V::Str(mobility)) if matches!(&**mobility, "independent" | "assisted" | "wheelchair") => {}
                _ => return Err(unsupported()),
            }
            match map.get("language") {
                None => {}
                Some(V::Str(language)) if is_stable_id(language) => {}
                _ => return Err(unsupported()),
            }
            found.admission_zone_ids = match map.get("admission_zone_ids") {
                None => Vec::new(),
                Some(V::List(items)) if items.len() <= 16 => items
                    .iter()
                    .map(|item| match item {
                        V::Str(id) if is_stable_id(id) => Ok(id.clone()),
                        _ => Err(unsupported()),
                    })
                    .collect::<Result<_>>()?,
                Some(_) => return Err(unsupported()),
            };
            if found.admission_zone_ids.iter().collect::<BTreeSet<_>>().len() != found.admission_zone_ids.len() {
                return Err(unsupported());
            }
            found.approach_route_id = match map.get("approach_route_id") {
                None | Some(V::Null) => None,
                Some(V::Str(id)) if is_stable_id(id) => Some(id.clone()),
                Some(_) => return Err(unsupported()),
            };
            // The people's contexts are a contract of their own, which the reading of a text
            // never gives.
            match map.get("person_context_overrides") {
                None => {}
                Some(V::List(items)) if items.is_empty() => {}
                _ => return Err(Error::unsupported("arrivals with people's contexts are not checked in the browser")),
            }
        }
    }
    Ok(found)
}

// ---- The world ---------------------------------------------------------------------------------------

fn has(world: &World, name: &str) -> Result<bool> {
    Ok(match world.slots.get(name) {
        Some(slot) => {
            let slot = slot.try_borrow()?;
            slot.place != Place::Field && slot.row.is_some()
        }
        None => false,
    })
}

/// `model_state.get(name)` of a namespace held as a value; None without the key.
fn value(world: &World, name: &str) -> Result<Option<V>> {
    if !has(world, name)? {
        return Ok(None);
    }
    let slot = world.slots.get(name).ok_or_else(|| Error::Runtime("a slot left the world".to_owned()))?.try_borrow()?;
    Ok(Some(slot.row::<ValueRow>()?.0.clone()))
}

fn configs(world: &World) -> Result<Map> {
    match &world.slots.try_row::<ValueRow>(MODEL_CONFIGS)?.0 {
        V::Map(configs) => Ok(configs.clone()),
        _ => Err(Error::unsupported("the model configs are not a dict")),
    }
}

fn pending(world: &World) -> Result<Vec<Arc<EventRow>>> {
    let queue = world.queue()?;
    queue.ordered().iter().map(|(_, serial)| queue.slot(*serial).row.clone().ok_or_else(|| Error::unsupported("a pending event has no row"))).collect()
}

fn payload<'a>(row: &'a EventRow, key: &str) -> Option<&'a V> {
    match &row.payload {
        V::Map(map) => map.get(key),
        _ => None,
    }
}

/// `sum(event.payload.get("count", 0) ...)` of the pending events of this type and operator.
fn pending_count(events: &[Arc<EventRow>], event_type: &str, op: &str) -> Result<i64> {
    let mut total = 0i64;
    for event in events {
        if &*event.event_type != event_type || !matches!(payload(event, "op"), Some(V::Str(found)) if &**found == op) {
            continue;
        }
        match payload(event, "count") {
            None => {}
            Some(V::Int(count)) => total = total.checked_add(*count).ok_or_else(|| Error::unsupported("a count past 64 bits"))?,
            Some(V::Bool(flag)) => total += i64::from(*flag),
            Some(_) => return Err(Error::unsupported("a pending count that is not an int")),
        }
    }
    Ok(total)
}

fn occupancy(facility: &FacilityRow, schema_version: i64) -> i64 {
    let mut count = facility.active_agent_ids.len();
    if schema_version == 6 {
        count += facility.resource_active_agent_ids.len();
    }
    count as i64
}

fn list_repr(items: &[&str]) -> String {
    let mut sorted: Vec<&str> = items.to_vec();
    sorted.sort_unstable();
    format!("[{}]", sorted.iter().map(|item| py_repr(item)).collect::<Vec<_>>().join(", "))
}

/// How the checks place added toilets: whether the Scenario places installed facilities on its
/// floor plan (`rules::overlay::placement_enabled`), and the floor plan of a world with its
/// overlay as it is (`Host::floor_plan` of that world).
pub struct Placement<'a> {
    pub enabled: bool,
    pub plan: &'a mut dyn FnMut(&World) -> Result<Arc<Geo>>,
}

// ---- The checks on a world --------------------------------------------------------------------------

/// The checks of a program on a world: the issues, in the order of the checks, each once.
/// It changes nothing. `Unsupported` for a program the browser does not check (above).
pub fn validate(world: &World, program: &EffectProgram, placement: &mut Placement<'_>) -> Result<Vec<String>> {
    for effect in &program.effects {
        if primitive(&effect.op).is_none() && crate::rules::effects::OPERATORS.contains(&&*effect.op) {
            return Err(Error::unsupported("an operator the offline reading of a text never makes"));
        }
    }
    let rows = world.rows()?;
    let minute = world.minute_index()?;
    let duration = world.duration_minutes()?;
    let schema_version = world.schema_version()?;
    let configs = configs(world)?;
    let events = pending(world)?;
    let mut issues: Vec<String> = Vec::new();

    program_issues(world, program, minute, duration, &events, &mut issues)?;

    let temporary = temporary_capacity(world, &events)?;

    let zone_known = |id: &str| rows.zones.contains_key(id);
    let mut added = Added::default();
    let cx = Checks { world, rows: &rows, configs: &configs, minute, duration, schema_version, zone_known: &zone_known };
    for effect in &program.effects {
        check_effect(cx, effect, &mut added, &mut issues)?;
    }
    let Added { facility_intervals, added_staff, added_facilities, added_evacuees, toilet_additions } = added;

    interval_issues(&rows, &events, facility_intervals, schema_version, &mut issues)?;

    if let Some(config) = configs.get("zone_environment") {
        environment_limits(world, config, &events, &zone_known, minute, duration, &mut issues)?;
    }

    let staff_total = rows.staff.len() as i64 + pending_count(&events, "effect_program_structure", "add_staff")? + added_staff;
    if staff_total > 100 {
        issues.push("program exceeds the 100-staff runtime bound".to_owned());
    }
    if temporary.values().any(|capacity| *capacity > 1000) {
        issues.push("temporary person capacity exceeds the 1000-person per-zone bound".to_owned());
    }
    let facility_total = rows.facilities.len() as i64 + pending_count(&events, "effect_program_structure", "add_toilet")? + added_facilities;
    if facility_total > 200 {
        issues.push("program exceeds the 200-facility runtime bound".to_owned());
    }
    drop(rows);
    if !toilet_additions.is_empty() && configs.contains_key("facility_geometry") {
        issues.extend(toilet_placement(world, &toilet_additions, placement)?);
    }
    population_issues(world, &events, added_evacuees, &mut issues)?;
    let mut seen = BTreeSet::new();
    issues.retain(|issue| seen.insert(issue.clone()));
    Ok(issues)
}

/// The checks of the program as a whole: its support mode, assumptions and start, whether this
/// branch scheduled it already, and its event counts.
fn program_issues(world: &World, program: &EffectProgram, minute: i64, duration: i64, events: &[Arc<EventRow>], issues: &mut Vec<String>) -> Result<()> {
    let mode = &*program.support_mode;
    if matches!(mode, "clarification_required" | "not_simulatable") {
        issues.push(format!("program support mode {} cannot be executed", py_repr(mode)));
    }
    if mode == "approximate" && program.assumptions.is_empty() {
        issues.push("approximate programs must include explicit assumptions".to_owned());
    }
    if program.simulation_start_minute < minute {
        issues.push("simulation_start_minute is before the current branch minute".to_owned());
    }
    if has(world, EFFECT_PROGRAM)? {
        let slot = world.slots.get(EFFECT_PROGRAM).ok_or_else(|| Error::Runtime("a slot left the world".to_owned()))?.try_borrow()?;
        let scheduled = match slot.row::<EffectProgramRow>() {
            Ok(row) => row.programs.contains_key(&*program.program_id),
            Err(_) => return Err(Error::unsupported("effect_program_v1 is not held as its row")),
        };
        if scheduled {
            issues.push(format!("program_id {} was already scheduled on this branch", py_repr(&program.program_id)));
        }
    }
    let expected_events: usize = program.effects.iter().map(|effect| if effect.end_minute.is_some() { 2 } else { 1 }).sum();
    if expected_events > MAX_SCHEDULED_EVENTS {
        issues.push("program exceeds the maximum scheduled effect event count".to_owned());
    }
    if events.len() + expected_events > MAX_QUEUE {
        issues.push("branch event queue exceeds the bounded 10000-event limit".to_owned());
    }
    if program.simulation_start_minute >= duration {
        issues.push("simulation_start_minute is outside the scenario duration".to_owned());
    }
    Ok(())
}

/// The people's capacity of the temporary spaces, active and pending, by zone.
fn temporary_capacity(world: &World, events: &[Arc<EventRow>]) -> Result<IndexMap<Arc<str>, i64>> {
    let mut temporary: IndexMap<Arc<str>, i64> = IndexMap::new();
    if let Some(V::Map(state)) = value(world, overlay::SPACES_KEY)? {
        if let Some(spaces) = state.get("spaces") {
            let V::Map(spaces) = spaces else { return Err(Error::unsupported("the temporary spaces are not a dict")) };
            for space in spaces.values() {
                let V::Map(space) = space else { return Err(Error::unsupported("a temporary space is not a dict")) };
                if space.get("active").is_some_and(V::is_truthy) {
                    let (Some(V::Str(zone)), Some(V::Int(capacity))) = (space.get("zone_id"), space.get("people_capacity")) else {
                        return Err(Error::unsupported("a temporary space without its zone or capacity"));
                    };
                    *temporary.entry(zone.clone()).or_default() += capacity;
                }
            }
        }
    }
    for event in events {
        if &*event.event_type != "effect_program_temporary_space" {
            continue;
        }
        if let (Some(V::Str(zone)), Some(V::Map(parameters))) = (payload(event, "zone_id"), payload(event, "parameters")) {
            if let Some(capacity) = space_capacity(parameters) {
                *temporary.entry(zone.clone()).or_default() += capacity;
            }
        }
    }
    Ok(temporary)
}

/// The facility modifiers' intervals by facility: (start, end, units, effect ID).
type Intervals = IndexMap<Arc<str>, Vec<(i64, i64, i64, Arc<str>)>>;

/// What the checks of each effect read.
#[derive(Clone, Copy)]
struct Checks<'a> {
    world: &'a World,
    rows: &'a Entities,
    configs: &'a Map,
    minute: i64,
    duration: i64,
    schema_version: i64,
    zone_known: &'a dyn Fn(&str) -> bool,
}

/// What the effects add, which the checks after them read: the facility modifiers' intervals by
/// facility, the staff, facilities and evacuees added, and the toilets to place.
#[derive(Default)]
struct Added {
    facility_intervals: Intervals,
    added_staff: i64,
    added_facilities: i64,
    added_evacuees: i64,
    toilet_additions: Vec<(String, Arc<str>, i64)>,
}

/// The checks of one effect, in their order; what it adds goes to `added`.
fn check_effect(cx: Checks<'_>, effect: &Effect, added: &mut Added, issues: &mut Vec<String>) -> Result<()> {
    let Checks { rows, duration, zone_known, .. } = cx;
    let label = format!("effect {}", py_repr(&effect.effect_id));
    let Some(primitive) = primitive(&effect.op) else {
        issues.push(format!("{label} uses unsupported operator {}", py_repr(&effect.op)));
        return Ok(());
    };
    let kind = &*effect.target_selector.kind;
    if !primitive.target_kinds.contains(&kind) {
        issues.push(format!("{label} target kind must be one of {}", list_repr(primitive.target_kinds)));
    }
    if !primitive.units.contains(&&*effect.units) {
        issues.push(format!("{label} has unsupported units {}", py_repr(&effect.units)));
    }
    let dependencies: BTreeSet<&str> = effect.policy_dependencies.iter().map(|id| &**id).collect();
    if dependencies != primitive.policies.iter().copied().collect() {
        issues.push(format!("{label} policy_dependencies must be exactly {}", list_repr(primitive.policies)));
    }
    let parameters = parameters(effect)?;
    let target = match (&effect.target_selector.target_id, effect.target_selector.candidate_ids.is_empty()) {
        (Some(target), true) => target.clone(),
        _ => {
            issues.push(format!("{label} requires one explicit target ID"));
            return Ok(());
        }
    };
    if let Some(guard) = &effect.guard {
        guard_issues(&label, kind, guard, issues);
    }
    window_issues(cx, effect, &primitive, &label, issues)?;
    if matches!(&*effect.op, "add_staff" | "add_toilet" | "add_evacuees" | "depart_evacuees") && !zone_known(&target) {
        issues.push(format!("{label} references unknown zone {}", py_repr(&target)));
    }
    if &*effect.op == "add_evacuees" {
        admission_issues(cx, &parameters, &target, &label, issues)?;
    }
    match &*effect.op {
        "add_staff" => added.added_staff += parameters.count,
        "add_toilet" => {
            added.added_facilities += parameters.count;
            added.toilet_additions.push((label.clone(), target.clone(), parameters.count));
        }
        "add_evacuees" => added.added_evacuees += parameters.count,
        _ => {}
    }
    if &*effect.op == "facility_outage" {
        if !rows.facilities.contains_key(&target) {
            issues.push(format!("{label} references unknown facility {}", py_repr(&target)));
            return Ok(());
        }
        added.facility_intervals.entry(target.clone()).or_default().push((
            effect.start_minute,
            effect.end_minute.unwrap_or(duration),
            parameters.units,
            effect.effect_id.clone(),
        ));
    } else if &*effect.op == "water_outage" {
        if effect.end_minute.is_none() {
            issues.push(format!("{label} requires end_minute so water service can be restored"));
        }
        if kind == "facility" {
            if !rows.facilities.get(&target).is_some_and(|facility| &*facility.kind == "water_station") {
                issues.push(format!("{label} facility must be an existing water_station"));
            }
        } else if !zone_known(&target) || !rows.facilities.iter().any(|(_, facility)| &*facility.kind == "water_station" && facility.zone_id == target) {
            issues.push(format!("{label} zone must contain an existing water_station"));
        }
    }
    Ok(())
}

/// The fields an effect's guard reads, against its target kind.
fn guard_issues(label: &str, kind: &str, guard: &Guard, issues: &mut Vec<String>) {
    let mut fields = BTreeSet::new();
    guard.fields(&mut fields);
    for field in fields {
        let shown = py_repr(&field);
        if field.starts_with("target.") && kind != "facility" {
            issues.push(format!("{label} guard field {shown} requires a facility target"));
        } else if &*field == "zone.occupancy" && !matches!(kind, "facility" | "zone") {
            issues.push(format!("{label} guard field {shown} needs a zone-scoped target"));
        } else if &*field == "resource.available_quantity" {
            issues.push(format!("{label} guard field {shown} requires a resource delivery target"));
        } else if &*field == "staff.available" {
            issues.push(format!("{label} guard field {shown} requires a staff availability target"));
        }
    }
}

/// An effect's time window, and the policies its operator needs.
fn window_issues(cx: Checks<'_>, effect: &Effect, primitive: &Primitive, label: &str, issues: &mut Vec<String>) -> Result<()> {
    let Checks { world, minute, duration, schema_version, .. } = cx;
    if effect.start_minute < minute || effect.start_minute >= duration {
        issues.push(format!("{label} start_minute must be within the remaining scenario"));
    }
    match effect.end_minute {
        Some(end) => {
            if end <= effect.start_minute || end > duration {
                issues.push(format!("{label} end_minute is outside its valid time window"));
            }
            if end - effect.start_minute > MAX_EFFECT_DURATION {
                issues.push(format!("{label} duration exceeds the 366-day limit"));
            }
        }
        None if duration - effect.start_minute > MAX_EFFECT_DURATION => issues.push(format!("{label} duration exceeds the 366-day limit")),
        None => {}
    }
    if !primitive.timed && effect.end_minute.is_some() {
        issues.push(format!("{label} is a one-shot operation and cannot have end_minute"));
    }
    if primitive.policies.contains(&"operational_events_v1") && !has(world, "operational_events_v1")? {
        issues.push(format!("{label} requires operational_events_v1 state"));
    }
    if primitive.policies.contains(&"resource_service_v1") && (schema_version != 6 || !has(world, "resource_service_v1")?) {
        issues.push(format!("{label} requires the WorldState v6 resource-service policy"));
    }
    if primitive.policies.contains(&"population_dynamics_v1") && !has(world, "population_dynamics_v1")? {
        issues.push(format!("{label} requires population_dynamics_v1 state"));
    }
    Ok(())
}

/// The alternate admission zones and the approach route of an `add_evacuees` effect.
fn admission_issues(cx: Checks<'_>, parameters: &Parameters, target: &Arc<str>, label: &str, issues: &mut Vec<String>) -> Result<()> {
    let Checks { configs, zone_known, .. } = cx;
    let admission = &parameters.admission_zone_ids;
    if admission.contains(target) || admission.iter().any(|zone| !zone_known(zone)) {
        issues.push(format!("{label} alternate admission zones must be known and exclude its preferred zone"));
    }
    let overlay_schema = match configs.get("geometry_overlay") {
        Some(V::Map(config)) => match config.get("schema_version") {
            Some(V::Int(version)) => Some(*version),
            _ => None,
        },
        _ => None,
    };
    if !admission.is_empty() && !matches!(overlay_schema, Some(3 | 4)) {
        issues.push(format!("{label} alternate physical room admission requires geometry_overlay schema_version=3 or 4"));
    }
    if let Some(route_id) = &parameters.approach_route_id {
        if !admission.is_empty() {
            issues.push(format!("{label} cannot combine one approach route with alternate admission zones"));
        }
        if let Some(problem) = approach_problem(configs, route_id, target)? {
            issues.push(format!("{label} arrival approach is invalid: {problem}"));
        }
    }
    Ok(())
}

/// The facility modifiers of the program, by facility: their stacking against the facility's
/// occupancy, and the pending facility events they overlap.
fn interval_issues(rows: &Entities, events: &[Arc<EventRow>], mut facility_intervals: Intervals, schema_version: i64, issues: &mut Vec<String>) -> Result<()> {
    for (facility_id, intervals) in &mut facility_intervals {
        intervals.sort_by(|first, second| first.0.cmp(&second.0).then_with(|| first.3.as_bytes().cmp(second.3.as_bytes())));
        let facility = rows.facilities.get(facility_id).ok_or_else(|| Error::Runtime("a facility left the table".to_owned()))?;
        let mut active: Vec<(i64, i64)> = Vec::new();
        for (start, end, units, effect_id) in intervals.iter() {
            active.retain(|(entry_end, _)| entry_end > start);
            let current = active.iter().map(|(_, units)| *units).fold(facility.working_units, i64::min);
            let target = current - units;
            if target < occupancy(facility, schema_version) {
                issues.push(format!("facility modifier {} would undercut active occupancy", py_repr(effect_id)));
            }
            if target > current {
                issues.push(format!("facility modifier {} conflicts with the restrictive stacking rule", py_repr(effect_id)));
            }
            active.push((*end, target));
        }
        for event in events {
            if matches!(&*event.event_type, "facility_outage" | "facility_recovery" | "effect_program_apply" | "effect_program_expire")
                && matches!(payload(event, "facility_id"), Some(V::Str(id)) if id == facility_id)
                && intervals.iter().any(|(start, end, _, _)| *start <= event.due_minute && event.due_minute < *end)
            {
                issues.push(format!("pending {} conflicts with a new EffectProgram facility modifier", event.event_type));
            }
        }
    }
    Ok(())
}

/// The population bound: the known profiles, the pending arrivals and the evacuees the program
/// adds.
fn population_issues(world: &World, events: &[Arc<EventRow>], added_evacuees: i64, issues: &mut Vec<String>) -> Result<()> {
    if let Some(V::Map(population)) = value(world, "population_dynamics_v1")? {
        let mut known: BTreeSet<Arc<str>> = BTreeSet::new();
        match population.get("initial_agent_ids") {
            None => {}
            Some(V::List(items)) => {
                for item in items {
                    let V::Str(id) = item else { return Err(Error::unsupported("an initial agent ID that is not a string")) };
                    known.insert(id.clone());
                }
            }
            Some(_) => return Err(Error::unsupported("the initial agent IDs are not a list")),
        }
        match population.get("profile_traits") {
            None => {}
            Some(V::Map(traits)) => known.extend(traits.keys().cloned()),
            Some(_) => return Err(Error::unsupported("the profile traits are not a dict")),
        }
        let arriving = pending_count(events, "effect_program_population", "add_evacuees")?;
        if known.len() as i64 + arriving + added_evacuees > 10_000 {
            issues.push("program exceeds the 10000-profile population runtime bound".to_owned());
        }
    }
    Ok(())
}

/// `_TemporarySpaceParameters.model_validate(parameters).people_capacity`, None where it refuses.
fn space_capacity(parameters: &Map) -> Option<i64> {
    if parameters.keys().any(|key| !matches!(&**key, "people_capacity" | "sleeping_capacity")) {
        return None;
    }
    let people = match parameters.get("people_capacity") {
        Some(V::Int(people)) if (1..=1000).contains(people) => *people,
        _ => return None,
    };
    let sleeping = match parameters.get("sleeping_capacity") {
        None => 0,
        Some(V::Int(sleeping)) if (0..=1000).contains(sleeping) => *sleeping,
        _ => return None,
    };
    (sleeping <= people).then_some(people)
}

/// The arrival approach's problem, as `ValueError`'s text, or None: the route must be one of the
/// approved geometry's, into the selected zone. The geometry a world holds is one its Scenario
/// validated.
fn approach_problem(configs: &Map, route_id: &str, zone_id: &str) -> Result<Option<String>> {
    let Some(V::Map(geometry)) = configs.get("facility_geometry") else {
        return Ok(Some("arrival approach requires approved physical Geometry".to_owned()));
    };
    let routes = match geometry.get("arrival_approach_routes") {
        None => &[][..],
        Some(V::List(routes)) => &routes[..],
        Some(_) => return Err(Error::unsupported("the approach routes are not a list")),
    };
    for route in routes {
        let V::Map(route) = route else { return Err(Error::unsupported("an approach route is not a dict")) };
        if matches!(route.get("route_id"), Some(V::Str(id)) if &**id == route_id) {
            let to_zone = matches!(route.get("destination_zone_id"), Some(V::Str(id)) if &**id == zone_id);
            return Ok((!to_zone).then(|| "arrival approach destination must match the selected zone".to_owned()));
        }
    }
    Ok(Some(format!("unknown arrival approach route {}", py_repr(route_id))))
}

/// The limit of active environment modifiers in each zone, over the modifiers active now, those
/// pending, and the program's (none of the six operators adds one).
fn environment_limits(
    world: &World,
    config: &V,
    events: &[Arc<EventRow>],
    zone_known: &dyn Fn(&str) -> bool,
    minute: i64,
    duration: i64,
    issues: &mut Vec<String>,
) -> Result<()> {
    let limit = match config {
        V::Map(config) => match config.get("max_active_modifiers_per_zone") {
            Some(V::Int(limit)) => *limit,
            _ => return Err(Error::unsupported("the zone environment config has no int limit")),
        },
        _ => return Err(Error::unsupported("the zone environment config is not a dict")),
    };
    let Some(V::Map(state)) = value(world, crate::rules::effects::ENVIRONMENT)? else {
        return Err(Error::unsupported("the zone environment state is missing"));
    };
    let Some(V::Map(modifiers)) = state.get("modifiers") else {
        return Err(Error::unsupported("the zone environment modifiers are not a dict"));
    };
    let mut intervals: IndexMap<Arc<str>, Vec<(i64, i64)>> = IndexMap::new();
    for record in modifiers.values() {
        let V::Map(record) = record else { return Err(Error::unsupported("a modifier is not a dict")) };
        let (Some(V::Str(zone)), Some(V::Int(end))) = (record.get("zone_id"), record.get("end_minute")) else {
            return Err(Error::unsupported("a modifier without its zone or int end"));
        };
        intervals.entry(zone.clone()).or_default().push((minute, *end));
    }
    for event in events {
        if &*event.event_type != "effect_program_environment" {
            continue;
        }
        let (Some(V::Str(modifier)), Some(V::Str(zone)), Some(V::Int(end))) =
            (payload(event, "modifier_id"), payload(event, "zone_id"), payload(event, "end_minute"))
        else {
            continue;
        };
        if !modifiers.contains_key(modifier) && zone_known(zone) && event.due_minute < *end && *end < duration {
            intervals.entry(zone.clone()).or_default().push((event.due_minute, *end));
        }
    }
    for (zone, intervals) in &intervals {
        let boundaries: BTreeSet<i64> = intervals.iter().flat_map(|(start, end)| [*start, *end]).collect();
        let boundaries: Vec<i64> = boundaries.into_iter().collect();
        let over = boundaries[..boundaries.len().saturating_sub(1)]
            .iter()
            .any(|at| intervals.iter().filter(|(start, end)| start <= at && at < end).count() as i64 > limit);
        if over {
            issues.push(format!("zone {} exceeds the active environment modifier limit", py_repr(zone)));
        }
    }
    Ok(())
}

/// The placement's issues: each added toilet gets a free point on the floor plan of a copy of
/// the world, the toilets before it placed there.
fn toilet_placement(world: &World, additions: &[(String, Arc<str>, i64)], placement: &mut Placement<'_>) -> Result<Vec<String>> {
    if !placement.enabled {
        return Ok(additions
            .iter()
            .map(|(label, _, _)| {
                format!("{label} cannot place added toilets on this floor plan; it needs geometry_overlay schema_version=4 (create the Scenario again)")
            })
            .collect());
    }
    let candidate = world.fork()?;
    let minute = candidate.minute_index()?;
    for (label, zone_id, count) in additions {
        if !candidate.rows()?.zones.contains_key(zone_id) {
            continue;
        }
        for index in 0..*count {
            let facility_id: Arc<str> = Arc::from(format!("placement-check-{}-{index}", candidate.rows()?.facilities.len()));
            candidate.rows_mut()?.facilities.insert(
                facility_id.clone(),
                FacilityRow {
                    id: facility_id.clone(),
                    kind: Arc::from("toilet"),
                    zone_id: zone_id.clone(),
                    capacity: 1,
                    working_units: 1,
                    active_agent_ids: Vec::new(),
                    queue_agent_ids: Vec::new(),
                    resource_active_agent_ids: Vec::new(),
                    resource_queue_agent_ids: Vec::new(),
                },
            );
            let plan = (placement.plan)(&candidate)?;
            let Some(point) = overlay::installed_point(&candidate, &Held::Held, Some(&plan), &facility_id)? else {
                return Ok(vec![format!("{label} has no free floor-plan point for toilet {} in zone {}", index + 1, py_repr(zone_id))]);
            };
            let mut state = candidate.row_mut::<ValueRow>(overlay::OVERLAY_KEY)?;
            let V::Map(state) = &mut state.0 else { return Err(Error::unsupported("the overlay is not a dict")) };
            let Some(V::Map(points)) = state.get_mut("facility_points") else {
                return Err(Error::unsupported("the overlay's facility points are not a dict"));
            };
            points.insert(
                facility_id.clone(),
                object([
                    ("point_m", object([("x", V::Float(point.x)), ("y", V::Float(point.y))])),
                    ("event_id", V::Str(Arc::from("placement-check"))),
                    ("source_ids", V::List(Vec::new())),
                    ("minute", V::Int(minute)),
                ]),
            );
        }
    }
    Ok(Vec::new())
}

// ---- Scheduling -------------------------------------------------------------------------------------

/// What scheduling a program gives back, with the branch's minute: the branch's `command_log`
/// entry of the program but its `"command"`.
#[derive(Clone, Debug, PartialEq)]
pub struct Receipt {
    pub program_id: Arc<str>,
    pub program_hash: String,
    pub event_ids: Vec<Arc<str>>,
    pub minute: i64,
}

impl Receipt {
    /// `{"command": "effect_program", "program_id", "program_hash", "event_ids", "minute"}`, the
    /// entry a fork appends to the branch's `command_log`.
    pub fn command(&self) -> V {
        object([
            ("command", V::Str(Arc::from("effect_program"))),
            ("program_id", string(&self.program_id)),
            ("program_hash", V::Str(Arc::from(self.program_hash.as_str()))),
            ("event_ids", ids(&self.event_ids)),
            ("minute", V::Int(self.minute)),
        ])
    }
}

fn sha256_text(text: &str) -> String {
    crate::checkpoint::sha256_hex(text.as_bytes())
}

/// A modifier's ID: `modifier-` and the first 32 hex digits of `sha256(f"{program}:{effect}")`.
fn modifier_id(program_id: &str, effect_id: &str) -> Arc<str> {
    Arc::from(format!("modifier-{}", &sha256_text(&format!("{program_id}:{effect_id}"))[..32]))
}

/// The parameters' dump with the model's defaults (`parameters_model.model_validate(...)
/// .model_dump(mode="json")`), for the operators whose payload holds them.
fn parameters_dump(effect: &Effect, parameters: &Parameters) -> Map {
    let V::Map(given) = &effect.parameters else { return Map::new() };
    let mut dump = Map::new();
    let mut put = |name: &str, value: V| {
        dump.insert(Arc::from(name), value);
    };
    match &*effect.op {
        "add_staff" | "add_toilet" => put("count", V::Int(parameters.count)),
        "add_evacuees" => {
            put("count", V::Int(parameters.count));
            put("household_size", given.get("household_size").cloned().unwrap_or(V::Int(1)));
            put("mobility", given.get("mobility").cloned().unwrap_or_else(|| V::Str(Arc::from("independent"))));
            put("language", given.get("language").cloned().unwrap_or_else(|| V::Str(Arc::from("ja"))));
            put("admission_zone_ids", ids(&parameters.admission_zone_ids));
            put("approach_route_id", optional(&parameters.approach_route_id));
            put("person_context_overrides", V::List(Vec::new()));
        }
        "depart_evacuees" => {
            put("count", V::Int(parameters.count));
            put("include_initial", given.get("include_initial").cloned().unwrap_or(V::Bool(false)));
        }
        _ => {}
    }
    dump
}

/// The working units a facility modifier sets, the program's earlier
/// modifiers of the facility in force.
fn projected_units(world: &World, program: &EffectProgram, target: &Effect) -> Result<i64> {
    let facility_id = target.target_selector.target_id.as_deref().unwrap_or_default();
    let current = world.rows()?.facilities.get(facility_id).ok_or_else(|| Error::Key(facility_id.to_owned()))?.working_units;
    let duration = world.duration_minutes()?;
    let mut effects: Vec<&Effect> = program.effects.iter().collect();
    effects.sort_by(|first, second| first.start_minute.cmp(&second.start_minute).then_with(|| first.effect_id.as_bytes().cmp(second.effect_id.as_bytes())));
    let mut active: Vec<(i64, i64)> = Vec::new();
    for effect in effects {
        if &*effect.op != "facility_outage" || effect.target_selector.target_id.as_deref() != Some(facility_id) {
            continue;
        }
        active.retain(|(end, _)| *end > effect.start_minute);
        let available = active.iter().map(|(_, units)| *units).fold(current, i64::min);
        let requested = available - parameters(effect)?.units;
        if effect.effect_id == target.effect_id {
            return Ok(requested);
        }
        active.push((effect.end_minute.unwrap_or(duration), requested));
    }
    Err(Error::Value(format!("facility modifier effect {} was not found", py_repr(&target.effect_id))))
}

/// A program scheduled on the world, as a fork does to its child: the
/// program checked (`validate`; its issues are the error, joined by `; `), then its
/// record and its start and expiry events written to the world, all or none. The world's own check
/// after the writes (`candidate.validate()`, E6 of #250) is not run.
pub fn schedule(world: &mut World, program: &EffectProgram, placement: &mut Placement<'_>) -> Result<Receipt> {
    let issues = validate(world, program, placement)?;
    if !issues.is_empty() {
        return Err(Error::Value(format!("EffectProgram could not be scheduled: {}", issues.join("; "))));
    }
    let snapshot = world.snapshot();
    match schedule_on(world, program) {
        Ok(receipt) => Ok(receipt),
        Err(error) => {
            world.restore(snapshot);
            Err(match error {
                Error::Value(text) => Error::Value(format!("EffectProgram could not be scheduled: {text}")),
                other => other,
            })
        }
    }
}

/// The writes of a program's schedule, for the six operators.
fn schedule_on(world: &World, program: &EffectProgram) -> Result<Receipt> {
    use crate::events::EventKind;
    use crate::rules::kit::{new_event, Events, NewEvent};

    let minute = world.minute_index()?;
    let program_hash = crate::checkpoint::sha256_hex(&program.dump_json()?);
    let source_text_hash = sha256_text(&program.source_text);
    let provenance = program.compiler_provenance.dump();
    start_record(world, program, &program_hash, &source_text_hash, &provenance)?;
    let events = Events::of(world)?;
    let mut event_ids: Vec<Arc<str>> = Vec::new();
    let mut make = |effect: &Effect, due: i64, event_type: &str, payload: Map, phase: &str, priority: i64, causation: Option<Arc<str>>| -> Result<Arc<str>> {
        let sequence = events.allocate_sequence()?;
        let event_id: Arc<str> = Arc::from(format!("effect-event-{sequence}"));
        let mut marker = Map::new();
        let mut put = |name: &str, value: V| {
            marker.insert(Arc::from(name), value);
        };
        put("program_id", string(&program.program_id));
        put("effect_id", string(&effect.effect_id));
        put("op", string(&effect.op));
        put("source_text_hash", V::Str(Arc::from(source_text_hash.as_str())));
        put("compiler_provenance", provenance.clone());
        put("assumption_ids", ids(&effect.assumption_ids));
        put("policy_dependencies", ids(&effect.policy_dependencies));
        put("units", string(&effect.units));
        put("phase", V::Str(Arc::from(phase)));
        if phase == "start" {
            if let Some(guard) = &effect.guard {
                put("guard", guard.dump());
            }
        }
        let row = new_event(NewEvent {
            event_id: event_id.clone(),
            kind: EventKind::Intervention,
            due_minute: due,
            priority,
            sequence,
            event_type: Arc::from(event_type),
            payload,
            source_id: Some(effect.source_id.clone()),
            causation_id: causation.or_else(|| effect.causation_id.clone()),
            provenance: [(Arc::from(EFFECT_PROGRAM), V::Map(marker))].into_iter().collect(),
        })?;
        events.schedule(row).map_err(Error::from)?;
        world
            .row_mut::<EffectProgramRow>(EFFECT_PROGRAM)?
            .programs
            .get_mut(&*program.program_id)
            .ok_or_else(|| Error::Runtime("the program left its state".to_owned()))?
            .scheduled_event_ids
            .push(event_id.clone());
        event_ids.push(event_id.clone());
        Ok(event_id)
    };
    let mut effects: Vec<&Effect> = program.effects.iter().collect();
    effects.sort_by(|first, second| first.start_minute.cmp(&second.start_minute).then_with(|| first.effect_id.as_bytes().cmp(second.effect_id.as_bytes())));
    for effect in effects {
        let parameters = parameters(effect)?;
        let target = effect.target_selector.target_id.clone().ok_or_else(|| Error::Runtime("an effect without its target".to_owned()))?;
        let head = |extra: &[(&str, V)]| -> Map {
            let mut payload = Map::new();
            payload.insert(Arc::from("program_id"), string(&program.program_id));
            payload.insert(Arc::from("effect_id"), string(&effect.effect_id));
            for (name, value) in extra {
                payload.insert(Arc::from(*name), value.clone());
            }
            payload
        };
        match &*effect.op {
            "facility_outage" => schedule_facility_outage(world, program, effect, &target, &mut make, &head)?,
            "water_outage" => schedule_water_outage(world, program, effect, &target, &mut make)?,
            "add_staff" | "add_toilet" => {
                let mut payload = head(&[
                    ("modifier_id", V::Str(modifier_id(&program.program_id, &effect.effect_id))),
                    ("op", string(&effect.op)),
                    ("zone_id", V::Str(target.clone())),
                ]);
                payload.extend(parameters_dump(effect, &parameters));
                make(effect, effect.start_minute, "effect_program_structure", payload, "start", 0, None)?;
            }
            "add_evacuees" | "depart_evacuees" => {
                let mut payload = head(&[("op", string(&effect.op)), ("zone_id", V::Str(target.clone()))]);
                payload.extend(parameters_dump(effect, &parameters));
                make(effect, effect.start_minute, "effect_program_population", payload, "start", 0, None)?;
            }
            _ => return Err(Error::unsupported("an operator the offline reading of a text never makes")),
        }
    }
    Ok(Receipt { program_id: program.program_id.clone(), program_hash, event_ids, minute })
}

/// The writer of one scheduled event (`make` of `schedule_on`): the effect, the due minute, the
/// event type, the payload, the phase, the priority and the causation; the event's ID.
type Make<'a> = dyn FnMut(&Effect, i64, &str, Map, &str, i64, Option<Arc<str>>) -> Result<Arc<str>> + 'a;

/// The payload head of an effect's event (`head` of `schedule_on`): the program's and the
/// effect's IDs, then the extra fields.
type Head<'a> = dyn Fn(&[(&str, V)]) -> Map + 'a;

/// The program's record in `effect_program_v1`, the namespace started where the world has none.
fn start_record(world: &World, program: &EffectProgram, program_hash: &str, source_text_hash: &str, provenance: &V) -> Result<()> {
    use crate::state::effect_program::Program;

    if !has(world, EFFECT_PROGRAM)? {
        world.start_namespace(
            EFFECT_PROGRAM,
            Box::new(EffectProgramRow { schema_version: 1, programs: Default::default(), active_facility_modifiers: Default::default() }),
        )?;
    }
    {
        let mut state = world.row_mut::<EffectProgramRow>(EFFECT_PROGRAM)?;
        state.programs.insert(
            program.program_id.clone(),
            Program {
                program_hash: Arc::from(program_hash),
                source_text_hash: Arc::from(source_text_hash),
                narrative: program.narrative.clone(),
                compiler_provenance: provenance.clone(),
                assumptions: V::List(program.assumptions.iter().map(Assumption::dump).collect()),
                support_mode: program.support_mode.clone(),
                effect_ids: program.effects.iter().map(|effect| effect.effect_id.clone()).collect(),
                operators: program.effects.iter().map(|effect| (effect.effect_id.clone(), effect.op.clone())).collect(),
                scheduled_event_ids: Vec::new(),
            },
        );
    }
    Ok(())
}

/// A `facility_outage` effect: its start event, and its expiry event where it ends.
fn schedule_facility_outage(world: &World, program: &EffectProgram, effect: &Effect, target: &Arc<str>, make: &mut Make<'_>, head: &Head<'_>) -> Result<()> {
    let units = projected_units(world, program, effect)?;
    let modifier = V::Str(modifier_id(&program.program_id, &effect.effect_id));
    let start = make(
        effect,
        effect.start_minute,
        "effect_program_apply",
        head(&[("modifier_id", modifier.clone()), ("facility_id", V::Str(target.clone())), ("working_units", V::Int(units))]),
        "start",
        0,
        None,
    )?;
    if let Some(end) = effect.end_minute {
        make(effect, end, "effect_program_expire", head(&[("modifier_id", modifier), ("facility_id", V::Str(target.clone()))]), "expiry", 20, Some(start))?;
    }
    Ok(())
}

/// A `water_outage` effect: its start event, its restoration event where it ends within the run,
/// and its outage record in `operational_events_v1`.
fn schedule_water_outage(world: &World, program: &EffectProgram, effect: &Effect, target: &Arc<str>, make: &mut Make<'_>) -> Result<()> {
    let outage_id: Arc<str> = Arc::from(format!("outage-{}", &sha256_text(&format!("{}:{}:water", program.program_id, effect.effect_id))[..32]));
    let zone = &*effect.target_selector.kind == "zone";
    let mut payload = Map::new();
    payload.insert(Arc::from("outage_id"), V::Str(outage_id.clone()));
    payload.insert(Arc::from(if zone { "zone_id" } else { "facility_id" }), V::Str(target.clone()));
    let start = make(effect, effect.start_minute, "water_outage", payload, "start", 0, None)?;
    let mut restore = V::Null;
    if let Some(end) = effect.end_minute.filter(|end| *end < world.duration_minutes().unwrap_or(i64::MAX)) {
        let mut payload = Map::new();
        payload.insert(Arc::from("outage_id"), V::Str(outage_id.clone()));
        restore = V::Str(make(effect, end, "water_restored", payload, "expiry", 10, Some(start.clone()))?);
    }
    let mut state = world.row_mut::<ValueRow>("operational_events_v1")?;
    let V::Map(state) = &mut state.0 else { return Err(Error::unsupported("operational_events_v1 is not a dict")) };
    let Some(V::Map(outages)) = state.get_mut("water_outages") else {
        return Err(Error::unsupported("the water outages are not a dict"));
    };
    if outages.contains_key(&outage_id) {
        return Err(Error::Value(format!("water outage ID collision for {}", py_repr(&outage_id))));
    }
    outages.insert(
        outage_id,
        object([
            ("target_kind", V::Str(Arc::from(if zone { "zone" } else { "facility" }))),
            ("target_id", V::Str(target.clone())),
            ("outage_event_id", V::Str(start)),
            ("restore_event_id", restore),
            ("active", V::Bool(false)),
        ]),
    );
    Ok(())
}
