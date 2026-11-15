//! `state_row!` and `str_enum!` from outside the core, and a row the core holds: the bytes are those
//! of `json.dumps(tree, sort_keys=True, separators=(",", ":"), ensure_ascii=False)` in CPython 3.12,
//! and the names keep the order of the tree.

// Field names as a tree may have them: capitals and non-ASCII sort before and after the rest.
#![allow(non_snake_case)]

use std::sync::Arc;

use sheltersim_core::error::Error;
use sheltersim_core::json::Encoder;
use sheltersim_core::row::{same_names, sorted_order, Field, IdMap, Maybe, Row, StrEnum, ValueRow};
use sheltersim_core::state::psychological_distress::{AgentRecord, DistressRow};
use sheltersim_core::value::V;
use sheltersim_core::{state_row, str_enum};

str_enum! {
    pub enum Phase { Idle = "idle", Busy = "busy" }
}

state_row! {
    pub struct Sample {
        zeta: f64,
        alpha: f64,
        Beta: bool,
        é: Arc<str>,
        ab: Option<i64>,
        a: Vec<i64>,
        gone: Maybe<i64>,
        phase: Phase,
    }
}

fn sample() -> Sample {
    Sample { zeta: 1.5, alpha: -0.0, Beta: true, é: Arc::from("x\"y"), ab: None, a: vec![1, 2], gone: Maybe(None), phase: Phase::Busy }
}

fn json<T: Field>(value: &T) -> String {
    let mut encoder = Encoder::default();
    Field::write_json(value, &mut encoder).unwrap();
    String::from_utf8(encoder.out).unwrap()
}

#[test]
fn a_record_writes_its_keys_in_byte_order_and_leaves_out_an_absent_one() {
    assert_eq!(Sample::NAMES, ["zeta", "alpha", "Beta", "é", "ab", "a", "gone", "phase"]);
    assert_eq!(json(&sample()), r#"{"Beta":true,"a":[1,2],"ab":null,"alpha":-0.0,"phase":"busy","zeta":1.5,"é":"x\"y"}"#,);
    let mut with_key = sample();
    with_key.gone = Maybe(Some(7));
    assert!(json(&with_key).contains(r#""ab":null,"alpha":-0.0,"gone":7,"phase""#));
    assert_eq!(sorted_order::<4>(&["b", "a", "B", "ab"]), [2, 1, 3, 0]);
    assert!(same_names(Sample::NAMES, &["zeta", "alpha", "Beta", "é", "ab", "a", "gone", "phase"]));
    assert!(!same_names(Sample::NAMES, &["alpha", "zeta", "Beta", "é", "ab", "a", "gone", "phase"]));
}

#[test]
fn a_set_of_strings_and_its_misfit() {
    assert_eq!(Phase::from_text("busy"), Some(Phase::Busy));
    assert_eq!(Phase::from_text("Busy"), None);
    assert_eq!(Phase::MISFIT, "not a Phase");
    assert_eq!(json(&Phase::Idle), r#""idle""#);
}

#[test]
fn an_absent_key_and_a_float_json_refuses() {
    let mut encoder = Encoder::default();
    assert_eq!(Field::write_json(&Maybe::<i64>(None), &mut encoder), Err(Error::Value("an absent key has no value".into())));
    assert_eq!(Field::write_json(&f64::NAN, &mut encoder), Err(Error::Unsupported("NaN and infinity are left to json".into())),);
}

#[test]
fn a_list_is_read_item_by_item() {
    let row = sample();
    let items = Field::part_items(&row, "a").unwrap();
    let mut scratch = Encoder::default();
    assert_eq!(items.count(), 2);
    assert_eq!(items.item_json(1, &mut scratch).unwrap(), b"2");
    assert!(Field::part_items(&row, "zeta").is_none());
    assert!(Field::part_items(&row, "gone").is_none());

    let value = ValueRow(V::Map([(Arc::from("h"), V::List(vec![V::Float(1.0), V::Str(Arc::from("é"))]))].into_iter().collect()));
    let items = value.part_items("h").unwrap();
    assert_eq!(items.item_json(1, &mut scratch).unwrap(), "\"é\"".as_bytes());
}

#[test]
fn the_distress_row_writes_the_bytes_of_its_tree() {
    let record = |baseline, long_term, sum, high, observed, days| AgentRecord {
        baseline_distress: baseline,
        long_term_distress: long_term,
        daily_acute_sum: sum,
        daily_high_distress_minutes: high,
        daily_observed_minutes: observed,
        completed_days: days,
    };
    let mut agents = IdMap::default();
    agents.insert(Arc::from("b"), record(0.25, 0.5, 1e-05, 0, 3, 2));
    agents.insert(Arc::from("a"), record(0.0, 1.0, 12.0, 1, 30, 0));
    let mut row = DistressRow { schema_version: 1, last_updated_minute: -1, agents };
    let mut encoder = Encoder::default();
    Row::write_json(&row, &mut encoder).unwrap();
    assert_eq!(
        String::from_utf8(encoder.out).unwrap(),
        concat!(
            r#"{"agents":{"a":{"baseline_distress":0.0,"completed_days":0,"daily_acute_sum":12.0,"#,
            r#""daily_high_distress_minutes":1,"daily_observed_minutes":30,"long_term_distress":1.0},"#,
            r#""b":{"baseline_distress":0.25,"completed_days":2,"daily_acute_sum":1e-05,"#,
            r#""daily_high_distress_minutes":0,"daily_observed_minutes":3,"long_term_distress":0.5}},"#,
            r#""last_updated_minute":-1,"schema_version":1}"#,
        ),
    );
    assert!(row.is_valid());
    // An acute sum past the minutes observed fails validate_psychological_state.
    row.agents.get_mut("b").unwrap().daily_acute_sum = 3.5;
    assert!(!row.is_valid());
}
