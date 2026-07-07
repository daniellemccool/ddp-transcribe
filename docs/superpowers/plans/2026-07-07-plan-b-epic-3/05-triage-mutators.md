# Task 05: Triage mutators + `failed_retryable` listing

**Files:**
- Modify: `src/state/mod.rs`
- Test: `tests/state_claims.rs` (or a new `tests/state_triage.rs` with `[[test]] required-features = ["test-helpers"]` in `Cargo.toml` — prefer the new file; the triage mutator family is its own concern)

**Interfaces:**
- Consumes: existing `Store`, `unix_now()`, video_events INSERT pattern.
- Produces (Task 10 depends on these exact signatures):
  - `pub struct TriageRow { pub video_id: String, pub last_retryable_kind: Option<String>, pub last_retryable_message: Option<String>, pub attempt_count: i64 }`
  - `Store::list_failed_retryable(&self) -> Result<Vec<TriageRow>>`
  - `Store::triage_mark_terminal(&mut self, video_id: &str, reason: &str, message: &str) -> Result<usize>` — predicate `status = 'failed_retryable'`; event `triaged_terminal`, worker_id `"triage"`.
  - `Store::requeue_retryable(&mut self, video_id: &str, new_kind: &str, max_attempts: i64) -> Result<usize>` — predicate `status = 'failed_retryable' AND attempt_count < max_attempts`; writes `new_kind` back to `last_retryable_kind`; event `requeued`, worker_id `"triage"`. Returns `0` when capped or already transitioned — callers distinguish "capped" by checking the row's `attempt_count` from `TriageRow` before calling.

Note the existing `mark_terminal_failure` (predicate `in_progress AND claimed_by`) is NOT reused here — triage operates on unclaimed rows; these are sibling mutators per 0023's family conventions (Immediate transaction, `Result<usize>`, event row only when the UPDATE matched, diagnostic columns preserved).

- [ ] **Step 1: Write the failing tests** (new `tests/state_triage.rs`; copy the clippy-allow header and fixture-DB helper pattern from `tests/state_claims.rs`)

```rust
#[test]
fn triage_mark_terminal_flips_only_failed_retryable() {
    let (mut store, _tmp) = fresh_store();
    seed_failed_retryable(&mut store, "7000000000000000001", "Fetch", "ERROR: Your IP address is blocked");

    let n = store
        .triage_mark_terminal("7000000000000000001", "IpBlockedMessage", "probe/message write-off")
        .unwrap();
    assert_eq!(n, 1);

    // Second call: predicate misses (already terminal) → 0, no extra event.
    let n2 = store
        .triage_mark_terminal("7000000000000000001", "IpBlockedMessage", "again")
        .unwrap();
    assert_eq!(n2, 0);

    let (status, reason, kept_kind): (String, String, Option<String>) = query_row(
        &store,
        "SELECT status, terminal_reason, last_retryable_kind FROM videos WHERE video_id='7000000000000000001'",
    );
    assert_eq!(status, "failed_terminal");
    assert_eq!(reason, "IpBlockedMessage");
    assert_eq!(kept_kind.as_deref(), Some("Fetch"), "last_retryable_* preserved for audit");
    assert_eq!(count_events(&store, "7000000000000000001", "triaged_terminal"), 1);
}

#[test]
fn requeue_retryable_respects_attempt_cap_and_writes_kind_back() {
    let (mut store, _tmp) = fresh_store();
    seed_failed_retryable(&mut store, "7000000000000000002", "Fetch", "ERROR: Did not get any data blocks");
    // seeded row has attempt_count = 1

    let n = store.requeue_retryable("7000000000000000002", "NoDataBlocks", 3).unwrap();
    assert_eq!(n, 1);
    let (status, kind): (String, Option<String>) = query_row(
        &store,
        "SELECT status, last_retryable_kind FROM videos WHERE video_id='7000000000000000002'",
    );
    assert_eq!(status, "pending");
    assert_eq!(kind.as_deref(), Some("NoDataBlocks"), "requeue normalizes the kind");
    assert_eq!(count_events(&store, "7000000000000000002", "requeued"), 1);

    // At the cap: attempt_count=1, max_attempts=1 → predicate misses.
    seed_failed_retryable(&mut store, "7000000000000000003", "Fetch", "msg");
    let n2 = store.requeue_retryable("7000000000000000003", "NoDataBlocks", 1).unwrap();
    assert_eq!(n2, 0);
}

#[test]
fn list_failed_retryable_returns_message_and_attempts() {
    let (mut store, _tmp) = fresh_store();
    seed_failed_retryable(&mut store, "7000000000000000004", "Fetch", "ERROR: whatever");
    let rows = store.list_failed_retryable().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].video_id, "7000000000000000004");
    assert_eq!(rows[0].attempt_count, 1);
    assert!(rows[0].last_retryable_message.as_deref().unwrap().contains("whatever"));
}
```

