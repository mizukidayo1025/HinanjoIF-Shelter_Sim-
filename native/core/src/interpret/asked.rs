//! The intervention compiler asked of an LLM (#463): what the compiler does with an API key,
//! around the core's port to an LLM (`crate::llm`).
//!
//! `Compiling::new` reads the request and builds the task (`intervention_compiler`, the
//! intervention stage, the same payload, instructions and output schema as the offline answer's
//! hashes name); the caller hands each task to its link and gives the reply to `read`. An answer
//! that fits the schema is checked (`Compiling::accept`: the status's rules, the
//! effects against the summary, the program read by the core's EffectProgram contract); one that
//! fails those checks is asked to be repaired once (`intervention_repair`, the errors given back).
//! The result is the `CompilerResult` dump: the LLM's plan, or what the rules make of the text
//! where no plan was accepted (with why), or, under a stop policy, that the compiler is
//! unavailable.
//!
//! The LLM is offered only the operators the core checks and schedules (`program::supports`):
//! a plan of another operator could be neither previewed nor applied here.

use serde_json::Value;

use super::parse::{self, Parsed};
use super::{
    canonical, eligible, obj, offline_result, output_schema, payload, plan_value, prompt_hash, s, sha256_text, strs, text, validate_plan, Answer, Primitive,
    Request, MAX_COMPILER_MINUTE, OPUS_COMPILER_PROMPT_VERSION, PREVIEW_NOTICE,
};
use crate::error::{Error, Result};
use crate::llm::{check_answer, Reply, Task, INTERVENTION_COMPILER, INTERVENTION_REPAIR};
use crate::program::EffectProgram;
use crate::text::py_repr;
use crate::value::V;

const STAGE_INTERVENTION: &str = "intervention";
const REPAIR_PROMPT_VERSION: &str = "h07-opus-effect-compiler-v1-repair1";
const COMPILER_VERSION: &str = "h07-opus-effect-compiler-v1";
const SOURCE_ID: &str = "opus-effect-compiler-v1";
const INPUT_BYTE_LIMIT: i64 = 65_536;
const MAX_OUTPUT_TOKENS: i64 = 4096;
const FALLBACK_SUMMARY: &str = "オフライン時の固定応答です。適用可能な案は確定していません。";
const FALLBACK_LIMITATION: &str = "Opus compiler unavailable; use offline parser only for its recognized forms.";
const REPAIR_INSTRUCTION: &str = "Return one corrected complete JSON object. Do not add unsupported mechanics.";
const UNVALIDATED: &str = "AI（Opus）から検証済みの案を受け取れませんでした。";
const ALLOWANCE_UNAVAILABLE: &str = "利用枠を確保できなかったため、AI（Opus）を呼び出していません。";
const FALLBACK_REASONS: [(&str, &str); 13] = [
    ("offline", "offline modeのため、AI（Opus）を呼び出していません。"),
    ("input_token_limit", "送信する状況説明が1回あたりの入力上限を超えたため、AI（Opus）を呼び出していません。"),
    ("max_api_calls", "このRunのAPI呼出し回数の上限に達したため、AI（Opus）を呼び出していません。"),
    ("budget_exceeded", "予算の上限を超えるため、AI（Opus）を呼び出していません。"),
    ("unknown_model_price", "モデルの料金設定がないため、AI（Opus）を呼び出していません。"),
    ("shared_allowance_unavailable", ALLOWANCE_UNAVAILABLE),
    ("runtime_call_share_exhausted", ALLOWANCE_UNAVAILABLE),
    ("replay_miss", "記録済みの応答がないため、AI（Opus）を呼び出していません。"),
    ("cache_invalid", "記録済みの応答が検証に通りませんでした。"),
    ("rate_limited", "Opus APIが429（rate limit）を返しました。"),
    ("api_error", "Opus APIへの接続または応答に失敗しました。"),
    ("invalid_output", "Opusの応答が出力schemaの検証に通りませんでした。"),
    ("contract_error", "Opusの応答が出力schemaの検証に通りませんでした。"),
];

