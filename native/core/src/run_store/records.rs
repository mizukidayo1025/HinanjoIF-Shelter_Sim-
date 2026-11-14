//! The pieces of append-only records (#368, decision 4): the decision records, the conversation
//! turns and the LLM tape's entries a Run stores, each a JSON text.
//!
//! A records piece is a varint, the encoding, a varint, the count of records, then:
//!   0 (texts)    each record as its length (a varint) and its UTF-8 JSON text
//!   1 (tokens)   the strings' table (a varint, the count, then each string's length and bytes:
//!                its JSON text between its quotes, escapes and all), then each record as tokens:
//!                a byte, the token's kind (`NULL` .. `MAP_END`), and for a string its number in
//!                the table, for an int its zigzag varint, for a float its 8 bytes (written back
//!                as `json::write_float` writes it), for any other number its text
//! A piece holds its own table, so each is read alone. The tokens hold a JSON text without its
//! spaces as it is, so a record reads back byte for byte (`read`); a text with spaces between its
//! tokens, or that is not JSON, is refused (`Tokens::push_json`). Decision records are tokens:
//! 7 days of 100 people take 87,000 records, 290 MB of JSON, 24 MB compressed as texts, about
//! 16 MB as tokens (most of what is left is their floats).

use rustc_hash::FxHashMap;

use super::corrupt;
use crate::error::{Error, Result};
use crate::events::packed::{put_varint, put_zigzag, Cursor};
use crate::json::write_float;

const TEXTS: u64 = 0;
const TOKENS: u64 = 1;

const NULL: u8 = 0;
const FALSE: u8 = 1;
const TRUE: u8 = 2;
const INT: u8 = 3;
const FLOAT: u8 = 4;
const NUMBER: u8 = 5;
const STRING: u8 = 6;
const LIST: u8 = 7;
const LIST_END: u8 = 8;
const MAP: u8 = 9;
const MAP_END: u8 = 10;

const MAX_DEPTH: usize = 64;

/// Records made tokens as they come (`push_json`), then the piece's bytes (`finish`).
#[derive(Clone, Debug, Default)]
pub struct Tokens {
    strings: Vec<Vec<u8>>,
    ids: FxHashMap<Vec<u8>, u32>,
    body: Vec<u8>,
    count: u64,
    number: Vec<u8>,
}

impl Tokens {
    pub fn new() -> Self {
        Self::default()
    }

    /// How many records it holds.
    pub fn count(&self) -> u64 {
        self.count
    }

    /// Adds the record of the JSON text `json`: an error for a text that is not JSON without spaces
    /// between its tokens, with nothing added.
    pub fn push_json(&mut self, json: &[u8]) -> Result<()> {
        if std::str::from_utf8(json).is_err() {
            return Err(not_json());
        }
        let (body_length, strings_length) = (self.body.len(), self.strings.len());
        let mut tokenizer = Tokenizer { bytes: json, at: 0 };
        let done = tokenizer.value(self, 0).and_then(|()| if tokenizer.at == json.len() { Ok(()) } else { Err(not_json()) });
        if let Err(error) = done {
            self.body.truncate(body_length);
            for string in self.strings.drain(strings_length..) {
                self.ids.remove(&string);
            }
            return Err(error);
        }
        self.count += 1;
        Ok(())
    }

    fn string(&mut self, text: &[u8]) -> Result<()> {
        let id = match self.ids.get(text) {
            Some(id) => *id,
            None => {
                let id = u32::try_from(self.strings.len()).map_err(|_| Error::unsupported("more than 2^32 strings in a records piece"))?;
                self.strings.push(text.to_vec());
                self.ids.insert(text.to_vec(), id);
                id
            }
        };
        self.body.push(STRING);
        put_varint(&mut self.body, u64::from(id));
        Ok(())
    }

    /// The piece's bytes.
    pub fn finish(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.body.len() + self.strings.iter().map(|text| text.len() + 2).sum::<usize>() + 16);
        put_varint(&mut out, TOKENS);
        put_varint(&mut out, self.count);
        put_varint(&mut out, self.strings.len() as u64);
        for text in &self.strings {
            put_varint(&mut out, text.len() as u64);
            out.extend_from_slice(text);
        }
        out.extend_from_slice(&self.body);
        out
    }
}

fn not_json() -> Error {
    Error::Value("a record that is not JSON without spaces".to_owned())
}

