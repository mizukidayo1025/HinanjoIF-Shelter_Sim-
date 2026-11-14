//! `parse_scenario_text(text, router)` when the router asked the LLM (Issue #421): the browser
//! site asks it through its relay (deploy/workers/relay_tasks.mjs's `scenario_text`), with the
//! task Python sends (`scenario_text_task.json`, which `native/core/tools/scenario_text_task.py`
//! writes from Python), and reads the answer here as Python reads it.
//!
//! The answer is untrusted input. As the router does (`validate_task_output`), a blank answer, one
//! that is not JSON (Python's `json.loads`, which refuses `NaN` and `Infinity`) or one the task's
//! schema refuses (`_validate_value` of `llm/contracts.py`) is replaced by the rules' fallback:
//! the parse is the offline one (`rules_fallback`). An answer that passes is read back as
//! `_parse_llm_contract` reads it (Python's `int`, `float` and `str` of its strings, the conditions
//! validated as their models) and merged with the rules' values by `_merge_explicit`: what the
//! rules found stays, what they did not is `llm_extracted`, and the source is `prepare_llm`. An
//! answer `_parse_llm_contract` refuses leaves the rules' values alone (`rules`).
//!
//! The browser computes only the conditions the request contract lets through (`request.rs`):
//! the initial water and food. A condition of the answer that is valid but outside them
//! (population conditions, an evacuation selection, facility zones, zone area shares, a schedule)
//! is `Refusal::Unsupported`; the Worker then reads the text without the answer and says why.

use std::sync::{Arc, OnceLock};

use indexmap::IndexMap;

use super::{finish, offline, read_rules, TextParse, Values, FALLBACK_WARNING};
use crate::scenario::request::{InitialScenarioRequest, Refusal};
use crate::text::{re_decimal, re_space};
use crate::value::V;

/// The task as Python sends it (`tests/support/core_scenario_text.py`'s `task_document`).
pub const TASK_JSON: &str = include_str!("../scenario_text_task.json");

/// The task's document; its `schema` is what an answer is checked against.
pub fn task() -> &'static V {
    static TASK: OnceLock<V> = OnceLock::new();
    TASK.get_or_init(|| {
        let parsed: serde_json::Value = serde_json::from_str(TASK_JSON).expect("the task's JSON");
        V::from_json(&parsed).expect("the task's numbers")
    })
}

fn member<'a>(value: &'a V, key: &str) -> Option<&'a V> {
    match value {
        V::Map(map) => map.get(key),
        _ => None,
    }
}

/// What the server's `scenario-preview` reads of a text when the LLM answered `output`.
pub fn parse_answer(text: &str, output: &str) -> Result<TextParse, Refusal> {
    let explicit = read_rules(text)?;
    let Some(answer) = checked_answer(output) else {
        return offline(text, explicit);
    };
    let rule_fields: Vec<&'static str> = explicit.keys().copied().collect();
    let parsed = match read_contract(&answer)? {
        Read::Values(values) => values,
        Read::Refused => return finish(text, &explicit, &rule_fields, &[], "rules", vec![FALLBACK_WARNING]),
    };
    let (merged, extracted) = merge_explicit(&explicit, parsed)?;
    finish(text, &merged, &rule_fields, &extracted, "prepare_llm", Vec::new())
}

// ---- The router's check (`validate_task_output`) ----------------------------------------------------

/// The answer as `json.loads` reads it, when it passes the task's schema.
fn checked_answer(output: &str) -> Option<V> {
    if output.chars().all(re_space) {
        return None;
    }
    let parsed: serde_json::Value = serde_json::from_str(output).ok()?;
    let value = V::from_json(&parsed)?;
    let schema = member(task(), "schema")?;
    valid(&value, schema, schema).then_some(value)
}

