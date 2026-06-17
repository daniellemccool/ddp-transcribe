// `unwrap_used`/`expect_used` are denied in production (Cargo.toml [lints]) but
// idiomatic in unit tests. Scope the crate-wide allow to `cfg(test)` ONLY, so even
// the `--features test-helpers` build keeps enforcing them on production code. The
// feature-gated test scaffolding (e.g. `fetcher::FakeFetcher`) carries its own
// targeted `#[allow]` at the item rather than exempting the whole crate.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod audio;
pub mod canonical;
pub mod errors;
pub mod fetcher;
pub mod ingest;
pub mod output;
pub mod pipeline;
pub mod process;
pub mod state;
pub mod transcribe;