Test helpers to write in the file (`seed_failed_retryable` = upsert + claim + `mark_retryable_failure`; `query_row` / `count_events` = thin rusqlite wrappers over a second read connection to the same path — mirror how `state_claims.rs` opens raw connections).

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --features test-helpers --test state_triage -- --test-threads=1`
Expected: compile error — mutators don't exist. (Remember the `Cargo.toml` `[[test]]` block first, or the test target won't build with the feature.)

- [ ] **Step 3: Implement in `src/state/mod.rs`**

```rust
/// One failed_retryable row, as triage sees it. Message included because
/// triage classifies stored messages (fast path) before deciding to probe.
#[derive(Debug)]
pub struct TriageRow {
    pub video_id: String,
    pub last_retryable_kind: Option<String>,
    pub last_retryable_message: Option<String>,
    pub attempt_count: i64,
}

impl Store {
    /// Snapshot of all failed_retryable rows, FIFO by first_seen_at. Read-only.
    pub fn list_failed_retryable(&self) -> Result<Vec<TriageRow>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT video_id, last_retryable_kind, last_retryable_message, attempt_count
                 FROM videos WHERE status = 'failed_retryable'
                 ORDER BY first_seen_at ASC, video_id ASC",
            )
            .context("prepare list_failed_retryable")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(TriageRow {
                    video_id: r.get(0)?,
                    last_retryable_kind: r.get(1)?,
                    last_retryable_message: r.get(2)?,
                    attempt_count: r.get(3)?,
                })
            })
            .context("query list_failed_retryable")?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("collect list_failed_retryable rows")?;
        Ok(rows)
    }

    /// Triage verdict: dead. failed_retryable → failed_terminal. Unlike
    /// mark_terminal_failure (in_progress + claimed_by predicate, pipeline
    /// caller), this operates on unclaimed failed rows; the operator-action
    /// audit trail is the 'triaged_terminal' event. last_retryable_* columns
    /// are preserved (0023 family convention: diagnostics accumulate).
    pub fn triage_mark_terminal(
        &mut self,
        video_id: &str,
        reason: &str,
        message: &str,
    ) -> Result<usize> {
        let now = unix_now();
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .context("begin immediate for triage_mark_terminal")?;
        let changed = tx
            .execute(
                "UPDATE videos
                 SET status = 'failed_terminal',
                     terminal_reason = ?2,
                     terminal_message = ?3,
                     updated_at = ?4
                 WHERE video_id = ?1 AND status = 'failed_retryable'",
                params![video_id, reason, message, now],
            )
            .with_context(|| format!("triage_mark_terminal update for {video_id}"))?;
        if changed > 0 {
            let detail = serde_json::json!({ "reason": reason, "message": message }).to_string();
            tx.execute(
                "INSERT INTO video_events (video_id, at, event_type, worker_id, detail_json)
                 VALUES (?1, ?2, 'triaged_terminal', 'triage', ?3)",
                params![video_id, now, detail],
            )
            .with_context(|| format!("triage_mark_terminal event for {video_id}"))?;
        }
        tx.commit().context("commit triage_mark_terminal")?;
        Ok(changed)
    }

    /// Triage verdict: alive. failed_retryable → pending, gated by the
    /// attempt cap IN THE PREDICATE (race-free: the cap check and the flip
    /// are one statement). Writes the re-classified kind back so historical
    /// placeholder kinds ("Fetch") become taxonomy kinds before the row is
    /// claimable — cookie routing (ADR 0035) reads the kind at claim time.
    pub fn requeue_retryable(
        &mut self,
        video_id: &str,
        new_kind: &str,
        max_attempts: i64,
    ) -> Result<usize> {
        let now = unix_now();
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .context("begin immediate for requeue_retryable")?;
        let changed = tx
            .execute(
                "UPDATE videos
                 SET status = 'pending',
                     last_retryable_kind = ?2,
                     updated_at = ?3
                 WHERE video_id = ?1
                   AND status = 'failed_retryable'
                   AND attempt_count < ?4",
                params![video_id, new_kind, now, max_attempts],
            )
            .with_context(|| format!("requeue_retryable update for {video_id}"))?;
        if changed > 0 {
            let detail = serde_json::json!({ "new_kind": new_kind }).to_string();
            tx.execute(
                "INSERT INTO video_events (video_id, at, event_type, worker_id, detail_json)
                 VALUES (?1, ?2, 'requeued', 'triage', ?3)",
                params![video_id, now, detail],
            )
            .with_context(|| format!("requeue_retryable event for {video_id}"))?;
        }
        tx.commit().context("commit requeue_retryable")?;
        Ok(changed)
    }
}
```

Dead-code note (0002): all three items get callers in Task 10; if the bin build flags them before that, suppress with `// consumed by Epic 3 T10 (triage subcommand)` and lift there.

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test --features test-helpers --test state_triage -- --test-threads=1`, then full suite.
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/state/mod.rs tests/state_triage.rs Cargo.toml
git commit -m "feat(state): triage mutators (triaged_terminal/requeued events, capped requeue with kind write-back) per ADR 0034"
```