/// `_validate_value`: whether `value` passes `schema` (`root` holds the `$defs`).
fn valid(value: &V, schema: &V, root: &V) -> bool {
    let V::Map(rules) = schema else {
        return false;
    };
    if let Some(reference) = rules.get("$ref") {
        let target = match reference {
            V::Str(text) => {
                text.strip_prefix("#/$defs/").map(|name| ("$defs", name)).or_else(|| text.strip_prefix("#/definitions/").map(|name| ("definitions", name)))
            }
            _ => None,
        };
        let Some(target) = target.and_then(|(container, name)| member(member(root, container)?, name)).filter(|target| matches!(target, V::Map(_))) else {
            return false;
        };
        return valid(value, target, root);
    }
    if let Some(V::List(candidates)) = rules.get("anyOf") {
        return candidates.iter().any(|candidate| valid(value, candidate, root));
    }
    if let Some(V::List(candidates)) = rules.get("oneOf") {
        return candidates.iter().filter(|candidate| valid(value, candidate, root)).count() == 1;
    }
    let expected = match rules.get("type") {
        Some(V::Str(name)) => Some(&**name),
        Some(_) => return false,
        None => None,
    };
    if let Some(expected) = expected {
        let fits = match expected {
            "object" => matches!(value, V::Map(_)),
            "array" => matches!(value, V::List(_)),
            "string" => matches!(value, V::Str(_)),
            "integer" => matches!(value, V::Int(_) | V::Big(_)),
            "number" => matches!(value, V::Int(_) | V::Big(_) | V::Float(_)),
            "boolean" => matches!(value, V::Bool(_)),
            "null" => matches!(value, V::Null),
            _ => false,
        };
        if !fits {
            return false;
        }
    }
    if let Some(fixed) = rules.get("const") {
        if !py_eq(value, fixed) || matches!(value, V::Bool(_)) != matches!(fixed, V::Bool(_)) {
            return false;
        }
    }
    if let Some(allowed) = rules.get("enum") {
        let V::List(allowed) = allowed else {
            return false;
        };
        if !allowed.iter().any(|item| py_eq(value, item)) {
            return false;
        }
    }
    if matches!(expected, Some("integer" | "number")) {
        let Some(number) = number_of(value) else {
            return false;
        };
        if !number.is_finite() {
            return false;
        }
        let bound = |key: &str| rules.get(key).and_then(number_of);
        if bound("minimum").is_some_and(|low| number < low) || bound("maximum").is_some_and(|high| number > high) {
            return false;
        }
        if bound("exclusiveMinimum").is_some_and(|low| number <= low) || bound("exclusiveMaximum").is_some_and(|high| number >= high) {
            return false;
        }
    }
    match (expected, value) {
        (Some("object"), V::Map(items)) => {
            let empty = IndexMap::new();
            let properties = match rules.get("properties") {
                Some(V::Map(properties)) => properties,
                _ => &empty,
            };
            let extra = rules.get("additionalProperties").filter(|extra| matches!(extra, V::Map(_)));
            if extra.is_none() && items.keys().any(|key| !properties.contains_key(key)) {
                return false;
            }
            if let Some(V::List(required)) = rules.get("required") {
                if required.iter().any(|field| !matches!(field, V::Str(name) if items.contains_key(name))) {
                    return false;
                }
            }
            items.iter().all(|(key, item)| match properties.get(key).or(extra) {
                Some(item_schema) => valid(item, item_schema, root),
                // `properties.get(key, extra_schema)` of a key the schema allows but does not type.
                None => false,
            })
        }
        (Some("array"), V::List(items)) => {
            let count = items.len() as f64;
            let low = rules.get("minItems").and_then(number_of).unwrap_or(0.0);
            let high = rules.get("maxItems").and_then(number_of).unwrap_or(1_000_000.0);
            if count < low || count > high {
                return false;
            }
            match rules.get("items") {
                Some(item_schema) => items.iter().all(|item| valid(item, item_schema, root)),
                None => true,
            }
        }
        (Some("string"), V::Str(text)) => {
            let count = text.chars().count() as f64;
            let low = rules.get("minLength").and_then(number_of).unwrap_or(0.0);
            let high = rules.get("maxLength").and_then(number_of).unwrap_or(1_000_000.0);
            if count < low || count > high {
                return false;
            }
            match rules.get("pattern") {
                Some(V::Str(pattern)) => Pattern::of(pattern).is_some_and(|pattern| pattern.searches(text)),
                Some(_) => false,
                None => true,
            }
        }
        _ => true,
    }
}

