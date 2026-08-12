# Task 02 — Schema v7: recency claim index + 19-digit assertion

**Files:**
- Modify: `src/state/schema.rs:1` (SCHEMA_VERSION), `:44-46` (index)
- Modify: `src/state/migrate.rs` (new ladder step after the v5→v6 block at 127-142)
- Test: `tests/state_migrate.rs`
- Check: any test asserting the literal version `"6"` (grep `\"6\"` in `tests/state_schema_version.rs`, `tests/state_migrate.rs`)

**Interfaces:**
- Consumes: the claim-order ADR (Task 01).
- Produces: schema v7 with index `idx_videos_pending_v4` on
  `(status, attempt_count, video_id DESC) WHERE status = 'pending'`.
  Task 03's ORDER BY relies on exactly this index.

**Report (ADR-0019):** ≤250 words, STATUS / SUMMARY / CHANGED FILES / DEVIATIONS.

Ground truth: `SCHEMA_VERSION = "6"` at `src/state/schema.rs:1`; the ladder
is inline `if version == "N"` blocks in `run_migrate` (`src/state/migrate.rs:17`;
v5→v6 template at 127-142; tail bail at 144-147; `meta.schema_version`
upsert at 149-153). Existing test pattern: `synthesize_v5_db:758`,
`migrate_upgrades_v5_to_v6_idempotently:851`.

- [ ] **Step 1: Write the failing tests**

In `tests/state_migrate.rs`, add a `synthesize_v6_db` helper: clone the
`synthesize_v5_db` body, then apply the v5→v6 delta yourself (create the
`ingested_files` table exactly as `src/state/migrate.rs:129-136` does) and
set `meta.schema_version = '6'`. Keep the OLD index (`idx_videos_pending_v3`)
in the synthesized DB — that's what a real v6 DB has. Then:

```rust
#[test]
fn migrate_upgrades_v6_to_v7_idempotently() -> anyhow::Result<()> {
    let (dir, path) = synthesize_v6_db()?;
    let _ = dir;
    run_migrate(&path)?;
    let conn = rusqlite::Connection::open(&path)?;
    let version: String = conn.query_row(
        "SELECT value FROM meta WHERE key = 'schema_version'", [], |r| r.get(0))?;
    assert_eq!(version, "7");
    let old: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_videos_pending_v3'",
        [], |r| r.get(0))?;
    assert_eq!(old, 0, "v3 index must be dropped");
    let new: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_videos_pending_v4'",
        [], |r| r.get(0))?;
    assert_eq!(new, 1, "v4 recency index must exist");
    drop(conn);
    run_migrate(&path)?; // idempotent second pass
    Ok(())
}

#[test]
fn migrate_v6_to_v7_rejects_non_19_digit_canonical_ids() -> anyhow::Result<()> {
    let (dir, path) = synthesize_v6_db()?;
    let _ = dir;
    let conn = rusqlite::Connection::open(&path)?;
    conn.execute(
        "INSERT INTO videos (video_id, source_url, canonical, status, first_seen_at, updated_at)
         VALUES ('123456789012345678', 'https://example/short', 1, 'pending', 0, 0)",
        [],
    )?;
    drop(conn);
    let err = run_migrate(&path).expect_err("18-digit canonical id must refuse the migration");
    assert!(err.to_string().contains("19"), "error names the width invariant: {err}");
    Ok(())
}
```

- [ ] **Step 2: Run them to verify they fail for the real reason**

Run: `cargo test --features test-helpers --test state_migrate -- --test-threads=1 migrate_upgrades_v6_to_v7`
Expected: FAIL — with `SCHEMA_VERSION` still `"6"`, `run_migrate` treats v6
as current and never writes `"7"` (assert_eq on version fails). Not a
compile error — if it fails to compile, fix the test first.

- [ ] **Step 3: Implement**

`src/state/schema.rs:1`:
```rust
pub const SCHEMA_VERSION: &str = "7";
```

`src/state/schema.rs:44-46` — fresh-create DBs get the new index directly:
```sql
CREATE INDEX IF NOT EXISTS idx_videos_pending_v4
    ON videos (status, attempt_count, video_id DESC)
    WHERE status = 'pending';
```
(Replace the `idx_videos_pending_v3` block; fresh DBs never carry the old name.)

`src/state/migrate.rs`, immediately after the v5→v6 block (after line 142),
following the ladder's exact idiom:
```rust
    if version == "6" {
        // Recency order relies on fixed-width ids: lexicographic DESC on
        // TEXT equals numeric DESC only when every id has the same length.
        // Claim-order ADR: refuse the migration rather than mis-order claims.
        let bad: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM videos WHERE canonical = 1 AND LENGTH(video_id) != 19",
                [],
                |r| r.get(0),
            )
            .context("v6→v7: canonical id-width census")?;
        if bad != 0 {
            bail!(
                "v6→v7: {bad} canonical rows have non-19-digit video_ids; \
                 recency claim order requires fixed-width ids — refusing to migrate"
            );
        }
        tx.execute_batch(
            "DROP INDEX IF EXISTS idx_videos_pending_v3;
             CREATE INDEX IF NOT EXISTS idx_videos_pending_v4
                 ON videos (status, attempt_count, video_id DESC)
                 WHERE status = 'pending';",
        )
        .context("v6→v7: recency claim index")?;
        version = "7".to_string();
    }
```
(`bail!` and `.context` are already imported in this module — check the
v5→v6 block's imports.)

- [ ] **Step 4: Sweep stale `"6"` assertions**

Run: `rg '"6"' tests/ src/state/` — update any test asserting the current
schema version literal (e.g. `tests/state_schema_version.rs`) to `"7"`.
Do NOT touch historical ladder literals inside `migrate.rs` itself.

- [ ] **Step 5: Run the migrate suite**

Run: `cargo test --features test-helpers --test state_migrate --test state_schema_version -- --test-threads=1`
Expected: PASS, including both new tests.

- [ ] **Step 6: Full gate and commit**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Commit: `git commit -am "feat(state): schema v7 — recency claim index with 19-digit width guard"`
