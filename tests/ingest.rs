#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use assert_cmd::Command as AssertCommand;
use ddp_transcribe::ingest::{ingest, IngestStats};
use ddp_transcribe::state::Store;
use tempfile::TempDir;

/// Public-facing fixture: news-organisation videos only. Committed to the
/// repo. Used by the always-running integration tests below.
fn news_orgs_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ddp/news_orgs")
}

/// Local-only fixture: real-looking watch-history kept on dev laptops for
/// ad-hoc testing but not committed (see .gitignore). The tests that use it
/// skip with a notice if the fixture is absent (CI, fresh clones, the SRC
/// workspace).
fn local_real_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ddp/20260416_test")
}

#[test]
fn ingest_news_orgs_fixture_writes_videos_and_watch_history() {
    let tmp = TempDir::new().unwrap();
    let mut store = Store::open(&tmp.path().join("state.sqlite")).unwrap();

    let stats: IngestStats = ingest(
        &news_orgs_fixture(),
        &mut store,
        ddp_transcribe::ingest::WindowBounds::default(),
        false,
    )
    .expect("ingest succeeds");

    // Fixture has 20 unique videos and 25 watch_history rows (5 are
    // re-watches at distinct timestamps).
    assert!(
        stats.unique_videos_seen >= 15,
        "expected >=15 unique videos, got {}",
        stats.unique_videos_seen
    );
    assert!(
        stats.watch_history_rows_processed >= 15,
        "expected >=15 watch_history rows, got {}",
        stats.watch_history_rows_processed
    );
    assert_eq!(stats.short_links_skipped, 0, "fixture has no short links");
    assert_eq!(stats.invalid_urls_skipped, 0, "fixture has no invalid URLs");

    // Spot-check: first NOS Stories video.
    let row = store
        .get_video_for_test("7636781376787795232")
        .unwrap()
        .expect("known video present");
    assert_eq!(row.status, "pending");
    assert!(row.canonical);
}

/// Clear the schema-v6 ingest ledger so the next `ingest` call actually
/// re-walks the rows. The row-level INSERT-OR-IGNORE idempotence below is a
/// correctness backstop independent of the file-level fast path, and stays
/// under test; the fast path itself is covered by
/// `unchanged_files_are_skipped_via_the_ledger_on_re_ingest`.
fn forget_ingested_files(db: &std::path::Path) {
    rusqlite::Connection::open(db)
        .unwrap()
        .execute("DELETE FROM ingested_files", [])
        .unwrap();
}

#[test]
fn ingest_news_orgs_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("state.sqlite");
    let mut store = Store::open(&db).unwrap();

    let first = ingest(
        &news_orgs_fixture(),
        &mut store,
        ddp_transcribe::ingest::WindowBounds::default(),
        false,
    )
    .unwrap();
    forget_ingested_files(&db);
    let second = ingest(
        &news_orgs_fixture(),
        &mut store,
        ddp_transcribe::ingest::WindowBounds::default(),
        false,
    )
    .unwrap();

    assert_eq!(first.unique_videos_seen, second.unique_videos_seen);
    assert_eq!(
        first.watch_history_rows_processed,
        second.watch_history_rows_processed
    );
    assert_eq!(
        second.watch_history_duplicates,
        second.watch_history_rows_processed
    );
}

