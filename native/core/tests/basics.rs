//! The core's floats and strings written as CPython 3.12 writes them (`repr`, `json.dumps`), its
//! math as Python's, and its keyed draws (`rng::KeyedDraws`). The draws were recorded before #430
//! and pin the core's output now.

use sheltersim_core::json::{python_float_repr, write_float, write_json_string, NotFinite};
use sheltersim_core::mathx::{pow, py_max, py_round};
use sheltersim_core::rng::{Key, KeyedDraws};

#[test]
fn float_repr_is_pythons() {
    let cases =
        [(1e16, "1e+16"), (1e-5, "1e-05"), (-0.0, "-0.0"), (0.1, "0.1"), (123456789012345678.0, "1.2345678901234568e+17"), (1.5e-7, "1.5e-07"), (2.5, "2.5")];
    for (value, expected) in cases {
        assert_eq!(python_float_repr(value), expected, "{value}");
    }
}

#[test]
fn json_refuses_non_finite_floats() {
    let mut out = Vec::new();
    assert_eq!(write_float(&mut out, f64::NAN), Err(NotFinite));
    assert_eq!(write_float(&mut out, f64::INFINITY), Err(NotFinite));
    write_float(&mut out, 0.5).unwrap();
    assert_eq!(out, b"0.5");
}

#[test]
fn json_strings_escape_as_json_dumps() {
    let mut out = Vec::new();
    write_json_string(&mut out, "避難\"\\\n\u{1}");
    assert_eq!(String::from_utf8(out).unwrap(), "\"避難\\\"\\\\\\n\\u0001\"");
}

#[test]
fn keyed_draws_are_the_recorded_ones() {
    let draws = KeyedDraws::new("2026");
    let mut key = Key::labelled("propose");
    key.int(1).text("agent-1");
    assert_eq!(key.draw(&draws), 0.25758270473760164);
    assert_eq!(Key::labelled("x").draw(&draws), 0.03090420718010574);
}

#[test]
fn math_follows_python() {
    assert_eq!(pow(2.0, 0.5), Ok(1.4142135623730951));
    assert!(pow(0.0, -1.0).is_err());
    assert!(py_max(f64::NAN, 1.0).is_nan());
    assert_eq!(py_round(2.675, 2), Some(2.67));
}
