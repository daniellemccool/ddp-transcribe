# Task 07: Start-of-batch sweep + `batch_runs` lifecycle + durable census

**Files:**
- Create: `src/batch.rs` (sweep + census types; single responsibility: batch bookkeeping policy)
- Modify: `src/lib.rs` + `src/main.rs` (register `batch` module, alphabetical: between `audio` and `canonical`)
- Modify: `src/state/mod.rs` (rename triage mutators → sweep vocabulary; event kinds)
- Modify: `src/main.rs` (Process arm: open_batch_run → sweep → run → close_batch_run → print census)
- Rename: `tests/state_triage.rs` → `tests/state_sweep.rs` (update `Cargo.toml` `[[test]]` name `state_triage` → `state_sweep`)
- Create: `tests/batch_census.rs` (+ `[[test]]` block, `required-features = ["test-helpers"]`)

**Interfaces:**
- Consumes: T01 `ClassificationTable` (+ `Disposition`); T02 `open_batch_run`/`close_batch_run`; the renamed mutators below; T06's `ProcessStats` counters + `retries`.
- Produces:
  - `state`: `sweep_mark_terminal(video_id, label, message) -> Result<usize>` (was `triage_mark_terminal`; event `swept_terminal`, worker `'sweep'`) and `sweep_requeue(video_id, new_kind, max_attempts) -> Result<usize>` (was `requeue_retryable`; event `requeued`, worker `'sweep'`). `list_failed_retryable` + `TriageRow` stay (rename the struct to `ParkedRow` and update its doc: "rows awaiting sweep adjudication").
  - `batch::SweepStats { pub examined: usize, pub swept_terminal: usize, pub swept_terminal_by_label: BTreeMap<String, usize>, pub requeued_for_retry: usize, pub parked_for_cookies: usize, pub kept_capped: usize }` (0007: input-side, verb-named; the by-label map is the spec's attrition breakdown)
  - `batch::run_sweep(store: &mut Store, table: &ClassificationTable, retries: i64, cookies_configured: bool) -> anyhow::Result<SweepStats>`
  - `batch::BatchCensus { pub sweep: SweepStats, pub run: RunCensus }` with `RunCensus { pub claimed: usize, pub succeeded: usize, pub failed: usize, pub requeued_for_retry: usize, pub exhausted_retries: usize, pub parked_for_cookies: usize, pub terminal_by_label: BTreeMap<String, usize>, pub stale_after_success: usize, pub stale_after_failure: usize }`, `impl From<&ProcessStats> for RunCensus` (clones the map), `serde::Serialize` on all three, and `impl std::fmt::Display for BatchCensus` (the operator table; after the aggregate lines, print one indented `    <label> <count>` line per by-label entry in each section).
  - Spec note (disclose per 0003 in the commit): the spec's run-section "bug-escalations" count is deliberately omitted — a Bug aborts the run before `close_batch_run`, so the census never records one; the honest bug record is the `batch_runs` row left with `finished_at IS NULL`. Document this in `BatchCensus`'s doc comment.

**Sweep semantics (binding, spec §3):** for each `list_failed_retryable` row, classify the STORED message (`last_retryable_message`, `""` when NULL — default-cautious fallback applies) through the table:
- `Terminal` → `sweep_mark_terminal(id, label, &format!("[sweep] {}", truncated_message))` — count `swept_terminal`. (Truncate the stored message to 500 chars for the terminal_message; the full text remains in `last_retryable_message`, which the mutator preserves.)
- `Retryable` → `sweep_requeue(id, label, retries + 1)`; `Ok(1)` → count `requeued_for_retry`; `Ok(0)` → count `kept_capped` (the predicate's `attempt_count < cap` missed).
- `RequiresCookie` → if `cookies_configured`, same as Retryable; else count `parked_for_cookies` and DO NOTHING (no attempt bump — sweeping isn't fetching; the row simply stays parked).
- Historical tolerance: rows with placeholder kind `"Fetch"` classify from message alone (the stored kind is never consulted); `sweep_requeue`'s kind write-back normalizes them before they're claimable (cookie gate reads the kind at claim time).
- Idempotence: predicates make a concurrent second sweep a no-op (`Ok(0)` everywhere → it just counts `kept_capped`; acceptable noise, note it in the doc comment).

- [ ] **Step 1: Rename the mutators + event vocabulary**

In `src/state/mod.rs`: rename `triage_mark_terminal` → `sweep_mark_terminal` and `requeue_retryable` → `sweep_requeue`; in both, change the event `worker_id` literal `'triage'` → `'sweep'` and the terminal event type `'triaged_terminal'` → `'swept_terminal'` (the requeue event type stays `'requeued'`). Rename `TriageRow` → `ParkedRow` (fields unchanged) and update `list_failed_retryable`'s return type + doc ("snapshot of rows awaiting sweep adjudication"). Update doc comments to describe the sweep (delete the triage-subcommand references). Compiler-driven sweep of uses: `src/triage.rs` (shim from T03 — mechanical rename here; it dies in T08) and `tests/state_triage.rs`.

- [ ] **Step 2: Rename + update the state test file**

`git mv tests/state_triage.rs tests/state_sweep.rs`; in `Cargo.toml` change the `[[test]] name = "state_triage"` block to `name = "state_sweep"`. Inside the file: update call sites for the renamed mutators, and update the event-kind assertions (`triaged_terminal` → `swept_terminal`, worker `'triage'` → `'sweep'`). The three behavioral tests keep their semantics verbatim (terminal flip, capped requeue, list shape).

Run: `cargo test --features test-helpers --test state_sweep -- --test-threads=1` — expected green.

- [ ] **Step 3: Write `src/batch.rs` tests-first**

Create `src/batch.rs` with this test module (plus the `use` skeleton so it compiles RED on missing items):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::classification::ClassificationTable;
    use crate::state::Store;
    use tempfile::TempDir;

    fn seed_parked(store: &mut Store, tmp: &TempDir, id: &str, kind: &str, msg: &str, attempts: i64) {
        store.upsert_video(id, &format!("https://example/{id}"), false).unwrap();
        let conn = rusqlite::Connection::open(tmp.path().join("state.sqlite")).unwrap();
        conn.execute(
            "UPDATE videos SET status='failed_retryable', last_retryable_kind=?2,
             last_retryable_message=?3, attempt_count=?4 WHERE video_id=?1",
            rusqlite::params![id, kind, msg, attempts],
        )
        .unwrap();
    }

    #[test]
    fn sweep_splits_by_disposition_and_cap() {
        let tmp = TempDir::new().unwrap();
        let mut store = Store::open(&tmp.path().join("state.sqlite")).unwrap();
        let table = ClassificationTable::compiled_default().unwrap();

        // Historical placeholder kind "Fetch" + write-off message → terminal.
        seed_parked(&mut store, &tmp, "v_dead", "Fetch",
            "ERROR: Your IP address is blocked, sad", 1);
        // 10240 (former YtDlpOther) → terminal.
        seed_parked(&mut store, &tmp, "v_10240", "Fetch",
            "ERROR: Video not available, status code 10240; report", 1);
        // Retryable under cap → pending, kind normalized.
        seed_parked(&mut store, &tmp, "v_alive", "Fetch",
            "ERROR: Did not get any data blocks", 1);
        // Retryable at cap → kept.
        seed_parked(&mut store, &tmp, "v_capped", "NoDataBlocks",
            "ERROR: Did not get any data blocks", 2);
        // requires-cookie, no cookies → parked untouched.
        seed_parked(&mut store, &tmp, "v_cookie", "Fetch",
            "This post may not be comfortable for some audiences", 1);

        let stats = run_sweep(&mut store, &table, 1, false).unwrap();
        assert_eq!(stats.examined, 5);
        assert_eq!(stats.swept_terminal, 2);
        assert_eq!(stats.swept_terminal_by_label.get("IpBlockedMessage"), Some(&1));
        assert_eq!(
            stats.swept_terminal_by_label.get("VideoNotAvailable10240"),
            Some(&1)
        );
        assert_eq!(stats.requeued_for_retry, 1);
        assert_eq!(stats.kept_capped, 1);
        assert_eq!(stats.parked_for_cookies, 1);

        let dead = store.get_video_for_test("v_dead").unwrap().unwrap();
        assert_eq!(dead.status, "failed_terminal");
        assert_eq!(dead.terminal_reason.as_deref(), Some("IpBlockedMessage"));
        let alive = store.get_video_for_test("v_alive").unwrap().unwrap();
        assert_eq!(alive.status, "pending");
        assert_eq!(alive.last_retryable_kind.as_deref(), Some("NoDataBlocks"));
        let cookie = store.get_video_for_test("v_cookie").unwrap().unwrap();
        assert_eq!(cookie.status, "failed_retryable", "no attempt bump, no move");
    }

    #[test]
    fn sweep_with_cookies_requeues_the_cookie_pool() {
        let tmp = TempDir::new().unwrap();
        let mut store = Store::open(&tmp.path().join("state.sqlite")).unwrap();
        let table = ClassificationTable::compiled_default().unwrap();
        seed_parked(&mut store, &tmp, "v_cookie", "SensitiveLoginGated",
            "This post may not be comfortable for some audiences", 1);
        let stats = run_sweep(&mut store, &table, 1, true).unwrap();
        assert_eq!(stats.requeued_for_retry, 1);
        assert_eq!(stats.parked_for_cookies, 0);
        assert_eq!(
            store.get_video_for_test("v_cookie").unwrap().unwrap().status,
            "pending"
        );
    }

    #[test]
    fn second_sweep_is_a_noop() {
        let tmp = TempDir::new().unwrap();
        let mut store = Store::open(&tmp.path().join("state.sqlite")).unwrap();
        let table = ClassificationTable::compiled_default().unwrap();
        seed_parked(&mut store, &tmp, "v_alive", "Fetch",
            "ERROR: Did not get any data blocks", 1);
        run_sweep(&mut store, &table, 1, false).unwrap();
        let second = run_sweep(&mut store, &table, 1, false).unwrap();
        assert_eq!(second.examined, 0, "requeued rows left failed_retryable — nothing to sweep");
    }

    #[test]
    fn census_serializes_and_displays() {
        let by_label =
            std::collections::BTreeMap::from([("IpBlockedMessage".to_string(), 2usize)]);
        let census = BatchCensus {
            sweep: SweepStats { examined: 5, swept_terminal: 2,
                swept_terminal_by_label: by_label.clone(), requeued_for_retry: 1,
                parked_for_cookies: 1, kept_capped: 1 },
            run: RunCensus { claimed: 3, succeeded: 2, failed: 1, requeued_for_retry: 1,
                exhausted_retries: 0, parked_for_cookies: 0,
                terminal_by_label: std::collections::BTreeMap::new(),
                stale_after_success: 0, stale_after_failure: 0 },
        };
        let json = serde_json::to_string(&census).unwrap();
        assert!(json.contains("\"swept_terminal\":2"));
        assert!(json.contains("\"IpBlockedMessage\":2"));
        let text = census.to_string();
        assert!(text.contains("swept_terminal"));
        assert!(text.contains("IpBlockedMessage"));
        assert!(text.contains("examined"));
    }
}
```

Register the module (`pub mod batch;` in lib.rs between `audio` and `canonical`; `mod batch;` in main.rs same position). Run `cargo test --lib batch -- --test-threads=1` — expected COMPILE FAILURE (RED).

- [ ] **Step 4: Implement `src/batch.rs`**

```rust
//! Batch lifecycle bookkeeping (Epic 4a): the start-of-batch sweep of
//! parked failures and the durable census. Policy layer — reads the
//! classification table, drives Store mutators, never touches the network
//! or the fetch path.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::classification::{ClassificationTable, Disposition};
use crate::pipeline::ProcessStats;
use crate::state::Store;

/// Input-side sweep counters (0007). Every examined row lands in exactly
/// one action bucket by construction of the match below, so
/// `examined == swept_terminal + requeued_for_retry + parked_for_cookies
/// + kept_capped` holds for the sweep (unlike the run section, where
/// stale-claim races open a gap).
#[derive(Debug, Default, Serialize)]
pub struct SweepStats {
    pub examined: usize,
    pub swept_terminal: usize,
    /// Attrition breakdown: which write-off classes died in the sweep.
    pub swept_terminal_by_label: BTreeMap<String, usize>,
    pub requeued_for_retry: usize,
    pub parked_for_cookies: usize,
    pub kept_capped: usize,
}

#[derive(Debug, Serialize)]
pub struct RunCensus {
    pub claimed: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub requeued_for_retry: usize,
    pub exhausted_retries: usize,
    pub parked_for_cookies: usize,
    /// Attrition breakdown: inline write-offs during the drain, by label.
    pub terminal_by_label: BTreeMap<String, usize>,
    pub stale_after_success: usize,
    pub stale_after_failure: usize,
}

impl From<&ProcessStats> for RunCensus {
    fn from(s: &ProcessStats) -> Self {
        RunCensus {
            claimed: s.claimed,
            succeeded: s.succeeded,
            failed: s.failed,
            requeued_for_retry: s.requeued_for_retry,
            exhausted_retries: s.exhausted_retries,
            parked_for_cookies: s.parked_for_cookies,
            terminal_by_label: s.terminal_by_label.clone(),
            stale_after_success: s.stale_after_success,
            stale_after_failure: s.stale_after_failure,
        }
    }
}

/// The batch's attrition record: persisted to batch_runs.census_json AND
/// printed for the operator. The generating policy rides alongside in
/// batch_runs.policy_toml — a census without its policy is not
/// reproducible attrition documentation.
///
/// Deliberately no bug-escalation counter (spec §4 deviation, disclosed):
/// a Bug aborts the run before close_batch_run, so a census recording one
/// can never be written — the honest bug record is the batch_runs row
/// left with finished_at IS NULL.
#[derive(Debug, Serialize)]
pub struct BatchCensus {
    pub sweep: SweepStats,
    pub run: RunCensus,
}

impl std::fmt::Display for BatchCensus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "batch census")?;
        writeln!(f, "  sweep: examined {:>6}", self.sweep.examined)?;
        writeln!(f, "    swept_terminal     {:>6}", self.sweep.swept_terminal)?;
        for (label, n) in &self.sweep.swept_terminal_by_label {
            writeln!(f, "      {label:<24} {n:>6}")?;
        }
        writeln!(f, "    requeued_for_retry {:>6}", self.sweep.requeued_for_retry)?;
        writeln!(f, "    parked_for_cookies {:>6}", self.sweep.parked_for_cookies)?;
        writeln!(f, "    kept_capped        {:>6}", self.sweep.kept_capped)?;
        writeln!(f, "  run:   claimed  {:>6}", self.run.claimed)?;
        writeln!(f, "    succeeded          {:>6}", self.run.succeeded)?;
        writeln!(f, "    failed             {:>6}", self.run.failed)?;
        writeln!(f, "    requeued_for_retry {:>6}", self.run.requeued_for_retry)?;
        writeln!(f, "    exhausted_retries  {:>6}", self.run.exhausted_retries)?;
        writeln!(f, "    parked_for_cookies {:>6}", self.run.parked_for_cookies)?;
        writeln!(f, "    terminal (inline)  {:>6}", self.run.terminal_by_label.values().sum::<usize>())?;
        for (label, n) in &self.run.terminal_by_label {
            writeln!(f, "      {label:<24} {n:>6}")?;
        }
        writeln!(f, "    stale_after_success {:>5}", self.run.stale_after_success)?;
        writeln!(f, "    stale_after_failure {:>5}", self.run.stale_after_failure)
    }
}