/// What the caller does next.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// Hand this task (its JSON) to the link, and its reply to `read`.
    Ask(Vec<u8>),
    /// The `CompilerResult` dump's canonical JSON.
    Done(Vec<u8>),
    /// No plan was accepted and the router's policy stops instead of falling back
    /// (`intervention_compiler_unavailable`): the reason, at most 500 characters.
    Unavailable(String),
}

/// An intervention's text being read by an LLM.
pub struct Compiling {
    request: Request,
    eligible: Vec<&'static Primitive>,
    schema: V,
    prompt_hash: String,
    payload: V,
    state_hash: String,
    fallback_text: String,
    first: Task,
    repairing: bool,
    replies: Vec<Reply>,
    errors: Vec<String>,
    stop: bool,
}

fn task(name: &'static str, prompt_version: &str, payload: &V, state_hash: &str, schema: &V, fallback_text: &str) -> Result<Task> {
    Ok(Task {
        task: name,
        stage: STAGE_INTERVENTION,
        prompt_version: prompt_version.to_owned(),
        schema_version: 1,
        input_text: String::from_utf8(canonical(payload)?).map_err(|_| Error::Value("a payload that is not UTF-8".to_owned()))?,
        state_hash: state_hash.to_owned(),
        output_schema: schema.clone(),
        fallback_text: fallback_text.to_owned(),
        max_output_tokens: MAX_OUTPUT_TOKENS,
        input_byte_limit: Some(INPUT_BYTE_LIMIT),
    })
}

impl Compiling {
    /// The compiler asked of an LLM for `request` (a `ParseRequest`'s JSON, as `compile_offline`
    /// reads it); `stop` whether the router's error policy stops instead of falling back. None
    /// where no operator the core runs is eligible: the answer is `compile_offline`'s.
    pub fn new(request: &[u8], stop: bool) -> Result<Option<Compiling>> {
        let request = super::read_request(request)?;
        let eligible = eligible(&request.summary, crate::program::supports)?;
        if eligible.is_empty() {
            return Ok(None);
        }
        let schema = output_schema(&request.summary, &eligible)?;
        let prompt_hash = prompt_hash(&schema)?;
        let payload = payload(&request, &eligible)?;
        let state_hash = sha256_text(&canonical(&obj([
            ("branch_id", s(&request.branch_id)),
            ("selected_minute", V::Int(request.selected_minute)),
            ("scenario_summary", request.summary_value.clone()),
            ("primitive_registry", strs(&eligible.iter().map(|item| item.op.as_str()).collect::<Vec<_>>())),
        ]))?);
        let fallback = obj([
            ("status", s("not_simulatable")),
            ("support_mode", s("not_simulatable")),
            ("mechanism_summary", s(FALLBACK_SUMMARY)),
            ("effects", V::List(Vec::new())),
            ("assumptions", V::List(Vec::new())),
            ("limitations", strs(&[FALLBACK_LIMITATION])),
            ("questions", V::List(Vec::new())),
            ("choices", V::List(Vec::new())),
        ]);
        let fallback_text = String::from_utf8(canonical(&fallback)?).map_err(|_| Error::Value("a fallback that is not UTF-8".to_owned()))?;
        let first = task(INTERVENTION_COMPILER, OPUS_COMPILER_PROMPT_VERSION, &payload, &state_hash, &schema, &fallback_text)?;
        Ok(Some(Compiling {
            request,
            eligible,
            schema,
            prompt_hash,
            payload,
            state_hash,
            fallback_text,
            first,
            repairing: false,
            replies: Vec::new(),
            errors: Vec::new(),
            stop,
        }))
    }

    /// The first task's JSON.
    pub fn task_json(&self) -> Result<Vec<u8>> {
        self.first.to_json()
    }

