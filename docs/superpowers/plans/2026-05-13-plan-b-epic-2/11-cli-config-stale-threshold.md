# Task 11 — CLI + Config: `--stale-claim-threshold` flag with 30-min default

**Goal:** Add `--stale-claim-threshold` to the CLI (parsed via `humantime`-style "30m"/"1h"/"45s"), a corresponding `Config::stale_claim_threshold: Duration` field with 30-min default, and plumb it through to `pipeline::ProcessOptions::stale_claim_threshold`. Per 0024.

**ADRs touched:** 0024 (30-min default).

**Files:**
- Modify: `Cargo.toml` (add `humantime = "2"` to deps)
- Modify: `src/cli.rs` (GlobalArgs gets the flag)
- Modify: `src/config.rs` (Config field + tests)
- Modify: `src/main.rs` (Process arm passes the value into `ProcessOptions`)
- Modify: `src/pipeline.rs` (ProcessOptions field — may already be added by T9 if T9 lands first)

**Pre-reqs:** none structurally, but recommend landing **before T9** so T9's wiring uses `opts.stale_claim_threshold` cleanly.

---

- [ ] **Step 1: Add `humantime` dependency**

In `Cargo.toml`:

```toml
[dependencies]
# ... existing ...
humantime = "2"
```

Quick build check:

```bash
cargo build
```

Expected: clean build (humantime is a small crate).

- [ ] **Step 2: Write the failing tests**

Append to `src/config.rs::tests`:

```rust
#[test]
fn default_stale_claim_threshold_is_30_min() {
    let cfg = Config::from_args(&dev_args());
    assert_eq!(cfg.stale_claim_threshold, Duration::from_secs(30 * 60));
}

#[test]
fn stale_claim_threshold_parsed_from_args() {
    let mut args = dev_args();
    args.stale_claim_threshold = Some(humantime::Duration::from_str("5m").unwrap().into());
    let cfg = Config::from_args(&args);
    assert_eq!(cfg.stale_claim_threshold, Duration::from_secs(5 * 60));
}
```

Run:

```bash
cargo test config::tests
```

