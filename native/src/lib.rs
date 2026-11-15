//! The Python entries of the Rust core (`sheltersim_core`) for the local and VM versions (#368,
//! #430): a Run of the core (`rules::plain`), the files of a stored Run (`rules::run_files`), the
//! answers of the Project routes (`rules::answers`), the minute-0 entry and records of a new Run of
//! a Scenario, and the offline parse of a Scenario's text. Python keeps no simulation of its own:
//! every computation is the core's, as in the browser (WASM, `native/wasm`).
//!
//! Each entry reads its arguments, calls the core and converts what it gives: bytes as `bytes`,
//! JSON the core wrote as `str`, an error as its Python class (`core::errors::to_py_err`).

use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use pyo3::types::PyBytes;

use crate::core::errors::OrPy;

mod core;

// The `rules` modules, found by build.rs.
include!(concat!(env!("OUT_DIR"), "/lanes.rs"));

mod exception {
    use super::*;
    create_exception!(sheltersim_native, Unsupported, PyException);
}

/// A value the core does not compute (`sheltersim_core::error::Error::Unsupported`): an input outside
/// what the browser and the server send.
pub(crate) use exception::Unsupported;

/// The core's offline parse of a Scenario text (`sheltersim_core::scenario::text::parse`, #294),
/// as canonical JSON, for the tests: ValueError where Python raises, Unsupported where the core
/// does not compute the request.
#[pyfunction]
fn scenario_text_parse(text: &str) -> PyResult<String> {
    use sheltersim_core::scenario::request::Refusal;
    match sheltersim_core::scenario::text::parse(text) {
        Ok(parsed) => {
            let bytes = parsed.to_value().to_json().or_py()?;
            Ok(String::from_utf8(bytes).expect("canonical JSON is UTF-8"))
        }
        Err(Refusal::Invalid(why)) => Err(pyo3::exceptions::PyValueError::new_err(why)),
        Err(Refusal::Unsupported(why)) => Err(Unsupported::new_err(why)),
    }
}

/// The core's entry of a Scenario's world at minute 0, from the Scenario's canonical JSON
/// (`sheltersim_core::initial::entry`, #249, #429), which `PlainRun` and the stored Runs start from.
#[pyfunction]
fn initial_entry<'py>(py: Python<'py>, scenario: &[u8]) -> PyResult<Bound<'py, PyBytes>> {
    let bytes = sheltersim_core::initial::entry(scenario).or_py()?;
    Ok(PyBytes::new(py, &bytes))
}

/// The core's `model_versions` of a new Run of a Scenario (`sheltersim_core::initial::model_versions`,
/// #301, #429), as canonical JSON.
#[pyfunction]
fn initial_model_versions(scenario: &[u8]) -> PyResult<String> {
    let bytes = sheltersim_core::initial::model_versions(scenario).or_py()?.to_json().or_py()?;
    Ok(String::from_utf8(bytes).expect("canonical JSON is UTF-8"))
}

/// The core's evidence of a new Run of a Scenario (`sheltersim_core::initial::evidence`, #301), as
/// canonical JSON bytes.
#[pyfunction]
fn initial_evidence<'py>(py: Python<'py>, scenario: &[u8]) -> PyResult<Bound<'py, PyBytes>> {
    let bytes = sheltersim_core::initial::evidence(scenario).or_py()?;
    Ok(PyBytes::new(py, &bytes))
}

/// Each person's compute of a tick goes to the core's helper threads from this many people on
/// (`sheltersim_core::parallel`); the gate's variants set 1. Returns the value before.
#[pyfunction]
fn set_people_per_helper(people: usize) -> usize {
    sheltersim_core::parallel::set_people_per_helper(people)
}

/// For the tests: `(helper threads, helpers for this many people)`.
#[pyfunction]
fn parallel_state(people: usize) -> (usize, usize) {
    sheltersim_core::parallel::parallel_state(people)
}

#[pymodule]
fn sheltersim_native(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add("Unsupported", module.py().get_type::<Unsupported>())?;
    module.add_function(wrap_pyfunction!(scenario_text_parse, module)?)?;
    module.add_function(wrap_pyfunction!(initial_entry, module)?)?;
    module.add_function(wrap_pyfunction!(initial_model_versions, module)?)?;
    module.add_function(wrap_pyfunction!(initial_evidence, module)?)?;
    module.add_function(wrap_pyfunction!(set_people_per_helper, module)?)?;
    module.add_function(wrap_pyfunction!(parallel_state, module)?)?;
    register_lanes(module)?;
    Ok(())
}