#[test]
fn ingest_local_real_fixture_writes_videos_and_watch_history() {
    let fixture = local_real_fixture();
    if !fixture.exists() {
        eprintln!(
            "skipping ingest_local_real_fixture: {} not present (local-only fixture)",
            fixture.display()
        );
        return;
    }

    let tmp = TempDir::new().unwrap();
    let mut store = Store::open(&tmp.path().join("state.sqlite")).unwrap();

    let stats: IngestStats = ingest(
        &fixture,
        &mut store,
        ddp_transcribe::ingest::WindowBounds::default(),
        false,
    )
    .expect("ingest succeeds");

    // The local fixture has ~200 watch_history rows but many duplicates;
    // expect a smaller number of unique videos plus all watch_history rows.
    assert!(
        stats.unique_videos_seen > 50,
        "expected >50 unique videos, got {}",
        stats.unique_videos_seen
    );
    assert!(
        stats.watch_history_rows_processed > 100,
        "expected >100 watch_history rows, got {}",
        stats.watch_history_rows_processed
    );
    assert_eq!(stats.short_links_skipped, 0, "fixture has no short links");
    assert_eq!(stats.invalid_urls_skipped, 0, "fixture has no invalid URLs");

    // Spot-check: a known video_id from the local fixture file.
    let row = store
        .get_video_for_test("7583050189527682336")
        .unwrap()
        .expect("known video present");
    assert_eq!(row.status, "pending");
    assert!(row.canonical);
}

#[test]
fn ingest_local_real_fixture_is_idempotent() {
    let fixture = local_real_fixture();
    if !fixture.exists() {
        eprintln!(
            "skipping ingest_local_real_fixture_is_idempotent: {} not present (local-only fixture)",
            fixture.display()
        );
        return;
    }

    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("state.sqlite");
    let mut store = Store::open(&db).unwrap();

    let first = ingest(
        &fixture,
        &mut store,
        ddp_transcribe::ingest::WindowBounds::default(),
        false,
    )
    .unwrap();
    forget_ingested_files(&db);
    let second = ingest(
        &fixture,
        &mut store,
        ddp_transcribe::ingest::WindowBounds::default(),
        false,
    )
    .unwrap();

    assert_eq!(first.unique_videos_seen, second.unique_videos_seen);
    assert_eq!(
        first.watch_history_rows_processed,
        second.watch_history_rows_processed
    );
    assert_eq!(
        second.watch_history_duplicates,
        second.watch_history_rows_processed
    );
}

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

fn watch_history_count(db: &std::path::Path) -> i64 {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.query_row("SELECT COUNT(*) FROM watch_history", [], |r| r.get(0))
        .unwrap()
}

/// Production incident (July 2026): the donation platform writes a decline
/// stub — a top-level JSON object, not the `Vec<Section>` array ingest
/// expects — into the same inbox as the real exports. It used to abort the
/// whole run at `serde_json::from_slice`. It must now be a counted skip.
#[test]
fn decline_stub_is_skipped_and_counted_without_aborting_the_run() {
    let tmp = tempfile::TempDir::new().unwrap();
    let inbox = tmp.path().join("inbox");
    write_ddp(
        &inbox,
        "participant=good1_source=tiktok.json",
        &[(
            "2026-02-10 12:00:00",
            "https://www.tiktokv.com/share/video/7000000000000000111/",
        )],
    );
    write_ddp(
        &inbox,
        "participant=good2_source=tiktok.json",
        &[(
            "2026-02-11 12:00:00",
            "https://www.tiktokv.com/share/video/7000000000000000222/",
        )],
    );
    std::fs::write(
        inbox.join("participant=declined_source=tiktok.json"),
        r#"{"status":"data_submission declined"}"#,
    )
    .unwrap();

    let db = tmp.path().join("state.sqlite");
    let mut store = Store::open(&db).unwrap();
    let stats = ingest(
        &inbox,
        &mut store,
        ddp_transcribe::ingest::WindowBounds::default(),
        false,
    )
    .expect("one bad file must not veto the run");

    assert_eq!(stats.files_skipped_unparseable, 1);
    assert_eq!(stats.files_processed, 2);
    assert_eq!(stats.watch_history_rows_processed, 2);
    for id in ["7000000000000000111", "7000000000000000222"] {
        assert!(
            store.get_video_for_test(id).unwrap().is_some(),
            "good file's rows must survive the bad file: {id}"
        );
    }
}

