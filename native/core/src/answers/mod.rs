//! The answers of the API's Project routes (`/api/v1/projects/...`, Issue #449, decision 1 of
//! #368): the JSON the screen `/demo-v2` reads, and each refusal with its HTTP status, text and
//! code, as pure functions of what a store holds. The browser calls them through WASM
//! (`native/wasm`, js/browser_api.mjs routes the requests), the local and VM versions through
//! PyO3 (`native/src/rules/answers.rs`), so both answer the same bytes.
//!
//! What stays outside: reading the files (OPFS, the file system), decompressing the pieces of a
//! stored Run, locks, routing a request to its answer, and computing. The caller reads the
//! records a route needs (`records::RunFiles` and the like: a file's bytes, or None where it is
//! missing) and hands them over; a function checks what the server checks, in its order, and
//! writes the answer.
//!
//! An answer is JSON bytes: keys and values as the Python server wrote them before #449 (sorted
//! keys, no spaces) where the browser always did so, and as `JSON.stringify` of the records where
//! the browser gave its records as they are. A refusal is a `Refusal`: the server's `StorageError`
//! is 409 with the leading word of its text as its code (or `storage_conflict`), a request it
//! cannot read 400. Anything else is the core's `Error`: the server's 500, but a stored Run of an
//! older format, 409 `stored_run_outdated` (`Failure::from`).

pub mod asking;
pub mod frames;
pub mod history;
pub mod interventions;
pub mod pdf;
pub mod query;
pub mod questions;
pub mod records;
pub mod report;
pub mod routes;

use std::fmt;

use serde_json::{Map, Value};

use crate::error::Error;
use crate::json::write_json_string;
use crate::run_store::OUTDATED_CODE;

/// A refusal of a request: its HTTP status, its text (`error`) and its code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub status: u16,
    pub error: String,
    pub code: Option<String>,
}

impl Refusal {
    pub fn new(status: u16, error: impl Into<String>, code: Option<&str>) -> Self {
        Refusal { status, error: error.into(), code: code.map(str::to_owned) }
    }

    /// The server's `StorageError(message)`: 409, its code the message's leading word where it is
    /// one, else `storage_conflict` (src/sheltersim/storage/projects.py).
    pub fn storage(message: impl Into<String>) -> Self {
        let error = message.into();
        let code = storage_code(&error);
        Refusal { status: 409, error, code: Some(code) }
    }

    /// A `StorageError` with its own code.
    pub fn storage_with(message: impl Into<String>, code: &str) -> Self {
        Refusal { status: 409, error: message.into(), code: Some(code.to_owned()) }
    }

    /// The server's 400 for a request body it cannot read: `{"error"}` alone.
    pub fn invalid_request() -> Self {
        Refusal::new(400, "invalid project request", None)
    }

    /// The server's 400 for a query its contracts refuse (a pydantic ValidationError or a bad int).
    pub fn invalid_input() -> Self {
        Refusal::new(400, "invalid query or saved contract", Some("invalid_input"))
    }

    /// The body the server answers it with: `{"code","error"}`, or `{"error"}` without a code.
    pub fn body(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.error.len() + 40);
        out.push(b'{');
        if let Some(code) = &self.code {
            out.extend_from_slice(b"\"code\":");
            write_json_string(&mut out, code);
            out.push(b',');
        }
        out.extend_from_slice(b"\"error\":");
        write_json_string(&mut out, &self.error);
        out.push(b'}');
        out
    }

    /// `{"code","error","status"}`: how a refusal crosses to the browser in an error's message.
    pub fn to_json(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.error.len() + 60);
        out.extend_from_slice(b"{\"code\":");
        match &self.code {
            Some(code) => write_json_string(&mut out, code),
            None => out.extend_from_slice(b"null"),
        }
        out.extend_from_slice(b",\"error\":");
        write_json_string(&mut out, &self.error);
        out.extend_from_slice(b",\"status\":");
        out.extend_from_slice(self.status.to_string().as_bytes());
        out.push(b'}');
        out
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.status, self.error)
    }
}

