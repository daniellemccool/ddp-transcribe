# Task 19 — CLI + Config: `--download-workers` and `--channel-capacity` flags

**Goal:** Add `--download-workers` (default 3, validate ≥ 1) and `--channel-capacity` (default 2, validate ≥ 1) flags. Plumb to `Config::download_workers` and `Config::channel_capacity`, then into `pipeline::ProcessOptions`. Per AD0027.

**ADRs touched:** AD0027 (defaults).

**Files:**
- Modify: `src/cli.rs` (GlobalArgs gains the two flags)
- Modify: `src/config.rs` (Config fields + defaults + tests)
- Modify: `src/main.rs` (Process arm passes the values into ProcessOptions)
- Modify: `src/pipeline.rs` (ProcessOptions fields — may already exist from T18 if T18 lands first)

**Pre-reqs:** none structurally; recommended order: **T19 before T18**.

---

- [ ] **Step 1: Write the failing tests**

In `src/config.rs::tests`:

```rust
#[test]
fn default_download_workers_is_3_per_ad0027() {
    let cfg = Config::from_args(&dev_args());
    assert_eq!(cfg.download_workers, 3);
}

#[test]
fn default_channel_capacity_is_2_per_ad0027() {
    let cfg = Config::from_args(&dev_args());
    assert_eq!(cfg.channel_capacity, 2);
}

#[test]
fn download_workers_override_flows_through() {
    let mut args = dev_args();
    args.download_workers = Some(5);
    let cfg = Config::from_args(&args);
    assert_eq!(cfg.download_workers, 5);
}

#[test]
fn channel_capacity_override_flows_through() {
    let mut args = dev_args();
    args.channel_capacity = Some(8);
    let cfg = Config::from_args(&args);
    assert_eq!(cfg.channel_capacity, 8);
}
```

Run:

```bash
cargo test --features test-helpers config::tests
```

Expected: FAIL (compile error — fields don't exist).

- [ ] **Step 2: Update `GlobalArgs` in `src/cli.rs`**

Append to `GlobalArgs`:

```rust
#[derive(Parser, Debug, Clone)]
pub struct GlobalArgs {
    // ... existing fields ...

    /// Number of parallel fetch workers in the pipelined orchestrator.
    /// AD0027: default 3 (curve-flattening point on the bake throughput
    /// math; ~3.5× serial wallclock on news_orgs fixture). Must be ≥ 1.
    #[arg(
        long,
        env = "UU_TIKTOK_DOWNLOAD_WORKERS",
        value_parser = clap::value_parser!(usize).range(1..)
    )]
    pub download_workers: Option<usize>,

    /// Bounded mpsc capacity between fetch workers and the transcribe
    /// worker. AD0027: default 2 (small backpressure smoothing for
    /// transcribe's ~1s variance; peak channel memory ~6 × 3 MB = 18 MB
    /// at default N=3 + capacity 2). Must be ≥ 1.
    #[arg(
        long,
        env = "UU_TIKTOK_CHANNEL_CAPACITY",
        value_parser = clap::value_parser!(usize).range(1..)
    )]
    pub channel_capacity: Option<usize>,
}
```

`clap::value_parser!(usize).range(1..)` enforces ≥ 1 at parse time; clap rejects 0 with a clear error.

Update `dev_args()` in `src/config.rs::tests` to include the new fields:

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
        download_workers: None,
        channel_capacity: None,
    }
}
```

- [ ] **Step 3: Update `Config`**

Already covered in T18 Step 1 (when removing dead config fields). If T19 lands first, the fields look like:

```rust
#[derive(Debug, Clone)]
pub struct Config {
    // ... existing fields (including, until T18, whisper_use_gpu/whisper_threads) ...
    pub stale_claim_threshold: Duration,
    /// AD0027: default 3 (curve-flattening point).
    pub download_workers: usize,
    /// AD0027: default 2 (small backpressure smoothing).
    pub channel_capacity: usize,
}
```

`from_args`:

```rust
download_workers: args.download_workers.unwrap_or(3),
channel_capacity: args.channel_capacity.unwrap_or(2),
```

- [ ] **Step 4: Plumb into `ProcessOptions`**

In `src/pipeline.rs::ProcessOptions`:

```rust
pub struct ProcessOptions {
    pub worker_id: String,
    pub transcripts_root: PathBuf,
    pub max_videos: Option<usize>,
    pub compute_lang_probs: bool,
    pub transcribe_timeout: Duration,
    pub stale_claim_threshold: Duration,
    /// AD0027: default 3; flag-tunable via --download-workers.
    pub download_workers: usize,
    /// AD0027: default 2; flag-tunable via --channel-capacity.
    pub channel_capacity: usize,
}
```

In `src/main.rs::Process` arm:

```rust
let opts = pipeline::ProcessOptions {
    // ... existing fields ...
    download_workers: cfg.download_workers,
    channel_capacity: cfg.channel_capacity,
};
```

- [ ] **Step 5: Run the tests**

```bash
cargo test --features test-helpers
```

Expected: all green — config tests pass; pipeline tests still pass (they default to N=1 / capacity=1 since the orchestrator wiring isn't exercised yet by T19's tests).

- [ ] **Step 6: Smoke-test CLI parsing**

```bash
cargo run -- --download-workers 5 --channel-capacity 4 process --help
```

Expected: clap accepts both; help screen renders.

```bash
cargo run -- --download-workers 0 process --help
```

Expected: clap rejects with `error: invalid value '0' for '--download-workers <DOWNLOAD_WORKERS>': value out of range`.

- [ ] **Step 7: cargo fmt + clippy**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
```

Expected: clean.

- [ ] **Step 8: Commit**

```bash
git add src/cli.rs src/config.rs src/main.rs src/pipeline.rs
git commit -m "$(cat <<'EOF'
feat(cli,config): --download-workers + --channel-capacity flags with AD0027 defaults

CLI/Config/ProcessOptions plumbing for the pipelined orchestrator's
two tunable knobs:

- --download-workers: number of parallel fetch workers. Default 3
  (AD0027: curve-flattening point on news_orgs bake; one stuck fetch
  drops effective capacity by only a third). Must be ≥ 1, enforced by
  clap's value_parser range.

- --channel-capacity: bounded mpsc capacity between fetch workers and
  the transcribe worker. Default 2 (AD0027: small backpressure smoothing
  for transcribe's ~1s variance; peak channel memory ~18 MB at defaults).
  Must be ≥ 1.

Both flags read env vars (UU_TIKTOK_DOWNLOAD_WORKERS,
UU_TIKTOK_CHANNEL_CAPACITY) via clap env support.

Tests: default values match AD0027; CLI override flows through to
Config and ProcessOptions.

Refs: AD0027

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-check

- [ ] `cargo test --features test-helpers config::tests` passes
- [ ] `cargo run -- --download-workers 5 process --help` accepted
- [ ] `cargo run -- --download-workers 0 process --help` rejected at parse time
- [ ] Defaults are 3 and 2 respectively
- [ ] Clippy/fmt clean
