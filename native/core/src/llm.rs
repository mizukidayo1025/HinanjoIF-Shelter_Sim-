//! The port to an LLM (#428): what the core asks (`Task`), what it is told (`Reply`), and the one
//! method a host gives it (`LlmLink::complete`). The core's rules that ask an LLM during a tick
//! (`rules::llm_runtime`: a support request's priority and the event-triggered social policy) build
//! the prompt, check the answer and write the world themselves; a link only passes the task on and
//! the answer back. The PyO3 crate's link is Python's `LLMRouter` (its budget, response tape, usage
//! ledger and recorded replay), the browser's would be the relay of #287 (`/llm/v1/tasks/...`).
//!
//! A task and a reply cross the port as JSON (`Task::to_json`, `Reply::from_json`), so that every
//! host links the same way:
//! - task: `{"task", "stage", "prompt_version", "schema_version", "input_text", "state_hash",
//!   "output_schema", "fallback_text", "max_output_tokens"}`. `input_text` is the canonical JSON of
//!   the prompt's data (keys sorted, no spaces, `json.dumps(ensure_ascii=False)`'s strings), and
//!   `task` names what is asked (`SUPPORT_PRIORITY`, `SOCIAL_SUPPORT_REQUEST`, `SOCIAL_INTERACTION`).
//! - reply: `{"text", "outcome", "fallback_used", "cache_hit", "model_id", "estimated_cost_usd",
//!   "api_request_count", "input_tokens", "output_tokens"}`, the fields of Python's `LLMCallRecord`
//!   the rules record. `text` is the answer, or the task's `fallback_text` where the link fell back.
//!
//! The answer is untrusted input: the core reads it against the task's schema (`check_answer`), and
//! an answer that does not fit is the fallback (outcome `invalid_output`), whatever the link said.
//!
//! Each call is one entry of the Run's LLM tape (`tape_entry`), which a stored Run keeps in its
//! `llm_tape` pieces (`run_store`, #368): the call's identity (task, stage, versions, the input's and
//! the state's hashes) and its reply, with no prompt and no free text of the model (a priority's
//! `rationale` is kept empty; the rules do not read it). `Replay` answers from a tape in its order,
//! as Python's `RecordedRouter` does, so that the ticks between two saved points are computed again
//! without asking anyone.

use std::sync::Arc;

use indexmap::IndexMap;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::checkpoint::hex;
use crate::error::{Error, Result};
use crate::json::write_json_string;
use crate::value::V;

/// The stage of every task a tick asks: Python's `LLMStage.RUNTIME` (Haiku).
pub const STAGE_RUNTIME: &str = "runtime";
/// A support request's queue priority (`_prioritize_support_request`).
pub const SUPPORT_PRIORITY: &str = "support_priority";
/// The social policy's response to a pending support request (`consider_support_request`).
pub const SOCIAL_SUPPORT_REQUEST: &str = "social_support_request";
/// The social policy's typed interaction: a complaint's response, a cooperation, an information
/// reaction (`_consider_typed_interaction`).
pub const SOCIAL_INTERACTION: &str = "social_interaction";

/// What the core asks of an LLM.
#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    pub task: &'static str,
    pub stage: &'static str,
    pub prompt_version: String,
    pub schema_version: i64,
    pub input_text: String,
    pub state_hash: String,
    /// A JSON schema of an object (the subset `check_answer` reads).
    pub output_schema: V,
    pub fallback_text: String,
    pub max_output_tokens: i64,
}

impl Task {
    /// The sha256 of the input, as Python's `LLMTask.input_hash` writes it: of the input's JSON
    /// string (`json.dumps(input_text, ensure_ascii=False)`), in hex.
    pub fn input_hash(&self) -> String {
        let mut quoted = Vec::with_capacity(self.input_text.len() + 2);
        write_json_string(&mut quoted, &self.input_text);
        hex(&Sha256::digest(&quoted))
    }

    /// The task as it crosses the port.
    pub fn to_json(&self) -> Result<Vec<u8>> {
        let mut map = IndexMap::with_capacity(9);
        map.insert(key("task"), text(self.task));
        map.insert(key("stage"), text(self.stage));
        map.insert(key("prompt_version"), text(&self.prompt_version));
        map.insert(key("schema_version"), V::Int(self.schema_version));
        map.insert(key("input_text"), text(&self.input_text));
        map.insert(key("state_hash"), text(&self.state_hash));
        map.insert(key("output_schema"), self.output_schema.clone());
        map.insert(key("fallback_text"), text(&self.fallback_text));
        map.insert(key("max_output_tokens"), V::Int(self.max_output_tokens));
        V::Map(map).to_json()
    }

