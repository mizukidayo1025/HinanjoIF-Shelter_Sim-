//! The seek of a stored Run (#501): the Run at a minute, restored from the nearest saved point at or
//! before it (with the stored log and samples it needs) or built from the exported world at minute
//! 0, then computed to the minute, the ticks of a Run that kept an LLM tape answered from the tape's
//! entries after the restored point's count (`llm::Replay`, #428): a seek never asks a provider.
//!
//! The seek is taken once here, without I/O, for every reader: the computing procedure's
//! (`Procedure`, going on and forking), the store's (`sheltersim_store::stored::StoredRun::seek_with`)
//! and the browser's (`js/run_store.mjs`'s `RunReader.seek`). `Seeker::next` says which piece of the
//! chain to read (a position of `Chain::pieces`), the reader reads it, decompresses it and gives its
//! bytes (`give`), which are checked against their line; once there is nothing more to read the Run
//! is restored, and `finish` computes it to the minute.

use crate::answers::{Answer, Failure, Refusal};
use crate::archive::Restore;
use crate::error::Error;
use crate::llm::{Limits, Replay};
use crate::plain::PlainRun;
use crate::run_store::{records, Chain, RecordKind, Seek};

/// How a seek answers the LLM's calls of a Run that kept a tape: whether the Run was offline, and
/// the limits its rules asked within.
#[derive(Clone, Copy, Debug, Default)]
pub struct Replaying {
    pub offline: bool,
    pub limits: Limits,
}

/// Where a seek put the Run: `run` at the minute asked for, computed from `from` (a minute), the
/// branch's line `base` it was restored from (the base a fork made there goes on from).
pub struct Sought {
    pub run: PlainRun,
    pub from: i64,
    pub base: usize,
}

/// What the seek reads next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reading {
    Export,
    Point,
    /// The log's and the samples' pieces the restore needs, from this position on.
    Restore(usize),
    /// The tape's pieces, from this one of them on.
    Tape(usize),
    /// Nothing more: the Run is restored.
    Restored,
}

/// A seek of a stored Run's chain to a minute. See the module's documentation.
pub struct Seeker {
    seek: Seek,
    world: Option<String>,
    replaying: Replaying,
    reading: Reading,
    /// The piece asked for and not yet given: what it is read for, and its position.
    asked: Option<(Reading, usize)>,
    export: Option<Vec<u8>>,
    restore: Option<Restore>,
    tape_pieces: Vec<usize>,
    tape: Vec<Vec<u8>>,
    /// How many of the tape's entries the saved point restored counts (none from minute 0).
    tape_from: usize,
    run: Option<PlainRun>,
}

fn refused(text: impl Into<String>) -> Failure {
    Failure::Refused(Refusal::storage(text))
}

impl Seeker {
    /// The seek of `chain` (with a fork's parents') to `minute`: an error for a minute out of the
    /// Run, and for a fork's Run without a saved point to start from (`Chain::seek`).
    pub fn new(chain: &Chain, minute: i64, replaying: Replaying) -> Answer<Self> {
        let seek = chain.seek(minute)?;
        let tape_from = match seek.from {
            Some(point) => chain.leaf().saved(point)?.counts.tape as usize,
            None => 0,
        };
        Ok(Self {
            seek,
            world: chain.leaf().run()?.world.clone(),
            replaying,
            reading: Reading::Export,
            asked: None,
            export: None,
            restore: None,
            tape_pieces: chain.records(RecordKind::Tape)?,
            tape: Vec::new(),
            tape_from,
            run: None,
        })
    }

    /// The exported world's bytes, read and checked before (a reader keeps them between its
    /// seeks): the seek does not ask for them. Before the first `next`.
    pub fn give_export(&mut self, export: Vec<u8>) {
        self.export = Some(export);
    }

    /// Where the seek starts (`Chain::seek`).
    pub fn seek(&self) -> &Seek {
        &self.seek
    }

