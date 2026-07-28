#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

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

#[test]
fn ingest_news_orgs_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let mut store = Store::open(&tmp.path().join("state.sqlite")).unwrap();

    let first = ingest(
        &news_orgs_fixture(),
        &mut store,
        ddp_transcribe::ingest::WindowBounds::default(),
    )
    .unwrap();
    let second = ingest(
        &news_orgs_fixture(),
        &mut store,
        ddp_transcribe::ingest::WindowBounds::default(),
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
    let mut store = Store::open(&tmp.path().join("state.sqlite")).unwrap();

    let first = ingest(
        &fixture,
        &mut store,
        ddp_transcribe::ingest::WindowBounds::default(),
    )
    .unwrap();
    let second = ingest(
        &fixture,
        &mut store,
        ddp_transcribe::ingest::WindowBounds::default(),
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
