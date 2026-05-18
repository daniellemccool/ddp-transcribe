# Task 7 — `Store::mark_terminal_failure` (SURFACE ONLY — no Epic 2 caller)

**Goal:** Add a `mark_terminal_failure(video_id, worker_id, reason, message) -> Result<usize>` mutator that flips a row from `in_progress` to `failed_terminal` and records reason + message in the terminal columns. **No Epic 2 caller wires this** — Epic 3's classifier dispatcher is the first caller. Surface lands in Epic 2 so Epic 3 is a classifier-add task, not a mutator-add task. Test the mutator anyway.

**ADRs touched:** 0006 (return shape), 0023 (predicate + signature).

**Files:**
- Modify: `src/state/mod.rs` (`mark_terminal_failure` mutator + `#[allow(dead_code)]` per 0002)
- Modify: `tests/state_claims.rs` (mutator round-trip test; gated on `--features test-helpers` so the dead-code suppression doesn't fire in the bin compile)

**Pre-reqs:** T4 (terminal_reason/terminal_message columns exist), T6 (mark_retryable_failure pattern established).

---

- [ ] **Step 1: Write the failing tests**

Append to `tests/state_claims.rs`:

```rust
#[test]
fn mark_terminal_failure_flips_status_and_records_columns() -> anyhow::Result<()> {
    use uu_tiktok::state::Store;
    use rusqlite::Connection;

    let tmp = tempfile::TempDir::new()?;
    let path = tmp.path().join("state.sqlite");
    let mut store = Store::open(&path)?;
    store.upsert_video("vid_a", "https://example/a", false)?;
    let _claim = store.claim_next("worker-1")?.expect("claim");

    let changed = store.mark_terminal_failure(
        "vid_a",
        "worker-1",
        "VideoUnavailable",
        "yt-dlp returned 410 Gone",
    )?;
    assert_eq!(changed, 1);

    let raw = Connection::open(&path)?;
    let (status, tr, tm): (String, Option<String>, Option<String>) = raw.query_row(
        "SELECT status, terminal_reason, terminal_message
         FROM videos WHERE video_id = ?1",
        ["vid_a"],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    assert_eq!(status, "failed_terminal");
    assert_eq!(tr.as_deref(), Some("VideoUnavailable"));
    assert_eq!(tm.as_deref(), Some("yt-dlp returned 410 Gone"));
    Ok(())
}

#[test]
fn mark_terminal_failure_with_stale_claim_returns_zero() -> anyhow::Result<()> {
    use uu_tiktok::state::Store;

    let tmp = tempfile::TempDir::new()?;
    let path = tmp.path().join("state.sqlite");
    let mut store = Store::open(&path)?;
    store.upsert_video("vid_a", "https://example/a", false)?;
    store.claim_next("worker-1")?.expect("claim");

    let changed = store.mark_terminal_failure(
        "vid_a",
        "worker-OTHER",
        "VideoUnavailable",
        "spurious",
    )?;
    assert_eq!(changed, 0);
    Ok(())
}
```

Run:

```bash
cargo test --features test-helpers --test state_claims
```

Expected: FAIL (compile error — `mark_terminal_failure` doesn't exist).

- [ ] **Step 2: Implement the mutator**

In `src/state/mod.rs`, alongside `mark_retryable_failure`:

```rust
/// Flip a video row from `in_progress` to `failed_terminal`, recording
/// the terminal reason + message in the terminal_reason/terminal_message
/// columns. Same stale-claim predicate as the rest of the family
/// (0023). Returns the row-change count per 0006.
///
/// **SURFACE ONLY in Epic 2 — no caller wires this.** Epic 3's classifier
/// dispatcher is the first caller (when failure classification distinguishes
/// VideoUnavailable / VideoNonExistent / similar terminal kinds from
/// transient classes that go through mark_retryable_failure). Landing the
/// surface in Epic 2 means Epic 3 is a classifier-add task, not a
/// mutator-add task — keeps Epic 3's diff focused on the new logic.
///
/// 0002 cleanup discipline: `#[allow(dead_code)]` lives on this method
/// until Epic 3's first caller wires it. The closing task of Epic 3's
/// classifier work removes the attribute.
#[allow(dead_code)]
pub fn mark_terminal_failure(
    &mut self,
    video_id: &str,
    worker_id: &str,
    reason: &str,
    message: &str,
) -> Result<usize> {
    let now = unix_now();
    let tx = self
        .conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .context("begin immediate for mark_terminal_failure")?;

    let changed = tx
        .execute(
            "UPDATE videos
             SET status = 'failed_terminal',
                 terminal_reason = ?2,
                 terminal_message = ?3,
                 claimed_by = NULL,
                 claimed_at = NULL,
                 updated_at = ?4
             WHERE video_id = ?1
               AND status = 'in_progress'
               AND claimed_by = ?5",
            params![video_id, reason, message, now, worker_id],
        )
        .with_context(|| format!("update videos for failed_terminal {}", video_id))?;

    if changed > 0 {
        let detail = serde_json::json!({ "reason": reason, "message": message }).to_string();
        tx.execute(
            "INSERT INTO video_events (video_id, at, event_type, worker_id, detail_json)
             VALUES (?1, ?2, 'failed_terminal', ?3, ?4)",
            params![video_id, now, worker_id, detail],
        )?;
    }

    tx.commit().context("commit mark_terminal_failure")?;
    Ok(changed)
}
```

The `#[allow(dead_code)]` is intentional and visible: per 0002, the bin compile has no caller, so without the attribute clippy `-D warnings` fails. The attribute documents that Epic 3 is the first caller (matching the doc comment).

- [ ] **Step 3: Run the tests**

```bash
cargo test --features test-helpers --test state_claims
```

Expected: PASS. With `--features test-helpers`, the test compile DOES see a caller (the test itself), so the dead-code lint doesn't fire on test compiles. The attribute only matters for bin compiles, which have no caller.

- [ ] **Step 4: Run the full suite**

```bash
cargo test --features test-helpers
```

Expected: all green.

- [ ] **Step 5: cargo fmt + clippy**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
```

Expected: clean (with the `#[allow(dead_code)]`).

- [ ] **Step 6: Commit**

```bash
git add src/state/mod.rs tests/state_claims.rs
git commit -m "$(cat <<'EOF'
feat(state): mark_terminal_failure mutator — surface only, no Epic 2 caller (0023)

Companion to mark_retryable_failure (T6). Flips status='in_progress' →
'failed_terminal' with the terminal_reason + terminal_message columns.
Same stale-claim predicate; same Result<usize> return per 0006.

**No Epic 2 caller wires this.** Epic 3's classifier dispatcher is the
first caller, when failure classification distinguishes terminal classes
(VideoUnavailable, VideoNonExistent) from transient classes. Landing the
mutator surface in Epic 2 means Epic 3 is a classifier-add task, not a
mutator-add task.

#[allow(dead_code)] lives on the method until Epic 3's classifier wires
it — 0002 cleanup discipline (the attribute is removed in the same
commit that lands the first caller).

Tests gated on --features test-helpers so the lint doesn't fire on bin
compiles. Round-trip + stale-claim cases covered.

Refs: 0002, 0006, 0023

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-check

- [ ] `cargo test --features test-helpers --test state_claims` passes
- [ ] `cargo clippy --all-targets -- -D warnings` clean (the `#[allow(dead_code)]` is necessary)
- [ ] The doc comment explicitly names Epic 3 as the first caller (future-reader signal)
- [ ] Same predicate + same return shape as mark_retryable_failure (0023 symmetric)
