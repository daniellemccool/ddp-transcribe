# Task 05: End-of-queue claim ordering (`attempt_count ASC`)

**Files:**
- Modify: `src/state/mod.rs` (`claim_next`'s SELECT, ~line 329-333, plus its doc comment)
- Modify: `tests/state_claims.rs` (new ordering test)

**Interfaces:**
- Consumes: Task 02's `idx_videos_pending_v3 (status, attempt_count, first_seen_at, video_id)` — the query below is exactly that index's shape.
- Produces: the claim-ordering CONTRACT Tasks 06/07 rely on: fresh videos (`attempt_count = 0`) drain before any retry; retries drain FIFO among themselves. This amends the Epic-2 claim contract — Task 08's ADR slate records it (ADR "retry semantics + claim ordering").

- [ ] **Step 1: Write the failing test**

In `tests/state_claims.rs`, append (copy the file's existing helper usage — it has `fresh_store_with`-style helpers; follow the file's local idiom for seeding):

```rust
/// Epic 4a: retries rejoin the queue BEHIND fresh work. A row that failed
/// and was requeued (attempt_count = 1) must not be re-claimed while any
/// never-attempted row (attempt_count = 0) is still pending — regardless
/// of first_seen_at order.
#[test]
fn fresh_rows_claim_before_requeued_retries() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = tmp.path().join("state.sqlite");
    let mut store = ddp_transcribe::state::Store::open(&db).unwrap();

    // OLDER row that has already been attempted once and requeued…
    store.upsert_video("vid_retry", "https://example/r", false).unwrap();
    // …and a NEWER never-attempted row.
    std::thread::sleep(std::time::Duration::from_secs(1)); // distinct first_seen_at
    store.upsert_video("vid_fresh", "https://example/f", false).unwrap();

    // Simulate the retry state directly: attempted once, back in pending.
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "UPDATE videos SET attempt_count = 1 WHERE video_id = 'vid_retry'",
        [],
    )
    .unwrap();

    let first = store.claim_next("w1").unwrap().expect("first claim");
    assert_eq!(
        first.video_id, "vid_fresh",
        "fresh work must drain before retries even when the retry is older"
    );
    let second = store.claim_next("w1").unwrap().expect("second claim");
    assert_eq!(second.video_id, "vid_retry");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --features test-helpers --test state_claims fresh_rows_claim_before_requeued_retries -- --test-threads=1`
Expected: FAIL — `first.video_id` is `"vid_retry"` (older `first_seen_at` wins under the current ordering).

- [ ] **Step 3: Change the ORDER BY**

In `src/state/mod.rs` `claim_next` (~line 329), change the SELECT to:

```sql
SELECT video_id, source_url, attempt_count, last_retryable_kind
FROM videos
WHERE status = 'pending'
ORDER BY attempt_count ASC, first_seen_at ASC, video_id ASC
LIMIT 1
```

Update `claim_next`'s doc comment: replace "claim the oldest pending video" with:

```rust
    /// Atomically claim the next pending video: fresh work first
    /// (`attempt_count ASC` — Epic 4a end-of-queue retries), FIFO by
    /// first_seen_at within each attempt tier. Matches
    /// idx_videos_pending_v3's column order.
```

- [ ] **Step 4: Run the test + the neighboring claim tests**

Run: `cargo test --features test-helpers --test state_claims -- --test-threads=1`
Expected: new test PASSES; every pre-existing claim test still passes (they seed same-attempt rows, so FIFO behavior within a tier is unchanged — if any test asserted cross-tier order, read it: the assertion change is exactly this task's contract change and belongs in this commit with a note).

- [ ] **Step 5: Full verification, then commit**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Expected: green.

```bash
git add src/state/mod.rs tests/state_claims.rs
git commit -m "feat(state): claim ordering attempt_count ASC — retries drain behind fresh work (Epic 4a end-of-queue)"
```