/// StorageError's code: the message's leading word (up to its first ":") where it is one, else
/// `storage_conflict`.
pub fn storage_code(message: &str) -> String {
    let candidate = message.split(':').next().unwrap_or("");
    let bytes = candidate.as_bytes();
    let word = (2..=81).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes[1..].iter().all(|&byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
    if word {
        candidate.to_owned()
    } else {
        "storage_conflict".to_owned()
    }
}

/// Why an answer was not made: a refusal, or an error of the core (the server's 500).
#[derive(Clone, Debug, PartialEq)]
pub enum Failure {
    Refused(Refusal),
    Error(Error),
}

impl From<Refusal> for Failure {
    fn from(refusal: Refusal) -> Self {
        Failure::Refused(refusal)
    }
}

impl From<Error> for Failure {
    /// A stored Run of an older format is refused, wherever it was read (409
    /// `stored_run_outdated`); any other error stays one.
    fn from(error: Error) -> Self {
        if let Error::Value(text) = &error {
            if text.strip_prefix(OUTDATED_CODE).is_some_and(|rest| rest.starts_with(": ")) {
                return Failure::Refused(Refusal::storage_with(text.clone(), OUTDATED_CODE));
            }
        }
        Failure::Error(error)
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::Refused(refusal) => refusal.fmt(f),
            Failure::Error(error) => error.fmt(f),
        }
    }
}

/// What an answer's function returns.
pub type Answer<T = Vec<u8>> = std::result::Result<T, Failure>;

// ---- JSON as the browser wrote it ----------------------------------------------------------------

/// `JSON.stringify` of a value `JSON.parse` read from JSON with sorted keys: keys in their order
/// (serde_json's, by bytes: the order of a record written with sorted keys), strings as Python's
/// `ensure_ascii=False` writes them (the same escapes), numbers as they were written.
pub fn write_value(out: &mut Vec<u8>, value: &Value) {
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(flag) => out.extend_from_slice(if *flag { b"true" } else { b"false" }),
        Value::Number(number) => out.extend_from_slice(number.to_string().as_bytes()),
        Value::String(text) => write_json_string(out, text),
        Value::Array(items) => {
            out.push(b'[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                write_value(out, item);
            }
            out.push(b']');
        }
        Value::Object(object) => write_object(out, object),
    }
}

pub fn write_object(out: &mut Vec<u8>, object: &Map<String, Value>) {
    out.push(b'{');
    for (index, (key, item)) in object.iter().enumerate() {
        if index > 0 {
            out.push(b',');
        }
        write_json_string(out, key);
        out.push(b':');
        write_value(out, item);
    }
    out.push(b'}');
}

/// The number a JSON number is to JS (`Number(text)`), or None for another value.
pub fn js_number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.to_string().parse::<f64>().ok(),
        _ => None,
    }
}

/// `Number.isInteger(value)` of a JSON value: the integer, or None.
pub fn js_integer(value: &Value) -> Option<f64> {
    js_number(value).filter(|number| number.is_finite() && number.fract() == 0.0)
}

/// `Number.isSafeInteger(value)` of a JSON value: the integer, or None.
pub fn js_safe_integer(value: &Value) -> Option<i64> {
    js_integer(value).filter(|number| number.abs() <= 9_007_199_254_740_991.0).map(|number| number as i64)
}

/// Whether JS's `trim` removes the character (and `\s` takes it): its WhiteSpace and LineTerminator.
pub fn js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
    )
}

/// JS's `text.trimEnd()`.
pub fn js_trim_end(text: &str) -> &str {
    text.trim_end_matches(js_space)
}

/// JS's `text.trim()`.
pub fn js_trim(text: &str) -> &str {
    text.trim_matches(js_space)
}

/// The text of a file read with a decoder that refuses bytes that are not UTF-8 (the browser's
/// `TextDecoder("utf-8", { fatal: true })`): an error of the core otherwise.
pub fn utf8(bytes: &[u8]) -> Answer<&str> {
    std::str::from_utf8(bytes).map_err(|_| Failure::Error(Error::Type("The encoded data was not valid for encoding utf-8".to_owned())))
}

