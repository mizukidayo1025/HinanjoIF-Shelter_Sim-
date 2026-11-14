//! The model versions a Run was computed with, and the rule that makes a Run of other versions
//! view-only (#368, decision 8): `src/sheltersim/core/model_version.py`, with its texts.
//!
//! A stored Run's "run" line holds the `model_versions` the Run was computed with
//! (`model_signatures` of its Scenario: the signatures of each installed policy and state
//! contract, and the whole-model items `engine:model` and `engine:rng_algorithm`). A Run whose
//! recorded versions differ from the installed ones is view-only: it is read and shown as stored,
//! and going on with it (continuing, forking, an intervention) is refused with
//! `ModelStatus::error` (`run_model_view_only`).
//!
//! What Python's `model_version_status` reads besides the two maps is left out, as no stored Run
//! of this format can need it: the explicit migration of saves from before independent facility
//! queues and the older shapes of `model_state` (`_legacy_shapes`) are Python's Checkpoints', and
//! the configs decide nothing when the recorded map holds model items, which every Run of this
//! format's does.

use std::collections::BTreeMap;

use crate::error::Error;

/// The whole-model version's key.
pub const ENGINE_VERSION_KEY: &str = "engine:model";
/// The RNG algorithm's key.
pub const RNG_ALGORITHM_KEY: &str = "engine:rng_algorithm";
/// `ENGINE_VERSION` of model_version.py: the version of the simulation as a whole. A test keeps it
/// equal to Python's.
pub const ENGINE_VERSION: &str = "3";
/// `CounterRNG.algorithm_id`.
pub const RNG_ALGORITHM: &str = crate::scenario::profiles::RNG_ALGORITHM;
/// The code of a refusal to go on with a Run of other versions.
pub const VIEW_ONLY_CODE: &str = "run_model_view_only";

/// What a save without the engine items was computed with: engine 1 and the only RNG there was.
const UNRECORDED_ENGINE: [(&str, &str); 2] = [(ENGINE_VERSION_KEY, "1"), (RNG_ALGORITHM_KEY, "sha256-keyed-v1")];
const MODEL_PREFIXES: [&str; 4] = ["policy:", "simulator:", "state:", "operator:"];

/// A map of model versions, by key.
pub type Versions = BTreeMap<String, String>;

/// The versions of a JSON object of strings: an error for any other JSON.
pub fn versions_of_json(json: &[u8]) -> Result<Versions, Error> {
    let bad = || Error::Value("the model versions are not a JSON object of strings".to_owned());
    match serde_json::from_slice::<serde_json::Value>(json).map_err(|_| bad())? {
        serde_json::Value::Object(object) => {
            object.into_iter().map(|(key, value)| value.as_str().map(|value| (key, value.to_owned())).ok_or_else(bad)).collect()
        }
        _ => Err(bad()),
    }
}

/// `engine_signatures()`: the whole-model items this code records.
pub fn engine_signatures() -> Versions {
    [(ENGINE_VERSION_KEY, ENGINE_VERSION), (RNG_ALGORITHM_KEY, RNG_ALGORITHM)].into_iter().map(|(key, value)| (key.to_owned(), value.to_owned())).collect()
}

/// `recorded_engine`: the `engine:` items of recorded versions, absent ones as engine 1's.
fn recorded_engine(versions: &Versions) -> Versions {
    let mut engine: Versions = UNRECORDED_ENGINE.iter().map(|(key, value)| ((*key).to_owned(), (*value).to_owned())).collect();
    engine.extend(versions.iter().filter(|(key, _)| key.starts_with("engine:")).map(|(key, value)| (key.clone(), value.clone())));
    engine
}

fn model_items(versions: &Versions) -> Versions {
    versions
        .iter()
        .filter(|(key, _)| MODEL_PREFIXES.iter().any(|prefix| key.starts_with(prefix)) || *key == "model_suite")
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// How a stored Run's recorded versions compare with the installed ones (`ModelVersionStatus`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelStatus {
    pub recorded_version: String,
    pub installed_version: String,
    /// The recorded items that differ from the installed ones, sorted.
    pub differences: Vec<String>,
}