/// A JSON text read into tokens.
struct Tokenizer<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Tokenizer<'a> {
    fn peek(&self) -> Result<u8> {
        self.bytes.get(self.at).copied().ok_or_else(not_json)
    }

    fn expect(&mut self, byte: u8) -> Result<()> {
        if self.peek()? != byte {
            return Err(not_json());
        }
        self.at += 1;
        Ok(())
    }

    fn value(&mut self, tokens: &mut Tokens, depth: usize) -> Result<()> {
        if depth > MAX_DEPTH {
            return Err(not_json());
        }
        let word = |tokenizer: &mut Self, word: &[u8], token: u8, tokens: &mut Tokens| -> Result<()> {
            if !tokenizer.bytes[tokenizer.at..].starts_with(word) {
                return Err(not_json());
            }
            tokenizer.at += word.len();
            tokens.body.push(token);
            Ok(())
        };
        match self.peek()? {
            b'n' => word(self, b"null", NULL, tokens),
            b't' => word(self, b"true", TRUE, tokens),
            b'f' => word(self, b"false", FALSE, tokens),
            b'"' => {
                let text = self.string()?;
                tokens.string(text)
            }
            b'[' => {
                self.at += 1;
                tokens.body.push(LIST);
                if self.peek()? != b']' {
                    loop {
                        self.value(tokens, depth + 1)?;
                        if self.peek()? != b',' {
                            break;
                        }
                        self.at += 1;
                    }
                }
                self.expect(b']')?;
                tokens.body.push(LIST_END);
                Ok(())
            }
            b'{' => {
                self.at += 1;
                tokens.body.push(MAP);
                if self.peek()? != b'}' {
                    loop {
                        if self.peek()? != b'"' {
                            return Err(not_json());
                        }
                        let key = self.string()?;
                        tokens.string(key)?;
                        self.expect(b':')?;
                        self.value(tokens, depth + 1)?;
                        if self.peek()? != b',' {
                            break;
                        }
                        self.at += 1;
                    }
                }
                self.expect(b'}')?;
                tokens.body.push(MAP_END);
                Ok(())
            }
            b'-' | b'0'..=b'9' => {
                let start = self.at;
                while self.bytes.get(self.at).is_some_and(|byte| matches!(byte, b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')) {
                    self.at += 1;
                }
                let text = &self.bytes[start..self.at];
                let number = std::str::from_utf8(text).map_err(|_| not_json())?;
                let float: f64 = number.parse().map_err(|_| not_json())?;
                tokens.number.clear();
                if text.iter().any(|byte| matches!(byte, b'.' | b'e' | b'E')) {
                    if write_float(&mut tokens.number, float).is_ok() && tokens.number == text {
                        tokens.body.push(FLOAT);
                        tokens.body.extend_from_slice(&float.to_bits().to_le_bytes());
                        return Ok(());
                    }
                } else if let Ok(int) = number.parse::<i64>() {
                    tokens.number.extend_from_slice(itoa::Buffer::new().format(int).as_bytes());
                    if tokens.number == text {
                        tokens.body.push(INT);
                        put_zigzag(&mut tokens.body, int);
                        return Ok(());
                    }
                }
                tokens.body.push(NUMBER);
                put_varint(&mut tokens.body, text.len() as u64);
                tokens.body.extend_from_slice(text);
                Ok(())
            }
            _ => Err(not_json()),
        }
    }

    /// A string's JSON text between its quotes, escapes and all: an error for a control character
    /// in it, an escape JSON does not have, or no closing quote.
    fn string(&mut self) -> Result<&'a [u8]> {
        self.at += 1;
        let start = self.at;
        loop {
            match self.peek()? {
                b'"' => break,
                b'\\' => {
                    self.at += 1;
                    match self.peek()? {
                        b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => self.at += 1,
                        b'u' => {
                            let hex = self.bytes.get(self.at + 1..self.at + 5).ok_or_else(not_json)?;
                            if !hex.iter().all(u8::is_ascii_hexdigit) {
                                return Err(not_json());
                            }
                            self.at += 5;
                        }
                        _ => return Err(not_json()),
                    }
                }
                byte if byte < 0x20 => return Err(not_json()),
                _ => self.at += 1,
            }
        }
        let text = &self.bytes[start..self.at];
        self.at += 1;
        Ok(text)
    }
}

/// The piece of the JSON texts `records`, as they are.
pub fn texts(records: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::with_capacity(records.iter().map(|record| record.len() + 4).sum::<usize>() + 8);
    put_varint(&mut out, TEXTS);
    put_varint(&mut out, records.len() as u64);
    for record in records {
        put_varint(&mut out, record.len() as u64);
        out.extend_from_slice(record);
    }
    out
}

