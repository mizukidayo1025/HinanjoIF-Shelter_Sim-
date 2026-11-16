//! The errors a kernel raises, with their texts.
//!
//! Each variant stands for one class of error, named after the Python exception of the rule it was
//! ported from (#152); the PyO3 crate raises each as a Python exception with this text
//! (`native/src/core/errors.rs`, `to_py_err`). The texts are saved data: the dispatcher writes
//! `str(exc)` of an event's outcome to its record as `detail`, which is hashed and shown. So every
//! text is built here, and a changed text changes saved bytes.

use std::borrow::Cow;
use std::fmt;

use crate::mathx::MathError;

/// Which of an event's two expected outcomes.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Outcome {
    /// `EventSuppressed`.
    Suppressed,
    /// `EventApplicationError`.
    Failed,
}

/// An error of the core, by its class.
#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// `Unsupported(reason)`: a case the core does not run (`sheltersim_native.Unsupported` in
    /// the PyO3 crate).
    Unsupported(Cow<'static, str>),
    /// A labelled refusal: `{label} refused: {reason}` (`refused_text`), made of an
    /// `Unsupported(reason)` by `Error::refused` (lane L10). `label` is `module.name` of the
    /// core's function that refused (`tests/refusal_labels.rs` checks that it has one).
    Refused { label: String, reason: Cow<'static, str> },
    /// `EventSuppressed(text)` or `EventApplicationError(text)`, marked as made here.
    Outcome(Outcome, String),
    /// `ValueError(text)` marked as made here, so that a step which turns another call's error
    /// into Unsupported lets it through (a check of `effect_program_v1`).
    MarkedValue(String),
    /// `ValueError(text)` of a rule.
    Value(String),
    /// pydantic's `ValidationError` (a `ValueError`) with its text, where a rule lets it through
    /// as it is (`contracts::pydantic::text`; the whole world's validation, #250).
    Validation(String),
    /// `KeyError(text)`.
    Key(String),
    /// `RuntimeError(text)`.
    Runtime(String),
    /// `TypeError(text)`.
    Type(String),
    /// `AttributeError(text)`: a rule reads a value with a method its type lacks (`.get` of a
    /// list), which no rule catches (#360).
    Attribute(String),
    /// `StateInvariantError(text)`: a broken state a rule found.
    Invariant(String),
    /// `ResourceLedgerError(text)`: the contract of the resource ledger (`rules::resources`).
    Ledger(String),
    /// `StateInvariantError(corrupt_text(namespace, check))`: a state the rules' checks never let
    /// through (the C group of docs/rust-port/step4-plan.md), with a text of the core's own.
    Corrupt { namespace: String, check: String },
    /// The exception CPython raised in `exp`, `expm1` or `pow` (`mathx`).
    Math(MathError),
    /// `TickExecutionError(CLOCK)`: a tick phase moved the clock the engine
    /// owns (lane P9-0, for the core's tick driver).
    Clock,
}

/// The text of `Error::Clock`, as the tick driver raises it (`rules::tick::clock`).
pub const CLOCK: &str = "tick phase attempted to change the engine-owned clock";

/// What the core's fallible calls return.
pub type Result<T> = std::result::Result<T, Error>;

/// The text of a kernel's refusal (`Error::Refused`).
pub fn refused_text(label: &str, reason: &str) -> String {
    format!("{label} refused: {reason}")
}

/// The text of a corrupt state: it names the namespace and the check.
pub fn corrupt_text(namespace: &str, check: &str) -> String {
    format!("{namespace}: corrupt state: {check}")
}

impl Error {
    /// `Unsupported(reason)`.
    #[inline]
    pub fn unsupported(reason: impl Into<Cow<'static, str>>) -> Self {
        Error::Unsupported(reason.into())
    }

    /// A refusal made an `Error::Refused` that names the kernel; any other error goes on as it is.
    pub fn refused(label: impl Into<String>, error: Error) -> Self {
        match error {
            Error::Unsupported(reason) => Error::Refused { label: label.into(), reason },
            other => other,
        }
    }

    /// `raise EventSuppressed(text)`.
    #[inline]
    pub fn suppressed(text: impl Into<String>) -> Self {
        Error::Outcome(Outcome::Suppressed, text.into())
    }

    /// `raise EventApplicationError(text)`.
    #[inline]
    pub fn failed(text: impl Into<String>) -> Self {
        Error::Outcome(Outcome::Failed, text.into())
    }

    /// `raise ValueError(text)` of a rule.
    pub fn value_error(text: impl Into<String>) -> Self {
        Error::Value(text.into())
    }

    /// `raise StateInvariantError(text)` of a rule.
    pub fn invariant(text: impl Into<String>) -> Self {
        Error::Invariant(text.into())
    }

    /// `raise ResourceLedgerError(text)` of the ledger's operation.
    pub fn ledger(text: impl Into<String>) -> Self {
        Error::Ledger(text.into())
    }

    /// The one exit for a corrupt state.
    pub fn corrupt(namespace: impl Into<String>, check: impl Into<String>) -> Self {
        Error::Corrupt { namespace: namespace.into(), check: check.into() }
    }
}

impl From<MathError> for Error {
    fn from(error: MathError) -> Self {
        Error::Math(error)
    }
}

/// The error's text, `str(exc)` of the exception the PyO3 crate raises; a Python `KeyError`'s
/// `str` quotes its text, and this gives it bare.
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Unsupported(reason) => f.write_str(reason),
            Error::Refused { label, reason } => f.write_str(&refused_text(label, reason)),
            Error::Outcome(_, text)
            | Error::MarkedValue(text)
            | Error::Value(text)
            | Error::Validation(text)
            | Error::Key(text)
            | Error::Runtime(text)
            | Error::Type(text)
            | Error::Attribute(text)
            | Error::Invariant(text)
            | Error::Ledger(text) => f.write_str(text),
            Error::Corrupt { namespace, check } => f.write_str(&corrupt_text(namespace, check)),
            Error::Math(error) => f.write_str(error.text()),
            Error::Clock => f.write_str(CLOCK),
        }
    }
}

impl std::error::Error for Error {}

/// Why a value or a write does not fit a row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Misfit(pub String);

impl Misfit {
    pub fn new(reason: impl Into<String>) -> Self {
        Self(reason.into())
    }
}