impl ModelStatus {
    /// `model_version_status(recorded, …, installed)` of two maps (see the module's note).
    pub fn of(recorded: &Versions, installed: &Versions) -> Self {
        let engine_recorded = recorded_engine(recorded);
        let engine_installed: Versions =
            installed.iter().filter(|(key, _)| key.starts_with("engine:")).map(|(key, value)| (key.clone(), value.clone())).collect();
        let mut differences: Vec<String> =
            engine_recorded.keys().chain(engine_installed.keys()).filter(|key| engine_recorded.get(*key) != engine_installed.get(*key)).cloned().collect();
        let (recorded_items, installed_items) = (model_items(recorded), model_items(installed));
        if recorded_items != installed_items && (!recorded_items.is_empty() || installed_items.keys().any(|key| key.starts_with("operator:"))) {
            differences
                .extend(recorded_items.keys().chain(installed_items.keys()).filter(|key| recorded_items.get(*key) != installed_items.get(*key)).cloned());
        }
        differences.sort();
        differences.dedup();
        Self {
            recorded_version: engine_recorded[ENGINE_VERSION_KEY].clone(),
            installed_version: engine_installed.get(ENGINE_VERSION_KEY).cloned().unwrap_or_else(|| ENGINE_VERSION.to_owned()),
            differences,
        }
    }

    /// `unreadable_status`: a save whose configs the installed code cannot sign at all.
    pub fn unreadable(recorded: &Versions) -> Self {
        Self {
            recorded_version: recorded_engine(recorded)[ENGINE_VERSION_KEY].clone(),
            installed_version: ENGINE_VERSION.to_owned(),
            differences: vec!["model_signatures".to_owned()],
        }
    }

    /// Whether the Run may be gone on with the installed model.
    pub fn current(&self) -> bool {
        self.differences.is_empty()
    }

    /// The user-facing Japanese text of a Run of other versions (`ModelVersionStatus.message`).
    pub fn message(&self) -> String {
        let tail = "閲覧のみです。保存した時点だけを表示し、続きの計算、分岐、介入の適用はできません。";
        if self.recorded_version != self.installed_version {
            let number =
                |text: &str| -> Option<u128> { (!text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())).then(|| text.parse().ok()).flatten() };
            let older = matches!((number(&self.recorded_version), number(&self.installed_version)), (Some(recorded), Some(installed)) if recorded < installed);
            return format!(
                "このシナリオは{}モデル（版 {}）で計算されました。今のモデルは版 {} です。{tail}",
                if older { "旧" } else { "別の版の" },
                self.recorded_version,
                self.installed_version
            );
        }
        let named = |keys: &[&str]| format!("{}{}", keys.iter().take(3).copied().collect::<Vec<_>>().join("、"), if keys.len() > 3 { " ほか" } else { "" });
        let models: Vec<&str> = self.differences.iter().map(String::as_str).filter(|key| !key.starts_with("legacy:")).collect();
        if !models.is_empty() {
            return format!("このシナリオは今と異なるモデルの版（{}）で計算されました。{tail}", named(&models));
        }
        let shapes: Vec<&str> = self.differences.iter().map(|key| key.strip_prefix("legacy:").unwrap_or(key)).collect();
        format!("このシナリオは以前の保存の形（{}）で保存されました。{tail}", named(&shapes))
    }

    /// `ViewOnlyRunError`'s text: `run_model_view_only: ` and the message.
    pub fn error_text(&self) -> String {
        format!("{VIEW_ONLY_CODE}: {}", self.message())
    }

    /// Nothing for a current Run; else the refusal to go on with it (`ViewOnlyRunError`).
    pub fn require_current(&self) -> Result<(), Error> {
        if self.current() {
            Ok(())
        } else {
            Err(Error::Value(self.error_text()))
        }
    }
}
