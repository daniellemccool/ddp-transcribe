// `unwrap_used`/`expect_used` are denied in production (Cargo.toml [lints]) but
// idiomatic in unit tests. Scope the crate-wide allow to `cfg(test)` ONLY, so even
// the `--features test-helpers` build keeps enforcing them on production code. The
// feature-gated test scaffolding (e.g. `fetcher::FakeFetcher`) carries its own
// targeted `#[allow]` at the item rather than exempting the whole crate.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
// 0045's backstop against `pub` drifting wider than the façade below. It is
// load-bearing from this commit on: six modules just became private `mod`s, so
// their `pub` items would otherwise sit unreachable and undetected. Clippy runs
// with `-D warnings`, so a finding here is a build failure — the fix is
// `pub(crate)`, never a new façade export.
#![warn(unreachable_pub)]

// 0045: `lib.rs` is the crate's SINGLE module root. `main.rs` declares no
// modules; every file below compiles exactly once. Modules that only the
// binary's dispatch path consumes stay private here — they reach each other
// through `crate::`, and nothing of theirs leaks into the public API.
mod backfill;
mod cli;
mod commands;
mod config;
mod failure;
mod metadata_loader;
mod status;

pub mod audio;
pub mod batch;
pub mod canonical;
pub mod classification;
pub mod errors;
pub mod fetcher;
pub mod ingest;
pub mod output;
pub mod pipeline;
pub mod process;
pub mod state;
pub mod transcribe;

// 0045: the public façade. Exactly four names — everything `main.rs` needs and
// nothing more. `Cli`/`LogFormat` serve argument parsing and tracing init;
// `dispatch`/`CommandExit` carry the subcommand's work and its exit semantics
// back across the bin/lib boundary (the library never calls `process::exit`).
pub use cli::{Cli, LogFormat};
pub use commands::{dispatch, CommandExit};