    /// What a link answers when it falls back: the task's fallback text, nothing spent.
    pub fn fallback(&self, outcome: &str) -> Reply {
        Reply {
            text: self.fallback_text.clone(),
            outcome: outcome.to_owned(),
            fallback_used: true,
            cache_hit: false,
            model_id: None,
            estimated_cost_usd: 0.0,
            api_request_count: 0,
            input_tokens: 0,
            output_tokens: 0,
        }
    }
}

/// What a link answers: the text and the call's record.
#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    pub text: String,
    /// `recorded`, `cache_hit`, `offline`, or a failure (`replay_miss`, `budget_exceeded`, ...).
    pub outcome: String,
    pub fallback_used: bool,
    pub cache_hit: bool,
    pub model_id: Option<String>,
    pub estimated_cost_usd: f64,
    pub api_request_count: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

impl Reply {
    /// The reply of its JSON: an error for anything but the fields above, of their types.
    pub fn from_json(raw: &[u8]) -> Result<Self> {
        let value: Value = serde_json::from_slice(raw).map_err(|_| bad_reply("not JSON"))?;
        let object = value.as_object().ok_or_else(|| bad_reply("not an object"))?;
        const FIELDS: [&str; 9] =
            ["text", "outcome", "fallback_used", "cache_hit", "model_id", "estimated_cost_usd", "api_request_count", "input_tokens", "output_tokens"];
        if let Some(name) = object.keys().find(|name| !FIELDS.contains(&name.as_str())) {
            return Err(bad_reply(&format!("an unknown field {name}")));
        }
        let string = |name: &str| object.get(name).and_then(Value::as_str).map(str::to_owned).ok_or_else(|| bad_reply(&format!("{name} is not a string")));
        let flag = |name: &str| object.get(name).and_then(Value::as_bool).ok_or_else(|| bad_reply(&format!("{name} is not a bool")));
        let count =
            |name: &str| object.get(name).and_then(Value::as_i64).filter(|count| *count >= 0).ok_or_else(|| bad_reply(&format!("{name} is not a count")));
        let cost = object.get("estimated_cost_usd").and_then(Value::as_f64).filter(|cost| cost.is_finite() && *cost >= 0.0);
        let model_id = match object.get("model_id") {
            None | Some(Value::Null) => None,
            Some(Value::String(model)) => Some(model.clone()),
            Some(_) => return Err(bad_reply("model_id is not a string")),
        };
        Ok(Self {
            text: string("text")?,
            outcome: string("outcome")?,
            fallback_used: flag("fallback_used")?,
            cache_hit: flag("cache_hit")?,
            model_id,
            estimated_cost_usd: cost.ok_or_else(|| bad_reply("estimated_cost_usd is not a cost"))?,
            api_request_count: count("api_request_count")?,
            input_tokens: count("input_tokens")?,
            output_tokens: count("output_tokens")?,
        })
    }

    /// The reply as it crosses the port.
    pub fn to_json(&self) -> Result<Vec<u8>> {
        let mut map = IndexMap::with_capacity(9);
        self.put(&mut map);
        V::Map(map).to_json()
    }

    fn put(&self, map: &mut IndexMap<Arc<str>, V>) {
        map.insert(key("text"), text(&self.text));
        map.insert(key("outcome"), text(&self.outcome));
        map.insert(key("fallback_used"), V::Bool(self.fallback_used));
        map.insert(key("cache_hit"), V::Bool(self.cache_hit));
        map.insert(key("model_id"), self.model_id.as_deref().map_or(V::Null, text));
        map.insert(key("estimated_cost_usd"), V::Float(self.estimated_cost_usd));
        map.insert(key("api_request_count"), V::Int(self.api_request_count));
        map.insert(key("input_tokens"), V::Int(self.input_tokens));
        map.insert(key("output_tokens"), V::Int(self.output_tokens));
    }
}

fn bad_reply(what: &str) -> Error {
    Error::Value(format!("an LLM link's reply: {what}"))
}

/// The one method a host gives the core to ask an LLM.
pub trait LlmLink {
    /// Whether every task is answered with its fallback (no API key: Python's `LLMMode.OFFLINE`).
    /// The rules then ask nothing, and record why (`offline_rule_mode`), as Python's do.
    fn offline(&self) -> bool {
        false
    }