Expected: FAIL (compile error — fields/imports don't exist yet).

- [ ] **Step 3: Update `GlobalArgs` in `src/cli.rs`**

Append a flag to `GlobalArgs`:

```rust
#[derive(Parser, Debug, Clone)]
pub struct GlobalArgs {
    // ... existing fields ...

    /// Threshold for sweeping stale (process-crashed) claims back to pending.
    /// Accepts humantime strings: "30m" (default), "1h", "45s".
    /// 0024: 30-min default is well above bake worst-case (~25s).
    #[arg(
        long,
        env = "UU_TIKTOK_STALE_CLAIM_THRESHOLD",
        value_parser = humantime::parse_duration
    )]
    pub stale_claim_threshold: Option<std::time::Duration>,
}
```

`value_parser = humantime::parse_duration` returns `Result<std::time::Duration, _>` directly, so the field type is `Option<Duration>`. The default value lives in `Config::from_args` (so `None` from CLI becomes 30 min in the resolved Config).

Update the `dev_args()` helper in `src/config.rs::tests` to include the new field:

```rust
fn dev_args() -> GlobalArgs {
    GlobalArgs {
        profile: Profile::Dev,
        state_db: PathBuf::from("/tmp/test.sqlite"),
        inbox: PathBuf::from("/tmp/in"),
        transcripts: PathBuf::from("/tmp/out"),
        log_format: crate::cli::LogFormat::Human,
        whisper_model: None,
        compute_lang_probs: false,
        stale_claim_threshold: None,
    }
}
```

- [ ] **Step 4: Update `Config` in `src/config.rs`**

```rust
#[derive(Debug, Clone)]
pub struct Config {
    pub profile: Profile,
    pub state_db: PathBuf,
    pub inbox: PathBuf,
    pub transcripts: PathBuf,
    pub whisper_model_path: PathBuf,
    // (whisper_use_gpu / whisper_threads stay — T18 removes them in Phase 2.)
    #[allow(dead_code)]
    pub whisper_use_gpu: bool,
    #[allow(dead_code)]
    pub whisper_threads: usize,
    pub ytdlp_timeout: Duration,
    pub transcribe_timeout: Duration,
    pub compute_lang_probs: bool,
    /// Stale-claim sweep threshold (0024). Default 30 min if CLI/env
    /// did not supply a value.
    pub stale_claim_threshold: Duration,
}

impl Config {
    pub fn from_args(args: &GlobalArgs) -> Self {
        match args.profile {
            Profile::Dev => Self {
                profile: Profile::Dev,
                state_db: args.state_db.clone(),
                inbox: args.inbox.clone(),
                transcripts: args.transcripts.clone(),
                whisper_model_path: args
                    .whisper_model
                    .clone()
                    .unwrap_or_else(|| PathBuf::from("./models/ggml-tiny.en.bin")),
                whisper_use_gpu: false,
                whisper_threads: num_cpus_safe(),
                ytdlp_timeout: Duration::from_secs(300),
                transcribe_timeout: Duration::from_secs(600),
                compute_lang_probs: args.compute_lang_probs,
                stale_claim_threshold: args
                    .stale_claim_threshold
                    .unwrap_or_else(|| Duration::from_secs(30 * 60)),
            },
        }
    }
}
```

- [ ] **Step 5: Plumb into `ProcessOptions` in `src/main.rs`**

In the `Process` dispatch arm in `src/main.rs`, update the `ProcessOptions` literal:

```rust
let opts = pipeline::ProcessOptions {
    worker_id: format!("{}-{}", hostname_or_default(), std::process::id()),
    transcripts_root: cfg.transcripts.clone(),
    max_videos,
    compute_lang_probs: cfg.compute_lang_probs,
    transcribe_timeout: cfg.transcribe_timeout,
    stale_claim_threshold: cfg.stale_claim_threshold,
};
```

The `ProcessOptions::stale_claim_threshold` field may already exist if T9 landed first; otherwise add it now per the snippet in T9 Step 1.

- [ ] **Step 6: Run the tests**

```bash
cargo test --features test-helpers
```

Expected: all green — `config::tests` includes the two new assertions; pipeline tests pass.

- [ ] **Step 7: Smoke test the CLI parse**

```bash
cargo run -- --stale-claim-threshold 5m process --help
```

Expected: clap parses `5m`; help screen renders.

```bash
cargo run -- --stale-claim-threshold notatime process --help
```

Expected: clap rejects with an error mentioning the parse failure.

- [ ] **Step 8: cargo fmt + clippy**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
```

Expected: clean.

- [ ] **Step 9: Commit**

```bash
git add Cargo.toml Cargo.lock src/cli.rs src/config.rs src/main.rs src/pipeline.rs
git commit -m "$(cat <<'EOF'
feat(cli,config): --stale-claim-threshold flag (humantime, 30-min default) per 0024

Plumbs the stale-claim sweep threshold from CLI → Config → ProcessOptions →
run_serial. Accepts humantime strings ("30m", "1h", "45s"). Env var
UU_TIKTOK_STALE_CLAIM_THRESHOLD also reads through clap's env support.

Default is 30 minutes per 0024 — well above bake worst-case (~25s
end-to-end per video on large-v3-turbo-q5_0) and prevents stealing
from healthy peers in any future multi-instance scenario.

Adds humantime = "2" to Cargo.toml; the dep is small and the convention
matches the rest of the project's "operator-readable" CLI ergonomics.

Tests: default is 30 min; CLI override flows through to Config.

Refs: 0024

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-check

- [ ] `cargo test --features test-helpers` all green (config tests + pipeline tests)
- [ ] `cargo run -- --stale-claim-threshold 5m process --help` succeeds
- [ ] Default is 30 min when flag is absent
- [ ] `UU_TIKTOK_STALE_CLAIM_THRESHOLD` env var also works (clap env support)
- [ ] Clippy/fmt clean
- [ ] Phase 1 task list is now complete — controller writes `PHASE-1-CLOSE.md` per 0019 before starting Phase 2
