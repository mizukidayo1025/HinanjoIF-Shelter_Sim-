//! The Python exceptions of the core's errors (`sheltersim_core::error::Error`): `to_py_err` makes
//! one into its Python class, and `OrPy` does it for a core call's result: `core_call().or_py()?`.

use pyo3::exceptions::{PyAttributeError, PyKeyError, PyOverflowError, PyRuntimeError, PyTypeError, PyValueError, PyZeroDivisionError};
use pyo3::prelude::*;

use sheltersim_core::error::{refused_text, Error};
use sheltersim_core::mathx::MathError;

/// The Python exception of a core error. A refusal of a rule is a ValueError with the refusal as
/// its cause; the outcomes of an event, a broken state and a ledger's error are ValueErrors too,
/// with the core's text.
pub(crate) fn to_py_err(error: Error) -> PyErr {
    match error {
        Error::Unsupported(reason) => crate::Unsupported::new_err(reason.into_owned()),
        Error::Refused { label, reason } => {
            let text = refused_text(&label, &reason);
            Python::with_gil(|py| {
                let refusal = PyValueError::new_err(text);
                refusal.set_cause(py, Some(crate::Unsupported::new_err(reason.into_owned())));
                refusal
            })
        }
        Error::Outcome(_, text) | Error::MarkedValue(text) => PyValueError::new_err(text),
        Error::Value(text) | Error::Validation(text) => PyValueError::new_err(text),
        Error::Key(text) => PyKeyError::new_err(text),
        Error::Runtime(text) => PyRuntimeError::new_err(text),
        Error::Type(text) => PyTypeError::new_err(text),
        Error::Attribute(text) => PyAttributeError::new_err(text),
        Error::Invariant(text) | Error::Ledger(text) => PyValueError::new_err(text),
        corrupt @ Error::Corrupt { .. } => PyValueError::new_err(corrupt.to_string()),
        Error::Math(error) => math_error(error),
        Error::Clock => PyRuntimeError::new_err(sheltersim_core::error::CLOCK),
    }
}

/// The exception CPython's math raises for a math error.
fn math_error(error: MathError) -> PyErr {
    let text = error.text();
    match error {
        MathError::Range | MathError::PowerRange => PyOverflowError::new_err(text),
        MathError::ZeroToNegative => PyZeroDivisionError::new_err(text),
        MathError::NegativeToFraction => PyValueError::new_err(text),
    }
}

/// A core call's result as Python's: `core_call().or_py()?`.
pub(crate) trait OrPy<T> {
    fn or_py(self) -> PyResult<T>;
}

impl<T> OrPy<T> for sheltersim_core::error::Result<T> {
    #[inline]
    fn or_py(self) -> PyResult<T> {
        self.map_err(to_py_err)
    }
}
