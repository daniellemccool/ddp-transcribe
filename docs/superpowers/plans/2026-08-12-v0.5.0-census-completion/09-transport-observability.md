# Task 09 — Transport observability in `params_json`

**Files:**
- Modify: `src/fetcher/ytdlp.rs` (env-echo helper + parser),
  `src/commands.rs:155-163` (params_json fields)
- Test: `src/fetcher/ytdlp.rs` unit tests (parser), `tests/batch_census.rs`
  (params keys, if the existing lifecycle test asserts params content —
  extend it; otherwise leave integration to the VM validation batch)

**Interfaces:**
- Consumes: `process::run` / `CommandSpec` (the ADR-0021 bounded runner —
  the same one `build_metadata_only_args`' caller uses, `src/backfill.rs:82-89`
  shows the shape).
- Produces: `params_json` additionally carries `fetch_url_form`
  (`"canonical-v1"`), `ytdlp_version` (string or null), and
  `ytdlp_impersonation_available` (bool or null). Incident-2's "three
  invisible states" become answerable from `batch_runs` alone.

**Report (ADR-0019):** ≤250 words, STATUS / SUMMARY / CHANGED FILES / DEVIATIONS.

Ground truth: `params_json` is built in the Process arm at
`src/commands.rs:155-163` (a `serde_json::json!` block — retries,
max_videos, cookies_present, download_workers, worker_host,
checkpoint_cmd, checkpoint_every_secs; Task 08 added breaker_threshold).
On the VM, `yt-dlp --list-impersonate-targets` prints every target with
`(unavailable)` when curl_cffi is absent (verified 2026-08-11 — that
output is the only positive witness impersonation is off).

- [ ] **Step 1: Write the failing parser tests**

In `src/fetcher/ytdlp.rs`'s `#[cfg(test)]` module:

```rust
#[test]
fn impersonation_availability_parses_all_unavailable_as_false() {
    let listing = "\
chrome-136        (unavailable)
chrome-133        (unavailable)
safari-18         (unavailable)";
    assert_eq!(impersonation_available_from_listing(listing), Some(false));
}

#[test]
fn impersonation_availability_parses_any_available_as_true() {
    let listing = "\
chrome-136
chrome-133        (unavailable)";
    assert_eq!(impersonation_available_from_listing(listing), Some(true));
}

#[test]
fn impersonation_availability_is_none_on_empty_output() {
    assert_eq!(impersonation_available_from_listing(""), None);
    assert_eq!(impersonation_available_from_listing("  \n"), None);
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --features test-helpers --lib ytdlp -- --test-threads=1 impersonation_availability`
Expected: compile failure — the function is the deliverable.

- [ ] **Step 3: Implement**

`src/fetcher/ytdlp.rs`:

```rust
/// Parse `yt-dlp --list-impersonate-targets` output. Some(true) = at
/// least one target line lacks "(unavailable)"; Some(false) = targets
/// listed, all unavailable; None = nothing parseable (echo stays honest:
/// unknown is unknown, per the transport-observability decision).
pub(crate) fn impersonation_available_from_listing(stdout: &str) -> Option<bool> {
    let lines: Vec<&str> = stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return None;
    }
    Some(lines.iter().any(|l| !l.contains("(unavailable)")))
}

/// Startup environment echo (spec D5): what yt-dlp will the fetch workers
/// actually run, and can it impersonate? Best-effort — every failure path
/// is a warn + None, never fatal (same posture as ADR-0044 hooks).
pub(crate) async fn ytdlp_env_echo(timeout: std::time::Duration) -> (Option<String>, Option<bool>) {
    let version = match run(CommandSpec {
        program: "yt-dlp".to_string(),
        args: vec!["--version".into()],
        timeout,
        stderr_capture_bytes: 8 * 1024,
        stdout_capture_bytes: 8 * 1024,
        redact_arg_indices: &[],
    })
    .await
    {
        Ok(o) if o.exit_code == 0 => o
            .stdout
            .as_deref()
            .and_then(|s| s.lines().next())
            .map(|s| s.trim().to_string()),
        Ok(o) => {
            tracing::warn!(exit_code = o.exit_code, "yt-dlp --version failed; env echo incomplete");
            None
        }
        Err(e) => {
            tracing::warn!(error = %e, "yt-dlp --version did not run; env echo incomplete");
            None
        }
    };
    let impersonation = match run(CommandSpec {
        program: "yt-dlp".to_string(),
        args: vec!["--list-impersonate-targets".into()],
        timeout,
        stderr_capture_bytes: 8 * 1024,
        stdout_capture_bytes: 64 * 1024,
        redact_arg_indices: &[],
    })
    .await
    {
        Ok(o) if o.exit_code == 0 => {
            o.stdout.as_deref().and_then(impersonation_available_from_listing)
        }
        Ok(_) | Err(_) => None,
    };
    (version, impersonation)
}
```
(Match the actual `CommandSpec`/`run` import paths and field set used at
`YtDlpFetcher::acquire` — same module; adapt field names to what compiles,
the runner contract is ADR-0021's.)

`src/commands.rs`, before the `params_json` block (:155):
```rust
    let (ytdlp_version, ytdlp_impersonation_available) =
        crate::fetcher::ytdlp::ytdlp_env_echo(std::time::Duration::from_secs(10)).await;
```
and in the `json!`:
```rust
    "fetch_url_form": "canonical-v1",
    "ytdlp_version": ytdlp_version,
    "ytdlp_impersonation_available": ytdlp_impersonation_available,
```

- [ ] **Step 4: Run the suites**

Run: `cargo test --features test-helpers -- --test-threads=1 ytdlp && cargo test --features test-helpers --test batch_census -- --test-threads=1`
Expected: PASS. If `tests/batch_census.rs::batch_lifecycle_persists_provenance_chain:26`
asserts `params_json` content, extend its assertions to include
`"fetch_url_form":"canonical-v1"`; if it only asserts presence, leave it.

- [ ] **Step 5: Full gate and commit**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Commit: `git commit -am "feat(commands): params_json carries fetch-url form and yt-dlp env echo"`

This closes **Phase 3** — controller writes `PHASE-3-CLOSE.md` and ends its
session per ADR-0019.