    /// The answer to `task`. An error stops the tick (a replay that does not match, a budget
    /// policy that stops instead of falling back).
    fn complete(&mut self, task: &Task) -> Result<Reply>;
}

/// The link of a Run without an API key: offline, every task its fallback.
#[derive(Clone, Copy, Debug, Default)]
pub struct Offline;

impl LlmLink for Offline {
    fn offline(&self) -> bool {
        true
    }

    fn complete(&mut self, task: &Task) -> Result<Reply> {
        Ok(task.fallback("offline"))
    }
}

// ---- The limits ------------------------------------------------------------------------------------

/// `SocialRuntimeLimits` of `llm/runtime.py`: how many interactions the social policy may ask for,
/// and when (source ASM-EVENT-TRIGGERED-SOCIAL-LIMITS-V1; the values are the model's, not this
/// port's).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SocialLimits {
    pub stress_threshold: f64,
    pub cooldown_minutes: i64,
    pub per_agent_per_day: i64,
    pub global_per_day: i64,
    pub event_delay_minutes: i64,
    pub complaint_wait_minutes: i64,
}

impl Default for SocialLimits {
    fn default() -> Self {
        Self { stress_threshold: 0.65, cooldown_minutes: 240, per_agent_per_day: 3, global_per_day: 4, event_delay_minutes: 1, complaint_wait_minutes: 30 }
    }
}

impl SocialLimits {
    /// `SocialRuntimeLimits.__post_init__`'s checks, with its texts.
    pub fn check(&self) -> Result<()> {
        if !(0.0..=1.0).contains(&self.stress_threshold) {
            return Err(Error::Value("social stress threshold must be in 0..1".to_owned()));
        }
        if self.cooldown_minutes < 0 || self.event_delay_minutes <= 0 || self.complaint_wait_minutes <= 0 {
            return Err(Error::Value("social cooldown must be non-negative and delays positive".to_owned()));
        }
        if self.per_agent_per_day <= 0 || self.global_per_day <= 0 {
            return Err(Error::Value("social daily request caps must be positive".to_owned()));
        }
        Ok(())
    }
}

/// `SupportPriorityLimits` of `llm/runtime.py` (source ASM-SUPPORT-PRIORITY-LIMITS-V1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PriorityLimits {
    pub stress_threshold: f64,
    pub global_per_day: i64,
}

impl Default for PriorityLimits {
    fn default() -> Self {
        Self { stress_threshold: 0.65, global_per_day: 4 }
    }
}

impl PriorityLimits {
    /// `SupportPriorityLimits.__post_init__`'s checks, with its texts.
    pub fn check(&self) -> Result<()> {
        if !(0.0..=1.0).contains(&self.stress_threshold) {
            return Err(Error::Value("support priority stress threshold must be in 0..1".to_owned()));
        }
        if self.global_per_day <= 0 {
            return Err(Error::Value("support priority daily cap must be positive".to_owned()));
        }
        Ok(())
    }
}

/// Both limits a Run's LLM rules read.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Limits {
    pub social: SocialLimits,
    pub priority: PriorityLimits,
}

impl Limits {
    pub fn check(&self) -> Result<()> {
        self.social.check()?;
        self.priority.check()
    }

    /// The limits of their JSON: `{"social": {...}, "priority": {...}}`, each field optional (the
    /// default where it is left out), as a test or a tool sets them.
    pub fn from_json(raw: &[u8]) -> Result<Self> {
        let value: Value = serde_json::from_slice(raw).map_err(|_| Error::Value("the LLM's limits are not JSON".to_owned()))?;
        let mut limits = Self::default();
        let object = value.as_object().ok_or_else(|| Error::Value("the LLM's limits are not an object".to_owned()))?;
        for (part, fields) in object {
            let fields = fields.as_object().ok_or_else(|| Error::Value(format!("the LLM's limits {part} are not an object")))?;
            for (name, item) in fields {
                let int = || item.as_i64().ok_or_else(|| Error::Value(format!("the LLM's limit {part}.{name} is not an int")));
                let float = || item.as_f64().ok_or_else(|| Error::Value(format!("the LLM's limit {part}.{name} is not a number")));
                match (part.as_str(), name.as_str()) {
                    ("social", "stress_threshold") => limits.social.stress_threshold = float()?,
                    ("social", "cooldown_minutes") => limits.social.cooldown_minutes = int()?,
                    ("social", "per_agent_per_day") => limits.social.per_agent_per_day = int()?,
                    ("social", "global_per_day") => limits.social.global_per_day = int()?,
                    ("social", "event_delay_minutes") => limits.social.event_delay_minutes = int()?,
                    ("social", "complaint_wait_minutes") => limits.social.complaint_wait_minutes = int()?,
                    ("priority", "stress_threshold") => limits.priority.stress_threshold = float()?,
                    ("priority", "global_per_day") => limits.priority.global_per_day = int()?,
                    _ => return Err(Error::Value(format!("the LLM's limits have no {part}.{name}"))),
                }
            }
        }
        limits.check()?;
        Ok(limits)
    }
}

