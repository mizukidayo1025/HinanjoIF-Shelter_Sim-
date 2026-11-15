//! The journal's lines: their typed values, their JSON, the head hash of a line, and the journal
//! read back (`Journal::parse`) with the checks of format v6.

use serde_json::{Map, Value};

use super::{corrupt, outdated, FORMAT, OUTDATED_FORMATS};
use crate::archive::SAVED_POINT_VERSION;
use crate::checkpoint::sha256_hex;
use crate::error::{Error, Result};
use crate::json::Encoder;
use crate::model_version::Versions;

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

/// How much of a packed store (the log's or the samples') a Run has stored: its tables' sizes the
/// last tables piece gave, and its blocks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Place {
    pub strings: u64,
    pub shapes: u64,
    pub blocks: u64,
}

/// How many records of each append-only kind a Run has stored (`RecordKind`): the LLM tape's
/// entries from the Run's start (a fork's Run counts its parent's up to its base), and the
/// conversation turns of the branch itself.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub conversations: u64,
    pub tape: u64,
}

/// What a Run has stored up to a line: the log's and the samples' places and the counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub log: Place,
    pub samples: Place,
    pub counts: Counts,
}

/// The kinds of append-only records a Run stores in pieces of their own (`records`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordKind {
    /// The turns of the questions about a branch's results (the browser's `questions.mjs`).
    Conversations,
    /// The LLM's responses a Run used (defined; nothing writes it yet).
    Tape,
}

impl RecordKind {
    pub const ALL: [RecordKind; 2] = [RecordKind::Conversations, RecordKind::Tape];

    /// The piece line's `kind`.
    pub fn name(self) -> &'static str {
        match self {
            RecordKind::Conversations => "conversations",
            RecordKind::Tape => "llm_tape",
        }
    }

    fn of(self, counts: &mut Counts) -> &mut u64 {
        match self {
            RecordKind::Conversations => &mut counts.conversations,
            RecordKind::Tape => &mut counts.tape,
        }
    }

    /// The count of this kind in `counts`.
    pub fn count(self, counts: &Counts) -> u64 {
        let mut counts = *counts;
        *self.of(&mut counts)
    }

    /// Adds `count` records of this kind to `counts`.
    pub fn add(self, counts: &mut Counts, count: u64) -> Result<()> {
        let total = self.of(counts);
        *total = total.checked_add(count).ok_or_else(|| corrupt("a count past any Run"))?;
        Ok(())
    }
}

/// What a "saved" line says of its saved point: the minute, the counts of records and samples, the
/// counts of the append-only records, the saved point's version, and how many log and sample
/// blocks the Run has stored (all of them: the blocks are sealed before a saved point).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SavedLine {
    pub minute: i64,
    pub records: u64,
    pub samples: u64,
    pub counts: Counts,
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
    Records { kind: RecordKind, count: u64 },
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
            Piece::Records { kind, .. } => kind.name(),
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

/// The fork a branch's Run starts at: the minute and the branch it was made from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForkStart {
    pub minute: i64,
    pub parent_branch_id: String,
}

/// The "run" line: the format, what the Run was computed with, how it is stored, and what it is
/// of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunLine {
    /// `engine:model` of the code that wrote it (`model_version::ENGINE_VERSION`).
    pub engine_version: String,
    /// The model versions the Run is computed with (`model_signatures` of its Scenario).
    pub model_versions: Versions,
    /// The name of the host's compression of the pieces ("deflate-raw" in the browser).
    pub compression: String,
    /// Minutes between saved points (0: a saved point at the end only).
    pub save_every: u64,
    /// The sha256 of the exported world's bytes, and their length.
    pub export_sha256: String,
    pub export_bytes: u64,
    /// The world's name in a file of named worlds (the fixtures), or None for an exported entry.
    pub world: Option<String>,
    pub duration_minutes: i64,
    /// The fork a branch's Run starts at.
    pub fork: Option<ForkStart>,
}

