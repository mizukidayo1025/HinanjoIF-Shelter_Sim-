//! Where a Run's trace goes and whether it has an LLM: `controller.artifacts` (its trace level),
//! `controller.llm_router` and `controller.social_policy` (lane P8-0b).
//!
//! The kernels read these at each step (the trace's level, `proposals.rs` and `staff.rs`); the Run
//! reads them once. Whether a trace writer has started or
//! finished changes during the Run, so the `Host` asks that where it records.

use std::sync::Arc;

use crate::config::read::Read;
use crate::config::{Fault, FaultClass};

use super::held::{Attr, Obj, Spec};

/// `controller.artifacts`: the trace level.
pub const ARTIFACTS: Attr = Attr::object("artifacts", &ARTIFACTS_SPEC);
const ARTIFACTS_SPEC: Spec = Spec::of(&[Attr::value("trace_level")]);

/// `controller.llm_router`, read for whether it is None.
pub const LLM_ROUTER: Attr = Attr::value("llm_router");

/// The refusal of a trace level `Level::of` does not know.
pub const UNKNOWN_LEVEL: &str = "a trace level this kernel does not know";

/// `artifacts.trace_level`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceLevel {
    Off,
    Summary,
    Full,
}

/// Whether the Run has an LLM: `llm_router is not None`, `social_policy is not None`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Llm {
    pub router: bool,
    pub social_policy: bool,
}

/// What the Run read of its sinks and its LLM.
#[derive(Clone, Debug, PartialEq)]
pub struct Sinks {
    pub trace_level: Read<TraceLevel>,
    pub llm_router: Read<bool>,
    pub social_policy: Read<bool>,
}

impl Sinks {
    pub fn of(controller: &Obj) -> Self {
        let artifacts = controller.get("artifacts");
        Self {
            trace_level: artifacts.and_then(|held| match held.object()? {
                Some(artifacts) => trace_level(artifacts),
                // `getattr(None, "trace_level")`.
                None => Err(Fault::new(FaultClass::Other(Arc::from("builtins.AttributeError")), "'NoneType' object has no attribute 'trace_level'")),
            }),
            llm_router: controller.is_none("llm_router").map(|none| !none),
            social_policy: controller.is_none("social_policy").map(|none| !none),
        }
    }

    /// Both flags, or the first fault.
    pub fn llm(&self) -> Read<Llm> {
        Ok(Llm { router: self.llm_router.clone()?, social_policy: self.social_policy.clone()? })
    }

    /// Every field the refusal of a Run without a controller.
    pub fn absent() -> Self {
        let fault = || Fault::refused(super::held::NO_CONTROLLER);
        Self { trace_level: Err(fault()), llm_router: Err(fault()), social_policy: Err(fault()) }
    }
}

/// `Level::of` of `trace.rs`: a str or a str subclass, one of three words.
fn trace_level(artifacts: &Obj) -> Read<TraceLevel> {
    let level = artifacts.get("trace_level")?;
    let Some(text) = level.text() else {
        return Err(Fault::new(FaultClass::Type, "trace_level is not a str"));
    };
    match text {
        "off" => Ok(TraceLevel::Off),
        "summary" => Ok(TraceLevel::Summary),
        "full" => Ok(TraceLevel::Full),
        _ => Err(Fault::refused(UNKNOWN_LEVEL)),
    }
}
