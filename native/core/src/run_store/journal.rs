//! The journal's lines: their typed values, their JSON as the browser's writer wrote it, and the
//! journal read back (`Journal::parse`).

use serde_json::{Map, Value};

use super::{corrupt, Format};
use crate::checkpoint::sha256_hex;
use crate::error::{Error, Result};
use crate::json::Encoder;

/// Where a piece is in data.pack and what it holds: `length` compressed bytes at `offset`, which
/// decompress to `raw_length` bytes whose sha256 is `sha256` (hex).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stored {
    pub offset: u64,
    pub length: u64,
    pub raw_length: u64,
    pub sha256: String,
}

/// The tables' new entries a tables piece holds: the strings and shapes from `strings_from` and
/// `shapes_from` up to `strings` and `shapes`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tables {
    pub strings_from: u64,
    pub shapes_from: u64,
    pub strings: u64,
    pub shapes: u64,
}

/// What a "saved" line says of its saved point: the minute, the counts of records and samples, the
/// saved point's version, and how many log and sample blocks hold those records and samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SavedLine {
    pub minute: i64,
    pub records: u64,
    pub samples: u64,
    pub point_version: u64,
    pub log_blocks: u64,
    pub sample_blocks: u64,
}

/// A piece's kind and what its line holds beside its place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    Export,
    LogTables(Tables),
    LogBlock { index: u64 },
    SampleTables(Tables),
    SampleBlock { index: u64 },
    Saved(SavedLine),
    World,
}

impl Piece {
    /// The line's `kind`.
    pub fn kind(&self) -> &'static str {
        match self {
            Piece::Export => "export",
            Piece::LogTables(_) => "log_tables",
            Piece::LogBlock { .. } => "log_block",
            Piece::SampleTables(_) => "sample_tables",
            Piece::SampleBlock { .. } => "sample_block",
            Piece::Saved(_) => "saved",
            Piece::World => "world",
        }
    }

    /// A piece of the stored log.
    pub fn is_log(&self) -> bool {
        matches!(self, Piece::LogTables(_) | Piece::LogBlock { .. })
    }

    /// A piece of the stored samples.
    pub fn is_samples(&self) -> bool {
        matches!(self, Piece::SampleTables(_) | Piece::SampleBlock { .. })
    }
}

/// A piece's line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PieceLine {
    pub piece: Piece,
    pub stored: Stored,
}

/// The fork a branch's Run starts at (v4): the minute and the branch it was made from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForkStart {
    pub minute: i64,
    pub parent_branch_id: String,
}

/// The "run" line: the format and what the Run is of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunLine {
    pub format: Format,
    /// The sha256 of the exported world's bytes, and their length.
    pub export_sha256: String,
    pub export_bytes: u64,
    /// The world's name in a file of named worlds (the fixtures), or None for an exported entry.
    pub world: Option<String>,
    pub duration_minutes: i64,
    /// Minutes between saved points (0: none); None in v1, whose line has none.
    pub save_every: Option<u64>,
    /// The fork a branch's Run starts at (v4).
    pub fork: Option<ForkStart>,
}

impl RunLine {
    /// The line of a new Run of the current format from the exported world `export`.
    pub fn new(export: &[u8], world: Option<String>, duration_minutes: i64, save_every: u64) -> Self {
        Self {
            format: Format::CURRENT,
            export_sha256: sha256_hex(export),
            export_bytes: export.len() as u64,
            world,
            duration_minutes,
            save_every: Some(save_every),
            fork: None,
        }
    }

    /// The line of a Run of the current format from the same exported world as `source`, saved as
    /// often (the default where `source` says none), from minute 0.
    pub fn same_start(source: &RunLine) -> Self {
        Self {
            format: Format::CURRENT,
            export_sha256: source.export_sha256.clone(),
            export_bytes: source.export_bytes,
            world: source.world.clone(),
            duration_minutes: source.duration_minutes,
            save_every: Some(source.save_every_or_default()),
            fork: None,
        }
    }

    /// The line of the Run of a branch forked from `parent`'s Run at `minute` (#306).
    pub fn forked(parent: &RunLine, minute: i64, parent_branch_id: &str) -> Self {
        Self { fork: Some(ForkStart { minute, parent_branch_id: parent_branch_id.to_owned() }), ..Self::same_start(parent) }
    }

    /// Minutes between saved points, the default (`SAVE_EVERY`) where the line has none or 0.
    pub fn save_every_or_default(&self) -> u64 {
        match self.save_every {
            Some(every) if every > 0 => every,
            _ => super::SAVE_EVERY,
        }
    }

