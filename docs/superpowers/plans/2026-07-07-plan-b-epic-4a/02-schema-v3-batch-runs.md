# Task 02: Schema v3 — `batch_runs` table, attempt-aware pending index, migrate ladder

**Files:**
- Modify: `src/state/schema.rs` (SCHEMA_VERSION "2"→"3"; add `batch_runs` DDL; replace the pending index definition)
- Modify: `src/state/migrate.rs` (extend the ladder: v1→v2→v3)
- Modify: `src/state/mod.rs` (add `open_batch_run` / `close_batch_run` after `requeue_retryable`, before the test-helper `impl` block)
- Create: `tests/state_batch_runs.rs`
- Modify: `Cargo.toml` (add `[[test]] name = "state_batch_runs"` with `required-features = ["test-helpers"]`)
- Modify: `tests/state_migrate.rs` (extend existing migrate coverage to v3)

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces (Task 07 relies on these EXACT signatures):
  - `Store::open_batch_run(&mut self, params_json: &str, policy_toml: &str) -> anyhow::Result<i64>` (returns `run_id`)
  - `Store::close_batch_run(&mut self, run_id: i64, census_json: &str) -> anyhow::Result<usize>` (row-change count per 0006)
  - Table `batch_runs(run_id INTEGER PRIMARY KEY, started_at, finished_at, params_json, policy_toml, census_json)`

Both new mutators get `#[allow(dead_code)]` + `// 0002: consumed by Epic 4a T07 (batch lifecycle); lift when it lands.` and the commit-message note.

**Existing-behavior guardrails:** `Store::open` already hard-fails on version mismatch (0022) — after this task, opening a v2 DB with the new binary must produce that error (pointing at `migrate`), and `migrate` must upgrade v1 and v2 DBs to v3 idempotently.

- [ ] **Step 1: Write the failing tests**

Create `tests/state_batch_runs.rs`:

```rust
#![allow(clippy::unwrap_used, clippy::expect_used)]

use ddp_transcribe::state::Store;
use tempfile::TempDir;

fn fresh_store(tmp: &TempDir) -> Store {
    Store::open(&tmp.path().join("state.sqlite")).expect("open store")
}

#[test]
fn batch_run_opens_and_closes_with_census() {
    let tmp = TempDir::new().unwrap();
    let mut store = fresh_store(&tmp);

    let run_id = store
        .open_batch_run(r#"{"retries":1,"max_videos":null}"#, "schema = 1\n")
        .expect("open_batch_run");
    assert!(run_id >= 1);

    let changed = store
        .close_batch_run(run_id, r#"{"sweep":{"examined":0}}"#)
        .expect("close_batch_run");
    assert_eq!(changed, 1);

    // Raw read-back: row carries everything, finished_at is set.
    let conn = rusqlite::Connection::open(tmp.path().join("state.sqlite")).unwrap();
    let (params, policy, census, finished): (String, String, Option<String>, Option<i64>) = conn
        .query_row(
            "SELECT params_json, policy_toml, census_json, finished_at
             FROM batch_runs WHERE run_id = ?1",
            [run_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert!(params.contains("retries"));
    assert_eq!(policy, "schema = 1\n");
    assert!(census.unwrap().contains("examined"));
    assert!(finished.is_some());
}

#[test]
fn close_of_unknown_run_returns_zero() {
    let tmp = TempDir::new().unwrap();
    let mut store = fresh_store(&tmp);
    let changed = store.close_batch_run(9999, "{}").expect("close");
    assert_eq!(changed, 0, "0006: predicate miss reports 0, not an error");
}

#[test]
fn interrupted_run_leaves_finished_at_null() {
    let tmp = TempDir::new().unwrap();
    let mut store = fresh_store(&tmp);
    let run_id = store.open_batch_run("{}", "schema = 1\n").expect("open");
    // No close — simulates a crash. finished_at must be NULL (honest record).
    let conn = rusqlite::Connection::open(tmp.path().join("state.sqlite")).unwrap();
    let finished: Option<i64> = conn
        .query_row(
            "SELECT finished_at FROM batch_runs WHERE run_id = ?1",
            [run_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(finished.is_none());
}
```

Add to `Cargo.toml` after the `state_triage` `[[test]]` block:

```toml
[[test]]
name = "state_batch_runs"
required-features = ["test-helpers"]
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --features test-helpers --test state_batch_runs -- --test-threads=1`
Expected: COMPILE FAILURE — `open_batch_run` not found.

- [ ] **Step 3: Bump the schema**

In `src/state/schema.rs`:

