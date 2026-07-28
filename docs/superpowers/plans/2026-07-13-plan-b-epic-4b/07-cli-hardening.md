# Task 07: CLI hardening — `--retries` range validation + config echo scoped to consumed config

**Files:**
- Modify: `src/cli.rs` (`--retries` ranged value parser)
- Modify: `src/main.rs` (per-subcommand config echo)
- Modify: `tests/cli.rs` (new tests)

**Interfaces:**
- Consumes (Tasks 02–06): the full `Command` enum incl. `Status` and `RecomputeWindow`.
- Produces: nothing downstream — this closes two epic-4 FOLLOWUPS entries (`--retries` i64 validation; config-echo scoping). Task 08 archives them.

**Background (from `docs/followups/epic-4.md`):**
1. `process --retries` is a bare `i64`; the cap is computed as `retries + 1`. `--retries -1` → cap 0 → every failure exhausts silently with no retry; `--retries 9223372036854775807` → `retries + 1` panics (debug) / wraps to `i64::MIN` (release). A parse-time range bound closes both. Mirror the existing `RangedU64ValueParser` pattern (`download_workers` / `channel_capacity`) with the i64 sibling.
2. The startup `"config resolved"` echo logs `whisper_model_path` for every subcommand including ones that never load a model (`init`, `ingest`, `migrate`) — on 2026-07-07 this sent the operator chasing a "why is it using tiny?" false alarm. Scope the echo to what the invoked subcommand actually consumes.

- [ ] **Step 1: Write the failing tests**

Append to `tests/cli.rs`:

```rust
#[test]
fn process_retries_rejects_negative_values() {
    Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["process", "--retries=-1"]) // = form: a bare "-1" token reads as a flag to clap
        .assert()
        .code(2); // clap range violation, not a silent zero-budget run
}

#[test]
fn process_retries_rejects_i64_max() {
    Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["process", "--retries", "9223372036854775807"])
        .assert()
        .code(2); // would overflow at retries + 1
}

#[test]
fn process_retries_accepts_bounds() {
    // Parse-only check: valid values get PAST argument parsing and fail
    // later on the missing model file, not with a usage error (exit != 2).
    for v in ["0", "1000000"] {
        let assert = Command::cargo_bin("ddp-transcribe")
            .unwrap()
            .args(["--state-db", "/nonexistent/x.sqlite", "process", "--retries", v])
            .assert()
            .failure();
        assert_ne!(assert.get_output().status.code(), Some(2), "--retries {v} must parse");
    }
}

#[test]
fn config_echo_omits_model_path_for_non_model_subcommands() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = tmp.path().join("state.sqlite");
    let out = Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["--state-db", db.to_str().unwrap(), "init"])
        .assert()
        .success()
        .get_output()
        .clone();
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !all.contains("whisper_model_path"),
        "init never loads a model; echoing the model path caused a production false alarm (epic-4 followup)"
    );
}

#[test]
fn config_echo_includes_model_path_for_process() {
    // process DOES consume the model — the echo must still advertise it.
    // The run fails later (missing model file); the echo happens first.
    let tmp = tempfile::TempDir::new().unwrap();
    let db = tmp.path().join("state.sqlite");
    Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["--state-db", db.to_str().unwrap(), "init"])
        .assert()
        .success();
    let out = Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["--state-db", db.to_str().unwrap(), "process", "--max-videos", "0"])
        .assert()
        .failure()
        .get_output()
        .clone();
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(all.contains("whisper_model_path"));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --test cli -- --test-threads=1`
Expected: the two rejection tests fail (negative and i64::MAX currently PARSE fine); the echo-omission test fails (echo currently unconditional).

- [ ] **Step 3: Ranged parser**

In `src/cli.rs`, the `retries` arg on `Process` becomes:

```rust
        /// Automatic in-batch retry budget per video (lifetime attempts =
        /// retries + 1). Default 1. Range-bounded at parse time: negative
        /// values would silently zero the budget and i64::MAX would
        /// overflow at `retries + 1` (epic-4 followup).
        #[arg(
            long,
            default_value_t = 1,
            value_parser = clap::builder::RangedI64ValueParser::<i64>::new().range(0..=1_000_000)
        )]
        retries: i64,
```

- [ ] **Step 4: Scoped config echo**

In `src/main.rs`, replace the unconditional `tracing::info!(profile = ?cfg.profile, state_db = ?cfg.state_db, whisper_model_path = ?cfg.whisper_model_path, "config resolved");` with a call to a new helper placed next to `hostname_or_default`:

```rust
/// Config echo scoped to what the invoked subcommand actually consumes
/// (epic-4 followup: echoing whisper_model_path for `ingest` sent the
/// operator chasing a "why is it using tiny?" false alarm). Process is
/// the only model-loading arm; ingest reads the inbox; status --verify
/// reads the transcripts tree.
fn log_resolved_config(cfg: &config::Config, command: &cli::Command) {
    match command {
        cli::Command::Process { .. } => tracing::info!(
            profile = ?cfg.profile,
            state_db = ?cfg.state_db,
            transcripts = ?cfg.transcripts,
            whisper_model_path = ?cfg.whisper_model_path,
            classification = ?cfg.classification_path,
            "config resolved"
        ),
        cli::Command::Ingest { .. } => tracing::info!(
            profile = ?cfg.profile,
            state_db = ?cfg.state_db,
            inbox = ?cfg.inbox,
            "config resolved"
        ),
        cli::Command::Status { verify: true, .. } => tracing::info!(
            profile = ?cfg.profile,
            state_db = ?cfg.state_db,
            transcripts = ?cfg.transcripts,
            "config resolved"
        ),
        cli::Command::Init
        | cli::Command::Migrate
        | cli::Command::Status { .. }
        | cli::Command::RecomputeWindow { .. } => tracing::info!(
            profile = ?cfg.profile,
            state_db = ?cfg.state_db,
            "config resolved"
        ),
    }
}
```

Call site in `main`:

```rust
    let cfg = config::Config::from_args(&cli.global);
    log_resolved_config(&cfg, &cli.command);
```

(`cli.command` is consumed by the `match` below — pass `&cli.command` BEFORE the `match cli.command` line; no clone needed.)

- [ ] **Step 5: Run the tests**

Run: `cargo test --test cli -- --test-threads=1`
Expected: all pass, including the five new ones.

- [ ] **Step 6: Full verification + commit**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Expected: green.

```bash
git add src/cli.rs src/main.rs tests/cli.rs
git commit -m "fix(cli): bound --retries to 0..=1_000_000 at parse time; scope config echo to consumed config

Closes both epic-4 CLI-hardening followups: negative retries silently
zeroed the budget and i64::MAX overflowed at retries+1; the
whisper_model_path echo on non-model subcommands caused a production
false alarm (2026-07-07)."
```
