# Task 03 — `claim_next` orders by publication recency

**Files:**
- Modify: `src/state/mod.rs:678-682` (the candidate SELECT)
- Test: `tests/state_claims.rs` (replace `claim_next_orders_by_first_seen_at:40`)

**Interfaces:**
- Consumes: schema v7 / `idx_videos_pending_v4` (Task 02); the claim-order ADR (Task 01).
- Produces: claim order `attempt_count ASC, video_id DESC`. Tasks 05/08's
  tests may rely on deterministic claim order by descending `video_id`.

**Report (ADR-0019):** ≤250 words, STATUS / SUMMARY / CHANGED FILES / DEVIATIONS.

Ground truth: the SELECT at `src/state/mod.rs:678-682` reads

```sql
SELECT video_id, source_url, attempt_count, last_retryable_kind
                 FROM videos
                 WHERE status = 'pending'
                 ORDER BY attempt_count ASC, first_seen_at ASC, video_id ASC
                 LIMIT 1
```

`tests/state_claims.rs` has `fresh_store_with:7` (helper),
`claim_next_orders_by_first_seen_at:40` (asserts the OLD order — replaced
here), `fresh_rows_claim_before_requeued_retries:757` (attempt-tier
precedence — must keep passing untouched).

- [ ] **Step 1: Write the failing test**

Replace `claim_next_orders_by_first_seen_at` (keep the same file section)
with:

```rust
/// Claim-order ADR: within an attempt tier, newest-published first.
/// video_id is a snowflake (upper 32 bits = creation epoch), 19 digits
/// wide (v7 migration guard), so DESC text order = DESC creation time.
#[test]
fn claim_next_orders_by_recency_within_attempt_tier() -> anyhow::Result<()> {
    // Deliberately inserted oldest-first with ascending first_seen_at to
    // prove first_seen_at no longer participates.
    let mut store = fresh_store_with(&[
        ("7600000000000000001", "https://example/old"),
        ("7650000000000000001", "https://example/mid"),
        ("7700000000000000001", "https://example/new"),
    ])?;
    let first = store.claim_next("w")?.expect("row available");
    assert_eq!(first.video_id, "7700000000000000001", "newest claims first");
    let second = store.claim_next("w")?.expect("row available");
    assert_eq!(second.video_id, "7650000000000000001");
    let third = store.claim_next("w")?.expect("row available");
    assert_eq!(third.video_id, "7600000000000000001", "oldest claims last");
    Ok(())
}
```

Adapt the seeding to `fresh_store_with`'s actual signature (check `:7`) —
if it doesn't stagger `first_seen_at`, seed via `upsert_video` calls with
explicit ordering so ascending `first_seen_at` correlates with ascending
`video_id` (the old order would then claim oldest-first, making the test
fail for the real reason).

- [ ] **Step 2: Run it to verify it fails for the real reason**

Run: `cargo test --features test-helpers --test state_claims -- --test-threads=1 claim_next_orders_by_recency`
Expected: FAIL with `first.video_id == "7600000000000000001"` (old order
claims oldest/earliest-seen first).

- [ ] **Step 3: Implement**

Change the SELECT at `src/state/mod.rs:678-682` to:

```sql
SELECT video_id, source_url, attempt_count, last_retryable_kind
                 FROM videos
                 WHERE status = 'pending'
                 ORDER BY attempt_count ASC, video_id DESC
                 LIMIT 1
```

Nothing else in `claim_next` changes (IMMEDIATE tx, UPDATE, `'claimed'`
event, `Claim` construction all stay).

- [ ] **Step 4: Run the claims suite**

Run: `cargo test --features test-helpers --test state_claims -- --test-threads=1`
Expected: PASS — including `fresh_rows_claim_before_requeued_retries`
(attempt precedence unchanged) and
`concurrent_claim_serializes_via_begin_immediate`.

- [ ] **Step 5: Confirm the index carries the query**

Run (any migrated DB, e.g. a test fixture):
`sqlite3 <db> "EXPLAIN QUERY PLAN SELECT video_id FROM videos WHERE status='pending' ORDER BY attempt_count ASC, video_id DESC LIMIT 1;"`
Expected: plan names `idx_videos_pending_v4` and contains no `USE TEMP
B-TREE FOR ORDER BY`. Paste the plan line into the commit body.

- [ ] **Step 6: Full gate and commit**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Commit: `git commit -am "feat(state): claim newest-published first within attempt tiers"`

This closes **Phase 1** — the controller writes `PHASE-1-CLOSE.md` (≤1 page:
what landed, suite state, deviations) and ends its session per ADR-0019.