// ---- The answer ------------------------------------------------------------------------------------

/// The answer `text` read against `schema` (`validate_task_output` of `llm/contracts.py`, for the
/// subset the core's schemas use: an object of closed properties, each a string or an integer with
/// an `enum`, a string's `maxLength`), as its value; None where it is not JSON or does not fit.
pub fn check_answer(text: &str, schema: &V) -> Option<Value> {
    if text.trim().is_empty() {
        return None;
    }
    let value: Value = serde_json::from_str(text).ok()?;
    fits(&value, schema).then_some(value)
}

fn fits(value: &Value, schema: &V) -> bool {
    let V::Map(schema) = schema else {
        return false;
    };
    let typed = match schema.get("type") {
        None => true,
        Some(V::Str(kind)) => match &**kind {
            "object" => value.is_object(),
            "string" => value.is_string(),
            "integer" => value.is_i64() || value.is_u64(),
            "number" => value.is_number(),
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),
            "array" => value.is_array(),
            _ => false,
        },
        Some(_) => false,
    };
    if !typed {
        return false;
    }
    if let Some(V::List(allowed)) = schema.get("enum") {
        if !allowed.iter().any(|item| same(value, item)) {
            return false;
        }
    }
    if let (Some(V::Int(most)), Some(text)) = (schema.get("maxLength"), value.as_str()) {
        if text.chars().count() as i64 > *most {
            return false;
        }
    }
    if let Some(object) = value.as_object() {
        let properties = match schema.get("properties") {
            Some(V::Map(properties)) => properties,
            _ => return false,
        };
        if object.keys().any(|name| !properties.contains_key(name.as_str())) {
            return false;
        }
        if let Some(V::List(required)) = schema.get("required") {
            if required.iter().any(|name| !matches!(name, V::Str(name) if object.contains_key(&**name))) {
                return false;
            }
        }
        return object.iter().all(|(name, item)| properties.get(name.as_str()).is_some_and(|property| fits(item, property)));
    }
    true
}

fn same(value: &Value, item: &V) -> bool {
    match (value, item) {
        (Value::String(text), V::Str(other)) => text == &**other,
        (Value::Number(number), V::Int(other)) => number.as_i64() == Some(*other),
        (Value::Bool(flag), V::Bool(other)) => flag == other,
        (Value::Null, V::Null) => true,
        _ => false,
    }
}

// ---- The tape --------------------------------------------------------------------------------------

/// The tape's entry of a call: `{"minute", "task", "stage", "prompt_version", "schema_version",
/// "input_hash", "state_hash"}` and the reply's fields, keys sorted. The reply's `text` is the
/// answer as the rules read it (`kept_text`): its canonical JSON with a free-text `rationale`
/// emptied, or the task's fallback.
pub fn tape_entry(minute: i64, task: &Task, reply: &Reply) -> Result<Vec<u8>> {
    let mut map = IndexMap::with_capacity(16);
    map.insert(key("minute"), V::Int(minute));
    map.insert(key("task"), text(task.task));
    map.insert(key("stage"), text(task.stage));
    map.insert(key("prompt_version"), text(&task.prompt_version));
    map.insert(key("schema_version"), V::Int(task.schema_version));
    map.insert(key("input_hash"), text(&task.input_hash()));
    map.insert(key("state_hash"), text(&task.state_hash));
    reply.put(&mut map);
    V::Map(map).to_json()
}

/// The text a tape keeps of an answer the rules read: the canonical JSON of its value, a
/// `rationale` emptied.
pub fn kept_text(answer: &Value) -> Result<String> {
    let mut value = V::from_json(answer).ok_or_else(|| Error::Value("an answer the core cannot hold".to_owned()))?;
    if let V::Map(map) = &mut value {
        if let Some(rationale) = map.get_mut("rationale") {
            *rationale = V::Str(Arc::from(""));
        }
    }
    String::from_utf8(value.to_json()?).map_err(|_| Error::Value("an answer that is not UTF-8".to_owned()))
}

