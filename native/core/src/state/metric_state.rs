//! `world.metric_state`: floats by name, a world field rather than a part of `model_state`
//! (`OWNED_FIELDS` in `initial/mod.rs`). The rules write single keys here; the distress monitor of
//! `rules/psychology.rs` is one of them.

use crate::row::IdMap;

pub type MetricRow = IdMap<f64>;