/// How a new Run is stored: the model versions it is computed with, the host's compression and
/// the minutes between saved points.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Storing {
    pub model_versions: Versions,
    pub compression: String,
    pub save_every: u64,
}

impl RunLine {
    /// The line of a new Run from the exported world `export`.
    pub fn new(export: &[u8], world: Option<String>, duration_minutes: i64, storing: &Storing) -> Self {
        Self {
            engine_version: crate::model_version::ENGINE_VERSION.to_owned(),
            model_versions: storing.model_versions.clone(),
            compression: storing.compression.clone(),
            save_every: storing.save_every,
            export_sha256: sha256_hex(export),
            export_bytes: export.len() as u64,
            world,
            duration_minutes,
            fork: None,
        }
    }

    /// The line of a Run from minute 0 of the same exported world as `source`, computed with the
    /// same versions and stored as it is.
    pub fn same_start(source: &RunLine) -> Self {
        Self { engine_version: crate::model_version::ENGINE_VERSION.to_owned(), fork: None, ..source.clone() }
    }

    /// The line of the Run of a branch forked from `parent`'s Run at `minute`.
    pub fn forked(parent: &RunLine, minute: i64, parent_branch_id: &str) -> Self {
        Self { fork: Some(ForkStart { minute, parent_branch_id: parent_branch_id.to_owned() }), ..Self::same_start(parent) }
    }

    /// The minute the Run starts at: the fork's, else 0.
    pub fn fork_minute(&self) -> i64 {
        self.fork.as_ref().map_or(0, |fork| fork.minute)
    }

    /// Whether `other` is the line of a Run of the same computation: the same exported world,
    /// clock, versions and storing.
    pub fn same_run(&self, other: &RunLine) -> bool {
        (&self.model_versions, &self.compression, self.save_every, &self.export_sha256, self.export_bytes, &self.world, self.duration_minutes)
            == (&other.model_versions, &other.compression, other.save_every, &other.export_sha256, other.export_bytes, &other.world, other.duration_minutes)
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

/// The line of the parent's stored Run a fork's Run goes on from (#368, decision 6): its index in
/// the parent's journal, its head hash, its minute, and what the parent had stored up to it. The
/// fork's Run reads the parent's pieces up to that line instead of a copy of them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Base {
    pub line: u64,
    pub head: String,
    pub minute: i64,
    /// Its conversation turns are the parent branch's, not counted.
    pub state: State,
}

/// The "fork" line of a branch made by a fork: its origin, its base in the parent's stored Run,
/// the EffectProgram applied (its JSON text), the approval's hash and the receipt (its JSON text).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForkLine {
    pub parent_branch_id: String,
    pub minute: i64,
    pub origin: ForkOrigin,
    pub base: Base,
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
    pub counts: Counts,
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

/// A line as it is written: its bytes (the JSON, a tab, the sha256, "\n"), the sha256 of its JSON
/// and its head hash (`Line::head_json`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Encoded {
    pub bytes: Vec<u8>,
    pub sha256: String,
    pub head: String,
}

fn key(encoder: &mut Encoder, name: &str) {
    if encoder.out.len() > 1 {
        encoder.out.push(b',');
    }
    encoder.string(name);
    encoder.out.push(b':');
}

fn uints(encoder: &mut Encoder, fields: &[(&str, u64)]) {
    for (name, value) in fields {
        key(encoder, name);
        encoder.out.extend_from_slice(itoa::Buffer::new().format(*value).as_bytes());
    }
}

fn string_or_null(encoder: &mut Encoder, text: Option<&str>) {
    match text {
        Some(text) => encoder.string(text),
        None => encoder.null(),
    }
}

