//! A Python object as the core's port to an LLM (`sheltersim_core::llm::LlmLink`, #428), for the
//! PyO3 `PlainRun` (`rules/plain.rs`): `offline`, a bool read once, and `complete(task: bytes) ->
//! bytes`, the JSON of the core's `llm::Task` and `llm::Reply`. `sheltersim.llm.link.RouterLink`
//! puts Python's `LLMRouter` behind it.
//!
//! What the object raises stops the tick: the core gets `LINK_RAISED`, and the exception waits in
//! `Raised` for the caller of the ticks, which raises it as the cause of its own error.

use std::cell::RefCell;
use std::rc::Rc;

use pyo3::prelude::*;
use pyo3::types::PyBytes;
use sheltersim_core::error::{Error, Result};
use sheltersim_core::llm::{LlmLink, Reply, Task};

/// The text of the core's error that stands for what the Python link raised.
pub(crate) const LINK_RAISED: &str = "the LLM link raised an exception";

/// What the Python link raised last, shared with the caller of the ticks.
pub(crate) type Raised = Rc<RefCell<Option<PyErr>>>;

/// A Python link.
pub(crate) struct PyLink {
    link: Py<PyAny>,
    offline: bool,
    raised: Raised,
}

impl PyLink {
    /// The link of `link`, its `offline` read now.
    pub(crate) fn new(py: Python<'_>, link: Py<PyAny>, raised: &Raised) -> PyResult<Self> {
        let offline = link.bind(py).getattr("offline")?.extract::<bool>()?;
        Ok(Self { link, offline, raised: Rc::clone(raised) })
    }
}

impl LlmLink for PyLink {
    fn offline(&self) -> bool {
        self.offline
    }

    fn complete(&mut self, task: &Task) -> Result<Reply> {
        let raw = task.to_json()?;
        let answered = Python::with_gil(|py| -> PyResult<Vec<u8>> {
            let reply = self.link.bind(py).call_method1("complete", (PyBytes::new(py, &raw),))?;
            reply.extract::<Vec<u8>>()
        });
        match answered {
            Ok(reply) => Reply::from_json(&reply),
            Err(error) => {
                *self.raised.borrow_mut() = Some(error);
                Err(Error::Runtime(LINK_RAISED.to_owned()))
            }
        }
    }
}
