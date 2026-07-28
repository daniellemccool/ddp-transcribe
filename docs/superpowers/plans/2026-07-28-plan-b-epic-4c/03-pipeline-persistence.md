# Task 03: Persist the envelope through both pipeline paths (before outcome dispatch)

> **ARCHIVE NOTE (descoped 2026-07-28):** caption/subtitle collection described below was removed by operator decision at commit afa0253 — the envelope is `{"schema":1,"printed":…}` only. The creator's caption TEXT (`description`, = Research API `video_description`) remains captured via the print template. See task-02-report.md § Descope.

**Files:**
- Modify: `src/pipeline/pipelined.rs` (fetch_worker: insert raw row after the fetch select, before the outcome match)
- Modify: `src/pipeline/serial.rs` (`process_one`: same, with direct `&mut Store`)
- Test: `tests/pipeline_fakes/pipelined_tests.rs` (two new integration tests; file is already registered with `required-features = ["test-helpers"]`)

**Interfaces:**
- Consumes (landed in Tasks 01–02, exact):
  - `Store::upsert_metadata_raw(&mut self, video_id: &str, envelope_json: &str) -> anyhow::Result<usize>`
  - `fetch_and_decode(…) -> (Option<MetadataCapture>, Result<(Vec<f32>, PathBuf), FetchPhaseError>)`; both call sites currently discard as `_metadata_capture` with a `// Epic 4c Task 03 wires persistence` comment.
  - `FakeFetcher.canned_metadata: Mutex<Option<String>>` — `Some(s)` makes every acquire return that envelope.
- Produces: raw-row persistence guarantee — **any fetch attempt whose tool produced an envelope leaves a `video_metadata_raw` row, whether the video subsequently succeeds, fails retryably, or fails terminally.** Best-effort: an insert error logs + counts nothing and never alters the video's outcome (epic invariant).

**Design constraints:**
- Pipelined lock discipline: the store guard is held ONLY for the insert, released before outcome dispatch re-acquires it. Never hold across an await on fetch/transcribe.
- The insert happens BEFORE `classify_fetch_phase` / the success send — mirroring the spec's "before exit-status interpretation".

- [ ] **Step 1: Write the failing integration tests**

In `tests/pipeline_fakes/pipelined_tests.rs`, following the file's existing fixture style (shared tempdir store, `FakeFetcher`, `run_pipelined`-or-worker harness — copy the setup of the nearest existing test that drives a fetch to completion, e.g. the success-path test, and the nearest failure-path test):

```rust
#[tokio::test]
async fn fetch_persists_metadata_raw_row_on_success() {
    // Fixture: one pending video with a canned WAV (existing success-path
    // setup), PLUS canned metadata on the fake fetcher:
    //   *fetcher.canned_metadata.lock().unwrap() =
    //       Some(r#"{"schema":1,"printed":"{\"id\":\"vid_a\"}","captions":null}"#.to_string());
    // Run the pipelined orchestrator to completion (existing harness).
    // Assert (raw rusqlite against the fixture DB):
    //   video_metadata_raw has exactly 1 row for the video, raw_json contains "schema";
    //   the video's status is 'succeeded' (metadata didn't disturb the outcome).
}

#[tokio::test]
async fn fetch_persists_metadata_raw_row_on_classified_failure() {
    // Fixture: one pending video, FakeFetcher::fails_with_stderr("Video not available")
    // (or the file's established retryable stderr) + canned_metadata as above.
    // Run to completion.
    // Assert: video_metadata_raw has the row (failure-path capture!) AND the
    // video landed in failed_retryable/failed_terminal per the canned stderr —
    // i.e. the failure outcome is exactly what this stderr produced before
    // this epic.
}
```

Write these as REAL tests against the file's actual harness (the pseudocode above names the required assertions; the file's existing tests show the orchestration calls, worker-id constants, and DB-open idioms to copy — mirror them exactly).

- [ ] **Step 2: Run to confirm both fail**

Run: `cargo test --features test-helpers --test pipeline_fakes fetch_persists_metadata -- --test-threads=1`
Expected: FAIL — 0 rows in `video_metadata_raw` (capture still discarded at the call sites).

- [ ] **Step 3: Wire the pipelined path**

In `src/pipeline/pipelined.rs` fetch_worker, replace the Task 02 discard line after the fetch select:

```rust
        let (metadata_capture, fetch_result) = fetch_result;

        // Epic 4c: persist the raw envelope BEFORE outcome dispatch so
        // mid-download deaths still leave metadata. Best-effort by
        // invariant — an insert failure must never change the video's
        // pipeline outcome. Guard scope: insert only, released before the
        // outcome match re-locks for failure dispatch.
        if let Some(capture) = metadata_capture {
            let insert = {
                let mut guard = store.lock().await;
                guard.upsert_metadata_raw(&claim.video_id, &capture.envelope_json)
            };
            if let Err(e) = insert {
                tracing::warn!(
                    worker = %worker_id,
                    video_id = claim.video_id.as_str(),
                    error = %e,
                    "metadata raw insert failed; continuing"
                );
            }
        }

        match fetch_result {
            …existing arms unchanged…
```

(Note `claim` is moved into `FetchedItem` in the success arm — the insert block runs before the match, where `claim` is still borrowable. Keep it that way.)

- [ ] **Step 4: Wire the serial path**

In `src/pipeline/serial.rs` `process_one`, replace the discard:

```rust
    let (metadata_capture, fetch_result) = fetch_and_decode(fetcher, claim, &fetch_opts).await;
    // Epic 4c: raw envelope persists regardless of fetch outcome; best-effort.
    if let Some(capture) = metadata_capture {
        if let Err(e) = store.upsert_metadata_raw(&claim.video_id, &capture.envelope_json) {
            tracing::warn!(
                video_id = claim.video_id.as_str(),
                error = %e,
                "metadata raw insert failed; continuing"
            );
        }
    }
    let (samples, wav_path) = fetch_result?;
```

- [ ] **Step 5: Run the two new tests to verify they pass**

Run: `cargo test --features test-helpers --test pipeline_fakes fetch_persists_metadata -- --test-threads=1` → PASS (2/2).

- [ ] **Step 6: Full verification**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Expected: all green (Task 02's 291 + 2 = 293 passed; every pre-existing pipeline test unchanged — if any pre-existing test breaks, that is a signal the wiring changed an outcome path; stop and re-examine rather than adapting the test).

- [ ] **Step 7: Commit**

```bash
git add src/pipeline/pipelined.rs src/pipeline/serial.rs tests/pipeline_fakes/pipelined_tests.rs
git commit -m "feat(pipeline): persist raw metadata envelope before outcome dispatch — both paths, best-effort"
```