/// The JSON texts of a records piece, in order: an error for bytes that are not one.
pub fn read(bytes: &[u8]) -> Result<Vec<Vec<u8>>> {
    let mut records = Vec::new();
    walk(bytes, true, |record| records.push(record))?;
    Ok(records)
}

/// How many records a piece holds, each read: an error for bytes that are not a records piece.
pub fn count(bytes: &[u8]) -> Result<u64> {
    let mut count = 0;
    walk(bytes, false, |_| count += 1)?;
    Ok(count)
}

fn walk(bytes: &[u8], write: bool, mut each: impl FnMut(Vec<u8>)) -> Result<()> {
    let mut cursor = Cursor::new(bytes);
    let length = |cursor: &mut Cursor<'_>| usize::try_from(cursor.varint()?).map_err(|_| corrupt("a record's length"));
    let encoding = cursor.varint()?;
    let count = cursor.varint()?;
    match encoding {
        TEXTS => {
            for _ in 0..count {
                let size = length(&mut cursor)?;
                let text = cursor.take(size)?;
                std::str::from_utf8(text).map_err(|_| corrupt("a record that is not UTF-8"))?;
                each(text.to_vec());
            }
        }
        TOKENS => {
            let strings = length(&mut cursor)?;
            let mut table = Vec::with_capacity(strings.min(cursor.rest().len()));
            for _ in 0..strings {
                let size = length(&mut cursor)?;
                table.push(cursor.take(size)?);
            }
            for _ in 0..count {
                let mut out = Vec::new();
                untoken(&mut cursor, &table, &mut out, write, 0)?;
                if write {
                    std::str::from_utf8(&out).map_err(|_| corrupt("a record that is not UTF-8"))?;
                }
                each(out);
            }
        }
        other => return Err(corrupt(&format!("records of encoding {other}"))),
    }
    if cursor.rest().is_empty() {
        Ok(())
    } else {
        Err(corrupt("bytes after the records"))
    }
}

/// Writes the JSON text of the value whose tokens start at `cursor` to `out` (when `write`).
fn untoken(cursor: &mut Cursor<'_>, table: &[&[u8]], out: &mut Vec<u8>, write: bool, depth: usize) -> Result<()> {
    if depth > MAX_DEPTH {
        return Err(corrupt("records nested too deep"));
    }
    let tag = cursor.byte()?;
    fn push(out: &mut Vec<u8>, write: bool, bytes: &[u8]) {
        if write {
            out.extend_from_slice(bytes);
        }
    }
    match tag {
        NULL => push(out, write, b"null"),
        FALSE => push(out, write, b"false"),
        TRUE => push(out, write, b"true"),
        INT => push(out, write, itoa::Buffer::new().format(cursor.zigzag()?).as_bytes()),
        FLOAT => {
            let value = cursor.float()?;
            if write {
                write_float(out, value).map_err(|_| corrupt("a float that is not finite"))?;
            }
        }
        NUMBER => {
            let size = usize::try_from(cursor.varint()?).map_err(|_| corrupt("a number's length"))?;
            push(out, write, cursor.take(size)?);
        }
        STRING => {
            let text = string(cursor, table)?;
            push(out, write, b"\"");
            push(out, write, text);
            push(out, write, b"\"");
        }
        LIST | MAP => {
            let (open, close, end) = if tag == LIST { (b"[", b"]", LIST_END) } else { (b"{", b"}", MAP_END) };
            if write {
                out.extend_from_slice(open);
            }
            let mut first = true;
            while cursor.rest().first() != Some(&end) {
                if !first && write {
                    out.push(b',');
                }
                first = false;
                if tag == MAP {
                    if cursor.byte()? != STRING {
                        return Err(corrupt("a map's key that is not a string"));
                    }
                    let key = string(cursor, table)?;
                    if write {
                        out.push(b'"');
                        out.extend_from_slice(key);
                        out.extend_from_slice(b"\":");
                    }
                }
                untoken(cursor, table, out, write, depth + 1)?;
            }
            cursor.byte()?;
            if write {
                out.extend_from_slice(close);
            }
        }
        other => return Err(corrupt(&format!("a record's token {other}"))),
    }
    Ok(())
}

fn string<'a>(cursor: &mut Cursor<'_>, table: &[&'a [u8]]) -> Result<&'a [u8]> {
    let id = usize::try_from(cursor.varint()?).map_err(|_| corrupt("a string's number"))?;
    table.get(id).copied().ok_or_else(|| corrupt("a string the table does not hold"))
}