/// A number's value (an int past f64's precision is close enough for the schema's bounds).
fn number_of(value: &V) -> Option<f64> {
    match value {
        V::Int(number) => Some(*number as f64),
        V::Big(text) => text.parse().ok(),
        V::Float(number) => Some(*number),
        V::Bool(flag) => Some(f64::from(u8::from(*flag))),
        _ => None,
    }
}

/// Python's `==` of two values `json.loads` read: `True == 1 == 1.0`.
fn py_eq(left: &V, right: &V) -> bool {
    match (left, right) {
        (V::Null, V::Null) => true,
        (V::Str(a), V::Str(b)) => a == b,
        (V::List(a), V::List(b)) => a.len() == b.len() && a.iter().zip(b).all(|(a, b)| py_eq(a, b)),
        (V::Map(a), V::Map(b)) => a.len() == b.len() && a.iter().all(|(key, item)| b.get(key).is_some_and(|other| py_eq(item, other))),
        (V::Big(a), V::Big(b)) => a == b,
        (V::Big(_), V::Int(_) | V::Bool(_)) | (V::Int(_) | V::Bool(_), V::Big(_)) => false,
        (V::Int(a), V::Int(b)) => a == b,
        (V::Bool(a), V::Bool(b)) => a == b,
        (V::Bool(a), V::Int(b)) | (V::Int(b), V::Bool(a)) => i64::from(*a) == *b,
        _ => match (number_of(left), number_of(right)) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        },
    }
}

/// The patterns the task's schema holds: `^`, then character classes each with a quantifier
/// (none, `*`, `+`, `?` or `{m,n}`), then `$`, as `re.search` matches them (`$` also before a
/// final newline). Another pattern is not read, and an answer it would check fails.
struct Pattern {
    atoms: Vec<(Vec<(char, char)>, usize, usize)>,
}

impl Pattern {
    fn of(pattern: &str) -> Option<Pattern> {
        let chars: Vec<char> = pattern.chars().collect();
        if chars.first() != Some(&'^') || chars.last() != Some(&'$') || chars.len() < 2 {
            return None;
        }
        let body = &chars[1..chars.len() - 1];
        let mut atoms = Vec::new();
        let mut at = 0;
        while at < body.len() {
            if body[at] != '[' {
                return None;
            }
            let close = body[at + 1..].iter().position(|char| *char == ']')? + at + 1;
            let inner = &body[at + 1..close];
            if inner.is_empty() || inner[0] == '^' || inner.contains(&'\\') || inner.contains(&'[') {
                return None;
            }
            let mut ranges = Vec::new();
            let mut index = 0;
            while index < inner.len() {
                if index + 2 < inner.len() && inner[index + 1] == '-' {
                    ranges.push((inner[index], inner[index + 2]));
                    index += 3;
                } else {
                    ranges.push((inner[index], inner[index]));
                    index += 1;
                }
            }
            at = close + 1;
            let (low, high) = match body.get(at) {
                Some('*') => {
                    at += 1;
                    (0, usize::MAX)
                }
                Some('+') => {
                    at += 1;
                    (1, usize::MAX)
                }
                Some('?') => {
                    at += 1;
                    (0, 1)
                }
                Some('{') => {
                    let end = body[at..].iter().position(|char| *char == '}')? + at;
                    let inside: String = body[at + 1..end].iter().collect();
                    at = end + 1;
                    match inside.split_once(',') {
                        Some((low, high)) => (low.parse().ok()?, if high.is_empty() { usize::MAX } else { high.parse().ok()? }),
                        None => {
                            let count = inside.parse().ok()?;
                            (count, count)
                        }
                    }
                }
                _ => (1, 1),
            };
            atoms.push((ranges, low, high));
        }
        Some(Pattern { atoms })
    }