1. Change line 1 to `pub const SCHEMA_VERSION: &str = "3";`
2. Replace the `idx_videos_pending` definition with the attempt-aware form (retries sort behind fresh work — Task 05 changes `claim_next`'s ORDER BY to match):

```sql
CREATE INDEX IF NOT EXISTS idx_videos_pending_v3
    ON videos (status, attempt_count, first_seen_at, video_id)
    WHERE status = 'pending';
```

(New index NAME — `IF NOT EXISTS` under the old name would silently keep the old column order on migrated DBs.)

3. Append to `SCHEMA_SQL` before the closing `";`:

```sql
CREATE TABLE IF NOT EXISTS batch_runs (
    run_id       INTEGER PRIMARY KEY,
    started_at   INTEGER NOT NULL,
    -- NULL means the run crashed or was interrupted before close — an
    -- honest record the operator can see.
    finished_at  INTEGER,
    params_json  TEXT NOT NULL,
    policy_toml  TEXT NOT NULL,
    census_json  TEXT
);
```

- [ ] **Step 4: Extend the migrate ladder**

In `src/state/migrate.rs`, replace the single `if found == "1" { … } else { bail }` block with a sequential ladder (each stage advances a local `version` string; unknown starts still bail):

```rust
    let tx = conn
        .transaction()
        .context("begin transaction for schema migrate")?;

    let mut version = found.clone();

    if version == "1" {
        tx.execute_batch(
            "ALTER TABLE videos ADD COLUMN last_retryable_kind TEXT;
             ALTER TABLE videos ADD COLUMN last_retryable_message TEXT;
             ALTER TABLE videos ADD COLUMN terminal_reason TEXT;
             ALTER TABLE videos ADD COLUMN terminal_message TEXT;",
        )
        .context("v1→v2: ALTER TABLE videos ADD COLUMN ×4")?;
        version = "2".to_string();
    }

    if version == "2" {
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS batch_runs (
                 run_id       INTEGER PRIMARY KEY,
                 started_at   INTEGER NOT NULL,
                 finished_at  INTEGER,
                 params_json  TEXT NOT NULL,
                 policy_toml  TEXT NOT NULL,
                 census_json  TEXT
             );
             DROP INDEX IF EXISTS idx_videos_pending;
             CREATE INDEX IF NOT EXISTS idx_videos_pending_v3
                 ON videos (status, attempt_count, first_seen_at, video_id)
                 WHERE status = 'pending';",
        )
        .context("v2→v3: batch_runs + attempt-aware pending index")?;
        version = "3".to_string();
    }

    if version != SCHEMA_VERSION {
        anyhow::bail!("migrate: don't know how to upgrade from v{found} to v{SCHEMA_VERSION}");
    }
```

(The `INSERT INTO meta … ON CONFLICT` upsert and commit below the ladder stay as they are.)

- [ ] **Step 5: Add the Store mutators**

In `src/state/mod.rs`, immediately after `requeue_retryable`'s closing brace (still inside the same `impl Store` block):

```rust
    /// Open a batch-run record (Epic 4a): one row per `process` invocation,
    /// carrying the run parameters and the FULL active classification policy
    /// TOML — the census without its generating policy is not reproducible
    /// attrition documentation. Returns the new run_id.
    // 0002: consumed by Epic 4a T07 (batch lifecycle); lift when it lands.
    #[allow(dead_code)]
    pub fn open_batch_run(&mut self, params_json: &str, policy_toml: &str) -> Result<i64> {
        let now = unix_now();
        self.conn
            .execute(
                "INSERT INTO batch_runs (started_at, params_json, policy_toml)
                 VALUES (?1, ?2, ?3)",
                params![now, params_json, policy_toml],
            )
            .context("insert batch_runs row")?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Close a batch-run record with its census. Returns the row-change
    /// count per 0006 (0 = unknown run_id or already closed by predicate
    /// miss — callers log, never panic).
    // 0002: consumed by Epic 4a T07 (batch lifecycle); lift when it lands.
    #[allow(dead_code)]
    pub fn close_batch_run(&mut self, run_id: i64, census_json: &str) -> Result<usize> {
        let now = unix_now();
        self.conn
            .execute(
                "UPDATE batch_runs
                 SET finished_at = ?2, census_json = ?3
                 WHERE run_id = ?1 AND finished_at IS NULL",
                params![run_id, now, census_json],
            )
            .context("close batch_runs row")
    }
```

- [ ] **Step 6: Extend the migrate test**

In `tests/state_migrate.rs`, find the existing test that builds a v1 DB and asserts migration to the current version; follow its construction pattern to add ONE new test: build a **v2-shaped** DB (create the v2 schema by hand exactly as the existing v1 test hand-builds v1 — copy its style, adding the four v2 columns to `videos` and `meta.schema_version = '2'`), run `run_migrate`, then assert: `meta.schema_version == "3"`, `batch_runs` exists (`SELECT count(*) FROM batch_runs` returns 0 without error), the old `idx_videos_pending` index is gone and `idx_videos_pending_v3` exists (`SELECT name FROM sqlite_master WHERE type='index' AND name LIKE 'idx_videos_pending%'`). Also verify the EXISTING v1 test still passes (it now lands on v3 — update its version assertion from "2" to `SCHEMA_VERSION`), and run `run_migrate` twice in the new test to pin idempotence (second call returns Ok, version stays "3").

- [ ] **Step 7: Run tests**

Run: `cargo test --features test-helpers --test state_batch_runs --test state_migrate --test state_schema_version --test state_schema_v2 -- --test-threads=1`
Expected: state_batch_runs 3/3 pass; state_migrate green (including your updated assertions); if `state_schema_version`/`state_schema_v2` assert the literal version "2" anywhere, update those assertions to `SCHEMA_VERSION`/v3 accordingly — read the failures, they name the lines.

- [ ] **Step 8: Full verification, then commit**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Expected: green across the board (the pilot-DB-shaped fixtures used by other suites are per-test fresh DBs, so the version bump is invisible to them).

```bash
git add src/state/schema.rs src/state/migrate.rs src/state/mod.rs tests/state_batch_runs.rs tests/state_migrate.rs Cargo.toml
git commit -m "feat(state): schema v3 — batch_runs table, attempt-aware pending index, v2→v3 migrate ladder

0002 dead-code note: open_batch_run/close_batch_run carry allow(dead_code)
with lift point Epic 4a T07 (batch lifecycle wires the first callers)."
```
