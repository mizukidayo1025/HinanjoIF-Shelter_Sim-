//! A route's query as the server reads it (Python's `urllib.parse.parse_qs`), and its ints.

use super::{js_trim, Refusal};

/// `urllib.parse.unquote(text.replace("+", " "))`: %XX escapes are bytes, read as UTF-8 with
/// U+FFFD for what is not; a % without two hex digits stays.
pub fn unquote(text: &str) -> String {
    let text = text.replace('+', " ");
    if !text.contains('%') {
        return text;
    }
    let raw = text.as_bytes();
    let mut bytes = Vec::with_capacity(raw.len());
    let mut index = 0;
    while index < raw.len() {
        if raw[index] == b'%' && index + 2 < raw.len() {
            let pair = (hex(raw[index + 1]), hex(raw[index + 2]));
            if let (Some(high), Some(low)) = pair {
                bytes.push(high * 16 + low);
                index += 3;
                continue;
            }
        }
        bytes.push(raw[index]);
        index += 1;
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn hex(byte: u8) -> Option<u8> {
    (byte as char).to_digit(16).map(|digit| digit as u8)
}

/// A query's fields as `parse_qs` reads them: each name with its values, in the order the names
/// first came. Fields are split at "&"; a field without "=" or with an empty value is left out. A
/// leading "?" is not part of the query.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    pub fields: Vec<(String, Vec<String>)>,
}

impl Query {
    pub fn parse(query: &str) -> Query {
        let text = query.strip_prefix('?').unwrap_or(query);
        let mut fields: Vec<(String, Vec<String>)> = Vec::new();
        for field in text.split('&') {
            let Some((name, value)) = field.split_once('=') else { continue };
            if value.is_empty() {
                continue;
            }
            let (name, value) = (unquote(name), unquote(value));
            match fields.iter_mut().find(|(each, _)| *each == name) {
                Some((_, values)) => values.push(value),
                None => fields.push((name, vec![value])),
            }
        }
        Query { fields }
    }

    pub fn len(&self) -> usize {
        self.fields.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    pub fn get(&self, name: &str) -> Option<&[String]> {
        self.fields.iter().find(|(each, _)| each == name).map(|(_, values)| values.as_slice())
    }

    pub fn has(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// The value of a field given once, or None.
    pub fn one(&self, name: &str) -> Option<&str> {
        match self.get(name) {
            Some([value]) => Some(value.as_str()),
            _ => None,
        }
    }
}

/// The route's query must be empty: the server answers any other with this.
pub fn no_query(query: &str) -> Result<(), Refusal> {
    if Query::parse(query).is_empty() {
        Ok(())
    } else {
        Err(Refusal::storage("unknown project route or query"))
    }
}

/// The server's `int()` of a query value of the Project routes: spaces around, underscores
/// dropped, a sign and ASCII digits; a value past 2^53 is refused too (`message`, a StorageError).
pub fn route_int(text: &str, message: &str) -> Result<i64, Refusal> {
    let trimmed = js_trim(text).replace('_', "");
    let digits = trimmed.strip_prefix(['+', '-']).unwrap_or(&trimmed);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(Refusal::storage(message));
    }
    let value: f64 = trimmed.parse().map_err(|_| Refusal::storage(message))?;
    if value.abs() > 9_007_199_254_740_991.0 {
        return Err(Refusal::storage(message));
    }
    Ok(value as i64)
}