/// A file whose name carries no `participant=` segment (and unreadable /
/// unstattable files, same arm) is a counted skip, not a fatal error.
#[test]
fn junk_filename_is_skipped_and_counted() {
    let tmp = tempfile::TempDir::new().unwrap();
    let inbox = tmp.path().join("inbox");
    write_ddp(
        &inbox,
        "participant=good1_source=tiktok.json",
        &[(
            "2026-02-10 12:00:00",
            "https://www.tiktokv.com/share/video/7000000000000000111/",
        )],
    );
    // No `participant=` segment: parse_respondent_id_from_filename errors.
    write_ddp(
        &inbox,
        "some-unrelated-export.json",
        &[(
            "2026-02-10 12:00:00",
            "https://www.tiktokv.com/share/video/7000000000000000999/",
        )],
    );

    let db = tmp.path().join("state.sqlite");
    let mut store = Store::open(&db).unwrap();
    let stats = ingest(
        &inbox,
        &mut store,
        ddp_transcribe::ingest::WindowBounds::default(),
        false,
    )
    .expect("junk filename must not veto the run");

    assert_eq!(stats.files_skipped_unparseable, 1);
    assert_eq!(stats.files_processed, 1);
    assert!(store
        .get_video_for_test("7000000000000000999")
        .unwrap()
        .is_none());
}

/// Ledger fast path: a second ingest over an unchanged inbox touches no
/// rows at all — it stats each file, matches the (name, size, mtime)
/// triple, and skips. This is the fix for the 4.85M-no-op-upsert re-run.
#[test]
fn unchanged_files_are_skipped_via_the_ledger_on_re_ingest() {
    let tmp = tempfile::TempDir::new().unwrap();
    let inbox = tmp.path().join("inbox");
    write_ddp(
        &inbox,
        "participant=w1_source=tiktok.json",
        &[(
            "2026-02-10 12:00:00",
            "https://www.tiktokv.com/share/video/7000000000000000111/",
        )],
    );
    write_ddp(
        &inbox,
        "participant=w2_source=tiktok.json",
        &[(
            "2026-02-11 12:00:00",
            "https://www.tiktokv.com/share/video/7000000000000000222/",
        )],
    );

    let db = tmp.path().join("state.sqlite");
    let mut store = Store::open(&db).unwrap();
    let first = ingest(
        &inbox,
        &mut store,
        ddp_transcribe::ingest::WindowBounds::default(),
        false,
    )
    .unwrap();
    assert_eq!(first.files_processed, 2);
    assert_eq!(first.files_skipped_already_ingested, 0);
    let rows_after_first = watch_history_count(&db);

    let second = ingest(
        &inbox,
        &mut store,
        ddp_transcribe::ingest::WindowBounds::default(),
        false,
    )
    .unwrap();
    assert_eq!(second.files_skipped_already_ingested, 2);
    assert_eq!(second.files_processed, 0);
    assert_eq!(
        second.watch_history_rows_processed, 0,
        "no file was opened, so no row was even considered"
    );
    assert_eq!(watch_history_count(&db), rows_after_first);
}