    fn repair_task(&self, previous: &str) -> Result<Task> {
        let V::Map(mut payload) = self.payload.clone() else {
            return Err(Error::Value("a payload that is not an object".to_owned()));
        };
        let errors: Vec<&str> = self.errors.iter().take(20).map(String::as_str).collect();
        payload.insert(
            "repair_request".into(),
            obj([("previous_candidate", s(previous)), ("validation_errors", strs(&errors)), ("instruction", s(REPAIR_INSTRUCTION))]),
        );
        task(INTERVENTION_REPAIR, REPAIR_PROMPT_VERSION, &V::Map(payload), &self.state_hash, &self.schema, &self.fallback_text)
    }

    /// Reads the link's reply to the last task (`llm::Reply`'s JSON; None where the call failed):
    /// the next step.
    pub fn read(&mut self, reply: Option<&[u8]>) -> Result<Step> {
        let Some(raw) = reply else {
            return self.finish(None);
        };
        let mut reply = Reply::from_json(raw)?;
        let document = if reply.fallback_used {
            None
        } else {
            match check_answer(&reply.text, &self.schema) {
                Some(document) => Some(document),
                None => {
                    reply.outcome = "invalid_output".to_owned();
                    reply.fallback_used = true;
                    None
                }
            }
        };
        let text = reply.text.clone();
        self.replies.push(reply);
        let Some(document) = document else {
            return self.finish(None);
        };
        match self.accept(&document) {
            Ok(plan) => self.finish(Some((document, plan))),
            Err(errors) => {
                self.errors = errors;
                if self.repairing {
                    return self.finish(None);
                }
                self.repairing = true;
                Ok(Step::Ask(self.repair_task(&text)?.to_json()?))
            }
        }
    }

    /// The compiler's usage: the first task's version and input, the replies' sums.
    fn usage(&self) -> V {
        let last = self.replies.last();
        let model_id = last.and_then(|reply| reply.model_id.as_deref()).filter(|model| !model.starts_with("offline:"));
        let error_kind = last.filter(|reply| reply.fallback_used).map(|reply| reply.outcome.as_str());
        obj([
            ("model_id", model_id.map(s).unwrap_or(V::Null)),
            ("prompt_version", s(&self.first.prompt_version)),
            ("prompt_hash", s(&self.prompt_hash)),
            ("input_hash", s(&self.first.input_hash())),
            ("api_request_count", V::Int(self.replies.iter().map(|reply| reply.api_request_count).sum())),
            ("input_tokens", V::Int(self.replies.iter().map(|reply| reply.input_tokens).sum())),
            ("output_tokens", V::Int(self.replies.iter().map(|reply| reply.output_tokens).sum())),
            ("estimated_cost_usd", V::Float(self.replies.iter().map(|reply| reply.estimated_cost_usd).sum())),
            ("cache_hit", V::Bool(!self.replies.is_empty() && self.replies.iter().all(|reply| reply.cache_hit))),
            ("error_kind", error_kind.map(s).unwrap_or(V::Null)),
        ])
    }

    /// Why the provider's answer was not used, from the last reply.
    fn failure_reason(&self) -> String {
        match self.replies.last() {
            Some(last) if last.fallback_used => {
                let message = FALLBACK_REASONS.iter().find(|(outcome, _)| *outcome == last.outcome).map_or(UNVALIDATED, |(_, message)| *message);
                format!("{message}（理由コード: {}）", last.outcome)
            }
            _ => UNVALIDATED.to_owned(),
        }
    }