/// Start-of-batch sweep (Epic 4a, spec §3): adjudicate every parked
/// failed_retryable row through the active table. Terminal dispositions
/// terminalize (this is where historical write-off classes die on the
/// first post-upgrade run); retryables requeue under the cap; the cookie
/// pool moves only when cookies are configured — no attempt bump either
/// way (sweeping isn't fetching). Idempotent: a concurrent second sweep's
/// predicates all miss.
pub fn run_sweep(
    store: &mut Store,
    table: &ClassificationTable,
    retries: i64,
    cookies_configured: bool,
) -> Result<SweepStats> {
    let mut stats = SweepStats::default();
    let rows = store.list_failed_retryable().context("sweep: list parked rows")?;
    for row in rows {
        stats.examined += 1;
        let message = row.last_retryable_message.as_deref().unwrap_or("");
        let m = table.classify(message);
        match m.disposition {
            Disposition::Terminal => {
                let mut msg = format!("[sweep] {message}");
                msg.truncate(500);
                let changed = store
                    .sweep_mark_terminal(&row.video_id, m.label, &msg)
                    .with_context(|| format!("sweep terminal for {}", row.video_id))?;
                if changed > 0 {
                    stats.swept_terminal += 1;
                    *stats
                        .swept_terminal_by_label
                        .entry(m.label.to_string())
                        .or_insert(0) += 1;
                }
            }
            Disposition::RequiresCookie if !cookies_configured => {
                stats.parked_for_cookies += 1;
            }
            Disposition::Retryable | Disposition::RequiresCookie => {
                let changed = store
                    .sweep_requeue(&row.video_id, m.label, retries + 1)
                    .with_context(|| format!("sweep requeue for {}", row.video_id))?;
                if changed > 0 {
                    stats.requeued_for_retry += 1;
                } else {
                    stats.kept_capped += 1;
                }
            }
        }
    }
    tracing::info!(
        examined = stats.examined,
        swept_terminal = stats.swept_terminal,
        requeued_for_retry = stats.requeued_for_retry,
        parked_for_cookies = stats.parked_for_cookies,
        kept_capped = stats.kept_capped,
        "start-of-batch sweep complete"
    );
    Ok(stats)
}
```

Run: `cargo test --lib batch -- --test-threads=1` — expected 4/4 PASS. (`SweepStats` doc comment: fix the wording to match reality — every examined row lands in exactly one bucket, so `examined == sum` here; the spec's examined-vs-actions gap language applies to the RUN census, not the sweep.)

- [ ] **Step 5: Wire the Process arm lifecycle**

In `src/main.rs` Process arm, after the classification table is built (T06) and the Store is opened but BEFORE the engine construction (fail fast on policy before paying model load):

```rust
            let mut store = store; // make mutable for sweep + batch_runs
            let params_json = serde_json::json!({
                "retries": retries,
                "max_videos": max_videos,
                "cookies_present": cookies_file.is_some(),
                "download_workers": cfg.download_workers,
                "worker_host": hostname_or_default(),
            })
            .to_string();
            let run_id = store.open_batch_run(&params_json, classification.source_toml())?;
            let sweep_stats = batch::run_sweep(
                &mut store,
                &classification,
                retries,
                cookies_file.is_some(),
            )?;
