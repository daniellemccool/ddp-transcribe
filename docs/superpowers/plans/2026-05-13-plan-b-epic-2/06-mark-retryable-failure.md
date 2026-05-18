# Task 6 — `Store::mark_retryable_failure(video_id, worker_id, kind, message)`

**Goal:** New `Store` mutator that flips a row from `in_progress` to `failed_retryable` and records the kind + message in the new columns. Same `WHERE status='in_progress' AND claimed_by = ?` predicate as the tightened `mark_succeeded` (0023 symmetric). Returns `Result<usize>` per 0006.

**ADRs touched:** 0006 (return shape), 0023 (predicate + signature).

**Files:**
- Modify: `src/state/mod.rs` (`mark_retryable_failure` mutator + doc comment)
- Modify: `tests/state_claims.rs` (round-trip test for the mutator)

**Pre-reqs:** T4 (the four new columns exist), T5 (mark_succeeded predicate convention established).

---

- [ ] **Step 1: Write the failing tests**

Append to `tests/state_claims.rs`:

```rust
#[test]
fn mark_retryable_failure_flips_status_and_records_columns() -> anyhow::Result<()> {
    use uu_tiktok::state::Store;
    use rusqlite::Connection;

    let tmp = tempfile::TempDir::new()?;
    let path = tmp.path().join("state.sqlite");
    let mut store = Store::open(&path)?;
    store.upsert_video("vid_a", "https://example/a", false)?;
    let claim = store.claim_next("worker-1")?.expect("claim");

    let changed = store.mark_retryable_failure(
        &claim.video_id,
        "worker-1",
        "FetchTimeout",
        "yt-dlp exceeded 300s budget",
    )?;
    assert_eq!(changed, 1);

    let raw = Connection::open(&path)?;
    let (status, rk, rm): (String, Option<String>, Option<String>) = raw.query_row(
        "SELECT status, last_retryable_kind, last_retryable_message
         FROM videos WHERE video_id = ?1",
        ["vid_a"],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    assert_eq!(status, "failed_retryable");
    assert_eq!(rk.as_deref(), Some("FetchTimeout"));
    assert_eq!(rm.as_deref(), Some("yt-dlp exceeded 300s budget"));
    Ok(())
}

#[test]
fn mark_retryable_failure_with_stale_claim_returns_zero() -> anyhow::Result<()> {
    use uu_tiktok::state::Store;

    let tmp = tempfile::TempDir::new()?;
    let path = tmp.path().join("state.sqlite");
    let mut store = Store::open(&path)?;
    store.upsert_video("vid_a", "https://example/a", false)?;
    store.claim_next("worker-1")?.expect("claim");

    // Stale: different worker tries to mark.
    let changed = store.mark_retryable_failure(
        "vid_a",
        "worker-OTHER",
        "FetchTimeout",
        "spurious",
    )?;
    assert_eq!(changed, 0, "stale-claim should be rejected by predicate");
    Ok(())
}
```

Run:

```bash
cargo test --features test-helpers --test state_claims
```

Expected: FAIL (compile error — `mark_retryable_failure` doesn't exist).

- [ ] **Step 2: Implement the mutator**

In `src/state/mod.rs`, alongside `mark_succeeded`:

```rust
/// Flip a video row from `in_progress` to `failed_retryable`, recording
/// the failure classification (kind + message) per 0023. Same
/// stale-claim predicate as `mark_succeeded`. Returns the row-change
/// count per 0006: 0 on stale claim, 1 on successful flip.
///
/// `kind` is a stable short tag (e.g. "FetchTimeout", "TranscribeError").
/// Epic 3's typed RetryableKind serializes via tag()/message() into the
/// same columns; no schema change at that point — just the caller switching
/// from string literals to enum projections.
pub fn mark_retryable_failure(
    &mut self,
    video_id: &str,
    worker_id: &str,
    kind: &str,
    message: &str,
) -> Result<usize> {
    let now = unix_now();
    let tx = self
        .conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .context("begin immediate for mark_retryable_failure")?;

    let changed = tx
        .execute(
            "UPDATE videos
             SET status = 'failed_retryable',
                 last_retryable_kind = ?2,
                 last_retryable_message = ?3,
                 claimed_by = NULL,
                 claimed_at = NULL,
                 updated_at = ?4
             WHERE video_id = ?1
               AND status = 'in_progress'
               AND claimed_by = ?5",
            params![video_id, kind, message, now, worker_id],
        )
        .with_context(|| format!("update videos for failed_retryable {}", video_id))?;

    if changed > 0 {
        tx.execute(
            "INSERT INTO video_events (video_id, at, event_type, worker_id, detail_json)
             VALUES (?1, ?2, 'failed_retryable', ?3, ?4)",
            params![
                video_id,
                now,
                worker_id,
                // Compact JSON for the detail; rich enough for ops eyeballing.
                format!(r#"{{"kind":"{}","message":"{}"}}"#,
                    kind.replace('"', "\\\""),
                    message.replace('"', "\\\""))
            ],
        )?;
    }

    tx.commit().context("commit mark_retryable_failure")?;
    Ok(changed)
}
```

Design note: `claimed_by` and `claimed_at` are cleared on transition to `failed_retryable` so a subsequent `claim_next` doesn't see the stale claim. 0024's sweep handles the operator-recovery path for `in_progress` rows; this mutator handles the application-noticed-failure path.

- [ ] **Step 3: Run the tests**

```bash
cargo test --features test-helpers --test state_claims
```

Expected: PASS for both new tests + all existing.

- [ ] **Step 4: cargo fmt + clippy**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
```

Expected: clean. Clippy may flag the manual JSON construction (`format!("{{...}}")`); accept the lint with a one-line `#[allow]` or switch to `serde_json::json!` — `serde_json` is already a dep. Recommended: switch.

```rust
let detail = serde_json::json!({ "kind": kind, "message": message }).to_string();
tx.execute(
    "INSERT INTO video_events (video_id, at, event_type, worker_id, detail_json)
     VALUES (?1, ?2, 'failed_retryable', ?3, ?4)",
    params![video_id, now, worker_id, detail],
)?;
```

- [ ] **Step 5: Commit**

```bash
git add src/state/mod.rs tests/state_claims.rs
git commit -m "$(cat <<'EOF'
feat(state): mark_retryable_failure mutator (0006, 0023)

Flips status='in_progress' → 'failed_retryable' when the application
notices a transient failure (fetch timeout, transcribe error). Records
kind + message in last_retryable_kind / last_retryable_message; clears
claimed_by/claimed_at so claim_next doesn't see a phantom claim.

Symmetric stale-claim predicate (status='in_progress' AND claimed_by=?)
returns 0 on the wrong-worker case per 0023.

Tests: happy path (status flips, columns populated, video_events row
inserted) + stale-claim path (returns 0, row unchanged).

`kind` is a string today per 0023. Epic 3 introduces typed
RetryableKind that serializes into the same columns via tag()/message();
no schema change.

Refs: 0006, 0023

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-check

- [ ] `cargo test --features test-helpers --test state_claims` passes (new tests + existing)
- [ ] `claimed_by` / `claimed_at` are NULL after the flip
- [ ] video_events row is inserted with `event_type='failed_retryable'` and JSON detail
- [ ] Stale-claim path returns 0 without inserting an event row
- [ ] Clippy/fmt clean
