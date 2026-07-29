# Task 05: `global = true` rider — all 10 non-global GlobalArgs flags

**Files:**
- Modify: `src/cli.rs` (`GlobalArgs`, lines ~19-88: add `global = true` to 10 field attributes; add a clap `debug_assert` unit test)
- Modify: `tests/cli.rs` (both-position parse test)

**Interfaces:**
- Consumes: `GlobalArgs` as it stands — 11 fields, only `compute_lang_probs` has `global = true`.
- Produces: every GlobalArgs flag accepted on either side of the subcommand. No parsing semantics change for currently-valid invocations (config resolution is byte-identical; the change strictly widens accepted argument orders).

**Semantics (binding):**
- Operator decision 2026-07-29: **all 10** flags (the kickoff's "six" and the followup body's "seven" are both stale counts — docs updated in Task 06). The 10: `profile`, `state_db`, `inbox`, `transcripts`, `log_format`, `whisper_model`, `classification`, `stale_claim_threshold`, `download_workers`, `channel_capacity`.
- Attribute edit only — never reorder fields, never touch defaults/env/value_parser attributes.
- Separate commit from the backfill work (independently revertable; codex-advisor concurrence).
- `Config::from_args` and its `dev_args()` fixture (`src/config.rs:68-82`) construct `GlobalArgs` literally — no field changes here, so they must compile untouched; if they don't, stop and report the deviation.

- [ ] **Step 1: Write the failing tests**

1. In `tests/cli.rs`, append:

```rust
#[test]
fn global_flags_accepted_after_subcommand() {
    // Parse-only checks: anything but clap's usage-error exit (2)
    // proves the flag was accepted in the post-subcommand position
    // (the run itself may then fail for other reasons, e.g. missing
    // DB — that's fine here).
    let cases: &[&[&str]] = &[
        &["status", "--profile", "dev"],
        &["status", "--state-db", "x.sqlite"],
        &["status", "--inbox", "in"],
        &["status", "--transcripts", "out"],
        &["status", "--log-format", "human"],
        &["status", "--whisper-model", "m.bin"],
        &["status", "--classification", "c.toml"],
        &["status", "--stale-claim-threshold", "30m"],
        &["status", "--download-workers", "2"],
        &["status", "--channel-capacity", "2"],
    ];
    for args in cases {
        let assert = assert_cmd::Command::cargo_bin("ddp-transcribe")
            .unwrap()
            .args(*args)
            .assert();
        let code = assert.get_output().status.code();
        assert_ne!(code, Some(2), "clap rejected {args:?}");
    }
}
```

(Adapt the value-enum literals `dev` / `human` to the actual clap value names if they differ — check `ddp-transcribe --help`. Match the file's existing import style: it uses `assert_cmd::Command` + the `.code(2)` convention from `process_retries_rejects_negative_values`.)

2. In `src/cli.rs`, add (or extend) the `#[cfg(test)] mod tests` with clap's self-check — it catches global/local flag-name collisions at test time instead of runtime panic:

```rust
    #[test]
    fn clap_definition_is_internally_consistent() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test --test cli global_flags_accepted_after_subcommand -- --test-threads=1`
Expected: FAIL — every case exits 2 (`unexpected argument`) today. (`clap_definition_is_internally_consistent` passes before AND after; it's a guard, not a proof of change.)

- [ ] **Step 3: Add `global = true` to the 10 fields**

In `src/cli.rs` `GlobalArgs`, extend each field's `#[arg(...)]` attribute — examples of each shape (apply the same pattern to all 10):

```rust
    #[arg(long, value_enum, default_value_t = Profile::Dev, env = "DDP_TRANSCRIBE_PROFILE", global = true)]
    pub profile: Profile,

    #[arg(
        long,
        default_value = "./state.sqlite",
        env = "DDP_TRANSCRIBE_STATE_DB",
        global = true
    )]
    pub state_db: PathBuf,
```

…and likewise for `inbox`, `transcripts`, `log_format`, `whisper_model`, `classification`, `stale_claim_threshold`, `download_workers`, `channel_capacity`. `compute_lang_probs` already has it — leave it alone.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test cli -- --test-threads=1`
Expected: all cli tests pass, including the 10-case both-position test and the existing `config_echo_*` tests (byte-identical resolution for valid invocations).

- [ ] **Step 5: Full verification**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Expected: all green, suite total = previous task's total + 2.

- [ ] **Step 6: Commit**

```bash
git add src/cli.rs tests/cli.rs
git commit -m "fix(cli): global = true on all 10 GlobalArgs flags — accepted on either side of the subcommand (SRC-bake + T11 followup)"
```