fn counts_fields(counts: &Counts) -> [(&'static str, u64); 2] {
    [("conversations", counts.conversations), ("tape", counts.tape)]
}

const BASE_FIELDS: [&str; 7] = ["log_strings", "log_shapes", "log_blocks", "sample_strings", "sample_shapes", "sample_blocks", "tape"];

fn base_fields(state: &State) -> [(&'static str, u64); 7] {
    let values = [state.log.strings, state.log.shapes, state.log.blocks, state.samples.strings, state.samples.shapes, state.samples.blocks, state.counts.tape];
    std::array::from_fn(|index| (BASE_FIELDS[index], values[index]))
}

/// A JSON object written in its own encoder.
fn object(fill: impl FnOnce(&mut Encoder)) -> Vec<u8> {
    let mut out = Encoder::with_capacity(256);
    out.out.push(b'{');
    fill(&mut out);
    out.out.push(b'}');
    out.out
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

    /// The line's JSON as the journal holds it: the keys in this writer's order, no spaces, strings
    /// escaped as `json::write_json_string` does (as the browser's `JSON.stringify` does).
    pub fn to_json(&self) -> Vec<u8> {
        self.json(true)
    }

    /// The line's JSON without where its piece is in data.pack (`offset`, `length`): what it holds,
    /// whatever the host's compression made of the piece. Its sha256 is the line's head hash, which
    /// a branch's record names (#368, decision 1): hosts that compress differently write the same
    /// heads.
    pub fn head_json(&self) -> Vec<u8> {
        self.json(false)
    }

    fn json(&self, place: bool) -> Vec<u8> {
        object(|out| {
            key(out, "kind");
            out.string(self.kind());
            match self {
                Line::Run(run) => {
                    key(out, "format");
                    out.string(FORMAT);
                    key(out, "engine_version");
                    out.string(&run.engine_version);
                    key(out, "model_versions");
                    let versions = object(|versions| {
                        for (name, value) in &run.model_versions {
                            key(versions, name);
                            versions.string(value);
                        }
                    });
                    out.out.extend_from_slice(&versions);
                    key(out, "compression");
                    out.string(&run.compression);
                    uints(out, &[("save_every", run.save_every)]);
                    key(out, "export_sha256");
                    out.string(&run.export_sha256);
                    uints(out, &[("export_bytes", run.export_bytes)]);
                    key(out, "world");
                    string_or_null(out, run.world.as_deref());
                    key(out, "duration_minutes");
                    out.int(run.duration_minutes);
                    if let Some(fork) = &run.fork {
                        key(out, "fork_minute");
                        out.int(fork.minute);
                        key(out, "parent_branch_id");
                        out.string(&fork.parent_branch_id);
                    }
                }
                Line::Fork(fork) => {
                    key(out, "parent_branch_id");
                    out.string(&fork.parent_branch_id);
                    key(out, "minute");
                    out.int(fork.minute);
                    key(out, "fork_origin");
                    let origin = object(|origin| {
                        key(origin, "branch_id");
                        origin.string(&fork.origin.branch_id);
                        key(origin, "minute");
                        origin.int(fork.origin.minute);
                        key(origin, "state_hash");
                        origin.string(&fork.origin.state_hash);
                        key(origin, "checkpoint_hash");
                        string_or_null(origin, fork.origin.checkpoint_hash.as_deref());
                    });
                    out.out.extend_from_slice(&origin);
                    key(out, "base");
                    let base = object(|base| {
                        uints(base, &[("line", fork.base.line)]);
                        key(base, "head");
                        base.string(&fork.base.head);
                        key(base, "minute");
                        base.int(fork.base.minute);
                        uints(base, &base_fields(&fork.base.state));
                    });
                    out.out.extend_from_slice(&base);
                    key(out, "program");
                    out.string(&fork.program);
                    key(out, "approved_program_hash");
                    out.string(&fork.approved_program_hash);
                    key(out, "receipt");
                    out.string(&fork.receipt);
                }
                Line::Piece(line) => {
                    let stored = &line.stored;
                    if place {
                        uints(out, &[("offset", stored.offset), ("length", stored.length)]);
                    }
                    uints(out, &[("raw_length", stored.raw_length)]);
                    key(out, "sha256");
                    out.string(&stored.sha256);
                    match &line.piece {
                        Piece::Export | Piece::World => {}
                        Piece::LogTables(tables) | Piece::SampleTables(tables) => uints(
                            out,
                            &[
                                ("strings_from", tables.strings_from),
                                ("shapes_from", tables.shapes_from),
                                ("strings", tables.strings),
                                ("shapes", tables.shapes),
                            ],
                        ),
                        Piece::LogBlock { index } | Piece::SampleBlock { index } => uints(out, &[("index", *index)]),
                        Piece::Records { count, .. } => uints(out, &[("count", *count)]),
                        Piece::Saved(saved) => {
                            key(out, "minute");
                            out.int(saved.minute);
                            uints(out, &[("records", saved.records), ("samples", saved.samples)]);
                            uints(out, &counts_fields(&saved.counts));
                            uints(out, &[("point_version", saved.point_version), ("log_blocks", saved.log_blocks), ("sample_blocks", saved.sample_blocks)]);
                        }
                    }
                }
                Line::End(end) => {
                    key(out, "minute");
                    out.int(end.minute);
                    uints(out, &[("records", end.records), ("samples", end.samples)]);
                    uints(out, &counts_fields(&end.counts));
                    key(out, "parts");
                    let parts = object(|parts| {
                        for (part, digest) in &end.parts {
                            key(parts, part);
                            parts.string(digest);
                        }
                    });
                    out.out.extend_from_slice(&parts);
                    key(out, "metrics");
                    out.string(&end.metrics);
                }
            }
        })
    }

    /// The sha256 of `head_json`: the line's head hash.
    pub fn head_hash(&self) -> String {
        sha256_hex(&self.head_json())
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
        Encoded { bytes, sha256, head: self.head_hash() }
    }

    /// The line of `json` (a journal line's JSON whose sha256 matched): an error for JSON that is
    /// not a line of this format (a key it does not have included), and for the "run" line of an
    /// older format, `stored_run_outdated` (the Run is computed again: `outdated`).
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
        let piece = |piece: Piece, more: &[&str]| -> Result<Line> {
            object.only(&[&["kind", "offset", "length", "raw_length", "sha256"][..], more].concat())?;
            Ok(Line::Piece(PieceLine { piece, stored: stored()? }))
        };
        const TABLES: [&str; 4] = ["strings_from", "shapes_from", "strings", "shapes"];
        let tables = || -> Result<Tables> {
            Ok(Tables {
                strings_from: object.uint("strings_from")?,
                shapes_from: object.uint("shapes_from")?,
                strings: object.uint("strings")?,
                shapes: object.uint("shapes")?,
            })
        };
        let counts = || -> Result<Counts> { Ok(Counts { conversations: object.uint("conversations")?, tape: object.uint("tape")? }) };
        match kind.as_str() {
            "run" => {
                let format = object.string("format")?;
                if OUTDATED_FORMATS.contains(&format.as_str()) {
                    return Err(outdated(&format));
                }
                if format != FORMAT {
                    return Err(Error::Value(format!("a stored Run of format {format}: this code reads {FORMAT}")));
                }
                let mut keys = vec![
                    "kind",
                    "format",
                    "engine_version",
                    "model_versions",
                    "compression",
                    "save_every",
                    "export_sha256",
                    "export_bytes",
                    "world",
                    "duration_minutes",
                ];
                let fork = match object.optional("fork_minute") {
                    Some(_) => {
                        keys.extend(["fork_minute", "parent_branch_id"]);
                        Some(ForkStart { minute: object.minute("fork_minute")?, parent_branch_id: object.string("parent_branch_id")? })
                    }
                    None => None,
                };
                object.only(&keys)?;
                let versions = Fields::of(object.get("model_versions")?, "model_versions")?;
                let model_versions = versions
                    .object
                    .iter()
                    .map(|(name, value)| {
                        value.as_str().map(|value| (name.clone(), value.to_owned())).ok_or_else(|| corrupt("a model version that is not text"))
                    })
                    .collect::<Result<Versions>>()?;
                Ok(Line::Run(RunLine {
                    engine_version: object.string("engine_version")?,
                    model_versions,
                    compression: object.string("compression")?,
                    save_every: object.uint("save_every")?,
                    export_sha256: object.string("export_sha256")?,
                    export_bytes: object.uint("export_bytes")?,
                    world: object.string_or_null("world")?,
                    duration_minutes: object.minute("duration_minutes")?,
                    fork,
                }))
            }
            "fork" => {
                object.only(&["kind", "parent_branch_id", "minute", "fork_origin", "base", "program", "approved_program_hash", "receipt"])?;
                let origin = Fields::of(object.get("fork_origin")?, "fork_origin")?;
                origin.only(&["branch_id", "minute", "state_hash", "checkpoint_hash"])?;
                let base = Fields::of(object.get("base")?, "base")?;
                base.only(&[&["line", "head", "minute"][..], &BASE_FIELDS].concat())?;
                let place = |prefix: &str| -> Result<Place> {
                    Ok(Place {
                        strings: base.uint(&format!("{prefix}_strings"))?,
                        shapes: base.uint(&format!("{prefix}_shapes"))?,
                        blocks: base.uint(&format!("{prefix}_blocks"))?,
                    })
                };
                Ok(Line::Fork(ForkLine {
                    parent_branch_id: object.string("parent_branch_id")?,
                    minute: object.minute("minute")?,
                    origin: ForkOrigin {
                        branch_id: origin.string("branch_id")?,
                        minute: origin.minute("minute")?,
                        state_hash: origin.string("state_hash")?,
                        checkpoint_hash: origin.string_or_null("checkpoint_hash")?,
                    },
                    base: Base {
                        line: base.uint("line")?,
                        head: base.string("head")?,
                        minute: base.minute("minute")?,
                        state: State { log: place("log")?, samples: place("sample")?, counts: Counts { conversations: 0, tape: base.uint("tape")? } },
                    },
                    program: object.string("program")?,
                    approved_program_hash: object.string("approved_program_hash")?,
                    receipt: object.string("receipt")?,
                }))
            }
            "end" => {
                object.only(&["kind", "minute", "records", "samples", "conversations", "tape", "parts", "metrics"])?;
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
                    counts: counts()?,
                    parts,
                    metrics: object.string("metrics")?,
                }))
            }
            "export" => piece(Piece::Export, &[]),
            "world" => piece(Piece::World, &[]),
            "log_tables" => piece(Piece::LogTables(tables()?), &TABLES),
            "sample_tables" => piece(Piece::SampleTables(tables()?), &TABLES),
            "log_block" => piece(Piece::LogBlock { index: object.uint("index")? }, &["index"]),
            "sample_block" => piece(Piece::SampleBlock { index: object.uint("index")? }, &["index"]),
            "saved" => piece(
                Piece::Saved(SavedLine {
                    minute: object.minute("minute")?,
                    records: object.uint("records")?,
                    samples: object.uint("samples")?,
                    counts: counts()?,
                    point_version: object.uint("point_version")?,
                    log_blocks: object.uint("log_blocks")?,
                    sample_blocks: object.uint("sample_blocks")?,
                }),
                &["minute", "records", "samples", "conversations", "tape", "point_version", "log_blocks", "sample_blocks"],
            ),
            other => match RecordKind::ALL.into_iter().find(|kind| kind.name() == other) {
                Some(kind) => piece(Piece::Records { kind, count: object.uint("count")? }, &["count"]),
                None => Err(corrupt(&format!("a line of an unknown kind: {other}"))),
            },
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

    /// An error for a key that is not one of `keys`.
    fn only(&self, keys: &[&str]) -> Result<()> {
        match self.object.keys().find(|name| !keys.contains(&name.as_str())) {
            Some(name) => Err(corrupt(&format!("{} with a key this format does not have: {name}", self.what))),
            None => Ok(()),
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
        match self.get(name)? {
            Value::Null => Ok(None),
            Value::String(text) => Ok(Some(text.clone())),
            _ => Err(self.bad(name, "text or null")),
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

/// A line read back: the line, its JSON text and sha256 as they are in the journal, its head hash,
/// and the byte just after it (its "\n").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub line: Line,
    pub json: String,
    pub sha256: String,
    pub head: String,
    pub end: u64,
}

/// A journal read back: its lines up to the first that is cut short or does not match its sha256
/// (`torn` then), and what the Run had stored after each line (`state_at`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Journal {
    entries: Vec<Entry>,
    states: Vec<State>,
    torn: bool,
    /// The minute of the last "saved" or "end" line.
    last_minute: Option<i64>,
}

impl Journal {
    /// The lines of a journal's bytes, up to the first that is cut short (no "\n", or no tab) or
    /// whose sha256 is not its JSON's. A line that matches its sha256 was written whole, so one
    /// that is not a line of this format there is an error, not the journal's end
    /// (`Journal::check_next` says which).
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
            let state = journal.check_next(&line)?;
            journal.states.push(state);
            journal.entries.push(Entry {
                head: line.head_hash(),
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

    /// What the Run has stored after `line` comes next, or an error where it cannot come next:
    /// - the first line is the "run" line, and only it is; the second line of a fork's Run is its
    ///   "fork" line (of the run line's fork), of a Run from minute 0 its "export" piece, and
    ///   neither is anywhere else;
    /// - a tables piece follows the tables stored, a block is the next one;
    /// - a "saved" line is of a later minute than the one before, within the Run's clock, of the
    ///   saved point's version, and its counts are what the Run has stored before it: every block
    ///   (the blocks are sealed before a saved point, #368 decision 2) and every record;
    /// - the "world" piece comes just before the "end" line, whose counts are what is stored;
    /// - after the "end" line, only conversation turns.
    fn check_next(&mut self, line: &Line) -> Result<State> {
        let index = self.entries.len();
        let run = match self.entries.first().map(|entry| &entry.line) {
            None if matches!(line, Line::Run(_)) => return Ok(State::default()),
            Some(Line::Run(run)) => run.clone(),
            _ => return Err(corrupt("its first line is not the run line")),
        };
        let state = *self.states.last().unwrap_or(&State::default());
        let previous = self.entries.last().map(|entry| &entry.line);
        if matches!(previous, Some(Line::End(_)))
            && !matches!(line, Line::Piece(PieceLine { piece: Piece::Records { kind: RecordKind::Conversations, .. }, .. }))
        {
            return Err(corrupt("a line after the end line"));
        }
        let after_world = matches!(previous, Some(Line::Piece(PieceLine { piece: Piece::World, .. })));
        if after_world && !matches!(line, Line::End(_)) {
            return Err(corrupt("a line between the world and the end line"));
        }
        if !after_world && matches!(line, Line::End(_)) {
            return Err(corrupt("an end line that does not follow the world"));
        }
        match line {
            Line::Run(_) => Err(corrupt("a second run line")),
            Line::Fork(fork) => {
                let start = run.fork.as_ref().filter(|_| index == 1).ok_or_else(|| corrupt("a fork line that is not the second line of a fork's Run"))?;
                if (start.minute, &start.parent_branch_id) != (fork.minute, &fork.parent_branch_id) || fork.base.minute > fork.minute {
                    return Err(corrupt("a fork line of another fork"));
                }
                Ok(fork.base.state)
            }
            Line::Piece(PieceLine { piece: Piece::Export, .. }) if index != 1 || run.fork.is_some() => {
                Err(corrupt("an exported world that is not the second line of a Run from minute 0"))
            }
            Line::Piece(PieceLine { piece: Piece::Export, .. }) => Ok(state),
            _ if index == 1 => {
                Err(corrupt(if run.fork.is_some() { "a fork's Run without its fork line" } else { "a Run from minute 0 without its exported world" }))
            }
            _ => self.next_state(state, line, &run),
        }
    }

    fn next_state(&mut self, mut state: State, line: &Line, run: &RunLine) -> Result<State> {
        fn follows(place: &mut Place, tables: &Tables) -> Result<()> {
            if (tables.strings_from, tables.shapes_from) != (place.strings, place.shapes)
                || tables.strings < tables.strings_from
                || tables.shapes < tables.shapes_from
            {
                return Err(corrupt("tables that do not follow the ones stored"));
            }
            (place.strings, place.shapes) = (tables.strings, tables.shapes);
            Ok(())
        }
        fn next(place: &mut Place, index: u64) -> Result<()> {
            if index != place.blocks {
                return Err(corrupt("a block that is not the next one"));
            }
            place.blocks += 1;
            Ok(())
        }
        let mut at_minute = |minute: i64, strictly: bool| -> Result<()> {
            let later = self.last_minute.is_none_or(|last| if strictly { minute > last } else { minute >= last });
            if !later || minute < run.fork_minute() || minute > run.duration_minutes {
                return Err(corrupt(&format!("a saved point or an end at minute {minute}: out of order, or out of the Run")));
            }
            self.last_minute = Some(minute);
            Ok(())
        };
        match line {
            Line::Run(_) | Line::Fork(_) => return Err(corrupt("a run or fork line out of its place")),
            Line::End(end) => {
                at_minute(end.minute, false)?;
                if end.counts != state.counts {
                    return Err(corrupt("an end line whose counts are not the records stored"));
                }
            }
            Line::Piece(piece) => match &piece.piece {
                Piece::Export | Piece::World => {}
                Piece::LogTables(tables) => follows(&mut state.log, tables)?,
                Piece::SampleTables(tables) => follows(&mut state.samples, tables)?,
                Piece::LogBlock { index } => next(&mut state.log, *index)?,
                Piece::SampleBlock { index } => next(&mut state.samples, *index)?,
                Piece::Records { kind, count } => kind.add(&mut state.counts, *count)?,
                Piece::Saved(saved) => {
                    at_minute(saved.minute, true)?;
                    if saved.point_version != SAVED_POINT_VERSION {
                        return Err(Error::Value(format!("a saved point of version {}: this core reads version {SAVED_POINT_VERSION}", saved.point_version)));
                    }
                    if (saved.log_blocks, saved.sample_blocks, saved.counts) != (state.log.blocks, state.samples.blocks, state.counts) {
                        return Err(corrupt("a saved point whose counts are not the blocks and records stored before it"));
                    }
                }
            },
        }
        Ok(state)
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

    /// What the Run had stored after the line `index` (a fork's Run from its base on).
    pub fn state_at(&self, index: usize) -> Result<State> {
        self.states.get(index).copied().ok_or_else(|| Error::Value(format!("the journal has no line {index}")))
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

    /// The index of the "end" line, or None for a Run not stored to its end.
    pub fn end_index(&self) -> Option<usize> {
        self.entries.iter().rposition(|entry| matches!(entry.line, Line::End(_)))
    }

    /// The "end" line, or None for a Run not stored to its end.
    pub fn end(&self) -> Option<&EndLine> {
        self.end_index().and_then(|index| match &self.entries[index].line {
            Line::End(end) => Some(end),
            _ => None,
        })
    }

    /// The "fork" line of a branch made by a fork.
    pub fn fork(&self) -> Option<&ForkLine> {
        match self.entries.get(1).map(|entry| &entry.line) {
            Some(Line::Fork(fork)) => Some(fork),
            _ => None,
        }
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
