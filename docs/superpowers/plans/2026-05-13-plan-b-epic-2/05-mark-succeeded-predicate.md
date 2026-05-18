# Task 5 — `mark_succeeded` WHERE predicate + round-trip test

**Goal:** Tighten `Store::mark_succeeded` to require `WHERE status='in_progress' AND claimed_by = ?` so calling it on a stale-claimed row returns 0 (no rows updated) instead of silently succeeding. Adds a round-trip test (`claim → mark_succeeded → second claim_next returns the row again if first claim was stale`). Resolves FOLLOWUPS T10 (mark_succeeded doesn't require `status='in_progress'`) and FOLLOWUPS T10 (missing round-trip test).

**ADRs touched:** 0006 (mutators return `Result<usize>`), 0023 (predicate symmetry with retryable/terminal mutators).

**Files:**
- Modify: `src/state/mod.rs` (`mark_succeeded` body; signature gains `worker_id: &str` parameter)
- Modify: `src/pipeline.rs` (caller passes `opts.worker_id`)
- Modify: existing `tests/state_*.rs` that exercise `mark_succeeded` (add the worker_id arg)
- Modify or create: `tests/state_claims.rs` (add the round-trip test)

**Pre-reqs:** T2 (Store::open version check; tests can assume v2 schema).

---

- [ ] **Step 1: Inventory existing callers**

```bash
grep -rn 'mark_succeeded' src/ tests/ 2>&1
```

Capture the list. The signature change is `(video_id, artifacts)` → `(video_id, worker_id, artifacts)`. Every caller needs the `worker_id` argument. Plan A's pipeline already has `opts.worker_id` — propagate it.

- [ ] **Step 2: Write the failing round-trip test**

Append to `tests/state_claims.rs` (or wherever the existing claim tests live):

```rust
#[test]
fn mark_succeeded_with_stale_claim_returns_zero_and_does_not_update() -> Result<()> {
    use uu_tiktok::state::{Store, SuccessArtifacts};

    let tmp = TempDir::new()?;
    let path = tmp.path().join("state.sqlite");
    let mut store = Store::open(&path)?;

    store.upsert_video("vid_a", "https://example/a", false)?;

    let claim = store
        .claim_next("worker-1")?
        .expect("first claim succeeds");
    assert_eq!(claim.video_id, "vid_a");

    // Simulate a different worker calling mark_succeeded with the wrong
    // worker_id (stale claim): the predicate rejects, returns 0.
    let artifacts = SuccessArtifacts {
        duration_s: Some(1.0),
        language_detected: Some("en".to_string()),
        fetcher: "fake",
        transcript_source: "fake",
    };
    let changed = store.mark_succeeded("vid_a", "worker-DIFFERENT", artifacts)?;
    assert_eq!(changed, 0, "stale-claim mark_succeeded should not update");

    // Row should still be in_progress, still claimed by worker-1.
    let row = store
        .get_video_for_test("vid_a")?
        .expect("row still present");
    assert_eq!(row.status, "in_progress");

    Ok(())
}

#[test]
fn claim_then_mark_succeeded_then_reclaim_returns_none() -> Result<()> {
    use uu_tiktok::state::{Store, SuccessArtifacts};

    let tmp = TempDir::new()?;
    let path = tmp.path().join("state.sqlite");
    let mut store = Store::open(&path)?;
    store.upsert_video("vid_a", "https://example/a", false)?;

    let claim = store.claim_next("worker-1")?.expect("first claim");
    let artifacts = SuccessArtifacts {
        duration_s: Some(1.0),
        language_detected: Some("en".to_string()),
        fetcher: "fake",
        transcript_source: "fake",
    };
    let changed = store.mark_succeeded(&claim.video_id, "worker-1", artifacts)?;
    assert_eq!(changed, 1);

    let second = store.claim_next("worker-1")?;
    assert!(second.is_none(), "round-trip: no pending rows left");
    Ok(())
}
```

Run:

```bash
cargo test --features test-helpers --test state_claims
```

Expected: FAIL (compile error: too many arguments to `mark_succeeded`; also `mark_succeeded` doesn't have a stale-claim predicate so the first test would fail at runtime once it compiles).

- [ ] **Step 3: Update `mark_succeeded` signature and body**

In `src/state/mod.rs`, replace the existing `mark_succeeded`:

```rust
pub fn mark_succeeded(
    &mut self,
    video_id: &str,
    worker_id: &str,
    artifacts: SuccessArtifacts,
) -> Result<usize> {
    let now = unix_now();
    let tx = self
        .conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .context("begin immediate for mark_succeeded")?;

    let changed = tx
        .execute(
            "UPDATE videos
             SET status = 'succeeded',
                 succeeded_at = ?2,
                 duration_s = ?3,
                 language_detected = ?4,
                 fetcher = ?5,
                 transcript_source = ?6,
                 updated_at = ?2
             WHERE video_id = ?1
               AND status = 'in_progress'
               AND claimed_by = ?7",
            params![
                video_id,
                now,
                artifacts.duration_s,
                artifacts.language_detected,
                artifacts.fetcher,
                artifacts.transcript_source,
                worker_id,
            ],
        )
        .with_context(|| format!("update videos for succeeded {}", video_id))?;

    // Only insert the event row if the UPDATE matched — symmetry with the
    // mutator's row-change count. 0008 invariant: artifacts are durable
    // before this call regardless of outcome; the event row is bookkeeping
    // for "the DB acknowledged the success."
    if changed > 0 {
        tx.execute(
            "INSERT INTO video_events (video_id, at, event_type, worker_id, detail_json)
             VALUES (?1, ?2, 'succeeded', ?3, NULL)",
            params![video_id, now, worker_id],
        )?;
    }

    tx.commit().context("commit mark_succeeded")?;
    Ok(changed)
}
```

Update the doc-comment above:

```rust
/// Mark a video as succeeded and record a `succeeded` event, atomically.
/// Returns the row-change count from the videos UPDATE per 0006.
///
/// The UPDATE is guarded by `WHERE status='in_progress' AND claimed_by = ?`
/// (0023 symmetric with mark_retryable_failure / mark_terminal_failure):
/// callers can detect "0 means the row was not in_progress or claimed by
/// a different worker (stale claim)" without a separate query. The event
/// row is inserted only when the UPDATE matches, so video_events stays
/// faithful to what actually changed.
```

- [ ] **Step 4: Update the pipeline.rs caller**

In `src/pipeline.rs::process_one`, change the `mark_succeeded` call:

```rust
// 0008: artifacts durable, now mark the row succeeded.
store.mark_succeeded(
    &claim.video_id,
    &opts.worker_id,
    SuccessArtifacts {
        duration_s,
        language_detected: Some(transcribe_output.language.clone()),
        fetcher: fetcher.name(),
        transcript_source: transcriber.name(),
    },
)?;
```

- [ ] **Step 5: Update existing tests that call `mark_succeeded`**

Grep:

```bash
grep -rn 'mark_succeeded(' tests/ src/ 2>&1
```

For each call site, add the `worker_id` argument. The pattern in tests is usually `"test-worker"` or `"worker-1"`; reuse whichever string the surrounding test already uses for `claim_next`.

- [ ] **Step 6: Run the new tests**

```bash
cargo test --features test-helpers --test state_claims
```

Expected: PASS for both new tests + all existing tests in the file.

- [ ] **Step 7: Run the full suite**

```bash
cargo test --features test-helpers
```

Expected: all green.

- [ ] **Step 8: cargo fmt + clippy**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
```

Expected: clean.

- [ ] **Step 9: Commit**

```bash
git add src/state/mod.rs src/pipeline.rs tests/state_claims.rs
git commit -m "$(cat <<'EOF'
feat(state): mark_succeeded gains WHERE status='in_progress' AND claimed_by=? predicate (0023)

Plan A's mark_succeeded had no predicate beyond video_id — a stale claim
(operator-recovered or test-injected) could still flip status to
succeeded. Plan B Epic 2 tightens the predicate to enforce
status='in_progress' AND claimed_by = ? (0023 symmetric with the
retryable/terminal mutators).

Signature change: (video_id, artifacts) → (video_id, worker_id, artifacts).
Callers updated. The UPDATE returns 0 on stale claim; the symmetric event
row insert is gated on changed > 0 so video_events doesn't carry phantom
'succeeded' rows.

Tests (tests/state_claims.rs, --features test-helpers):
- stale-claim mark_succeeded returns 0 and leaves status='in_progress'
- claim → mark_succeeded → second claim_next returns None (round-trip)

Refs: 0006, 0023; FOLLOWUPS T10 (mark_succeeded predicate + missing
round-trip test) resolved.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-check

- [ ] `cargo test --features test-helpers --test state_claims` passes (new and existing tests)
- [ ] All other tests still pass
- [ ] Pipeline-level integration test (e.g. `tests/pipeline_fakes.rs`) still passes — caller updated
- [ ] No `mark_succeeded` call site missed
- [ ] Clippy/fmt clean
