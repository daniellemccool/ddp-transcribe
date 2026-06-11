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

    let stats: IngestStats = ingest(&news_orgs_fixture(), &mut store).expect("ingest succeeds");

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

    let first = ingest(&news_orgs_fixture(), &mut store).unwrap();
    let second = ingest(&news_orgs_fixture(), &mut store).unwrap();

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

    let stats: IngestStats = ingest(&fixture, &mut store).expect("ingest succeeds");

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

    let first = ingest(&fixture, &mut store).unwrap();
    let second = ingest(&fixture, &mut store).unwrap();

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
