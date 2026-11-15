# sheltersim_native

The Rust of 避難所IF！: the simulation's core (`core/`, `sheltersim_core`), the files of a stored Run
(`store/`, `sheltersim_store`), the browser's boundary (`wasm/`, `sheltersim_wasm`) and the Python
extension (`src/`, `sheltersim_native`) the local and VM versions call (#152, #368, #430). Every
computation of a Run is the core's: the minute-0 world of a Scenario, the ticks, the
EffectPrograms, the stored Run of format v5 and the answers of the Project routes. Python keeps the
HTTP server, the Scenario authoring, the regional data and the LLM Provider; the browser keeps its
pages, OPFS and the relay to the LLM.

The extension is required: importing `sheltersim` without it raises ImportError
(`docs/development.md` gives the build). CI builds and installs it on Linux, Windows and macOS. Its
test jobs build with the profile `ci` of `native/Cargo.toml` (thin LTO, 16 codegen units; the same
bytes, built sooner), and its gate job with `--release` (#217). The results of a Run are pinned by
the gate (`benchmarks/gate_all_paths.py --expect fixtures/gate-all-paths.json`) and the seven-day
digests (`benchmarks/investigate_performance.py --expect`).

## Build and use without installing

```shell
cd native
../.venv/bin/maturin build --release -i ../.venv/bin/python
mkdir -p pylib && ../.venv/bin/python -m zipfile -e target/wheels/sheltersim_native-*.whl pylib
cd .. && PYTHONPATH=src:native/pylib SHELTERSIM_LLM_MODE=offline .venv/bin/python -m pytest -q -n auto
```

## Format

`rustfmt.toml` sets the style of every crate in the workspace (#367): lines up to 160
columns, each item filling the line (`use_small_heuristics = "Max"`). Run
`cargo fmt --all` here before a commit; CI's job 「書式・規則」 checks `cargo fmt --all -- --check`.
The tables that `core/tools/*.py` write keep their generator's layout: the `mod` that reads
each one carries `#[rustfmt::skip]`.

## The workspace

- **`core/` (`sheltersim_core`)**: the simulation without Python or any platform. Its `cargo tree`
  has no pyo3 (CI checks `cargo tree -p sheltersim_core -e normal,build`). It holds the World
  (`world.rs`: the scalars, the entity rows, the slots of the namespaces, the event queue and the
  EventLog), the typed configs (`config/`), the floor plan and the routes (`geometry.rs`,
  `routes.rs`), the transactions (`txn.rs`), the Run (`run/`), every rule of the tick (`rules/`),
  the validation and the input contracts (`validate/`, `contracts/`), the minute-0 world of any
  Scenario (`initial/`, #429), the Run without Python (`plain.rs`, `PlainRun` and `PlainHost`), the
  LLM's port (`llm.rs`, `LlmLink::complete`, #428), the saved points and the stored Run's format
  (`archive.rs`, `run_store/`), the computing procedure of a stored Run as steps without I/O
  (`compute.rs`, #476: going on from a branch's head in chunks, a fork, a question's turn, the
  start, the recovery of an interrupted branch), the answers of the API (`answers/`, #449) and the Scenario
  authoring of the browser (`scenario/`).
- **`store/` (`sheltersim_store`)**: the files of a stored Run on a disk (#426): the records, the
  journal and the pieces, the locks, the capacity, the readers of another process, and the steps of
  the core's computing procedure done with them (`compute.rs`, #476).
- **`wasm/` (`sheltersim_wasm`)**: the browser's boundary to the core (wasm-bindgen), the core's
  computing procedure among it (`Procedure`, #476), whose steps `js/run_store.mjs`'s `StepHost`
  does with OPFS.
- **`src/` (`sheltersim_native`)**: the thin PyO3 wrapper. Each entry reads its arguments, calls the
  core and converts the result:
  - `rules.plain`: `PlainRun` (a Run of an entry or of a Scenario, its ticks, digests, frame,
    EffectProgram, saved points, LLM link) and `Restore`.
  - `rules.run_files`: the stored Runs' files (`ProjectFiles`, `Lease`, `StoredRun`,
    `Computation`).
  - `rules.answers`: the answers of the Project routes, as the browser's, and the answers made
    with an LLM (#463): a question about a branch's results (`questions::QuestionPlan::ask`,
    `Asking::answer`), the review of a completed branch (`report::review`), the reading of an
    intervention's text (`interpret::Compiling`), the PDF report's data, facts and analysis
    (`pdf::PdfPlan`, `PdfAnalysis::read`) and the llm-status of a Run computed with an LLM.
    Each is steps around the port of `llm.rs` (`answers::asking`): the core gives the task's JSON,
    the caller hands it to its link and gives the reply's JSON back. `core/tests/llm_answers.rs`
    checks the reading; `tests/integration/test_llm_answers.py` asks through the Project API with
    a fake Provider.
  - `initial_entry`, `initial_model_versions`, `initial_evidence`, `scenario_text_parse`,
    `set_people_per_helper`, `parallel_state`, and the exception `Unsupported`.
  - `core/errors.rs` makes the core's errors Python's exceptions; `core/llm_link.rs` calls a
    Python link (`sheltersim.llm.link.RouterLink`) from the core's port.

The PyO3 host of the Python world (`PyHost`), the Python world's mirrors (`WorldCore`, `RunCore`,
the tracked containers), the Checkpoint and history-index kernels and every Python rule's kernel
went with the Python simulation in #430. The history of the port is in `docs/rust-port.md` and
`docs/rust-port/`.

## The core's tests

The files of `core/tests/` are the modules of one test binary, `core` (`core/tests/main.rs`,
#388): Cargo would otherwise build and link the core into a binary per file, and run those one
after another. `core/Cargo.toml` sets `autotests = false`, so a new file needs its `mod <name>;`
in `main.rs`; `core/tests/test_files.rs` fails when one is missing. Run one file's tests with
`cargo test -p sheltersim_core --test core <name>::`. `effect_guard_dispatch.rs` stays a binary
of its own (`--test effect_guard_dispatch`): it compares the process's dispatch counts before and
after each case, which the other tests, on other threads of the same process, would move.

## The core's feature `parallel` and the wasm32 build

`sheltersim_core` has one feature, `parallel`, on by default. It brings in rayon for the
helper threads of `parallel.rs` (`in_order`, `parts_in_order`). Without it, the same functions run
on the calling thread, in the same parts and the same order, and give the same bytes;
`parallel_state` is then `(0, 0)`. The wrapper names the feature in `native/Cargo.toml`, so the
extension keeps its threads. The build for the browser (#132, #152) leaves the feature out:

```shell
rustup target add wasm32-unknown-unknown
cargo build --manifest-path native/Cargo.toml -p sheltersim_core --target wasm32-unknown-unknown --no-default-features
cargo test --manifest-path native/Cargo.toml -p sheltersim_core --no-default-features
```

CI's job 「ブラウザー版の計算」 runs these, beside 「Rustの計算部分のテスト」, which runs
`cargo test -p sheltersim_core` with the feature.
`core/tests/parallel.rs` compares both builds with one frozen digest, and
`core/tests/no_platform.rs` fails if the core's source names `std::time`, `fs`, `env` or
`thread` (they are missing on wasm32 and would tie results to the machine; `thread_local!`,
`Mutex` and `OnceLock` work on one thread and are allowed).

## Float rules

Every float operation of a rule goes through `sheltersim_core::mathx`, and keeps the order of the
Python expression the rule was ported from (the rules were Python's until #152 and #430, and their
results are pinned by the recorded data of `core/tests/data/` and the gate). The core's tests pin
the bits of exp, expm1, pow and fma as fixed vectors.
`cargo clippy --manifest-path native/Cargo.toml --workspace --all-targets -- -A clippy::all -D clippy::disallowed_methods`
enforces the bans in `native/clippy.toml`, in the tests too: an expected value a test makes
with `f64::max` would part from the rule's `py_max` at a tie or a NaN (#390).

| Python | Rust | Note |
|---|---|---|
| `math.exp(x)`, `math.expm1(x)` | `mathx::exp`, `mathx::expm1` | the `libm` crate. `MathError` where Python raises, as Python's exception |
| `x ** y`, `x ** 2`, `x ** 0.5` | `mathx::pow` | `float_pow`'s special cases, then the `libm` crate. Never `x * x` or `sqrt`: see below. `MathError` where Python raises or returns a complex |
| `math.hypot(*xs)` | `mathx::hypot` | CPython's algorithm, with `libm::fma` |
| `sum(xs)` | `mathx::py_sum`, `sum_nums` | compensated in 3.12. `sum([])` and a sum of ints are ints: `sum_nums` |
| `math.fsum(xs)` | `mathx::py_fsum` | `None` for what is not finite |
| `math.isclose(a, b)` | `mathx::py_is_close` | the default tolerances |
| `max(a, b)`, `min(a, b)` of floats | `mathx::py_max`, `py_min` | first argument on a tie; a NaN compares false |
| `max(0, x)`, any int with a float | `mathx::max_num`, `min_num` | returns the int: JSON writes `0`, not `0.0` |
| `round(x, 8)` | `mathx::py_round` | half to even on the exact binary value |
| `round(x)` | `mathx::py_round_int` | an int, half to even |

`None` means the rule raises `Unsupported`.

Since model version 2, exp, expm1, pow and fma are the pure Rust `libm` crate at one version
(`=0.2.16` and `Cargo.lock`), not the platform's libm CPython calls, so a rule gives the same bits on
Linux, Windows and macOS. They are within one ulp of glibc's and differ from it in the last bit for
some arguments (`exp(1.0)` among them). `f64::powf`, `f64::exp` and `f64::mul_add` would call the
platform's libm, and LLVM rewrites `powf(x, 2.0)` to `x * x` and `powf(x, 0.5)` to `sqrt`;
`native/clippy.toml` bans them. Python code outside the rules writes `x * x` and `math.sqrt`, which
are correctly rounded everywhere, and not `**` with a float.