    /// The minute the Run starts at: the fork's, else 0.
    pub fn fork_minute(&self) -> i64 {
        self.fork.as_ref().map_or(0, |fork| fork.minute)
    }
}

/// Where a fork was made: the parent branch, its minute, its world's digest there and its head.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForkOrigin {
    pub branch_id: String,
    pub minute: i64,
    pub state_hash: String,
    pub checkpoint_hash: Option<String>,
}

/// The "fork" line of a branch made by a fork (v4): its origin, the EffectProgram applied (its
/// JSON text), the approval's hash and the receipt (its JSON text).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForkLine {
    pub parent_branch_id: String,
    pub minute: i64,
    pub origin: ForkOrigin,
    pub program: String,
    pub approved_program_hash: String,
    pub receipt: String,
}

/// The "end" line: the minute the Run reached, its counts, and the digests it gave (each part's,
/// in the order of the parts, and the metric samples').
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndLine {
    pub minute: i64,
    pub records: u64,
    pub samples: u64,
    pub parts: Vec<(String, String)>,
    pub metrics: String,
}

/// A line of the journal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    Run(RunLine),
    Fork(ForkLine),
    Piece(PieceLine),
    End(EndLine),
}

/// A line as it is written: its bytes (the JSON, a tab, the sha256, "\n") and the sha256 of its
/// JSON (a branch's head is one).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Encoded {
    pub bytes: Vec<u8>,
    pub sha256: String,
}

fn key(encoder: &mut Encoder, name: &str) {
    if encoder.out.len() > 1 {
        encoder.out.push(b',');
    }
    encoder.string(name);
    encoder.out.push(b':');
}

fn uint(encoder: &mut Encoder, number: u64) {
    encoder.out.extend_from_slice(itoa::Buffer::new().format(number).as_bytes());
}

fn string_or_null(encoder: &mut Encoder, text: Option<&str>) {
    match text {
        Some(text) => encoder.string(text),
        None => encoder.null(),
    }
}

