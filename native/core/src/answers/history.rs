//! `GET …/branches/{branch}/history?kind=kpis&start=&end=&step=&limit=&cursor=` (#266): a
//! branch's KPI history from its stored metric samples (`history::KpiTable`), paged as the history
//! index of the Python server paged it before #430.
//!
//! Two steps, so that the samples are read only for a query the server takes: `HistoryPlan::new`
//! checks the query, the branch's record, the range, the kind and the cursor, in the server's
//! order; the caller then reads the branch's samples (where the branch has a head,
//! `HistoryPlan::anchor`) into a `KpiTable`, and `HistoryPlan::answer` writes the page.
//!
//! Where the browser differs from the server: `anchor_hash` and the cursor's anchor are the
//! branch's head (its record's `checkpoint_hash`), `first_indexed_checkpoint_minute` is its fork
//! minute, only `kind=kpis` is answered (any other kind: `browser_unsupported`), a number of the
//! query takes ASCII digits only, and a cursor's minute written as 5.0 is taken as 5.

use serde_json::Value;

use super::query::Query;
use super::records::{is_id, Record, RunFiles};
use super::{code_points, js_integer, js_space, sha256_hex, string, Answer, Failure, Obj, Refusal};
use crate::history::{KpiQuery, KpiTable};

// The kinds a history query (`HistoryQuery`) takes.
const KINDS: [&str; 13] =
    ["people", "households", "events", "causal", "kpis", "conversations", "social", "staff", "actions", "transitions", "dialogue", "care", "operations"];
const SELECTORS: [(&str, &[&str]); 5] = [
    ("agent_id", &["people", "events", "causal", "social", "actions", "transitions", "dialogue", "care"]),
    ("household_id", &["households", "events", "actions", "transitions", "dialogue", "care"]),
    ("staff_id", &["staff", "actions", "dialogue", "operations"]),
    ("conversation_id", &["conversations"]),
    ("event_id", &["causal", "events", "social", "staff", "actions", "transitions", "dialogue", "care", "operations"]),
];
const IDENTIFIERS: [&str; 9] = ["agent_id", "household_id", "event_id", "staff_id", "conversation_id", "zone_id", "event_type", "source_id", "status"];
const FIELDS: [&str; 8] = ["kind", "start", "end", "step", "limit", "cursor", "text", "include_related"];
const MAX_SAFE: i128 = 9_007_199_254_740_991;

/// An int of Python, its value kept to i128 (a larger one is saturated: no check tells them apart)
/// and its digits as `str(int)` writes them (the cursor's scope holds them).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PyInt {
    pub value: i128,
    pub text: String,
}

impl PyInt {
    fn of(value: i128) -> Self {
        PyInt { value, text: value.to_string() }
    }
}

/// `int(text)` of Python: spaces around, a sign, decimal digits with single underscores between
/// them; else the server's 400. ASCII digits only.
fn python_int(text: &str) -> Result<PyInt, Refusal> {
    let trimmed = text.trim_matches(js_space);
    let (negative, digits) = match trimmed.as_bytes().first() {
        Some(b'+') => (false, &trimmed[1..]),
        Some(b'-') => (true, &trimmed[1..]),
        _ => (false, trimmed),
    };
    let bytes = digits.as_bytes();
    let valid = !bytes.is_empty()
        && bytes[0].is_ascii_digit()
        && bytes[bytes.len() - 1].is_ascii_digit()
        && bytes.iter().all(|&byte| byte.is_ascii_digit() || byte == b'_')
        && !digits.contains("__");
    if !valid {
        return Err(Refusal::invalid_input());
    }
    let plain: String = digits.chars().filter(|c| *c != '_').collect();
    let stripped = plain.trim_start_matches('0');
    let magnitude = if stripped.is_empty() { "0" } else { stripped };
    let value = magnitude.parse::<i128>().unwrap_or(i128::MAX);
    let text = if negative && magnitude != "0" { format!("-{magnitude}") } else { magnitude.to_owned() };
    Ok(PyInt { value: if negative { -value } else { value }, text })
}

/// pydantic's bool of a str (lax mode).
fn pydantic_bool(text: &str) -> Result<bool, Refusal> {
    match text.to_lowercase().as_str() {
        "1" | "on" | "t" | "true" | "y" | "yes" => Ok(true),
        "0" | "off" | "f" | "false" | "n" | "no" => Ok(false),
        _ => Err(Refusal::invalid_input()),
    }
}

