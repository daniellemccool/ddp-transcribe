//! Pipelined orchestrator: N fetch workers feed 1 transcribe worker via a
//! bounded mpsc channel (0027).
//!
//! **SKELETON ONLY in T15.** The function compiles and returns
//! `Ok(ProcessStats::default())` so callers (T18 wires `main.rs`'s
//! `--pipelined` branch) can be added with stable types. The fetch/
//! transcribe worker bodies land in T16 and T17; the `JoinSet` +
//! `CancellationToken` wiring with shutdown ORDER per 0025 lands in T18.

use anyhow::Result;

use super::{ProcessOptions, ProcessStats};
use crate::fetcher::VideoFetcher;
use crate::state::Store;
use crate::transcribe::WhisperEngine;

/// Shared mutable access to the `Store` across N fetch workers + 1
/// transcribe worker.
///
/// Implemented as `Arc<tokio::sync::Mutex<Store>>` because the workers
/// contend on `claim_next` (which already serializes via SQLite
/// `BEGIN IMMEDIATE` — the Mutex is for Rust-level `&mut self` access;
/// SQLite handles inter-connection contention). The expensive work
/// (fetch, decode, transcribe, write) happens *outside* the lock; only
/// `claim_next` / `mark_*` calls hold it, and those are sub-millisecond.
///
/// Alternative considered (per 0025 brainstorm): each worker opens its
/// own `Store::open` connection. Also valid. The `Mutex<Store>` choice
/// keeps the type surface uniform (one Store handle for both serial and
/// pipelined paths). A reviewer should re-check this is fine before T16/
/// T17 commit; if the Mutex shows up as a bottleneck, the alternative is
/// one Store per worker.
pub type SharedStore = std::sync::Arc<tokio::sync::Mutex<Store>>;

/// Phase 2 entry point: pipelined orchestrator.
///
/// **SKELETON ONLY in T15** — returns `Ok(ProcessStats::default())`
/// without doing real work. T16 fills in `fetch_worker`; T17 fills in
/// `transcribe_worker`; T18 wires `JoinSet` + `CancellationToken` +
/// shutdown ordering per 0025 and changes `main.rs`'s `--pipelined`
/// branch to call this.
///
/// Signature note: the `&WhisperEngine` parameter matches the T15 brief.
/// T18 may amend to `Arc<dyn Transcriber>` (the `WhisperEngineHandle`
/// pattern) if the worker structure needs an owned shared handle for
/// `tokio::spawn`.
#[allow(clippy::needless_pass_by_value, dead_code)]
pub async fn run_pipelined(
    _store: SharedStore,
    _fetcher: &dyn VideoFetcher,
    _engine: &WhisperEngine,
    _opts: ProcessOptions,
) -> Result<ProcessStats> {
    // T16/T17/T18 fill this in. Returning empty stats keeps the type
    // signature stable so callers compile.
    Ok(ProcessStats::default())
}