```

(The existing `let store = state::Store::open(…)` binding becomes `let mut store = …` directly; adjust.) After the existing `let stats = stats_result?;` and its tracing line, add:

```rust
            let census = batch::BatchCensus {
                sweep: sweep_stats,
                run: batch::RunCensus::from(&stats),
            };
            {
                let mut guard = shared.try_lock().context(
                    "store lock free after run_pipelined resolved — workers have exited",
                )?;
                let closed = guard.close_batch_run(
                    run_id,
                    &serde_json::to_string(&census).context("serializing census")?,
                )?;
                if closed == 0 {
                    tracing::warn!(run_id, "close_batch_run matched no open row");
                }
            }
            print!("{census}");
```

Note the ownership order: `store` moves into `shared: SharedStore` where it does today (AFTER sweep + open_batch_run — move the `let shared = …` line below the sweep block); the census close uses `shared.try_lock()` because `run_pipelined` has resolved and every worker has exited (the 0025 shutdown-ORDER comment block explains why the lock is necessarily free — do not touch that block's steps).

- [ ] **Step 6: End-to-end lifecycle test**

Create `tests/batch_census.rs` (+ `[[test]]` in Cargo.toml, `required-features = ["test-helpers"]`), header `#![allow(clippy::unwrap_used, clippy::expect_used)]`: seed a store with one parked write-off row (`"Your IP address is blocked"` message) + one parked retryable row, call `batch::run_sweep` + `open_batch_run`/`close_batch_run` in the main-arm order with a hand-built `BatchCensus`, then assert the `batch_runs` row: `policy_toml == ClassificationTable::compiled_default().unwrap().source_toml()`, `census_json` parses back via `serde_json::from_str::<serde_json::Value>` and `["sweep"]["swept_terminal"] == 1`, `finished_at` non-NULL. This pins the provenance chain end to end without driving the full binary.

- [ ] **Step 7: Full verification, then commit**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Expected: green (this task lifts T02's allows on `open_batch_run`/`close_batch_run` — remove those attributes + inline comments now that main() reaches them; note in commit).

```bash
git add -A src/ tests/ Cargo.toml
git commit -m "feat(batch): start-of-batch sweep + batch_runs lifecycle + durable census (Epic 4a)

Sweep adjudicates parked rows through the active table (terminal /
requeue-under-cap / cookie-gated); process opens a batch_runs row with the
policy snapshot and closes it with the census; census prints and persists.
Renames triage_mark_terminal/requeue_retryable → sweep_mark_terminal/
sweep_requeue (events swept_terminal/requeued, worker 'sweep').

0002 dead-code note: lifts T02's allows on open_batch_run/close_batch_run."
```
