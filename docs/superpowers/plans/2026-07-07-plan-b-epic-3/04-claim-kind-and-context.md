# Task 04: `Claim` carries `last_retryable_kind`; `with_context` hygiene

**Files:**
- Modify: `src/state/mod.rs` (`Claim` struct ~line 268; `claim_next` ~line 288; `mark_succeeded` ~line 348)
- Test: `tests/state_claims.rs` (extend existing suite)

**Interfaces:**
- Consumes: existing `Claim { video_id, source_url, attempt_count }`.
- Produces: `Claim` gains `pub last_retryable_kind: Option<String>` — Task 08's fetch worker reads it to decide cookie routing. All existing `Claim` construction sites (tests, fakes) must add the field.

Resolves FOLLOWUPS T10 (`claim_next`/`mark_succeeded` inner statements bare-`?`).

- [ ] **Step 1: Write the failing test**

In `tests/state_claims.rs` (copy the file's existing fixture-DB setup helper — it creates a per-test temp `Store`):

```rust
#[test]
fn claim_next_carries_last_retryable_kind() {
    let (mut store, _tmp) = fresh_store(); // reuse the file's existing helper name
    store
        .upsert_video("7000000000000000001", "https://example.com/v", true)
        .unwrap();

    // First claim: kind is None (never failed).
    let claim = store.claim_next("w1").unwrap().unwrap();
    assert_eq!(claim.last_retryable_kind, None);

    // Fail it with a kind, requeue it manually to pending, re-claim: the
    // kind must ride along (cookie routing in the pipeline depends on it).
    store
        .mark_retryable_failure(&claim.video_id, "w1", "SensitiveLoginGated", "login gated")
        .unwrap();
    store
        .conn_for_tests() // if no such accessor exists, use a raw rusqlite::Connection on the same path
        .execute("UPDATE videos SET status='pending' WHERE video_id=?1", ["7000000000000000001"])
        .unwrap();
    let claim2 = store.claim_next("w1").unwrap().unwrap();
    assert_eq!(claim2.last_retryable_kind.as_deref(), Some("SensitiveLoginGated"));
}
```

(Adapt the helper names to what `tests/state_claims.rs` actually uses — read the file first; it already has per-test fixture DBs and raw-connection patterns from the Epic 2 concurrent-claim tests. Do not invent new scaffolding.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --features test-helpers --test state_claims claim_next_carries -- --test-threads=1`
Expected: compile error — field doesn't exist.

- [ ] **Step 3: Implement**

`Claim`:

```rust
#[derive(Debug, Clone)]
pub struct Claim {
    pub video_id: String,
    pub source_url: String,
    pub attempt_count: i64,
    /// Kind tag recorded by the most recent retryable failure, if any.
    /// None on first attempt. Epic 3 cookie routing keys on this being
    /// "SensitiveLoginGated" (ADR 0035); triage's requeue normalizes
    /// historical placeholder kinds before the row becomes claimable again.
    pub last_retryable_kind: Option<String>,
}
```

`claim_next` SELECT + row mapping:

```rust
let candidate: Option<(String, String, i64, Option<String>)> = tx
    .query_row(
        "SELECT video_id, source_url, attempt_count, last_retryable_kind
         FROM videos
         WHERE status = 'pending'
         ORDER BY first_seen_at ASC, video_id ASC
         LIMIT 1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )
    .optional()
    .context("claim_next: select oldest pending row")?;
```

Thread the fourth tuple element through to the returned `Claim`. While in the function, add the T10 `with_context` hygiene — the two bare-`?` `tx.execute` calls:

```rust
tx.execute(/* UPDATE videos … */)
    .with_context(|| format!("claim_next: flip {video_id} to in_progress for {worker_id}"))?;
tx.execute(/* INSERT video_events … */)
    .with_context(|| format!("claim_next: insert claimed event for {video_id}"))?;
```

Same in `mark_succeeded` for its event INSERT:

```rust
.with_context(|| format!("mark_succeeded: insert succeeded event for {video_id}"))?;
```

Fix every `Claim { … }` construction site the compiler flags (fakes in `tests/pipeline_fakes.rs`, any test constructors): add `last_retryable_kind: None` unless the test is specifically about the field.

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test --features test-helpers -- --test-threads=1`
Expected: PASS across all suites (the compiler-driven construction-site sweep is complete when the whole workspace builds).

- [ ] **Step 5: Commit**

```bash
git add src/state/mod.rs tests/
git commit -m "feat(state): Claim carries last_retryable_kind; with_context on claim/succeed inner statements

Kind snapshot at claim time is the cookie-routing input (ADR 0035).
Resolves FOLLOWUPS T10 context hygiene."
```
