# Task 06: `requeue-failures` subcommand (Phase 2)

**Files:**
- Modify: `src/cli.rs` (new subcommand + arg group), `src/commands.rs` (dispatch arm), `src/state/mod.rs` (`requeue_failures` mutator)
- Test: `tests/requeue_failures.rs` (new; auto-discovered per ADR-0005), `tests/cli.rs` (parse cases)

**Interfaces:**
- Consumes: Task 05's record (title: requeue-failures operator override) — its contract is binding; `Store::transaction_immediate()`; the `sweep_stale_claims` select-then-update-then-events shape (src/state/mod.rs ~:965-1051) as the tx template; `hostname_or_default()` from its post-Task-03 lib location; `CommandExit`.
- Produces: `pub(crate) struct RequeueFilter { error_kinds: Vec<String>, max_attempts: Option<u32>, older_than: Option<Duration>, include_terminal: bool, all: bool, max: Option<u32> }`; `Store::requeue_failures(&mut self, f: &RequeueFilter, actor: &str, dry_run: bool) -> Result<RequeueOutcome>` where `RequeueOutcome { matched: usize, requeued: usize, by_kind: Vec<(String, usize)> }` (`requeued == 0` when `dry_run`; mutator's row-change count inside per ADR-0006).

**Semantics (binding):** the Task-05 record verbatim — grammar, allowlist clock, transition, forensics, arithmetic doc. Selected specifics the tests below pin; anything ambiguous: the record governs, then the spec.

- [ ] **Step 1: Write the failing CLI parse tests** in `tests/cli.rs` (match its existing exit-code-2 parse-case style):
  - bare `requeue-failures` → exit 2 (default-deny)
  - `requeue-failures --max 5` → exit 2 (modifier alone)
  - `requeue-failures --all --older-than 30d` → exit 2 (conflict)
  - `requeue-failures --include-terminal --all` → exit 2
  - `requeue-failures --include-terminal --max 100` → exit 2
  - `requeue-failures --max-attempts 0` → exit 2 (positive range)
  - `requeue-failures --all`, `requeue-failures --error-kind timeout --error-kind geo_block`, `requeue-failures --include-terminal --error-kind unavailable --older-than 7d` → parse (≠2)
- [ ] **Step 2: Write the failing behavior tests** in `tests/requeue_failures.rs` using the state fixture style of `tests/state_sweep.rs` (seed rows via public/test-helpers API, drive `Store::requeue_failures`, raw-rusqlite readback):

```rust
// Binding assertions (write as real tests against the actual helpers):
// 1. eligibility: failed_retryable row with matching --error-kind → pending,
//    claimed_by/claimed_at NULL, attempt_count unchanged, last_retryable_* retained;
//    one video_events row event_type='operator_requeued', worker_id='operator:<host>-<pid>'
//    whose detail_json carries prior_status/prior_kind/attempt_count.
// 2. terminal exclusion: failed_terminal row untouched without --include-terminal,
//    matched (terminal_reason) only with --include-terminal + qualifying selector.
// 3. failure clock: row whose only recent event is 'requeued' (administrative) with an
//    old 'failed_retryable' event → matches --older-than D per the OLD failure event;
//    a fresh 'failed_retryable' event blocks the match; a row with NO allowlist events
//    never matches --older-than. Pin 'swept_terminal' as clock-neutral explicitly.
// 4. --max determinism: 3 eligible rows, --max 2 → the two lowest
//    (attempt_count ASC, video_id ASC) transition; run twice from identical fixtures
//    → identical selection.
// 5. dry-run: matched counts reported, zero DB writes (row states + event count
//    unchanged), RequeueOutcome.requeued == 0.
// 6. zero-match: RequeueOutcome { matched: 0, requeued: 0, .. }, no events.
// 7. filter conjunction: --error-kind X --max-attempts N --older-than D ANDs.
```

- [ ] **Step 3: Run to confirm failure** — `cargo test --test requeue_failures --features test-helpers -- --test-threads=1` (compile fail: mutator absent) and `cargo test --test cli -- --test-threads=1` (parse cases fail).
- [ ] **Step 4: Implement.**
  1. `src/cli.rs`: subcommand with clap `conflicts_with_all`/`requires` encoding the grammar (`value_parser` range 1.. for `--max`/`--max-attempts`; `humantime::parse_duration` for `--older-than` per the existing precedent).
  2. `src/state/mod.rs`: `requeue_failures` — one `transaction_immediate()`; CTE `last_failure AS (SELECT video_id, MAX(at) AS last_failure_at FROM video_events WHERE event_type IN ('failed_retryable','failed_terminal','retry_requeued','cookie_parked') GROUP BY video_id)`; SELECT eligible per filter → UPDATE … SET status='pending', claimed_by=NULL, claimed_at=NULL WHERE video_id IN (…) → one `operator_requeued` INSERT per row (copy the `swept_stale` insert shape); `debug_assert_eq!(selected.len(), changed)`; ORDER BY `attempt_count ASC, video_id ASC` LIMIT `--max`. Dry-run: the SELECT only.
  3. `src/commands.rs` arm: build `RequeueFilter`, call the mutator with `actor = format!("operator:{}-{}", hostname_or_default(), std::process::id())`, print per-kind counts + total (`" (dry-run)"` suffix mirrors ingest), tracing::info! with counts; zero matches prints `requeue-failures: 0 rows matched` and returns `Ok(CommandExit::Success)`.
- [ ] **Step 5: Run tests to verify green** (both new files + full `state_*` suites for regressions).
- [ ] **Step 6: Full gate** (incl. `cargo build --release`).
- [ ] **Step 7: Commit**

```bash
git add src/cli.rs src/commands.rs src/state/mod.rs tests/requeue_failures.rs tests/cli.rs
git commit -m "feat(state): requeue-failures — forensic default-deny eligibility override per ADR-0036 carve-out; operator_requeued events, allowlist failure clock"
```