/// One entry of a tape, read back.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub minute: i64,
    pub task: String,
    pub stage: String,
    pub prompt_version: String,
    pub schema_version: i64,
    pub input_hash: String,
    pub state_hash: String,
    pub reply: Reply,
}

impl Entry {
    /// The entry of its JSON (`tape_entry`'s).
    pub fn from_json(raw: &[u8]) -> Result<Self> {
        let mut value: Value = serde_json::from_slice(raw).map_err(|_| bad_entry("not JSON"))?;
        let object = value.as_object_mut().ok_or_else(|| bad_entry("not an object"))?;
        let mut take = |name: &str| object.remove(name).ok_or_else(|| bad_entry(&format!("no {name}")));
        let string = |value: Value, name: &str| value.as_str().map(str::to_owned).ok_or_else(|| bad_entry(&format!("{name} is not a string")));
        let minute = take("minute")?.as_i64().ok_or_else(|| bad_entry("minute is not an int"))?;
        let task = string(take("task")?, "task")?;
        let stage = string(take("stage")?, "stage")?;
        let prompt_version = string(take("prompt_version")?, "prompt_version")?;
        let schema_version = take("schema_version")?.as_i64().ok_or_else(|| bad_entry("schema_version is not an int"))?;
        let input_hash = string(take("input_hash")?, "input_hash")?;
        let state_hash = string(take("state_hash")?, "state_hash")?;
        let reply = Reply::from_json(&serde_json::to_vec(&value).map_err(|_| bad_entry("not JSON"))?)?;
        Ok(Self { minute, task, stage, prompt_version, schema_version, input_hash, state_hash, reply })
    }

    /// Whether the entry is the record of `task`: Python's `RecordedRouter` compares the stage, the
    /// input's and the state's hashes and the versions, and so does this, with the task's name.
    pub fn is_of(&self, task: &Task) -> bool {
        self.task == task.task
            && self.stage == task.stage
            && self.input_hash == task.input_hash()
            && self.state_hash == task.state_hash
            && self.prompt_version == task.prompt_version
            && self.schema_version == task.schema_version
    }
}

fn bad_entry(what: &str) -> Error {
    Error::Value(format!("an LLM tape's entry: {what}"))
}

/// The refusal of a call past the tape's end, without a link to go on with: Python's
/// `RecordedRouter`'s text.
pub const REPLAY_MISSING: &str = "missing recorded LLM decision; history cannot call a provider";
/// The refusal of a call that is not the tape's next entry.
pub const REPLAY_MISMATCH: &str = "recorded LLM input/state/version does not match reconstructed history";

/// A link that answers from a tape, in its order (Python's `RecordedRouter`): each call must be the
/// next entry's (`Entry::is_of`), and gets that entry's reply. Past the tape's end it goes on with
/// the link `then` (Python's `ContinuingRouter`), or refuses: computing the past again never asks a
/// provider. It is offline where the Run it replays was.
pub struct Replay {
    entries: Vec<Entry>,
    at: usize,
    offline: bool,
    then: Option<Box<dyn LlmLink>>,
}

impl Replay {
    /// The replay of the tape's entries (their JSON), from the first.
    pub fn new(entries: &[Vec<u8>], offline: bool) -> Result<Self> {
        let entries = entries.iter().map(|raw| Entry::from_json(raw)).collect::<Result<Vec<_>>>()?;
        Ok(Self { entries, at: 0, offline, then: None })
    }

    /// Goes on with `link` past the tape's end.
    pub fn then(mut self, link: Box<dyn LlmLink>) -> Self {
        self.then = Some(link);
        self
    }

    /// How many entries it has answered with.
    pub fn used(&self) -> usize {
        self.at
    }

    /// How many entries it holds.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl LlmLink for Replay {
    fn offline(&self) -> bool {
        self.offline
    }

    fn complete(&mut self, task: &Task) -> Result<Reply> {
        let Some(entry) = self.entries.get(self.at) else {
            return match &mut self.then {
                Some(link) => link.complete(task),
                None => Err(Error::Runtime(REPLAY_MISSING.to_owned())),
            };
        };
        if !entry.is_of(task) {
            return Err(Error::Runtime(REPLAY_MISMATCH.to_owned()));
        }
        self.at += 1;
        let mut reply = entry.reply.clone();
        if reply.fallback_used {
            reply.text = task.fallback_text.clone();
        }
        Ok(reply)
    }
}

fn key(name: &str) -> Arc<str> {
    Arc::from(name)
}

fn text(value: &str) -> V {
    V::Str(Arc::from(value))
}
