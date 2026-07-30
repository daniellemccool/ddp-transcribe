//! Executable spec for the `requeue-failures` operator override (ADR-0046):
//! a forensic, default-deny override of ADR-0036 retry eligibility. These
//! tests pin the binding behavior — eligibility, terminal opt-in, the
//! failure-event allowlist clock, deterministic `--max` ordering, dry-run
//! non-mutation, zero-match, and filter conjunction.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::time::Duration;

use rusqlite::Connection;
use tempfile::TempDir;

use ddp_transcribe::state::{FailureRecordOutcome, RequeueFilter, RequeueOutcome, Store};

const ACTOR: &str = "operator:test-host-4242";

fn fresh_store() -> (Store, TempDir) {
    let tmp = TempDir::new().unwrap();
    let store = Store::open(&tmp.path().join("state.sqlite")).unwrap();
    (store, tmp)
}

/// No selector set: every test starts here and switches on exactly the
/// selectors it is pinning. Deliberately not a `Default` impl on the type —
/// "no selector" is precisely the invocation ADR-0046 forbids.
fn base_filter() -> RequeueFilter {
    RequeueFilter {
        error_kinds: Vec::new(),
        max_attempts: None,
        older_than: None,
        include_terminal: false,
        all: false,
        max: None,
    }
}

fn now_secs() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    )
    .unwrap()
}

/// Seeds one `failed_retryable` row. Called one row at a time so
/// `claim_next` provably takes the row just upserted.
fn seed_failed_retryable(store: &mut Store, video_id: &str, kind: &str, message: &str) {
    store
        .upsert_video(video_id, "https://example.com/v", true)
        .unwrap();
    let claim = store.claim_next("seed-worker").unwrap().expect("claim");
    assert_eq!(claim.video_id, video_id, "claim_next took the seeded row");
    store
        .mark_retryable_failure(video_id, "seed-worker", kind, message)
        .unwrap();
}

/// Seeds a `failed_terminal` row that still carries its retryable kind —
/// the shape that proves terminal matching keys on `terminal_reason` and
/// never on a retained `last_retryable_kind`.
fn seed_failed_terminal(
    store: &mut Store,
    video_id: &str,
    retryable_kind: &str,
    terminal_reason: &str,
) {
    seed_failed_retryable(store, video_id, retryable_kind, "seed message");
    let n = store
        .sweep_mark_terminal(video_id, terminal_reason, "write-off")
        .unwrap();
    assert_eq!(n, 1);
}

fn raw(db_path: &Path) -> Connection {
    Connection::open(db_path).unwrap()
}

/// The columns 0046 makes claims about, read back raw so the assertions see
/// the database rather than the library's view of it.
struct RawVideoRow {
    status: String,
    claimed_by: Option<String>,
    claimed_at: Option<i64>,
    attempt_count: i64,
    last_retryable_kind: Option<String>,
    last_retryable_message: Option<String>,
    updated_at: i64,
}

fn read_row(db_path: &Path, video_id: &str) -> RawVideoRow {
    raw(db_path)
        .query_row(
            "SELECT status, claimed_by, claimed_at, attempt_count, last_retryable_kind,
                    last_retryable_message, updated_at
             FROM videos WHERE video_id = ?1",
            [video_id],
            |r| {
                Ok(RawVideoRow {
                    status: r.get(0)?,
                    claimed_by: r.get(1)?,
                    claimed_at: r.get(2)?,
                    attempt_count: r.get(3)?,
                    last_retryable_kind: r.get(4)?,
                    last_retryable_message: r.get(5)?,
                    updated_at: r.get(6)?,
                })
            },
        )
        .unwrap()
}

fn status_of(db_path: &Path, video_id: &str) -> String {
    raw(db_path)
        .query_row(
            "SELECT status FROM videos WHERE video_id = ?1",
            [video_id],
            |r| r.get(0),
        )
        .unwrap()
}

fn set_attempt_count(db_path: &Path, video_id: &str, attempts: i64) {
    raw(db_path)
        .execute(
            "UPDATE videos SET attempt_count = ?2 WHERE video_id = ?1",
            rusqlite::params![video_id, attempts],
        )
        .unwrap();
}

fn backdate_all_events(db_path: &Path, video_id: &str, at: i64) {
    raw(db_path)
        .execute(
            "UPDATE video_events SET at = ?2 WHERE video_id = ?1",
            rusqlite::params![video_id, at],
        )
        .unwrap();
}

fn insert_event(db_path: &Path, video_id: &str, event_type: &str, at: i64) {
    raw(db_path)
        .execute(
            "INSERT INTO video_events (video_id, at, event_type, worker_id, detail_json)
             VALUES (?1, ?2, ?3, 'sweep', NULL)",
            rusqlite::params![video_id, at, event_type],
        )
        .unwrap();
}

