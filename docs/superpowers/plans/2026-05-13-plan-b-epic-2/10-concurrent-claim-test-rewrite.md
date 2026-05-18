# Task 10 — Rewrite `concurrent_claim_serializes_via_begin_immediate` to actually race

**Goal:** The existing `concurrent_claim_serializes_via_begin_immediate` test (in `tests/state_claims.rs` or equivalent) runs two claim attempts sequentially — it asserts BEGIN-IMMEDIATE serialization but never actually contends. Rewrite using `std::thread::spawn` + `std::sync::Barrier` so both threads enter `claim_next` simultaneously, racing for the same pending row. Confirms exactly one wins and one returns `None`. Resolves FOLLOWUPS T10 (test rewrite).

**ADRs touched:** — (this is a test-discipline cleanup per AD0003; no design ADR).

**Files:**
- Modify: `tests/state_claims.rs` (or wherever the existing test lives)

**Pre-reqs:** none (independent of other Phase 1 tasks).

---

- [ ] **Step 1: Locate and inspect the existing test**

```bash
grep -rn 'concurrent_claim' tests/ src/ 2>&1
```

Read the current implementation. Capture its shape (signature, what it claims to test, what it actually does). The current test almost certainly looks like:

```rust
// BEFORE (sequential — not actually racing)
#[test]
fn concurrent_claim_serializes_via_begin_immediate() -> Result<()> {
    let mut store_a = Store::open(&path)?;
    let mut store_b = Store::open(&path)?;
    store_a.upsert_video("vid_a", "url", false)?;

    let c1 = store_a.claim_next("worker-1")?;
    let c2 = store_b.claim_next("worker-2")?;
    // ... asserts only one is Some
}
```

This passes today because `claim_next` is implicitly serial. The intent was to confirm BEGIN-IMMEDIATE prevents two concurrent transactions from both winning.

- [ ] **Step 2: Write the rewritten test**

Replace the test body with a barrier-synchronized race. Two `std::thread::spawn`s, each opening its own `Store`, both arriving at `claim_next` at the same moment via `Barrier::wait`.

```rust
#[test]
fn concurrent_claim_serializes_via_begin_immediate() -> anyhow::Result<()> {
    use std::sync::{Arc, Barrier};
    use std::thread;
    use tempfile::TempDir;
    use uu_tiktok::state::{Claim, Store};

    let tmp = TempDir::new()?;
    let path = tmp.path().join("state.sqlite");

    // Seed the row from a one-shot Store, then drop it so the two
    // racing threads have unambiguous ownership of the file.
    {
        let mut seed = Store::open(&path)?;
        seed.upsert_video("vid_a", "https://example/a", false)?;
    }

    let barrier = Arc::new(Barrier::new(2));
    let path_a = path.clone();
    let path_b = path.clone();
    let barrier_a = Arc::clone(&barrier);
    let barrier_b = Arc::clone(&barrier);

    let handle_a = thread::spawn(move || -> anyhow::Result<Option<Claim>> {
        let mut store = Store::open(&path_a)?;
        barrier_a.wait();
        Ok(store.claim_next("worker-A")?)
    });
    let handle_b = thread::spawn(move || -> anyhow::Result<Option<Claim>> {
        let mut store = Store::open(&path_b)?;
        barrier_b.wait();
        Ok(store.claim_next("worker-B")?)
    });

    let result_a = handle_a.join().expect("thread A joins")?;
    let result_b = handle_b.join().expect("thread B joins")?;

    // Exactly one wins; the other returns None (or could error if the
    // SQLite busy_timeout fires before the winner commits — pragma is
    // 5000ms which is far above any contention window). Default assertion:
    // exactly one Some, one None.
    let some_count = [&result_a, &result_b]
        .iter()
        .filter(|r| r.is_some())
        .count();
    let none_count = [&result_a, &result_b]
        .iter()
        .filter(|r| r.is_none())
        .count();
    assert_eq!(some_count, 1, "exactly one thread should win the claim");
    assert_eq!(none_count, 1, "exactly one thread should see no pending");

    // The winning thread's claim should be for vid_a.
    let winner = result_a.or(result_b).unwrap();
    assert_eq!(winner.video_id, "vid_a");
    Ok(())
}
```

- [ ] **Step 3: Run the rewritten test**

```bash
cargo test --features test-helpers --test state_claims concurrent_claim
```

Expected: PASS. The two threads race for the single pending row; BEGIN-IMMEDIATE ensures one wins atomically.

If the test fails intermittently (`some_count == 2` or `some_count == 0`), the rest of the assertions reveal the failure mode. A `some_count == 2` would mean BEGIN-IMMEDIATE isn't serializing — that's a real bug, not a test flake. A `some_count == 0` would suggest both transactions hit `busy_timeout` and failed — adjust `claim_next` to surface that error if it's swallowing it.

- [ ] **Step 4: Run with --test-threads=1 then default to confirm**

```bash
cargo test --features test-helpers --test state_claims -- --test-threads=1
cargo test --features test-helpers --test state_claims
```

Expected: both pass.

- [ ] **Step 5: cargo fmt + clippy**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
```

Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add tests/state_claims.rs
git commit -m "$(cat <<'EOF'
test(state): rewrite concurrent_claim_serializes_via_begin_immediate to actually race

Plan A's test was named for the race condition it was supposed to
guard but ran two claim_next calls sequentially — it asserted
BEGIN-IMMEDIATE serialization without ever contending. FOLLOWUPS T10
flagged this as a real test gap.

Rewrite uses std::thread::spawn + std::sync::Barrier(2) so both
threads enter claim_next at the same instant after each opens its
own Store handle to the same SQLite file. Confirms exactly one wins
and one returns None — the real BEGIN-IMMEDIATE serialization
invariant.

If BEGIN-IMMEDIATE ever regresses (or a future schema change breaks
it), this test fails with some_count == 2 (both threads won — bug
in the claim path) instead of passing silently.

Refs: AD0003 (test discipline); FOLLOWUPS T10 (test rewrite) resolved.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-check

- [ ] `cargo test --features test-helpers --test state_claims concurrent_claim` passes
- [ ] Test runs 100 iterations clean (loop locally with `for i in {1..100}; do cargo test ... || break; done` to confirm non-flakiness)
- [ ] Clippy/fmt clean
- [ ] No other test was affected