/// A history query as the server checks it (`one_query`, the ints, `HistoryQuery.model_validate`).
#[derive(Clone, Debug)]
pub struct HistoryQuery {
    pub kind: String,
    pub start: PyInt,
    pub end: PyInt,
    pub step: PyInt,
    pub limit: PyInt,
    pub cursor: Option<String>,
    pub text: Option<String>,
    pub include_related: bool,
    /// Each of `IDENTIFIERS`, in their order.
    pub identifiers: Vec<(&'static str, Option<String>)>,
}

impl HistoryQuery {
    pub fn parse(query: &str) -> Result<HistoryQuery, Refusal> {
        let fields = Query::parse(query);
        for (name, values) in &fields.fields {
            let known = FIELDS.contains(&name.as_str()) || IDENTIFIERS.contains(&name.as_str());
            if !known || values.len() != 1 {
                return Err(Refusal::storage("invalid_query_fields"));
            }
        }
        let given = |name: &str| fields.one(name).map(str::to_owned);
        let int = |name: &str, default: Option<i128>| -> Result<Option<PyInt>, Refusal> {
            match fields.one(name) {
                Some(text) => python_int(text).map(Some),
                None => Ok(default.map(PyInt::of)),
            }
        };
        let start = int("start", Some(0))?;
        let end = int("end", None)?;
        let step = int("step", Some(1))?;
        let limit = int("limit", Some(25))?;
        let include_related = match fields.one("include_related") {
            Some(text) => pydantic_bool(text)?,
            None => false,
        };
        let identifiers: Vec<(&'static str, Option<String>)> = IDENTIFIERS.iter().map(|name| (*name, given(name))).collect();
        let kind = given("kind");
        let (Some(kind), Some(end), Some(start), Some(step), Some(limit)) = (kind, end, start, step, limit) else {
            return Err(Refusal::invalid_input());
        };
        if !KINDS.contains(&kind.as_str()) {
            return Err(Refusal::invalid_input());
        }
        let query = HistoryQuery { kind, start, end, step, limit, cursor: given("cursor"), text: given("text"), include_related, identifiers };
        query.check()?;
        Ok(query)
    }

    fn identifier(&self, name: &str) -> Option<&str> {
        self.identifiers.iter().find(|(each, _)| *each == name).and_then(|(_, value)| value.as_deref())
    }

    fn check(&self) -> Result<(), Refusal> {
        let invalid = Refusal::invalid_input;
        if self.start.value < 0 || self.end.value < 0 || self.step.value < 1 || self.limit.value < 1 || self.limit.value > 100 {
            return Err(invalid());
        }
        if self.cursor.as_deref().is_some_and(|cursor| code_points(cursor) > 2048) {
            return Err(invalid());
        }
        if self.text.as_deref().is_some_and(|text| !(1..=160).contains(&code_points(text))) {
            return Err(invalid());
        }
        if self.identifiers.iter().any(|(_, value)| value.as_deref().is_some_and(|value| !is_id(value))) {
            return Err(invalid());
        }
        for (selector, kinds) in SELECTORS {
            if self.identifier(selector).is_some() && !kinds.contains(&self.kind.as_str()) {
                return Err(invalid());
            }
        }
        if self.include_related && self.kind != "causal" {
            return Err(invalid());
        }
        if self.start.value > self.end.value {
            return Err(invalid());
        }
        let needs = [("people", "agent_id"), ("households", "household_id"), ("causal", "event_id")];
        if needs.iter().any(|(kind, selector)| self.kind == *kind && self.identifier(selector).is_none()) {
            return Err(invalid());
        }
        Ok(())
    }

    /// Whether a KPI row could pass the query's filters: the index has no identity, type, source,
    /// status or text term of a KPI row, so any of them leaves no rows.
    fn filtered(&self) -> bool {
        // The text's terms: `re.findall(r"[\w:-]{1,80}", text.casefold())`; Python's \w is letters,
        // numbers and _.
        let terms = self.text.as_deref().is_some_and(|text| text.chars().any(|c| c == '_' || c == ':' || c == '-' || letter_or_number(c)));
        terms || ["zone_id", "event_type", "source_id", "status"].iter().any(|name| self.identifier(name).is_some())
    }

    /// The `scope` a cursor holds: the digest of the branch's ids and the query but its cursor and
    /// limit, so a cursor pages only the query it came from (#278).
    fn scope(&self, project_id: &str, run_id: &str, branch_id: &str) -> String {
        let mut scope = Obj::new();
        scope.str("project", project_id).str("run", run_id).str("branch", branch_id).int("schema_version", 1);
        scope.str("kind", &self.kind).raw("start", self.start.text.clone()).raw("end", self.end.text.clone()).raw("step", self.step.text.clone());
        scope.opt_str("text", self.text.as_deref()).bool("include_related", self.include_related);
        for (name, value) in &self.identifiers {
            scope.opt_str(name, value.as_deref());
        }
        sha256_hex(&scope.finish())
    }
}

/// Whether a character is of Unicode's general categories L or N (JS's `\p{L}` or `\p{N}`).
fn letter_or_number(c: char) -> bool {
    use std::sync::OnceLock;
    static PATTERN: OnceLock<fancy_regex::Regex> = OnceLock::new();
    let pattern = PATTERN.get_or_init(|| fancy_regex::Regex::new(r"^[\p{L}\p{N}]$").expect("the pattern is valid"));
    let mut buffer = [0; 4];
    pattern.is_match(c.encode_utf8(&mut buffer)).unwrap_or(false)
}

// ---- The cursor ------------------------------------------------------------------------------

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// `base64.urlsafe_b64encode`: the standard alphabet with "-" and "_", padded.
fn base64_url(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let triple = (u32::from(chunk[0]) << 16) | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8) | u32::from(*chunk.get(2).unwrap_or(&0));
        for index in 0..4 {
            if index <= chunk.len() {
                let digit = ALPHABET[((triple >> (18 - 6 * index)) & 63) as usize];
                out.push(match digit {
                    b'+' => '-',
                    b'/' => '_',
                    other => other as char,
                });
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// `base64.b64decode(value, altchars=b"-_", validate=True)` as the browser checks it: both
/// alphabets, padded to a multiple of 4, at most two "=" at the end; None otherwise.
fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    if bytes.len() % 4 != 0 {
        return None;
    }
    let body = text.trim_end_matches('=');
    if bytes.len() - body.len() > 2 || body.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(body.len() * 3 / 4);
    let (mut buffer, mut bits) = (0u32, 0);
    for byte in body.bytes() {
        let digit = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(digit);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// The cursor's row for this scope and anchor: its minute and the key after `kpis:`, or the
/// server's refusal.
fn cursor_row(cursor: &str, scope: &str, anchor: Option<&str>) -> Result<(i64, String), Refusal> {
    let stale = || Refusal::storage("stale_or_foreign_cursor: restart this query");
    let data = base64_decode(cursor).ok_or_else(stale)?;
    let text = std::str::from_utf8(&data).map_err(|_| stale())?;
    let Ok(Value::Object(data)) = serde_json::from_str::<Value>(text) else {
        return Err(stale());
    };
    let keys: Vec<&str> = data.keys().map(String::as_str).collect();
    if keys != ["anchor", "key", "minute", "scope"] || data["scope"].as_str() != Some(scope) {
        return Err(stale());
    }
    let same_anchor = match (&data["anchor"], anchor) {
        (Value::Null, None) => true,
        (Value::String(kept), Some(anchor)) => kept == anchor,
        _ => false,
    };
    let minute = js_integer(&data["minute"]);
    let key = data["key"].as_str();
    let (true, Some(minute), Some(key)) = (same_anchor, minute, key) else {
        return Err(stale());
    };
    if code_points(key) > 128 {
        return Err(stale());
    }
    match key.split_once(':') {
        Some(("kpis", rest)) => Ok((minute as i64, rest.to_owned())),
        _ => Err(Refusal::storage("stale_or_foreign_cursor")),
    }
}

// ---- The answer ------------------------------------------------------------------------------

/// A history query the server takes, of a branch (step 1).
#[derive(Clone, Debug)]
pub struct HistoryPlan {
    query: HistoryQuery,
    record: Record,
    run_id: String,
    branch_id: String,
    anchor: Option<String>,
    scope: String,
    after: Option<(i64, String)>,
}

impl HistoryPlan {
    /// Checks a history request: the query (the text after "?"), the ids, the branch's record
    /// (`branch.json`, None where missing), the range, the kind and the cursor.
    pub fn new(query: &str, project_id: &str, run_id: &str, branch_id: &str, branch: Option<&[u8]>) -> Answer<HistoryPlan> {
        let query = HistoryQuery::parse(query)?;
        let files = RunFiles { run: None, branches: branch.map(|bytes| vec![(branch_id.to_owned(), bytes.to_vec())]).unwrap_or_default() };
        let record = files.branch_record(project_id, run_id, branch_id)?;
        let at = |key: &str| record.get(key).and_then(js_integer).map(|value| value as i128);
        let (fork, until) = (at("fork_minute"), at("computed_until"));
        let within = matches!((fork, until), (Some(fork), Some(until)) if fork <= query.start.value && query.end.value <= until);
        if !within {
            return Err(Refusal::storage("history_outside_computed_range").into());
        }
        if query.kind != "kpis" {
            let text = format!("browser_unsupported: history of kind {} is not answered by the browser version", query.kind);
            return Err(Refusal::storage(text).into());
        }
        let anchor = record.get("checkpoint_hash").and_then(Value::as_str).map(str::to_owned);
        let scope = query.scope(project_id, run_id, branch_id);
        let after = match &query.cursor {
            Some(cursor) => Some(cursor_row(cursor, &scope, anchor.as_deref())?),
            None => None,
        };
        Ok(HistoryPlan { query, record, run_id: run_id.to_owned(), branch_id: branch_id.to_owned(), anchor, scope, after })
    }

    /// The branch's head, where it has one: the samples to read are its stored Run's.
    pub fn anchor(&self) -> Option<&str> {
        self.anchor.as_deref()
    }

    /// The answer, from the branch's samples read back (an empty table for a branch without a
    /// head).
    pub fn answer(&self, table: &KpiTable) -> Answer {
        let start = self.query.start.value as i64;
        let end = self.query.end.value as i64;
        let (mut items, mut next, mut coverage) = (b"[]".to_vec(), None, None);
        if self.anchor.is_some() {
            let step = self.query.step.value.min(MAX_SAFE) as i64;
            let after = self.after.as_ref().map(|(minute, key)| (*minute, key.as_str()));
            let query = KpiQuery {
                start,
                end,
                step,
                limit: self.query.limit.value as usize,
                after,
                filtered: self.query.filtered(),
                run_id: &self.run_id,
                branch_id: &self.branch_id,
            };
            let page = table.page(&query).map_err(Failure::from)?;
            items = page.items;
            if page.more {
                let minute = page.last.unwrap_or(0);
                let mut cursor = Obj::new();
                cursor.str("scope", &self.scope).opt_str("anchor", self.anchor.as_deref()).int("minute", minute).str("key", &format!("kpis:{minute}"));
                next = Some(base64_url(&cursor.finish()));
            }
            coverage = table.coverage(start, end);
        }
        let integer = |key: &str| self.record.get(key).and_then(super::js_safe_integer).ok_or_else(not_an_integer);
        let computed_until = integer("computed_until")?;
        let mut answer = Obj::new();
        answer.int("schema_version", 1).raw("items", items).opt_str("next_cursor", next.as_deref()).opt_str("anchor_hash", self.anchor.as_deref());
        match &self.anchor {
            Some(_) => answer.int("indexed_through_minute", computed_until),
            None => answer.null("indexed_through_minute"),
        };
        answer.null("total_items").str("count_status", "not_calculated").str("query_strategy", "indexed_keyset");
        answer.str("coverage", "persisted_index_records").str("coverage_scope", "branch_kind_time_window_before_identity_and_text_filters");
        answer
            .str("completeness", "unknown")
            .str("completeness_reason", "The index exposes persisted producer records but cannot certify that every domain event was produced or retained.");
        match coverage {
            Some((count, oldest, newest)) => {
                answer.int("oldest_minute", oldest).int("newest_minute", newest);
                let mut kpis = Obj::new();
                kpis.int("oldest_minute", oldest).int("newest_minute", newest).int("indexed_records", count as i64);
                let mut by_kind = Obj::new();
                by_kind.object("kpis", kpis);
                answer.object("coverage_by_kind", by_kind);
            }
            None => {
                answer.null("oldest_minute").null("newest_minute").raw("coverage_by_kind", b"{}".to_vec());
            }
        }
        answer
            .null("retention_limit")
            .str("retention_limit_reason", "No per-kind source eviction boundary is persisted; absence of records is not evidence that no event occurred.");
        answer.raw("requested_range", format!("[{start},{end}]").into_bytes());
        match &self.anchor {
            Some(_) => answer.int("first_indexed_checkpoint_minute", integer("fork_minute")?),
            None => answer.null("first_indexed_checkpoint_minute"),
        };
        answer.bool("complete_lifetime_history", false).str("causal_interpretation", "Recorded model links, not empirical causal proof");
        answer.raw("run_id", string(&self.run_id)).raw("branch_id", string(&self.branch_id)).str("kind", "kpis");
        answer.int("computed_until", computed_until).str("time_semantics", "end_of_tick");
        Ok(answer.finish())
    }
}

fn not_an_integer() -> Failure {
    Failure::Error(crate::error::Error::Value("not an integer of an answer".to_owned()))
}