fn delete_events(db_path: &Path, video_id: &str) {
    raw(db_path)
        .execute("DELETE FROM video_events WHERE video_id = ?1", [video_id])
        .unwrap();
}

fn operator_events(db_path: &Path) -> Vec<(String, String, Option<String>)> {
    let conn = raw(db_path);
    let mut stmt = conn
        .prepare(
            "SELECT video_id, worker_id, detail_json FROM video_events
             WHERE event_type = 'operator_requeued' ORDER BY video_id ASC",
        )
        .unwrap();
    let rows = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    rows
}

fn event_count(db_path: &Path) -> i64 {
    raw(db_path)
        .query_row("SELECT COUNT(*) FROM video_events", [], |r| r.get(0))
        .unwrap()
}

// 1. Eligibility: the transition, the retained history, and the forensics.
#[test]
fn matching_retryable_row_goes_pending_with_history_retained_and_one_event() {
    let (mut store, tmp) = fresh_store();
    let db_path = tmp.path().join("state.sqlite");
    seed_failed_retryable(&mut store, "7000000000000000101", "GeoBlock", "geo-blocked");

    let (before_updated_at, before_attempts): (i64, i64) = raw(&db_path)
        .query_row(
            "SELECT updated_at, attempt_count FROM videos WHERE video_id = '7000000000000000101'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(before_attempts, 1);

    let mut filter = base_filter();
    filter.error_kinds = vec!["GeoBlock".to_string()];
    let outcome = store.requeue_failures(&filter, ACTOR, false).unwrap();
    assert_eq!(
        outcome,
        RequeueOutcome {
            matched: 1,
            requeued: 1,
            by_kind: vec![("GeoBlock".to_string(), 1)],
        }
    );

    let row = read_row(&db_path, "7000000000000000101");
    assert_eq!(row.status, "pending");
    assert_eq!(row.claimed_by, None, "claim defensively cleared");
    assert_eq!(row.claimed_at, None, "claim defensively cleared");
    assert_eq!(
        row.attempt_count, 1,
        "attempt_count is never reset or decremented"
    );
    assert_eq!(
        row.last_retryable_kind.as_deref(),
        Some("GeoBlock"),
        "history retained"
    );
    assert_eq!(
        row.last_retryable_message.as_deref(),
        Some("geo-blocked"),
        "history retained"
    );
    assert_eq!(
        row.updated_at, before_updated_at,
        "videos.updated_at is not touched (ADR-0046)"
    );

    let events = operator_events(&db_path);
    assert_eq!(events.len(), 1, "exactly one operator_requeued event");
    let (video_id, worker_id, detail_json) = &events[0];
    assert_eq!(video_id, "7000000000000000101");
    assert_eq!(worker_id, ACTOR, "operator:<host>-<pid> attribution");
    let detail: serde_json::Value =
        serde_json::from_str(detail_json.as_deref().expect("detail_json present")).unwrap();
    assert_eq!(detail["prior_status"], "failed_retryable");
    assert_eq!(detail["prior_kind"], "GeoBlock");
    assert_eq!(detail["attempt_count"], 1);
}

// 2. Terminal rows are opt-in twice over, and match on terminal_reason only.
#[test]
fn terminal_rows_need_include_terminal_and_match_on_terminal_reason() {
    let (mut store, tmp) = fresh_store();
    let db_path = tmp.path().join("state.sqlite");
    seed_failed_terminal(
        &mut store,
        "7000000000000000102",
        "GeoBlock",
        "IpBlockedMessage",
    );

    // Without --include-terminal the terminal row is invisible.
    let mut filter = base_filter();
    filter.error_kinds = vec!["IpBlockedMessage".to_string()];
    let outcome = store.requeue_failures(&filter, ACTOR, false).unwrap();
    assert_eq!(outcome.matched, 0);
    assert_eq!(
        status_of(&db_path, "7000000000000000102"),
        "failed_terminal",
        "terminal row untouched without --include-terminal"
    );
    assert!(operator_events(&db_path).is_empty());

    // The retained retryable kind never matches a terminal row.
    let mut retained = base_filter();
    retained.error_kinds = vec!["GeoBlock".to_string()];
    retained.include_terminal = true;
    let outcome = store.requeue_failures(&retained, ACTOR, false).unwrap();
    assert_eq!(
        outcome.matched, 0,
        "terminal matching uses terminal_reason, never a retained retryable kind"
    );

    // With --include-terminal plus a qualifying terminal selector it moves.
    filter.include_terminal = true;
    let outcome = store.requeue_failures(&filter, ACTOR, false).unwrap();
    assert_eq!(outcome.matched, 1);
    assert_eq!(outcome.requeued, 1);
    assert_eq!(
        outcome.by_kind,
        vec![("IpBlockedMessage".to_string(), 1)],
        "per-kind counts report the terminal reason"
    );
    assert_eq!(status_of(&db_path, "7000000000000000102"), "pending");

    let events = operator_events(&db_path);
    assert_eq!(events.len(), 1);
    let detail: serde_json::Value =
        serde_json::from_str(events[0].2.as_deref().expect("detail_json")).unwrap();
    assert_eq!(detail["prior_status"], "failed_terminal");
    assert_eq!(detail["prior_kind"], "IpBlockedMessage");
}

// 3. The failure clock is the event allowlist, not the row's latest event.
#[test]
fn older_than_reads_the_failure_event_allowlist_only() {
    let (mut store, tmp) = fresh_store();
    let db_path = tmp.path().join("state.sqlite");
    let now = now_secs();
    let old = now - 10 * 86_400;

    // A: old failure event, but recent ADMINISTRATIVE events on top.
    seed_failed_retryable(&mut store, "7000000000000000111", "GeoBlock", "old failure");
    backdate_all_events(&db_path, "7000000000000000111", old);
    insert_event(&db_path, "7000000000000000111", "requeued", now);
    insert_event(&db_path, "7000000000000000111", "swept_terminal", now);
    insert_event(&db_path, "7000000000000000111", "swept_stale", now);

    // B: a fresh failure event blocks the match.
    seed_failed_retryable(
        &mut store,
        "7000000000000000112",
        "GeoBlock",
        "fresh failure",
    );

    // C: no allowlist events at all — never matches --older-than.
    seed_failed_retryable(&mut store, "7000000000000000113", "GeoBlock", "no events");
    delete_events(&db_path, "7000000000000000113");

    let mut filter = base_filter();
    filter.older_than = Some(Duration::from_secs(7 * 86_400));
    let outcome = store.requeue_failures(&filter, ACTOR, false).unwrap();

    assert_eq!(outcome.matched, 1, "only the genuinely old failure matches");
    assert_eq!(
        status_of(&db_path, "7000000000000000111"),
        "pending",
        "administrative events ('requeued', 'swept_terminal', 'swept_stale') never reset the clock"
    );
    assert_eq!(
        status_of(&db_path, "7000000000000000112"),
        "failed_retryable",
        "a fresh failed_retryable event blocks the match"
    );
    assert_eq!(
        status_of(&db_path, "7000000000000000113"),
        "failed_retryable",
        "a row with no allowlist event never matches --older-than"
    );
}

// 4. --max is deterministic: attempt_count ASC, video_id ASC.
#[test]
fn max_takes_the_lowest_rows_deterministically() {
    fn fixture() -> (Store, TempDir) {
        let (mut store, tmp) = fresh_store();
        let db_path = tmp.path().join("state.sqlite");
        for id in [
            "7000000000000000121",
            "7000000000000000122",
            "7000000000000000123",
        ] {
            seed_failed_retryable(&mut store, id, "GeoBlock", "msg");
        }
        // Lowest tier is attempt_count = 1, shared by …122 and …123;
        // …121 sorts first by video_id but LAST by attempt_count.
        set_attempt_count(&db_path, "7000000000000000121", 2);
        (store, tmp)
    }

    let mut selected = Vec::new();
    for _ in 0..2 {
        let (mut store, tmp) = fixture();
        let db_path = tmp.path().join("state.sqlite");
        let mut filter = base_filter();
        filter.all = true;
        filter.max = Some(2);
        let outcome = store.requeue_failures(&filter, ACTOR, false).unwrap();
        assert_eq!(outcome.matched, 2);
        assert_eq!(outcome.requeued, 2);
        assert_eq!(
            status_of(&db_path, "7000000000000000121"),
            "failed_retryable",
            "highest attempt_count is left behind by --max 2"
        );
        selected.push(
            operator_events(&db_path)
                .into_iter()
                .map(|(v, _, _)| v)
                .collect::<Vec<_>>(),
        );
    }
    assert_eq!(
        selected[0],
        vec![
            "7000000000000000122".to_string(),
            "7000000000000000123".to_string()
        ],
        "attempt_count ASC, video_id ASC"
    );
    assert_eq!(selected[0], selected[1], "selection is reproducible");
}

// 5. --dry-run reports matches and writes nothing.
#[test]
fn dry_run_reports_matches_and_writes_nothing() {
    let (mut store, tmp) = fresh_store();
    let db_path = tmp.path().join("state.sqlite");
    seed_failed_retryable(&mut store, "7000000000000000131", "GeoBlock", "msg");
    seed_failed_retryable(&mut store, "7000000000000000132", "GeoBlock", "msg");
    let events_before = event_count(&db_path);

    let mut filter = base_filter();
    filter.all = true;
    let outcome = store.requeue_failures(&filter, ACTOR, true).unwrap();
    assert_eq!(outcome.matched, 2);
    assert_eq!(outcome.requeued, 0, "dry-run requeues nothing");
    assert_eq!(outcome.by_kind, vec![("GeoBlock".to_string(), 2)]);

    for id in ["7000000000000000131", "7000000000000000132"] {
        assert_eq!(status_of(&db_path, id), "failed_retryable");
    }
    assert_eq!(event_count(&db_path), events_before, "zero DB writes");
    assert!(operator_events(&db_path).is_empty());
}

// 6. Zero matches: nothing reported, nothing written.
#[test]
fn zero_match_reports_nothing_and_writes_no_events() {
    let (mut store, tmp) = fresh_store();
    let db_path = tmp.path().join("state.sqlite");
    seed_failed_retryable(&mut store, "7000000000000000141", "GeoBlock", "msg");
    let events_before = event_count(&db_path);

    let mut filter = base_filter();
    filter.error_kinds = vec!["NoSuchKind".to_string()];
    let outcome = store.requeue_failures(&filter, ACTOR, false).unwrap();
    assert_eq!(
        outcome,
        RequeueOutcome {
            matched: 0,
            requeued: 0,
            by_kind: Vec::new(),
        }
    );
    assert_eq!(
        status_of(&db_path, "7000000000000000141"),
        "failed_retryable"
    );
    assert_eq!(event_count(&db_path), events_before);
}

// 7. Different selector types AND together.
#[test]
fn selectors_of_different_types_and_together() {
    let (mut store, tmp) = fresh_store();
    let db_path = tmp.path().join("state.sqlite");
    let old = now_secs() - 10 * 86_400;

    // Matches all three selectors.
    seed_failed_retryable(&mut store, "7000000000000000151", "GeoBlock", "msg");
    backdate_all_events(&db_path, "7000000000000000151", old);
    // Over the attempt cap.
    seed_failed_retryable(&mut store, "7000000000000000152", "GeoBlock", "msg");
    backdate_all_events(&db_path, "7000000000000000152", old);
    set_attempt_count(&db_path, "7000000000000000152", 5);
    // Wrong kind.
    seed_failed_retryable(&mut store, "7000000000000000153", "Timeout", "msg");
    backdate_all_events(&db_path, "7000000000000000153", old);
    // Too recent.
    seed_failed_retryable(&mut store, "7000000000000000154", "GeoBlock", "msg");

    let mut filter = base_filter();
    filter.error_kinds = vec!["GeoBlock".to_string()];
    filter.max_attempts = Some(3);
    filter.older_than = Some(Duration::from_secs(7 * 86_400));
    let outcome = store.requeue_failures(&filter, ACTOR, false).unwrap();

    assert_eq!(outcome.matched, 1, "selectors AND, they do not union");
    assert_eq!(status_of(&db_path, "7000000000000000151"), "pending");
    for id in [
        "7000000000000000152",
        "7000000000000000153",
        "7000000000000000154",
    ] {
        assert_eq!(status_of(&db_path, id), "failed_retryable");
    }
}

// Repeated --error-kind values OR with each other (controller resolution).
#[test]
fn repeated_error_kinds_or_together() {
    let (mut store, tmp) = fresh_store();
    let db_path = tmp.path().join("state.sqlite");
    seed_failed_retryable(&mut store, "7000000000000000161", "GeoBlock", "msg");
    seed_failed_retryable(&mut store, "7000000000000000162", "Timeout", "msg");
    seed_failed_retryable(&mut store, "7000000000000000163", "Other", "msg");

    let mut filter = base_filter();
    filter.error_kinds = vec!["GeoBlock".to_string(), "Timeout".to_string()];
    let outcome = store.requeue_failures(&filter, ACTOR, false).unwrap();
    assert_eq!(outcome.matched, 2);
    assert_eq!(
        outcome.by_kind,
        vec![("GeoBlock".to_string(), 1), ("Timeout".to_string(), 1)],
        "per-kind counts are sorted and cover every matched kind"
    );
    assert_eq!(
        status_of(&db_path, "7000000000000000163"),
        "failed_retryable"
    );
}

// Kind matching is exact byte equality: no case folding, no comma splitting.
#[test]
fn error_kind_matching_is_exact_byte_equality() {
    let (mut store, tmp) = fresh_store();
    let db_path = tmp.path().join("state.sqlite");
    seed_failed_retryable(&mut store, "7000000000000000171", "GeoBlock", "msg");
    seed_failed_retryable(&mut store, "7000000000000000172", "a,b", "msg");

    let mut wrong_case = base_filter();
    wrong_case.error_kinds = vec!["geoblock".to_string()];
    assert_eq!(
        store
            .requeue_failures(&wrong_case, ACTOR, false)
            .unwrap()
            .matched,
        0,
        "no case folding"
    );

    let mut comma = base_filter();
    comma.error_kinds = vec!["a,b".to_string()];
    let outcome = store.requeue_failures(&comma, ACTOR, false).unwrap();
    assert_eq!(outcome.matched, 1, "a comma-bearing label is one kind");
    assert_eq!(status_of(&db_path, "7000000000000000172"), "pending");
    assert_eq!(
        status_of(&db_path, "7000000000000000171"),
        "failed_retryable"
    );
}

// Post-override arithmetic, exact (ADR-0046): the override grants ONE forced
// attempt. For pre-requeue attempt_count = A the next claim bumps it to
// A + 1, and 0036's in-pipeline requeue needs attempt_count < retries + 1 —
// so an automatic retry after the forced fetch requires --retries > A
// strictly. Worked example from the record: A = 3.
#[test]
fn post_override_retry_needs_retries_strictly_greater_than_prior_attempts() {
    /// Row exhausted at A = 3, overridden back to pending, then claimed —
    /// leaving it in_progress at attempt_count = 4, exactly where the next
    /// failure has to decide requeue-vs-exhaust.
    fn forced_attempt(video_id: &str) -> (Store, TempDir) {
        let (mut store, tmp) = fresh_store();
        let db_path = tmp.path().join("state.sqlite");
        seed_failed_retryable(&mut store, video_id, "GeoBlock", "msg");
        set_attempt_count(&db_path, video_id, 3);

        let mut filter = base_filter();
        filter.error_kinds = vec!["GeoBlock".to_string()];
        assert_eq!(
            store
                .requeue_failures(&filter, ACTOR, false)
                .unwrap()
                .matched,
            1
        );
        assert_eq!(
            read_row(&db_path, video_id).attempt_count,
            3,
            "the override never resets or decrements A"
        );

        let claim = store.claim_next("worker-1").unwrap().expect("claim");
        assert_eq!(claim.video_id, video_id);
        assert_eq!(claim.attempt_count, 4, "the next claim bumps A to A + 1");
        (store, tmp)
    }

    // --retries 3 (max_attempts = retries + 1 = 4): 4 < 4 is false — the
    // forced attempt is the ONLY one; the row exhausts again.
    let (mut store, _tmp) = forced_attempt("7000000000000000191");
    let outcome = store
        .record_fetch_failure(
            "7000000000000000191",
            "worker-1",
            "GeoBlock",
            "still blocked",
            "deterministic-audio",
            4,
            false,
            false,
        )
        .unwrap();
    assert_eq!(
        outcome,
        FailureRecordOutcome::Exhausted,
        "--retries = A is insufficient for an automatic retry"
    );

    // --retries 4 (max_attempts = 5): 4 < 5 — automatic retry resumes.
    let (mut store, _tmp) = forced_attempt("7000000000000000192");
    let outcome = store
        .record_fetch_failure(
            "7000000000000000192",
            "worker-1",
            "GeoBlock",
            "still blocked",
            "deterministic-audio",
            5,
            false,
            false,
        )
        .unwrap();
    assert_eq!(
        outcome,
        FailureRecordOutcome::Requeued,
        "--retries > A strictly is what buys an automatic retry"
    );
}

// Defense in depth behind clap's default-deny grammar: the mutator itself
// refuses a filter carrying no qualifying selector and no --all.
#[test]
fn mutator_refuses_an_unqualified_filter() {
    let (mut store, tmp) = fresh_store();
    let db_path = tmp.path().join("state.sqlite");
    seed_failed_retryable(&mut store, "7000000000000000181", "GeoBlock", "msg");

    let mut modifiers_only = base_filter();
    modifiers_only.max = Some(10);
    let err = store
        .requeue_failures(&modifiers_only, ACTOR, false)
        .unwrap_err();
    assert!(
        err.to_string().contains("qualifying selector"),
        "unexpected error: {err}"
    );
    assert_eq!(
        status_of(&db_path, "7000000000000000181"),
        "failed_retryable"
    );
}
