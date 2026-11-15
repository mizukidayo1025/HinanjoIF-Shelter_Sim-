//! Kernels over saved bytes (docs/rust-port.md, stage B3): the privacy check's candidates, which a
//! stored Run's privacy check (`sensitive`) reads, and the hex digests. Without the `parallel`
//! feature (the wasm32 build), `sensitive_candidates` reads its inputs one after another on the
//! calling thread, with the same result.
//!
//! `candidates` reads a chunk's canonical JSON bytes and returns what the privacy check must look
//! at: the distinct object keys, and the distinct string values that its patterns could match. The
//! patterns are those of `src/sheltersim/storage/projects.py` (`interpret::redacts`); a string left
//! out here can not match them, for these reasons:
//!
//! - `_SECRET`'s address alternative needs "@".
//! - `sk-(?:ant-)?[A-Za-z0-9_-]{12,}` needs "sk-" and 12 more such characters.
//! - `Bearer\s+\S+` needs "bearer", in any case.
//! - `PHONE_NUMBER` needs two separators, each between digits.
//!
//! The patterns ignore case, under which U+017F matches "s" and U+212A matches "k", and `\d`
//! matches every Unicode decimal digit; a string with one of those is returned whatever else it
//! holds (`char::is_numeric` covers more than `\d`).

#[cfg(feature = "parallel")]
use rayon::prelude::*;
use rustc_hash::FxHashSet;
use sha2::{Digest, Sha256};

/// The keys and the strings of `candidates`.
pub type Candidates = (FxHashSet<String>, FxHashSet<String>);

const SK_RUN: usize = 12;

fn token_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
}

fn starts_ignoring_case(bytes: &[u8], lower: &[u8]) -> bool {
    bytes.len() >= lower.len() && bytes[..lower.len()].eq_ignore_ascii_case(lower)
}

/// Whether the patterns could match text, the UTF-8 of one decoded JSON string.
fn could_match(text: &[u8]) -> bool {
    let mut separators = 0;
    for (index, &byte) in text.iter().enumerate() {
        match byte {
            b'@' => return true,
            b's' | b'S' if starts_ignoring_case(&text[index..], b"sk-") => {
                let run = text[index + 3..].iter().take(SK_RUN).take_while(|b| token_char(**b));
                if run.count() == SK_RUN {
                    return true;
                }
            }
            b'b' | b'B' if starts_ignoring_case(&text[index..], b"bearer") => return true,
            b'-' | b' ' => {
                let digit_before = index > 0 && text[index - 1].is_ascii_digit();
                let digit_after = text.get(index + 1).is_some_and(u8::is_ascii_digit);
                if digit_before && digit_after {
                    separators += 1;
                    if separators == 2 {
                        return true;
                    }
                }
            }
            _ => {}
        }
    }
    if text.is_ascii() {
        return false;
    }
    match std::str::from_utf8(text) {
        Ok(decoded) => decoded.chars().any(|c| c == '\u{17f}' || c == '\u{212a}' || (!c.is_ascii() && c.is_numeric())),
        Err(_) => true,
    }
}

fn decode(raw: &[u8]) -> std::result::Result<String, String> {
    serde_json::from_slice(raw).map_err(|error| error.to_string())
}

/// decode of a quoted string with no escape or control character in it.
fn decode_plain(quoted: &[u8]) -> std::result::Result<String, String> {
    std::str::from_utf8(&quoted[1..quoted.len() - 1]).map(str::to_owned).map_err(|error| error.to_string())
}

/// The distinct keys and the distinct string values to check in raw, one JSON text.
pub fn candidates(raw: &[u8]) -> std::result::Result<Candidates, String> {
    let mut keys: FxHashSet<String> = FxHashSet::default();
    let mut strings: FxHashSet<String> = FxHashSet::default();
    let mut seen_keys: FxHashSet<&[u8]> = FxHashSet::default();
    let mut seen_strings: FxHashSet<&[u8]> = FxHashSet::default();
    let mut index = 0;
    let length = raw.len();
    while index < length {
        if raw[index] != b'"' {
            index += 1;
            continue;
        }
        let start = index;
        index += 1;
        let mut escaped = false;
        while index < length && raw[index] != b'"' {
            if raw[index] == b'\\' {
                escaped = true;
                index += 1;
            } else if raw[index] < 0x20 {
                // serde_json says what is wrong with it.
                escaped = true;
            }
            index += 1;
        }
        if index >= length {
            return Err("unterminated JSON string".to_owned());
        }
        index += 1;
        let quoted = &raw[start..index];
        // In JSON only an object key is followed by ":".
        if raw.get(index) == Some(&b':') {
            if seen_keys.insert(quoted) {
                keys.insert(if escaped { decode(quoted)? } else { decode_plain(quoted)? });
            }
        } else if seen_strings.insert(quoted) {
            if escaped {
                let text = decode(quoted)?;
                if could_match(text.as_bytes()) {
                    strings.insert(text);
                }
            } else if could_match(&quoted[1..quoted.len() - 1]) {
                strings.insert(decode_plain(quoted)?);
            }
        }
    }
    Ok((keys, strings))
}

/// The `candidates` of several JSON texts together, read in parallel. The sets are the same
/// in any order of reading; the order they iterate in is not part of the result. Of several texts
/// that can not be read, which one's error is returned is not
/// fixed in parallel; read in order, it is the first one's.
pub fn sensitive_candidates(raws: &[&[u8]]) -> std::result::Result<Candidates, String> {
    #[cfg(feature = "parallel")]
    let read = raws.par_iter().map(|raw| candidates(raw)).try_reduce(Candidates::default, merge_candidates);
    #[cfg(not(feature = "parallel"))]
    let read = raws.iter().try_fold(Candidates::default(), |read, raw| merge_candidates(read, candidates(raw)?));
    read
}

fn merge_candidates((mut keys, mut strings): Candidates, (more_keys, more_strings): Candidates) -> std::result::Result<Candidates, String> {
    keys.extend(more_keys);
    strings.extend(more_strings);
    Ok((keys, strings))
}

/// Lowercase hex digits of bytes, as `hashlib`'s `hexdigest` writes them.
pub fn hex(digest: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(digest.len() * 2);
    for byte in digest {
        text.push(DIGITS[usize::from(byte >> 4)] as char);
        text.push(DIGITS[usize::from(byte & 15)] as char);
    }
    text
}

/// `hashlib.sha256(raw).hexdigest()`.
pub fn sha256_hex(raw: &[u8]) -> String {
    hex(&Sha256::digest(raw))
}