impl Line {
    /// The line's `kind`.
    pub fn kind(&self) -> &'static str {
        match self {
            Line::Run(_) => "run",
            Line::Fork(_) => "fork",
            Line::Piece(line) => line.piece.kind(),
            Line::End(_) => "end",
        }
    }

    /// The line's JSON, as the browser's writer wrote it (`JSON.stringify` of its object).
    pub fn to_json(&self) -> Vec<u8> {
        let mut out = Encoder::with_capacity(256);
        out.out.push(b'{');
        key(&mut out, "kind");
        out.string(self.kind());
        match self {
            Line::Run(run) => {
                key(&mut out, "format");
                out.string(run.format.name());
                key(&mut out, "export_sha256");
                out.string(&run.export_sha256);
                key(&mut out, "export_bytes");
                uint(&mut out, run.export_bytes);
                key(&mut out, "world");
                string_or_null(&mut out, run.world.as_deref());
                key(&mut out, "duration_minutes");
                out.int(run.duration_minutes);
                if let Some(every) = run.save_every {
                    key(&mut out, "save_every");
                    uint(&mut out, every);
                }
                if let Some(fork) = &run.fork {
                    key(&mut out, "fork_minute");
                    out.int(fork.minute);
                    key(&mut out, "parent_branch_id");
                    out.string(&fork.parent_branch_id);
                }
            }
            Line::Fork(fork) => {
                key(&mut out, "parent_branch_id");
                out.string(&fork.parent_branch_id);
                key(&mut out, "minute");
                out.int(fork.minute);
                key(&mut out, "fork_origin");
                let mut origin = Encoder::with_capacity(256);
                origin.out.push(b'{');
                key(&mut origin, "branch_id");
                origin.string(&fork.origin.branch_id);
                key(&mut origin, "minute");
                origin.int(fork.origin.minute);
                key(&mut origin, "state_hash");
                origin.string(&fork.origin.state_hash);
                key(&mut origin, "checkpoint_hash");
                string_or_null(&mut origin, fork.origin.checkpoint_hash.as_deref());
                origin.out.push(b'}');
                out.out.extend_from_slice(&origin.out);
                key(&mut out, "program");
                out.string(&fork.program);
                key(&mut out, "approved_program_hash");
                out.string(&fork.approved_program_hash);
                key(&mut out, "receipt");
                out.string(&fork.receipt);
            }
            Line::Piece(line) => {
                let stored = &line.stored;
                key(&mut out, "offset");
                uint(&mut out, stored.offset);
                key(&mut out, "length");
                uint(&mut out, stored.length);
                key(&mut out, "raw_length");
                uint(&mut out, stored.raw_length);
                key(&mut out, "sha256");
                out.string(&stored.sha256);
                match &line.piece {
                    Piece::Export | Piece::World => {}
                    Piece::LogTables(tables) | Piece::SampleTables(tables) => {
                        for (name, value) in
                            [("strings_from", tables.strings_from), ("shapes_from", tables.shapes_from), ("strings", tables.strings), ("shapes", tables.shapes)]
                        {
                            key(&mut out, name);
                            uint(&mut out, value);
                        }
                    }
                    Piece::LogBlock { index } | Piece::SampleBlock { index } => {
                        key(&mut out, "index");
                        uint(&mut out, *index);
                    }
                    Piece::Saved(saved) => {
                        key(&mut out, "minute");
                        out.int(saved.minute);
                        for (name, value) in [
                            ("records", saved.records),
                            ("samples", saved.samples),
                            ("point_version", saved.point_version),
                            ("log_blocks", saved.log_blocks),
                            ("sample_blocks", saved.sample_blocks),
                        ] {
                            key(&mut out, name);
                            uint(&mut out, value);
                        }
                    }
                }
            }
            Line::End(end) => {
                key(&mut out, "minute");
                out.int(end.minute);
                key(&mut out, "records");
                uint(&mut out, end.records);
                key(&mut out, "samples");
                uint(&mut out, end.samples);
                key(&mut out, "parts");
                out.out.push(b'{');
                for (index, (part, digest)) in end.parts.iter().enumerate() {
                    if index > 0 {
                        out.out.push(b',');
                    }
                    out.string(part);
                    out.out.push(b':');
                    out.string(digest);
                }
                out.out.push(b'}');
                key(&mut out, "metrics");
                out.string(&end.metrics);
            }
        }
        out.out.push(b'}');
        out.out
    }

    /// The line as it is written to the journal.
    pub fn encode(&self) -> Encoded {
        let json = self.to_json();
        let sha256 = sha256_hex(&json);
        let mut bytes = json;
        bytes.reserve(66);
        bytes.push(b'\t');
        bytes.extend_from_slice(sha256.as_bytes());
        bytes.push(b'\n');
        Encoded { bytes, sha256 }
    }

    /// The line of `json` (a journal line's JSON whose sha256 matched): an error for JSON that is
    /// not a line of a stored Run, as this core reads them. Keys it does not know are let through.
    pub fn of_json(json: &[u8]) -> Result<Line> {
        let value: Value = serde_json::from_slice(json).map_err(|_| corrupt("a line that is not JSON"))?;
        let object = Fields::of(&value, "a line")?;
        let kind = object.string("kind")?;
        let stored = || -> Result<Stored> {
            Ok(Stored {
                offset: object.uint("offset")?,
                length: object.uint("length")?,
                raw_length: object.uint("raw_length")?,
                sha256: object.string("sha256")?,
            })
        };
        let tables = || -> Result<Tables> {
            Ok(Tables {
                strings_from: object.uint("strings_from")?,
                shapes_from: object.uint("shapes_from")?,
                strings: object.uint("strings")?,
                shapes: object.uint("shapes")?,
            })
        };
        let piece = |piece: Piece| -> Result<Line> { Ok(Line::Piece(PieceLine { piece, stored: stored()? })) };
        match kind.as_str() {
            "run" => {
                let fork = match object.optional("fork_minute") {
                    Some(_) => Some(ForkStart { minute: object.minute("fork_minute")?, parent_branch_id: object.string("parent_branch_id")? }),
                    None => None,
                };
                Ok(Line::Run(RunLine {
                    format: Format::of_name(&object.string("format")?)?,
                    export_sha256: object.string("export_sha256")?,
                    export_bytes: object.uint("export_bytes")?,
                    world: object.string_or_null("world")?,
                    duration_minutes: object.minute("duration_minutes")?,
                    save_every: object.optional("save_every").map(|_| object.uint("save_every")).transpose()?,
                    fork,
                }))
            }
            "fork" => {
                let origin = Fields::of(object.get("fork_origin")?, "fork_origin")?;
                Ok(Line::Fork(ForkLine {
                    parent_branch_id: object.string("parent_branch_id")?,
                    minute: object.minute("minute")?,
                    origin: ForkOrigin {
                        branch_id: origin.string("branch_id")?,
                        minute: origin.minute("minute")?,
                        state_hash: origin.string("state_hash")?,
                        checkpoint_hash: origin.string_or_null("checkpoint_hash")?,
                    },
                    program: object.string("program")?,
                    approved_program_hash: object.string("approved_program_hash")?,
                    receipt: object.string("receipt")?,
                }))
            }
            "end" => {
                let parts = match object.get("parts")? {
                    Value::Object(parts) => parts
                        .iter()
                        .map(|(part, digest)| {
                            digest.as_str().map(|digest| (part.clone(), digest.to_owned())).ok_or_else(|| corrupt("a part's digest that is not text"))
                        })
                        .collect::<Result<Vec<_>>>()?,
                    _ => return Err(corrupt("the end line's parts are not an object")),
                };
                Ok(Line::End(EndLine {
                    minute: object.minute("minute")?,
                    records: object.uint("records")?,
                    samples: object.uint("samples")?,
                    parts,
                    metrics: object.string("metrics")?,
                }))
            }
            "export" => piece(Piece::Export),
            "world" => piece(Piece::World),
            "log_tables" => piece(Piece::LogTables(tables()?)),
            "sample_tables" => piece(Piece::SampleTables(tables()?)),
            "log_block" => piece(Piece::LogBlock { index: object.uint("index")? }),
            "sample_block" => piece(Piece::SampleBlock { index: object.uint("index")? }),
            "saved" => piece(Piece::Saved(SavedLine {
                minute: object.minute("minute")?,
                records: object.uint("records")?,
                samples: object.uint("samples")?,
                point_version: object.uint("point_version")?,
                log_blocks: object.uint("log_blocks")?,
                sample_blocks: object.uint("sample_blocks")?,
            })),
            other => Err(corrupt(&format!("a line of an unknown kind: {other}"))),
        }
    }
}