    fn searches(&self, text: &str) -> bool {
        let chars: Vec<char> = text.chars().collect();
        // `$` matches at the end, or before a newline that ends the text.
        let ends = |at: usize| at == chars.len() || (at + 1 == chars.len() && chars[at] == '\n');
        fn walk(atoms: &[(Vec<(char, char)>, usize, usize)], chars: &[char], at: usize, ends: &dyn Fn(usize) -> bool) -> bool {
            let Some(((ranges, low, high), rest)) = atoms.split_first() else {
                return ends(at);
            };
            let fits = |char: char| ranges.iter().any(|(from, to)| *from <= char && char <= *to);
            let mut count = 0;
            while count < *high && at + count < chars.len() && fits(chars[at + count]) {
                count += 1;
            }
            // Greedy, then backtrack.
            loop {
                if count < *low {
                    return false;
                }
                if walk(rest, chars, at + count, ends) {
                    return true;
                }
                if count == 0 {
                    return false;
                }
                count -= 1;
            }
        }
        walk(&self.atoms, &chars, 0, &ends)
    }
}

// ---- `_parse_llm_contract` -----------------------------------------------------------------------

enum Read {
    Values(Values),
    /// Python raised: the rules' values stay alone.
    Refused,
}

/// The scalar fields of the contract and how Python converts each.
const SCALARS: [(&str, Kind); 9] = [
    ("municipality", Kind::Text),
    ("disaster", Kind::Text),
    ("evacuees", Kind::Int),
    ("duration_days", Kind::Float),
    ("gross_area_m2", Kind::Float),
    ("accessible_toilet_count", Kind::Int),
    ("staff_count", Kind::Int),
    ("seed", Kind::Int),
    ("season", Kind::Text),
];

#[derive(Clone, Copy)]
enum Kind {
    Text,
    Int,
    Float,
}

/// The conditions validated as their models, in the order Python reads them.
const CONDITIONS: [&str; 5] = ["facility_zones", "operational_conditions", "population_conditions", "evacuation_selection", "zone_area_shares"];

fn read_contract(answer: &V) -> Result<Read, Refusal> {
    let V::Map(items) = answer else {
        return Ok(Read::Refused);
    };
    let mut parsed = Values::new();
    for (field, kind) in SCALARS {
        let raw = items.get(field).unwrap_or(&V::Null);
        if matches!(raw, V::Null) || matches!(raw, V::Str(text) if text.is_empty()) {
            continue;
        }
        let V::Str(text) = raw else {
            return Ok(Read::Refused);
        };
        let value = match kind {
            Kind::Text => Some(V::Str(text.clone())),
            Kind::Int => py_int(text),
            Kind::Float => py_float(text).map(V::Float),
        };
        let Some(value) = value else {
            return Ok(Read::Refused);
        };
        parsed.insert(field, value);
    }
    if let Some(conditions) = items.get("facility_conditions").filter(|value| value.is_truthy()) {
        match conditions {
            V::List(list) if list.iter().all(|item| matches!(item, V::Str(_))) => {
                parsed.insert("facility_conditions", conditions.clone());
            }
            _ => return Ok(Read::Refused),
        }
    }
    // Each condition is validated as the request validates it (its model, in a request otherwise
    // at its defaults). Python reads them in order and the first it refuses refuses the answer; a
    // valid condition the browser does not compute is unsupported, unless a later one is refused.
    let mut unsupported = None;
    for field in CONDITIONS {
        let Some(value) = items.get(field).filter(|value| value.is_truthy()) else {
            continue;
        };
        if field == "zone_area_shares" {
            let V::Map(shares) = value else {
                return Ok(Read::Refused);
            };
            if shares.values().any(|weight| !matches!(weight, V::Int(_) | V::Big(_) | V::Float(_))) {
                return Ok(Read::Refused);
            }
        }
        match validated(field, value) {
            Ok(dumped) => {
                parsed.insert(field, dumped);
            }
            Err(Refusal::Invalid(_)) => return Ok(Read::Refused),
            Err(Refusal::Unsupported(why)) => {
                unsupported.get_or_insert(why);
            }
        }
    }
    if let Some(why) = unsupported {
        return Err(Refusal::Unsupported(format!("the AI's {why}")));
    }
    Ok(Read::Values(parsed))
}

