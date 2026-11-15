//! The compression of a stored Run's pieces: deflate-raw (RFC 1951 without a header), the one the
//! browser's `CompressionStream("deflate-raw")` writes and the "run" line names (`COMPRESSION`).
//! A writer may be given another compressor of the same format (`Compress`): the tests write
//! deflate's stored blocks, as the browser's fixtures are written (`node_stored_runs.mjs`).

use std::io::{Read, Write};

use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression;

use crate::{Error, Result};

/// The compression the "run" line names, the only one read.
pub const COMPRESSION: &str = "deflate-raw";

/// A compressor of deflate-raw.
pub type Compress = fn(&[u8]) -> Vec<u8>;

/// `raw` as deflate-raw, at zlib's default level (the browser's).
pub fn deflate_raw(raw: &[u8]) -> Vec<u8> {
    let mut encoder = DeflateEncoder::new(Vec::with_capacity(raw.len() / 4 + 64), Compression::default());
    // Writing to a Vec does not fail.
    encoder.write_all(raw).expect("deflate into memory");
    encoder.finish().expect("deflate into memory")
}

/// The bytes of the deflate-raw `stored`, which hold `raw_length` bytes (a piece's line says how
/// many): an error for bytes that are not deflate-raw or that hold more or fewer. It never inflates
/// more than `raw_length` and one byte.
pub fn inflate_raw(stored: &[u8], raw_length: u64) -> Result<Vec<u8>> {
    let capacity = usize::try_from(raw_length).map_err(|_| Error::refused("a piece longer than this machine holds"))?;
    let mut raw = Vec::with_capacity(capacity.min(1 << 26));
    DeflateDecoder::new(stored)
        .take(raw_length.saturating_add(1))
        .read_to_end(&mut raw)
        .map_err(|error| Error::refused(format!("a piece is not deflate-raw: {error}")))?;
    if raw.len() as u64 != raw_length {
        return Err(Error::refused(format!("a piece holds {} bytes, not the {raw_length} its line says", raw.len())));
    }
    Ok(raw)
}

/// deflate-raw of stored blocks of at most `size` bytes each (`BTYPE` 00): any inflater reads them,
/// and they are the same bytes everywhere.
pub fn stored_blocks(raw: &[u8], size: usize) -> Vec<u8> {
    let size = size.clamp(1, 65535);
    let chunks: Vec<&[u8]> = if raw.is_empty() { vec![raw] } else { raw.chunks(size).collect() };
    let mut out = Vec::with_capacity(raw.len() + 5 * chunks.len());
    for (index, chunk) in chunks.iter().enumerate() {
        let length = chunk.len() as u16;
        out.push(u8::from(index + 1 == chunks.len()));
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&(!length).to_le_bytes());
        out.extend_from_slice(chunk);
    }
    out
}

/// What the browser's fixtures are compressed with (`node_stored_runs.mjs`): stored blocks of at
/// most 65535 bytes.
pub fn stored_deflate(raw: &[u8]) -> Vec<u8> {
    stored_blocks(raw, 65535)
}
