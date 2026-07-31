#![allow(clippy::unwrap_used, clippy::expect_used)]

use ddp_transcribe::state::Store;
use tempfile::TempDir;

fn fresh_store() -> (TempDir, Store) {
    let tmp = TempDir::new().unwrap();
    let store = Store::open(&tmp.path().join("state.sqlite")).expect("open");
    (tmp, store)
}

#[test]
fn upsert_video_inserts_new_row_with_pending_status() {
    let (_tmp, mut store) = fresh_store();
    store
        .upsert_video(
            "7234567890123456789",
            "https://www.tiktokv.com/share/video/7234567890123456789/",
            true,
        )
        .expect("upsert");

    let row = store
        .get_video_for_test("7234567890123456789")
        .unwrap()
        .expect("row exists");
    assert_eq!(row.status, "pending");
    assert!(row.canonical);
    assert!(row.first_seen_at > 0);
    assert_eq!(row.attempt_count, 0);
}

#[test]
fn upsert_video_is_idempotent_first_seen_at_unchanged() {
    let (_tmp, mut store) = fresh_store();
    store
        .upsert_video("7234567890123456789", "url-A", true)
        .unwrap();
    let first = store
        .get_video_for_test("7234567890123456789")
        .unwrap()
        .unwrap();

    std::thread::sleep(std::time::Duration::from_millis(1100));

    store
        .upsert_video("7234567890123456789", "url-B", true)
        .unwrap();
    let second = store
        .get_video_for_test("7234567890123456789")
        .unwrap()
        .unwrap();

    assert_eq!(
        first.first_seen_at, second.first_seen_at,
        "first_seen_at must NOT change on re-upsert"
    );
    assert_eq!(
        first.source_url, second.source_url,
        "source_url must NOT change on re-upsert (we keep the first one)"
    );
}

/// Pins the operator-ruled `updated_at` contract: the column records
/// *lifecycle-mutation* time, so a no-op re-ingest is clock-neutral. Guards
/// against someone "fixing" `UPSERT_VIDEO_SQL` into an
/// `ON CONFLICT DO UPDATE SET updated_at = excluded.updated_at`, which would
/// make every re-ingest of the (multi-million-row) inbox rewrite the column
/// and destroy its meaning as a last-lifecycle-change marker.
#[test]
fn re_ingest_of_an_existing_row_does_not_change_updated_at() {
    let (tmp, mut store) = fresh_store();
    let db = tmp.path().join("state.sqlite");
    store
        .upsert_video("7234567890123456789", "url-A", true)
        .unwrap();

    let read_updated_at = || -> i64 {
        rusqlite::Connection::open(&db)
            .unwrap()
            .query_row(
                "SELECT updated_at FROM videos WHERE video_id = '7234567890123456789'",
                [],
                |r| r.get(0),
            )
            .unwrap()
    };
    let first = read_updated_at();
    assert!(first > 0);

    std::thread::sleep(std::time::Duration::from_millis(1100));

    let changed = store
        .upsert_video("7234567890123456789", "url-A", true)
        .unwrap();
    assert_eq!(changed, 0, "re-ingest changes no rows (0006)");
    assert_eq!(
        first,
        read_updated_at(),
        "a no-op ingest is clock-neutral: updated_at is lifecycle-mutation time"
    );
}

#[test]
fn upsert_watch_history_inserts() {
    let (_tmp, mut store) = fresh_store();
    store
        .upsert_video(
            "7234567890123456789",
            "https://example/video/7234567890123456789",
            true,
        )
        .unwrap();
    let inserted = store
        .upsert_watch_history(
            "respondent-1",
            "7234567890123456789",
            1_700_000_000,
            "2026-01-01 00:00:00",
            true,
        )
        .unwrap();
    assert_eq!(inserted, 1);
}

#[test]
fn upsert_watch_history_is_idempotent_on_duplicate_pk() {
    let (_tmp, mut store) = fresh_store();
    store
        .upsert_video("7234567890123456789", "url", true)
        .unwrap();
    let first = store
        .upsert_watch_history(
            "r1",
            "7234567890123456789",
            1_700_000_000,
            "2026-01-01 00:00:00",
            true,
        )
        .unwrap();
    let second = store
        .upsert_watch_history(
            "r1",
            "7234567890123456789",
            1_700_000_000,
            "2026-01-01 00:00:00",
            false,
        )
        .unwrap();
    assert_eq!(first, 1);
    assert_eq!(second, 0, "duplicate PK insert returns 0 rows changed");
}

#[test]
fn upsert_watch_history_fk_violation_when_video_missing() {
    let (_tmp, mut store) = fresh_store();
    let result = store.upsert_watch_history(
        "r1",
        "7234567890123456789",
        1_700_000_000,
        "2026-01-01 00:00:00",
        true,
    );
    assert!(result.is_err(), "FK should fail when video row absent");
}
