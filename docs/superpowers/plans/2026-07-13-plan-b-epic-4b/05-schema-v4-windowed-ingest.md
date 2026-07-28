# Task 05: Schema v4 (`watched_at_raw`) + windowed ingest (`--window-start` / `--window-end`)

**Files:**
- Modify: `src/state/schema.rs` (SCHEMA_VERSION "3"→"4"; `watched_at_raw TEXT` on watch_history)
- Modify: `src/state/migrate.rs` (ladder block v3→v4)
- Modify: `src/state/mod.rs` (upsert SQL consts + tx helpers gain `watched_at_raw`; new backfill helper)
- Modify: `src/ingest.rs` (`WindowBounds`, `ingest` signature, `in_window` computation, raw preservation, new stats counters)
- Modify: `src/cli.rs` (`Ingest` gains `--window-start`/`--window-end`; `parse_window_date`)
- Modify: `src/main.rs` (Ingest arm threads the window; log line gains the new counters)
- Modify: `tests/state_migrate.rs` (v3→v4 coverage), `tests/state_ingest.rs` + any other caller of the changed upsert signatures (compiler-driven)
- Modify: `tests/ingest.rs` (window + backfill tests)

**Interfaces:**
- Consumes: Task 01's verdict (ADR number for doc comments; the window semantics below assume the UTC verdict — if Task 01 landed on local-time, the semantics are identical but the ADR language in Task 08 documents day-granularity absorption of the unknown offset).
- Produces (Task 06 relies on these EXACT items):
  - `ingest::WindowBounds { pub start: Option<i64>, pub end_exclusive: Option<i64> }` with `pub fn from_dates(start: Option<chrono::NaiveDate>, end: Option<chrono::NaiveDate>) -> Self` and `pub fn contains(&self, ts: i64) -> bool`
  - `cli::parse_window_date` (`pub(crate)`)
  - Schema v4: `watch_history.watched_at_raw TEXT` (NULL = pre-v4 row)
- Window semantics (Task 08's ADR records these):
  - `--window-start`/`--window-end` are **inclusive calendar dates (UTC)**: start = that day's 00:00:00Z; end = the FOLLOWING day's 00:00:00Z, exclusive. Both optional; absent side = unbounded; both absent = everything `in_window = 1`.
  - `in_window` is computed at ingest and updated ONLY via `recompute-window` (Task 06). Re-ingest never silently changes an existing row's flag.
  - Re-ingest of an existing PK backfills `watched_at_raw` when (and only when) it is NULL — reinterpretation never requires a schema reset.

- [ ] **Step 1: Write the failing tests**

**`tests/state_migrate.rs`** — add one test following the file's existing hand-built-DB style (open the file, copy the v2-builder's shape): hand-build a **v3** DB (current v3 tables incl. `batch_runs`, `idx_videos_pending_v3`, `watch_history` WITHOUT `watched_at_raw`, `meta.schema_version = '3'`, plus one `watch_history` row), then:

```rust
#[test]
fn migrate_upgrades_v3_to_v4_idempotently() {
    // ... hand-build v3 DB at `db` (existing style) with one watch_history row ...
    ddp_transcribe::state::migrate::run_migrate(&db).expect("v3→v4");
    ddp_transcribe::state::migrate::run_migrate(&db).expect("idempotent second run");

    let conn = rusqlite::Connection::open(&db).unwrap();
    let version: String = conn
        .query_row("SELECT value FROM meta WHERE key='schema_version'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, ddp_transcribe::state::SCHEMA_VERSION);
    // Column exists and pre-v4 rows carry NULL raw.
    let raw: Option<String> = conn
        .query_row("SELECT watched_at_raw FROM watch_history LIMIT 1", [], |r| r.get(0))
        .unwrap();
    assert!(raw.is_none(), "pre-v4 rows must carry NULL watched_at_raw");
}
```

