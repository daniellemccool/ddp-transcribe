# Task 04: `Store::record_fetch_failure` — the failure-time requeue/exhaust/park decision

**Files:**
- Modify: `src/state/mod.rs` (new outcome enum + mutator, placed directly after `mark_retryable_failure`)
- Create: `tests/state_retry.rs`
- Modify: `Cargo.toml` (add `[[test]] name = "state_retry"` with `required-features = ["test-helpers"]`)

**Interfaces:**
- Consumes: nothing typed from other tasks (labels and cookie policy arrive as plain `&str`/`bool` — this task is independent of 01/03 and can be reviewed standalone).
- Produces (Task 06 relies on these EXACT shapes):

```rust
pub enum FailureRecordOutcome {
    /// Row went back to 'pending' (end of queue via T05 ordering).
    Requeued,
    /// Attempt cap reached — row parked in 'failed_retryable' (exhausted pool).
    Exhausted,
    /// requires-cookie failure with no cookies configured this run — parked
    /// in 'failed_retryable' without consuming the remaining retry budget.
    ParkedForCookies,
    /// Claim predicate missed (concurrent sweep re-claimed the row) — no
    /// mutation happened; caller counts it as stale_after_failure.
    StaleClaim,
}

pub fn record_fetch_failure(
    &mut self,
    video_id: &str,
    worker_id: &str,
    label: &str,
    message: &str,
    max_attempts: i64,          // retries + 1, computed by the caller
    requires_cookie: bool,
    cookies_configured: bool,
) -> Result<FailureRecordOutcome>
```

- Dead-code: both new items get `#[allow(dead_code)]` + `// 0002: consumed by Epic 4a T06 (worker dispatch); lift when it lands.` + commit-message note.

