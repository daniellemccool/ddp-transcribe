# Task 15 — Pipeline module reshape: `run_pipelined` alongside `run_serial`

**Goal:** Introduce `pipeline::run_pipelined` as a sibling of `run_serial`. Shared helpers (`process_one` or its decomposed parts) stay accessible to both. Module layout decision (single file vs `pipeline/{mod,serial,pipelined}.rs` submodule split) is made at task time per the existing-code-pattern discipline. **This task lands the orchestrator skeleton only** — fetch and transcribe workers are T16 and T17.

**ADRs touched:** — (structural, no ADR; 0025/0027 inform but don't directly land here).

**Files:**
- Modify or restructure: `src/pipeline.rs` → potentially `src/pipeline/{mod,serial,pipelined}.rs`
- Modify: `src/main.rs` (re-export path stays the same: `pipeline::run_serial` and `pipeline::run_pipelined`)
- Modify: `tests/pipeline_fakes.rs` (may need minor adjustments for the new module path; no new test added here)

**Pre-reqs:** T13 (tokio-util available). Phase 1 done.

---

- [ ] **Step 1: Decide module layout**

Inspect the current state of `src/pipeline.rs`:

```bash
wc -l src/pipeline.rs
```

Heuristic:
- **If `pipeline.rs` is ≤ 250 lines after Phase 1**: keep single-file layout. Add `pub async fn run_pipelined(...)` alongside `run_serial`; share `process_one` private helper.
- **If `pipeline.rs` is > 250 lines after Phase 1**: split into `src/pipeline/mod.rs` (re-exports + shared types) + `src/pipeline/serial.rs` (`run_serial`) + `src/pipeline/pipelined.rs` (`run_pipelined` skeleton).

Plan A's convention has been single files until they grow unwieldy; if the line count is borderline (200–250), prefer keeping the single file to minimize diff for downstream tasks. Document the choice in the commit message.

- [ ] **Step 2: Extract the per-video helper (if useful)**

The Phase 1 `process_one` body has roughly four logical phases:

1. **Fetch:** `fetcher.acquire(...)` returns `Acquisition::AudioFile(path)`.
2. **Decode:** `audio::decode_wav(&path)` returns `Vec<f32>`.
3. **Transcribe:** `transcriber.transcribe(samples, per_call, timeout)` returns `TranscribeOutput`.
4. **Write + commit:** atomic_write txt → atomic_write json → `mark_succeeded` → cleanup wav.

In `run_pipelined`, phases 1+2 happen on a fetch worker (per 0027 — fetch workers decode WAV so the transcribe path is lean) and phases 3+4 happen on the transcribe worker. Two options:

(a) **Decompose into helpers**: `pipeline::fetch_and_decode(...)` returns `(Vec<f32>, PathBuf)`; `pipeline::transcribe_and_write(...)` runs phases 3+4. Both `process_one` (serial) and the Phase 2 workers call into these helpers.

(b) **Inline duplication**: keep `process_one` whole; the Phase 2 workers re-implement the same phases inline.

**Recommended: option (a).** Extracting `fetch_and_decode` and `transcribe_and_write` keeps the 0008 artifact-write-before-mark_succeeded invariant in ONE place (`transcribe_and_write`) and lets the orchestrator and serial loop share that discipline. DRY pays off here.

```rust
/// Phase 1+2: fetch the video and decode to samples. Returns the
/// owned samples + the WAV path (for cleanup after mark_succeeded).
///
/// Used by run_serial's process_one AND by Phase 2's fetch_worker.
pub(crate) async fn fetch_and_decode(
    fetcher: &dyn VideoFetcher,
    claim: &Claim,
) -> Result<(Vec<f32>, PathBuf)> {
    let acquisition = fetcher
        .acquire(&claim.video_id, &claim.source_url)
        .await
        .with_context(|| format!("fetching {}", claim.video_id))?;

    #[allow(clippy::infallible_destructuring_match)]
    let wav_path = match acquisition {
        Acquisition::AudioFile(p) => p,
    };
    tracing::info!(video_id = claim.video_id.as_str(), wav = %wav_path.display(), "audio acquired");

    let samples = audio::decode_wav(&wav_path)
        .with_context(|| format!("decoding wav {}", wav_path.display()))?;

    Ok((samples, wav_path))
}

/// Phase 3+4: transcribe → write artifacts → mark_succeeded → cleanup.
///
/// Preserves 0008: artifacts (txt + json) are durable on disk BEFORE
/// mark_succeeded. Cleanup of the WAV happens after the DB commit; a
/// failure here leaves the WAV as disk churn that an operator can sweep.
///
/// Used by run_serial's process_one AND by Phase 2's transcribe_worker.
pub(crate) async fn transcribe_and_write(
    store: &mut Store,
    transcriber: &dyn Transcriber,
    claim: &Claim,
    samples: Vec<f32>,
    wav_path: PathBuf,
    opts: &ProcessOptions,
) -> Result<()> {
    let duration_s = Some(samples.len() as f64 / 16_000.0);

    let per_call = PerCallConfig {
        compute_lang_probs: opts.compute_lang_probs,
        ..PerCallConfig::default()
    };

    let transcribe_output = transcriber
        .transcribe(samples, per_call, opts.transcribe_timeout)
        .await
        .with_context(|| format!("transcribing {}", claim.video_id))?;
    tracing::info!(
        video_id = claim.video_id.as_str(),
        chars = transcribe_output.text.len(),
        language = transcribe_output.language.as_str(),
        "transcribed"
    );

    let shard_dir = opts.transcripts_root.join(shard(&claim.video_id));
    std::fs::create_dir_all(&shard_dir)
        .with_context(|| format!("creating shard dir {}", shard_dir.display()))?;

    // 0008: artifacts before mark_succeeded.
    let txt_path = shard_dir.join(format!("{}.txt", claim.video_id));
    artifacts::atomic_write(&txt_path, transcribe_output.text.as_bytes())
        .with_context(|| format!("writing transcript {}", txt_path.display()))?;

    let metadata = TranscriptMetadata {
        video_id: claim.video_id.clone(),
        source_url: claim.source_url.clone(),
        duration_s,
        language_detected: Some(transcribe_output.language.clone()),
        transcribed_at: Utc::now().to_rfc3339(),
        fetcher: "ytdlp".to_string(), // T17 will pass the actual fetcher name via opts
        transcript_source: transcriber.name().to_string(),
        model: transcribe_output.model_id.clone(),
        raw_signals: Some(RawSignals::from_transcribe_output(&transcribe_output)),
    };
    let json_bytes =
        serde_json::to_vec_pretty(&metadata).context("serializing transcript metadata")?;
    let json_path = shard_dir.join(format!("{}.json", claim.video_id));
    artifacts::atomic_write(&json_path, &json_bytes)?;

    store.mark_succeeded(
        &claim.video_id,
        &opts.worker_id,
        SuccessArtifacts {
            duration_s,
            language_detected: Some(transcribe_output.language.clone()),
            fetcher: "ytdlp", // T17 ditto
            transcript_source: transcriber.name(),
        },
    )?;

    if let Err(e) = std::fs::remove_file(&wav_path) {
        tracing::warn!(path = %wav_path.display(), error = %e, "could not remove wav after success");
    }

    tracing::info!(video_id = claim.video_id.as_str(), "succeeded");
    Ok(())
}
```

**Fetcher name plumbing**: the existing `process_one` uses `fetcher.name()` for both the metadata field and the SuccessArtifacts field. The pipelined orchestrator can pass it via `ProcessOptions::fetcher_name: &'static str` or have the transcribe worker accept a `&dyn VideoFetcher` reference just to call `.name()`. T17 decides — simplest is to add `fetcher_name: &'static str` to `ProcessOptions`.

Update `process_one` to call the new helpers:

```rust
async fn process_one(
    store: &mut Store,
    fetcher: &dyn VideoFetcher,
    transcriber: &dyn Transcriber,
    claim: &Claim,
    opts: &ProcessOptions,
) -> Result<()> {
    tracing::info!(
        video_id = claim.video_id.as_str(),
        attempt = claim.attempt_count,
        "claimed"
    );
    let (samples, wav_path) = fetch_and_decode(fetcher, claim).await?;
    transcribe_and_write(store, transcriber, claim, samples, wav_path, opts).await
}
```

- [ ] **Step 3: Add the `run_pipelined` skeleton**

In `src/pipeline.rs` (or `src/pipeline/pipelined.rs`):

```rust
/// Phase 2 entry point: pipelined orchestrator with N fetch workers
/// feeding 1 transcribe worker over a bounded mpsc channel.
///
/// **SKELETON ONLY in T15** — the fetch/transcribe worker bodies land
/// in T16 and T17; the JoinSet + CancellationToken wiring lands in T18.
/// This function compiles and returns Ok(ProcessStats::default()) so
/// the call from `main` (which T18 wires) can be added on a per-task
/// granularity.
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
```

`SharedStore` is a new type alias. Define alongside:

```rust
/// Shared mutable access to the Store across N fetch workers + 1
/// transcribe worker. Implemented as `Arc<tokio::sync::Mutex<Store>>`
/// because the workers contend on `claim_next` (which already
/// serializes via SQLite BEGIN IMMEDIATE — the Mutex is for Rust-level
/// `&mut self` access; SQLite handles inter-connection contention).
///
/// Alternative: each worker opens its own Connection. 0025 brainstorm
/// noted this is also valid; Mutex<Store> keeps the type surface
/// uniform (one Store handle for both serial and pipelined paths).
pub type SharedStore = std::sync::Arc<tokio::sync::Mutex<Store>>;
```

Note: `Mutex<Store>` is a contention point only at `claim_next`/`mark_*` boundaries — short-held. The expensive work (fetch, decode, transcribe, write) happens *outside* the lock. A reviewer should confirm this is fine before T16/T17 commit; if the Mutex shows up as a bottleneck (unlikely given <1ms hold times), the alternative is one Store per worker.

- [ ] **Step 4: Verify the build**

```bash
cargo build --features test-helpers
```

Expected: clean build. `run_pipelined` is a no-op skeleton; `process_one` now calls helpers; existing tests pass.

```bash
cargo test --features test-helpers
```

Expected: all green. The `pipeline_fakes` tests exercise `run_serial`; they still work because `process_one` still produces the same observable behavior.

- [ ] **Step 5: cargo fmt + clippy**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
```

Expected: clean. The `#[allow(unused_variables)]` warning may fire on the `run_pipelined` skeleton's unused parameters — silence with `_`-prefixed names (already done in the snippet) or `#[allow(unused_variables)]` on the function.

- [ ] **Step 6: Commit**

```bash
git add src/pipeline.rs src/main.rs tests/pipeline_fakes.rs
git commit -m "$(cat <<'EOF'
feat(pipeline): reshape with run_pipelined skeleton + fetch_and_decode/transcribe_and_write helpers

Extracts process_one's body into two helpers so run_serial and Phase 2's
workers share the same artifact-write-before-mark_succeeded invariant
(0008):

- fetch_and_decode(fetcher, claim) -> (Vec<f32>, PathBuf)
  Phase 1+2: acquire + decode WAV. Owned samples + WAV path returned.

- transcribe_and_write(store, transcriber, claim, samples, wav, opts)
  Phase 3+4: transcribe → atomic_write txt → atomic_write json →
  mark_succeeded → cleanup wav. 0008 invariant lives here.

process_one is now a thin caller of both helpers.

Adds run_pipelined SKELETON (returns Ok(ProcessStats::default())) and
the SharedStore = Arc<Mutex<Store>> type alias. T16 fills in
fetch_worker; T17 fills in transcribe_worker; T18 wires the
JoinSet + CancellationToken + shutdown ORDER per 0025.

Module layout: <single file | submodule split> per the existing-code-
pattern discipline. <Rationale in 1-2 sentences>.

Refs: 0008 (preserved by transcribe_and_write being the sole
mark_succeeded caller), 0025 (skeleton), 0027 (skeleton)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-check

- [ ] `cargo build --features test-helpers` clean
- [ ] All Phase 1 tests still pass (helper extraction is a no-op refactor)
- [ ] `process_one` is now a thin caller of `fetch_and_decode` + `transcribe_and_write`
- [ ] `run_pipelined` is callable (returns empty stats skeleton)
- [ ] `SharedStore` type alias is documented
- [ ] Module layout decision noted in the commit message
- [ ] Clippy/fmt clean
