#![allow(clippy::unwrap_used, clippy::expect_used)]
//! `backfill-metadata` end-to-end via a fake yt-dlp shim on the child's
//! PATH. Public API seeding (Store::open + pub upserts) plus raw
//! rusqlite, so this file needs no `[[test]]` block per 0005.

use assert_cmd::Command as AssertCommand;

/// Shim: prints one metadata JSON line for any URL, unless the URL
/// contains "dead" (exit 1, stderr) — the dead-link cohort stand-in.
const SHIM: &str = r#"#!/bin/sh
for last; do :; done
case "$last" in
  *dead*) echo "ERROR: video unavailable" >&2; exit 1 ;;
  *) printf '{"id":"shim","description":"backfilled by shim"}\n' ;;
esac
"#;

/// Writes the shim as `yt-dlp` in a fresh dir and returns a PATH value
/// putting it first. Child-process env only — never `std::env::set_var`.
fn shim_path(dir: &tempfile::TempDir) -> String {
    use std::os::unix::fs::PermissionsExt;
    let bin = dir.path().join("shim-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let shim = bin.join("yt-dlp");
    std::fs::write(&shim, SHIM).unwrap();
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    )
}

/// Tripwire shim: a `yt-dlp` on the child's PATH that records the fact of
/// its own invocation in `marker` and then behaves like a total failure.
/// Returns `(PATH value, marker path)`.
///
/// Epic 5b bundle: the dry-run test previously asserted only that the
/// process exited 0 and wrote no rows — "invokes nothing" was an inference
/// from absence of evidence, and would have held equally well if dry-run
/// had shelled out to the operator's real yt-dlp. With this shim on PATH,
/// the marker's absence is a positive assertion.
fn tripwire_path(dir: &tempfile::TempDir) -> (String, std::path::PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let bin = dir.path().join("tripwire-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let marker = dir.path().join("yt-dlp-was-invoked");
    let shim = bin.join("yt-dlp");
    std::fs::write(
        &shim,
        format!("#!/bin/sh\n: > \"{}\"\nexit 1\n", marker.display()),
    )
    .unwrap();
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    (path, marker)
}

/// Argv-recording shim: appends each argument yt-dlp was invoked with to
/// `argv_log` (one per line, so the last line is the final positional
/// arg — the URL) and then succeeds like the ordinary shim. The tripwire
/// shim above proves only that yt-dlp *ran*; this proves *which URL* it
/// received, which is what the derived-URL assertion needs.
fn recording_shim_path(dir: &tempfile::TempDir) -> (String, std::path::PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let bin = dir.path().join("recording-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let log = dir.path().join("yt-dlp-argv.log");
    let shim = bin.join("yt-dlp");
    std::fs::write(
        &shim,
        format!(
            "#!/bin/sh\nfor a; do printf '%s\\n' \"$a\" >> \"{}\"; done\nprintf '{{\"id\":\"shim\",\"description\":\"backfilled by shim\"}}\\n'\n",
            log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    (path, log)
}

/// v1 succeeded+envelope (not in cohort), v2 succeeded (cohort),
/// v3 succeeded with a "dead" URL (cohort), v4 pending (not in cohort).
fn seeded_db(dir: &tempfile::TempDir) -> std::path::PathBuf {
    let db = dir.path().join("state.sqlite");
    {
        let mut store = ddp_transcribe::state::Store::open(&db).unwrap();
        store
            .upsert_video("v1", "https://example/1", false)
            .unwrap();
        store
            .upsert_video("v2", "https://example/2", false)
            .unwrap();
        store
            .upsert_video("v3", "https://example/dead3", false)
            .unwrap();
        store
            .upsert_video("v4", "https://example/4", false)
            .unwrap();
        store
            .insert_metadata_raw_if_missing("v1", r#"{"schema":1,"printed":"{\"id\":\"v1\"}"}"#)
            .unwrap();
    }
    // Flip statuses with raw rusqlite — no public mutator sets
    // `succeeded` without a claim, and tests must not grow one.
    // `attempt_count` / `succeeded_at` are seeded to NON-default values so
    // the widened `statuses()` snapshot has something to lose: an all-zero
    // baseline would pass a regression that reset them.
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "UPDATE videos SET status = 'succeeded', attempt_count = 2, succeeded_at = 1750000000
         WHERE video_id IN ('v1','v2','v3')",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE videos SET attempt_count = 1 WHERE video_id = 'v4'",
        [],
    )
    .unwrap();
    db
}

/// One video's lifecycle columns — every column `backfill-metadata` must
/// never write (ADR-0042's metadata-only carve-out), not just `status`.
/// Widened in the Epic 5b test-hardening bundle: a regression that stamped
/// a claim, bumped `attempt_count`, or rewrote `succeeded_at` used to pass
/// the old `(video_id, status)` snapshot silently.
type LifecycleRow = (
    String,         // video_id
    String,         // status
    Option<String>, // claimed_by
    Option<i64>,    // claimed_at
    i64,            // attempt_count
    Option<i64>,    // succeeded_at
);

fn statuses(db: &std::path::Path) -> Vec<LifecycleRow> {
    let conn = rusqlite::Connection::open(db).unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT video_id, status, claimed_by, claimed_at, attempt_count, succeeded_at
             FROM videos ORDER BY video_id",
        )
        .unwrap();
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    rows
}

#[test]
fn dry_run_prints_cohort_and_invokes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = seeded_db(&dir);
    let before = statuses(&db);
    let (path, marker) = tripwire_path(&dir);

    let assert = AssertCommand::cargo_bin("ddp-transcribe")
        .unwrap()
        .env("PATH", path)
        .args([
            "--state-db",
            db.to_str().unwrap(),
            "backfill-metadata",
            "--dry-run",
        ])
        .assert()
        .success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(out.contains("cohort 2"), "stdout was: {out}");
    assert!(out.contains("dry-run"));

    // The positive assertion: the tripwire `yt-dlp` was never reached. Its
    // shim exits 1, so had dry-run invoked it the run would also have had a
    // capture failure to report.
    assert!(
        !marker.exists(),
        "dry-run must invoke no subprocess; the tripwire yt-dlp ran"
    );

    let conn = rusqlite::Connection::open(&db).unwrap();
    let raw_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM video_metadata_raw", [], |r| r.get(0))
        .unwrap();
    assert_eq!(raw_rows, 1, "dry-run must not write");
    assert_eq!(statuses(&db), before);
}

#[test]
fn backfill_captures_cohort_best_effort_and_never_touches_status() {
    let dir = tempfile::tempdir().unwrap();
    let db = seeded_db(&dir);
    let before = statuses(&db);

    let assert = AssertCommand::cargo_bin("ddp-transcribe")
        .unwrap()
        .env("PATH", shim_path(&dir))
        .args(["--state-db", db.to_str().unwrap(), "backfill-metadata"])
        .assert()
        .success(); // dead video must NOT fail the run (best-effort)
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(out.contains("cohort 2"), "stdout was: {out}");
    assert!(
        out.contains("examined 2")
            && out.contains("captured 1")
            && out.contains("capture-failed 1"),
        "stdout was: {out}"
    );

    let conn = rusqlite::Connection::open(&db).unwrap();
    // v2 gained a schema:1 envelope wrapping the shim's printed line.
    let v2_raw: String = conn
        .query_row(
            "SELECT raw_json FROM video_metadata_raw WHERE video_id = 'v2'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(v2_raw.contains(r#""schema":1"#), "raw was: {v2_raw}");
    assert!(v2_raw.contains("backfilled by shim"), "raw was: {v2_raw}");
    // v1's pre-existing envelope untouched; v3 (dead) has none.
    let v1_raw: String = conn
        .query_row(
            "SELECT raw_json FROM video_metadata_raw WHERE video_id = 'v1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(v1_raw.contains(r#"\"id\":\"v1\""#), "raw was: {v1_raw}");
    let v3_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM video_metadata_raw WHERE video_id = 'v3'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(v3_rows, 0);

    // THE invariant: statuses and lifecycle byte-identical.
    assert_eq!(statuses(&db), before);

    // Re-run converges: only v3 (still dead) is attempted.
    let assert = AssertCommand::cargo_bin("ddp-transcribe")
        .unwrap()
        .env("PATH", shim_path(&dir))
        .args(["--state-db", db.to_str().unwrap(), "backfill-metadata"])
        .assert()
        .success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        out.contains("cohort 1") && out.contains("examined 1"),
        "stdout was: {out}"
    );
}

#[test]
fn limit_caps_attempts() {
    let dir = tempfile::tempdir().unwrap();
    let db = seeded_db(&dir);

    let assert = AssertCommand::cargo_bin("ddp-transcribe")
        .unwrap()
        .env("PATH", shim_path(&dir))
        .args([
            "--state-db",
            db.to_str().unwrap(),
            "backfill-metadata",
            "--limit",
            "1",
        ])
        .assert()
        .success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(out.contains("examined 1"), "stdout was: {out}");
}

/// Fetch-URL ADR (0049): a canonical backfill row's stored `source_url`
/// may be a surviving share form, but the recovery fetch must use the
/// derived `@x` transport form — the same form the fetch-path pipeline
/// uses (`tests/pipeline_fakes/pipelined_tests.rs`), never a second
/// URL-format literal.
#[test]
fn backfill_fetches_derived_canonical_url() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.sqlite");
    let vid = "7700000000000000009";
    let stored = format!("https://www.tiktokv.com/share/video/{vid}/");
    {
        let mut store = ddp_transcribe::state::Store::open(&db).unwrap();
        store.upsert_video(vid, &stored, true).unwrap();
    }
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute(
            "UPDATE videos SET status = 'succeeded', attempt_count = 1, succeeded_at = 1750000000
             WHERE video_id = ?1",
            [vid],
        )
        .unwrap();

    let (path, log) = recording_shim_path(&dir);

    let assert = AssertCommand::cargo_bin("ddp-transcribe")
        .unwrap()
        .env("PATH", path)
        .args(["--state-db", db.to_str().unwrap(), "backfill-metadata"])
        .assert()
        .success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(out.contains("captured 1"), "stdout was: {out}");

    let logged = std::fs::read_to_string(&log).unwrap();
    let last_arg = logged.lines().last().expect("shim recorded argv");
    assert_eq!(
        last_arg,
        format!("https://www.tiktok.com/@x/video/{vid}/"),
        "backfill must fetch the derived canonical form, not the stored share URL"
    );
}

#[test]
fn refuses_missing_db() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("nope.sqlite");
    AssertCommand::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["--state-db", db.to_str().unwrap(), "backfill-metadata"])
        .assert()
        .failure();
    assert!(!db.exists(), "must not create an empty DB");
}