/// The fields of a line's object, read with their types.
struct Fields<'a> {
    object: &'a Map<String, Value>,
    what: &'a str,
}

impl<'a> Fields<'a> {
    fn of(value: &'a Value, what: &'a str) -> Result<Self> {
        match value {
            Value::Object(object) => Ok(Self { object, what }),
            _ => Err(corrupt(&format!("{what} that is not an object"))),
        }
    }

    fn optional(&self, name: &str) -> Option<&'a Value> {
        self.object.get(name)
    }

    fn get(&self, name: &str) -> Result<&'a Value> {
        self.object.get(name).ok_or_else(|| corrupt(&format!("{} without {name}", self.what)))
    }

    fn bad(&self, name: &str, wanted: &str) -> Error {
        corrupt(&format!("{name} of {} is not {wanted}", self.what))
    }

    fn string(&self, name: &str) -> Result<String> {
        self.get(name)?.as_str().map(str::to_owned).ok_or_else(|| self.bad(name, "text"))
    }

    fn string_or_null(&self, name: &str) -> Result<Option<String>> {
        match self.optional(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(text)) => Ok(Some(text.clone())),
            Some(_) => Err(self.bad(name, "text or null")),
        }
    }

    /// A whole number, not negative, as JSON.stringify writes one (no fraction, no exponent).
    fn uint(&self, name: &str) -> Result<u64> {
        match self.get(name)? {
            Value::Number(number) => number.as_u64().ok_or_else(|| self.bad(name, "a count")),
            _ => Err(self.bad(name, "a count")),
        }
    }

    /// A minute: a whole number, not negative.
    fn minute(&self, name: &str) -> Result<i64> {
        i64::try_from(self.uint(name)?).map_err(|_| self.bad(name, "a minute"))
    }
}

/// A line read back: the line, its JSON text and sha256 as they are in the journal, and the byte
/// just after it (its "\n").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub line: Line,
    pub json: String,
    pub sha256: String,
    pub end: u64,
}

/// A journal read back: its lines up to the first that is cut short or does not match its sha256
/// (`torn` then).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Journal {
    entries: Vec<Entry>,
    torn: bool,
}

