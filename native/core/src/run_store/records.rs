//! The pieces of append-only records (#368, decision 4): the conversation turns and the LLM tape's
//! entries a Run stores, each a JSON text.
//!
//! A records piece is a varint, the encoding (0, texts: the only one), a varint, the count of
//! records, then each record as its length (a varint) and its UTF-8 JSON text. A piece is read
//! alone. Encoding 1 (tokens, for the decision records of format v5) is gone with them (#478).

use super::corrupt;
use crate::error::Result;
use crate::events::packed::{put_varint, Cursor};

const TEXTS: u64 = 0;

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
    walk(bytes, |record| records.push(record))?;
    Ok(records)
}

/// How many records a piece holds, each read: an error for bytes that are not a records piece.
pub fn count(bytes: &[u8]) -> Result<u64> {
    let mut count = 0;
    walk(bytes, |_| count += 1)?;
    Ok(count)
}

fn walk(bytes: &[u8], mut each: impl FnMut(Vec<u8>)) -> Result<()> {
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
        other => return Err(corrupt(&format!("records of encoding {other}"))),
    }
    if cursor.rest().is_empty() {
        Ok(())
    } else {
        Err(corrupt("bytes after the records"))
    }
}