    /// The position in `chain` of the piece to read next, decompressed, and give (`give`); the same
    /// until it is given. None once the Run is restored: then `finish`.
    pub fn next(&mut self, chain: &Chain) -> Answer<Option<usize>> {
        if let Some((_, position)) = self.asked {
            return Ok(Some(position));
        }
        loop {
            let position = match self.reading {
                Reading::Export => match self.export {
                    None => self.seek.export,
                    Some(_) => {
                        self.reading = if self.seek.from.is_some() { Reading::Point } else { Reading::Tape(0) };
                        continue;
                    }
                },
                Reading::Point => {
                    let point = self.seek.from.expect("a saved point");
                    let leaf = chain.levels().len() - 1;
                    chain
                        .pieces()
                        .iter()
                        .position(|at| at.level == leaf && at.index == point)
                        .ok_or_else(|| Error::Value(format!("the chain has no saved point at line {point}")))?
                }
                Reading::Restore(start) => match chain.next_restore_read(start, self.restore.as_ref().expect("a restore"))? {
                    Some(position) => position,
                    None => {
                        let export = self.export.as_deref().expect("read");
                        let entry = match &self.world {
                            Some(name) => PlainRun::entry_of_worlds_json(export, name)?,
                            None => PlainRun::entry_of_json(export)?,
                        };
                        self.run = Some(PlainRun::restore(entry, self.restore.take().expect("a restore"))?);
                        self.reading = Reading::Tape(0);
                        continue;
                    }
                },
                Reading::Tape(index) => match self.tape_pieces.get(index) {
                    Some(&position) => position,
                    None => {
                        self.restored()?;
                        self.reading = Reading::Restored;
                        continue;
                    }
                },
                Reading::Restored => return Ok(None),
            };
            self.asked = Some((self.reading, position));
            return Ok(Some(position));
        }
    }

    /// The bytes of the piece `next` asked for, decompressed: checked against its line, then read.
    pub fn give(&mut self, chain: &Chain, raw: Vec<u8>) -> Answer<()> {
        let (reading, position) = self.asked.take().ok_or_else(|| Error::Value("the seek was given a piece it did not ask for".to_owned()))?;
        match reading {
            Reading::Export => {
                chain.check_raw(position, &raw)?;
                self.export = Some(raw);
            }
            Reading::Point => {
                chain.check_raw(position, &raw)?;
                self.restore = Some(Restore::new(&raw)?);
                self.reading = Reading::Restore(0);
            }
            Reading::Restore(_) => {
                chain.restore_read(position, &raw, self.restore.as_mut().expect("a restore"))?;
                self.reading = Reading::Restore(position + 1);
            }
            Reading::Tape(index) => {
                chain.check_raw(position, &raw)?;
                self.tape.extend(records::read(&raw)?);
                self.reading = Reading::Tape(index + 1);
            }
            Reading::Restored => unreachable!("nothing is asked once the Run is restored"),
        }
        Ok(())
    }

    /// The Run built from the exported world where no saved point was restored, then its LLM's
    /// calls answered from the tape's entries after the restored point's count.
    fn restored(&mut self) -> Answer<()> {
        if self.run.is_none() {
            let export = self.export.as_deref().expect("read");
            self.run = Some(match &self.world {
                Some(name) => PlainRun::of_worlds_json(export, name)?,
                None => PlainRun::of_json(export)?,
            });
        }
        if !self.tape.is_empty() {
            let entries = self.tape.get(self.tape_from..).ok_or_else(|| refused("the stored Run's LLM tape is shorter than its saved point counts"))?;
            let replay = Replay::new(entries, self.replaying.offline)?;
            self.run.as_mut().expect("the Run").set_llm(Box::new(replay), self.replaying.limits)?;
            self.tape = Vec::new();
        }
        Ok(())
    }

    /// The Run restored (`next` gave None) computed to the minute.
    pub fn finish(mut self) -> Answer<Sought> {
        let minute = self.seek.minute;
        let mut run = self.run.take().ok_or_else(|| Error::Value("the seek has not restored the Run yet".to_owned()))?;
        let ticks = minute - run.minute()?;
        let ran = if ticks < 0 { None } else { run.advance(ticks as u64).ok() };
        if ran != Some(ticks as u64) {
            return Err(refused(format!("the Run does not reach minute {minute}")));
        }
        Ok(Sought { run, from: self.seek.from_minute, base: self.seek.base })
    }
}