/// A JSON object written with its keys sorted (Python's `json.dumps(sort_keys=True)`): each field
/// its key and its value's JSON text.
#[derive(Default)]
pub struct Obj {
    fields: Vec<(String, Vec<u8>)>,
}

impl Obj {
    pub fn new() -> Self {
        Obj::default()
    }

    /// A field of JSON text written already.
    pub fn raw(&mut self, key: &str, json: impl Into<Vec<u8>>) -> &mut Self {
        self.fields.push((key.to_owned(), json.into()));
        self
    }

    pub fn str(&mut self, key: &str, text: &str) -> &mut Self {
        let mut out = Vec::with_capacity(text.len() + 2);
        write_json_string(&mut out, text);
        self.raw(key, out)
    }

    pub fn opt_str(&mut self, key: &str, text: Option<&str>) -> &mut Self {
        match text {
            Some(text) => self.str(key, text),
            None => self.raw(key, b"null".to_vec()),
        }
    }

    pub fn int(&mut self, key: &str, number: i64) -> &mut Self {
        self.raw(key, number.to_string().into_bytes())
    }

    pub fn bool(&mut self, key: &str, flag: bool) -> &mut Self {
        self.raw(key, if flag { b"true".to_vec() } else { b"false".to_vec() })
    }

    pub fn null(&mut self, key: &str) -> &mut Self {
        self.raw(key, b"null".to_vec())
    }

    pub fn object(&mut self, key: &str, object: Obj) -> &mut Self {
        let json = object.finish();
        self.raw(key, json)
    }

    /// The object's JSON: keys by their code points (the order of their UTF-8 bytes).
    pub fn finish(mut self) -> Vec<u8> {
        self.fields.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        let mut out = Vec::with_capacity(self.fields.iter().map(|(key, value)| key.len() + value.len() + 4).sum::<usize>() + 2);
        out.push(b'{');
        for (index, (key, value)) in self.fields.iter().enumerate() {
            if index > 0 {
                out.push(b',');
            }
            write_json_string(&mut out, key);
            out.push(b':');
            out.extend_from_slice(value);
        }
        out.push(b'}');
        out
    }
}

/// A JSON list of texts written already.
pub fn list(items: impl IntoIterator<Item = Vec<u8>>) -> Vec<u8> {
    let mut out = vec![b'['];
    for (index, item) in items.into_iter().enumerate() {
        if index > 0 {
            out.push(b',');
        }
        out.extend_from_slice(&item);
    }
    out.push(b']');
    out
}

/// A JSON string's text.
pub fn string(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + 2);
    write_json_string(&mut out, text);
    out
}

/// The sha256 of bytes, as hex.
pub use crate::checkpoint::sha256_hex;

/// The number of a string's code points (Python's `len`).
pub fn code_points(text: &str) -> usize {
    text.chars().count()
}

// ---- A POST's body ------------------------------------------------------------------------------

/// A POST's body as the server reads it: refused when empty (409), when not JSON (400) and when
/// JSON that is not an object (409).
pub fn post_document(body: Option<&[u8]>) -> Answer<Map<String, Value>> {
    let body = match body {
        Some(body) if !body.is_empty() => body,
        _ => return Err(Refusal::storage("project request exceeds the body limit").into()),
    };
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(object)) => Ok(object),
        Ok(_) => Err(Refusal::storage("project request must be an object").into()),
        Err(_) => Err(Refusal::invalid_request().into()),
    }
}

/// The server's `fields`: the document has the required keys and no others but the optional ones.
pub fn check_fields(document: &Map<String, Value>, required: &[&str], optional: &[&str]) -> Answer<()> {
    let unknown = document.keys().any(|key| !required.contains(&key.as_str()) && !optional.contains(&key.as_str()));
    if unknown || required.iter().any(|key| !document.contains_key(*key)) {
        return Err(Refusal::storage("request fields do not match this operation").into());
    }
    Ok(())
}
