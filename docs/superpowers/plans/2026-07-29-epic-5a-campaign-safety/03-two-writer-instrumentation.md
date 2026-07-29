# Task 03: two-writer instrumentation — sweep events + real hostname

**Files:**
- Modify: `src/state/mod.rs` (`sweep_stale_claims`: per-row `swept_stale` events)
- Modify: `src/main.rs` (`hostname_or_default` reads the real hostname)
- Modify: `tests/state_sweep.rs` (event assertions)

**Interfaces:**
- Consumes (existing): `sweep_stale_claims(&mut self, threshold: Duration) -> Result<usize>` at `src/state/mod.rs:952-985` — single guarded `UPDATE videos SET status='pending', ... WHERE status='in_progress' AND claimed_at IS NOT NULL AND claimed_at < cutoff`, aggregate `tracing::info!` only, **no event rows**. Event-insert precedent: the inline `INSERT INTO video_events (video_id, at, event_type, worker_id, detail_json)` sites (e.g. `swept_terminal` at :1039, worker_id literal `'sweep'`). `event_type` is open TEXT — no CHECK, **no migration**. `hostname_or_default()` at `src/main.rs:488-490` (`$HOSTNAME` → `"host"`).
- Produces:
  - `sweep_stale_claims` unchanged in signature and behavior, but every recovered row also gets a `video_events` row: `event_type='swept_stale'`, `worker_id='sweep'`, `detail_json` carrying the stale claim's provenance: `{"was_claimed_by":"host-1234","claimed_at":1769..., "threshold_secs":1800}`.
  - `hostname_or_default()` resolution order: `/proc/sys/kernel/hostname` (trimmed, non-empty) → `$HOSTNAME` → `"host"`.

**Semantics (binding):**
- **ADR-0024 is untouched**: the sweep stays blind — no validation, no attempt bump, no behavior change. Events are pure observability. Review rejects any conditional added to the sweep's UPDATE predicate.
- Purpose (record in the doc comment): after this task, every legitimate `in_progress → pending` transition leaves a `swept_stale` event; a pending-count increase WITHOUT matching events is hard evidence for the concurrent-writer-loss hypothesis (FOLLOWUPS, production-run group — this task is that entry's "instrument before fixing" leg).
- Implementation shape: inside the existing transaction, first `SELECT video_id, claimed_by, claimed_at FROM videos WHERE <same predicate>`, then the existing UPDATE, then one event insert per selected row — all in the one tx so the event set exactly matches the recovered set. (Select-then-update inside one IMMEDIATE tx is race-free.) Keep returning the UPDATE's change count per ADR-0006, and `debug_assert_eq!(selected.len(), changed)`.
- Hostname: no new dependencies — read `/proc/sys/kernel/hostname` with `std::fs::read_to_string`, trim; empty/`Err` falls through. This fixes `worker_id` (`{host}-{pid}`) and `batch_runs.params_json.worker_host` attribution for the two-instance A/B (both currently report `"host"`).

- [ ] **Step 1: Write the failing tests**

In `tests/state_sweep.rs` (match its existing fixture style — it already builds claimed rows and drives `sweep_stale_claims`):

```rust
#[test]
fn sweep_writes_one_swept_stale_event_per_recovered_row() {
    // Arrange per the file's existing pattern: two rows claimed by
    // "host-999" with claimed_at far in the past, one freshly claimed row.
    // Act: sweep with a threshold that recovers exactly the two stale rows.
    // Assert:
    //   - sweep returns 2 (unchanged contract);
    //   - exactly 2 video_events rows with event_type='swept_stale',
    //     worker_id='sweep';
    //   - each detail_json contains "was_claimed_by":"host-999" and a
    //     numeric claimed_at (raw rusqlite readback + serde_json::Value);
    //   - the fresh row has NO swept_stale event.
}

#[test]
fn sweep_with_nothing_stale_writes_no_events() {
    // Fresh claim only; sweep returns 0; COUNT(*) of swept_stale events == 0.
}
```

(Write these as real tests against the file's actual helpers — the comments above are the binding assertions, not placeholders to leave in.)

- [ ] **Step 2: Run to confirm failure** — `cargo test --test state_sweep -- --test-threads=1`: new tests fail (no events emitted today).

- [ ] **Step 3: Implement the sweep events**

In `sweep_stale_claims` (keep the existing tx + UPDATE; add around them):

```rust
        let cutoff = ...; // existing
        let stale: Vec<(String, Option<String>, Option<i64>)> = {
            let mut stmt = tx.prepare_cached(
                "SELECT video_id, claimed_by, claimed_at FROM videos
                 WHERE status = 'in_progress' AND claimed_at IS NOT NULL AND claimed_at < ?1",
            )?;
            stmt.query_map(params![cutoff], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        // ... existing UPDATE (unchanged predicate) → `changed` ...
        debug_assert_eq!(stale.len(), changed, "event set must match recovered set");
        {
            let now = unix_now();
            let mut ev = tx.prepare_cached(
                "INSERT INTO video_events (video_id, at, event_type, worker_id, detail_json)
                 VALUES (?1, ?2, 'swept_stale', 'sweep', ?3)",
            )?;
            for (video_id, claimed_by, claimed_at) in &stale {
                let detail = serde_json::json!({
                    "was_claimed_by": claimed_by,
                    "claimed_at": claimed_at,
                    "threshold_secs": threshold.as_secs(),
                })
                .to_string();
                ev.execute(params![video_id, now, detail])?;
            }
        }
```

(Adapt to the function's real local names, tx variable, and the file's inline event-insert style — copy the `swept_terminal` insert at :1039 as the template. Extend the doc comment with the observability purpose sentence from Semantics.)

- [ ] **Step 4: Implement the hostname fix**

`src/main.rs:488-490`:

```rust
/// Real hostname for worker attribution (two-instance deployments):
/// /proc/sys/kernel/hostname → $HOSTNAME → "host". Before this, both SRC
/// instances reported the literal "host" and A/B attribution leaned on
/// pid ranges alone.
fn hostname_or_default() -> String {
    if let Ok(h) = std::fs::read_to_string("/proc/sys/kernel/hostname") {
        let h = h.trim();
        if !h.is_empty() {
            return h.to_string();
        }
    }
    std::env::var("HOSTNAME").unwrap_or_else(|_| "host".to_string())
}
```

- [ ] **Step 5: Run tests to verify they pass** — new sweep tests + full `state_sweep`/`state_claims`/`batch_census` suites (census/status tests may assert on event streams — check for breakage from the new event type; `status` renders raw kinds, no enum to extend, but verify `tests/` greps for exhaustive event-type lists: `rg 'swept_terminal' tests/ src/status.rs`).

- [ ] **Step 6: Full verification**

`cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1` — green.

- [ ] **Step 7: Commit**

```bash
git add src/state/mod.rs src/main.rs tests/state_sweep.rs
git commit -m "feat(state): swept_stale event per sweep-recovered row + real hostname attribution — two-writer anomalies become explainable (0024 semantics untouched)"
```
