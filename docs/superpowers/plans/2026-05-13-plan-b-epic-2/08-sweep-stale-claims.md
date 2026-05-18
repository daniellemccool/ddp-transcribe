# Task 8 — `Store::sweep_stale_claims(threshold)` for operator-recovery on crashed processes

**Goal:** New mutator that flips rows with `status='in_progress' AND claimed_at < (now - threshold)` back to `status='pending'`, clearing `claimed_by`/`claimed_at` and touching `updated_at`. **No `attempt_count` bump** (operator-recovery semantics, not application-retry per AD0023). **No artifact validation** (AD0023 explicitly defers validate-and-mark-succeeded). Returns `Result<usize>` per AD0006.

**ADRs touched:** AD0006, AD0023.

**Files:**
- Modify: `src/state/mod.rs` (`sweep_stale_claims` mutator)
- Modify: `tests/state_claims.rs` (sweep tests: stale row recovered; fresh row left alone; idempotent)

**Pre-reqs:** T4 (schema v2 — though sweep doesn't touch the new columns, integration tests assume v2).

---

- [ ] **Step 1: Write the failing tests**

Append to `tests/state_claims.rs`:

```rust
#[test]
fn sweep_stale_claims_recovers_stale_row() -> anyhow::Result<()> {
    use std::time::Duration;
    use rusqlite::Connection;
    use uu_tiktok::state::Store;

    let tmp = tempfile::TempDir::new()?;
    let path = tmp.path().join("state.sqlite");
    let mut store = Store::open(&path)?;
    store.upsert_video("vid_a", "https://example/a", false)?;
    let _claim = store.claim_next("worker-crashed")?.expect("claim");

    // Backdate claimed_at via a raw UPDATE so the row appears stale.
    {
        let raw = Connection::open(&path)?;
        raw.execute(
            "UPDATE videos SET claimed_at = ?1 WHERE video_id = 'vid_a'",
            [0_i64], // 1970 — definitely stale
        )?;
    }

    let recovered = store.sweep_stale_claims(Duration::from_secs(60))?;
    assert_eq!(recovered, 1);

    // Confirm row is back to pending with cleared claim metadata.
    let row = store.get_video_for_test("vid_a")?.expect("row present");
    assert_eq!(row.status, "pending");

    let raw = Connection::open(&path)?;
    let (cb, ca): (Option<String>, Option<i64>) = raw.query_row(
        "SELECT claimed_by, claimed_at FROM videos WHERE video_id = 'vid_a'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    assert_eq!(cb, None);
    assert_eq!(ca, None);

    // attempt_count is NOT bumped by sweep (AD0023).
    assert_eq!(row.attempt_count, 1, "attempt_count unchanged by sweep");
    Ok(())
}

#[test]
fn sweep_stale_claims_leaves_fresh_claim_alone() -> anyhow::Result<()> {
    use std::time::Duration;
    use uu_tiktok::state::Store;

    let tmp = tempfile::TempDir::new()?;
    let path = tmp.path().join("state.sqlite");
    let mut store = Store::open(&path)?;
    store.upsert_video("vid_a", "https://example/a", false)?;
    let _claim = store.claim_next("worker-1")?.expect("claim");

    // No backdating — claimed_at is `unix_now()`, well within any sane
    // threshold.
    let recovered = store.sweep_stale_claims(Duration::from_secs(60 * 60 * 24))?;
    assert_eq!(recovered, 0, "fresh claim should not be swept");

    let row = store.get_video_for_test("vid_a")?.expect("row present");
    assert_eq!(row.status, "in_progress");
    Ok(())
}

#[test]
fn sweep_stale_claims_is_idempotent() -> anyhow::Result<()> {
    use std::time::Duration;
    use uu_tiktok::state::Store;

    let tmp = tempfile::TempDir::new()?;
    let path = tmp.path().join("state.sqlite");
    let mut store = Store::open(&path)?;
    store.upsert_video("vid_a", "https://example/a", false)?;
    store.claim_next("w1")?;

    // First sweep on no-stale: 0.
    assert_eq!(store.sweep_stale_claims(Duration::from_secs(60 * 60 * 24))?, 0);
    // Second sweep on no-stale: still 0.
    assert_eq!(store.sweep_stale_claims(Duration::from_secs(60 * 60 * 24))?, 0);
    Ok(())
}
```

Run:

```bash
cargo test --features test-helpers --test state_claims
```

Expected: FAIL (compile error — `sweep_stale_claims` doesn't exist).

- [ ] **Step 2: Implement the mutator**

In `src/state/mod.rs` alongside the other mutators:

```rust
/// Recover rows abandoned by a crashed process. Flips rows with
/// `status='in_progress' AND claimed_at < (now - threshold)` back to
/// `status='pending'`, clearing claimed_by/claimed_at. Returns the
/// row-change count (per AD0006).
///
/// Per AD0023: no artifact validation, no attempt_count bump. The
/// sweep is operator-recovery semantics; application-retry semantics
/// (and the `attempt_count` ladder) belong to Epic 3's classifier.
pub fn sweep_stale_claims(&mut self, threshold: std::time::Duration) -> Result<usize> {
    let now = unix_now();
    let threshold_secs = threshold.as_secs() as i64;
    let cutoff = now - threshold_secs;

    let changed = self
        .conn
        .execute(
            "UPDATE videos
             SET status = 'pending',
                 claimed_by = NULL,
                 claimed_at = NULL,
                 updated_at = ?1
             WHERE status = 'in_progress'
               AND claimed_at IS NOT NULL
               AND claimed_at < ?2",
            params![now, cutoff],
        )
        .context("UPDATE videos for sweep_stale_claims")?;

    if changed > 0 {
        tracing::info!(recovered = changed, threshold_secs, "sweep_stale_claims");
    }

    Ok(changed)
}
```

Design notes:

- `claimed_at IS NOT NULL`: defense-in-depth. In v2 schema all `in_progress` rows have non-NULL `claimed_at` (set by `claim_next`); the IS-NOT-NULL check protects against a malformed DB.
- No `video_events` row inserted: per AD0023, sweep is operator-recovery, not an application event. A single tracing line per recovered row (well, per sweep call) is the operational record.
- Threshold is `Duration` for ergonomics; converted to seconds internally because SQLite stores `claimed_at` as a Unix epoch integer.

- [ ] **Step 3: Run the tests**

```bash
cargo test --features test-helpers --test state_claims
```

Expected: PASS for all three new tests.

- [ ] **Step 4: Full suite + clippy**

```bash
cargo test --features test-helpers
cargo fmt --all && cargo clippy --all-targets -- -D warnings
```

Expected: clean. T9 will wire `sweep_stale_claims` into `run_serial`; until then the bin compile doesn't reference the method but the public API is consumed by the integration test, so no dead-code lint fires on the bin path (the method is `pub`, called from a test that compiles into a separate test binary).

- [ ] **Step 5: Commit**

```bash
git add src/state/mod.rs tests/state_claims.rs
git commit -m "$(cat <<'EOF'
feat(state): sweep_stale_claims mutator for crash-recovery (AD0006, AD0023)

Flips rows with status='in_progress' AND claimed_at < (now - threshold)
back to pending. Clears claimed_by/claimed_at. Threshold is a Duration
(converted to seconds internally for the SQLite epoch comparison).

Per AD0023:
- No artifact validation (validate-and-mark-succeeded deferred to Plan C)
- No attempt_count bump (operator-recovery is not application-retry;
  Epic 3's classifier owns the retry ladder)
- No video_events row inserted (sweep is an operator action, not an
  application event); tracing::info!() records the recovery count

Tests cover: stale row recovered (status flips, claim metadata cleared,
attempt_count unchanged); fresh claim left alone; idempotent on no-stale.

T9 wires the sweep into the top of run_serial; this task lands the
mutator surface independently.

Refs: AD0006, AD0023

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-check

- [ ] `cargo test --features test-helpers --test state_claims` passes (all three new tests + existing)
- [ ] Sweep does NOT touch `attempt_count`
- [ ] Sweep does NOT insert a video_events row
- [ ] No `video_events` table pollution from sweep
- [ ] `claimed_by` / `claimed_at` are NULL after recovery
- [ ] Clippy/fmt clean
