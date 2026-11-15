//! The store's tests, in one test binary (as the core's, #388). Run one file's tests with
//! `cargo test -p sheltersim_store --test store <file name>::`.

mod support;

mod browser_runs;
mod limits;
mod locks;
mod records;
mod server;
mod torn_lines;
