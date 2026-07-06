# Task 08: Cookie plumbing — `FetchOpts`, `--cookies-file`, kind-gated routing, redaction

**Files:**
- Modify: `src/fetcher/mod.rs` (trait signature + `FetchOpts`; FakeFetcher records opts)
- Modify: `src/fetcher/ytdlp.rs` (args + redaction + stderr scrub)
- Modify: `src/cli.rs` (`Process` gains `--cookies-file`)
- Modify: `src/main.rs` (thread flag into `ProcessOptions`)
- Modify: `src/pipeline/mod.rs` (`ProcessOptions.cookies_file`; `fetch_and_decode` passes opts)
- Modify: `src/pipeline/pipelined.rs` + `src/pipeline/serial.rs` (per-claim cookie decision)
- Test: `tests/pipeline_fakes/fetch_worker_tests.rs`, `src/fetcher/ytdlp.rs` unit tests

**Interfaces:**
- Consumes: Task 04's `Claim.last_retryable_kind`; Task 03's `RetryableKind::SensitiveLoginGated.tag()`; `CommandSpec.redact_arg_indices` (exists in `src/process.rs`).
- Produces:
  - `#[derive(Debug, Clone, Default)] pub struct FetchOpts { pub cookies_file: Option<PathBuf> }` in `src/fetcher/mod.rs`
  - Trait change: `async fn acquire(&self, video_id: &str, source_url: &str, opts: &FetchOpts) -> Result<Acquisition, FetchError>`
  - `ProcessOptions.cookies_file: Option<PathBuf>`
  - Policy fn the workers share: `pub(crate) fn cookie_opts_for(claim: &Claim, cookies_file: Option<&Path>) -> FetchOpts` — cookies **iff** `claim.last_retryable_kind.as_deref() == Some("SensitiveLoginGated")` and the flag is set. First attempts (`None` kind) never get cookies (ADR 0035).

- [ ] **Step 1: Write the failing tests**

`src/fetcher/ytdlp.rs` unit tests (pure-function level — extend `build_yt_dlp_args` to take `Option<&Path>` cookies):

```rust
#[test]
fn build_args_appends_cookies_and_reports_redact_index() {
    let video_dir = PathBuf::from("/tmp/test-dir");
    let cookie = PathBuf::from("/secret/tiktok-cookies.txt");
    let (args, _, redact) =
        build_yt_dlp_args("abc123", "https://example.com/v", &video_dir, Some(&cookie));
    let ci = args.iter().position(|a| a == "--cookies").expect("--cookies present");
    assert_eq!(args.get(ci + 1).map(String::as_str), Some("/secret/tiktok-cookies.txt"));
    assert_eq!(redact, vec![ci + 1], "cookie path arg index must be redacted in logs");
}

#[test]
fn build_args_without_cookies_is_unchanged() {
    let video_dir = PathBuf::from("/tmp/test-dir");
    let (args, _, redact) = build_yt_dlp_args("abc123", "https://example.com/v", &video_dir, None);
    assert!(!args.iter().any(|a| a == "--cookies"));
    assert!(redact.is_empty());
}
```

`src/pipeline/mod.rs` or `pipelined.rs` unit test for the policy fn:

```rust
#[test]
fn cookies_only_for_sensitive_login_gated_retries() {
    let cookie = PathBuf::from("/secret/c.txt");
    let mk = |kind: Option<&str>| Claim {
        video_id: "7".into(),
        source_url: "u".into(),
        attempt_count: 1,
        last_retryable_kind: kind.map(String::from),
    };
    assert_eq!(cookie_opts_for(&mk(None), Some(&cookie)).cookies_file, None);
    assert_eq!(cookie_opts_for(&mk(Some("NoDataBlocks")), Some(&cookie)).cookies_file, None);
    assert_eq!(
        cookie_opts_for(&mk(Some("SensitiveLoginGated")), Some(&cookie)).cookies_file,
        Some(cookie.clone())
    );
    assert_eq!(cookie_opts_for(&mk(Some("SensitiveLoginGated")), None).cookies_file, None);
}
```

