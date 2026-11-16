//! The EffectProgram of an intervention before it is scheduled, in the core (#304): the contract
//! (`sheltersim_core::program::EffectProgram`) reads each program of `tests/data/programs.json`, or
//! refuses it where the case is `refused`; its dump has the recorded sha256 of its canonical JSON
//! (`dump_sha256`, the approval's hash); and `program::validate` on the same world at the same
//! minute (`plain::PlainRun`, the browser's host) gives the recorded issues, in the same order and
//! texts. The values were recorded before #430 and pin the core's output now.
//!
//! The cases are the six operators the offline reading of a text makes, on `small_replay` and
//! `generated` of `tick_core.json` and on `operations`, a world with pending outages. A case marked
//! `unsupported` is one the browser does not check (another operator of the registry, or
//! parameters whose issue holds pydantic's text): the core refuses it.

use std::collections::BTreeMap;
use std::fs;

use serde_json::{json, Value};
use sheltersim_core::checkpoint::sha256_hex;
use sheltersim_core::error::Error;
use sheltersim_core::plain::PlainRun;
use sheltersim_core::program::EffectProgram;
use sheltersim_core::world::WorldCell;

fn read(name: &str) -> Vec<u8> {
    fs::read(format!("{}/tests/data/{name}.json", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn run_to(run: &mut PlainRun, minute: i64) {
    while run.minute().unwrap() < minute {
        run.step().unwrap_or_else(|error| panic!("minute {}: {error:?}", run.minute().unwrap()));
    }
}

#[test]
fn the_core_reads_dumps_and_checks_each_program_as_recorded() {
    let fixture: Value = serde_json::from_slice(&read("programs")).unwrap();
    let mut files = BTreeMap::new();
    let mut runs: BTreeMap<String, PlainRun> = BTreeMap::new();
    let (mut refused, mut unsupported, mut checked, mut with_issues) = (0, 0, 0, 0);
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let world = case["world"].as_str().unwrap();
        let minute = case["minute"].as_i64().unwrap();
        let raw = serde_json::to_vec(&case["program"]).unwrap();
        let read = EffectProgram::of_json(&raw);
        if case["refused"].as_bool() == Some(true) {
            assert!(matches!(read, Err(Error::Value(_))), "{name}: {read:?}");
            refused += 1;
            continue;
        }
        let program = read.unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(sha256_hex(&program.dump_json().unwrap()), case["dump_sha256"].as_str().unwrap(), "{name}");
        let file = fixture["files"][world].as_str().unwrap();
        let bytes = files.entry(file.to_owned()).or_insert_with(|| self::read(file));
        let run = runs.entry(world.to_owned()).or_insert_with(|| PlainRun::of_worlds_json(bytes, world).unwrap());
        run_to(run, minute);
        let before = run.digests().unwrap();
        let issues = run.validate_effect_program(&program);
        assert_eq!(run.digests().unwrap(), before, "{name}: the check changed the world");
        if case["unsupported"].as_bool() == Some(true) {
            assert!(matches!(issues, Err(Error::Unsupported(_))), "{name}: {issues:?}");
            unsupported += 1;
            continue;
        }
        let expected: Vec<String> = case["issues"].as_array().unwrap().iter().map(|issue| issue.as_str().unwrap().to_owned()).collect();
        assert_eq!(issues.unwrap_or_else(|error| panic!("{name}: {error}")), expected, "{name}");
        checked += 1;
        with_issues += usize::from(!expected.is_empty());
    }
    assert_eq!((refused, unsupported), (10, 2));
    assert!(checked >= 35 && with_issues >= 25, "{checked} {with_issues}");
}

/// The world's digests by part (`canonical::run_digests`).
fn digests(run: &PlainRun) -> Value {
    let digests = run.loaded.cell.with(|world| sheltersim_core::canonical::run_digests(world)).unwrap().unwrap();
    Value::Object(digests.into_iter().map(|(part, digest)| (part.to_owned(), Value::from(digest))).collect())
}

/// A program is scheduled on the world as recorded (#305): the same `command_log` entry, the same
/// world after it and at the end of the Run; a program with issues is refused with the recorded
/// issues' text, the world as it was.
#[test]
fn the_core_schedules_each_program_as_recorded() {
    let fixture: Value = serde_json::from_slice(&read("programs")).unwrap();
    let (mut scheduled, mut refused) = (0, 0);
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let issues = match case["issues"].as_array() {
            Some(issues) if case["unsupported"].as_bool() != Some(true) => issues,
            _ => continue,
        };
        let world = case["world"].as_str().unwrap();
        let program = EffectProgram::of_json(&serde_json::to_vec(&case["program"]).unwrap()).unwrap();
        let mut run = PlainRun::of_worlds_json(&read(fixture["files"][world].as_str().unwrap()), world).unwrap();
        run_to(&mut run, case["minute"].as_i64().unwrap());
        let before = digests(&run);
        let applied = run.apply_effect_program(&program);
        if !issues.is_empty() {
            let texts: Vec<&str> = issues.iter().map(|issue| issue.as_str().unwrap()).collect();
            let expected = format!("EffectProgram could not be scheduled: {}", texts.join("; "));
            assert!(matches!(&applied, Err(Error::Value(text)) if *text == expected), "{name}: {applied:?}");
            assert_eq!(digests(&run), before, "{name}: a refused program changed the world");
            refused += 1;
            continue;
        }
        let expected = &case["scheduled"];
        let receipt = applied.unwrap_or_else(|error| panic!("{name}: {error}"));
        let command: Value = serde_json::from_slice(&receipt.command().to_json().unwrap()).unwrap();
        assert_eq!(command, expected["command"], "{name}");
        assert_eq!(digests(&run), expected["digests"], "{name}: after scheduling");
        let end = run.duration().unwrap();
        run_to(&mut run, end);
        assert_eq!(digests(&run), expected["end_digests"], "{name}: at the end");
        scheduled += 1;
    }
    assert!(scheduled >= 10 && refused >= 20, "{scheduled} {refused}");
}

/// A program the screen builds from the catalog (#584): its operator, a target of the world, the
/// parameters given, its start, its end where the operator has one; built as the screen builds it
/// (`compiler_kind` `manual`).
fn built(entry: &Value, target: &Value, parameters: Value, minute: i64) -> EffectProgram {
    let op = entry["op"].as_str().unwrap();
    let end = if entry["timed"].as_bool().unwrap() { json!(minute + 30) } else { Value::Null };
    let program = json!({
        "schema_version": 1,
        "program_id": format!("manual-{op}"),
        "narrative": entry["label"],
        "source_text": entry["label"],
        "simulation_start_minute": minute,
        "effects": [{
            "effect_id": "effect-1",
            "op": op,
            "target_selector": target,
            "start_minute": minute,
            "end_minute": end,
            "parameters": parameters,
            "units": entry["units"][0],
            "source_id": "screen-builder-v1",
            "policy_dependencies": entry["required_policies"],
            "reversible": true,
        }],
        "support_mode": "native",
        "compiler_provenance": {"compiler_kind": "manual", "compiler_version": "screen-builder-v1"},
    });
    EffectProgram::of_json(&serde_json::to_vec(&program).unwrap()).unwrap()
}

/// The catalog the screen builds an intervention from (`program::catalog`, #584) lists each
/// operator the core schedules once, with the registry's ranges of the parameters it asks for:
/// a program built from it at the bounds is checked on a world, and one past them is refused as a
/// parameter its model refuses.
#[test]
fn the_catalog_lists_what_the_core_schedules_with_the_ranges_it_checks() {
    use sheltersim_core::program::{catalog, supports};
    let catalog: Value = serde_json::from_slice(&catalog().unwrap().to_json().unwrap()).unwrap();
    let entries = catalog.as_array().unwrap();
    let ops: Vec<&str> = entries.iter().map(|entry| entry["op"].as_str().unwrap()).collect();
    assert_eq!(ops.len(), 6, "{ops:?}");
    assert!(ops.iter().all(|op| supports(op)) && ops.iter().collect::<std::collections::BTreeSet<_>>().len() == ops.len(), "{ops:?}");
    let fixture: Value = serde_json::from_slice(&read("programs")).unwrap();
    let mut run = PlainRun::of_worlds_json(&read(fixture["files"]["operations"].as_str().unwrap()), "operations").unwrap();
    run_to(&mut run, 10);
    let mut bounds = 0;
    for entry in entries {
        let op = entry["op"].as_str().unwrap();
        assert!(!entry["label"].as_str().unwrap().is_empty() && entry["units"].as_array().unwrap().len() == 1, "{op}");
        // A target of a recorded case of the operator on the world.
        let target = fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["world"] == "operations" && case["program"]["effects"][0]["op"] == op)
            .map(|case| case["program"]["effects"][0]["target_selector"].clone())
            .unwrap_or_else(|| panic!("{op}: no recorded case"));
        let least: serde_json::Map<String, Value> = entry["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|parameter| parameter["required"] == true)
            .map(|parameter| (parameter["name"].as_str().unwrap().to_owned(), parameter["minimum"].clone()))
            .collect();
        let mut checked = |parameters: Value| run.validate_effect_program(&built(entry, &target, parameters, 10));
        assert!(checked(Value::Object(least.clone())).is_ok(), "{op}: the least values");
        for parameter in entry["parameters"].as_array().unwrap() {
            let name = parameter["name"].as_str().unwrap();
            match parameter["type"].as_str().unwrap() {
                "integer" => {
                    let (low, high) = (parameter["minimum"].as_i64().unwrap(), parameter["maximum"].as_i64().unwrap());
                    for (value, accepted) in [(low, true), (high, true), (low - 1, false), (high + 1, false)] {
                        let mut parameters = least.clone();
                        parameters.insert(name.to_owned(), json!(value));
                        let issues = checked(Value::Object(parameters));
                        assert_eq!(!matches!(issues, Err(Error::Unsupported(_))), accepted, "{op}.{name}={value}: {issues:?}");
                        bounds += 1;
                    }
                }
                "boolean" => {
                    let mut parameters = least.clone();
                    parameters.insert(name.to_owned(), json!(true));
                    assert!(checked(Value::Object(parameters)).is_ok(), "{op}.{name}");
                }
                other => panic!("{op}.{name}: a parameter of type {other} the builder cannot show"),
            }
        }
    }
    assert_eq!(bounds, 4 * 5);
}