impl Journal {
    /// The lines of a journal's bytes, up to the first that is cut short (no "\n", or no tab) or
    /// whose sha256 is not its JSON's. A line that matches its sha256 was written whole, so one
    /// that is not a line of this format is an error, not the journal's end: a JSON that is not a
    /// known line (`Line::of_json`), a first line that is not "run", a second "run" line, a line
    /// after "end", a "saved" line in v1, a "fork" line in a Run that is not a fork's or twice.
    pub fn parse(bytes: &[u8]) -> Result<Journal> {
        let mut journal = Journal::default();
        let mut at = 0;
        while at < bytes.len() {
            let Some(newline) = bytes[at..].iter().position(|byte| *byte == b'\n').map(|offset| at + offset) else {
                journal.torn = true;
                break;
            };
            let text = &bytes[at..newline];
            let Some(tab) = text.iter().rposition(|byte| *byte == b'\t') else {
                journal.torn = true;
                break;
            };
            let (json, sha256) = (&text[..tab], &text[tab + 1..]);
            if sha256_hex(json).as_bytes() != sha256 {
                journal.torn = true;
                break;
            }
            let line = Line::of_json(json)?;
            journal.check_next(&line)?;
            journal.entries.push(Entry {
                line,
                // Both are UTF-8: the JSON parsed, and the sha256 is the hex it was compared with.
                json: String::from_utf8_lossy(json).into_owned(),
                sha256: String::from_utf8_lossy(sha256).into_owned(),
                end: (newline + 1) as u64,
            });
            at = newline + 1;
        }
        Ok(journal)
    }

    /// An error where `line` cannot come next.
    fn check_next(&self, line: &Line) -> Result<()> {
        let run = match (self.entries.first().map(|entry| &entry.line), line) {
            (None, Line::Run(_)) => return Ok(()),
            (None, _) => return Err(corrupt("its first line is not the run line")),
            (Some(Line::Run(run)), _) => run,
            (Some(_), _) => return Err(corrupt("its first line is not the run line")),
        };
        if matches!(self.entries.last().map(|entry| &entry.line), Some(Line::End(_))) {
            return Err(corrupt("a line after the end line"));
        }
        match line {
            Line::Run(_) => Err(corrupt("a second run line")),
            Line::Piece(PieceLine { piece: Piece::Saved(_), .. }) if !run.format.has_saved_points() => {
                Err(Error::Value("a saved point in a Run of format v1".to_owned()))
            }
            Line::Fork(_) if run.fork.is_none() || !run.format.has_forks() => Err(corrupt("a fork line in a Run that is not a fork's")),
            Line::Fork(_) if self.entries.iter().any(|entry| matches!(entry.line, Line::Fork(_))) => Err(corrupt("a second fork line")),
            _ => Ok(()),
        }
    }

    /// The lines read.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Whether reading stopped at a line cut short or not matching its sha256.
    pub fn torn(&self) -> bool {
        self.torn
    }

    /// The "run" line; an error for a journal without one (not a stored Run).
    pub fn run(&self) -> Result<&RunLine> {
        match self.entries.first().map(|entry| &entry.line) {
            Some(Line::Run(run)) => Ok(run),
            _ => Err(Error::Value("not a stored Run".to_owned())),
        }
    }

    /// The piece line `index`; an error for a line that holds no piece.
    pub fn piece(&self, index: usize) -> Result<&PieceLine> {
        match self.entries.get(index).map(|entry| &entry.line) {
            Some(Line::Piece(line)) => Ok(line),
            Some(line) => Err(Error::Value(format!("the journal's line {index} ({}) holds no piece", line.kind()))),
            None => Err(Error::Value(format!("the journal has no line {index}"))),
        }
    }

    /// The first line of `kind`'s index, if any.
    pub fn find(&self, kind: &str) -> Option<usize> {
        self.entries.iter().position(|entry| entry.line.kind() == kind)
    }

    /// The "end" line, or None for a Run not stored to its end.
    pub fn end(&self) -> Option<&EndLine> {
        self.entries.last().and_then(|entry| match &entry.line {
            Line::End(end) => Some(end),
            _ => None,
        })
    }

    /// The "fork" line of a branch made by a fork.
    pub fn fork(&self) -> Option<&ForkLine> {
        self.entries.iter().find_map(|entry| match &entry.line {
            Line::Fork(fork) => Some(fork),
            _ => None,
        })
    }

    /// An error where `length` bytes were read for the piece `index` (data.pack ends before it).
    pub fn check_stored(&self, index: usize, length: u64) -> Result<()> {
        let line = self.piece(index)?;
        if line.stored.length != length {
            return Err(Error::Value(format!("{}: data.pack ends before the piece", line.piece.kind())));
        }
        Ok(())
    }

    /// An error where `raw`, the piece `index` decompressed, is not the bytes written (its length
    /// and sha256 are not the line's).
    pub fn check_raw(&self, index: usize, raw: &[u8]) -> Result<()> {
        let line = self.piece(index)?;
        if raw.len() as u64 != line.stored.raw_length || sha256_hex(raw) != line.stored.sha256 {
            return Err(Error::Value(format!("{}: the bytes are not the ones written", line.piece.kind())));
        }
        Ok(())
    }
}
