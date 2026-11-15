//! Every file of `tests/` runs (Issue #388). The package sets `autotests = false`, so Cargo builds
//! only the test binary its `[[test]]` section names, `main.rs`: a file that `main.rs` does not
//! declare as a module would never run, and nothing would say so.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

fn stems(directory: &Path) -> BTreeSet<String> {
    fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
        .map(|path| path.file_stem().unwrap().to_str().unwrap().to_owned())
        .collect()
}

#[test]
fn each_file_of_the_tests_is_a_module_of_main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let main = fs::read_to_string(root.join("tests/main.rs")).unwrap();
    let declared: BTreeSet<String> = main.lines().filter_map(|line| line.strip_prefix("mod ")?.strip_suffix(';')).map(str::to_owned).collect();
    let mut missing = Vec::new();
    for stem in stems(&root.join("tests")) {
        if stem != "main" && !declared.contains(&stem) {
            missing.push(stem);
        }
    }
    assert!(missing.is_empty(), "add `mod <name>;` to tests/main.rs for: {missing:?}");
}