/// A request at its defaults with `field` set to `value`.
fn probe(field: Option<(&str, &V)>) -> Result<InitialScenarioRequest, Refusal> {
    let mut map: IndexMap<Arc<str>, V> = IndexMap::new();
    map.insert(Arc::from("municipality"), V::Str(Arc::from("probe")));
    map.insert(Arc::from("disaster"), V::Str(Arc::from("earthquake")));
    map.insert(Arc::from("evacuees"), V::Int(1));
    if let Some((field, value)) = field {
        map.insert(Arc::from(field), value.clone());
    }
    InitialScenarioRequest::of_value(&V::Map(map))
}

/// A condition as its model validates and dumps it: `Invalid` where Python's model refuses it,
/// `Unsupported` where the browser does not compute it. Only the operational conditions may differ
/// from their default (in the initial water and food); the others are the default's dump.
fn validated(field: &str, value: &V) -> Result<V, Refusal> {
    let dumped = |request: &InitialScenarioRequest| member(&request.dump(), field).cloned().unwrap_or(V::Null);
    let request = probe(Some((field, value)))?;
    let read = dumped(&request);
    if field != "operational_conditions" && read != dumped(&probe(None)?) {
        return Err(Refusal::Unsupported(field.to_string()));
    }
    Ok(read)
}

/// Python's `int(text)`: white space around it, a sign, decimal digits of any script with single
/// underscores between them; None where Python raises.
fn py_int(text: &str) -> Option<V> {
    let ascii = ascii_number(text)?;
    let (negative, digits) = match ascii.as_bytes().first()? {
        b'-' => (true, &ascii[1..]),
        b'+' => (false, &ascii[1..]),
        _ => (false, &ascii[..]),
    };
    if !underscores_between_digits(digits) || digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit() || byte == b'_') {
        return None;
    }
    let digits: String = digits.chars().filter(|char| *char != '_').collect();
    let trimmed = digits.trim_start_matches('0');
    let magnitude = if trimmed.is_empty() { "0" } else { trimmed };
    let signed = if negative && magnitude != "0" { format!("-{magnitude}") } else { magnitude.to_string() };
    Some(match signed.parse::<i64>() {
        Ok(number) => V::Int(number),
        Err(_) => V::Big(signed.into()),
    })
}

/// Python's `float(text)`; None where Python raises.
fn py_float(text: &str) -> Option<f64> {
    let ascii = ascii_number(text)?;
    let body = ascii.strip_prefix(['+', '-']).unwrap_or(&ascii);
    let word = body.to_ascii_lowercase();
    if ["inf", "infinity", "nan"].contains(&word.as_str()) {
        return ascii.parse::<f64>().ok();
    }
    if !underscores_between_digits(body) || !body.bytes().all(|byte| byte.is_ascii_digit() || matches!(byte, b'_' | b'.' | b'e' | b'E' | b'+' | b'-')) {
        return None;
    }
    let plain: String = ascii.chars().filter(|char| *char != '_').collect();
    plain.parse::<f64>().ok()
}