    fn finish(&self, accepted: Option<(Value, Option<V>)>) -> Result<Step> {
        let usage = self.usage();
        let request = &self.request;
        let Some((document, plan)) = accepted else {
            let reason = if self.errors.is_empty() {
                self.failure_reason()
            } else {
                format!("Opusの案をschema/domain検証できませんでした: {}", self.errors.join("; "))
            };
            if self.stop {
                return Ok(Step::Unavailable(text::head(&reason, 500).to_owned()));
            }
            let parsed = parse::parse(&request.text, &request.branch_id, request.selected_minute, &request.summary)?;
            let answer = offline_result(request, parsed, &self.eligible, true, &reason)?;
            return Ok(Step::Done(canonical(&super::answer_value(answer, usage))?));
        };
        let status = document["status"].as_str().unwrap_or_default();
        let zone_ids: Vec<&str> = request.summary.zones.iter().map(|zone| zone.id.as_str()).collect();
        let choices: Vec<&str> = document["choices"].as_array().map(|items| items.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        if status == "clarification_required" && !choices.is_empty() && choices.iter().all(|choice| zone_ids.contains(choice)) {
            if let Parsed::Parsed(offline) = parse::parse(&request.text, &request.branch_id, request.selected_minute, &request.summary)? {
                if parse::covers_all_clauses(&request.text)? {
                    // Opus asked which zone, but the rules choose it from the scenario data the
                    // same way every time (#35); the plan says which data chose it.
                    let reason = "Opus asked for the zone, which the scenario data determines";
                    let answer = match validate_plan(&offline, &request.summary, &self.eligible) {
                        Ok(()) => Answer {
                            status: "compiled",
                            support_mode: offline.support_mode,
                            plan: Some(plan_value(&offline)),
                            mechanism_summary:
                                "Opusは対象Zoneを聞き返しましたが、シナリオのデータから一意に決まるため、認識済みのオフライン規則で対象と作用を構成しました。",
                            limitations: super::with_reason(
                                vec!["これは規則ベースのfallbackで、自由文全体をOpusが解釈した結果ではありません。".to_owned()],
                                reason,
                            ),
                            questions: Vec::new(),
                            choices: Vec::new(),
                            fallback_used: true,
                        },
                        Err(message) => Answer {
                            status: "not_simulatable",
                            support_mode: "not_simulatable",
                            plan: None,
                            mechanism_summary: "オフライン案は現在のscenario/policy条件を満たしません。",
                            limitations: vec![text::head(&format!("{reason}; offline proposal rejected: {message}"), 500).to_owned()],
                            questions: Vec::new(),
                            choices: Vec::new(),
                            fallback_used: true,
                        },
                    };
                    return Ok(Step::Done(canonical(&super::answer_value(answer, usage))?));
                }
            }
        }
        let texts = |key: &str| -> Vec<String> {
            document[key].as_array().map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default()
        };
        let compiled = status == "compiled";
        let result = obj([
            ("status", s(status)),
            ("support_mode", s(document["support_mode"].as_str().unwrap_or_default())),
            ("draft_plan", plan.unwrap_or(V::Null)),
            ("mechanism_summary", s(document["mechanism_summary"].as_str().unwrap_or_default())),
            ("limitations", strs(&texts("limitations"))),
            ("questions", if compiled { V::List(Vec::new()) } else { strs(&texts("questions")) }),
            ("choices", if compiled { V::List(Vec::new()) } else { strs(&texts("choices")) }),
            ("preview_notice", s(PREVIEW_NOTICE)),
            ("fallback_used", V::Bool(false)),
            ("usage", usage),
        ]);
        Ok(Step::Done(canonical(&result)?))
    }

    /// The answer's checks, and for a compiled one its plan (the `DraftPlan` dump); Err with the
    /// errors to repair.
    fn accept(&self, document: &Value) -> std::result::Result<Option<V>, Vec<String>> {
        let one = |message: String| vec![message];
        let status = document["status"].as_str().unwrap_or_default();
        let mode = document["support_mode"].as_str().unwrap_or_default();
        let list = |key: &str| document[key].as_array().map(Vec::as_slice).unwrap_or(&[]);
        let (effects, assumptions, limitations, questions, choices) =
            (list("effects"), list("assumptions"), list("limitations"), list("questions"), list("choices"));
        match status {
            "not_simulatable" => {
                if mode != "not_simulatable" || !effects.is_empty() {
                    return Err(one("not_simulatable output cannot contain executable effects".to_owned()));
                }
                if limitations.is_empty() {
                    return Err(one("not_simulatable output must explain the missing mechanism".to_owned()));
                }
            }
            "clarification_required" => {
                if mode != "clarification_required" || !effects.is_empty() {
                    return Err(one("clarification output cannot contain executable effects".to_owned()));
                }
                if questions.is_empty() && choices.is_empty() {
                    return Err(one("clarification output must ask a question or offer choices".to_owned()));
                }
                let chosen: Vec<&str> = choices.iter().filter_map(Value::as_str).collect();
                let summary = &self.request.summary;
                if !chosen.is_empty() && !summary.target_ids.iter().any(|(_, ids)| chosen.iter().all(|choice| ids.iter().any(|id| id == choice))) {
                    return Err(one("clarification choices must be existing IDs of one target kind".to_owned()));
                }
            }
            "compiled" => {
                if !matches!(mode, "native" | "composed" | "approximate") {
                    return Err(one("compiled output needs a supported or explicitly approximate mode".to_owned()));
                }
                if effects.is_empty() {
                    return Err(one("compiled output must contain at least one effect".to_owned()));
                }
                if mode == "approximate" && (assumptions.is_empty() || limitations.is_empty()) {
                    return Err(one("approximate output requires assumptions and limitations".to_owned()));
                }
                let mut ops: Vec<&str> = effects.iter().filter_map(|effect| effect["op"].as_str()).collect();
                ops.sort_unstable();
                ops.dedup();
                if mode == "composed" && ops.len() < 2 {
                    return Err(one("composed output must combine at least two primitive operators".to_owned()));
                }
                self.check_effects(effects, assumptions.len()).map_err(one)?;
            }
            other => return Err(one(format!("unknown compiler status {}", py_repr(other)))),
        }
        let summary = document["mechanism_summary"].as_str().unwrap_or_default();
        if text::strip(summary).is_empty() || summary.chars().count() > 1000 {
            return Err(one("mechanism_summary must describe the proposed mapping".to_owned()));
        }
        for (key, items, most) in [("limitations", limitations, 500), ("questions", questions, 10)] {
            let bad = |item: &Value| item.as_str().is_none_or(|item| text::strip(item).is_empty() || item.chars().count() > 500);
            if items.len() > most || items.iter().any(bad) {
                return Err(one(format!("{key} must contain bounded, non-empty text")));
            }
        }
        if status != "compiled" {
            return Ok(None);
        }
        for item in assumptions {
            let description = item["description"].as_str().unwrap_or_default();
            let units = item["units"].as_str().unwrap_or_default();
            if description.is_empty() || description.chars().count() > 500 || !super::stable_id(units) {
                return Err(one("an assumption must have a description and stable units".to_owned()));
            }
        }
        self.plan(document).map(Some).map_err(|error| one(error.to_string()))
    }

    /// The semantic checks of the answer's effects against the summary, for the operators the core
    /// runs.
    fn check_effects(&self, effects: &[Value], assumptions: usize) -> std::result::Result<(), String> {
        let summary = &self.request.summary;
        let events: usize = effects.iter().map(|effect| if effect.get("end_minute").is_some() { 2 } else { 1 }).sum();
        if effects.len() > 50 || events > 100 {
            return Err("program exceeds the bounded effect/event count".to_owned());
        }
        let end_of_clock = summary.duration.unwrap_or(MAX_COMPILER_MINUTE);
        let mut staff_additions = 0;
        let mut toilet_additions = 0;
        for (index, effect) in effects.iter().enumerate() {
            let op = effect["op"].as_str().unwrap_or_default();
            let Some(primitive) = self.eligible.iter().find(|item| item.op == op) else {
                return Err(format!("effect {index} uses an unavailable primitive {}", py_repr(op)));
            };
            if !primitive.required_policies.iter().all(|policy| summary.available_policies.contains(policy)) {
                return Err(format!("effect {index} requires an unavailable policy"));
            }
            let kind = effect["target_kind"].as_str().unwrap_or_default();
            let target = effect["target_id"].as_str().unwrap_or_default();
            if !primitive.target_kinds.iter().any(|item| item == kind) || !summary.ids(kind).iter().any(|id| id == target) {
                return Err(format!("effect {index} references an unknown or incompatible target"));
            }
            if effect.get("candidate_ids").and_then(Value::as_array).is_some_and(|ids| !ids.is_empty()) {
                // Only a reunion names candidates, and the core does not run one.
                return Err("candidate_ids are restricted to known people in a reunion".to_owned());
            }
            let start = effect["start_minute"].as_i64().unwrap_or_default();
            if start >= end_of_clock {
                return Err(format!("effect {index} starts outside the scenario"));
            }
            let end = effect.get("end_minute").and_then(Value::as_i64);
            if let Some(end) = end {
                if end <= start {
                    return Err(format!("effect {index} has an invalid time window"));
                }
                if summary.duration.is_some_and(|duration| end > duration) {
                    return Err(format!("effect {index} ends outside the scenario"));
                }
                if !primitive.timed {
                    return Err(format!("effect {index} adds end_minute to a one-shot operation"));
                }
            }
            if primitive.units.len() != 1 {
                return Err(format!("primitive {} needs an explicit registered unit", py_repr(op)));
            }
            let indexes = effect.get("assumption_indexes").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
            if indexes.iter().any(|item| item.as_u64().is_none_or(|item| item as usize >= assumptions)) {
                return Err(format!("effect {index} references an undeclared assumption"));
            }
            let parameter = |name: &str| effect["parameters"].get(name).and_then(Value::as_i64).unwrap_or_default();
            match op {
                "facility_outage" => {
                    let Some(facility) = summary.facilities.iter().find(|item| item.id == target) else {
                        return Err(format!("effect {index} references an unknown facility"));
                    };
                    if parameter("units") > facility.working_units {
                        return Err(format!("effect {index} outage exceeds working units"));
                    }
                }
                "water_outage" => {
                    if end.is_none() {
                        return Err(format!("effect {index} requires an explicit water-restoration minute"));
                    }
                    if kind == "facility" {
                        if !summary.facilities.iter().any(|item| item.id == target && item.kind == "water_station") {
                            return Err(format!("effect {index} must target a water station"));
                        }
                    } else if !summary.zones.iter().any(|zone| zone.id == target && zone.has_water_station) {
                        return Err(format!("effect {index} zone has no water station"));
                    }
                }
                "add_staff" => staff_additions += parameter("count"),
                "add_toilet" => toilet_additions += parameter("count"),
                _ => {}
            }
        }
        if summary.staff_count as i64 + staff_additions > 100 {
            return Err("program exceeds the 100-staff runtime limit".to_owned());
        }
        if summary.facilities.len() as i64 + toilet_additions > 200 {
            return Err("program exceeds the 200-facility runtime limit".to_owned());
        }
        Ok(())
    }

    /// The plan of an accepted answer, as the `DraftPlan` dump, its parameters as the core's
    /// EffectProgram contract reads and dumps them (the contract refuses what it cannot read).
    fn plan(&self, document: &Value) -> Result<V> {
        let request = &self.request;
        let answer = V::from_json(document).ok_or_else(|| Error::Value("an answer the core cannot hold".to_owned()))?;
        let response_hash = sha256_text(&canonical(&answer)?);
        let identity = format!("{}\0{}\0{}\0{}\0{}", request.branch_id, request.text, request.selected_minute, self.first.input_hash(), response_hash);
        let program_id = format!("opus-{}", &sha256_text(identity.as_bytes())[..24]);
        let mut assumption_ids = Vec::new();
        let mut assumptions = Vec::new();
        for (index, item) in document["assumptions"].as_array().map(Vec::as_slice).unwrap_or(&[]).iter().enumerate() {
            let number = index + 1;
            let id = format!("assumption-{number:02}-{}", &sha256_text(format!("{program_id}{number}").as_bytes())[..10]);
            assumptions.push(obj([
                ("assumption_id", s(&id)),
                ("description", s(item["description"].as_str().unwrap_or_default())),
                ("source_id", s(SOURCE_ID)),
                ("units", s(item["units"].as_str().unwrap_or_default())),
                ("value", s(item["value"].as_str().unwrap_or_default())),
            ]));
            assumption_ids.push(id);
        }
        let mut effects = Vec::new();
        let mut starts = Vec::new();
        for (index, item) in document["effects"].as_array().map(Vec::as_slice).unwrap_or(&[]).iter().enumerate() {
            let number = index + 1;
            let op = item["op"].as_str().unwrap_or_default();
            let primitive =
                self.eligible.iter().find(|primitive| primitive.op == op).ok_or_else(|| Error::Value(format!("unknown operator {}", py_repr(op))))?;
            let start = item["start_minute"].as_i64().unwrap_or_default();
            starts.push(start);
            let effect_id = format!("effect-{number:02}-{}", &sha256_text(format!("{program_id}{op}{number}").as_bytes())[..10]);
            let referenced: Vec<&str> = item
                .get("assumption_indexes")
                .and_then(Value::as_array)
                .map(|indexes| indexes.iter().filter_map(Value::as_u64).filter_map(|index| assumption_ids.get(index as usize)).map(String::as_str).collect())
                .unwrap_or_default();
            let mut policies = primitive.required_policies.clone();
            policies.sort();
            let parameters = V::from_json(&item["parameters"]).ok_or_else(|| Error::Value("parameters the core cannot hold".to_owned()))?;
            effects.push(obj([
                ("effect_id", s(&effect_id)),
                ("op", s(op)),
                (
                    "target_selector",
                    obj([
                        ("kind", s(item["target_kind"].as_str().unwrap_or_default())),
                        ("target_id", s(item["target_id"].as_str().unwrap_or_default())),
                        ("candidate_ids", V::List(Vec::new())),
                    ]),
                ),
                ("start_minute", V::Int(start)),
                ("end_minute", item.get("end_minute").and_then(Value::as_i64).map_or(V::Null, V::Int)),
                ("guard", V::Null),
                ("parameters", parameters),
                ("units", s(&primitive.units[0])),
                ("source_id", s(SOURCE_ID)),
                ("assumption_ids", strs(&referenced)),
                ("policy_dependencies", strs(&policies)),
                ("causation_id", V::Null),
                ("reversible", V::Bool(primitive.timed)),
            ]));
        }
        let fork = starts.iter().copied().min().map_or(request.selected_minute, |first| first.min(request.selected_minute));
        let program = obj([
            ("schema_version", V::Int(1)),
            ("program_id", s(&program_id)),
            ("narrative", s(document["mechanism_summary"].as_str().unwrap_or_default())),
            ("source_text", s(&request.text)),
            ("simulation_start_minute", V::Int(fork)),
            ("effects", V::List(effects)),
            ("assumptions", V::List(assumptions)),
            ("support_mode", s(document["support_mode"].as_str().unwrap_or_default())),
            (
                "compiler_provenance",
                obj([
                    ("compiler_kind", s("llm")),
                    ("compiler_version", s(COMPILER_VERSION)),
                    ("model_id", self.replies.last().and_then(|reply| reply.model_id.as_deref()).map(s).unwrap_or(V::Null)),
                    ("prompt_hash", s(&self.prompt_hash)),
                ]),
            ),
        ]);
        // The contract reads the program (each effect's parameters by its operator's model) and
        // dumps it with the defaults: the plan holds that dump.
        let dump = EffectProgram::of_json(&program.to_json()?)?.dump();
        let V::Map(mut plan) = dump else {
            return Err(Error::Value("a program's dump that is not an object".to_owned()));
        };
        plan.insert("branch_id".into(), s(&request.branch_id));
        plan.insert("fork_at_minute".into(), V::Int(fork));
        plan.insert("ambiguities".into(), V::List(Vec::new()));
        plan.insert("requires_confirmation".into(), V::Bool(true));
        Ok(V::Map(plan))
    }
}