/// A file whose content changed since the last ingest reprocesses: the
/// ledger match is on the whole (name, size, mtime) triple, and the row
/// upserts stay the correctness backstop for the entries that repeat.
#[test]
fn changed_file_is_reprocessed_and_its_ledger_row_updated() {
    let tmp = tempfile::TempDir::new().unwrap();
    let inbox = tmp.path().join("inbox");
    let changing = "participant=w1_source=tiktok.json";
    write_ddp(
        &inbox,
        changing,
        &[(
            "2026-02-10 12:00:00",
            "https://www.tiktokv.com/share/video/7000000000000000111/",
        )],
    );
    write_ddp(
        &inbox,
        "participant=w2_source=tiktok.json",
        &[(
            "2026-02-11 12:00:00",
            "https://www.tiktokv.com/share/video/7000000000000000222/",
        )],
    );

    let db = tmp.path().join("state.sqlite");
    let mut store = Store::open(&db).unwrap();
    ingest(
        &inbox,
        &mut store,
        ddp_transcribe::ingest::WindowBounds::default(),
        false,
    )
    .unwrap();

    let size_before: i64 = {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.query_row(
            "SELECT size_bytes FROM ingested_files WHERE file_name = ?1",
            rusqlite::params![changing],
            |r| r.get(0),
        )
        .unwrap()
    };

    // Rewrite with one extra entry: strictly larger, so the fingerprint
    // differs even if the filesystem's mtime granularity is coarse.
    write_ddp(
        &inbox,
        changing,
        &[
            (
                "2026-02-10 12:00:00",
                "https://www.tiktokv.com/share/video/7000000000000000111/",
            ),
            (
                "2026-02-12 12:00:00",
                "https://www.tiktokv.com/share/video/7000000000000000333/",
            ),
        ],
    );

    let second = ingest(
        &inbox,
        &mut store,
        ddp_transcribe::ingest::WindowBounds::default(),
        false,
    )
    .unwrap();
    assert_eq!(second.files_processed, 1, "only the changed file");
    assert_eq!(
        second.files_skipped_already_ingested, 1,
        "the unchanged one"
    );
    assert_eq!(second.watch_history_rows_processed, 2);
    assert!(store
        .get_video_for_test("7000000000000000333")
        .unwrap()
        .is_some());

    let size_after: i64 = {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.query_row(
            "SELECT size_bytes FROM ingested_files WHERE file_name = ?1",
            rusqlite::params![changing],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert!(
        size_after > size_before,
        "ledger row refreshed to the new fingerprint ({size_before} -> {size_after})"
    );
}

#[test]
fn ingest_window_flags_and_raw_preservation() {
    let tmp = tempfile::TempDir::new().unwrap();
    let inbox = tmp.path().join("inbox");
    write_ddp(
        &inbox,
        "participant=w1_source=tiktok.json",
        &[
            (
                "2026-02-10 12:00:00",
                "https://www.tiktokv.com/share/video/7000000000000000111/",
            ),
            (
                "2026-03-05 12:00:00",
                "https://www.tiktokv.com/share/video/7000000000000000222/",
            ),
        ],
    );
    let mut store = ddp_transcribe::state::Store::open(&tmp.path().join("state.sqlite")).unwrap();
    let window = ddp_transcribe::ingest::WindowBounds::from_dates(
        Some(chrono::NaiveDate::from_ymd_opt(2026, 2, 1).unwrap()),
        Some(chrono::NaiveDate::from_ymd_opt(2026, 2, 28).unwrap()),
    );
    let stats = ddp_transcribe::ingest::ingest(&inbox, &mut store, window, false).unwrap();
    assert_eq!(stats.watch_history_rows_processed, 2);
    assert_eq!(stats.computed_out_of_window, 1);

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
    assert_eq!(
        raw, "2026-02-10 12:00:00",
        "raw DDP Date string preserved verbatim"
    );
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
        &[(
            "2026-02-10 12:00:00",
            "https://www.tiktokv.com/share/video/7000000000000000111/",
        )],
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
        false,
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
    assert_eq!(
        in_w, 0,
        "re-ingest must NOT touch in_window (recompute-window is the explicit path)"
    );
    assert_eq!(raw.as_deref(), Some("2026-02-10 12:00:00"));
}

/// Two-file inbox, one watch-history row each: the shared fixture for the
/// dry-run tests below.
fn dry_run_inbox(tmp: &tempfile::TempDir) -> (std::path::PathBuf, std::path::PathBuf) {
    let inbox = tmp.path().join("inbox");
    write_ddp(
        &inbox,
        "participant=w1_source=tiktok.json",
        &[(
            "2026-02-10 12:00:00",
            "https://www.tiktokv.com/share/video/7000000000000000111/",
        )],
    );
    write_ddp(
        &inbox,
        "participant=w2_source=tiktok.json",
        &[(
            "2026-02-11 12:00:00",
            "https://www.tiktokv.com/share/video/7000000000000000222/",
        )],
    );
    (inbox, tmp.path().join("state.sqlite"))
}

fn table_count(db: &std::path::Path, table: &str) -> i64 {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

/// `--dry-run` runs the full per-file transaction — every upsert, so every
/// row-change-derived counter is real — and then rolls it back. Nothing
/// persists, not even the ingest ledger.
#[test]
fn dry_run_reports_real_stats_but_writes_nothing() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (inbox, db) = dry_run_inbox(&tmp);
    let mut store = Store::open(&db).unwrap();

    let stats = ingest(
        &inbox,
        &mut store,
        ddp_transcribe::ingest::WindowBounds::default(),
        true,
    )
    .expect("dry-run ingest succeeds");

    assert_eq!(stats.files_processed, 2);
    assert_eq!(stats.unique_videos_seen, 2);
    assert_eq!(stats.watch_history_rows_processed, 2);
    assert_eq!(stats.files_skipped_already_ingested, 0);
    assert_eq!(stats.files_skipped_unparseable, 0);

    assert_eq!(
        (
            table_count(&db, "videos"),
            table_count(&db, "watch_history")
        ),
        (0, 0),
        "dry-run must persist nothing"
    );
    assert_eq!(
        table_count(&db, "ingested_files"),
        0,
        "dry-run must not poison the ingest ledger"
    );
}

/// The ledger write rides the rolled-back transaction, so a dry-run leaves
/// no fingerprint behind: the next real run ingests everything, with stats
/// identical to the dry-run's.
#[test]
fn real_ingest_after_dry_run_ingests_everything() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (inbox, db) = dry_run_inbox(&tmp);
    let mut store = Store::open(&db).unwrap();

    let dry = ingest(
        &inbox,
        &mut store,
        ddp_transcribe::ingest::WindowBounds::default(),
        true,
    )
    .unwrap();
    let real = ingest(
        &inbox,
        &mut store,
        ddp_transcribe::ingest::WindowBounds::default(),
        false,
    )
    .unwrap();

    assert_eq!(
        real.files_skipped_already_ingested, 0,
        "ledger must be clean after a dry-run"
    );
    assert_eq!(real.files_processed, dry.files_processed);
    assert_eq!(real.unique_videos_seen, dry.unique_videos_seen);
    assert_eq!(
        real.watch_history_rows_processed,
        dry.watch_history_rows_processed
    );
    assert_eq!(real.watch_history_duplicates, dry.watch_history_duplicates);
    assert_eq!(real.computed_out_of_window, dry.computed_out_of_window);
    assert_eq!(real.backfilled_raw_dates, dry.backfilled_raw_dates);

    assert!(
        table_count(&db, "videos") > 0,
        "real run after dry-run ingests normally"
    );
    assert_eq!(table_count(&db, "ingested_files"), 2);
}

/// The operator-facing half: `ingest --dry-run` marks its summary line so a
/// dry run is never mistaken for a real one, and still writes nothing.
#[test]
fn dry_run_cli_marks_its_output_and_writes_nothing() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (inbox, db) = dry_run_inbox(&tmp);

    let assert = AssertCommand::cargo_bin("ddp-transcribe")
        .unwrap()
        .args([
            "--state-db",
            db.to_str().unwrap(),
            "--inbox",
            inbox.to_str().unwrap(),
            "ingest",
            "--dry-run",
        ])
        .assert()
        .success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(out.contains("(dry-run)"), "stdout was: {out}");
    // The dry-run pass still reports real counts.
    assert!(out.contains("files 2"), "stdout was: {out}");

    assert_eq!(
        (
            table_count(&db, "videos"),
            table_count(&db, "watch_history")
        ),
        (0, 0),
        "dry-run must persist nothing"
    );
    assert_eq!(table_count(&db, "ingested_files"), 0);
}