/// The text as Python's `int` and `float` read it: each decimal digit of any script as its ASCII
/// digit, white space around it taken off; None when another non-ASCII character is left.
fn ascii_number(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    for char in text.chars() {
        if re_space(char) {
            out.push(' ');
        } else if char.is_ascii() {
            out.push(char);
        } else if re_decimal(char) {
            out.push(char::from(b'0' + decimal_value(char)?));
        } else {
            return None;
        }
    }
    let trimmed = out.trim_matches(' ');
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// The value of a decimal digit (Unicode's decimal digits come in runs of ten, from 0 to 9).
fn decimal_value(char: char) -> Option<u8> {
    let point = u32::from(char);
    let mut start = point;
    while start > 0 && char::from_u32(start - 1).is_some_and(re_decimal) {
        start -= 1;
    }
    u8::try_from((point - start) % 10).ok()
}

/// Whether each underscore stands between two digits.
fn underscores_between_digits(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes
        .iter()
        .enumerate()
        .all(|(index, byte)| *byte != b'_' || (index > 0 && index + 1 < bytes.len() && bytes[index - 1].is_ascii_digit() && bytes[index + 1].is_ascii_digit()))
}

// ---- `_merge_explicit` --------------------------------------------------------------------------

/// The rules' values with what the answer read back adds, and the conditions read at their
/// defaults (their origin only; the request's own defaults hold them).
fn merge_explicit(explicit: &Values, parsed: Values) -> Result<(Values, Vec<&'static str>), Refusal> {
    let mut merged = explicit.clone();
    let mut extracted = Vec::new();
    for (key, value) in parsed {
        match key {
            "operational_conditions" => {
                let combined = match explicit.get(key) {
                    // The rules' conditions as their model dumps them, then merged as Python merges.
                    Some(rules) => merge_operational(&validated(key, rules)?, &value),
                    None => value,
                };
                merged.insert(key, combined);
            }
            "population_conditions" | "evacuation_selection" | "facility_zones" | "zone_area_shares" => {
                // At their defaults (`validated`): the request's defaults, so only the origin.
                extracted.push(key);
            }
            _ => {
                if !merged.contains_key(key) && value != V::Null {
                    merged.insert(key, value);
                }
            }
        }
    }
    Ok((merged, extracted))
}

/// `_merge_operational_conditions(explicit, extracted)`.
fn merge_operational(explicit: &V, extracted: &V) -> V {
    let mut merged = merge_object(explicit, extracted);
    let stock = |value: &V| member(value, "initial_resources").cloned();
    if let (Some(left), Some(right), V::Map(map)) = (stock(explicit), stock(extracted), &mut merged) {
        if let (V::Map(_), V::Map(_)) = (&left, &right) {
            let mut combined = merge_object(&left, &right);
            let pick = |key: &str| match member(&left, key) {
                Some(V::Null) | None => member(&right, key).cloned().unwrap_or(V::Null),
                Some(value) => value.clone(),
            };
            let water = pick("water_ml");
            let food = pick("food_meal_units");
            if let V::Map(stock) = &mut combined {
                let both = water != V::Null && food != V::Null;
                let either = water != V::Null || food != V::Null;
                stock.insert(Arc::from("water_ml"), water);
                stock.insert(Arc::from("food_meal_units"), food);
                if both {
                    stock.insert(Arc::from("status"), V::Str(Arc::from("entered")));
                } else if either {
                    stock.insert(Arc::from("status"), V::Str(Arc::from("incomplete")));
                }
                let mut ids: Vec<Arc<str>> = [&left, &right]
                    .iter()
                    .filter_map(|side| match member(side, "source_ids") {
                        Some(V::List(ids)) => Some(ids.iter().filter_map(|id| if let V::Str(id) = id { Some(id.clone()) } else { None }).collect::<Vec<_>>()),
                        _ => None,
                    })
                    .flatten()
                    .collect();
                ids.sort();
                ids.dedup();
                stock.insert(Arc::from("source_ids"), V::List(ids.into_iter().map(V::Str).collect()));
            }
            map.insert(Arc::from("initial_resources"), combined);
        }
    }
    if let V::Map(map) = &mut merged {
        for key in ["equipment", "scheduled"] {
            if let (Some(V::List(left)), Some(V::List(right))) = (member(explicit, key), member(extracted, key)) {
                let list = if key == "equipment" {
                    let mut by_id: IndexMap<String, V> = IndexMap::new();
                    for item in right.iter().chain(left) {
                        if let Some(V::Str(id)) = member(item, "facility_id") {
                            by_id.insert(id.to_string(), item.clone());
                        }
                    }
                    by_id.sort_keys();
                    by_id.into_values().collect()
                } else {
                    left.iter().chain(right).cloned().collect()
                };
                map.insert(Arc::from(key), V::List(list));
            }
        }
    }
    merged
}

/// `_merge_condition_object(explicit, extracted)`.
fn merge_object(explicit: &V, extracted: &V) -> V {
    let (V::Map(left), V::Map(right)) = (explicit, extracted) else {
        return extracted.clone();
    };
    let mut merged = right.clone();
    for (key, value) in left {
        let empty = matches!(value, V::Null)
            || matches!(value, V::Str(text) if text.is_empty())
            || matches!(value, V::List(items) if items.is_empty())
            || matches!(value, V::Map(items) if items.is_empty());
        match (value, merged.get(key)) {
            (V::Map(_), Some(V::Map(_))) => {
                let inner = merge_object(value, &merged[key]);
                merged.insert(key.clone(), inner);
            }
            (V::List(items), Some(V::List(_))) if !items.is_empty() => {
                merged.insert(key.clone(), value.clone());
            }
            _ if !empty => {
                merged.insert(key.clone(), value.clone());
            }
            _ => {
                if !merged.contains_key(key) {
                    merged.insert(key.clone(), value.clone());
                }
            }
        }
    }
    V::Map(merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_schema_s_patterns_are_read() {
        fn patterns(value: &V, out: &mut Vec<String>) {
            match value {
                V::Map(map) => {
                    for (key, item) in map {
                        match (&**key, item) {
                            ("pattern", V::Str(text)) => out.push(text.to_string()),
                            _ => patterns(item, out),
                        }
                    }
                }
                V::List(items) => items.iter().for_each(|item| patterns(item, out)),
                _ => {}
            }
        }
        let mut found = Vec::new();
        patterns(task(), &mut found);
        assert!(!found.is_empty());
        for pattern in found {
            assert!(Pattern::of(&pattern).is_some(), "{pattern}");
        }
        let id = Pattern::of("^[A-Za-z0-9][A-Za-z0-9._:-]{0,63}$").unwrap();
        assert!(id.searches("main_warehouse") && id.searches("a\n") && !id.searches("-x") && !id.searches(""));
        assert!(!id.searches(&"a".repeat(65)) && id.searches(&"a".repeat(64)));
    }

    #[test]
    fn numbers_are_read_as_python_s_int_and_float_read_them() {
        assert_eq!(py_int(" 1_000 "), Some(V::Int(1000)));
        assert_eq!(py_int("１００"), Some(V::Int(100)));
        assert_eq!(py_int("٣"), Some(V::Int(3)));
        assert_eq!(py_int("-0"), Some(V::Int(0)));
        assert_eq!(py_int("99999999999999999999"), Some(V::Big("99999999999999999999".into())));
        for refused in ["", " ", "1__0", "_1", "1_", "1.0", "1e3", "0x10", "many", "2日"] {
            assert_eq!(py_int(refused), None, "{refused:?}");
        }
        assert_eq!(py_float("1e1"), Some(10.0));
        assert_eq!(py_float("12_3.5"), Some(123.5));
        assert_eq!(py_float(" 2.50 "), Some(2.5));
        assert_eq!(py_float(".5"), Some(0.5));
        assert_eq!(py_float("5."), Some(5.0));
        assert!(py_float("nan").is_some_and(f64::is_nan));
        assert_eq!(py_float("-Infinity"), Some(f64::NEG_INFINITY));
        for refused in ["", "1_.5", "1._5", "e5", "1e", ".", "2日", "1,000"] {
            assert_eq!(py_float(refused), None, "{refused:?}");
        }
    }
}