/// The URL the live test fetches. Override with `DDP_TRANSCRIBE_E2E_URL`
/// (the `tests/e2e_real_tools.rs` idiom); the compiled-in default is a
/// real public URL from the `news_orgs` bake fixture, which may age out —
/// set the variable for a dependable manual run.
fn live_url() -> String {
    std::env::var("DDP_TRANSCRIBE_E2E_URL")
        .unwrap_or_else(|_| "https://www.tiktok.com/@nosstories/video/7636781376787795232".into())
}

/// Live smoke: real yt-dlp + network. Run explicitly:
/// `cargo test --test backfill_metadata -- --ignored --test-threads=1`
#[test]
#[ignore = "network + real yt-dlp; run explicitly before release"]
fn live_backfill_captures_one_real_video() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.sqlite");
    {
        let mut store = ddp_transcribe::state::Store::open(&db).unwrap();
        store.upsert_video("live1", &live_url(), false).unwrap();
    }
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute("UPDATE videos SET status = 'succeeded'", [])
        .unwrap();

    AssertCommand::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["--state-db", db.to_str().unwrap(), "backfill-metadata"])
        .timeout(std::time::Duration::from_secs(120))
        .assert()
        .success();

    let raw: String = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row(
            "SELECT raw_json FROM video_metadata_raw WHERE video_id = 'live1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(raw.contains(r#""schema":1"#), "raw was: {raw}");
}
