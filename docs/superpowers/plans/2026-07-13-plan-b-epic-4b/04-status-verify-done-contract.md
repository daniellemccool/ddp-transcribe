# Task 04: `status --verify` — the archived ADR-0017 done-contract (artifacts, schema_version, pause-safe)

**Files:**
- Modify: `src/state/queries.rs` (add `list_succeeded_ids`)
- Modify: `src/status.rs` (add `VerifyReport`, `run_verify`, render section; `StatusReport` gains an optional `verify` field)
- Modify: `src/cli.rs` (Status gains `--verify`, conflicting with the detail modes)
- Modify: `src/main.rs` (wire `--verify`; non-zero exit when not pause-safe)
- Modify: `tests/status.rs` (verify fixtures + exit-code tests)

**Interfaces:**
- Consumes (Tasks 02/03): `StatusReport`/`build_report`/`render_report`, the Status arm dispatch, `Store` queries module, `output::shard`, `output::artifacts::{TranscriptMetadata, EXPECTED_RAW_SIGNALS_SCHEMA_VERSION}`.
- Produces:
  - `Store::list_succeeded_ids(&self) -> anyhow::Result<Vec<String>>`
  - `status::run_verify(store: &Store, transcripts_root: &Path, counts: &BTreeMap<String, i64>) -> anyhow::Result<VerifyReport>`
  - `StatusReport.verify: Option<VerifyReport>` (`#[serde(skip_serializing_if = "Option::is_none")]`)
  - Exit contract: `status --verify` exits **1** when the pause-safe verdict is false; 0 otherwise. (Task 08's status ADR records this.)

**Design constraints (0017 contract, brainstorm-note performance shape):**
- Artifact-existence check batches: **one `read_dir` per shard** into a filename set, then set lookups — NEVER a per-row `stat` (1M-row Plan C scale would pay 5M stats).
- Schema-version check fully parses every succeeded row's `.json` into `TranscriptMetadata` and compares `raw_signals.schema_version` against `EXPECTED_RAW_SIGNALS_SCHEMA_VERSION`. Full parse is fine at Plan B scale; **sampling is Plan C** — do not build it.
- Pause-safe (0017 + 0011): `pending == 0 AND in_progress == 0 AND artifacts_missing == 0 AND schema_version_mismatches == 0 AND unreadable_artifacts == 0`. The rendering notes that a nonzero `pending` may be deliberate under `--max-videos`.
- The check runs against `--transcripts` (global arg). A wholly absent tree reports every succeeded row missing — that is honest (status run away from the artifacts volume), not an error path.

- [ ] **Step 1: Write the failing tests**

Append to `tests/status.rs`:

```rust
/// Minimal VALID transcript JSON for the schema-version check —
/// TranscriptMetadata's mandatory fields + raw_signals.schema_version.
fn artifact_json(video_id: &str, schema_version: &str) -> String {
    format!(
        r#"{{"video_id":"{video_id}","source_url":"https://example/{video_id}",
"duration_s":1.0,"language_detected":"en","transcribed_at":"2026-07-13T00:00:00Z",
"fetcher":"ytdlp","transcript_source":"whisper-rs","model":"test",
"raw_signals":{{"schema_version":"{schema_version}","language":"en","lang_probs":null,"segments":[]}}}}"#
    )
}

fn write_artifacts(root: &std::path::Path, video_id: &str, txt: bool, json: Option<&str>) {
    // Shard = last two chars of the id (output::shard contract).
    let shard = &video_id[video_id.len() - 2..];
    let dir = root.join(shard);
    std::fs::create_dir_all(&dir).unwrap();
    if txt {
        std::fs::write(dir.join(format!("{video_id}.txt")), "text").unwrap();
    }
    if let Some(ver) = json {
        std::fs::write(dir.join(format!("{video_id}.json")), artifact_json(video_id, ver)).unwrap();
    }
}

#[test]
fn verify_reports_missing_and_mismatched_artifacts_and_exits_1() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = seeded_db(&tmp); // has v_ok1 + v_ok2 succeeded, v_pend pending, v_prog in_progress
    let transcripts = tmp.path().join("transcripts");
    write_artifacts(&transcripts, "v_ok1", true, Some("1")); // complete + valid
    write_artifacts(&transcripts, "v_ok2", true, None); // .json missing

    let out = Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args([
            "--state-db", db.to_str().unwrap(),
            "--transcripts", transcripts.to_str().unwrap(),
            "status", "--verify", "--json",
        ])
        .assert()
        .code(1) // pending + in_progress + missing artifact → NOT pause-safe
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let ver = &v["verify"];
    assert_eq!(ver["succeeded_rows"], 2);
    assert_eq!(ver["artifacts_missing"], 1);
    assert_eq!(ver["schema_version_mismatches"], 0);
    assert_eq!(ver["pause_safe"], false);
    assert_eq!(ver["sample_missing"][0], "v_ok2");
}

#[test]
fn verify_flags_schema_version_mismatch() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = tmp.path().join("state.sqlite");
    {
        let _s = ddp_transcribe::state::Store::open(&db).unwrap();
    }
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "INSERT INTO videos (video_id, source_url, canonical, status, first_seen_at, updated_at)
         VALUES ('v_badver', 'https://example/v_badver', 1, 'succeeded', 100, 100)",
        [],
    )
    .unwrap();
    let transcripts = tmp.path().join("transcripts");
    write_artifacts(&transcripts, "v_badver", true, Some("999"));

    let out = Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args([
            "--state-db", db.to_str().unwrap(),
            "--transcripts", transcripts.to_str().unwrap(),
            "status", "--verify", "--json",
        ])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["verify"]["schema_version_mismatches"], 1);
    assert_eq!(v["verify"]["sample_mismatched"][0], "v_badver");
}

#[test]
fn verify_all_green_is_pause_safe_and_exits_0() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = tmp.path().join("state.sqlite");
    {
        let _s = ddp_transcribe::state::Store::open(&db).unwrap();
    }
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "INSERT INTO videos (video_id, source_url, canonical, status, first_seen_at, updated_at)
         VALUES ('v_ok9', 'https://example/v_ok9', 1, 'succeeded', 100, 100)",
        [],
    )
    .unwrap();
    let transcripts = tmp.path().join("transcripts");
    write_artifacts(&transcripts, "v_ok9", true, Some("1"));

    Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args([
            "--state-db", db.to_str().unwrap(),
            "--transcripts", transcripts.to_str().unwrap(),
            "status", "--verify",
        ])
        .assert()
        .success()
        .stdout(contains("pause-safe: YES"));
}

#[test]
fn verify_conflicts_with_detail_modes() {
    Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["status", "--verify", "--video-id", "x"])
        .assert()
        .code(2); // clap usage error
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --test status -- --test-threads=1`
Expected: new tests fail — `--verify` is an unexpected argument.

- [ ] **Step 3: Add the query + verify engine**

Append to `src/state/queries.rs`, inside the `impl Store` block:

```rust
    /// All succeeded video_ids — the population the 0017 done-contract
    /// checks walk. Plain Vec: the caller groups by shard.
    pub fn list_succeeded_ids(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT video_id FROM videos WHERE status = 'succeeded' ORDER BY video_id")
            .context("prepare list_succeeded_ids")?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .context("query list_succeeded_ids")?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("collect list_succeeded_ids")?;
        Ok(rows)
    }
```

Append to `src/status.rs` (`use std::path::Path;` and `use std::collections::HashSet; use std::ffi::OsString;` join the imports):

```rust
/// The archived ADR-0017 done-contract, mechanised. Sample vectors cap at
/// [`VERIFY_SAMPLE_CAP`] ids so a catastrophically wrong tree doesn't blow
/// up the report; counts are always complete.
pub const VERIFY_SAMPLE_CAP: usize = 20;

#[derive(Debug, Serialize)]
pub struct VerifyReport {
    pub succeeded_rows: usize,
    /// Rows missing `.txt` or `.json` at the sharded path.
    pub artifacts_missing: usize,
    /// Rows whose `.json` parsed but `raw_signals.schema_version` differs
    /// from EXPECTED_RAW_SIGNALS_SCHEMA_VERSION (or raw_signals is absent).
    pub schema_version_mismatches: usize,
    /// Rows whose `.json` exists but could not be read/parsed at all.
    pub unreadable_artifacts: usize,
    pub pending: i64,
    pub in_progress: i64,
    /// 0017 + 0011: everything terminal, all artifacts present and
    /// schema-valid, nothing awaiting recovery → safe to spin down.
    pub pause_safe: bool,
    pub sample_missing: Vec<String>,
    pub sample_mismatched: Vec<String>,
    pub sample_unreadable: Vec<String>,
}

pub fn run_verify(
    store: &Store,
    transcripts_root: &Path,
    counts: &BTreeMap<String, i64>,
) -> Result<VerifyReport> {
    let ids = store.list_succeeded_ids().context("listing succeeded ids")?;

    // Brainstorm-note batching: group ids per shard, ONE read_dir per
    // shard into a filename set, then set lookups. Never per-row stat.
    let mut by_shard: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for id in &ids {
        by_shard.entry(crate::output::shard(id)).or_default().push(id);
    }

    let mut report = VerifyReport {
        succeeded_rows: ids.len(),
        artifacts_missing: 0,
        schema_version_mismatches: 0,
        unreadable_artifacts: 0,
        pending: counts.get("pending").copied().unwrap_or(0),
        in_progress: counts.get("in_progress").copied().unwrap_or(0),
        pause_safe: false,
        sample_missing: Vec::new(),
        sample_mismatched: Vec::new(),
        sample_unreadable: Vec::new(),
    };

    for (shard, shard_ids) in by_shard {
        let dir = transcripts_root.join(shard);
        // Absent shard dir → empty set → every row in it counts missing.
        // Honest when status runs away from the artifacts volume.
        let names: HashSet<OsString> = match std::fs::read_dir(&dir) {
            Ok(entries) => entries
                .filter_map(|e| e.ok().map(|e| e.file_name()))
                .collect(),
            Err(_) => HashSet::new(),
        };
        for id in shard_ids {
            let txt = OsString::from(format!("{id}.txt"));
            let json = OsString::from(format!("{id}.json"));
            if !(names.contains(&txt) && names.contains(&json)) {
                report.artifacts_missing += 1;
                push_capped(&mut report.sample_missing, id);
                continue;
            }
            match std::fs::read(dir.join(format!("{id}.json"))) {
                Ok(bytes) => match serde_json::from_slice::<
                    crate::output::artifacts::TranscriptMetadata,
                >(&bytes)
                {
                    Ok(meta) => {
                        let ok = meta.raw_signals.as_ref().is_some_and(|rs| {
                            rs.schema_version
                                == crate::output::artifacts::EXPECTED_RAW_SIGNALS_SCHEMA_VERSION
                        });
                        if !ok {
                            report.schema_version_mismatches += 1;
                            push_capped(&mut report.sample_mismatched, id);
                        }
                    }
                    Err(_) => {
                        report.unreadable_artifacts += 1;
                        push_capped(&mut report.sample_unreadable, id);
                    }
                },
                Err(_) => {
                    report.unreadable_artifacts += 1;
                    push_capped(&mut report.sample_unreadable, id);
                }
            }
        }
    }

    report.pause_safe = report.pending == 0
        && report.in_progress == 0
        && report.artifacts_missing == 0
        && report.schema_version_mismatches == 0
        && report.unreadable_artifacts == 0;
    Ok(report)
}

fn push_capped(v: &mut Vec<String>, id: &str) {
    if v.len() < VERIFY_SAMPLE_CAP {
        v.push(id.to_string());
    }
}
```

`StatusReport` (Task 02's struct) gains the field:

```rust
    /// Present only under --verify (Task 04): the 0017 done-contract.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verify: Option<VerifyReport>,
```

(`build_report` initializes it `None`; the main arm fills it.) Extend `render_report` — append after the batch-runs section:

```rust
    if let Some(v) = &report.verify {
        let _ = writeln!(out, "done-contract (0017) --verify:");
        let _ = writeln!(out, "  succeeded rows            {:>7}", v.succeeded_rows);
        let _ = writeln!(out, "  artifacts missing         {:>7}", v.artifacts_missing);
        let _ = writeln!(out, "  schema_version mismatches {:>7}", v.schema_version_mismatches);
        let _ = writeln!(out, "  unreadable artifacts      {:>7}", v.unreadable_artifacts);
        let _ = writeln!(
            out,
            "  pending {}  in_progress {}  (pending may be deliberate under --max-videos)",
            v.pending, v.in_progress
        );
        for (label, sample) in [
            ("missing", &v.sample_missing),
            ("mismatched", &v.sample_mismatched),
            ("unreadable", &v.sample_unreadable),
        ] {
            if !sample.is_empty() {
                let _ = writeln!(out, "  first {} {label}: {}", sample.len(), sample.join(", "));
            }
        }
        let _ = writeln!(
            out,
            "  pause-safe: {}",
            if v.pause_safe { "YES — safe to spin down (0011)" } else { "NO" }
        );
    }
```

- [ ] **Step 4: Wire CLI + main**

`src/cli.rs` — add to the Status variant:

```rust
        /// Run the done-contract checks (artifact existence at the sharded
        /// paths + raw_signals.schema_version parse + pause-safe verdict).
        /// Reads the --transcripts tree; exits 1 when not pause-safe.
        #[arg(long, conflicts_with_all = ["video_id", "respondent_id", "errors", "retryable"])]
        verify: bool,
```

`src/main.rs` — destructure `verify` in the Status arm; in the default-report `else` branch:

```rust
            } else {
                let mut report = status::build_report(&store, state::unix_now())?;
                if verify {
                    report.verify = Some(
                        status::run_verify(&store, &cfg.transcripts, &report.counts)
                            .context("running --verify checks")?,
                    );
                }
                if json {
                    println!("{}", serde_json::to_string_pretty(&report)?);
                } else {
                    print!("{}", status::render_report(&report));
                }
                if let Some(v) = &report.verify {
                    if !v.pause_safe {
                        std::process::exit(1);
                    }
                }
            }
```

- [ ] **Step 5: Run the tests**

Run: `cargo test --test status -- --test-threads=1`
Expected: 12/12 pass.

- [ ] **Step 6: Honest-degradation check against the pilot snapshot (v3, read-only)**

```bash
cargo run --quiet -- --state-db ddp-run-export.sqlite --transcripts ./transcripts status --verify; echo "exit: $?"
```

Expected: `succeeded rows 51903`, `artifacts missing 51903` (the artifacts volume is on the VM, not this workstation — honest reporting), `pause-safe: NO`, `exit: 1`. This is the designed away-from-volume behavior; paste the verify block into the task report.

- [ ] **Step 7: Full verification + commit**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Expected: green.

```bash
git add src/state/queries.rs src/status.rs src/cli.rs src/main.rs tests/status.rs
git commit -m "feat(status): --verify — 0017 done-contract (per-shard artifact existence, raw_signals schema_version parse, pause-safe verdict, exit 1 on violation)

Batched per-shard read_dir + set lookup per the Epic 4 brainstorm note
(never per-row stat); schema check is a full parse — sampling stays a
Plan C concern."
```