Also update any existing assertion pinning the literal end version — they should already use `SCHEMA_VERSION` (Epic 4a's Step 6 converted them); fix any stragglers the failures name.

**`tests/ingest.rs`** — append (self-contained inbox; no fixture dependency):

```rust
fn write_ddp(inbox: &std::path::Path, filename: &str, entries: &[(&str, &str)]) {
    let rows: Vec<String> = entries
        .iter()
        .map(|(date, link)| format!(r#"{{"Date":"{date}","Link":"{link}"}}"#))
        .collect();
    let body = format!(
        r#"[{{"tiktok_watch_history":[{}],"deleted row count":"0"}}]"#,
        rows.join(",")
    );
    std::fs::create_dir_all(inbox).unwrap();
    std::fs::write(inbox.join(filename), body).unwrap();
}

#[test]
fn ingest_window_flags_and_raw_preservation() {
    let tmp = tempfile::TempDir::new().unwrap();
    let inbox = tmp.path().join("inbox");
    write_ddp(
        &inbox,
        "participant=w1_source=tiktok.json",
        &[
            ("2026-02-10 12:00:00", "https://www.tiktokv.com/share/video/7000000000000000111/"),
            ("2026-03-05 12:00:00", "https://www.tiktokv.com/share/video/7000000000000000222/"),
        ],
    );
    let mut store = ddp_transcribe::state::Store::open(&tmp.path().join("state.sqlite")).unwrap();
    let window = ddp_transcribe::ingest::WindowBounds::from_dates(
        Some(chrono::NaiveDate::from_ymd_opt(2026, 2, 1).unwrap()),
        Some(chrono::NaiveDate::from_ymd_opt(2026, 2, 28).unwrap()),
    );
    let stats = ddp_transcribe::ingest::ingest(&inbox, &mut store, window).unwrap();
    assert_eq!(stats.watch_history_rows_processed, 2);
    assert_eq!(stats.marked_out_of_window, 1);

    let conn = rusqlite::Connection::open(tmp.path().join("state.sqlite")).unwrap();
    let (in_w, raw): (i64, String) = conn
        .query_row(
            "SELECT in_window, watched_at_raw FROM watch_history
             WHERE video_id='7000000000000000111'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(in_w, 1);
    assert_eq!(raw, "2026-02-10 12:00:00", "raw DDP Date string preserved verbatim");
    let in_w2: i64 = conn
        .query_row(
            "SELECT in_window FROM watch_history WHERE video_id='7000000000000000222'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(in_w2, 0, "outside the window");
}

#[test]
fn reingest_backfills_null_raw_without_touching_in_window() {
    let tmp = tempfile::TempDir::new().unwrap();
    let inbox = tmp.path().join("inbox");
    write_ddp(
        &inbox,
        "participant=w1_source=tiktok.json",
        &[("2026-02-10 12:00:00", "https://www.tiktokv.com/share/video/7000000000000000111/")],
    );
    let db = tmp.path().join("state.sqlite");
    let mut store = ddp_transcribe::state::Store::open(&db).unwrap();
    // Simulate a pre-v4 row: same PK ingest would produce, NULL raw, and a
    // deliberately contrarian in_window=0 to prove re-ingest doesn't touch it.
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute(
            "INSERT INTO videos (video_id, source_url, canonical, status, first_seen_at, updated_at)
             VALUES ('7000000000000000111', 'https://www.tiktokv.com/share/video/7000000000000000111/', 1, 'pending', 1, 1)",
            [],
        )
        .unwrap();
        // watched_at for '2026-02-10 12:00:00' parsed as UTC:
        let ts = chrono::NaiveDate::from_ymd_opt(2026, 2, 10)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp();
        conn.execute(
            "INSERT INTO watch_history (respondent_id, video_id, watched_at, in_window)
             VALUES ('w1', '7000000000000000111', ?1, 0)",
            rusqlite::params![ts],
        )
        .unwrap();
    }
    let stats = ddp_transcribe::ingest::ingest(
        &inbox,
        &mut store,
        ddp_transcribe::ingest::WindowBounds::default(),
    )
    .unwrap();
    assert_eq!(stats.watch_history_duplicates, 1);
    assert_eq!(stats.backfilled_raw_dates, 1);

    let conn = rusqlite::Connection::open(&db).unwrap();
    let (in_w, raw): (i64, Option<String>) = conn
        .query_row(
            "SELECT in_window, watched_at_raw FROM watch_history
             WHERE video_id='7000000000000000111'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(in_w, 0, "re-ingest must NOT touch in_window (recompute-window is the explicit path)");
    assert_eq!(raw.as_deref(), Some("2026-02-10 12:00:00"));
}
```

Also add `WindowBounds` unit tests in `src/ingest.rs`'s `mod tests`:

```rust
    #[test]
    fn window_bounds_inclusive_dates() {
        let d = |y, m, dd| chrono::NaiveDate::from_ymd_opt(y, m, dd).unwrap();
        let w = WindowBounds::from_dates(Some(d(2026, 2, 1)), Some(d(2026, 2, 28)));
        let ts = |s: &str| parse_watched_at(s).unwrap();
        assert!(w.contains(ts("2026-02-01 00:00:00")), "start midnight inclusive");
        assert!(w.contains(ts("2026-02-28 23:59:59")), "end date inclusive through its last second");
        assert!(!w.contains(ts("2026-01-31 23:59:59")));
        assert!(!w.contains(ts("2026-03-01 00:00:00")), "day after end excluded");
        assert!(WindowBounds::default().contains(ts("1999-01-01 00:00:00")), "no flags = everything in window");
        let start_only = WindowBounds::from_dates(Some(d(2026, 2, 1)), None);
        assert!(start_only.contains(ts("2030-01-01 00:00:00")));
        assert!(!start_only.contains(ts("2026-01-31 23:59:59")));
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --features test-helpers --test ingest --test state_migrate -- --test-threads=1`
Expected: COMPILE FAILURE — `WindowBounds` not found; `ingest` arity mismatch.

- [ ] **Step 3: Schema + migrate ladder (0022: both halves together)**

`src/state/schema.rs`: line 1 → `pub const SCHEMA_VERSION: &str = "4";` and the watch_history DDL becomes:

```sql
CREATE TABLE IF NOT EXISTS watch_history (
    respondent_id  TEXT NOT NULL,
    video_id       TEXT NOT NULL,
    watched_at     INTEGER NOT NULL,
    in_window      INTEGER NOT NULL,
    -- Plan B Epic 4b (schema v4): the verbatim DDP `Date` string, so a
    -- future timezone reinterpretation never requires re-ingest (see the
    -- Epic 4b timezone ADR). NULL = row ingested pre-v4; re-ingesting the
    -- same DDP file backfills it.
    watched_at_raw TEXT,
    PRIMARY KEY (respondent_id, video_id, watched_at),
    FOREIGN KEY (video_id) REFERENCES videos(video_id)
);
```

`src/state/migrate.rs` — append after the v2→v3 block (module doc comment: extend "two stages" wording accordingly):

```rust
    if version == "3" {
        tx.execute_batch("ALTER TABLE watch_history ADD COLUMN watched_at_raw TEXT;")
            .context("v3→v4: watch_history.watched_at_raw")?;
        version = "4".to_string();
    }
```

- [ ] **Step 4: Store upsert + backfill helpers**

`src/state/mod.rs`:

```rust
const UPSERT_WATCH_HISTORY_SQL: &str = "INSERT OR IGNORE INTO watch_history
                 (respondent_id, video_id, watched_at, in_window, watched_at_raw)
                 VALUES (?1, ?2, ?3, ?4, ?5)";
/// Backfill the raw DDP Date string onto a pre-v4 row. Deliberately does
/// NOT touch in_window: recompute-window is the only path that changes
/// flags after ingest (Epic 4b window ADR).
const BACKFILL_WATCH_RAW_SQL: &str = "UPDATE watch_history
                 SET watched_at_raw = ?4
                 WHERE respondent_id = ?1 AND video_id = ?2 AND watched_at = ?3
                   AND watched_at_raw IS NULL";
```

`upsert_watch_history_tx` gains `watched_at_raw: &str` — full replacement (note the params order: `?4` is `in_window`, `?5` is the raw string, matching the SQL column list):

```rust
/// Transaction-scoped sibling of [`upsert_video_tx`]; shares
/// [`UPSERT_WATCH_HISTORY_SQL`] with [`Store::upsert_watch_history`].
pub(crate) fn upsert_watch_history_tx(
    tx: &rusqlite::Transaction<'_>,
    respondent_id: &str,
    video_id: &str,
    watched_at: i64,
    watched_at_raw: &str,
    in_window: bool,
) -> Result<usize> {
    let changed = tx
        .prepare_cached(UPSERT_WATCH_HISTORY_SQL)
        .context("preparing upsert_watch_history")?
        .execute(params![
            respondent_id,
            video_id,
            watched_at,
            i64::from(in_window),
            watched_at_raw
        ])
        .with_context(|| {
            format!(
                "upserting watch_history (respondent={respondent_id}, video={video_id}, watched_at={watched_at})"
            )
        })?;
    Ok(changed)
}
```

Add the sibling:

```rust
/// Transaction-scoped backfill of watched_at_raw for an existing row
/// (INSERT OR IGNORE hit). Returns the row-change count per 0006:
/// 1 = backfilled a NULL, 0 = row already carried its raw string.
pub(crate) fn backfill_watch_raw_tx(
    tx: &rusqlite::Transaction<'_>,
    respondent_id: &str,
    video_id: &str,
    watched_at: i64,
    watched_at_raw: &str,
) -> Result<usize> {
    let changed = tx
        .prepare_cached(BACKFILL_WATCH_RAW_SQL)
        .context("preparing backfill_watch_raw")?
        .execute(params![respondent_id, video_id, watched_at, watched_at_raw])
        .with_context(|| {
            format!("backfilling watched_at_raw (respondent={respondent_id}, video={video_id})")
        })?;
    Ok(changed)
}
```

The `#[allow(dead_code)]` convenience method `Store::upsert_watch_history` gains the same `watched_at_raw: &str` parameter (keep signatures symmetric with the tx helper); update its callers in `tests/state_ingest.rs` compiler-driven (pass a literal like `"2026-01-01 00:00:00"`).

- [ ] **Step 5: Windowed ingest**

`src/ingest.rs`:

```rust
/// Analysis-window bounds in unix seconds, derived from inclusive UTC
/// calendar dates (Epic 4b window ADR). `Default` = no filter (everything
/// in-window) — matches pre-4b behavior.
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowBounds {
    /// Inclusive: 00:00:00Z of --window-start.
    pub start: Option<i64>,
    /// Exclusive: 00:00:00Z of the day AFTER --window-end (an inclusive
    /// calendar end date covers its whole day).
    pub end_exclusive: Option<i64>,
}

impl WindowBounds {
    pub fn from_dates(
        start: Option<chrono::NaiveDate>,
        end: Option<chrono::NaiveDate>,
    ) -> Self {
        let to_ts = |d: chrono::NaiveDate| {
            Utc.from_utc_datetime(&d.and_time(chrono::NaiveTime::MIN)).timestamp()
        };
        WindowBounds {
            start: start.map(to_ts),
            // succ_opt is None only at NaiveDate::MAX — saturate to "no
            // upper bound reachable" rather than wrap.
            end_exclusive: end.map(|d| d.succ_opt().map_or(i64::MAX, to_ts)),
        }
    }

    pub fn contains(&self, ts: i64) -> bool {
        self.start.map_or(true, |s| ts >= s) && self.end_exclusive.map_or(true, |e| ts < e)
    }
}
```

`IngestStats` gains (0007 input-side, verb-named):

```rust
    /// Rows this pass marked in_window = 0 (outside the supplied window).
    pub marked_out_of_window: usize,
    /// Existing rows whose NULL watched_at_raw this pass backfilled.
    pub backfilled_raw_dates: usize,
```

`ingest` signature: `pub fn ingest(inbox: &Path, store: &mut Store, window: WindowBounds) -> Result<IngestStats>`; thread `window` into `process_watch_entry`. In `process_watch_entry`, replace the upsert block:

```rust
    unique_videos.insert(video_id.clone());
    upsert_video_tx(tx, &video_id, &entry.link, true)?;

    let in_window = window.contains(watched_at);
    if !in_window {
        stats.marked_out_of_window += 1;
    }
    let inserted = upsert_watch_history_tx(
        tx,
        respondent_id,
        &video_id,
        watched_at,
        &entry.date,
        in_window,
    )?;
    stats.watch_history_rows_processed += 1;
    if inserted == 0 {
        stats.watch_history_duplicates += 1;
        // Pre-v4 rows carry NULL watched_at_raw; re-ingest is the designed
        // backfill path. in_window is deliberately untouched here.
        stats.backfilled_raw_dates +=
            backfill_watch_raw_tx(tx, respondent_id, &video_id, watched_at, &entry.date)?;
    }
    Ok(())
```

(`use crate::state::backfill_watch_raw_tx;` joins the imports.)

- [ ] **Step 6: CLI + main**

`src/cli.rs`:

```rust
pub(crate) fn parse_window_date(s: &str) -> Result<chrono::NaiveDate, String> {
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|e| format!("invalid date {s:?} (expected YYYY-MM-DD): {e}"))
}
```

`Ingest` variant becomes:

```rust
    /// Walk --inbox, parse DDP JSONs, upsert into videos and watch_history.
    Ingest {
        #[arg(long)]
        dry_run: bool,
        /// Inclusive analysis-window start (YYYY-MM-DD, UTC). Rows outside
        /// the window ingest with in_window = 0. Absent = unbounded.
        #[arg(long, value_parser = parse_window_date)]
        window_start: Option<chrono::NaiveDate>,
        /// Inclusive analysis-window end (YYYY-MM-DD, UTC; covers that
        /// whole day). Absent = unbounded.
        #[arg(long, value_parser = parse_window_date)]
        window_end: Option<chrono::NaiveDate>,
    },
```

`src/main.rs` Ingest arm: destructure the new fields, build `let window = ingest::WindowBounds::from_dates(window_start, window_end);`, pass to `ingest::ingest(&cfg.inbox, &mut store, window)`, and add `marked_out_of_window = stats.marked_out_of_window, backfilled_raw_dates = stats.backfilled_raw_dates,` to the "ingest complete" log line.

- [ ] **Step 7: Run the tests**

Run: `cargo test --features test-helpers --test ingest --test state_migrate --test state_ingest -- --test-threads=1`
Expected: green, including the two new ingest tests and the v3→v4 migrate test.

- [ ] **Step 8: Both 0022 directions + real-data migration on a snapshot COPY**

```bash
SCRATCH=<session scratchpad>/v4-check && mkdir -p "$SCRATCH"
cp ddp-run-export.sqlite "$SCRATCH/snapshot.sqlite"
# Direction 1: v4 binary refuses the v3 DB with the typed operator message.
cargo run --quiet -- --state-db "$SCRATCH/snapshot.sqlite" status; echo "exit: $?"
```
Expected: failure mentioning `schema version mismatch` and `ddp-transcribe migrate`; exit nonzero. **Never run this against `ddp-run-export.sqlite` itself.**

```bash
# Direction 2: migrate the COPY, then ground truth must reproduce on v4.
cargo run --quiet -- --state-db "$SCRATCH/snapshot.sqlite" migrate
cargo run --quiet -- --state-db "$SCRATCH/snapshot.sqlite" status
sqlite3 "$SCRATCH/snapshot.sqlite" \
  "SELECT COUNT(*) FROM watch_history WHERE watched_at_raw IS NULL;
   SELECT in_window, COUNT(*) FROM watch_history GROUP BY in_window;"
```
Expected: migrate logs v3→v4; `status` reproduces the ground-truth table exactly (51903/3928/789, six-kind breakdown, run 1 INTERRUPTED); 64,956 NULL raws; all rows still `in_window = 1`. Paste the numbers into the task report.

- [ ] **Step 9: Full verification + commit**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Expected: green.

```bash
git add src/state/schema.rs src/state/migrate.rs src/state/mod.rs src/ingest.rs src/cli.rs src/main.rs tests/state_migrate.rs tests/state_ingest.rs tests/ingest.rs
git commit -m "feat(ingest): schema v4 — watched_at_raw preservation + --window-start/--window-end in_window computation

Inclusive UTC calendar-date window; in_window computed at ingest and
changed only via recompute-window (next task); re-ingest backfills NULL
raws without touching flags. Migrated a copy of the pilot snapshot:
ground truth reproduces on v4 (51903/3928/789)."
```