**Semantics (binding, from the design spec §2):**
- `attempt_count` was ALREADY bumped by `claim_next` at claim time — it counts actual fetch attempts, lifetime. This mutator never touches it.
- Always records `last_retryable_kind = label`, `last_retryable_message = message` (whatever the destination status).
- Decision order inside ONE `BEGIN IMMEDIATE` transaction, all predicates carrying the stale-claim guard `status='in_progress' AND claimed_by = ?worker`:
  1. `requires_cookie && !cookies_configured` → UPDATE to `failed_retryable` (park). Event `cookie_parked`.
  2. else UPDATE to `pending` with `AND attempt_count < ?max_attempts` in the predicate (race-free cap, same pattern as the old `requeue_retryable`). If it changes 1 row → `Requeued`, event `retry_requeued`, and clear `claimed_by`/`claimed_at` (a pending row must be unowned).
  3. else UPDATE to `failed_retryable` (cap exhausted) → `Exhausted`, event `failed_retryable` (the existing event vocabulary — an exhausted row looks exactly like Epic 3's terminal-state-of-the-batch row).
  4. If that also changes 0 rows → `StaleClaim`, no event (nothing changed; symmetric with `handle_mutator_result`'s Ok(0) convention).
- 0006 note for the doc comment: the `Result<usize>` contract is honored internally — each UPDATE's row count drives the outcome; the typed enum IS the row-count information, made unambiguous.

- [ ] **Step 1: Write the failing tests**

Create `tests/state_retry.rs`:

```rust
#![allow(clippy::unwrap_used, clippy::expect_used)]

use ddp_transcribe::state::{FailureRecordOutcome, Store};
use tempfile::TempDir;

fn store_with_claimed_row(tmp: &TempDir, attempts_before_claim: i64) -> (Store, String) {
    let mut store = Store::open(&tmp.path().join("state.sqlite")).expect("open");
    store.upsert_video("vid_a", "https://example/a", false).unwrap();
    if attempts_before_claim > 0 {
        let conn = rusqlite::Connection::open(tmp.path().join("state.sqlite")).unwrap();
        conn.execute(
            "UPDATE videos SET attempt_count = ?1 WHERE video_id = 'vid_a'",
            [attempts_before_claim],
        )
        .unwrap();
    }
    let claim = store.claim_next("w1").unwrap().expect("claim");
    (store, claim.video_id)
}

fn status_of(tmp: &TempDir, id: &str) -> (String, Option<String>, i64) {
    let conn = rusqlite::Connection::open(tmp.path().join("state.sqlite")).unwrap();
    conn.query_row(
        "SELECT status, last_retryable_kind, attempt_count FROM videos WHERE video_id = ?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .unwrap()
}

#[test]
fn under_cap_requeues_to_pending_unowned() {
    let tmp = TempDir::new().unwrap();
    // attempt_count is now 1 (claim bumped 0→1); cap 2 ⇒ retry budget remains.
    let (mut store, id) = store_with_claimed_row(&tmp, 0);
    let out = store
        .record_fetch_failure(&id, "w1", "NoDataBlocks", "msg", 2, false, false)
        .unwrap();
    assert!(matches!(out, FailureRecordOutcome::Requeued));
    let (status, kind, attempts) = status_of(&tmp, &id);
    assert_eq!(status, "pending");
    assert_eq!(kind.as_deref(), Some("NoDataBlocks"));
    assert_eq!(attempts, 1, "mutator must NOT bump attempts (claim_next owns that)");
    // Pending rows must be unowned.
    let conn = rusqlite::Connection::open(tmp.path().join("state.sqlite")).unwrap();
    let claimed_by: Option<String> = conn
        .query_row("SELECT claimed_by FROM videos WHERE video_id = ?1", [id], |r| r.get(0))
        .unwrap();
    assert!(claimed_by.is_none());
}

#[test]
fn at_cap_lands_in_exhausted_pool() {
    let tmp = TempDir::new().unwrap();
    // Seeded at 1, claim bumps to 2; cap 2 ⇒ attempt_count < 2 is false.
    let (mut store, id) = store_with_claimed_row(&tmp, 1);
    let out = store
        .record_fetch_failure(&id, "w1", "NoDataBlocks", "msg", 2, false, false)
        .unwrap();
    assert!(matches!(out, FailureRecordOutcome::Exhausted));
    assert_eq!(status_of(&tmp, &id).0, "failed_retryable");
}

#[test]
fn requires_cookie_without_cookies_parks_regardless_of_budget() {
    let tmp = TempDir::new().unwrap();
    let (mut store, id) = store_with_claimed_row(&tmp, 0);
    let out = store
        .record_fetch_failure(&id, "w1", "SensitiveLoginGated", "msg", 2, true, false)
        .unwrap();
    assert!(matches!(out, FailureRecordOutcome::ParkedForCookies));
    let (status, kind, _) = status_of(&tmp, &id);
    assert_eq!(status, "failed_retryable");
    assert_eq!(kind.as_deref(), Some("SensitiveLoginGated"));
}

#[test]
fn requires_cookie_with_cookies_requeues() {
    let tmp = TempDir::new().unwrap();
    let (mut store, id) = store_with_claimed_row(&tmp, 0);
    let out = store
        .record_fetch_failure(&id, "w1", "SensitiveLoginGated", "msg", 2, true, true)
        .unwrap();
    assert!(matches!(out, FailureRecordOutcome::Requeued));
    assert_eq!(status_of(&tmp, &id).0, "pending");
}

#[test]
fn stale_claim_mutates_nothing() {
    let tmp = TempDir::new().unwrap();
    let (mut store, id) = store_with_claimed_row(&tmp, 0);
    let out = store
        .record_fetch_failure(&id, "DIFFERENT-WORKER", "NoDataBlocks", "msg", 2, false, false)
        .unwrap();
    assert!(matches!(out, FailureRecordOutcome::StaleClaim));
    assert_eq!(status_of(&tmp, &id).0, "in_progress", "row untouched");
}

#[test]
fn events_record_each_outcome() {
    let tmp = TempDir::new().unwrap();
    let (mut store, id) = store_with_claimed_row(&tmp, 0);
    store
        .record_fetch_failure(&id, "w1", "NoDataBlocks", "msg", 2, false, false)
        .unwrap();
    let events = store.get_events_for_test(&id).unwrap();
    assert!(
        events.iter().any(|e| e.event_type == "retry_requeued"),
        "requeue must leave a retry_requeued event; got {events:?}"
    );
}
```

Add the `[[test]]` block to `Cargo.toml` after `state_batch_runs`:

```toml
[[test]]
name = "state_retry"
required-features = ["test-helpers"]
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --features test-helpers --test state_retry -- --test-threads=1`
Expected: COMPILE FAILURE — `FailureRecordOutcome` / `record_fetch_failure` not found. (If `get_events_for_test`'s `EventRow.event_type` field has a different name, check `src/state/mod.rs:753-763` and match it — the struct exists.)

- [ ] **Step 3: Implement the mutator**

Two additions to `src/state/mod.rs`. First the outcome enum at module level, placed next to `Claim` (NOT inside the impl block — Rust doesn't allow enum definitions there):

```rust
/// Outcome of `record_fetch_failure`'s one-transaction decision (Epic 4a):
/// where did the failed row land, and did anything change at all.
// 0002: consumed by Epic 4a T06 (worker dispatch); lift when it lands.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureRecordOutcome {
    Requeued,
    Exhausted,
    ParkedForCookies,
    StaleClaim,
}
```

And inside `impl Store` (after `mark_retryable_failure`):

```rust
    /// Failure-time retry decision (Epic 4a, supersedes the Epic 3 pattern
    /// of always parking in failed_retryable). One IMMEDIATE transaction:
    ///
    /// - requires-cookie without cookies configured → park (failed_retryable);
    ///   a cookie-less retry is a guaranteed refail that would burn budget.
    /// - under the cap (`attempt_count < max_attempts`; attempt_count was
    ///   already bumped at claim time by claim_next) → back to 'pending',
    ///   unowned, rejoining the queue behind fresh work (T05 ordering).
    /// - cap exhausted → failed_retryable (the "exhausted, adjudicate" pool).
    /// - claim predicate miss everywhere → StaleClaim, nothing recorded.
    ///
    /// Always writes label+message to last_retryable_kind/_message on any
    /// row it changes. Events: 'cookie_parked' / 'retry_requeued' /
    /// 'failed_retryable' (existing vocabulary for the exhausted case).
    // 0002: consumed by Epic 4a T06 (worker dispatch); lift when it lands.
    #[allow(dead_code)]
    #[allow(clippy::too_many_arguments)] // one logical decision; every arg participates
    pub fn record_fetch_failure(
        &mut self,
        video_id: &str,
        worker_id: &str,
        label: &str,
        message: &str,
        max_attempts: i64,
        requires_cookie: bool,
        cookies_configured: bool,
    ) -> Result<FailureRecordOutcome> {
        let now = unix_now();
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .context("begin immediate for record_fetch_failure")?;

        let park = |tx: &rusqlite::Transaction<'_>, event: &str| -> Result<usize> {
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
                    params![video_id, label, message, now, worker_id],
                )
                .with_context(|| format!("record_fetch_failure park for {video_id}"))?;
            if changed > 0 {
                let detail =
                    serde_json::json!({ "label": label, "message": message }).to_string();
                tx.execute(
                    "INSERT INTO video_events (video_id, at, event_type, worker_id, detail_json)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![video_id, now, event, worker_id, detail],
                )
                .with_context(|| format!("record_fetch_failure {event} event for {video_id}"))?;
            }
            Ok(changed)
        };

        let outcome = if requires_cookie && !cookies_configured {
            if park(&tx, "cookie_parked")? > 0 {
                FailureRecordOutcome::ParkedForCookies
            } else {
                FailureRecordOutcome::StaleClaim
            }
        } else {
            let requeued = tx
                .execute(
                    "UPDATE videos
                     SET status = 'pending',
                         last_retryable_kind = ?2,
                         last_retryable_message = ?3,
                         claimed_by = NULL,
                         claimed_at = NULL,
                         updated_at = ?4
                     WHERE video_id = ?1
                       AND status = 'in_progress'
                       AND claimed_by = ?5
                       AND attempt_count < ?6",
                    params![video_id, label, message, now, worker_id, max_attempts],
                )
                .with_context(|| format!("record_fetch_failure requeue for {video_id}"))?;
            if requeued > 0 {
                let detail = serde_json::json!({
                    "label": label, "max_attempts": max_attempts
                })
                .to_string();
                tx.execute(
                    "INSERT INTO video_events (video_id, at, event_type, worker_id, detail_json)
                     VALUES (?1, ?2, 'retry_requeued', ?3, ?4)",
                    params![video_id, now, worker_id, detail],
                )
                .with_context(|| format!("record_fetch_failure retry_requeued event for {video_id}"))?;
                FailureRecordOutcome::Requeued
            } else if park(&tx, "failed_retryable")? > 0 {
                FailureRecordOutcome::Exhausted
            } else {
                FailureRecordOutcome::StaleClaim
            }
        };

        tx.commit().context("commit record_fetch_failure")?;
        Ok(outcome)
    }
```

- [ ] **Step 4: Run the new tests**

Run: `cargo test --features test-helpers --test state_retry -- --test-threads=1`
Expected: 6/6 PASS.

- [ ] **Step 5: Full verification, then commit**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Expected: green.

```bash
git add src/state/mod.rs tests/state_retry.rs Cargo.toml
git commit -m "feat(state): record_fetch_failure — one-transaction requeue/exhaust/park decision (Epic 4a)

0002 dead-code note: FailureRecordOutcome + record_fetch_failure carry
allow(dead_code) with lift point Epic 4a T06 (worker dispatch)."
```