Integration test in `tests/pipeline_fakes/fetch_worker_tests.rs`: give `FakeFetcher` a `pub received_opts: std::sync::Mutex<Vec<FetchOpts>>` recorder (pushed at the top of `acquire`), seed a row whose `last_retryable_kind` is `SensitiveLoginGated` (seed → fail with the sensitive fixture message → requeue via raw SQL or Task 05's `requeue_retryable` → re-claim path through the worker), run the worker with `cookies_file` set in `ProcessOptions`, assert the recorded opts carry the path.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --features test-helpers -- --test-threads=1`
Expected: compile errors (signature/tuple arity changes are compiler-driven).

- [ ] **Step 3: Implement**

`src/fetcher/mod.rs`:

```rust
/// Per-request fetch options (Epic 3). Cookie scope is policy: ADR 0035
/// pins cookies to SensitiveLoginGated retries only; this struct just
/// carries the decision to the tool adapter.
#[derive(Debug, Clone, Default)]
pub struct FetchOpts {
    pub cookies_file: Option<std::path::PathBuf>,
}

#[async_trait]
pub trait VideoFetcher: Send + Sync {
    async fn acquire(
        &self,
        video_id: &str,
        source_url: &str,
        opts: &FetchOpts,
    ) -> Result<Acquisition, FetchError>;
    fn name(&self) -> &'static str;
}
```

`build_yt_dlp_args` gains `cookies: Option<&Path>` and returns `(Vec<String>, PathBuf, Vec<usize>)`; when `Some`, push `"--cookies".into()` then the path string **before** the final `source_url` positional, recording the path's index. `acquire` threads `opts.cookies_file.as_deref()` in and passes the indices:

```rust
let (args, wav_path, redact) =
    build_yt_dlp_args(video_id, source_url, &video_dir, opts.cookies_file.as_deref());
let outcome = run(CommandSpec {
    program: "yt-dlp",
    args,
    timeout: self.timeout,
    stderr_capture_bytes: 8 * 1024,
    stdout_capture_bytes: 0,
    redact_arg_indices: &redact,
})
.await?;
```

Stderr scrub (cookie path must not reach error messages or the state DB):

```rust
if outcome.exit_code != 0 {
    let mut excerpt = outcome.stderr_excerpt;
    if let Some(cookie) = opts.cookies_file.as_deref() {
        excerpt = excerpt.replace(&cookie.display().to_string(), "[COOKIES-REDACTED]");
    }
    return Err(FetchError::ToolFailed { tool: "yt-dlp", exit_code: outcome.exit_code, signal: outcome.signal, stderr_excerpt: excerpt });
}
```

`src/cli.rs`:

```rust
Process {
    #[arg(long)]
    max_videos: Option<usize>,
    /// Netscape-format cookie file passed to yt-dlp ONLY on retries of
    /// sensitive/login-gated videos (ADR 0035). Never sent on first attempts.
    #[arg(long, env = "DDP_TRANSCRIBE_COOKIES_FILE")]
    cookies_file: Option<PathBuf>,
},
```

`main.rs` Process arm: destructure the new field, set `ProcessOptions { cookies_file, .. }`. `ProcessOptions` gains `pub cookies_file: Option<PathBuf>`.

Policy fn (in `src/pipeline/mod.rs`, next to `fetch_and_decode`):

```rust
pub(crate) fn cookie_opts_for(claim: &Claim, cookies_file: Option<&Path>) -> FetchOpts {
    let sensitive = claim.last_retryable_kind.as_deref()
        == Some(crate::failure::RetryableKind::SensitiveLoginGated.tag());
    FetchOpts {
        cookies_file: if sensitive { cookies_file.map(Path::to_path_buf) } else { None },
    }
}
```

`fetch_and_decode` gains `opts: &FetchOpts` and passes it to `fetcher.acquire(…)`. Both workers + `process_one` compute `cookie_opts_for(&claim, opts.cookies_file.as_deref())` at the call site. Update `FakeFetcher::acquire` for the new signature + recorder; `FetchedItem`/other structs unchanged.

Tracing hygiene: when cookies are attached, log only `cookies = true` — never the path.

- [ ] **Step 4: Run tests to verify pass**

Run: full verification command.
Expected: PASS; `grep -rn "cookies_file" src/ | grep -i "tracing\|info!\|warn!\|error!"` shows no path interpolation.

- [ ] **Step 5: Commit**

```bash
git add src/ tests/
git commit -m "feat(fetch): cookie-scoped retry for SensitiveLoginGated claims with argv+stderr redaction (ADR 0035)"
```
